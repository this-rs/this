//! Contrat : **l'abstention du middleware d'autorisation est un refus.**
//!
//! Écrit AVANT le correctif (chantier PASSERELLE-THIS-RS). La faille
//! constatée : une route que le résolveur ne reconnaît pas — chemin
//! lien/nested à 4 segments d'un pluriel non enregistré, forme non reconnue —
//! produisait `Decision::Abstain`, et le middleware **laissait passer**
//! (`Allow | Abstain => next.run(...)`). Un service qui arme ses politiques
//! était donc traversable par toute route hors des formes reconnues.
//!
//! Le contrat, après correctif :
//! - une abstention hors chemin public déclaré ⇒ **403**, même pour un
//!   principal authentifié (ce n'est pas un problème d'identité : la route
//!   n'a pas de garde, donc elle ne passe pas) ;
//! - les chemins publics sont une liste **explicite** (`/health`, `/healthz`,
//!   préfixe `/auth/` par défaut ; remplaçable par `with_public_paths`) —
//!   jamais une abstention silencieuse ;
//! - les routes **jugées** (entités déclarées, politique graphe) ne changent
//!   pas de comportement ;
//! - le mode observation (`report_only`) continue d'observer : l'abstention
//!   passe, mais la réponse le DIT (`x-this-rs-authz: observed-deny-abstain`).

use std::sync::Arc;

use axum::Router;
use axum::body::Body;
use axum::extract::Request;
use axum::http::{Method, StatusCode};
use axum::middleware::{self, Next};
use axum::response::Response;
use axum::routing::{get, post};
use tower::ServiceExt;
use uuid::Uuid;

use this::config::{EntityAuthConfig, EntityConfig, LinksConfig};
use this::core::auth::{AuthContext, AuthResolverRegistry};
use this::server::entity_registry::{EntityDescriptor, EntityRegistry};
use this::server::middleware::authorization::{Authorization, authorization_middleware};

// ── Le banc ──────────────────────────────────────────────────────────────

struct Descriptor;

impl EntityDescriptor for Descriptor {
    fn entity_type(&self) -> &str {
        "order"
    }
    fn plural(&self) -> &str {
        "orders"
    }
    fn build_routes(&self) -> Router {
        Router::new()
    }
}

/// Un point d'application armé avec UNE entité déclarée (`orders`, liste
/// publique) — tout le reste du monde est inconnu du résolveur.
fn authz() -> Authorization {
    let config = LinksConfig {
        entities: vec![EntityConfig {
            singular: "order".to_string(),
            plural: "orders".to_string(),
            auth: EntityAuthConfig {
                list: "public".to_string(),
                ..Default::default()
            },
        }],
        links: Vec::new(),
        validation_rules: None,
        events: None,
        sinks: None,
        auth: None,
    };
    let mut registry = EntityRegistry::new();
    registry.register(Box::new(Descriptor));
    Authorization::new(
        Arc::new(config),
        &registry,
        Arc::new(AuthResolverRegistry::new()),
    )
}

/// Injecte un principal authentifié — pour prouver que le refus d'abstention
/// n'est PAS une affaire d'identité.
async fn inject_user(mut request: Request, next: Next) -> Response {
    request.extensions_mut().insert(AuthContext::User {
        user_id: Uuid::new_v4(),
        tenant_id: Uuid::new_v4(),
        roles: vec!["admin".to_string()],
    });
    next.run(request).await
}

/// Le routeur d'essai : un handler qui répond 200 partout où le middleware
/// laisse passer (c'est le « store nu » derrière la passerelle).
fn app(authz: Authorization, authenticated: bool) -> Router {
    let mut router = Router::new()
        .route("/health", get(|| async { "ok" }))
        .route("/healthz", get(|| async { "ok" }))
        .route("/auth/token", post(|| async { "token" }))
        .route("/sonde", get(|| async { "ok" }))
        .fallback(|| async { "atteint" })
        .layer(middleware::from_fn_with_state(
            authz,
            authorization_middleware,
        ));
    if authenticated {
        router = router.layer(middleware::from_fn(inject_user));
    }
    router
}

async fn call(router: Router, method: Method, path: &str) -> Response {
    router
        .oneshot(
            Request::builder()
                .method(method)
                .uri(path)
                .body(Body::empty())
                .expect("requête d'essai"),
        )
        .await
        .expect("réponse du routeur")
}

// ── 1. La mort de la faille 4-segments ──────────────────────────────────

