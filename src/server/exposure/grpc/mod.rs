//! gRPC API exposure for the framework
//!
//! This module provides gRPC-specific services for exposing the API.
//! It uses tonic as the gRPC framework and supports dynamic entity types
//! via `google.protobuf.Struct` messages.
//!
//! ## Architecture
//!
//! - **EntityService**: Generic CRUD operations for any registered entity type
//! - **LinkService**: Relationship management between entities
//! - **EventService**: Real-time event streaming via server-streaming RPC
//! - **NotificationService**: In-app notification CRUD and streaming (when NotificationStore is configured)
//! - **ProtoGenerator**: Generates typed `.proto` files for client code generation
//!
//! The gRPC services consume a `ServerHost` (same as REST, GraphQL, WebSocket)
//! and are mounted alongside other exposures on the same port via axum interop.
//!
//! ## Dual mode: standalone vs cohabitation
//!
//! Two builder methods are available:
//!
//! - [`GrpcExposure::build_router`] — Standalone gRPC server. Includes tonic's
//!   default `UNIMPLEMENTED` fallback for unknown services. **Cannot** be merged
//!   with a REST router (causes panic due to double fallback).
//!
//! - [`GrpcExposure::build_router_no_fallback`] — For REST+gRPC cohabitation.
//!   Omits the tonic fallback so the router can be safely merged with REST.
//!   Use [`combine_rest_and_grpc`](crate::server::router::combine_rest_and_grpc)
//!   or [`ServerBuilder::build_with_grpc`](crate::server::ServerBuilder::build_with_grpc)
//!   for convenience.

pub mod entity_service;
pub mod event_service;
pub mod identity;
pub mod link_service;
pub mod notification_service;
pub mod proto_generator;

mod convert;

// Include the generated protobuf code
pub mod proto {
    tonic::include_proto!("this_grpc");
}

use crate::server::host::ServerHost;
use anyhow::Result;
use axum::Router;
use std::sync::Arc;

/// gRPC API exposure implementation
///
/// This struct encapsulates all gRPC-specific logic for exposing the API.
/// It is completely separate from the framework core and can coexist
/// with other exposure types (REST, GraphQL, WebSocket).
///
/// # Example — Standalone gRPC server
///
/// ```rust,ignore
/// let host = Arc::new(builder.build_host()?);
/// let grpc_router = GrpcExposure::build_router(host)?;
/// axum::serve(listener, grpc_router).await?;
/// ```
///
/// # Example — REST + gRPC on the same port
///
/// ```rust,ignore
/// let host = Arc::new(builder.build_host()?);
/// let rest_router = RestExposure::build_router(host.clone(), vec![])?;
/// let grpc_router = GrpcExposure::build_router_no_fallback(host)?;
/// let app = rest_router.merge(grpc_router); // Safe: no double fallback
/// ```
///
/// # Warning
///
/// **Do NOT merge `build_router()` with a REST router directly.**
/// Both install axum fallback handlers, and `Router::merge()` panics when
/// two routers have fallbacks. Use `build_router_no_fallback()` instead.
pub struct GrpcExposure;

impl GrpcExposure {
    /// Build the gRPC router from a host (**standalone mode**)
    ///
    /// This method takes a `ServerHost` (which is transport-agnostic) and
    /// builds an Axum router with gRPC services mounted.
    ///
    /// The router includes:
    /// - `EntityService` for CRUD operations on any entity type
    /// - `LinkService` for relationship management
    /// - `GET /grpc/proto` endpoint for exporting the typed `.proto` definition
    /// - A tonic `UNIMPLEMENTED` fallback for unknown gRPC services
    ///
    /// # Warning
    ///
    /// This router includes a fallback handler. **Do not merge it** with another
    /// router that also has a fallback (e.g., REST), or axum will panic.
    /// Use [`build_router_no_fallback`](Self::build_router_no_fallback) for
    /// REST+gRPC cohabitation.
    ///
    /// # Arguments
    ///
    /// * `host` - The server host containing all framework state
    ///
    /// # Returns
    ///
    /// Returns a fully configured Axum router with gRPC services and tonic fallback.
    ///
    /// # Policies (armed host)
    ///
    /// Every `EntityService` / `LinkService` call is judged per operation by the
    /// declared policies (`SurfaceGuard`: descriptor, then YAML `auth:`, then the
    /// default), with the caller resolved from the call **metadata**
    /// ([`identity::identity_layer`]). The event stream is filtered by the same
    /// policies. Refusal: `PERMISSION_DENIED` with the `this.auth.forbidden` contract
    /// error in the details. Only `NotificationService` is not covered yet: with a
    /// `NotificationStore` and armed policies, the router is still REFUSED.
    pub fn build_router(host: Arc<ServerHost>) -> Result<Router> {
        crate::server::exposure::refuse_uncovered_surface(
            &host,
            "gRPC",
            &uncovered_parts(&host),
            "GrpcExposure::build_router_unenforced",
        )?;
        Ok(Self::routes(host, true, true))
    }

    /// Comme [`GrpcExposure::build_router`], en **acceptant explicitement** que
    /// les politiques `auth:` ne s'appliquent pas au gRPC.
    ///
    /// Voir [`GraphQLExposure::build_router_unenforced`](crate::server::GraphQLExposure::build_router_unenforced)
    /// pour ce que cet opt-in signifie.
    pub fn build_router_unenforced(host: Arc<ServerHost>) -> Result<Router> {
        crate::server::exposure::warn_unenforced_surface(
            "gRPC",
            "GrpcExposure::build_router_unenforced",
        );
        Ok(Self::routes(host, false, true))
    }

