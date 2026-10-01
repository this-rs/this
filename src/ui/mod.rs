//! ÉTAT D'INTERFACE — le document que le tableau de bord LIT et MODIFIE
//!
//! Ce module expose une seule chose : le document d'état d'interface d'une
//! portée (une session, un utilisateur, un poste). Il ne touche PAS au store
//! obrain, et c'est délibéré : `docs/integration-consommateurs.md` §4 rappelle
//! qu'« un tableau de bord doit ouvrir le store en lecture seule », parce que
//! *qui peut écrire dans le store peut écrire une décision*. Mêler l'état
//! d'affichage aux décisions d'autorisation serait exactement la faute que ce
//! document interdit.
//!
//! Le motif imité est celui des préférences de notification
//! (`events/sinks/preferences.rs`) : un document `serde`, un store
//! `RwLock<HashMap<..>>` qui rend un DÉFAUT quand la portée est inconnue, et des
//! routes `GET`/`PUT` montées conditionnellement dans `RestExposure`, derrière
//! une garde fail-closed.

pub mod routes;
pub mod state;

pub use state::{UI_SCHEMA, UiState, UiStateStore, UiSubject};