/// LE test : un chemin lien/nested dont le pluriel n'est pas enregistré
/// (4 segments) s'abstenait et PASSAIT. Il doit être refusé — même porté par
/// un principal authentifié.
#[tokio::test]
async fn un_chemin_quatre_segments_sans_garde_est_refuse() {
    let response = call(
        app(authz(), true),
        Method::GET,
        "/inconnus/1/liens/cibles",
    )
    .await;
    assert_eq!(
        response.status(),
        StatusCode::FORBIDDEN,
        "un chemin sans garde explicite ne doit JAMAIS passer (la faille 4-segments)"
    );
    assert_eq!(
        response
            .headers()
            .get("x-this-rs-authz")
            .and_then(|v| v.to_str().ok()),
        Some("deny-abstain"),
        "le refus d'abstention se déclare (l'opérateur doit voir pourquoi)"
    );
}

/// La même chose en anonyme : refus aussi (fail-closed des deux côtés).
#[tokio::test]
async fn un_chemin_quatre_segments_anonyme_est_refuse() {
    let response = call(
        app(authz(), false),
        Method::GET,
        "/inconnus/1/liens/cibles",
    )
    .await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

/// La forme non reconnue (PUT à 5 segments) s'abstenait aussi : refusée.
#[tokio::test]
async fn une_forme_non_reconnue_est_refusee() {
    let response = call(
        app(authz(), true),
        Method::PUT,
        "/orders/1/liens/cibles/2/profond",
    )
    .await;
    assert_eq!(
        response.status(),
        StatusCode::FORBIDDEN,
        "une forme que le résolveur ne reconnaît pas ne passe pas, même sur une entité connue"
    );
}

// ── 2. Les chemins publics sont EXPLICITES ──────────────────────────────

/// `/health` et `/healthz` restent vivants — par déclaration, plus par
/// abstention silencieuse.
#[tokio::test]
async fn la_liveness_reste_publique_par_declaration() {
    for path in ["/health", "/healthz"] {
        let response = call(app(authz(), false), Method::GET, path).await;
        assert_eq!(
            response.status(),
            StatusCode::OK,
            "{path} doit rester atteignable sans principal (liste publique explicite)"
        );
    }
}

/// L'émission de jetons (`/auth/…`) doit rester atteignable sans jeton —
/// sinon personne ne peut jamais s'authentifier.
#[tokio::test]
async fn l_emission_de_jetons_reste_atteignable() {
    let response = call(app(authz(), false), Method::POST, "/auth/token").await;
    assert_eq!(response.status(), StatusCode::OK);
}

/// La liste publique est remplaçable, et le remplacement est TOTAL : ce qui
/// n'y figure plus est refusé.
#[tokio::test]
async fn la_liste_publique_est_remplacable() {
    let authz = authz().with_public_paths(vec!["/sonde".to_string()]);
    let sonde = call(app(authz.clone(), false), Method::GET, "/sonde").await;
    assert_eq!(sonde.status(), StatusCode::OK, "le chemin déclaré passe");

    let health = call(app(authz, false), Method::GET, "/health").await;
    assert_eq!(
        health.status(),
        StatusCode::FORBIDDEN,
        "un chemin retiré de la liste publique ne vit plus d'abstention"
    );
}

// ── 3. Les routes jugées ne changent pas ────────────────────────────────

/// Une entité déclarée publique répond comme avant : le correctif durcit
/// l'abstention, pas les décisions.
#[tokio::test]
async fn une_route_jugee_publique_repond_comme_avant() {
    let response = call(app(authz(), false), Method::GET, "/orders").await;
    assert_eq!(response.status(), StatusCode::OK);
}

// ── 4. Le mode observation observe encore ───────────────────────────────

/// En `report_only`, l'abstention passe mais se DIT : c'est le chemin
/// d'adoption d'un déploiement existant (on lit les journaux, puis on arme).
#[tokio::test]
async fn en_observation_l_abstention_passe_mais_se_dit() {
    let authz = authz().report_only();
    let response = call(app(authz, true), Method::GET, "/inconnus/1/liens/cibles").await;
    assert_eq!(
        response.status(),
        StatusCode::OK,
        "le mode observation ne bloque pas"
    );
    assert_eq!(
        response
            .headers()
            .get("x-this-rs-authz")
            .and_then(|v| v.to_str().ok()),
        Some("observed-deny-abstain"),
        "mais la réponse porte l'observation"
    );
}
