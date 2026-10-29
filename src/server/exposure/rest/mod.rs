//! REST API exposure for the framework
//!
//! This module provides REST-specific routing and handlers.
//! It is isolated from the core framework logic and can be replaced
//! or extended with other protocols (GraphQL, gRPC, etc.)
//!
//! The REST exposure consumes a `ServerHost` and produces an Axum `Router`.

#[cfg(feature = "wami")]
pub mod auth_routes;
pub mod gdpr_routes;
pub mod notifications;
pub mod sse;

use super::super::host::ServerHost;
use crate::links::handlers::AppState;
use crate::server::middleware::auth::{AuthLayer, auth_middleware};
use crate::server::router::build_link_routes;
use anyhow::Result;
use axum::{Json, Router, middleware, routing::get};
use serde_json::{Value, json};
use std::sync::Arc;

/// REST API exposure implementation
///
/// This struct encapsulates all REST-specific logic for exposing the API.
/// It is completely separate from the framework core and can be replaced
/// with other exposure types (GraphQL, gRPC, etc.).
pub struct RestExposure;

impl RestExposure {
    /// Build the REST router from a host
    ///
    /// This method takes a `ServerHost` (which is transport-agnostic) and
    /// builds an Axum router with all REST endpoints.
    ///
    /// # Arguments
    ///
    /// * `host` - The server host containing all framework state
    /// * `custom_routes` - Additional custom routes to merge
    ///
    /// # Returns
    ///
    /// Returns a fully configured Axum router with:
    /// - Health check routes
    /// - Entity CRUD routes
    /// - Link routes
    /// - Custom routes
    pub fn build_router(host: Arc<ServerHost>, custom_routes: Vec<Router>) -> Result<Router> {
        // Create link app state from host
        let link_state = AppState {
            link_service: host.link_service.clone(),
            config: host.config.clone(),
            registry: host.registry.clone(),
            entity_fetchers: host.entity_fetchers.clone(),
            entity_creators: host.entity_creators.clone(),
            event_bus: host.event_bus.clone(),
        };

        // Build all routes
        let health_routes = Self::health_routes();
        let entity_routes = host.entity_registry.build_routes();
        let link_routes = build_link_routes(link_state.clone());

        // Merge everything
        let mut app = health_routes.merge(entity_routes);

        for custom_router in custom_routes {
            app = app.merge(custom_router);
        }

        app = app.merge(link_routes);

        // SSE event stream — only if EventBus is configured
        if let Some(event_bus) = &host.event_bus {
            let sse_routes = Router::new()
                .route("/events/stream", get(sse::sse_handler))
                .with_state(event_bus.clone());
            app = app.merge(sse_routes);
        }

        // Notification, preferences, and device token endpoints
        // — only if notification_store is configured
        if let (Some(notification_store), Some(preferences_store), Some(device_token_store)) = (
            &host.notification_store,
            &host.preferences_store,
            &host.device_token_store,
        ) {
            let notif_state = notifications::NotificationState {
                notification_store: notification_store.clone(),
                preferences_store: preferences_store.clone(),
                device_token_store: device_token_store.clone(),
            };
            app = app.merge(notifications::notification_routes(notif_state));
        }

        // État d'interface : monté SEULEMENT si un store est attaché, comme le
        // relief plus bas. Un handle absent = aucune route `/ui/state`, plutôt
        // qu'une route qui répondrait un état qu'elle ne détient pas.
        // Posé AVANT les couches (CORS, domaine, authentification) : l'ordre
        // compte, c'est ce qui fait que la garde de la route voit un appelant.
        if let Some(ui) = &host.ui_state {
            app = app.merge(crate::ui::routes::ui_routes(
                crate::ui::routes::UiStateCtx { store: ui.clone() },
            ));
        }

        // Auto-wire the self-describing graph relief — only if an obrain store
        // is attached. Absent handle = no `/graph` routes at all: better no
        // relief than a relief that would have to invent what it cannot read.
        #[cfg(feature = "obrain")]
        if let Some(store) = &host.graph_store {
            let mut state =
                crate::graph::routes::GraphState::new(store.clone(), host.identity.clone());

            // **Une politique déclarée rend la lecture filtrée.**
            //
            // Les deux vont ensemble : `graph.policy` dit qui a le droit de lire
            // le graphe ; le périmètre, adossé à l'IAM, dit **ce que** ce
            // lecteur voit — label par label, type de relation par type, échelle
            // par échelle. Déclarer l'un sans brancher l'autre laisserait un
            // appelant autorisé lire la base entière.
            if host.security.graph.armed() {
                let mut resolver = crate::graph::scope::IamScopeResolver::new(store.iam());
                if let Some(mapping) = &host.principal_mapping {
                    resolver = resolver.with_mapping(mapping.clone());
                }
                state = state.with_scope_resolver(std::sync::Arc::new(resolver));
                tracing::info!(
                    "lectures de graphe : politique + périmètre IAM (ce que le demandeur peut lire \
                     décide de ce qu'il voit)"
                );
            }

            app = app.merge(crate::graph::routes::graph_routes(state));
            tracing::info!("graph relief auto-wired (GET /graph)");
        }

        // Auto-wire GDPR erasure endpoint if configured
        let gdpr_enabled = host
            .config
            .auth
            .as_ref()
            .map(|a| a.gdpr.erasure_cascade)
            .unwrap_or(false);

        if gdpr_enabled {
            let gdpr_state = std::sync::Arc::new(gdpr_routes::GdprState {
                notification_store: host.notification_store.clone().unwrap_or_else(|| {
                    std::sync::Arc::new(crate::events::sinks::in_app::NotificationStore::new())
                }),
                link_service: Some(host.link_service.clone()),
                sink_registry: host.sink_registry.clone(),
                event_bus: host.event_bus.clone(),
            });
            app = app.merge(gdpr_routes::gdpr_routes(gdpr_state));
            tracing::info!("GDPR erasure endpoint auto-wired (DELETE /tenants/:tenant_id/data)");
        }

        // Auto-wire STS auth endpoints if STS is configured (embedded/bootstrap mode)
        #[cfg(feature = "wami")]
        if let Some(ref sts_state) = host.sts_state {
            let auth_routes = auth_routes::auth_router(sts_state.clone());
            app = app.merge(auth_routes);
            tracing::info!(
                "STS auth endpoints auto-wired (/auth/token, /auth/keys, /auth/refresh, /auth/revoke)"
            );
        }

        // **Le repli multi-sites, monté AVANT les couches.** Posé ici, il est
        // couvert par le CORS, la garde de domaine, l'authentification et les
        // politiques ; posé par l'application APRÈS elles, il les contournerait
        // (mesuré : `tests/frontend_surface.rs`). C'est la seule façon de
        // composer proprement « N domaines, N contenus, un jeu de couches ».
        #[cfg(feature = "frontend")]
        if let Some(sites) = &host.frontend_sites {
            app = app.fallback_service(sites.router());
            tracing::info!(
                sites = sites.len(),
                "repli multi-sites monté (les couches s'appliquent aussi au repli)"
            );
        }

        // ═══════════════════════════════════════════════════════════════════
        // L'ORDRE DES COUCHES EST UN MAILLON DE LA CHAÎNE
        //
        // En axum, **la dernière couche appliquée est la plus EXTERNE** : l'ordre
        // d'exécution est l'inverse de l'ordre d'écriture. Ici, l'ordre voulu est
        //
        //   CORS → domaine servi → authentification → locataire → politiques
        //
        // donc on applique dans l'ordre inverse : politiques d'abord, CORS en
        // dernier.
        //
        // Ce n'était pas le cas : les politiques étaient appliquées EN DERNIER,
        // donc exécutées EN PREMIER — **avant l'authentification**. Elles ne
        // voyaient qu'un anonyme, et toute route non publique était refusée à un
        // appelant pourtant porteur d'un jeton valide. Fail-closed, donc pas une
        // fuite, mais une application cassée qui accusait ses politiques.
        // (`tests/chain_order.rs` le prouve sur l'exposition réelle.)
        // ═══════════════════════════════════════════════════════════════════

        // Application des politiques : **ARMÉE PAR DÉFAUT** (`ServerHost::authorize`
        // vaut `true` — `server/host.rs`), désarmable explicitement
        // (`ServerBuilder::without_authorization`, journalisé comme un
        // avertissement).
        //
        // Voici pourquoi : depuis que l'abstention est un refus (2026-09-13),
        // une couche armée refuse tout ce qu'elle ne sait pas juger — y compris
        // les lectures de graphe sans `graph.policy`. Ce commentaire disait
        // « armée explicitement, jamais par défaut » : c'était faux, et la
        // contradiction entre le commentaire et le champ a coûté cher (des
        // tests écrits sur le contrat d'avant). L'état réel est celui du champ,
        // pas d'une phrase.
        if host.authorize {
            let mut authz = crate::server::middleware::authorization::Authorization::new(
                host.config.clone(),
                &host.entity_registry,
                host.resolver_registry.clone(),
            );
            // Avec un store obrain, les politiques `perm:<permission>` sont
            // tranchées par la **décision IAM matérialisée** (une lecture
            // indexée), au lieu d'être recalculées par traversée.
            // **Politique des lectures de graphe** : quand l'application en
            // déclare une, le relief, la vue multi-échelle et l'identité cessent
            // de s'abstenir — un moteur d'inférence doit prouver son droit.
            if let Some(policy) = &host.security.graph.policy
                && host.security.graph.armed()
            {
                authz = authz.with_graph_policy(policy.clone());
                tracing::info!(
                    policy = %policy,
                    "lectures de graphe soumises à une politique (relief, multi-échelle, identité)"
                );
            }

            // Mode observation : décider, journaliser, publier — sans bloquer.
            if host.authorize_report_only {
                authz = authz.report_only();
            }
            // Mappage d'identité de l'application (IAM natif d'un modèle existant).
            if let Some(mapping) = host.principal_mapping.clone() {
                authz = authz.with_principal_mapping_shared(mapping);
                tracing::info!("authorization: mappage d'identité fourni par l'application");
            }

            #[cfg(feature = "obrain")]
            let authz = match &host.graph_store {
                Some(store) => {
                    tracing::info!("authorization: IAM matérialisé branché (politiques `perm:`)");
                    authz.with_iam(store)
                }
                None => authz,
            };
            app = app.layer(middleware::from_fn_with_state(
                authz,
                crate::server::middleware::authorization::authorization_middleware,
            ));
            tracing::info!(
                "authorization armed — declared `auth:` policies are now ENFORCED \
                 (403 on refusal; abstention denied outside declared public paths)"
            );
        }

        // Runtime modules (modules-runtime stream): dispatch of hot-loaded routes, then the
        // admin interface. OUTSIDE the policy layer above (its entity table is frozen at
        // build time: it would refuse every hot-loaded path by abstention); they judge
        // their requests themselves with the descriptors' policies. INSIDE
        // authentication, domain guard and CORS below, which still apply.
        // Mounted BEFORE routing (see `ModuleRuntime::mount`).
        if let Some(runtime) = &host.module_runtime {
            app = runtime.mount(app);
            tracing::info!("module runtime mounted (dispatch layer + admin interface)");
        }

        // Auto-wire auth middleware if auth provider is configured
        // This layer must be added last so it wraps all routes
        if let Some(ref auth_provider) = host.auth_provider {
            let default_policy = host
                .config
                .auth
                .as_ref()
                .map(|a| a.default_policy.clone())
                .unwrap_or_else(|| "public".to_string());

            // Tenant resolver runs AFTER auth (inner layer = added first)
            // so AuthContext is available in extensions
            app = app.layer(middleware::from_fn(
                crate::server::middleware::tenant::tenant_resolver_middleware,
            ));

            let auth_layer = AuthLayer::new(auth_provider.clone(), default_policy);
            app = app.layer(middleware::from_fn_with_state(auth_layer, auth_middleware));
            tracing::info!("REST auth + tenant resolver middleware auto-wired");
        }

        // Domaine servi : refusé AVANT l'authentification — un hôte qui n'est pas
        // servi n'a pas à coûter une vérification de jeton — et le locataire du
        // domaine est posé ici, pour que le résolveur ne l'écrase pas.
        let domains = &host.security.domains;
        if domains.guards()
            || domains.subdomain_base.is_some()
            || !domains.tenants.is_empty()
            || domains.default_tenant.is_some()
        {
            app = app.layer(middleware::from_fn_with_state(
                crate::server::middleware::origin::DomainGuard::new(std::sync::Arc::new(
                    domains.clone(),
                )),
                crate::server::middleware::origin::domain_guard_middleware,
            ));
            tracing::info!(
                allowed = ?domains.allowed,
                "garde de domaine armée (les autres hôtes sont refusés en 421)"
            );
        }

        // CORS : la couche la plus EXTERNE. Un pré-vol (`OPTIONS`) ne porte pas
        // de jeton : s'il rencontrait l'authentification d'abord, il recevrait
        // 401 et le navigateur refuserait un appel légitime.
        if let Some(cors) = crate::server::middleware::origin::cors_layer(&host.security.cors) {
            app = app.layer(cors);
            tracing::info!(
                origins = ?host.security.cors.origins,
                credentials = host.security.cors.credentials,
                "CORS configuré par l'application"
            );
        }

        Ok(app)
    }

