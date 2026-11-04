//! Event contract test harness for storage backends.
//!
//! The storage backends implement `DataService<T>` / `LinkService`, which sit **below** the
//! protocol seam: protocols call `EntityCreator` / `EntityFetcher` (JSON), not `DataService`.
//! The framework's single write seam for events is `SurfaceGuard::creator`, which decorates the
//! catalog creator with a `PublishingCreator`. This module provides the same decorator
//! (`EventedCreator`) plus a canonical JSON adapter (`JsonDataCreator`), so the `event_contract_tests!`
//! macro can validate the event contract against ANY backend with the exact same assertions:
//!
//! - exactly one `EntityEvent` per successful write (`created`, `updated`, `deleted`), in order,
//! - no extra event,
//! - a failed write emits nothing,
//! - and the backend's behavior on deleting an absent id is documented (not silently fixed).

use async_trait::async_trait;
use serde_json::Value;
use std::sync::Arc;
use uuid::Uuid;

use super::{TestDataEntity, create_test_entity_with_id};
use this::core::events::{EntityEvent, EventBus, FrameworkEvent};
use this::core::module::EntityCreator;
use this::core::service::DataService;

/// Canonical JSON `EntityCreator` adapter over a `DataService<TestDataEntity>`.
///
/// This is the glue a user writes to expose a typed store through the framework's JSON
/// protocol seam (see `docs/audit-events-backends.md` §0). `create` generates an id and
/// timestamps through `create_test_entity_with_id`; `update` reads the existing row, merges
/// the JSON patch and persists it (the same merge pattern as the REST integration harness).
pub struct JsonDataCreator<S> {
    inner: S,
}

impl<S> JsonDataCreator<S> {
    pub fn new(inner: S) -> Self {
        Self { inner }
    }
}

#[async_trait]
impl<S> EntityCreator for JsonDataCreator<S>
where
    S: DataService<TestDataEntity>,
{
    async fn create_from_json(&self, data: Value) -> anyhow::Result<Value> {
        let id = data
            .get("id")
            .and_then(|v| v.as_str())
            .and_then(|s| Uuid::parse_str(s).ok())
            .unwrap_or_else(Uuid::new_v4);
        let name = data
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or("unnamed")
            .to_string();
        let email = data
            .get("email")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let age = data.get("age").and_then(|v| v.as_i64()).unwrap_or(0);
        let score = data.get("score").and_then(|v| v.as_f64()).unwrap_or(0.0);
        let active = data.get("active").and_then(|v| v.as_bool()).unwrap_or(true);

        let created = self
            .inner
            .create(create_test_entity_with_id(
                id, &name, &email, age, score, active,
            ))
            .await?;
        Ok(serde_json::to_value(created)?)
    }

    async fn update_from_json(&self, id: &Uuid, data: Value) -> anyhow::Result<Value> {
        let existing = self
            .inner
            .get(id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("not found"))?;
        let updated = TestDataEntity {
            name: data
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or(&existing.name)
                .to_string(),
            email: data
                .get("email")
                .and_then(|v| v.as_str())
                .unwrap_or(&existing.email)
                .to_string(),
            age: data
                .get("age")
                .and_then(|v| v.as_i64())
                .unwrap_or(existing.age),
            score: data
                .get("score")
                .and_then(|v| v.as_f64())
                .unwrap_or(existing.score),
            active: data
                .get("active")
                .and_then(|v| v.as_bool())
                .unwrap_or(existing.active),
            updated_at: chrono::Utc::now(),
            ..existing
        };
        let updated = self.inner.update(id, updated).await?;
        Ok(serde_json::to_value(updated)?)
    }

    async fn delete(&self, id: &Uuid) -> anyhow::Result<()> {
        self.inner.delete(id).await
    }
}

/// The event decorator of the framework's write seam — mirrors `SurfaceGuard::creator`'s
/// `PublishingCreator`: a successful write publishes exactly one `EntityEvent`, a failed
/// write publishes nothing, and a `Deleted` event is published for whatever id the backend
/// accepted (which reveals the "phantom delete" behavior documented by the audit).
pub struct EventedCreator {
    inner: Arc<dyn EntityCreator>,
    bus: Arc<EventBus>,
    entity: String,
}

impl EventedCreator {
    pub fn new(
        inner: Arc<dyn EntityCreator>,
        bus: Arc<EventBus>,
        entity: impl Into<String>,
    ) -> Self {
        Self {
            inner,
            bus,
            entity: entity.into(),
        }
    }
}

#[async_trait]
impl EntityCreator for EventedCreator {
    async fn create_from_json(&self, data: Value) -> anyhow::Result<Value> {
        let created = self.inner.create_from_json(data).await?;
        if let Some(entity_id) = created
            .get("id")
            .and_then(|v| v.as_str())
            .and_then(|s| Uuid::parse_str(s).ok())
        {
            self.bus
                .publish(FrameworkEvent::Entity(EntityEvent::Created {
                    entity_type: self.entity.clone(),
                    entity_id,
                    data: created.clone(),
                }));
        }
        Ok(created)
    }

