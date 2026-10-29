//! API Exposure modules for different protocols
//!
//! This module provides implementations for exposing the API via different protocols.
//! Each exposure type consumes a `ServerHost` and produces a Router for that protocol.

pub mod rest;

// Export of the module descriptors and derived documents (`/_this/*`).
pub mod descriptor;

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
/// Les politiques `auth:` déclarées sont-elles **armées** sur cet hôte ?
pub(crate) fn policies_armed(host: &crate::server::host::ServerHost) -> bool {
    host.authorize
}

#[cfg(any(feature = "graphql", feature = "grpc", feature = "websocket"))]
/// **Refuse** de servir une surface qui ne sait pas appliquer les politiques.
///
/// # Pourquoi refuser plutôt que prévenir
///
/// Les surfaces hors REST (GraphQL, gRPC, WebSocket) ne sont pas résolues par
/// route : aucune de leurs opérations ne peut être rattachée à une politique
/// déclarée. Les servir alors que l'application des politiques est armée
/// publierait donc les entités **sans leur politique** — un client atteindrait
/// par GraphQL, et jusqu'à la mutation, ce que le REST refuse.
///
/// Ce n'est pas une limite qu'on peut réparer en passant un middleware : le
/// contrôle devrait être fait **par opération**, dans les résolveurs. En
/// attendant, le framework refuse, et l'application qui accepte le risque le
/// dit **dans son code** (voir `opt_in`).
pub(crate) fn refuse_unenforced_surface(
    host: &crate::server::host::ServerHost,
    surface: &str,
    opt_in: &str,
) -> anyhow::Result<()> {
    if !policies_armed(host) {
        return Ok(());
    }
    anyhow::bail!(
        "les politiques `auth:` sont ARMÉES, et la surface {surface} ne peut pas les appliquer : \
         ses opérations ne sont pas résolues par route, donc aucune politique ne s'y rattache. \
         La servir ainsi publierait les entités sans leur politique. Deux issues : \
         (a) accepter explicitement le risque avec `{opt_in}` — et le dire dans les journaux, \
         (b) ou désarmer l'application des politiques (`without_authorization()`) si c'est voulu."
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