    /// Build a gRPC router **without** a fallback handler (**cohabitation mode**)
    ///
    /// Unlike [`build_router`](Self::build_router), this method does **not** install
    /// tonic's default `UNIMPLEMENTED` fallback, so the router can be merged with a
    /// REST router (which has its own fallback):
    ///
    /// ```rust,ignore
    /// let rest_router = RestExposure::build_router(host.clone(), vec![])?;
    /// let grpc_router = GrpcExposure::build_router_no_fallback(host)?;
    /// let app = combine_rest_and_grpc(rest_router, grpc_router);
    /// ```
    ///
    /// Without the tonic fallback, unknown gRPC paths get the REST fallback (HTTP
    /// 404) instead of `UNIMPLEMENTED`. Each service is registered with
    /// `route_service("/{package.ServiceName}/{*rest}")`, as tonic does internally.
    /// Policies: as [`build_router`](Self::build_router).
    pub fn build_router_no_fallback(host: Arc<ServerHost>) -> Result<Router> {
        crate::server::exposure::refuse_uncovered_surface(
            &host,
            "gRPC",
            &uncovered_parts(&host),
            "GrpcExposure::build_router_no_fallback_unenforced",
        )?;
        Ok(Self::routes(host, true, false))
    }

    /// Variante sans repli, en **acceptant explicitement** l'absence
    /// d'application des politiques (voir [`GrpcExposure::build_router`]).
    pub fn build_router_no_fallback_unenforced(host: Arc<ServerHost>) -> Result<Router> {
        crate::server::exposure::warn_unenforced_surface(
            "gRPC (sans repli)",
            "GrpcExposure::build_router_no_fallback_unenforced",
        );
        Ok(Self::routes(host, false, false))
    }

    /// The router: services enforced or not, with or without tonic's fallback.
    fn routes(host: Arc<ServerHost>, enforce: bool, fallback: bool) -> Router {
        use axum::routing::get;
        use proto::entity_service_server::EntityServiceServer;
        use proto::event_service_server::EventServiceServer;
        use proto::link_service_server::LinkServiceServer;
        use proto::notification_service_server::NotificationServiceServer;

        let (entity_svc, link_svc, event_svc) = if enforce {
            (
                entity_service::EntityServiceImpl::new(host.clone()),
                link_service::LinkServiceImpl::new(host.clone()),
                event_service::EventServiceImpl::new(host.clone()),
            )
        } else {
            (
                entity_service::EntityServiceImpl::unenforced(host.clone()),
                link_service::LinkServiceImpl::unenforced(host.clone()),
                event_service::EventServiceImpl::unenforced(host.clone()),
            )
        };
        let notification_svc = host
            .notification_store()
            .is_some()
            .then(|| notification_service::NotificationServiceImpl::new(host.clone()));

        let mut grpc_router = if fallback {
            // NOTE: Routes::default() installs a fallback(UNIMPLEMENTED) handler.
            let mut builder = tonic::service::Routes::builder();
            builder.add_service(EntityServiceServer::new(entity_svc));
            builder.add_service(LinkServiceServer::new(link_svc));
            builder.add_service(EventServiceServer::new(event_svc));
            if let Some(svc) = notification_svc {
                builder.add_service(NotificationServiceServer::new(svc));
            }
            builder.routes().into_axum_router()
        } else {
            use tonic::server::NamedService;
            use tower::ServiceExt;
            fn path(name: &str) -> String {
                format!("/{name}/{{*rest}}")
            }
            let to_tonic =
                |req: axum::http::Request<axum::body::Body>| req.map(tonic::body::Body::new);
            let mut r = Router::new()
                .route_service(
                    &path(EntityServiceServer::<entity_service::EntityServiceImpl>::NAME),
                    EntityServiceServer::new(entity_svc).map_request(to_tonic),
                )
                .route_service(
                    &path(LinkServiceServer::<link_service::LinkServiceImpl>::NAME),
                    LinkServiceServer::new(link_svc).map_request(to_tonic),
                )
                .route_service(
                    &path(EventServiceServer::<event_service::EventServiceImpl>::NAME),
                    EventServiceServer::new(event_svc).map_request(to_tonic),
                );
            if let Some(svc) = notification_svc {
                r =
                    r.route_service(
                        &path(
                            NotificationServiceServer::<
                                notification_service::NotificationServiceImpl,
                            >::NAME,
                        ),
                        NotificationServiceServer::new(svc).map_request(to_tonic),
                    );
            }
            r
        };

        if enforce {
            // Identity from the call metadata, once per HTTP/2 request.
            grpc_router = grpc_router.layer(axum::middleware::from_fn_with_state(
                host.clone(),
                identity::identity_layer,
            ));
        }

        // Add the proto export endpoint
        let proto_host = host.clone();
        let proto_route =
            Router::new().route("/grpc/proto", get(move || proto_export_handler(proto_host)));

        grpc_router.merge(proto_route)
    }
}

/// Parts of the gRPC surface that do not apply the policies yet.
fn uncovered_parts(host: &ServerHost) -> Vec<&'static str> {
    let mut out = Vec::new();
    if host.notification_store().is_some() {
        // Takes the user id from the request message: an identity check is needed
        // before it can be served armed.
        out.push("NotificationService");
    }
    out
}

/// Handler for GET /grpc/proto — exports a typed `.proto` definition
///
/// This generates a `.proto` file dynamically based on the registered
/// entity types, allowing clients to generate typed gRPC stubs.
async fn proto_export_handler(host: Arc<ServerHost>) -> impl axum::response::IntoResponse {
    let generator = proto_generator::ProtoGenerator::new(host);
    let proto_content = generator.generate_proto().await;

    (
        [(
            axum::http::header::CONTENT_TYPE,
            "text/plain; charset=utf-8",
        )],
        proto_content,
    )
}
