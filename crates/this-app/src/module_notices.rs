//! # Notices published by modules
//!
//! A module publishes a notice (info, warning, error) with a duration and, optionally, an
//! action. The host keeps a board of them:
//!
//! 1. **one key, one notice**: the key is scoped by module (`module` + `key`), so two
//!    modules never overwrite each other; publishing the same key again keeps a single
//!    notice, the most recent one (by the caller's clock, ties go to the later call);
//! 2. **expiry by the caller's clock**: a notice with a duration disappears at
//!    `published_at_ms + ttl_ms` (exactly then); a notice without a duration stays until
//!    it is dismissed or its module is withdrawn;
//! 3. **order**: errors first, then warnings, then infos; inside a level, the most recent
//!    first; ties broken by module then key, so the order is total.
//! 4. **bound**: at most `max` notices; when full, the least important and oldest gives way.
//!
//! [`Board::visible`] filters the expired ones at `now_ms` even before [`Board::expire`] is
//! called, so a late sweep never shows a stale notice.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::device::{Notice, Tone};

/// How serious a notice, or a lifecycle state, is.
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum Level {
    #[default]
    Info,
    Warning,
    Error,
}

impl Level {
    /// The device tone that carries this level.
    pub fn tone(self) -> Tone {
        match self {
            Level::Info => Tone::Info,
            Level::Warning => Tone::Warn,
            Level::Error => Tone::Danger,
        }
    }

    /// The label for the person.
    pub fn label(self) -> &'static str {
        match self {
            Level::Info => "Information",
            Level::Warning => "Avertissement",
            Level::Error => "Erreur",
        }
    }
}

/// What the person can do from the notice: a label, and the name of an action of the
/// module (the host routes it, it executes nothing on its own).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct NoticeAction {
    pub label: String,
    pub action: String,
}

/// A notice as a module publishes it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModuleNotice {
    pub module: String,
    /// Grouping key inside the module.
    pub key: String,
    #[serde(default)]
    pub level: Level,
    pub title: String,
    #[serde(default)]
    pub body: String,
    /// Display duration; `None` keeps the notice until it is dismissed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ttl_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub action: Option<NoticeAction>,
}

impl ModuleNotice {
    pub fn new(
        module: impl Into<String>,
        key: impl Into<String>,
        level: Level,
        title: impl Into<String>,
    ) -> Self {
        Self {
            module: module.into(),
            key: key.into(),
            level,
            title: title.into(),
            body: String::new(),
            ttl_ms: None,
            action: None,
        }
    }

    pub fn with_body(mut self, body: impl Into<String>) -> Self {
        self.body = body.into();
        self
    }

    pub fn for_ms(mut self, ttl_ms: u64) -> Self {
        self.ttl_ms = Some(ttl_ms);
        self
    }

    pub fn with_action(mut self, label: impl Into<String>, action: impl Into<String>) -> Self {
        self.action = Some(NoticeAction {
            label: label.into(),
            action: action.into(),
        });
        self
    }

    /// The device notice (system notification or toast), keyed `module/key`.
    pub fn to_device(&self) -> Notice {
        Notice::new(
            format!("{}/{}", self.module, self.key),
            self.title.as_str(),
            self.body.as_str(),
        )
        .with_tone(self.level.tone())
    }
}

/// Why a notice was refused.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum NoticeError {
    EmptyModule,
    EmptyKey,
    EmptyTitle,
    /// A zero duration would never be seen.
    ZeroDuration,
    /// An action needs both a label and an action name.
    IncompleteAction,
}

/// What publishing did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Published {
    Added,
    /// A notice with the same module and key was replaced.
    Replaced,
    /// A more recent notice with the same key is already there; this one is ignored.
    Older,
}

/// A notice on the board.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Posted {
    pub notice: ModuleNotice,
    pub published_at_ms: u64,
    /// `None` for a notice without a duration.
    pub expires_at_ms: Option<u64>,
    /// Publication sequence number, to break clock ties in favour of the later call.
    pub seq: u64,
}

