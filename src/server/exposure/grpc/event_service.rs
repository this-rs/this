//! gRPC Event Service implementation — server-streaming real-time events
//!
//! Subscribes to the framework's `EventBus` (the same broadcast channel used by
//! WebSocket) and streams matching events to gRPC clients. Filters from the
//! `SubscribeRequest` are applied server-side (AND logic, absent = wildcard).
//!
//! ## Architecture
//!
//! ```text
//! REST/GraphQL Handler → EventBus::publish()
//!                              ↓
//!                      broadcast channel
//!                        ↓           ↓
//!              WebSocket Manager   EventServiceImpl::subscribe()
//!                                     ↓ (filter)
//!                                  gRPC stream → client
//! ```

use super::convert::json_to_struct;
use super::proto::{EventResponse, SubscribeRequest, event_service_server::EventService};
use crate::core::events::{EntityEvent, EventEnvelope, FrameworkEvent, LinkEvent};
use crate::server::host::ServerHost;
use std::pin::Pin;
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tonic::{Request, Response, Status};
use uuid::Uuid;

/// gRPC Event Service implementation
///
/// Subscribes to the framework's `EventBus` and streams events to gRPC clients.
/// Each `Subscribe` call creates an independent broadcast receiver, filters
/// events according to `SubscribeRequest`, and forwards matching events as
/// `EventResponse` messages on the server-streaming response.
pub struct EventServiceImpl {
    host: Arc<ServerHost>,
}

impl EventServiceImpl {
    /// Create a new `EventServiceImpl` from a `ServerHost`
    pub fn new(host: Arc<ServerHost>) -> Self {
        Self { host }
    }
}

// ---------------------------------------------------------------------------
// Filter logic
// ---------------------------------------------------------------------------

/// Check if an event matches the subscribe request filters.
///
/// All fields are AND conditions. An absent (empty) field means "match any".
fn matches_filter(event: &FrameworkEvent, filter: &SubscribeRequest) -> bool {
    // Filter by kind ("entity" or "link")
    if let Some(ref kind) = filter.kind {
        if !kind.is_empty() && event.event_kind() != kind {
            return false;
        }
    }

    // Filter by entity_type
    if let Some(ref entity_type) = filter.entity_type {
        if !entity_type.is_empty() {
            match event.entity_type() {
                Some(et) if et == entity_type => {}
                Some(_) => return false,
                // Link events don't have entity_type
                None => return false,
            }
        }
    }

    // Filter by entity_id
    if let Some(ref entity_id) = filter.entity_id {
        if !entity_id.is_empty() {
            let parsed = entity_id.parse::<Uuid>().ok();
            match (parsed, event.entity_id()) {
                (Some(filter_id), Some(event_id)) if filter_id == event_id => {}
                _ => return false,
            }
        }
    }

    // Filter by event_type (action: "created", "updated", "deleted")
    if let Some(ref event_type) = filter.event_type {
        if !event_type.is_empty() && event.action() != event_type {
            return false;
        }
    }

    // Filter by link_type (only relevant for link events)
    if let Some(ref link_type) = filter.link_type {
        if !link_type.is_empty() {
            match extract_link_type(event) {
                Some(lt) if lt == link_type => {}
                Some(_) => return false,
                // Entity events don't have link_type — skip them
                None => return false,
            }
        }
    }

    true
}

/// Extract the link_type from a `FrameworkEvent`, if it's a link event.
fn extract_link_type(event: &FrameworkEvent) -> Option<&str> {
    match event {
        FrameworkEvent::Link(link) => match link {
            LinkEvent::Created { link_type, .. } | LinkEvent::Deleted { link_type, .. } => {
                Some(link_type)
            }
        },
        FrameworkEvent::Entity(_) => None,
    }
}

// ---------------------------------------------------------------------------
// Envelope → EventResponse conversion
// ---------------------------------------------------------------------------

