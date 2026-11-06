//! `JsonEntityService`: the adapter that serves a DECLARATIVE entity (a module read from a
//! manifest or a `ModuleStore`, no Rust code) through the same contracts as a compiled
//! one: `EntityFetcher` / `EntityCreator` (so link enrichment, linked creation and the
//! host catalog see it like any other entity), the event bus (`EntityEvent::*`), and a
//! generated REST surface (`GET/POST /{plural}`, `GET/PUT/PATCH/DELETE /{plural}/{id}`).
//!
//! Rows live in a [`RowStore`], one per entity: in memory by
//! default, or in `obrain` when the host injects that factory (`rows_obrain`). The adapter
//! only depends on the trait, so swapping the store does not change the routes, the
//! validation or the events.
//!
//! Every write goes through the descriptor's validation engine
//! (`engine::validate::apply`): unknown keys, read-only and immutable fields, presence,
//! types and validators are enforced with the same codes as on every other surface.

use super::rows::{MemoryRowStore, RowStore};
use crate::core::errors::{ApiError, from_anyhow};
use crate::core::events::{EntityEvent, EventBus, FrameworkEvent};
use crate::core::module::{EntityCreator, EntityFetcher};
use crate::descriptor::engine::validate::{Side, ValidateCtx, WriteOp, apply};
use crate::descriptor::framework::codes;
use crate::descriptor::{CompiledEntity, ContractError, CustomRegistry, QuerySchema};
use anyhow::Result;
use async_trait::async_trait;
use axum::Router;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use serde_json::{Value, json};
use std::sync::Arc;
use uuid::Uuid;

/// One declarative entity, served from its descriptor.
pub struct JsonEntityService {
    entity: Arc<CompiledEntity>,
    custom: CustomRegistry,
    /// Where this entity's rows live. `Arc<dyn RowStore>` and not a concrete map: the
    /// same entity is served from memory in tests and from `obrain` in a durable host,
    /// with the routes, the validation and the events unchanged (see [`super::rows`]).
    rows: Arc<dyn RowStore>,
    event_bus: Option<Arc<EventBus>>,
}

impl JsonEntityService {
    /// Rows in memory — the behaviour of every declarative module before the seam.
    pub fn new(
        entity: Arc<CompiledEntity>,
        custom: CustomRegistry,
        event_bus: Option<Arc<EventBus>>,
    ) -> Self {
        Self::with_row_store(entity, custom, event_bus, Arc::new(MemoryRowStore::new()))
    }

    /// Rows in the store of the caller's choice (a database, a file, a graph).
    pub fn with_row_store(
        entity: Arc<CompiledEntity>,
        custom: CustomRegistry,
        event_bus: Option<Arc<EventBus>>,
        rows: Arc<dyn RowStore>,
    ) -> Self {
        Self {
            entity,
            custom,
            rows,
            event_bus,
        }
    }

    pub fn entity_type(&self) -> &str {
        &self.entity.desc.name
    }

    pub fn plural(&self) -> &str {
        &self.entity.desc.plural
    }

    fn ctx(&self) -> ValidateCtx<'_> {
        ValidateCtx {
            registry: &self.custom,
            side: Side::Server,
        }
    }

    fn not_found(&self, id: &Uuid) -> anyhow::Error {
        anyhow::Error::new(
            ContractError::framework(codes::ENTITY_NOT_FOUND)
                .with_param("entity", self.entity_type())
                .with_param("id", id.to_string()),
        )
    }

    fn publish(&self, event: EntityEvent) {
        if let Some(bus) = &self.event_bus {
            bus.publish(FrameworkEvent::Entity(event));
        }
    }

    /// `Created` from the row just stored (the row carries the generated `id`).
    fn publish_created(&self, row: &Value) {
        if let Some(id) = row
            .get("id")
            .and_then(|v| v.as_str())
            .and_then(|s| Uuid::parse_str(s).ok())
        {
            self.publish(EntityEvent::Created {
                entity_type: self.entity_type().to_string(),
                entity_id: id,
                data: row.clone(),
            });
        }
    }

    fn publish_updated(&self, entity_id: &Uuid, row: &Value) {
        self.publish(EntityEvent::Updated {
            entity_type: self.entity_type().to_string(),
            entity_id: *entity_id,
            data: row.clone(),
        });
    }

    fn publish_deleted(&self, entity_id: &Uuid) {
        self.publish(EntityEvent::Deleted {
            entity_type: self.entity_type().to_string(),
            entity_id: *entity_id,
        });
    }

    /// REST routes of this entity (no state leaks out: the router owns an `Arc<Self>`).
    pub fn routes(self: &Arc<Self>) -> Router {
        let collection = format!("/{}", self.plural());
        let item = format!("/{}/{{id}}", self.plural());
        Router::new()
            .route(&collection, get(list_handler).post(create_handler))
            .route(
                &item,
                get(get_handler)
                    .put(update_handler)
                    .patch(update_handler)
                    .delete(delete_handler),
            )
            .with_state(self.clone())
    }
}

