//! **L'ordre des couches** — la chaîne d'autorisation vue de bout en bout.
//!
//! Une politique ne sert à rien si elle est évaluée **avant** que le contexte
//! d'authentification soit posé : elle voit un anonyme, et refuse (ou, pire,
//! applique une politique `public` à tout le monde). L'ordre des couches est
//! donc un maillon de la chaîne au même titre que la résolution de route.
//!
//! En axum, **la dernière couche appliquée est la plus externe** : l'ordre
//! d'exécution est l'inverse de l'ordre d'écriture. Ce fichier le vérifie sur
//! l'exposition réelle, avec un fournisseur d'authentification qui authentifie
//! sur un en-tête — de quoi distinguer « le contexte est vu » de « il ne l'est
//! pas ».

use std::sync::Arc;

use async_trait::async_trait;
use axum::Router;
use axum::body::Body;
use axum::http::{HeaderMap, Method, Request, StatusCode};
use tower::ServiceExt;

use this::config::{EntityAuthConfig, EntityConfig, LinksConfig};
use this::core::auth::{AuthContext, AuthProvider};
use this::core::link::LinkDefinition;
use this::core::module::Module;
use this::server::RestExposure;
use this::server::builder::ServerBuilder;
use this::server::entity_registry::{EntityDescriptor, EntityRegistry};
use this::storage::in_memory::InMemoryLinkService;

/// Un fournisseur qui **authentifie sur un en-tête** : sans lui, anonyme.
struct HeaderAuth;

#[async_trait]
impl AuthProvider for HeaderAuth {
    async fn extract_context(&self, headers: &HeaderMap) -> anyhow::Result<AuthContext> {
        if headers.contains_key("x-test-auth") {
            Ok(AuthContext::User {
                user_id: uuid::Uuid::from_u128(1),
                tenant_id: uuid::Uuid::from_u128(1),
                roles: vec![],
                scopes: Vec::new(),
            })
        } else {
            Ok(AuthContext::Anonymous)
        }
    }

    async fn is_owner(&self, _: &uuid::Uuid, _: &uuid::Uuid, _: &str) -> anyhow::Result<bool> {
        Ok(false)
    }

    async fn has_role(&self, _: &uuid::Uuid, _: &str) -> anyhow::Result<bool> {
        Ok(false)
    }
}

struct Orders;

impl EntityDescriptor for Orders {
    fn entity_type(&self) -> &str {
        "order"
    }
    fn plural(&self) -> &str {
        "orders"
    }
    fn build_routes(&self) -> Router {
        Router::new().route("/orders", axum::routing::get(|| async { "[]" }))
    }
}

struct Shop;

impl Module for Shop {
    fn name(&self) -> &str {
        "shop"
    }
    fn entity_types(&self) -> Vec<&str> {
        vec!["order"]
    }
    fn links_config(&self) -> anyhow::Result<LinksConfig> {
        Ok(LinksConfig {
            entities: vec![EntityConfig {
                singular: "order".to_string(),
                plural: "orders".to_string(),
                auth: EntityAuthConfig {
                    // `authenticated` : autorise **tout appelant identifié**,
                    // refuse un anonyme. C'est la politique qui distingue les
                    // deux ordres.
                    list: "authenticated".to_string(),
                    ..Default::default()
                },
            }],
            links: Vec::<LinkDefinition>::new(),
            validation_rules: None,
            events: None,
            sinks: None,
            auth: None,
        })
    }
    fn register_entities(&self, registry: &mut EntityRegistry) {
        registry.register(Box::new(Orders));
    }
}

async fn get(app: Router, authenticated: bool) -> StatusCode {
    let mut request = Request::builder().method(Method::GET).uri("/orders");
    if authenticated {
        request = request.header("x-test-auth", "1");
    }
    app.oneshot(request.body(Body::empty()).unwrap())
        .await
        .expect("réponse")
        .status()
}

/// **Le test qui porte le maillon.** Avec un fournisseur d'authentification
/// installé et les politiques armées, une requête portant un jeton valide doit
/// être **autorisée** — ce qui n'arrive que si la politique est évaluée *après*
/// l'authentification.
#[tokio::test]
async fn the_policy_is_evaluated_after_authentication() {
    let host = ServerBuilder::new()
        .register_module(Shop)
        .expect("module")
        .with_link_service(InMemoryLinkService::new())
        .build_host()
        .expect("hôte")
        .with_auth_provider(Arc::new(HeaderAuth));
    assert!(host.authorize, "les politiques sont armées");
    assert!(
        host.auth_provider.is_some(),
        "un fournisseur d'authentification est installé : c'est la configuration qui \
         révèle l'ordre des couches"
    );

    let app = RestExposure::build_router(Arc::new(host), vec![]).expect("routeur");

    // Sans jeton : anonyme, la politique `authenticated` refuse.
    assert_eq!(
        get(app.clone(), false).await,
        StatusCode::FORBIDDEN,
        "un appelant anonyme doit être refusé"
    );

    // Avec un jeton valide : autorisé — SI la politique voit le contexte.
    assert_eq!(
        get(app, true).await,
        StatusCode::OK,
        "un appelant AUTHENTIFIÉ doit passer `authenticated` : sinon la politique est \
         évaluée avant l'authentification et ne voit qu'un anonyme"
    );
}
