//! The persistent consent store: which module may use which device capability, and the queue
//! of first-use questions waiting for the person's answer.
//!
//! The store is the [`Consent`] that [`this_platform::device_bridge::gate`] consults:
//!
//! - no grant for a `(module, capability)` pair is [`Decision::Ask`], never an agreement;
//! - a grant in the [`GrantState::Granted`] state is [`Decision::Allow`];
//! - [`GrantState::Refused`] and [`GrantState::Revoked`] are [`Decision::Deny`].
//!
//! An `Ask` is queued: [`ConsentStore::request_access`] puts the first request of a pair in
//! [`ConsentStore::list_pending`], and the module gets `NotAuthorized` until the person
//! answers. Allow only ever applies to the exact `(module, capability)` pair: no module
//! inherits another module's grant, and no capability inherits another's.
//!
//! No I/O and no clock here. Persistence goes through [`GrantStore`]; the instant of each
//! decision is passed in by the caller, as milliseconds.
//!
//! The Obrain-backed [`GrantStore`] is not in this crate: it belongs to the shell that owns
//! the database. It must implement [`GrantStore`] and nothing else, so that the decision
//! logic stays the one tested here.

use std::fmt;

use serde::{Deserialize, Serialize};

use this_platform::capability::Capability;
use this_platform::device_bridge::{
    Consent, Decision, DeviceError, Reply, Request, gate, required_capability,
};

/// The state of a grant, as the person left it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GrantState {
    /// The person allowed it.
    Granted,
    /// The person refused it. A refusal is a decision, not an absence of one.
    Refused,
    /// The person allowed it, then took it back.
    Revoked,
}

/// The person's answer to a first-use question.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Answer {
    Allow,
    Deny,
}

/// One decision for one `(module, capability)` pair, with the instant it was taken.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Grant {
    pub module: String,
    pub capability: Capability,
    pub state: GrantState,
    /// When the decision was taken, in milliseconds since the Unix epoch, as the caller read it.
    pub decided_at_ms: u64,
}

impl Grant {
    fn is_for(&self, module: &str, capability: Capability) -> bool {
        self.module == module && self.capability == capability
    }
}

/// A first-use question waiting for the person's answer.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PendingRequest {
    pub module: String,
    pub capability: Capability,
    /// When the module first asked, in milliseconds since the Unix epoch, as the caller read it.
    pub asked_at_ms: u64,
}

/// A failure of the grant store: the grants could not be read or written.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StoreError(pub String);

impl fmt::Display for StoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "consent store: {}", self.0)
    }
}

impl std::error::Error for StoreError {}

/// Where the grants persist. The Obrain implementation lives outside this crate.
///
/// `load` returns every grant saved by the last successful `save`. `save` replaces the whole
/// set and must be atomic: either every grant is written, or the previous set is kept.
pub trait GrantStore {
    fn load(&self) -> Result<Vec<Grant>, StoreError>;
    fn save(&mut self, grants: &[Grant]) -> Result<(), StoreError>;
}

/// An in-memory [`GrantStore`]. For tests and for a shell that does not persist yet: the
/// grants then last as long as the process.
#[derive(Clone, Debug, Default)]
pub struct MemoryStore {
    grants: Vec<Grant>,
}

impl MemoryStore {
    pub fn new() -> Self {
        Self::default()
    }
}

impl GrantStore for MemoryStore {
    fn load(&self) -> Result<Vec<Grant>, StoreError> {
        Ok(self.grants.clone())
    }

    fn save(&mut self, grants: &[Grant]) -> Result<(), StoreError> {
        self.grants = grants.to_vec();
        Ok(())
    }
}

/// The consent store: the persisted grants and the in-memory queue of first-use questions.
///
/// The queue is not persisted. After a restart a module asks again, and is queued again.
pub struct ConsentStore<S: GrantStore> {
    store: S,
    grants: Vec<Grant>,
    pending: Vec<PendingRequest>,
}

impl<S: GrantStore> ConsentStore<S> {
    /// Open the store on the grants it has saved. Refuses a store that holds two grants for
    /// the same pair: the decision for that pair would be ambiguous.
    pub fn open(store: S) -> Result<Self, StoreError> {
        let grants = store.load()?;
        for (index, grant) in grants.iter().enumerate() {
            if grants[..index]
                .iter()
                .any(|other| other.is_for(&grant.module, grant.capability))
            {
                return Err(StoreError(format!(
                    "duplicate grant for module `{}` and capability {:?}",
                    grant.module, grant.capability
                )));
            }
        }
        Ok(Self {
            store,
            grants,
            pending: Vec::new(),
        })
    }