    async fn update_from_json(&self, id: &Uuid, data: Value) -> anyhow::Result<Value> {
        let updated = self.inner.update_from_json(id, data).await?;
        self.bus
            .publish(FrameworkEvent::Entity(EntityEvent::Updated {
                entity_type: self.entity.clone(),
                entity_id: *id,
                data: updated.clone(),
            }));
        Ok(updated)
    }

    async fn delete(&self, id: &Uuid) -> anyhow::Result<()> {
        self.inner.delete(id).await?;
        self.bus
            .publish(FrameworkEvent::Entity(EntityEvent::Deleted {
                entity_type: self.entity.clone(),
                entity_id: *id,
            }));
        Ok(())
    }
}

/// Generate the event contract test suite for a storage backend.
///
/// `$data_factory` must produce an `impl DataService<TestDataEntity>`. It is re-evaluated
/// for each test (fresh store per test, same isolation as `data_service_tests!`).
#[macro_export]
macro_rules! event_contract_tests {
    ($data_factory:expr) => {
        mod event_contract_tests {
            use super::*;
            use serde_json::json;
            use std::sync::Arc;
            use this::core::events::EventBus;
            use this::core::module::EntityCreator;
            use uuid::Uuid;

            use storage_harness::event_contract::{EventedCreator, JsonDataCreator};

            /// Every event received so far, as `(kind, action, entity_id)`.
            fn drain(
                rx: &mut tokio::sync::broadcast::Receiver<this::core::events::EventEnvelope>,
            ) -> Vec<(String, String, Uuid)> {
                let mut out = vec![];
                while let Ok(env) = rx.try_recv() {
                    out.push((
                        env.event.event_kind().to_string(),
                        env.event.action().to_string(),
                        env.event.entity_id().unwrap_or_else(Uuid::nil),
                    ));
                }
                out
            }

            fn setup() -> (
                Arc<EventBus>,
                tokio::sync::broadcast::Receiver<this::core::events::EventEnvelope>,
                EventedCreator,
            ) {
                let bus = Arc::new(EventBus::new(64));
                let rx = bus.subscribe();
                let inner: Arc<dyn EntityCreator> = Arc::new(JsonDataCreator::new($data_factory));
                let creator = EventedCreator::new(inner, bus.clone(), "test_data");
                (bus, rx, creator)
            }

            /// create → update → delete emits exactly `[entity.created, entity.updated,
            /// entity.deleted]` and nothing else, whatever the backend.
            #[tokio::test]
            async fn create_update_delete_emit_exactly_one_event_each_in_order() {
                let (_bus, mut rx, creator) = setup();

                let created = creator
                    .create_from_json(json!({
                        "name": "a",
                        "email": "a@test.com",
                        "age": 1,
                        "score": 1.5,
                        "active": true
                    }))
                    .await
                    .unwrap();
                let id = Uuid::parse_str(created["id"].as_str().unwrap()).unwrap();
                assert_eq!(
                    drain(&mut rx),
                    vec![("entity".to_string(), "created".to_string(), id)],
                    "exactly one `entity.created` event"
                );

                creator
                    .update_from_json(&id, json!({ "name": "b" }))
                    .await
                    .unwrap();
                assert_eq!(
                    drain(&mut rx),
                    vec![("entity".to_string(), "updated".to_string(), id)],
                    "exactly one `entity.updated` event"
                );

                creator.delete(&id).await.unwrap();
                assert_eq!(
                    drain(&mut rx),
                    vec![("entity".to_string(), "deleted".to_string(), id)],
                    "exactly one `entity.deleted` event"
                );

                assert!(rx.try_recv().is_err(), "no extra events");
            }

            /// A failed write emits nothing.
            #[tokio::test]
            async fn failed_write_emits_nothing() {
                let (_bus, mut rx, creator) = setup();

                let res = creator
                    .update_from_json(&Uuid::new_v4(), json!({ "name": "x" }))
                    .await;
                assert!(res.is_err(), "updating a nonexistent id must fail");
                assert!(rx.try_recv().is_err(), "a failed write emits no event");
            }

            /// Deleting an absent id — DOCUMENTS the current backend behavior, not a fix.
            ///
            /// The audit (`docs/audit-events-backends.md` §1) found that all nine backends
            /// return `Ok(())` when asked to delete an id that never existed, so the
            /// `EventedCreator` decorator publishes a **phantom** `entity.deleted` for that
            /// id. This test locks that measured behavior: if a backend is later made strict
            /// (delete of an absent id → `Err`), this test's `Err` branch flips and the
            /// change becomes a deliberate, visible contract decision instead of an accident.
            #[tokio::test]
            async fn delete_of_absent_id_is_documented() {
                let (_bus, mut rx, creator) = setup();
                let absent = Uuid::new_v4();

                match creator.delete(&absent).await {
                    Ok(()) => {
                        // Idempotent delete (in-memory, lmdb, ... style): a phantom
                        // `entity.deleted` is emitted for the absent id.
                        assert_eq!(
                            drain(&mut rx),
                            vec![("entity".to_string(), "deleted".to_string(), absent)],
                            "backend returns Ok on an absent id → the decorator emits a phantom `deleted`"
                        );
                    }
                    Err(_) => {
                        // Strict delete (SQL style): no event.
                        assert!(rx.try_recv().is_err());
                    }
                }
            }
        }
    };
}
