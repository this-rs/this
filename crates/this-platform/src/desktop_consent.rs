//! # Desktop control consent — the grant store behind the Atelier's direct control of a Mac
//!
//! RFC `c054153b` ("Contrôle direct du bureau") asks for four desktop capabilities, each
//! with its own consent, scope and lifetime. This module is the **pure decision core**:
//! it holds the grants the person gave, answers "may this module do this to this target
//! now?", and lets a grant be revoked while an action is already running.
//!
//! The rules, all fail-closed:
//!
//! - **Unknown capability refused first.** A wire name that is not one of
//!   [`DesktopCapability::ALL`] is refused by [`DesktopCapability::parse`] before any grant
//!   is looked at, so no consent is ever asked for it.
//! - **Default deny.** No grant for `(module, capability)` means [`Refusal::NoGrant`].
//!   Nothing here is an implicit agreement.
//! - **Scope.** A grant names the targets it covers (bundle ids for `Observe` and `Act`,
//!   command names or path prefixes for `ShellExec` and `FilesWrite`). An empty scope
//!   covers nothing.
//! - **Lifetime.** A grant may carry an expiry in milliseconds; at or after it, the grant
//!   is refused as [`Refusal::Expired`].
//! - **Revocation interrupts.** An [`Ticket`] is issued per authorised action. Revoking a
//!   grant makes every ticket issued under it dead, so a long action checks
//!   [`DesktopConsent::is_live`] between steps and stops, not only the next request.
//!
//! No OS call happens here. The shell (the macOS helper, later) asks the person, stores the
//! answer with [`DesktopConsent::grant`], and checks every step with this module.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

/// The four desktop capabilities of the RFC. Ordered from least to most powerful.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DesktopCapability {
    /// Read the screen and the structure of the windows of the scoped apps.
    Observe,
    /// Click and type in the scoped apps.
    Act,
    /// Run a shell command. Scope: the command names allowed.
    ShellExec,
    /// Write a file. Scope: the path prefixes allowed.
    FilesWrite,
}

impl DesktopCapability {
    pub const ALL: [DesktopCapability; 4] = [
        DesktopCapability::Observe,
        DesktopCapability::Act,
        DesktopCapability::ShellExec,
        DesktopCapability::FilesWrite,
    ];

    /// The name a module uses on the wire.
    pub fn wire_name(self) -> &'static str {
        match self {
            DesktopCapability::Observe => "desktop.observe",
            DesktopCapability::Act => "desktop.act",
            DesktopCapability::ShellExec => "desktop.shell_exec",
            DesktopCapability::FilesWrite => "desktop.files_write",
        }
    }

    /// Reads a wire name. `None` for anything the bridge does not know: such a request is
    /// refused before consent is consulted.
    pub fn parse(name: &str) -> Option<DesktopCapability> {
        DesktopCapability::ALL
            .into_iter()
            .find(|capability| capability.wire_name() == name)
    }
}

/// Why an authorisation was refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// The wire name is not a desktop capability of the RFC.
    UnknownCapability,
    /// The person never granted this capability to this module, or revoked it.
    NoGrant,
    /// The grant existed but its lifetime is over.
    Expired,
    /// The grant exists and is live, but the target is not in its scope.
    OutOfScope,
}

/// Proof that one action was authorised under one grant. It stops being live as soon as
/// that grant is revoked or expires.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ticket {
    module: String,
    capability: DesktopCapability,
    grant_id: u64,
}

impl Ticket {
    pub fn module(&self) -> &str {
        &self.module
    }

    pub fn capability(&self) -> DesktopCapability {
        self.capability
    }
}

#[derive(Clone, Debug)]
struct Grant {
    id: u64,
    targets: BTreeSet<String>,
    expires_at_ms: Option<u64>,
}

impl Grant {
    fn expired_at(&self, now_ms: u64) -> bool {
        self.expires_at_ms.is_some_and(|at| now_ms >= at)
    }
}

/// The grants the person gave. Pure state: the caller persists it and decides when to
/// call it.
#[derive(Debug, Default)]
pub struct DesktopConsent {
    grants: BTreeMap<(String, DesktopCapability), Grant>,
    next_id: u64,
}

impl DesktopConsent {
    pub fn new() -> Self {
        Self::default()
    }

