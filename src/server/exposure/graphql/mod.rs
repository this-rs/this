//! GraphQL API exposure for the framework
//!
//! This module provides GraphQL-specific routing and schema generation.
//! It is completely separate from the core framework logic.

mod schema;
mod schema_generator;
mod dynamic_schema;
mod executor;

#[cfg(feature = "graphql")]
use crate::server::host::ServerHost;
#[cfg(feature = "graphql")]
use anyhow::Result;
#[cfg(feature = "graphql")]
use async_graphql::{
    Request as GraphQLRequest, Schema, EmptySubscription,
    http::{GraphQLPlaygroundConfig, playground_source},
};
#[cfg(feature = "graphql")]
use axum::{
    Router,
    extract::{Extension, Json as AxumJson},
    response::{Html, IntoResponse},
    routing::{get, post},
    body::Body,
};
#[cfg(feature = "graphql")]
use dynamic_schema::{DynamicQueryRoot, DynamicMutationRoot, build_dynamic_schema};
#[cfg(feature = "graphql")]
use executor::GraphQLExecutor;
#[cfg(feature = "graphql")]
use std::sync::Arc;
#[cfg(feature = "graphql")]
use serde::{Deserialize, Serialize};

#[cfg(feature = "graphql")]
#[derive(Debug, Deserialize)]
struct GraphQLRequestBody {
    query: String,
    variables: Option<std::collections::HashMap<String, serde_json::Value>>,
    #[allow(dead_code)]
    operation_name: Option<String>,
}

#[cfg(feature = "graphql")]
/// GraphQL API exposure implementation
///
/// This struct encapsulates all GraphQL-specific logic for exposing the API.
/// It is completely separate from the framework core.
pub struct GraphQLExposure;

#[cfg(feature = "graphql")]
impl GraphQLExposure {
    /// Build the GraphQL router from a host
    ///
    /// This method takes a `ServerHost` (which is transport-agnostic) and
    /// builds an Axum router with all GraphQL endpoints.
    ///
    /// # Arguments
    ///
    /// * `host` - The server host containing all framework state
    ///
    /// # Returns
    ///
    /// Returns a fully configured Axum router with:
    /// - GraphQL query endpoint
    /// - GraphQL mutation endpoint
    /// - GraphQL subscription endpoint (optional)
    /// - GraphQL playground (optional)
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// let host = builder.build_host()?;
    /// let graphql_app = GraphQLExposure::build_router(host)?;
    /// ```
    pub fn build_router(host: Arc<ServerHost>) -> Result<Router> {
        // Create the GraphQL router with playground, query endpoint, and schema endpoint
        // The executor will be created lazily on first request
        let router = Router::new()
            .route("/graphql", post(graphql_handler_custom))
            .route("/graphql/playground", get(graphql_playground))
            .route("/graphql/schema", get(graphql_dynamic_schema))
            .layer(Extension(host));

        Ok(router)
    }
}

#[cfg(feature = "graphql")]
type GraphQLSchema = Schema<DynamicQueryRoot, DynamicMutationRoot, EmptySubscription>;

#[cfg(feature = "graphql")]
/// Handler for GraphQL queries and mutations using custom executor
async fn graphql_handler_custom(
    Extension(host): Extension<Arc<ServerHost>>,
    AxumJson(request): AxumJson<GraphQLRequestBody>,
) -> impl IntoResponse {
    // Create executor on each request (or we could cache it)
    let executor = GraphQLExecutor::new(host).await;
    
    match executor.execute(&request.query, request.variables).await {
        Ok(response) => AxumJson(response),
        Err(e) => AxumJson(serde_json::json!({
            "errors": [{
                "message": e.to_string()
            }]
        })),
    }
}

#[cfg(feature = "graphql")]
/// Handler for GraphQL playground UI
async fn graphql_playground() -> impl IntoResponse {
    Html(playground_source(GraphQLPlaygroundConfig::new("/graphql")))
}

#[cfg(feature = "graphql")]
/// Handler for GraphQL schema SDL export
/// This generates the schema dynamically from entity introspection
async fn graphql_dynamic_schema(Extension(host): Extension<Arc<ServerHost>>) -> impl IntoResponse {
    use schema_generator::SchemaGenerator;

    let generator = SchemaGenerator::new(host);
    let sdl = generator.generate_sdl().await;

    (
        [(
            axum::http::header::CONTENT_TYPE,
            "text/plain; charset=utf-8",
        )],
        sdl,
    )
}

#[cfg(not(feature = "graphql"))]
mod graphql_placeholder {
    use super::super::super::host::ServerHost;
    use anyhow::Result;
    use axum::Router;

    pub struct GraphQLExposure;

    impl GraphQLExposure {
        pub fn build_router(_host: ServerHost) -> Result<Router> {
            anyhow::bail!(
                "GraphQL support is not enabled. Enable the 'graphql' feature to use GraphQL."
            );
        }
    }
}
