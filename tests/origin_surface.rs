//! **Domaines, CORS et délégation de proxy** — vérifiés sur le routeur réel.
//!
//! Trois réglages qui répondent à la même question : *qui atteint cette
//! application, et par où*. Ce fichier vérifie chacun là où il compte :
//!
//! * un **hôte non servi** est refusé (421), y compris avec un `Host` forgé ;
//! * `X-Forwarded-Host` n'est cru que si l'application l'a demandé ;
//! * un **pré-vol CORS** passe **sans jeton** — c'est tout l'intérêt de poser la
//!   couche CORS à l'extérieur de l'authentification ;
//! * une origine non autorisée n'obtient **aucun** en-tête ;
//! * un domaine peut désigner le **locataire** ;
//! * le **Caddyfile** est rendu depuis la configuration, et reste une
//!   description que l'exploitant applique.

use std::sync::Arc;

use axum::Router;
use axum::body::Body;
use axum::extract::Extension;
use axum::http::{Method, Request, StatusCode};
use axum::routing::get;
use tower::ServiceExt;

use this::config::{
    CaddyConfig, CaddyTls, CorsConfig, DomainConfig, ProxyConfig, ServerSecurityConfig,
};
use this::server::RestExposure;
use this::server::builder::ServerBuilder;
use this::server::middleware::origin::DomainContext;

/// Une route qui publie ce que le garde de domaine a retenu.
///
/// Le contexte est **optionnel** : sans garde configurée, il n'existe pas — et
/// c'est le comportement voulu (le framework ne pose un contexte de domaine que
/// lorsqu'une configuration en parle).
async fn whoami(domain: Option<Extension<DomainContext>>) -> String {
    match domain {
        Some(Extension(domain)) => format!(
            "{}|{}",
            domain.host,
            domain
                .tenant
                .map(|t| t.to_string())
                .unwrap_or_else(|| "aucun".to_string())
        ),
        None => "aucun-domaine|aucun".to_string(),
    }
}

fn app(security: ServerSecurityConfig) -> Router {
    let host = ServerBuilder::new()
        .with_server_security(security)
        .with_link_service(this::storage::in_memory::InMemoryLinkService::new())
        .build_host()
        .expect("hôte");
    RestExposure::build_router(
        Arc::new(host),
        vec![Router::new().route("/whoami", get(whoami))],
    )
    .expect("routeur")
}

async fn call(
    app: Router,
    method: Method,
    uri: &str,
    headers: &[(&str, &str)],
) -> (StatusCode, String) {
    let mut request = Request::builder().method(method).uri(uri);
    for (name, value) in headers {
        request = request.header(*name, *value);
    }
    let response = app
        .oneshot(request.body(Body::empty()).unwrap())
        .await
        .expect("réponse");
    let status = response.status();
    let body = axum::body::to_bytes(response.into_body(), 4096)
        .await
        .expect("corps");
    (status, String::from_utf8_lossy(&body).to_string())
}

async fn cors_headers(app: Router, origin: &str) -> axum::http::HeaderMap {
    let (_, _, headers) = cors_preflight(app, origin).await;
    headers
}