impl Posted {
    pub fn is_expired(&self, now_ms: u64) -> bool {
        self.expires_at_ms.is_some_and(|at| at <= now_ms)
    }

    fn rank(
        &self,
    ) -> (
        std::cmp::Reverse<Level>,
        std::cmp::Reverse<(u64, u64)>,
        &str,
        &str,
    ) {
        (
            std::cmp::Reverse(self.notice.level),
            std::cmp::Reverse((self.published_at_ms, self.seq)),
            self.notice.module.as_str(),
            self.notice.key.as_str(),
        )
    }
}

/// The host's notice board.
#[derive(Clone, Debug)]
pub struct Board {
    max: usize,
    seq: u64,
    posted: Vec<Posted>,
}

impl Default for Board {
    fn default() -> Self {
        Self::new(crate::notices::DEFAULT_MAX)
    }
}

impl Board {
    pub fn new(max: usize) -> Self {
        Self {
            max: max.max(1),
            seq: 0,
            posted: Vec::new(),
        }
    }

    /// Validates and posts a notice.
    pub fn publish(&mut self, notice: ModuleNotice, now_ms: u64) -> Result<Published, NoticeError> {
        if notice.module.trim().is_empty() {
            return Err(NoticeError::EmptyModule);
        }
        if notice.key.trim().is_empty() {
            return Err(NoticeError::EmptyKey);
        }
        if notice.title.trim().is_empty() {
            return Err(NoticeError::EmptyTitle);
        }
        if notice.ttl_ms == Some(0) {
            return Err(NoticeError::ZeroDuration);
        }
        if let Some(a) = &notice.action
            && (a.label.trim().is_empty() || a.action.trim().is_empty())
        {
            return Err(NoticeError::IncompleteAction);
        }
        self.seq += 1;
        let posted = Posted {
            expires_at_ms: notice.ttl_ms.map(|ttl| now_ms.saturating_add(ttl)),
            notice,
            published_at_ms: now_ms,
            seq: self.seq,
        };
        if let Some(existing) = self
            .posted
            .iter_mut()
            .find(|p| p.notice.module == posted.notice.module && p.notice.key == posted.notice.key)
        {
            if existing.published_at_ms > posted.published_at_ms {
                return Ok(Published::Older);
            }
            *existing = posted;
            return Ok(Published::Replaced);
        }
        self.posted.push(posted);
        while self.posted.len() > self.max {
            // the least important, then the oldest, gives way
            let weakest = self
                .posted
                .iter()
                .enumerate()
                .min_by_key(|(_, p)| (p.notice.level, p.published_at_ms, p.seq))
                .map(|(i, _)| i);
            match weakest {
                Some(i) => {
                    self.posted.remove(i);
                }
                None => break,
            }
        }
        Ok(Published::Added)
    }

    /// Removes one notice.
    pub fn dismiss(&mut self, module: &str, key: &str) -> bool {
        let before = self.posted.len();
        self.posted
            .retain(|p| !(p.notice.module == module && p.notice.key == key));
        before != self.posted.len()
    }

    /// Removes every notice of a module (it was unloaded).
    pub fn withdraw_module(&mut self, module: &str) -> usize {
        let before = self.posted.len();
        self.posted.retain(|p| p.notice.module != module);
        before - self.posted.len()
    }

    /// Keeps only the notices of loaded modules.
    pub fn retain_modules<'a, I: IntoIterator<Item = &'a str>>(&mut self, loaded: I) -> usize {
        let loaded: BTreeSet<&str> = loaded.into_iter().collect();
        let before = self.posted.len();
        self.posted
            .retain(|p| loaded.contains(p.notice.module.as_str()));
        before - self.posted.len()
    }

    /// Removes the expired notices and returns how many went.
    pub fn expire(&mut self, now_ms: u64) -> usize {
        let before = self.posted.len();
        self.posted.retain(|p| !p.is_expired(now_ms));
        before - self.posted.len()
    }

