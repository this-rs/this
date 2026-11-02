//! `DescriptorEntity`: serve a COMPILED entity through descriptor-driven REST routes,
//! delegating storage to the module's own `EntityFetcher` / `EntityCreator`.
//!
//! This is the generic bridge that makes "declared once" wireable anywhere: given an
//! `EntityDesc` and the entity's store, it produces the same CRUD surface as the runtime's
//! declarative adapter (`src/server/module_runtime/json_entity.rs`):
//!
//! - `GET/POST /{plural}`
//! - `GET/PUT/PATCH/DELETE /{plural}/{id}`
//! - typed `?filter=<json>&sort=<a,-b>&page=&limit=` (malformed or not permitted = 400)
//! - stable error codes (`ApiError` / `ContractError`), `201` on create, `204` on delete
//!
//! without any hand-written handler. It implements [`EntityDescriptor`], so it can be
//! registered in an `EntityRegistry`
//! like any other entity — by a module (`register_entities`), by the builder for a
//! descriptor-only module, or directly.
//!
//! Storage is NOT owned here: rows live in the module's store (in-memory, SQL, ...). The
//! adapter only depends on the two traits, so swapping the backend does not change routes.

use crate::core::errors::{ApiError, from_anyhow};
use crate::core::module::{EntityCreator, EntityFetcher};
use crate::descriptor::engine::query::parse_legacy;
use crate::descriptor::{EntityDesc, Page, QuerySchema};
use axum::Router;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use serde_json::Value;
use std::sync::Arc;
use uuid::Uuid;

use crate::server::entity_registry::EntityDescriptor;

/// State handed to the generated handlers: the descriptor (identity + typed query) and
/// the entity's store. All members are cheap to clone; no `Arc<Self>` is required, so
/// `EntityDescriptor::build_routes(&self)` can build the router.
#[derive(Clone)]
struct EntityState {
    entity: EntityDesc,
    fetcher: Arc<dyn EntityFetcher>,
    creator: Arc<dyn EntityCreator>,
}

/// A compiled entity served by its descriptor, with storage delegated to its store.
#[derive(Clone)]
pub struct DescriptorEntity {
    state: EntityState,
}

impl DescriptorEntity {
    /// Wrap an entity descriptor and the store that backs it.
    pub fn new(
        entity: EntityDesc,
        fetcher: Arc<dyn EntityFetcher>,
        creator: Arc<dyn EntityCreator>,
    ) -> Self {
        Self {
            state: EntityState {
                entity,
                fetcher,
                creator,
            },
        }
    }
}

impl EntityDescriptor for DescriptorEntity {
    fn entity_type(&self) -> &str {
        &self.state.entity.name
    }

    fn plural(&self) -> &str {
        &self.state.entity.plural
    }

    fn build_routes(&self) -> Router {
        let collection = format!("/{}", self.state.entity.plural);
        let item = format!("/{}/{{id}}", self.state.entity.plural);
        Router::new()
            .route(&collection, get(list_handler).post(create_handler))
            .route(
                &item,
                get(get_handler)
                    .put(update_handler)
                    .patch(update_handler)
                    .delete(delete_handler),
            )
            .with_state(self.state.clone())
    }
}

type Svc = State<EntityState>;

fn err(e: anyhow::Error) -> Response {
    ApiError(from_anyhow(&e)).into_response()
}

/// `?filter=<json>&sort=<a,-b>&page=&limit=`: malformed or not permitted by the entity's
/// `QuerySchema` = 400 with a stable code (never ignored).
#[derive(serde::Deserialize)]
struct ListParams {
    filter: Option<String>,
    sort: Option<String>,
    #[serde(default = "one")]
    page: usize,
    #[serde(default = "twenty")]
    limit: usize,
}
fn one() -> usize {
    1
}
fn twenty() -> usize {
    20
}

