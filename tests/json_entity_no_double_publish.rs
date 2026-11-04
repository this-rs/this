//! A write to a DECLARATIVE entity (served by `JsonEntityService`) emits exactly ONE
//! `EntityEvent`, whatever the surface.
//!
//! # Why this test exists
//!
//! `JsonEntityService` (the adapter of declarative / runtime-loaded modules) used to
//! publish its own `EntityEvent` *and* is registered in the host catalog, so any surface
//! that writes through `SurfaceGuard::creator` (GraphQL, gRPC, createAndLink) received the
//! event **twice** — once from the service itself, once from the guard's `PublishingCreator`
//! decorator. This test mounts a real `Host` with an `EventBus` and a declarative module,
//! then writes through the guard seam and through the service's own REST routes, and
//! asserts exactly one event per write on both paths.
//!
//! Run: `cargo test -p this-rs --features grpc,graphql,websocket --test json_entity_no_double_publish`.

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use serde_json::{Value, json};
use tower::ServiceExt;
use uuid::Uuid;

use this::core::auth::AuthContext;
use this::core::events::{EntityEvent, FrameworkEvent};
use this::descriptor::{EntityDesc, FieldDesc, FieldType, ModuleDescriptor, Operation, PolicyExpr};
use this::server::RestExposure;
use this::server::builder::ServerBuilder;
use this::server::exposure::SurfaceGuard;
use this::server::host::ServerHost;
use this::server::module_runtime::{ModuleRuntime, ModuleSource};
use this::storage::in_memory::InMemoryLinkService;

/// A declarative module (`billing`) with one entity (`invoice`). Stored modules have no
/// implicit default policies, so every CRUD operation is declared explicitly.
fn billing() -> ModuleDescriptor {
    let mut e = EntityDesc::new("invoice");
    e.fields.push(FieldDesc::new("amount", FieldType::F64));
    for op in Operation::CRUD {
        e.permissions.ops.insert(op, PolicyExpr::Authenticated);
    }
    let mut d = ModuleDescriptor::new("billing", "1.0.0");
    d.entities.push(e);
    d.normalize();
    d
}

/// A host with a module runtime and an event bus, hot loading enabled, policies disarmed.
async fn host_with_declarative_invoice() -> Arc<ServerHost> {
    let host = ServerBuilder::new()
        .with_module_runtime()
        .with_hot_load_enabled(true)
        .with_link_service(InMemoryLinkService::new())
        .with_event_bus(256)
        .without_authorization()
        .build_host()
        .expect("host");
    let host = Arc::new(host);
    let runtime: &Arc<ModuleRuntime> = host.module_runtime.as_ref().expect("runtime enabled");
    let status = runtime
        .load(ModuleSource::Manifest(billing().to_json_pretty()))
        .await
        .expect("load declarative module");
    assert_eq!(
        status.state,
        this::descriptor::ModuleState::Active,
        "declarative module must be active"
    );
    host
}

/// Everything published so far, as (kind, action, entity_id) for entity events only.
fn drain(
    rx: &mut tokio::sync::broadcast::Receiver<this::core::events::EventEnvelope>,
) -> Vec<(String, String, Uuid)> {
    let mut out = vec![];
    while let Ok(env) = rx.try_recv() {
        if let FrameworkEvent::Entity(e) = env.event {
            let (action, id) = match e {
                EntityEvent::Created { entity_id, .. } => ("created".to_string(), entity_id),
                EntityEvent::Updated { entity_id, .. } => ("updated".to_string(), entity_id),
                EntityEvent::Deleted { entity_id, .. } => ("deleted".to_string(), entity_id),
            };
            out.push(("entity".to_string(), action, id));
        }
    }
    out
}

/// The shared write seam (`SurfaceGuard::creator`) publishes exactly one event per write on
/// a declarative entity — not two (one from the service, one from the decorator).
#[tokio::test]
async fn guard_seam_write_emits_exactly_one_event_each() {
    let host = host_with_declarative_invoice().await;
    let mut rx = host.event_bus().expect("event bus configured").subscribe();

    let guard = SurfaceGuard::for_host(host.as_ref(), AuthContext::Anonymous, "test");
    let creator = guard
        .creator(host.as_ref(), "invoice", Operation::Create, None)
        .expect("creator for invoice");

    // Create through the guard seam (GraphQL / gRPC / createAndLink all use this).
    let created = creator
        .create_from_json(json!({ "amount": 10.0 }))
        .await
        .expect("create");
    let id = Uuid::parse_str(created["id"].as_str().unwrap()).unwrap();
    assert_eq!(
        drain(&mut rx),
        vec![("entity".to_string(), "created".to_string(), id)],
        "exactly one `entity.created` event"
    );

    let creator = guard
        .creator(host.as_ref(), "invoice", Operation::Update, Some(&id))
        .expect("creator for update");
    creator
        .update_from_json(&id, json!({ "amount": 20.0 }))
        .await
        .expect("update");
    assert_eq!(
        drain(&mut rx),
        vec![("entity".to_string(), "updated".to_string(), id)],
        "exactly one `entity.updated` event"
    );

    let creator = guard
        .creator(host.as_ref(), "invoice", Operation::Delete, Some(&id))
        .expect("creator for delete");
    creator.delete(&id).await.expect("delete");
    assert_eq!(
        drain(&mut rx),
        vec![("entity".to_string(), "deleted".to_string(), id)],
        "exactly one `entity.deleted` event"
    );

    // Nothing else was published.
    assert!(rx.try_recv().is_err(), "no extra events");
}

/// The declarative service's own REST routes (the only write path that does not go through
/// `SurfaceGuard::creator`) must still emit exactly one event per write.
#[tokio::test]
async fn rest_write_emits_exactly_one_event_each() {
    let host = host_with_declarative_invoice().await;
    let mut rx = host.event_bus().expect("event bus configured").subscribe();
    let app = RestExposure::build_router(host.clone(), vec![]).expect("rest router");

    let create = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/invoices")
                .header("content-type", "application/json")
                .body(Body::from(json!({ "amount": 10.0 }).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(create.status(), StatusCode::CREATED);
    let body = axum::body::to_bytes(create.into_body(), usize::MAX)
        .await
        .unwrap();
    let row: Value = serde_json::from_slice(&body).unwrap();
    let id = Uuid::parse_str(row["id"].as_str().unwrap()).unwrap();
    assert_eq!(
        drain(&mut rx),
        vec![("entity".to_string(), "created".to_string(), id)],
        "exactly one `entity.created` event"
    );

    let update = app
        .clone()
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri(format!("/invoices/{id}"))
                .header("content-type", "application/json")
                .body(Body::from(json!({ "amount": 20.0 }).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(update.status(), StatusCode::OK);
    assert_eq!(
        drain(&mut rx),
        vec![("entity".to_string(), "updated".to_string(), id)],
        "exactly one `entity.updated` event"
    );

    let delete = app
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri(format!("/invoices/{id}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(delete.status(), StatusCode::NO_CONTENT);
    assert_eq!(
        drain(&mut rx),
        vec![("entity".to_string(), "deleted".to_string(), id)],
        "exactly one `entity.deleted` event"
    );
    assert!(rx.try_recv().is_err(), "no extra events");
}