#[async_trait]
impl EntityFetcher for JsonEntityService {
    async fn fetch_as_json(&self, entity_id: &Uuid) -> Result<Value> {
        self.rows
            .get(entity_id)
            .await?
            .ok_or_else(|| self.not_found(entity_id))
    }

    async fn get_sample_entity(&self) -> Result<Value> {
        // Shape from the descriptor, never from a stored row.
        let mut obj = serde_json::Map::new();
        for f in self.entity.desc.all_fields() {
            obj.insert(f.name.clone(), Value::Null);
        }
        Ok(Value::Object(obj))
    }

    async fn list_as_json(&self, limit: Option<i32>, offset: Option<i32>) -> Result<Vec<Value>> {
        let offset = offset.unwrap_or(0).max(0) as usize;
        let limit = limit.map(|l| l.max(0) as usize);
        self.rows.list(limit, offset).await
    }
}

#[async_trait]
impl EntityCreator for JsonEntityService {
    async fn create_from_json(&self, entity_data: Value) -> Result<Value> {
        let outcome = apply(&self.entity, WriteOp::Create, entity_data, &self.ctx())
            .map_err(anyhow::Error::new)?;
        let Value::Object(mut obj) = outcome.value else {
            unreachable!("validated payload is an object")
        };
        let id = Uuid::new_v4();
        let now = chrono::Utc::now().to_rfc3339();
        obj.insert("id".into(), json!(id));
        obj.insert("type".into(), json!(self.entity_type()));
        obj.insert("created_at".into(), json!(now));
        obj.insert("updated_at".into(), json!(now));
        obj.entry("status").or_insert_with(|| json!("active"));
        let row = Value::Object(obj);
        self.rows.insert(id, row.clone()).await?;
        // No event here: the write seam is `SurfaceGuard::creator` (which decorates this
        // creator with `PublishingCreator`) for the surfaces that reach the catalog, and the
        // REST handlers below for the service's own routes. Publishing here as well would
        // emit the event twice on every guard-routed surface (GraphQL, gRPC, createAndLink).
        Ok(row)
    }

    async fn update_from_json(&self, entity_id: &Uuid, entity_data: Value) -> Result<Value> {
        let outcome = apply(&self.entity, WriteOp::Update, entity_data, &self.ctx())
            .map_err(anyhow::Error::new)?;
        // The merge is the store's job (one atomic read-modify-write on ONE row); the
        // timestamp is ours. Splitting it the other way round would let two concurrent
        // updates overwrite each other's keys.
        let mut patch = outcome.value;
        if let Value::Object(map) = &mut patch {
            map.insert("updated_at".into(), json!(chrono::Utc::now().to_rfc3339()));
        }
        let row = self
            .rows
            .patch(entity_id, &patch)
            .await?
            .ok_or_else(|| self.not_found(entity_id))?;
        // No event here: see `create_from_json` (the guard and the REST handlers publish).
        Ok(row)
    }

    async fn delete(&self, entity_id: &Uuid) -> Result<()> {
        if !self.rows.remove(entity_id).await? {
            return Err(self.not_found(entity_id));
        }
        // No event here: see `create_from_json` (the guard and the REST handlers publish).
        Ok(())
    }
}

type Svc = State<Arc<JsonEntityService>>;

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

async fn list_handler(State(svc): Svc, Query(p): Query<ListParams>) -> Response {
    let schema = QuerySchema::for_entity(&svc.entity.desc);
    let q = match crate::descriptor::engine::query::parse_legacy(
        &schema,
        p.filter.as_deref(),
        p.sort.as_deref(),
        p.page,
        p.limit,
    ) {
        Ok(q) => q,
        Err(e) => return ApiError(e).into_response(),
    };
    match svc.list_query(&q).await {
        Ok((data, total)) => {
            let (page, limit) = match q.page {
                crate::descriptor::Page::Offset { page, limit } => (page as usize, limit as usize),
                _ => (p.page, p.limit),
            };
            axum::Json(json!({
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

async fn create_handler(State(svc): Svc, body: axum::Json<Value>) -> Response {
    match svc.create_from_json(body.0).await {
        Ok(v) => {
            svc.publish_created(&v);
            (StatusCode::CREATED, axum::Json(v)).into_response()
        }
        Err(e) => err(e),
    }
}

async fn get_handler(State(svc): Svc, Path(id): Path<Uuid>) -> Response {
    match svc.fetch_as_json(&id).await {
        Ok(v) => axum::Json(v).into_response(),
        Err(e) => err(e),
    }
}

async fn update_handler(
    State(svc): Svc,
    Path(id): Path<Uuid>,
    body: axum::Json<Value>,
) -> Response {
    match svc.update_from_json(&id, body.0).await {
        Ok(v) => {
            svc.publish_updated(&id, &v);
            axum::Json(v).into_response()
        }
        Err(e) => err(e),
    }
}

async fn delete_handler(State(svc): Svc, Path(id): Path<Uuid>) -> Response {
    match EntityCreator::delete(&*svc, &id).await {
        Ok(()) => {
            svc.publish_deleted(&id);
            StatusCode::NO_CONTENT.into_response()
        }
        Err(e) => err(e),
    }
}