async fn list_handler(State(s): Svc, Query(p): Query<ListParams>) -> Response {
    let schema = QuerySchema::for_entity(&s.entity);
    let q = match parse_legacy(
        &schema,
        p.filter.as_deref(),
        p.sort.as_deref(),
        p.page,
        p.limit,
    ) {
        Ok(q) => q,
        Err(e) => return ApiError(e).into_response(),
    };
    match s.fetcher.list_query(&q).await {
        Ok((data, total)) => {
            let (page, limit) = match q.page {
                Page::Offset { page, limit } => (page as usize, limit as usize),
                _ => (p.page, p.limit),
            };
            axum::Json(serde_json::json!({
                "data": data,
                "pagination": {
                    "page": page,
                    "limit": limit,
                    "total": total,
                    "total_pages": total.div_ceil(limit.max(1)),
                },
            }))
            .into_response()
        }
        Err(e) => err(e),
    }
}

async fn create_handler(State(s): Svc, body: axum::Json<Value>) -> Response {
    match s.creator.create_from_json(body.0).await {
        Ok(v) => (StatusCode::CREATED, axum::Json(v)).into_response(),
        Err(e) => err(e),
    }
}

async fn get_handler(State(s): Svc, Path(id): Path<Uuid>) -> Response {
    match s.fetcher.fetch_as_json(&id).await {
        Ok(v) => axum::Json(v).into_response(),
        Err(e) => err(e),
    }
}

async fn update_handler(State(s): Svc, Path(id): Path<Uuid>, body: axum::Json<Value>) -> Response {
    match s.creator.update_from_json(&id, body.0).await {
        Ok(v) => axum::Json(v).into_response(),
        Err(e) => err(e),
    }
}

async fn delete_handler(State(s): Svc, Path(id): Path<Uuid>) -> Response {
    match s.creator.delete(&id).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => err(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::Result;
    use axum::body::Body;
    use axum::body::to_bytes;
    use axum::http::{Request, StatusCode};
    use serde_json::json;
    use tower::ServiceExt;

    struct Mock;

    #[async_trait::async_trait]
    impl EntityFetcher for Mock {
        async fn fetch_as_json(&self, id: &Uuid) -> Result<Value> {
            Ok(json!({ "id": id }))
        }
        async fn list_as_json(
            &self,
            _limit: Option<i32>,
            _offset: Option<i32>,
        ) -> Result<Vec<Value>> {
            Ok(vec![
                json!({ "id": "00000000-0000-0000-0000-000000000001" }),
            ])
        }
    }

    #[async_trait::async_trait]
    impl EntityCreator for Mock {
        async fn create_from_json(&self, entity_data: Value) -> Result<Value> {
            Ok(entity_data)
        }
    }

    fn entity() -> DescriptorEntity {
        let mut e = EntityDesc::new("invoice");
        e.plural = "invoices".into();
        DescriptorEntity::new(e, Arc::new(Mock), Arc::new(Mock))
    }

    #[test]
    fn exposes_the_descriptor_identity() {
        let d = entity();
        assert_eq!(d.entity_type(), "invoice");
        assert_eq!(d.plural(), "invoices");
    }

    #[tokio::test]
    async fn list_serves_the_plural_with_the_typed_envelope() {
        let app = entity().build_routes();
        let res = app
            .oneshot(
                Request::builder()
                    .uri("/invoices")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let bytes = to_bytes(res.into_body(), usize::MAX).await.unwrap();
        let v: Value = serde_json::from_slice(&bytes).unwrap();
        assert!(v.get("data").is_some(), "typed list envelope: {v}");
        assert!(v.get("pagination").is_some(), "typed list envelope: {v}");
    }

    #[tokio::test]
    async fn create_is_201_and_get_returns_the_row() {
        let app = entity().build_routes();
        let created = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/invoices")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"amount":1}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(created.status(), StatusCode::CREATED);

        let id = "00000000-0000-0000-0000-000000000001";
        let got = app
            .oneshot(
                Request::builder()
                    .uri(format!("/invoices/{id}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(got.status(), StatusCode::OK);
    }
}
