//! API Exposure modules for different protocols
//!
//! This module provides implementations for exposing the API via different protocols.
//! Each exposure type consumes a `ServerHost` and produces a Router for that protocol.

pub mod rest;

#[cfg(feature = "graphql")]
pub mod graphql;

// Re-export for convenience
pub use rest::RestExposure;

#[cfg(feature = "graphql")]
pub use graphql::GraphQLExposure;

#[cfg(feature = "websocket")]
pub mod websocket;

#[cfg(feature = "websocket")]
pub use websocket::WebSocketExposure;

#[cfg(feature = "grpc")]
pub mod grpc;

#[cfg(feature = "grpc")]
pub use grpc::GrpcExposure;

// Future expositions
// pub mod openapi;

#[cfg(any(feature = "graphql", feature = "grpc", feature = "websocket"))]
pub mod guard;
#[cfg(any(feature = "graphql", feature = "grpc", feature = "websocket"))]
pub use guard::{SurfaceGuard, resolve_identity};

#[cfg(any(feature = "graphql", feature = "grpc", feature = "websocket"))]
/// Les politiques `auth:` déclarées sont-elles **armées** sur cet hôte ?
pub(crate) fn policies_armed(host: &crate::server::host::ServerHost) -> bool {
    host.authorize
}

#[cfg(any(feature = "graphql", feature = "grpc", feature = "websocket"))]
/// (Remplace l'ancien `refuse_unenforced_surface`, qui refusait toute surface
/// hors REST dès que les politiques étaient armées : GraphQL, gRPC et WebSocket
/// appliquent désormais les politiques par opération.)
///
/// **Lève** le refus pour une surface désormais appliquée par opération — et le
/// garde pour ce qui ne l'est pas encore.
///
/// GraphQL et gRPC appliquent les politiques **dans leurs résolveurs / services**
/// (`SurfaceGuard`, même résolution que REST : descripteur, puis `auth:` YAML, puis
/// défaut). Ils sont donc servis armés. Mais une surface n'est servie armée que si
/// **toutes** ses parties montées sont couvertes : `uncovered` liste celles qui ne le
/// sont pas (ex. `NotificationService` gRPC, qui prend l'utilisateur dans la
/// requête). Liste non vide + politiques armées = refus, comme avant (fail-closed).
pub(crate) fn refuse_uncovered_surface(
    host: &crate::server::host::ServerHost,
    surface: &str,
    uncovered: &[&str],
    opt_in: &str,
) -> anyhow::Result<()> {
    if !policies_armed(host) || uncovered.is_empty() {
        return Ok(());
    }
    anyhow::bail!(
        "les politiques `auth:` sont ARMÉES, et la surface {surface} est appliquée par \
         opération SAUF : {}. Ces parties ne savent pas encore appliquer les politiques ; \
         les servir publierait leurs données sans politique. Issues : (a) retirer ce qui \
         les monte (ex. le NotificationStore), (b) accepter le risque avec `{opt_in}`, \
         (c) désarmer (`without_authorization()`).",
        uncovered.join(", ")
    );
}

#[cfg(any(feature = "graphql", feature = "grpc", feature = "websocket"))]
/// Le journal d'une surface servie **sans** application des politiques.
pub(crate) fn warn_unenforced_surface(surface: &str, opt_in: &str) {
    tracing::warn!(
        surface,
        "surface servie SANS application des politiques (opt-in `{opt_in}`) : les entités \
         qu'elle expose ne sont pas filtrées par les `auth:` déclarées"
    );
}
