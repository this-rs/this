//! Les routes de l'état d'interface : LIRE et MODIFIER un document par portée.
//!
//! Motif imité : `server/exposure/rest/notifications.rs` (état `Clone`, une
//! seule couche de garde sur tout le routeur, `{"…": document}` en réponse).

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::response::IntoResponse;
use axum::routing::get;
use axum::{Json, Router};
use serde_json::json;

use super::state::{UI_SCHEMA, UiState, UiStateStore};

/// État partagé des routes d'interface — `Clone`, comme `NotificationState`.
#[derive(Clone)]
pub struct UiStateCtx {
    pub store: Arc<UiStateStore>,
}

/// Construit le routeur de l'état d'interface.
///
/// **Une seule garde pour tout le routeur**, exactement comme les routes de
/// notifications : `/ui/state/{portee}` n'est pas une route d'entité, donc le
/// résolveur de politiques s'abstient dessus. Sans cette couche, la portée du
/// chemin serait crue sur parole — n'importe quel appelant pourrait lire ou
/// réécrire l'interface d'un autre. La garde est celle qui existe déjà
/// (`ensure_self_middleware`, rendue réutilisable et étendue d'un segment) :
/// dupliquer une garde de sécurité est la façon la plus sûre d'en laisser une
/// trouée.
pub fn ui_routes(ctx: UiStateCtx) -> Router {
    Router::new()
        .route("/ui/state/{portee}", get(lire_etat).put(ecrire_etat))
        .layer(axum::middleware::from_fn(
            crate::server::exposure::rest::notifications::ensure_self_middleware,
        ))
        .with_state(ctx)
}

/// `GET /ui/state/{portee}` — le document, ou le défaut s'il n'a jamais été écrit.
async fn lire_etat(State(ctx): State<UiStateCtx>, Path(portee): Path<String>) -> impl IntoResponse {
    let etat = ctx.store.get(&portee).await;
    Json(json!({
        "schema": UI_SCHEMA,
        "portee": portee,
        "ui": etat,
    }))
}

/// `PUT /ui/state/{portee}` — remplace le document et rend ce qui a été écrit.
///
/// Rendre le document ÉCRIT (et non un accusé vide) est délibéré : le client
/// peut ainsi constater l'état réellement retenu — un champ qu'il n'a pas su
/// remplir se relit comme défaut au lieu d'être supposé inchangé.
async fn ecrire_etat(
    State(ctx): State<UiStateCtx>,
    Path(portee): Path<String>,
    Json(etat): Json<UiState>,
) -> impl IntoResponse {
    let ecrit = ctx.store.update(portee.clone(), etat).await;
    Json(json!({
        "schema": UI_SCHEMA,
        "portee": portee,
        "ui": ecrit,
    }))
}