    /// Records the person's agreement. A new grant replaces the previous one for the same
    /// `(module, capability)`, and the replaced grant's tickets die with it.
    pub fn grant<I, S>(
        &mut self,
        module: &str,
        capability: DesktopCapability,
        targets: I,
        expires_at_ms: Option<u64>,
    ) where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.next_id += 1;
        let grant = Grant {
            id: self.next_id,
            targets: targets.into_iter().map(Into::into).collect(),
            expires_at_ms,
        };
        self.grants.insert((module.to_owned(), capability), grant);
    }

    /// Authorises one action on one target. Checks, in order: the grant exists, it has
    /// not expired, the target is in its scope. An expired grant is removed.
    pub fn authorize(
        &mut self,
        module: &str,
        capability: DesktopCapability,
        target: &str,
        now_ms: u64,
    ) -> Result<Ticket, Refusal> {
        let key = (module.to_owned(), capability);
        let grant = self.grants.get(&key).ok_or(Refusal::NoGrant)?;
        if grant.expired_at(now_ms) {
            self.grants.remove(&key);
            return Err(Refusal::Expired);
        }
        if !grant.targets.contains(target) {
            return Err(Refusal::OutOfScope);
        }
        Ok(Ticket {
            module: module.to_owned(),
            capability,
            grant_id: grant.id,
        })
    }

    /// Whether the grant under `ticket` still stands. A caller running a multi-step action
    /// checks this before each step.
    pub fn is_live(&self, ticket: &Ticket, now_ms: u64) -> bool {
        self.grants
            .get(&(ticket.module.clone(), ticket.capability))
            .is_some_and(|grant| grant.id == ticket.grant_id && !grant.expired_at(now_ms))
    }

    /// Revokes one capability of one module. Its tickets die at once. Returns whether a
    /// grant was removed.
    pub fn revoke(&mut self, module: &str, capability: DesktopCapability) -> bool {
        self.grants
            .remove(&(module.to_owned(), capability))
            .is_some()
    }

    /// Revokes every capability of one module.
    pub fn revoke_module(&mut self, module: &str) {
        self.grants.retain(|(owner, _), _| owner != module);
    }

    /// The kill switch: every grant of every module is removed.
    pub fn revoke_all(&mut self) {
        self.grants.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: u64 = 1_000;

    #[test]
    fn unknown_capability_name_is_refused_by_parse() {
        assert_eq!(DesktopCapability::parse("desktop.root"), None);
        assert_eq!(DesktopCapability::parse("desktop.observe "), None);
        assert_eq!(DesktopCapability::parse(""), None);
        for capability in DesktopCapability::ALL {
            assert_eq!(
                DesktopCapability::parse(capability.wire_name()),
                Some(capability)
            );
        }
    }

    #[test]
    fn with_no_grant_every_capability_is_refused() {
        let mut consent = DesktopConsent::new();
        for capability in DesktopCapability::ALL {
            assert_eq!(
                consent.authorize("atelier", capability, "com.apple.Safari", NOW),
                Err(Refusal::NoGrant)
            );
        }
    }

    #[test]
    fn a_grant_to_one_module_does_not_reach_another() {
        let mut consent = DesktopConsent::new();
        consent.grant(
            "atelier",
            DesktopCapability::Observe,
            ["com.apple.Safari"],
            None,
        );
        assert!(
            consent
                .authorize(
                    "autre-module",
                    DesktopCapability::Observe,
                    "com.apple.Safari",
                    NOW
                )
                .is_err()
        );
    }

    #[test]
    fn a_grant_to_one_capability_does_not_reach_another() {
        let mut consent = DesktopConsent::new();
        consent.grant(
            "atelier",
            DesktopCapability::Observe,
            ["com.apple.Safari"],
            None,
        );
        assert_eq!(
            consent.authorize("atelier", DesktopCapability::Act, "com.apple.Safari", NOW),
            Err(Refusal::NoGrant)
        );
    }

    #[test]
    fn an_empty_scope_covers_nothing() {
        let mut consent = DesktopConsent::new();
        let none: [&str; 0] = [];
        consent.grant("atelier", DesktopCapability::Act, none, None);
        assert_eq!(
            consent.authorize("atelier", DesktopCapability::Act, "com.apple.Safari", NOW),
            Err(Refusal::OutOfScope)
        );
    }

    #[test]
    fn a_target_outside_the_scope_is_refused() {
        let mut consent = DesktopConsent::new();
        consent.grant(
            "atelier",
            DesktopCapability::Act,
            ["com.apple.Safari"],
            None,
        );
        assert_eq!(
            consent.authorize("atelier", DesktopCapability::Act, "com.apple.Terminal", NOW),
            Err(Refusal::OutOfScope)
        );
        assert!(
            consent
                .authorize("atelier", DesktopCapability::Act, "com.apple.Safari", NOW)
                .is_ok()
        );
    }

    #[test]
    fn an_expired_grant_is_refused_and_removed() {
        let mut consent = DesktopConsent::new();
        consent.grant(
            "atelier",
            DesktopCapability::Observe,
            ["com.apple.Safari"],
            Some(2_000),
        );
        assert!(
            consent
                .authorize(
                    "atelier",
                    DesktopCapability::Observe,
                    "com.apple.Safari",
                    1_999
                )
                .is_ok()
        );
        assert_eq!(
            consent.authorize(
                "atelier",
                DesktopCapability::Observe,
                "com.apple.Safari",
                2_000
            ),
            Err(Refusal::Expired)
        );
        assert_eq!(
            consent.authorize(
                "atelier",
                DesktopCapability::Observe,
                "com.apple.Safari",
                2_001
            ),
            Err(Refusal::NoGrant),
            "an expired grant is removed, not kept around"
        );
    }

    #[test]
    fn revoking_interrupts_a_ticket_already_issued() {
        let mut consent = DesktopConsent::new();
        consent.grant(
            "atelier",
            DesktopCapability::Act,
            ["com.apple.Safari"],
            None,
        );
        let ticket = consent
            .authorize("atelier", DesktopCapability::Act, "com.apple.Safari", NOW)
            .expect("authorised");
        assert!(consent.is_live(&ticket, NOW));

        assert!(consent.revoke("atelier", DesktopCapability::Act));
        assert!(
            !consent.is_live(&ticket, NOW),
            "a step of a running action must see the revocation"
        );
    }

    #[test]
    fn replacing_a_grant_kills_the_tickets_of_the_old_one() {
        let mut consent = DesktopConsent::new();
        consent.grant(
            "atelier",
            DesktopCapability::Act,
            ["com.apple.Safari"],
            None,
        );
        let old = consent
            .authorize("atelier", DesktopCapability::Act, "com.apple.Safari", NOW)
            .expect("authorised");
        consent.grant(
            "atelier",
            DesktopCapability::Act,
            ["com.apple.Safari"],
            None,
        );
        assert!(!consent.is_live(&old, NOW));
    }

    #[test]
    fn a_ticket_dies_when_its_grant_expires_mid_action() {
        let mut consent = DesktopConsent::new();
        consent.grant(
            "atelier",
            DesktopCapability::FilesWrite,
            ["/Users/triviere/Notes/"],
            Some(1_500),
        );
        let ticket = consent
            .authorize(
                "atelier",
                DesktopCapability::FilesWrite,
                "/Users/triviere/Notes/",
                NOW,
            )
            .expect("authorised");
        assert!(consent.is_live(&ticket, 1_499));
        assert!(!consent.is_live(&ticket, 1_500));
    }

    #[test]
    fn revoking_one_module_leaves_the_others_intact() {
        let mut consent = DesktopConsent::new();
        consent.grant(
            "atelier",
            DesktopCapability::Observe,
            ["com.apple.Safari"],
            None,
        );
        consent.grant(
            "autre",
            DesktopCapability::Observe,
            ["com.apple.Safari"],
            None,
        );
        consent.revoke_module("atelier");
        assert_eq!(
            consent.authorize(
                "atelier",
                DesktopCapability::Observe,
                "com.apple.Safari",
                NOW
            ),
            Err(Refusal::NoGrant)
        );
        assert!(
            consent
                .authorize("autre", DesktopCapability::Observe, "com.apple.Safari", NOW)
                .is_ok()
        );
    }

    #[test]
    fn the_kill_switch_removes_every_grant_and_kills_every_ticket() {
        let mut consent = DesktopConsent::new();
        consent.grant(
            "atelier",
            DesktopCapability::Observe,
            ["com.apple.Safari"],
            None,
        );
        consent.grant("atelier", DesktopCapability::ShellExec, ["ls"], None);
        let observe = consent
            .authorize(
                "atelier",
                DesktopCapability::Observe,
                "com.apple.Safari",
                NOW,
            )
            .expect("authorised");
        let shell = consent
            .authorize("atelier", DesktopCapability::ShellExec, "ls", NOW)
            .expect("authorised");

        consent.revoke_all();

        assert!(!consent.is_live(&observe, NOW));
        assert!(!consent.is_live(&shell, NOW));
        assert_eq!(
            consent.authorize("atelier", DesktopCapability::ShellExec, "ls", NOW),
            Err(Refusal::NoGrant)
        );
    }

    #[test]
    fn revoking_what_was_never_granted_reports_nothing_removed() {
        let mut consent = DesktopConsent::new();
        assert!(!consent.revoke("atelier", DesktopCapability::Act));
    }

    #[test]
    fn capability_names_are_stable_on_the_wire() {
        assert_eq!(
            serde_json::to_value(DesktopCapability::ShellExec).unwrap(),
            serde_json::json!("shell_exec")
        );
        let back: DesktopCapability =
            serde_json::from_value(serde_json::json!("files_write")).unwrap();
        assert_eq!(back, DesktopCapability::FilesWrite);
    }
}
