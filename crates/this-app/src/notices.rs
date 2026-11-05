//! # La file d'avis présentés à l'utilisateur
//!
//! Trois règles suffisent, et c'est tout ce qu'on veut garantir :
//!
//! 1. **une clé = un avis** : dix erreurs identiques ne font pas dix toasts, la dernière
//!    remplace la précédente ;
//! 2. **borne** : au plus `max` avis vivants, le plus ancien cède ;
//! 3. **expiration par l'horloge de l'appelant** (`now_ms`), pour que la logique se teste
//!    à la milliseconde près, sans `sleep`.
//!
//! Le rendu (toast, bannière, notification système) ne vit pas ici : la coquille décide.

use crate::device::Notice;

/// Durée d'affichage par défaut d'un avis.
pub const DEFAULT_TTL_MS: u64 = 6_000;
/// Nombre d'avis simultanés par défaut.
pub const DEFAULT_MAX: usize = 3;

/// Un avis en vie, avec son échéance.
#[derive(Clone, Debug, PartialEq)]
pub struct Live {
    pub notice: Notice,
    pub expires_at_ms: u64,
}

/// La file d'avis.
#[derive(Clone, Debug)]
pub struct Notices {
    max: usize,
    live: Vec<Live>,
}

impl Default for Notices {
    fn default() -> Self {
        Self::new(DEFAULT_MAX)
    }
}

impl Notices {
    pub fn new(max: usize) -> Self {
        Self {
            max: max.max(1),
            live: Vec::new(),
        }
    }

    /// Ajoute un avis avec la durée par défaut ; une clé déjà présente est **remplacée**.
    pub fn push(&mut self, notice: Notice, now_ms: u64) -> bool {
        self.push_for(notice, now_ms, DEFAULT_TTL_MS)
    }

    pub fn push_for(&mut self, notice: Notice, now_ms: u64, ttl_ms: u64) -> bool {
        let expires_at_ms = now_ms.saturating_add(ttl_ms);
        if let Some(existing) = self
            .live
            .iter_mut()
            .find(|live| live.notice.key == notice.key)
        {
            existing.notice = notice;
            existing.expires_at_ms = expires_at_ms;
            return false;
        }
        self.live.push(Live {
            notice,
            expires_at_ms,
        });
        // le plus ancien cède : la file garde les `max` derniers
        while self.live.len() > self.max {
            self.live.remove(0);
        }
        true
    }

    /// Retire un avis par sa clé.
    pub fn dismiss(&mut self, key: &str) -> bool {
        let before = self.live.len();
        self.live.retain(|live| live.notice.key != key);
        self.live.len() != before
    }

    /// Retire les avis échus et rend le nombre retiré.
    pub fn expire(&mut self, now_ms: u64) -> usize {
        let before = self.live.len();
        self.live.retain(|live| live.expires_at_ms > now_ms);
        before - self.live.len()
    }

    /// Les avis en vie, du plus ancien au plus récent.
    pub fn live(&self) -> &[Live] {
        &self.live
    }

    pub fn len(&self) -> usize {
        self.live.len()
    }

    pub fn is_empty(&self) -> bool {
        self.live.is_empty()
    }

    /// La prochaine échéance, pour programmer un réveil sans scruter.
    pub fn next_expiry(&self) -> Option<u64> {
        self.live.iter().map(|live| live.expires_at_ms).min()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device::Tone;

    fn notice(key: &str) -> Notice {
        Notice::new(key, key, "corps")
    }

    #[test]
    fn pushing_appends_in_order() {
        let mut notices = Notices::new(3);
        assert!(notices.push(notice("a"), 0));
        assert!(notices.push(notice("b"), 10));
        assert_eq!(notices.len(), 2);
        assert_eq!(notices.live()[0].notice.key, "a");
        assert_eq!(notices.live()[1].notice.key, "b");
    }

    #[test]
    fn the_same_key_replaces_instead_of_stacking() {
        let mut notices = Notices::new(3);
        notices.push(notice("save"), 0);
        let added = notices.push(notice("save"), 1_000);
        assert!(!added, "remplacé, pas ajouté");
        assert_eq!(notices.len(), 1);
        // l'échéance est repoussée par le remplacement
        assert_eq!(notices.next_expiry(), Some(1_000 + DEFAULT_TTL_MS));
    }

    #[test]
    fn the_queue_is_bounded_and_drops_the_oldest() {
        let mut notices = Notices::new(2);
        notices.push(notice("a"), 0);
        notices.push(notice("b"), 1);
        notices.push(notice("c"), 2);
        assert_eq!(notices.len(), 2);
        assert_eq!(notices.live()[0].notice.key, "b");
        assert_eq!(notices.live()[1].notice.key, "c");
    }

    #[test]
    fn expiry_follows_the_callers_clock() {
        let mut notices = Notices::new(3);
        notices.push_for(notice("court"), 1_000, 500);
        notices.push_for(notice("long"), 1_000, 5_000);

        assert_eq!(notices.expire(1_400), 0, "rien n'est échu avant l'échéance");
        assert_eq!(notices.expire(1_500), 1, "échu à l'échéance exacte");
        assert_eq!(notices.len(), 1);
        assert_eq!(notices.live()[0].notice.key, "long");
    }

    #[test]
    fn dismiss_targets_one_key() {
        let mut notices = Notices::new(3);
        notices.push(notice("a"), 0);
        notices.push(notice("b"), 0);
        assert!(notices.dismiss("a"));
        assert!(!notices.dismiss("a"), "déjà retiré");
        assert_eq!(notices.len(), 1);
        assert_eq!(notices.live()[0].notice.key, "b");
    }

    #[test]
    fn a_tone_survives_the_queue() {
        let mut notices = Notices::new(1);
        notices.push(notice("erreur").with_tone(Tone::Danger), 0);
        assert_eq!(notices.live()[0].notice.tone, Tone::Danger);
    }
}
