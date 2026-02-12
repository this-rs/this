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
/// # Example
///
/// ```rust,ignore
/// let host = builder.build_host()?;
/// let host = Arc::new(host);
/// let grpc_router = GrpcExposure::build_router(host.clone())?;
/// let rest_router = RestExposure::build_router(host.clone(), vec![])?;
///
/// // Merge both routers to serve on the same port
/// let app = rest_router.merge(grpc_router);
/// ```
pub struct GrpcExposure;

impl GrpcExposure {
    /// Build the gRPC router from a host
    ///
    /// This method takes a `ServerHost` (which is transport-agnostic) and
    /// builds an Axum router with gRPC services mounted.
    ///
    /// The router includes:
    /// - `EntityService` for CRUD operations on any entity type
    /// - `LinkService` for relationship management
    /// - `GET /grpc/proto` endpoint for exporting the typed `.proto` definition
    ///
    /// # Arguments
    ///
    /// * `host` - The server host containing all framework state
    ///
    /// # Returns
    ///
    /// Returns a fully configured Axum router with gRPC services.
    pub fn build_router(host: Arc<ServerHost>) -> Result<Router> {
        use axum::routing::get;
        use proto::entity_service_server::EntityServiceServer;
        use proto::link_service_server::LinkServiceServer;
        use tonic::service::Routes;

        // Create gRPC service implementations
        let entity_svc = entity_service::EntityServiceImpl::new(host.clone());
        let link_svc = link_service::LinkServiceImpl::new(host.clone());

        // Build tonic Routes and convert to axum Router
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