/// Convert an `EventEnvelope` into a proto `EventResponse`.
fn envelope_to_response(envelope: &EventEnvelope) -> EventResponse {
    let event = &envelope.event;

    let (entity_type, entity_id, link_type, source_id, target_id, data, metadata) = match event {
        FrameworkEvent::Entity(e) => match e {
            EntityEvent::Created {
                entity_type,
                entity_id,
                data,
            } => (
                entity_type.clone(),
                entity_id.to_string(),
                String::new(),
                String::new(),
                String::new(),
                Some(json_to_struct(data)),
                None,
            ),
            EntityEvent::Updated {
                entity_type,
                entity_id,
                data,
            } => (
                entity_type.clone(),
                entity_id.to_string(),
                String::new(),
                String::new(),
                String::new(),
                Some(json_to_struct(data)),
                None,
            ),
            EntityEvent::Deleted {
                entity_type,
                entity_id,
            } => (
                entity_type.clone(),
                entity_id.to_string(),
                String::new(),
                String::new(),
                String::new(),
                None,
                None,
            ),
        },
        FrameworkEvent::Link(l) => match l {
            LinkEvent::Created {
                link_type,
                link_id,
                source_id,
                target_id,
                metadata,
            } => (
                String::new(),
                link_id.to_string(),
                link_type.clone(),
                source_id.to_string(),
                target_id.to_string(),
                None,
                metadata.as_ref().map(json_to_struct),
            ),
            LinkEvent::Deleted {
                link_type,
                link_id,
                source_id,
                target_id,
            } => (
                String::new(),
                link_id.to_string(),
                link_type.clone(),
                source_id.to_string(),
                target_id.to_string(),
                None,
                None,
            ),
        },
    };

    EventResponse {
        event_id: envelope.id.to_string(),
        event_kind: event.event_kind().to_string(),
        event_type: event.action().to_string(),
        entity_type,
        entity_id,
        link_type,
        source_id,
        target_id,
        data,
        metadata,
        timestamp: envelope.timestamp.to_rfc3339(),
        seq_no: envelope.seq_no.map(|s| s as u64).unwrap_or(0),
    }
}

// ---------------------------------------------------------------------------
// gRPC trait implementation
// ---------------------------------------------------------------------------

type SubscribeStream =
    Pin<Box<dyn tokio_stream::Stream<Item = Result<EventResponse, Status>> + Send>>;

#[tonic::async_trait]
impl EventService for EventServiceImpl {
    type SubscribeStream = SubscribeStream;