    /// Build health check routes
    fn health_routes() -> Router {
        Router::new()
            .route("/health", get(Self::health_check))
            .route("/healthz", get(Self::health_check))
    }

    /// Health check endpoint handler
    async fn health_check() -> Json<Value> {
        Json(json!({
            "status": "ok",
            "service": "this-rs"
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::LinksConfig;
    use crate::server::entity_registry::EntityRegistry;
    use crate::server::host::ServerHost;
    use crate::storage::InMemoryLinkService;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use std::collections::HashMap;
    use tower::ServiceExt;

    /// Build a minimal ServerHost for testing
    fn test_host() -> Arc<ServerHost> {
        let host = ServerHost::from_builder_components(
            Arc::new(InMemoryLinkService::new()),
            LinksConfig::default_config(),
            EntityRegistry::new(),
            HashMap::new(),
            HashMap::new(),
        )
        .expect("should build host");
        Arc::new(host)
    }

    #[test]
    fn test_health_routes_builds_router() {
        let router = RestExposure::health_routes();
        let _ = router;
    }

    #[tokio::test]
    async fn test_health_endpoint_returns_ok() {
        let router = RestExposure::health_routes();
        let response = router
            .oneshot(
                Request::builder()
                    .uri("/health")
                    .body(Body::empty())
                    .expect("request should build"),
            )
            .await
            .expect("response should succeed");
        assert_eq!(response.status(), StatusCode::OK);

        let body = axum::body::to_bytes(response.into_body(), 1024)
            .await
            .expect("body should read");
        let json: serde_json::Value =
            serde_json::from_slice(&body).expect("body should be valid JSON");
        assert_eq!(json["status"], "ok");
        assert_eq!(json["service"], "this-rs");
    }

    #[tokio::test]
    async fn test_healthz_endpoint_returns_ok() {
        let router = RestExposure::health_routes();
        let response = router
            .oneshot(
                Request::builder()
                    .uri("/healthz")
                    .body(Body::empty())
                    .expect("request should build"),
            )
            .await
            .expect("response should succeed");
        assert_eq!(response.status(), StatusCode::OK);

        let body = axum::body::to_bytes(response.into_body(), 1024)
            .await
            .expect("body should read");
        let json: serde_json::Value =
            serde_json::from_slice(&body).expect("body should be valid JSON");
        assert_eq!(json["status"], "ok");
    }

    #[test]
    fn test_build_router_succeeds_with_host() {
        let host = test_host();
        let router = RestExposure::build_router(host, vec![]);
        assert!(router.is_ok());
    }

    #[test]
    fn test_build_router_with_custom_routes() {
        use axum::routing::get;

        let host = test_host();
        let custom = Router::new().route("/custom", get(|| async { "custom" }));
        let router = RestExposure::build_router(host, vec![custom]);
        assert!(router.is_ok());
    }

    #[tokio::test]
    async fn test_build_router_health_endpoint_reachable() {
        let host = test_host();
        let router = RestExposure::build_router(host, vec![]).expect("build should succeed");
        let response = router
            .oneshot(
                Request::builder()
                    .uri("/health")
                    .body(Body::empty())
                    .expect("request should build"),
            )
            .await
            .expect("response should succeed");
        assert_eq!(response.status(), StatusCode::OK);
    }
}
