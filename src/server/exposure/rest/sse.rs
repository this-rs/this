//! Server-Sent Events (SSE) endpoint for real-time event streaming
//!
//! Provides a `GET /events/stream` endpoint that streams events from
//! the EventBus as SSE (text/event-stream). Supports filtering by
//! query parameters and sends heartbeat comments every 30 seconds.
//!
//! # Query parameters
//!
//! - `kind` — Filter by event kind: "entity" or "link"
//! - `entity_type` — Filter by entity type (e.g., "user", "order") or link type (e.g., "follows")
//! - `event_type` — Filter by action: "created", "updated", "deleted"
//!
//! All filters are optional. When absent, all events are streamed.
//!
//! # Example
//!
//! ```text
//! GET /events/stream?kind=entity&entity_type=user
//!
//! data: {"kind":"entity","action":"created","entity_type":"user","entity_id":"...","data":{...},"timestamp":"..."}
//!
//! : heartbeat
//!
//! data: {"kind":"entity","action":"updated","entity_type":"user","entity_id":"...","data":{...},"timestamp":"..."}
//! ```

use crate::core::events::{EntityEvent, EventBus, EventEnvelope, FrameworkEvent, LinkEvent};
use axum::extract::{Query, State};
use axum::response::sse::{Event, KeepAlive, Sse};
use futures::StreamExt;
use futures::stream::Stream;
use serde::Deserialize;
use serde_json::json;
use std::convert::Infallible;
use std::sync::Arc;
use std::time::Duration;
use tokio_stream::wrappers::BroadcastStream;

/// Query parameters for SSE event filtering
#[derive(Debug, Deserialize, Default)]
pub struct SseFilter {
    /// Filter by event kind: "entity" or "link"
    pub kind: Option<String>,

    /// Filter by entity type (e.g., "user", "order") or link type (e.g., "follows")
    pub entity_type: Option<String>,

    /// Filter by action: "created", "updated", "deleted"
    pub event_type: Option<String>,
}

/// SSE event stream handler
///
/// Subscribes to the EventBus and streams matching events as SSE.
/// Sends heartbeat comments every 30 seconds to keep the connection alive.
pub async fn sse_handler(
    State(event_bus): State<Arc<EventBus>>,
    Query(filter): Query<SseFilter>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let rx = event_bus.subscribe();

    let stream = BroadcastStream::new(rx).filter_map(move |result| {
        let item = match result {
            Ok(envelope) => {
                if matches_filter(&envelope, &filter) {
                    envelope_to_sse_event(&envelope).map(Ok)
                } else {
                    None
                }
            }
            Err(tokio_stream::wrappers::errors::BroadcastStreamRecvError::Lagged(n)) => {
                tracing::warn!(missed = n, "SSE client lagged, missed events");
                let warning = Event::default()
                    .event("warning")
                    .data(format!("missed {} events due to slow consumption", n));
                Some(Ok(warning))
            }
        };
        std::future::ready(item)
    });

    Sse::new(stream).keep_alive(
        KeepAlive::new()
            .interval(Duration::from_secs(30))
            .text("heartbeat"),
    )
}

/// Check if an event envelope matches the SSE filter
fn matches_filter(envelope: &EventEnvelope, filter: &SseFilter) -> bool {
    // Filter by kind (entity / link)
    if let Some(ref kind) = filter.kind
        && envelope.event.event_kind() != kind
    {
        return false;
    }

    // Filter by entity_type (matches entity_type for entities, link_type for links)
    if let Some(ref entity_type) = filter.entity_type {
        let matches = match &envelope.event {
            FrameworkEvent::Entity(e) => match e {
                EntityEvent::Created {
                    entity_type: et, ..
                }
                | EntityEvent::Updated {
                    entity_type: et, ..
                }
                | EntityEvent::Deleted {
                    entity_type: et, ..
                } => et == entity_type,
            },
            FrameworkEvent::Link(l) => match l {
                LinkEvent::Created { link_type: lt, .. }
                | LinkEvent::Deleted { link_type: lt, .. } => lt == entity_type,
            },
        };
        if !matches {
            return false;
        }
    }

    // Filter by event_type (created, updated, deleted)
    if let Some(ref event_type) = filter.event_type
        && envelope.event.action() != event_type
    {
        return false;
    }

    true
}