    /// The live notices at `now_ms`, in display order.
    pub fn visible(&self, now_ms: u64) -> Vec<&Posted> {
        let mut out: Vec<&Posted> = self
            .posted
            .iter()
            .filter(|p| !p.is_expired(now_ms))
            .collect();
        out.sort_by(|a, b| a.rank().cmp(&b.rank()));
        out
    }

    pub fn len(&self) -> usize {
        self.posted.len()
    }

    pub fn is_empty(&self) -> bool {
        self.posted.is_empty()
    }

    /// The next expiry, to schedule a wake-up instead of polling.
    pub fn next_expiry(&self) -> Option<u64> {
        self.posted.iter().filter_map(|p| p.expires_at_ms).min()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys(board: &Board, now: u64) -> Vec<String> {
        board
            .visible(now)
            .iter()
            .map(|p| format!("{}/{}", p.notice.module, p.notice.key))
            .collect()
    }

    #[test]
    fn notices_are_ordered_by_level_then_recency() {
        let mut b = Board::new(10);
        b.publish(ModuleNotice::new("m", "i1", Level::Info, "i1"), 0)
            .unwrap();
        b.publish(ModuleNotice::new("m", "e1", Level::Error, "e1"), 1)
            .unwrap();
        b.publish(ModuleNotice::new("m", "w1", Level::Warning, "w1"), 2)
            .unwrap();
        b.publish(ModuleNotice::new("m", "i2", Level::Info, "i2"), 3)
            .unwrap();
        assert_eq!(keys(&b, 4), ["m/e1", "m/w1", "m/i2", "m/i1"]);
    }

    #[test]
    fn the_same_key_keeps_one_notice_the_most_recent() {
        let mut b = Board::new(10);
        assert_eq!(
            b.publish(ModuleNotice::new("sync", "state", Level::Info, "v1"), 0),
            Ok(Published::Added)
        );
        assert_eq!(
            b.publish(ModuleNotice::new("sync", "state", Level::Error, "v2"), 5),
            Ok(Published::Replaced)
        );
        assert_eq!(
            b.publish(ModuleNotice::new("sync", "state", Level::Info, "stale"), 3),
            Ok(Published::Older)
        );
        let v = b.visible(6);
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].notice.title, "v2");
        assert_eq!(v[0].notice.level, Level::Error);
    }

    #[test]
    fn the_same_key_in_two_modules_is_two_notices() {
        let mut b = Board::new(10);
        b.publish(ModuleNotice::new("a", "sync", Level::Info, "a"), 0)
            .unwrap();
        b.publish(ModuleNotice::new("b", "sync", Level::Info, "b"), 0)
            .unwrap();
        assert_eq!(b.len(), 2);
    }

    #[test]
    fn an_expired_notice_disappears_exactly_at_its_deadline() {
        let mut b = Board::new(10);
        b.publish(
            ModuleNotice::new("m", "short", Level::Info, "s").for_ms(500),
            1_000,
        )
        .unwrap();
        b.publish(ModuleNotice::new("m", "sticky", Level::Info, "k"), 1_000)
            .unwrap();
        assert_eq!(
            keys(&b, 1_499),
            ["m/sticky", "m/short"],
            "same instant: the later call first"
        );
        assert_eq!(
            keys(&b, 1_500),
            ["m/sticky"],
            "hidden even before the sweep"
        );
        assert_eq!(b.next_expiry(), Some(1_500));
        assert_eq!(b.expire(1_500), 1);
        assert_eq!(b.expire(1_000_000), 0, "a notice without duration stays");
        assert_eq!(b.next_expiry(), None);
    }

    #[test]
    fn replacing_a_notice_restarts_its_duration() {
        let mut b = Board::new(10);
        b.publish(ModuleNotice::new("m", "k", Level::Info, "a").for_ms(100), 0)
            .unwrap();
        b.publish(
            ModuleNotice::new("m", "k", Level::Info, "b").for_ms(100),
            90,
        )
        .unwrap();
        assert_eq!(keys(&b, 150), ["m/k"]);
        assert_eq!(b.next_expiry(), Some(190));
    }

    #[test]
    fn a_notice_carries_its_action() {
        let mut b = Board::new(10);
        b.publish(
            ModuleNotice::new("sync", "offline", Level::Warning, "Hors ligne")
                .with_action("Réessayer", "sync.retry"),
            0,
        )
        .unwrap();
        let v = b.visible(0);
        assert_eq!(
            v[0].notice.action.as_ref().map(|a| a.action.as_str()),
            Some("sync.retry")
        );
    }

    #[test]
    fn invalid_notices_are_refused() {
        let mut b = Board::new(10);
        assert_eq!(
            b.publish(ModuleNotice::new("", "k", Level::Info, "t"), 0),
            Err(NoticeError::EmptyModule)
        );
        assert_eq!(
            b.publish(ModuleNotice::new("m", " ", Level::Info, "t"), 0),
            Err(NoticeError::EmptyKey)
        );
        assert_eq!(
            b.publish(ModuleNotice::new("m", "k", Level::Info, ""), 0),
            Err(NoticeError::EmptyTitle)
        );
        assert_eq!(
            b.publish(ModuleNotice::new("m", "k", Level::Info, "t").for_ms(0), 0),
            Err(NoticeError::ZeroDuration)
        );
        assert_eq!(
            b.publish(
                ModuleNotice::new("m", "k", Level::Info, "t").with_action("", "x"),
                0
            ),
            Err(NoticeError::IncompleteAction)
        );
        assert!(b.is_empty());
    }

    #[test]
    fn when_full_the_least_important_and_oldest_gives_way() {
        let mut b = Board::new(2);
        b.publish(ModuleNotice::new("m", "err", Level::Error, "e"), 0)
            .unwrap();
        b.publish(ModuleNotice::new("m", "info_old", Level::Info, "i"), 1)
            .unwrap();
        b.publish(ModuleNotice::new("m", "info_new", Level::Info, "i"), 2)
            .unwrap();
        assert_eq!(keys(&b, 3), ["m/err", "m/info_new"]);
    }

    #[test]
    fn withdrawing_a_module_removes_its_notices() {
        let mut b = Board::new(10);
        b.publish(ModuleNotice::new("a", "k", Level::Info, "a"), 0)
            .unwrap();
        b.publish(ModuleNotice::new("b", "k", Level::Info, "b"), 0)
            .unwrap();
        b.publish(ModuleNotice::new("c", "k", Level::Info, "c"), 0)
            .unwrap();
        assert_eq!(b.withdraw_module("a"), 1);
        assert_eq!(b.retain_modules(["c"]), 1);
        assert_eq!(keys(&b, 0), ["c/k"]);
        assert!(b.dismiss("c", "k"));
        assert!(!b.dismiss("c", "k"));
    }

    #[test]
    fn a_module_notice_maps_to_a_device_notice() {
        let n = ModuleNotice::new("sync", "offline", Level::Error, "Hors ligne")
            .with_body("Réseau coupé");
        let d = n.to_device();
        assert_eq!(d.key, "sync/offline");
        assert_eq!(d.tone, Tone::Danger);
        assert_eq!(d.body, "Réseau coupé");
    }

    #[test]
    fn the_wire_shape_is_stable() {
        let raw = r#"{ "module": "sync", "key": "offline", "level": "warning", "title": "Hors ligne",
                       "ttl_ms": 4000, "action": { "label": "Réessayer", "action": "sync.retry" } }"#;
        let n: ModuleNotice = serde_json::from_str(raw).expect("notice json");
        assert_eq!(n.level, Level::Warning);
        assert_eq!(n.ttl_ms, Some(4000));
        assert_eq!(n.body, "");
    }
}