    /// Every grant, in the order they were first decided.
    pub fn list_grants(&self) -> &[Grant] {
        &self.grants
    }

    /// The first-use questions still waiting for an answer, oldest first.
    pub fn list_pending(&self) -> &[PendingRequest] {
        &self.pending
    }

    /// Record the person's answer for a pair, and drop its pending question. Replaces an
    /// earlier decision, which is how a refused or revoked pair is allowed again.
    ///
    /// On a store failure nothing changes: neither the grant nor the queue.
    pub fn answer(
        &mut self,
        module: &str,
        capability: Capability,
        answer: Answer,
        at_ms: u64,
    ) -> Result<(), StoreError> {
        let state = match answer {
            Answer::Allow => GrantState::Granted,
            Answer::Deny => GrantState::Refused,
        };
        self.set_state(module, capability, state, at_ms)?;
        self.dequeue(module, capability);
        Ok(())
    }

    /// Take a grant back. The pair is then denied, not asked again.
    ///
    /// On a store failure nothing changes: neither the grant nor the queue.
    pub fn revoke(
        &mut self,
        module: &str,
        capability: Capability,
        at_ms: u64,
    ) -> Result<(), StoreError> {
        self.set_state(module, capability, GrantState::Revoked, at_ms)?;
        self.dequeue(module, capability);
        Ok(())
    }

    /// Queue a first-use question. Returns `true` when it was added: a pair already pending,
    /// or already decided, is not queued again.
    pub fn enqueue(&mut self, module: &str, capability: Capability, at_ms: u64) -> bool {
        if self.find(module, capability).is_some() || self.is_pending(module, capability) {
            return false;
        }
        self.pending.push(PendingRequest {
            module: module.to_owned(),
            capability,
            asked_at_ms: at_ms,
        });
        true
    }

    /// Gate a request for `module`, and queue it when the answer is still `Ask`. The module
    /// gets the same answer as [`gate`] would give; the queue is the only side effect.
    pub fn request_access(
        &mut self,
        module: &str,
        request: &Request,
        at_ms: u64,
    ) -> Result<Capability, Reply> {
        let result = gate(module, request, &*self);
        if let Err(Reply::Error {
            code: DeviceError::NotAuthorized,
        }) = result
        {
            // Denied and asked pairs both come back as NotAuthorized. `enqueue` queues only
            // the pairs that have no decision yet, so a denied or revoked pair stays denied.
            // A request whose method the bridge does not know has no capability at all: it was
            // refused as `NotSupported` above, so there is nothing to queue.
            if let Some(capability) = required_capability(request) {
                self.enqueue(module, capability, at_ms);
            }
        }
        result
    }

    fn find(&self, module: &str, capability: Capability) -> Option<&Grant> {
        self.grants
            .iter()
            .find(|grant| grant.is_for(module, capability))
    }

    fn is_pending(&self, module: &str, capability: Capability) -> bool {
        self.pending
            .iter()
            .any(|request| request.module == module && request.capability == capability)
    }

    fn dequeue(&mut self, module: &str, capability: Capability) {
        self.pending
            .retain(|request| !(request.module == module && request.capability == capability));
    }

    /// Repose la question après un refus ou une révocation : la décision est retirée, la
    /// capacité redevient `Ask`, et une question part en file.
    ///
    /// C'est ce qui rend une révocation **réversible**. Sans elle, une capacité révoquée
    /// était niée pour toujours (`Decision::Deny`), et l'agent n'avait aucun moyen de
    /// redemander — il lisait « en attente » alors que rien n'attendait plus. Une décision
    /// ne doit être définitive que si la personne veut qu'elle le soit.
    pub fn reask(
        &mut self,
        module: &str,
        capability: Capability,
        at_ms: u64,
    ) -> Result<(), StoreError> {
        let mut grants: Vec<Grant> = self
            .grants
            .iter()
            .filter(|grant| !grant.is_for(module, capability))
            .cloned()
            .collect();
        self.store.save(&grants)?;
        self.grants = grants;
        self.enqueue(module, capability, at_ms);
        Ok(())
    }

