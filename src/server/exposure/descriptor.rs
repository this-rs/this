//! Export of the host's module descriptors and of the documents derived from them.
//!
//! | Route | Body |
//! |---|---|
//! | `GET /_this/descriptors` | `{ "generation": n, "modules": [ModuleDescriptor, ...] }` |
//! | `GET /_this/descriptors/{module}` | one `ModuleDescriptor` (404 `this.entity.not_found`) |
//! | `GET /_this/schema/{entity}/{create\|update}` | JSON Schema of the write payload |
//! | `GET /_this/openapi.json` | minimal OpenAPI 3.1 of the CRUD routes of described entities |
//!
//! Every request reads the CURRENT catalog snapshot (`ServerHost::descriptors`), so a module
//! published at runtime is exported without rebuilding the router. Descriptors carry no
//! secret (no texts, no data): the same exposure level as `/graphql/schema` and
//! `/grpc/proto`. The `_this` prefix cannot collide with an entity plural (identifiers
//! start with a letter). The authorization middleware abstains on these routes (unknown to
//! it): the global auth layer decides, as for `/health`.

use crate::core::errors::ApiError;
use crate::descriptor::derive;
use crate::descriptor::engine::validate::WriteOp;
use crate::descriptor::framework::codes;
use crate::descriptor::{ContractError, ModuleDescriptor};
use crate::server::host::ServerHost;
use axum::extract::Path;
use axum::routing::get;
use axum::{Json, Router};
use serde_json::{Value, json};
use std::sync::Arc;

/// Prefix of the framework's own routes.
pub const PREFIX: &str = "/_this";

/// Router of the export routes.
pub fn descriptor_routes(host: Arc<ServerHost>) -> Router {
    let (h1, h2, h3, h4) = (host.clone(), host.clone(), host.clone(), host);
    Router::new()
        .route(
            "/_this/descriptors",
            get(move || async move { Json(all_descriptors(&h1)) }),
        )
        .route(
            "/_this/descriptors/{module}",
            get(move |Path(module): Path<String>| async move {
                one_descriptor(&h2, &module)
                    .map(Json)
                    .map_err(|e| ApiError(*e))
            }),
        )
        .route(
            "/_this/schema/{entity}/{op}",
            get(
                move |Path((entity, op)): Path<(String, String)>| async move {
                    write_schema(&h3, &entity, &op)
                        .map(Json)
                        .map_err(|e| ApiError(*e))
                },
            ),
        )
        .route(
            "/_this/openapi.json",
            get(move || async move { Json(openapi(&h4)) }),
        )
}

fn all_descriptors(host: &ServerHost) -> Value {
    let set = host.descriptors();
    let modules: Vec<&ModuleDescriptor> = set.modules().map(|m| m.as_ref()).collect();
    json!({ "generation": set.generation(), "modules": modules })
}

fn not_found(what: &str, id: &str) -> Box<ContractError> {
    Box::new(
        ContractError::framework(codes::ENTITY_NOT_FOUND)
            .with_param("entity", what)
            .with_param("id", id),
    )
}

fn one_descriptor(host: &ServerHost, module: &str) -> Result<ModuleDescriptor, Box<ContractError>> {
    host.descriptors()
        .module(module)
        .map(|m| (**m).clone())
        .ok_or_else(|| not_found("module", module))
}

fn write_schema(host: &ServerHost, entity: &str, op: &str) -> Result<Value, Box<ContractError>> {
    let op = match op {
        "create" => WriteOp::Create,
        "update" => WriteOp::Update,
        other => return Err(not_found("operation", other)),
    };
    let set = host.descriptors();
    let (_, desc) = set
        .entity(entity)
        .ok_or_else(|| not_found("entity", entity))?;
    derive::json_schema(desc, op).map_err(Box::new)
}

fn openapi(host: &ServerHost) -> Value {
    derive::json_schema::openapi(&host.descriptors(), "this-rs", env!("CARGO_PKG_VERSION"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::LinksConfig;
    use crate::descriptor::{DescriptorSet, EntityDesc, FieldDesc, FieldType};
    use crate::server::entity_registry::EntityRegistry;
    use crate::storage::InMemoryLinkService;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use std::collections::HashMap;
    use tower::ServiceExt;

    fn host() -> Arc<ServerHost> {
        let mut e = EntityDesc::new("knife");
        e.fields
            .push(FieldDesc::new("price", FieldType::F64).tag(16));
        let mut m = ModuleDescriptor::new("shop", "1.2.0");
        m.entities.push(e);
        let mut set = DescriptorSet::new();
        set.insert(m).unwrap();
        let h = ServerHost::from_builder_components(
            Arc::new(InMemoryLinkService::new()),
            LinksConfig::default_config(),
            EntityRegistry::new(),
            HashMap::new(),
            HashMap::new(),
        )
        .unwrap()
        .with_descriptors(set);
        Arc::new(h)
    }

    async fn get_json(path: &str) -> (StatusCode, Value) {
        let resp = descriptor_routes(host())
            .oneshot(Request::get(path).body(Body::empty()).unwrap())
            .await
            .unwrap();
        let status = resp.status();
        let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20)
            .await
            .unwrap();
        (status, serde_json::from_slice(&bytes).unwrap())
    }

    #[tokio::test]
    async fn descriptor_export_endpoint_roundtrips() {
        let (s, v) = get_json("/_this/descriptors").await;
        assert_eq!(s, StatusCode::OK);
        assert_eq!(v["modules"][0]["name"], "shop");
        let (s, v) = get_json("/_this/descriptors/shop").await;
        assert_eq!(s, StatusCode::OK);
        // What is exported parses back into the same descriptor.
        let back = ModuleDescriptor::from_json(&v.to_string()).unwrap();
        assert_eq!(back.entities[0].field("price").unwrap().proto_tag, Some(16));
        let (s, v) = get_json("/_this/descriptors/nope").await;
        assert_eq!(s, StatusCode::NOT_FOUND);
        assert_eq!(v["code"], "this.entity.not_found");
    }

    #[tokio::test]
    async fn schema_and_openapi_endpoints() {
        let (s, v) = get_json("/_this/schema/knife/create").await;
        assert_eq!(s, StatusCode::OK);
        assert!(v["required"].as_array().unwrap().contains(&json!("price")));
        let (s, _) = get_json("/_this/schema/knife/upsert").await;
        assert_eq!(s, StatusCode::NOT_FOUND);
        let (s, v) = get_json("/_this/openapi.json").await;
        assert_eq!(s, StatusCode::OK);
        assert!(v["paths"]["/knives"]["post"].is_object());
    }
}
