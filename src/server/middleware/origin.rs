//! **L'entrée HTTP** : quels domaines sont servis, quel locataire ils
//! désignent, et qui a le droit d'appeler depuis un navigateur (CORS).
//!
//! Ces deux réglages vont ensemble parce qu'ils répondent à la même question —
//! *qui atteint cette application, et par où* — et parce qu'ils sont les seuls
//! à devoir s'appliquer **avant** l'authentification :
//!
//! * un **pré-vol** CORS (`OPTIONS`) ne porte pas d'en-tête `Authorization` : si
//!   la couche d'authentification le voyait d'abord, elle répondrait 401 et le
//!   navigateur refuserait l'appel légitime ;
//! * un **hôte non servi** doit être refusé avant tout travail, sans quoi un
//!   `Host` forgé ferait résoudre des liens absolus ou des redirections vers un
//!   domaine que l'application ne sert pas.
//!
//! Ordre appliqué par l'exposition REST : **CORS**, puis **garde de domaine**,
//! puis authentification, puis politiques.

use std::sync::Arc;

use axum::extract::{Request, State};
use axum::http::{HeaderMap, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use serde_json::json;

use crate::config::{CorsConfig, DomainConfig};
use crate::core::tenant::TenantContext;

/// L'hôte réellement servi, et le locataire qu'il désigne.
///
/// Posé dans les extensions par le garde de domaine : un handler qui construit
/// une URL absolue ou un cookie doit utiliser **cet** hôte, jamais l'en-tête
/// brut — c'est la différence entre un lien correct et un lien forgé.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DomainContext {
    /// L'hôte retenu (normalisé : minuscules, sans port).
    pub host: String,
    /// Le locataire désigné par cet hôte, s'il y en a un.
    pub tenant: Option<uuid::Uuid>,
}

/// Le garde de domaine.
#[derive(Clone)]
pub struct DomainGuard {
    config: Arc<DomainConfig>,
}

impl DomainGuard {
    /// Construit le garde.
    pub fn new(config: Arc<DomainConfig>) -> Self {
        Self { config }
    }
}

/// **Refuse un hôte non servi**, et désigne le locataire de l'hôte.
///
/// Sans liste de domaines configurée, le garde **laisse tout passer** : le
/// framework ne devine pas les domaines d'un déploiement, et l'ajout d'une garde
/// silencieuse casserait une application existante.
pub async fn domain_guard_middleware(
    State(guard): State<DomainGuard>,
    mut request: Request,
    next: Next,
) -> Response {
    let host = effective_host(request.headers(), guard.config.trust_forwarded_host);

    let Some(host) = host else {
        // HTTP/1.0 sans `Host` : sans garde, on laisse passer (comportement
        // d'avant) ; avec une garde, on refuse — un hôte absent ne peut pas être
        // un hôte servi.
        if guard.config.guards() {
            tracing::warn!("requête sans en-tête `Host` : refusée (garde de domaine active)");
            return refused("(absent)", "en-tête `Host` absent");
        }
        return next.run(request).await;
    };

    if !guard.config.allows(&host) {
        tracing::warn!(
            host = %host,
            path = %request.uri().path(),
            "hôte NON SERVI : refusé (voir `domains.allowed` dans config/server.yaml)"
        );
        return refused(&host, "cet hôte n'est pas servi par cette application");
    }

    let tenant = guard.config.tenant_of(&host);
    if let Some(tenant) = tenant {
        // Le locataire du **domaine** est posé ici : le résolveur de locataire
        // ne l'écrasera pas (une valeur déjà posée est respectée), donc un jeton
        // d'un autre locataire ne peut pas changer de locataire par la parole.
        request.extensions_mut().insert(TenantContext::new(tenant));
    }
    request
        .extensions_mut()
        .insert(DomainContext { host, tenant });

    next.run(request).await
}

/// L'hôte effectif : `X-Forwarded-Host` **seulement** si on lui fait confiance.
///
/// Faire confiance à cet en-tête sans proxy devant revient à laisser le client
/// choisir l'hôte que l'application croit servir.
fn effective_host(headers: &HeaderMap, trust_forwarded: bool) -> Option<String> {
    if trust_forwarded
        && let Some(forwarded) = headers.get("x-forwarded-host").and_then(|v| v.to_str().ok())
        // Un proxy peut empiler les valeurs : la première est celle du client.
        && let Some(first) = forwarded.split(',').next()
        && !first.trim().is_empty()
    {
        return Some(normalize(first));
    }
    headers
        .get(axum::http::header::HOST)
        .and_then(|v| v.to_str().ok())
        .map(normalize)
}

/// `Exemple.FR:8443` → `exemple.fr`.
fn normalize(host: &str) -> String {
    let host = host.trim().trim_end_matches('.');
    let host = host.split(':').next().unwrap_or(host);
    host.to_ascii_lowercase()
}