    fn set_state(
        &mut self,
        module: &str,
        capability: Capability,
        state: GrantState,
        at_ms: u64,
    ) -> Result<(), StoreError> {
        let mut grants = self.grants.clone();
        match grants
            .iter_mut()
            .find(|grant| grant.is_for(module, capability))
        {
            Some(grant) => {
                grant.state = state;
                grant.decided_at_ms = at_ms;
            }
            None => grants.push(Grant {
                module: module.to_owned(),
                capability,
                state,
                decided_at_ms: at_ms,
            }),
        }
        self.store.save(&grants)?;
        self.grants = grants;
        Ok(())
    }
}

impl<S: GrantStore> Consent for ConsentStore<S> {
    fn decide(&self, module: &str, capability: Capability) -> Decision {
        match self.find(module, capability).map(|grant| grant.state) {
            Some(GrantState::Granted) => Decision::Allow,
            Some(GrantState::Refused | GrantState::Revoked) => Decision::Deny,
            None => Decision::Ask,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const AT: u64 = 1_700_000_000_000;

    fn gyro_request() -> Request {
        Request::GyroscopeSample
    }

    fn bt_request() -> Request {
        Request::BluetoothScan {
            duration_ms: 3000,
            services: vec![],
        }
    }

    fn fresh() -> ConsentStore<MemoryStore> {
        ConsentStore::open(MemoryStore::new()).unwrap()
    }

    /// A store that refuses every write, to check that a failed save changes nothing.
    struct FailingStore {
        grants: Vec<Grant>,
    }

    impl GrantStore for FailingStore {
        fn load(&self) -> Result<Vec<Grant>, StoreError> {
            Ok(self.grants.clone())
        }

        fn save(&mut self, _grants: &[Grant]) -> Result<(), StoreError> {
            Err(StoreError("disk full".into()))
        }
    }

    #[test]
    fn absence_of_grant_is_ask() {
        let store = fresh();
        assert_eq!(
            store.decide("atelier", Capability::Gyroscope),
            Decision::Ask
        );
    }

    #[test]
    fn deny_is_deny() {
        let mut store = fresh();
        store
            .answer("atelier", Capability::Gyroscope, Answer::Deny, AT)
            .unwrap();
        assert_eq!(
            store.decide("atelier", Capability::Gyroscope),
            Decision::Deny
        );
    }

    #[test]
    fn allow_is_allow() {
        let mut store = fresh();
        store
            .answer("atelier", Capability::Gyroscope, Answer::Allow, AT)
            .unwrap();
        assert_eq!(
            store.decide("atelier", Capability::Gyroscope),
            Decision::Allow
        );
    }

    #[test]
    fn revoked_after_allow_is_deny() {
        let mut store = fresh();
        store
            .answer("atelier", Capability::Gyroscope, Answer::Allow, AT)
            .unwrap();
        store
            .revoke("atelier", Capability::Gyroscope, AT + 1)
            .unwrap();
        assert_eq!(
            store.decide("atelier", Capability::Gyroscope),
            Decision::Deny
        );
        assert_eq!(store.list_grants()[0].state, GrantState::Revoked);
    }

    #[test]
    fn revoke_without_prior_grant_is_deny_not_ask() {
        let mut store = fresh();
        store.revoke("atelier", Capability::Gyroscope, AT).unwrap();
        assert_eq!(
            store.decide("atelier", Capability::Gyroscope),
            Decision::Deny
        );
    }

    #[test]
    fn allow_again_after_revoke() {
        let mut store = fresh();
        store.revoke("atelier", Capability::Gyroscope, AT).unwrap();
        store
            .answer("atelier", Capability::Gyroscope, Answer::Allow, AT + 1)
            .unwrap();
        assert_eq!(
            store.decide("atelier", Capability::Gyroscope),
            Decision::Allow
        );
        assert_eq!(store.list_grants().len(), 1, "one grant per pair");
    }

    #[test]
    fn allow_applies_only_to_the_exact_pair() {
        let mut store = fresh();
        store
            .answer("atelier", Capability::Gyroscope, Answer::Allow, AT)
            .unwrap();
        assert_eq!(
            store.decide("atelier", Capability::Bluetooth),
            Decision::Ask,
            "another capability of the same module"
        );
        assert_eq!(
            store.decide("atelier", Capability::Gyroscope),
            Decision::Allow
        );
    }

    #[test]
    fn a_module_does_not_inherit_another_modules_grant() {
        let mut store = fresh();
        store
            .answer("atelier", Capability::Gyroscope, Answer::Allow, AT)
            .unwrap();
        assert_eq!(
            store.decide("sport", Capability::Gyroscope),
            Decision::Ask,
            "the same capability, another module"
        );
        assert_eq!(
            store.decide("atelier-extra", Capability::Gyroscope),
            Decision::Ask,
            "a module whose name only starts the same way"
        );
    }

    #[test]
    fn a_refusal_for_one_module_does_not_deny_another() {
        let mut store = fresh();
        store
            .answer("atelier", Capability::Bluetooth, Answer::Deny, AT)
            .unwrap();
        assert_eq!(store.decide("sport", Capability::Bluetooth), Decision::Ask);
    }

    #[test]
    fn queue_holds_no_duplicate() {
        let mut store = fresh();
        assert!(store.enqueue("atelier", Capability::Gyroscope, AT));
        assert!(!store.enqueue("atelier", Capability::Gyroscope, AT + 1));
        assert_eq!(store.list_pending().len(), 1);
        assert_eq!(store.list_pending()[0].asked_at_ms, AT, "first ask is kept");
    }

    #[test]
    fn queue_does_not_hold_a_decided_pair() {
        let mut store = fresh();
        store
            .answer("atelier", Capability::Gyroscope, Answer::Deny, AT)
            .unwrap();
        assert!(!store.enqueue("atelier", Capability::Gyroscope, AT + 1));
        assert!(store.list_pending().is_empty());
    }

    #[test]
    fn answer_removes_the_pending_question() {
        let mut store = fresh();
        store.enqueue("atelier", Capability::Gyroscope, AT);
        store.enqueue("atelier", Capability::Bluetooth, AT);
        store
            .answer("atelier", Capability::Gyroscope, Answer::Allow, AT + 1)
            .unwrap();
        assert_eq!(store.list_pending().len(), 1);
        assert_eq!(store.list_pending()[0].capability, Capability::Bluetooth);
    }

    #[test]
    fn gate_is_not_authorized_on_ask() {
        let store = fresh();
        assert_eq!(
            gate("atelier", &gyro_request(), &store),
            Err(Reply::Error {
                code: DeviceError::NotAuthorized
            })
        );
    }

    #[test]
    fn gate_is_not_authorized_on_deny() {
        let mut store = fresh();
        store
            .answer("atelier", Capability::Gyroscope, Answer::Deny, AT)
            .unwrap();
        assert_eq!(
            gate("atelier", &gyro_request(), &store),
            Err(Reply::Error {
                code: DeviceError::NotAuthorized
            })
        );
    }

    #[test]
    fn gate_is_not_authorized_on_revoked() {
        let mut store = fresh();
        store
            .answer("atelier", Capability::Gyroscope, Answer::Allow, AT)
            .unwrap();
        store
            .revoke("atelier", Capability::Gyroscope, AT + 1)
            .unwrap();
        assert_eq!(
            gate("atelier", &gyro_request(), &store),
            Err(Reply::Error {
                code: DeviceError::NotAuthorized
            })
        );
    }

    #[test]
    fn gate_lets_an_allowed_request_through() {
        let mut store = fresh();
        store
            .answer("atelier", Capability::Gyroscope, Answer::Allow, AT)
            .unwrap();
        assert_eq!(
            gate("atelier", &gyro_request(), &store),
            Ok(Capability::Gyroscope)
        );
    }

    #[test]
    fn request_access_queues_the_first_ask_only() {
        let mut store = fresh();
        assert_eq!(
            store.request_access("atelier", &bt_request(), AT),
            Err(Reply::Error {
                code: DeviceError::NotAuthorized
            })
        );
        assert_eq!(
            store.request_access("atelier", &bt_request(), AT + 1),
            Err(Reply::Error {
                code: DeviceError::NotAuthorized
            })
        );
        assert_eq!(store.list_pending().len(), 1);
        assert_eq!(store.list_pending()[0].capability, Capability::Bluetooth);
    }

    #[test]
    fn request_access_does_not_queue_a_refused_pair() {
        let mut store = fresh();
        store
            .answer("atelier", Capability::Bluetooth, Answer::Deny, AT)
            .unwrap();
        let _ = store.request_access("atelier", &bt_request(), AT + 1);
        assert!(store.list_pending().is_empty());
    }

    #[test]
    fn request_access_passes_an_allowed_request() {
        let mut store = fresh();
        store
            .answer("atelier", Capability::Bluetooth, Answer::Allow, AT)
            .unwrap();
        assert_eq!(
            store.request_access("atelier", &bt_request(), AT + 1),
            Ok(Capability::Bluetooth)
        );
        assert!(store.list_pending().is_empty());
    }

    // Lire un état n'est pas lire une donnée : la question ne se pose pas, donc rien n'est
    // mis en file. C'est ce qui rend un diagnostic des 21 capacités utilisable — la version
    // précédente posait 21 questions et remplissait la file de demandes sans objet.
    #[test]
    fn reading_a_status_is_answered_without_queueing_anything() {
        let mut store = fresh();
        for capability in Capability::ALL {
            let status = Request::Status { capability };
            assert_eq!(store.request_access("atelier", &status, AT), Ok(capability));
            assert!(store.list_pending().is_empty(), "{capability:?}");
        }
    }

    #[test]
    fn a_revoked_grant_can_be_asked_again() {
        let mut store = fresh();
        store
            .answer("atelier", Capability::Contacts, Answer::Allow, AT)
            .unwrap();
        store
            .revoke("atelier", Capability::Contacts, AT + 1)
            .unwrap();
        assert_eq!(
            store.decide("atelier", Capability::Contacts),
            Decision::Deny
        );
        // Réversible : la décision est retirée, une question repart.
        store
            .reask("atelier", Capability::Contacts, AT + 2)
            .unwrap();
        assert_eq!(store.decide("atelier", Capability::Contacts), Decision::Ask);
        assert_eq!(store.list_pending().len(), 1);
        // Et la révocation a bien quitté le store persistant.
        let relu = store.store.load().unwrap();
        assert!(
            relu.iter()
                .all(|g| !(g.module == "atelier" && g.capability == Capability::Contacts))
        );
    }

    #[test]
    fn decisions_persist_through_the_store() {
        let mut store = fresh();
        store
            .answer("atelier", Capability::Gyroscope, Answer::Allow, AT)
            .unwrap();
        let saved = store.store.grants.clone();
        assert_eq!(saved.len(), 1);

        let reopened = ConsentStore::open(MemoryStore { grants: saved }).unwrap();
        assert_eq!(
            reopened.decide("atelier", Capability::Gyroscope),
            Decision::Allow
        );
        assert!(
            reopened.list_pending().is_empty(),
            "the queue is not persisted"
        );
    }

    #[test]
    fn open_refuses_duplicate_grants() {
        let grant = Grant {
            module: "atelier".into(),
            capability: Capability::Gyroscope,
            state: GrantState::Granted,
            decided_at_ms: AT,
        };
        let store = MemoryStore {
            grants: vec![grant.clone(), grant],
        };
        assert!(ConsentStore::open(store).is_err());
    }

    #[test]
    fn a_failed_save_changes_nothing() {
        let mut store = ConsentStore::open(FailingStore { grants: vec![] }).unwrap();
        store.enqueue("atelier", Capability::Gyroscope, AT);

        assert!(
            store
                .answer("atelier", Capability::Gyroscope, Answer::Allow, AT + 1)
                .is_err()
        );
        assert_eq!(
            store.decide("atelier", Capability::Gyroscope),
            Decision::Ask
        );
        assert_eq!(store.list_pending().len(), 1, "the question stays asked");

        assert!(
            store
                .revoke("atelier", Capability::Gyroscope, AT + 2)
                .is_err()
        );
        assert!(store.list_grants().is_empty());
    }

    #[test]
    fn grant_serializes_in_snake_case() {
        let grant = Grant {
            module: "atelier".into(),
            capability: Capability::LiveActivity,
            state: GrantState::Revoked,
            decided_at_ms: 7,
        };
        let value = serde_json::to_value(&grant).unwrap();
        assert_eq!(value["capability"], "live_activity");
        assert_eq!(value["state"], "revoked");
    }
}