/// Le pré-vol, avec son statut (utile pour diagnostiquer).
async fn cors_preflight(app: Router, origin: &str) -> (StatusCode, String, axum::http::HeaderMap) {
    let response = app
        .oneshot(
            Request::builder()
                .method(Method::OPTIONS)
                .uri("/whoami")
                .header("origin", origin)
                .header("access-control-request-method", "GET")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .expect("réponse");
    let status = response.status();
    let headers = response.headers().clone();
    let body = axum::body::to_bytes(response.into_body(), 4096)
        .await
        .expect("corps");
    (status, String::from_utf8_lossy(&body).to_string(), headers)
}

// ===========================================================================
// 1. DOMAINES
// ===========================================================================

/// Sans liste configurée, **rien ne change** : le framework ne devine pas les
/// domaines d'un déploiement.
#[tokio::test]
async fn without_a_domain_list_every_host_is_served_as_before() {
    let app = app(ServerSecurityConfig::default());
    for host in ["app.exemple.fr", "n-importe-quoi", "127.0.0.1:8080"] {
        let (status, _) = call(app.clone(), Method::GET, "/whoami", &[("host", host)]).await;
        assert_eq!(status, StatusCode::OK, "`{host}` doit passer sans garde");
    }
}

/// Une liste configurée est **fermée** : tout autre hôte est refusé, et la
/// réponse dit pourquoi.
#[tokio::test]
async fn an_unserved_host_is_refused() {
    let app = app(ServerSecurityConfig {
        domains: DomainConfig {
            allowed: vec!["app.exemple.fr".into(), "*.interne.exemple.fr".into()],
            ..Default::default()
        },
        ..Default::default()
    });

    let (status, body) = call(
        app.clone(),
        Method::GET,
        "/whoami",
        &[("host", "app.exemple.fr")],
    )
    .await;
    assert_eq!(status, StatusCode::OK, "un hôte servi passe");
    assert!(body.starts_with("app.exemple.fr|"), "{body}");

    let (status, body) = call(
        app.clone(),
        Method::GET,
        "/whoami",
        &[("host", "evil.example.fr")],
    )
    .await;
    assert_eq!(
        status,
        StatusCode::MISDIRECTED_REQUEST,
        "un hôte non servi doit être refusé en 421 : {body}"
    );
    assert!(body.contains("misdirected"), "{body}");

    // Le joker d'un niveau, et un host forgé par suffixe.
    assert_eq!(
        call(
            app.clone(),
            Method::GET,
            "/whoami",
            &[("host", "a.interne.exemple.fr")]
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        call(
            app.clone(),
            Method::GET,
            "/whoami",
            &[("host", "a.b.interne.exemple.fr")]
        )
        .await
        .0,
        StatusCode::MISDIRECTED_REQUEST,
        "le joker ne couvre qu'un niveau"
    );
    assert_eq!(
        call(
            app,
            Method::GET,
            "/whoami",
            &[("host", "app.exemple.fr.evil.fr")]
        )
        .await
        .0,
        StatusCode::MISDIRECTED_REQUEST,
        "un suffixe trompeur n'est pas un hôte servi"
    );
}

/// `X-Forwarded-Host` n'est cru que si l'application l'a demandé : sinon, c'est
/// le client qui choisit l'hôte que le framework croit servir.
#[tokio::test]
async fn the_forwarded_host_is_trusted_only_when_configured() {
    let domains = |trust: bool| ServerSecurityConfig {
        domains: DomainConfig {
            allowed: vec!["app.exemple.fr".into()],
            trust_forwarded_host: trust,
            ..Default::default()
        },
        ..Default::default()
    };

    // Sans confiance : l'en-tête est ignoré, l'hôte réel est servi.
    let (status, body) = call(
        app(domains(false)),
        Method::GET,
        "/whoami",
        &[
            ("host", "app.exemple.fr"),
            ("x-forwarded-host", "forge.evil.fr"),
        ],
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.starts_with("app.exemple.fr|"), "{body}");

    // Avec confiance (délégation à Caddy) : l'en-tête décide — donc un hôte
    // forgé est refusé, ce qui est le comportement voulu d'un proxy de confiance
    // qui REMPLACE l'en-tête du client.
    let (status, _) = call(
        app(domains(true)),
        Method::GET,
        "/whoami",
        &[
            ("host", "app.exemple.fr"),
            ("x-forwarded-host", "forge.evil.fr"),
        ],
    )
    .await;
    assert_eq!(
        status,
        StatusCode::MISDIRECTED_REQUEST,
        "avec confiance, c'est `X-Forwarded-Host` qui est servi — et il est filtré"
    );
}

/// Un domaine peut **désigner le locataire** : c'est le multi-tenant par domaine.
#[tokio::test]
async fn a_domain_designates_its_tenant() {
    let tenant = uuid::Uuid::from_u128(0xabc);
    let app = app(ServerSecurityConfig {
        domains: DomainConfig {
            // **L'application liste explicitement** les hôtes qu'elle sert, y
            // compris le joker des sous-domaines : un mappage de locataire
            // n'élargit JAMAIS la liste des hôtes servis.
            allowed: vec![
                "app.exemple.fr".into(),
                "multi.exemple.fr".into(),
                "*.multi.exemple.fr".into(),
            ],
            tenants: [("app.exemple.fr".to_string(), tenant.to_string())]
                .into_iter()
                .collect(),
            subdomain_base: Some("multi.exemple.fr".into()),
            ..Default::default()
        },
        ..Default::default()
    });

    let (_, body) = call(
        app.clone(),
        Method::GET,
        "/whoami",
        &[("host", "app.exemple.fr")],
    )
    .await;
    assert!(
        body.ends_with(&tenant.to_string()),
        "le domaine doit désigner le locataire : {body}"
    );

    let sub = uuid::Uuid::from_u128(0xdef);
    let (_, body) = call(
        app.clone(),
        Method::GET,
        "/whoami",
        &[("host", &format!("{sub}.multi.exemple.fr"))],
    )
    .await;
    assert!(
        body.ends_with(&sub.to_string()),
        "sous-domaine porteur : {body}"
    );

    let (_, body) = call(app, Method::GET, "/whoami", &[("host", "multi.exemple.fr")]).await;
    assert!(
        body.ends_with("aucun"),
        "sans locataire dans l'hôte, il n'y en a pas : {body}"
    );
}

// ===========================================================================
// 2. CORS
// ===========================================================================

/// Un pré-vol passe **sans jeton** : c'est la raison pour laquelle la couche CORS
/// est posée à l'extérieur de l'authentification. Un navigateur n'envoie pas
/// d'en-tête `Authorization` sur un `OPTIONS`.
#[tokio::test]
async fn a_preflight_passes_without_a_token_even_when_authentication_is_installed() {
    use this::core::auth::{AuthContext, AuthProvider};

    struct RefuseEverything;

    #[async_trait::async_trait]
    impl AuthProvider for RefuseEverything {
        async fn extract_context(
            &self,
            _headers: &axum::http::HeaderMap,
        ) -> anyhow::Result<AuthContext> {
            anyhow::bail!("aucun jeton présenté")
        }
        async fn is_owner(&self, _: &uuid::Uuid, _: &uuid::Uuid, _: &str) -> anyhow::Result<bool> {
            Ok(false)
        }
        async fn has_role(&self, _: &uuid::Uuid, _: &str) -> anyhow::Result<bool> {
            Ok(false)
        }
    }

    let host = ServerBuilder::new()
        .with_server_security(ServerSecurityConfig {
            cors: CorsConfig {
                enabled: true,
                origins: vec!["https://app.exemple.fr".into()],
                ..Default::default()
            },
            ..Default::default()
        })
        .with_link_service(this::storage::in_memory::InMemoryLinkService::new())
        .build_host()
        .expect("hôte")
        // Un fournisseur qui REFUSE tout : sans la couche CORS à l'extérieur, le
        // pré-vol recevrait 401 et le navigateur bloquerait un appel légitime.
        .with_auth_provider(Arc::new(RefuseEverything));

    let app = RestExposure::build_router(Arc::new(host), vec![]).expect("routeur");
    let (status, body, headers) = cors_preflight(app, "https://app.exemple.fr").await;
    println!("  pre-vol : {status} | {body} | {headers:?}");

    assert_eq!(
        headers
            .get("access-control-allow-origin")
            .map(|v| v.to_str().unwrap()),
        Some("https://app.exemple.fr"),
        "le pré-vol doit être autorisé, origine en clair"
    );
    assert!(
        headers.contains_key("access-control-allow-methods"),
        "les méthodes autorisées sont annoncées"
    );
}

/// Une origine **non autorisée** n'obtient aucun en-tête : le navigateur bloque,
/// et c'est le seul comportement sûr.
#[tokio::test]
async fn a_disallowed_origin_gets_no_cors_header() {
    let app = app(ServerSecurityConfig {
        cors: CorsConfig {
            enabled: true,
            origins: vec!["https://app.exemple.fr".into()],
            ..Default::default()
        },
        ..Default::default()
    });

    let headers = cors_headers(app.clone(), "https://attaque.example.fr").await;
    assert!(
        !headers.contains_key("access-control-allow-origin"),
        "aucune origine non listée ne doit obtenir d'en-tête"
    );

    // Et l'origine autorisée, elle, l'obtient.
    let headers = cors_headers(app, "https://app.exemple.fr").await;
    assert!(headers.contains_key("access-control-allow-origin"));
}

/// Sans `cors.enabled`, **aucun** en-tête CORS n'est posé : le framework ne
/// décide pas à la place de l'application.
#[tokio::test]
async fn without_configuration_no_cors_header_is_ever_set() {
    let app = app(ServerSecurityConfig::default());
    let headers = cors_headers(app, "https://app.exemple.fr").await;
    assert!(
        !headers.contains_key("access-control-allow-origin"),
        "rien par défaut"
    );
}

/// Une configuration CORS **impossible** échoue à la construction, pas au
/// premier appel.
#[tokio::test]
async fn an_impossible_cors_configuration_fails_at_build_time() {
    let result = ServerBuilder::new()
        .with_server_security(ServerSecurityConfig {
            cors: CorsConfig {
                enabled: true,
                origins: vec!["*".into()],
                credentials: true,
                ..Default::default()
            },
            ..Default::default()
        })
        .with_link_service(this::storage::in_memory::InMemoryLinkService::new())
        .build_host();
    let message = match result {
        Ok(_) => panic!("une configuration CORS impossible doit faire échouer la construction"),
        Err(e) => format!("{e:#}"),
    };
    assert!(message.contains("credentials"), "{message}");
}

// ===========================================================================
// 3. DÉLÉGATION CADDYFILE
// ===========================================================================

/// Le `Caddyfile` est rendu **depuis la configuration** : mêmes domaines, même
/// upstream — c'est ce qui fait de la délégation une description fidèle.
#[tokio::test]
async fn the_caddyfile_is_rendered_from_the_same_configuration() {
    let host = ServerBuilder::new()
        .with_server_security(ServerSecurityConfig {
            cors: CorsConfig {
                enabled: true,
                origins: vec!["https://app.exemple.fr".into()],
                ..Default::default()
            },
            domains: DomainConfig {
                allowed: vec!["app.exemple.fr".into(), "admin.exemple.fr".into()],
                trust_forwarded_host: true,
                ..Default::default()
            },
            proxy: ProxyConfig {
                caddy: CaddyConfig {
                    enabled: true,
                    upstream: "127.0.0.1:3000".into(),
                    email: Some("ops@exemple.fr".into()),
                    tls: CaddyTls::Acme,
                    security_headers: true,
                    upstreams: std::collections::BTreeMap::new(),
                    extra_directives: vec!["encode zstd gzip".into()],
                },
            },
            ..Default::default()
        })
        .with_link_service(this::storage::in_memory::InMemoryLinkService::new())
        .build_host()
        .expect("hôte");

    let caddyfile = host.caddyfile().expect("délégation demandée");
    for expected in [
        "app.exemple.fr {",
        "admin.exemple.fr {",
        "reverse_proxy 127.0.0.1:3000",
        "email ops@exemple.fr",
        "encode zstd gzip",
        "posé par this-rs",
    ] {
        assert!(
            caddyfile.contains(expected),
            "le Caddyfile doit contenir `{expected}` :\n{caddyfile}"
        );
    }
    assert!(
        !caddyfile.contains("tls internal"),
        "ACME ne s'écrit pas : c'est le défaut de Caddy"
    );
}

/// Sans délégation demandée, **aucun** fichier n'est produit : le framework
/// n'écrit pas de configuration de proxy que personne n'a demandée.
#[tokio::test]
async fn no_caddyfile_without_an_explicit_delegation() {
    let host = ServerBuilder::new()
        .with_link_service(this::storage::in_memory::InMemoryLinkService::new())
        .build_host()
        .expect("hôte");
    assert!(host.caddyfile().is_none());
}