/// Convert an EventEnvelope to an SSE Event
fn envelope_to_sse_event(envelope: &EventEnvelope) -> Option<Event> {
    let data = match &envelope.event {
        FrameworkEvent::Entity(e) => match e {
            EntityEvent::Created {
                entity_type,
                entity_id,
                data,
            }
            | EntityEvent::Updated {
                entity_type,
                entity_id,
                data,
            } => json!({
                "kind": "entity",
                "action": envelope.event.action(),
                "entity_type": entity_type,
                "entity_id": entity_id,
                "data": data,
                "timestamp": envelope.timestamp.to_rfc3339(),
            }),
            EntityEvent::Deleted {
                entity_type,
                entity_id,
            } => json!({
                "kind": "entity",
                "action": "deleted",
                "entity_type": entity_type,
                "entity_id": entity_id,
                "timestamp": envelope.timestamp.to_rfc3339(),
            }),
        },
        FrameworkEvent::Link(l) => match l {
            LinkEvent::Created {
                link_type,
                link_id,
                source_id,
                target_id,
                metadata,
            } => json!({
                "kind": "link",
                "action": "created",
                "link_type": link_type,
                "link_id": link_id,
                "source_id": source_id,
                "target_id": target_id,
                "metadata": metadata,
                "timestamp": envelope.timestamp.to_rfc3339(),
            }),
            LinkEvent::Deleted {
                link_type,
                link_id,
                source_id,
                target_id,
            } => json!({
                "kind": "link",
                "action": "deleted",
                "link_type": link_type,
                "link_id": link_id,
                "source_id": source_id,
                "target_id": target_id,
                "timestamp": envelope.timestamp.to_rfc3339(),
            }),
        },
    };

    let json_str = serde_json::to_string(&data).ok()?;
    Some(Event::default().data(json_str))
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    fn make_entity_envelope(entity_type: &str, action: &str) -> EventEnvelope {
        let event = match action {
            "created" => FrameworkEvent::Entity(EntityEvent::Created {
                entity_type: entity_type.to_string(),
                entity_id: Uuid::new_v4(),
                data: json!({"name": "test"}),
            }),
            "updated" => FrameworkEvent::Entity(EntityEvent::Updated {
                entity_type: entity_type.to_string(),
                entity_id: Uuid::new_v4(),
                data: json!({"name": "updated"}),
            }),
            "deleted" => FrameworkEvent::Entity(EntityEvent::Deleted {
                entity_type: entity_type.to_string(),
                entity_id: Uuid::new_v4(),
            }),
            _ => unreachable!(),
        };

        EventEnvelope::new(event)
    }

    fn make_link_envelope(link_type: &str, action: &str) -> EventEnvelope {
        let event = match action {
            "created" => FrameworkEvent::Link(LinkEvent::Created {
                link_type: link_type.to_string(),
                link_id: Uuid::new_v4(),
                source_id: Uuid::new_v4(),
                target_id: Uuid::new_v4(),
                metadata: None,
            }),
            "deleted" => FrameworkEvent::Link(LinkEvent::Deleted {
                link_type: link_type.to_string(),
                link_id: Uuid::new_v4(),
                source_id: Uuid::new_v4(),
                target_id: Uuid::new_v4(),
            }),
            _ => unreachable!(),
        };

        EventEnvelope::new(event)
    }

    #[test]
    fn test_matches_filter_no_filter() {
        let envelope = make_entity_envelope("user", "created");
        let filter = SseFilter::default();
        assert!(matches_filter(&envelope, &filter));
    }

    #[test]
    fn test_matches_filter_kind_entity() {
        let envelope = make_entity_envelope("user", "created");
        let filter = SseFilter {
            kind: Some("entity".to_string()),
            ..Default::default()
        };
        assert!(matches_filter(&envelope, &filter));

        let filter = SseFilter {
            kind: Some("link".to_string()),
            ..Default::default()
        };
        assert!(!matches_filter(&envelope, &filter));
    }

    #[test]
    fn test_matches_filter_kind_link() {
        let envelope = make_link_envelope("follows", "created");
        let filter = SseFilter {
            kind: Some("link".to_string()),
            ..Default::default()
        };
        assert!(matches_filter(&envelope, &filter));

        let filter = SseFilter {
            kind: Some("entity".to_string()),
            ..Default::default()
        };
        assert!(!matches_filter(&envelope, &filter));
    }

    #[test]
    fn test_matches_filter_entity_type() {
        let envelope = make_entity_envelope("user", "created");
        let filter = SseFilter {
            entity_type: Some("user".to_string()),
            ..Default::default()
        };
        assert!(matches_filter(&envelope, &filter));

        let filter = SseFilter {
            entity_type: Some("order".to_string()),
            ..Default::default()
        };
        assert!(!matches_filter(&envelope, &filter));
    }

    #[test]
    fn test_matches_filter_event_type() {
        let envelope = make_entity_envelope("user", "created");
        let filter = SseFilter {
            event_type: Some("created".to_string()),
            ..Default::default()
        };
        assert!(matches_filter(&envelope, &filter));

        let filter = SseFilter {
            event_type: Some("deleted".to_string()),
            ..Default::default()
        };
        assert!(!matches_filter(&envelope, &filter));
    }

    #[test]
    fn test_matches_filter_combined() {
        let envelope = make_entity_envelope("user", "created");
        let filter = SseFilter {
            kind: Some("entity".to_string()),
            entity_type: Some("user".to_string()),
            event_type: Some("created".to_string()),
        };
        assert!(matches_filter(&envelope, &filter));

        // Wrong event_type
        let filter = SseFilter {
            kind: Some("entity".to_string()),
            entity_type: Some("user".to_string()),
            event_type: Some("deleted".to_string()),
        };
        assert!(!matches_filter(&envelope, &filter));
    }

    #[test]
    fn test_envelope_to_sse_event_entity_created() {
        let envelope = make_entity_envelope("user", "created");
        let event = envelope_to_sse_event(&envelope);
        assert!(event.is_some());
    }

    #[test]
    fn test_envelope_to_sse_event_entity_deleted() {
        let envelope = make_entity_envelope("user", "deleted");
        let event = envelope_to_sse_event(&envelope);
        assert!(event.is_some());
    }

    #[test]
    fn test_envelope_to_sse_event_link_created() {
        let envelope = make_link_envelope("follows", "created");
        let event = envelope_to_sse_event(&envelope);
        assert!(event.is_some());
    }

    #[test]
    fn test_envelope_to_sse_event_link_deleted() {
        let envelope = make_link_envelope("follows", "deleted");
        let event = envelope_to_sse_event(&envelope);
        assert!(event.is_some());
    }

    #[test]
    fn test_link_filter_by_link_type() {
        let envelope = make_link_envelope("follows", "created");

        // Matches link_type
        let filter = SseFilter {
            entity_type: Some("follows".to_string()),
            ..Default::default()
        };
        assert!(matches_filter(&envelope, &filter));

        // Doesn't match different type
        let filter = SseFilter {
            entity_type: Some("owns".to_string()),
            ..Default::default()
        };
        assert!(!matches_filter(&envelope, &filter));
    }

    #[test]
    fn test_link_event_type_filter() {
        let created = make_link_envelope("follows", "created");
        let deleted = make_link_envelope("follows", "deleted");

        let filter = SseFilter {
            event_type: Some("created".to_string()),
            ..Default::default()
        };
        assert!(matches_filter(&created, &filter));
        assert!(!matches_filter(&deleted, &filter));
    }
}
