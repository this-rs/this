//! Links are isomorphic across protocols: whichever surface writes them, the event bus sees
//! exactly `[LinkEvent::Created, LinkEvent::Deleted]` for a create then a delete — published
//! once, by `EventedLinkService`, never by the surface itself.

#![cfg(all(feature = "grpc", feature = "graphql"))]

use std::collections::HashMap;
use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use tower::ServiceExt;
use uuid::Uuid;

use this::config::{EntityAuthConfig, EntityConfig, LinksConfig};
use this::core::events::{EventBus, FrameworkEvent, LinkEvent};
use this::core::link::{LinkDefinition, LinkEntity};
use this::server::entity_registry::EntityRegistry;
use this::server::exposure::RestExposure;
use this::server::host::ServerHost;
use this::storage::in_memory::InMemoryLinkService;

fn config() -> LinksConfig {
    LinksConfig {
        entities: vec![
            EntityConfig {
                singular: "order".into(),
                plural: "orders".into(),
                auth: EntityAuthConfig::default(),
            },
            EntityConfig {
                singular: "invoice".into(),
                plural: "invoices".into(),
                auth: EntityAuthConfig::default(),
            },
        ],
        links: vec![LinkDefinition {
            link_type: "has_invoice".into(),
            source_type: "order".into(),
            target_type: "invoice".into(),
            forward_route_name: "invoices".into(),
            reverse_route_name: "order".into(),
            description: None,
            required_fields: None,
            auth: None,
        }],
        validation_rules: None,
        events: None,
        sinks: None,
        auth: None,
    }
}

fn host() -> (
    Arc<ServerHost>,
    tokio::sync::broadcast::Receiver<this::core::events::EventEnvelope>,
) {
    let bus = EventBus::new(64);
    let rx = bus.subscribe();
    let mut host = ServerHost::from_builder_components(
        Arc::new(InMemoryLinkService::new()),
        config(),
        EntityRegistry::new(),
        HashMap::new(),
        HashMap::new(),
    )
    .expect("host")
    .with_event_bus(bus);
    host.authorize = false; // the subject here is events, not authorization
    (Arc::new(host), rx)
}

fn drain(
    rx: &mut tokio::sync::broadcast::Receiver<this::core::events::EventEnvelope>,
) -> Vec<LinkEvent> {
    let mut out = Vec::new();
    while let Ok(env) = rx.try_recv() {
        match env.event {
            FrameworkEvent::Link(e) => out.push(e),
            other => panic!("unexpected non-link event: {other:?}"),
        }
    }
    out
}

fn shape(events: &[LinkEvent]) -> Vec<&'static str> {
    events
        .iter()
        .map(|e| match e {
            LinkEvent::Created { .. } => "created",
            LinkEvent::Deleted { .. } => "deleted",
        })
        .collect()
}

#[tokio::test]
async fn rest_create_then_delete_emits_created_then_deleted() {
    let (host, mut rx) = host();
    let app = RestExposure::build_router(host, vec![]).expect("router");
    let (o, i) = (Uuid::new_v4(), Uuid::new_v4());
    let uri = format!("/orders/{o}/invoices/{i}");

    let res = app
        .clone()
        .oneshot(
            Request::post(&uri)
                .header("content-type", "application/json")
                .body(Body::from("{}"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::CREATED);
    let res = app
        .clone()
        .oneshot(Request::delete(&uri).body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::NO_CONTENT);

    let events = drain(&mut rx);
    assert_eq!(shape(&events), ["created", "deleted"], "{events:?}");
}

#[tokio::test]
async fn grpc_create_then_delete_emits_created_then_deleted() {
    use this::server::exposure::grpc::link_service::LinkServiceImpl;
    use this::server::exposure::grpc::proto::link_service_server::LinkService as _;
    use this::server::exposure::grpc::proto::{CreateLinkRequest, DeleteLinkRequest};

    let (host, mut rx) = host();
    let svc = LinkServiceImpl::unenforced(host);
    let (o, i) = (Uuid::new_v4(), Uuid::new_v4());

    let created = svc
        .create_link(tonic::Request::new(CreateLinkRequest {
            link_type: "has_invoice".into(),
            source_id: o.to_string(),
            target_id: i.to_string(),
            metadata: None,
        }))
        .await
        .expect("create")
        .into_inner();
    svc.delete_link(tonic::Request::new(DeleteLinkRequest {
        link_id: created.id.clone(),
    }))
    .await
    .expect("delete");

    let events = drain(&mut rx);
    assert_eq!(shape(&events), ["created", "deleted"], "{events:?}");
}

#[tokio::test]
async fn service_level_semantics_no_phantom_and_per_link_events() {
    let (host, mut rx) = host();
    let svc = host.link_service.clone();
    let (a, b, c) = (Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4());

    // Deleting an unknown id: nothing published.
    let _ = svc.delete(&Uuid::new_v4()).await;
    assert!(drain(&mut rx).is_empty());

    let l1 = svc
        .create(LinkEntity::new("has_invoice", a, b, None))
        .await
        .unwrap();
    svc.create(LinkEntity::new("has_invoice", a, c, None))
        .await
        .unwrap();
    // Update is silent (no LinkEvent::Updated).
    svc.update(&l1.id, l1.clone()).await.unwrap();
    assert_eq!(shape(&drain(&mut rx)), ["created", "created"]);

    // delete_by_entity: one Deleted per removed link.
    svc.delete_by_entity(&a).await.unwrap();
    assert_eq!(shape(&drain(&mut rx)), ["deleted", "deleted"]);
    assert!(svc.list().await.unwrap().is_empty());
}