fn refused(host: &str, reason: &str) -> Response {
    (
        // 421 : « la requête visait une autorité que ce serveur ne sert pas » —
        // exactement le sens d'un hôte absent de la liste.
        StatusCode::MISDIRECTED_REQUEST,
        axum::Json(json!({
            "error": "misdirected_request",
            "host": host,
            "reason": reason,
        })),
    )
        .into_response()
}

// ---------------------------------------------------------------------------
// CORS
// ---------------------------------------------------------------------------

/// La couche CORS correspondant à la configuration, ou `None` si elle est
/// désactivée.
///
/// **Rien par défaut** : sans `cors.enabled`, aucun en-tête n'est posé — le
/// framework ne décide pas à la place de l'application qui a le droit d'appeler.
pub fn cors_layer(config: &CorsConfig) -> Option<tower_http::cors::CorsLayer> {
    use tower_http::cors::{AllowOrigin, CorsLayer};

    if !config.enabled {
        return None;
    }

    let mut layer = CorsLayer::new();

    // Les origines : `*` est rendu explicite, une liste est respectée telle
    // quelle — une origine absente n'obtient aucun en-tête, donc le navigateur
    // bloque.
    if config.origins.iter().any(|o| o == "*") {
        layer = layer.allow_origin(AllowOrigin::any());
    } else {
        let origins: Vec<axum::http::HeaderValue> = config
            .origins
            .iter()
            .filter_map(|o| o.parse().ok())
            .collect();
        layer = layer.allow_origin(AllowOrigin::list(origins));
    }

    let methods: Vec<axum::http::Method> = config
        .methods
        .iter()
        .filter_map(|m| m.parse().ok())
        .collect();
    layer = layer.allow_methods(methods);

    let headers: Vec<axum::http::HeaderName> = config
        .headers
        .iter()
        .filter_map(|h| h.parse().ok())
        .collect();
    layer = layer.allow_headers(headers);

    if !config.expose_headers.is_empty() {
        let expose: Vec<axum::http::HeaderName> = config
            .expose_headers
            .iter()
            .filter_map(|h| h.parse().ok())
            .collect();
        layer = layer.expose_headers(expose);
    }

    if config.credentials {
        layer = layer.allow_credentials(true);
    }

    if let Some(max_age) = config.max_age_secs {
        layer = layer.max_age(std::time::Duration::from_secs(max_age));
    }

    Some(layer)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn headers(pairs: &[(&str, &str)]) -> HeaderMap {
        let mut map = HeaderMap::new();
        for (name, value) in pairs {
            let name: axum::http::HeaderName = name.parse().expect("nom d'en-tête");
            let value: axum::http::HeaderValue = value.parse().expect("valeur d'en-tête");
            map.insert(name, value);
        }
        map
    }

    #[test]
    fn the_effective_host_ignores_forwarded_headers_unless_trusted() {
        let h = headers(&[
            ("host", "reel.exemple.fr"),
            ("x-forwarded-host", "forge.evil.fr"),
        ]);

        assert_eq!(
            effective_host(&h, false).as_deref(),
            Some("reel.exemple.fr"),
            "sans confiance, `X-Forwarded-Host` est ignoré : sinon le client choisit l'hôte"
        );
        assert_eq!(
            effective_host(&h, true).as_deref(),
            Some("forge.evil.fr"),
            "derrière un proxy de confiance, il est lu"
        );
    }

    #[test]
    fn the_forwarded_host_chain_takes_the_first_value() {
        let h = headers(&[(
            "x-forwarded-host",
            "client.exemple.fr, proxy.interne, autre",
        )]);
        assert_eq!(
            effective_host(&h, true).as_deref(),
            Some("client.exemple.fr")
        );
    }

    #[test]
    fn the_host_is_normalised() {
        let h = headers(&[("host", "APP.Exemple.FR:8443")]);
        assert_eq!(effective_host(&h, false).as_deref(), Some("app.exemple.fr"));
        let h = headers(&[("host", "app.exemple.fr.")]);
        assert_eq!(
            effective_host(&h, false).as_deref(),
            Some("app.exemple.fr"),
            "le point final du FQDN ne change pas l'hôte"
        );
    }

    #[test]
    fn no_cors_configuration_means_no_layer() {
        assert!(cors_layer(&CorsConfig::default()).is_none());
    }

    #[test]
    fn a_configured_cors_builds_a_layer() {
        let config = CorsConfig {
            enabled: true,
            origins: vec!["https://app.exemple.fr".into()],
            credentials: true,
            max_age_secs: Some(600),
            ..Default::default()
        };
        assert!(cors_layer(&config).is_some());
    }
}
