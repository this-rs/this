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
pub mod link_service;
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
    pub fn build_router(host: Arc<ServerHost>) -> Result<Router> {
        use axum::routing::get;
        use proto::entity_service_server::EntityServiceServer;
        use proto::link_service_server::LinkServiceServer;
        use tonic::service::Routes;

        // Create gRPC service implementations
        let entity_svc = entity_service::EntityServiceImpl::new(host.clone());
        let link_svc = link_service::LinkServiceImpl::new(host.clone());

        // Build tonic Routes and convert to axum Router
        // NOTE: Routes::default() installs a fallback(UNIMPLEMENTED) handler.
        let mut builder = Routes::builder();
        builder.add_service(EntityServiceServer::new(entity_svc));
        builder.add_service(LinkServiceServer::new(link_svc));
        let grpc_router = builder.routes().into_axum_router();

        // Add the proto export endpoint
        let proto_host = host.clone();
        let proto_route =
            Router::new().route("/grpc/proto", get(move || proto_export_handler(proto_host)));

        Ok(grpc_router.merge(proto_route))
    }

    /// Build a gRPC router **without** a fallback handler (**cohabitation mode**)
    ///
    /// Unlike [`build_router`](Self::build_router), this method does **not** install
    /// tonic's default `UNIMPLEMENTED` fallback. This allows the returned router to
    /// be safely merged with another router that already has a fallback (e.g., REST
    /// with its nested link path handler).
    ///
    /// The router includes:
    /// - `EntityService` for CRUD operations on any entity type
    /// - `LinkService` for relationship management
    /// - `GET /grpc/proto` endpoint for exporting the typed `.proto` definition
    ///
    /// # When to use
    ///
    /// Use this method when combining gRPC with REST on the same server:
    ///
    /// ```rust,ignore
    /// use this::server::router::combine_rest_and_grpc;
    ///
    /// let host = Arc::new(builder.build_host()?);
    /// let rest_router = RestExposure::build_router(host.clone(), vec![])?;
    /// let grpc_router = GrpcExposure::build_router_no_fallback(host)?;
    /// let app = combine_rest_and_grpc(rest_router, grpc_router);
    /// ```
    ///
    /// Or use the convenience method:
    ///
    /// ```rust,ignore
    /// let app = builder.build_with_grpc()?;
    /// ```
    ///
    /// # Trade-off
    ///
    /// Without the tonic fallback, requests to unknown gRPC service paths will be
    /// handled by the REST fallback (returning HTTP 404) instead of the standard
    /// gRPC `UNIMPLEMENTED` status. This is acceptable for cohabitation scenarios.
    ///
    /// # How it works
    ///
    /// Instead of using `tonic::service::Routes` (which installs a fallback via
    /// `Routes::default()`), this method registers each gRPC service directly on
    /// a bare `axum::Router` using `route_service()`, replicating the path format
    /// `/{package.ServiceName}/{*rest}` that tonic uses internally.
    pub fn build_router_no_fallback(host: Arc<ServerHost>) -> Result<Router> {
        use axum::routing::get;
        use proto::entity_service_server::EntityServiceServer;
        use proto::link_service_server::LinkServiceServer;
        use tonic::server::NamedService;
        use tower::ServiceExt;

        // Create gRPC service implementations
        let entity_svc = entity_service::EntityServiceImpl::new(host.clone());
        let link_svc = link_service::LinkServiceImpl::new(host.clone());

        let entity_server = EntityServiceServer::new(entity_svc);
        let link_server = LinkServiceServer::new(link_svc);

        // Build axum Router directly, bypassing tonic's Routes which installs
        // a fallback via Routes::default() → axum::Router::new().fallback(unimplemented).
        //
        // This replicates what tonic::service::Routes::add_service() does internally:
        //   router.route_service("/{ServiceName}/{*rest}", svc.map_request(body_convert))
        let grpc_router = Router::new()
            .route_service(
                &format!(
                    "/{}/{{*rest}}",
                    EntityServiceServer::<entity_service::EntityServiceImpl>::NAME
                ),
                entity_server.map_request(|req: axum::http::Request<axum::body::Body>| {
                    req.map(tonic::body::Body::new)
                }),
            )
            .route_service(
                &format!(
                    "/{}/{{*rest}}",
                    LinkServiceServer::<link_service::LinkServiceImpl>::NAME
                ),
                link_server.map_request(|req: axum::http::Request<axum::body::Body>| {
                    req.map(tonic::body::Body::new)
                }),
            );

        // Add the proto export endpoint
        let proto_host = host.clone();
        let proto_route =
            Router::new().route("/grpc/proto", get(move || proto_export_handler(proto_host)));

        Ok(grpc_router.merge(proto_route))
    }
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