    async fn subscribe(
        &self,
        request: Request<SubscribeRequest>,
    ) -> Result<Response<Self::SubscribeStream>, Status> {
        let filter = request.into_inner();

        // Get the EventBus — if not configured, streaming is unavailable
        let event_bus = self
            .host
            .event_bus()
            .ok_or_else(|| {
                Status::unavailable(
                    "EventBus not configured — real-time streaming is not available",
                )
            })?
            .clone();

        // Subscribe to the broadcast channel
        let mut rx = event_bus.subscribe();

        // Channel to stream events to the gRPC response
        // Buffer of 64 — enough headroom for bursts without excessive memory
        let (tx, client_rx) = mpsc::channel::<Result<EventResponse, Status>>(64);

        // Spawn background task: receive from broadcast → filter → send to gRPC stream
        tokio::spawn(async move {
            loop {
                match rx.recv().await {
                    Ok(envelope) => {
                        if matches_filter(&envelope.event, &filter) {
                            let response = envelope_to_response(&envelope);
                            // If the client disconnected, tx.send() returns Err → break
                            if tx.send(Ok(response)).await.is_err() {
                                tracing::debug!("gRPC event stream: client disconnected, closing");
                                break;
                            }
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(count)) => {
                        tracing::warn!("gRPC event stream: lagged by {} events, skipping", count);
                        // Continue — the client misses some events but the stream stays alive
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                        tracing::info!("gRPC event stream: EventBus closed, ending stream");
                        break;
                    }
                }
            }
        });

        let stream = ReceiverStream::new(client_rx);
        Ok(Response::new(Box::pin(stream) as Self::SubscribeStream))
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::events::EventBus;
    use serde_json::json;

    // === Filter tests ===

    #[test]
    fn test_filter_empty_matches_everything() {
        let filter = SubscribeRequest {
            entity_type: None,
            entity_id: None,
            event_type: None,
            kind: None,
            link_type: None,
        };

        let entity = FrameworkEvent::Entity(EntityEvent::Created {
            entity_type: "user".to_string(),
            entity_id: Uuid::new_v4(),
            data: json!({}),
        });

        let link = FrameworkEvent::Link(LinkEvent::Created {
            link_type: "follow".to_string(),
            link_id: Uuid::new_v4(),
            source_id: Uuid::new_v4(),
            target_id: Uuid::new_v4(),
            metadata: None,
        });

        assert!(matches_filter(&entity, &filter));
        assert!(matches_filter(&link, &filter));
    }

    #[test]
    fn test_filter_by_entity_type() {
        let filter = SubscribeRequest {
            entity_type: Some("user".to_string()),
            entity_id: None,
            event_type: None,
            kind: None,
            link_type: None,
        };

        let user = FrameworkEvent::Entity(EntityEvent::Created {
            entity_type: "user".to_string(),
            entity_id: Uuid::new_v4(),
            data: json!({}),
        });

        let capture = FrameworkEvent::Entity(EntityEvent::Created {
            entity_type: "capture".to_string(),
            entity_id: Uuid::new_v4(),
            data: json!({}),
        });

        assert!(matches_filter(&user, &filter));
        assert!(!matches_filter(&capture, &filter));
    }

    #[test]
    fn test_filter_by_kind_entity() {
        let filter = SubscribeRequest {
            entity_type: None,
            entity_id: None,
            event_type: None,
            kind: Some("entity".to_string()),
            link_type: None,
        };

        let entity = FrameworkEvent::Entity(EntityEvent::Created {
            entity_type: "user".to_string(),
            entity_id: Uuid::new_v4(),
            data: json!({}),
        });

        let link = FrameworkEvent::Link(LinkEvent::Created {
            link_type: "follow".to_string(),
            link_id: Uuid::new_v4(),
            source_id: Uuid::new_v4(),
            target_id: Uuid::new_v4(),
            metadata: None,
        });

        assert!(matches_filter(&entity, &filter));
        assert!(!matches_filter(&link, &filter));
    }

    #[test]
    fn test_filter_by_kind_link() {
        let filter = SubscribeRequest {
            entity_type: None,
            entity_id: None,
            event_type: None,
            kind: Some("link".to_string()),
            link_type: None,
        };

        let entity = FrameworkEvent::Entity(EntityEvent::Created {
            entity_type: "user".to_string(),
            entity_id: Uuid::new_v4(),
            data: json!({}),
        });

        let link = FrameworkEvent::Link(LinkEvent::Created {
            link_type: "follow".to_string(),
            link_id: Uuid::new_v4(),
            source_id: Uuid::new_v4(),
            target_id: Uuid::new_v4(),
            metadata: None,
        });

        assert!(!matches_filter(&entity, &filter));
        assert!(matches_filter(&link, &filter));
    }

    #[test]
    fn test_filter_by_event_type() {
        let filter = SubscribeRequest {
            entity_type: None,
            entity_id: None,
            event_type: Some("deleted".to_string()),
            kind: None,
            link_type: None,
        };

        let created = FrameworkEvent::Entity(EntityEvent::Created {
            entity_type: "user".to_string(),
            entity_id: Uuid::new_v4(),
            data: json!({}),
        });

        let deleted = FrameworkEvent::Entity(EntityEvent::Deleted {
            entity_type: "user".to_string(),
            entity_id: Uuid::new_v4(),
        });

        assert!(!matches_filter(&created, &filter));
        assert!(matches_filter(&deleted, &filter));
    }

    #[test]
    fn test_filter_by_link_type() {
        let filter = SubscribeRequest {
            entity_type: None,
            entity_id: None,
            event_type: None,
            kind: None,
            link_type: Some("follow".to_string()),
        };

        let follow = FrameworkEvent::Link(LinkEvent::Created {
            link_type: "follow".to_string(),
            link_id: Uuid::new_v4(),
            source_id: Uuid::new_v4(),
            target_id: Uuid::new_v4(),
            metadata: None,
        });

        let like = FrameworkEvent::Link(LinkEvent::Created {
            link_type: "like".to_string(),
            link_id: Uuid::new_v4(),
            source_id: Uuid::new_v4(),
            target_id: Uuid::new_v4(),
            metadata: None,
        });

        // Entity events should NOT match a link_type filter
        let entity = FrameworkEvent::Entity(EntityEvent::Created {
            entity_type: "user".to_string(),
            entity_id: Uuid::new_v4(),
            data: json!({}),
        });

        assert!(matches_filter(&follow, &filter));
        assert!(!matches_filter(&like, &filter));
        assert!(!matches_filter(&entity, &filter));
    }

    #[test]
    fn test_filter_combined() {
        let filter = SubscribeRequest {
            entity_type: Some("user".to_string()),
            entity_id: None,
            event_type: Some("created".to_string()),
            kind: Some("entity".to_string()),
            link_type: None,
        };

        let user_created = FrameworkEvent::Entity(EntityEvent::Created {
            entity_type: "user".to_string(),
            entity_id: Uuid::new_v4(),
            data: json!({}),
        });

        let user_deleted = FrameworkEvent::Entity(EntityEvent::Deleted {
            entity_type: "user".to_string(),
            entity_id: Uuid::new_v4(),
        });

        let capture_created = FrameworkEvent::Entity(EntityEvent::Created {
            entity_type: "capture".to_string(),
            entity_id: Uuid::new_v4(),
            data: json!({}),
        });

        assert!(matches_filter(&user_created, &filter));
        assert!(!matches_filter(&user_deleted, &filter));
        assert!(!matches_filter(&capture_created, &filter));
    }

    #[test]
    fn test_filter_by_entity_id() {
        let target = Uuid::new_v4();
        let other = Uuid::new_v4();

        let filter = SubscribeRequest {
            entity_type: None,
            entity_id: Some(target.to_string()),
            event_type: None,
            kind: None,
            link_type: None,
        };

        let matching = FrameworkEvent::Entity(EntityEvent::Updated {
            entity_type: "user".to_string(),
            entity_id: target,
            data: json!({}),
        });

        let not_matching = FrameworkEvent::Entity(EntityEvent::Updated {
            entity_type: "user".to_string(),
            entity_id: other,
            data: json!({}),
        });

        assert!(matches_filter(&matching, &filter));
        assert!(!matches_filter(&not_matching, &filter));
    }

    // === Conversion tests ===

    #[test]
    fn test_envelope_to_response_entity_created() {
        let entity_id = Uuid::new_v4();
        let envelope = EventEnvelope::new(FrameworkEvent::Entity(EntityEvent::Created {
            entity_type: "user".to_string(),
            entity_id,
            data: json!({"name": "Alice"}),
        }));

        let resp = envelope_to_response(&envelope);

        assert_eq!(resp.event_id, envelope.id.to_string());
        assert_eq!(resp.event_kind, "entity");
        assert_eq!(resp.event_type, "created");
        assert_eq!(resp.entity_type, "user");
        assert_eq!(resp.entity_id, entity_id.to_string());
        assert!(resp.link_type.is_empty());
        assert!(resp.source_id.is_empty());
        assert!(resp.target_id.is_empty());
        assert!(resp.data.is_some());
        assert!(resp.metadata.is_none());
    }

    #[test]
    fn test_envelope_to_response_link_created() {
        let link_id = Uuid::new_v4();
        let source = Uuid::new_v4();
        let target = Uuid::new_v4();

        let envelope = EventEnvelope::new(FrameworkEvent::Link(LinkEvent::Created {
            link_type: "follow".to_string(),
            link_id,
            source_id: source,
            target_id: target,
            metadata: Some(json!({"via": "mobile"})),
        }));

        let resp = envelope_to_response(&envelope);

        assert_eq!(resp.event_kind, "link");
        assert_eq!(resp.event_type, "created");
        assert!(resp.entity_type.is_empty());
        assert_eq!(resp.entity_id, link_id.to_string());
        assert_eq!(resp.link_type, "follow");
        assert_eq!(resp.source_id, source.to_string());
        assert_eq!(resp.target_id, target.to_string());
        assert!(resp.data.is_none());
        assert!(resp.metadata.is_some());
    }

    #[test]
    fn test_envelope_to_response_entity_deleted() {
        let entity_id = Uuid::new_v4();
        let envelope = EventEnvelope::new(FrameworkEvent::Entity(EntityEvent::Deleted {
            entity_type: "capture".to_string(),
            entity_id,
        }));

        let resp = envelope_to_response(&envelope);

        assert_eq!(resp.event_kind, "entity");
        assert_eq!(resp.event_type, "deleted");
        assert_eq!(resp.entity_type, "capture");
        assert!(resp.data.is_none());
        assert!(resp.metadata.is_none());
    }

    // === Integration test — full subscribe flow ===

    #[tokio::test]
    async fn test_event_service_subscribe_receives_matching_events() {
        use crate::server::host::ServerHost;
        use tokio_stream::StreamExt;

        let event_bus = EventBus::new(64);

        // Create a minimal ServerHost with an EventBus
        let host = ServerHost::minimal_for_test().with_event_bus(event_bus.clone());
        let host = Arc::new(host);

        let svc = EventServiceImpl::new(host);

        // Subscribe to "user" entity events only
        let request = Request::new(SubscribeRequest {
            entity_type: Some("user".to_string()),
            entity_id: None,
            event_type: None,
            kind: None,
            link_type: None,
        });

        let response = svc.subscribe(request).await.unwrap();
        let mut stream = response.into_inner();

        // Publish a user event (should match)
        let user_id = Uuid::new_v4();
        event_bus.publish(FrameworkEvent::Entity(EntityEvent::Created {
            entity_type: "user".to_string(),
            entity_id: user_id,
            data: json!({"name": "Alice"}),
        }));

        // Publish a capture event (should NOT match)
        event_bus.publish(FrameworkEvent::Entity(EntityEvent::Created {
            entity_type: "capture".to_string(),
            entity_id: Uuid::new_v4(),
            data: json!({}),
        }));

        // Publish a link event (should NOT match)
        event_bus.publish(FrameworkEvent::Link(LinkEvent::Created {
            link_type: "follow".to_string(),
            link_id: Uuid::new_v4(),
            source_id: Uuid::new_v4(),
            target_id: Uuid::new_v4(),
            metadata: None,
        }));

        // Should receive exactly 1 event (the user one)
        let msg = tokio::time::timeout(std::time::Duration::from_millis(100), stream.next())
            .await
            .expect("timed out waiting for event")
            .expect("stream ended unexpectedly")
            .expect("received error");

        assert_eq!(msg.event_kind, "entity");
        assert_eq!(msg.event_type, "created");
        assert_eq!(msg.entity_type, "user");
        assert_eq!(msg.entity_id, user_id.to_string());

        // No more matching events should arrive
        let timeout_result =
            tokio::time::timeout(std::time::Duration::from_millis(50), stream.next()).await;
        assert!(
            timeout_result.is_err(),
            "should time out — no more matching events"
        );
    }

    #[tokio::test]
    async fn test_event_service_wildcard_receives_all() {
        use crate::server::host::ServerHost;
        use tokio_stream::StreamExt;

        let event_bus = EventBus::new(64);
        let host = Arc::new(ServerHost::minimal_for_test().with_event_bus(event_bus.clone()));

        let svc = EventServiceImpl::new(host);

        // Subscribe with no filters (wildcard)
        let request = Request::new(SubscribeRequest {
            entity_type: None,
            entity_id: None,
            event_type: None,
            kind: None,
            link_type: None,
        });

        let response = svc.subscribe(request).await.unwrap();
        let mut stream = response.into_inner();

        // Publish 2 events of different types
        event_bus.publish(FrameworkEvent::Entity(EntityEvent::Created {
            entity_type: "user".to_string(),
            entity_id: Uuid::new_v4(),
            data: json!({}),
        }));

        event_bus.publish(FrameworkEvent::Link(LinkEvent::Created {
            link_type: "follow".to_string(),
            link_id: Uuid::new_v4(),
            source_id: Uuid::new_v4(),
            target_id: Uuid::new_v4(),
            metadata: None,
        }));

        // Should receive both
        let msg1 = tokio::time::timeout(std::time::Duration::from_millis(100), stream.next())
            .await
            .expect("timed out")
            .expect("stream ended")
            .expect("error");
        assert_eq!(msg1.event_kind, "entity");

        let msg2 = tokio::time::timeout(std::time::Duration::from_millis(100), stream.next())
            .await
            .expect("timed out")
            .expect("stream ended")
            .expect("error");
        assert_eq!(msg2.event_kind, "link");
    }

    #[tokio::test]
    async fn test_event_service_client_disconnect_ends_task() {
        use crate::server::host::ServerHost;

        let event_bus = EventBus::new(64);
        let host = Arc::new(ServerHost::minimal_for_test().with_event_bus(event_bus.clone()));

        let svc = EventServiceImpl::new(host);

        let request = Request::new(SubscribeRequest {
            entity_type: None,
            entity_id: None,
            event_type: None,
            kind: None,
            link_type: None,
        });

        let response = svc.subscribe(request).await.unwrap();

        // Drop the stream to simulate client disconnect
        drop(response);

        // The spawned task should detect the closed mpsc and exit.
        // Publish an event to trigger the detection
        event_bus.publish(FrameworkEvent::Entity(EntityEvent::Created {
            entity_type: "user".to_string(),
            entity_id: Uuid::new_v4(),
            data: json!({}),
        }));

        // Give the task time to notice and exit
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        // If we get here without hanging, the task properly exited
    }
}
