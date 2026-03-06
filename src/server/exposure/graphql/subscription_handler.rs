//! GraphQL-over-WebSocket subscription handler
//!
//! Implements the `graphql-transport-ws` protocol (used by Apollo, urql, etc.)
//! for streaming GraphQL subscriptions over WebSocket connections.
//!
//! # Protocol Messages
//!
//! Client → Server:
//! - `connection_init` → Server responds with `connection_ack`
//! - `subscribe { id, payload: { query, variables } }` → Server streams `next` messages
//! - `complete { id }` → Client cancels a subscription
//! - `ping` → Server responds with `pong`
//!
//! Server → Client:
//! - `connection_ack` — Connection accepted
//! - `next { id, payload }` — Subscription data
//! - `error { id, payload }` — Subscription error
//! - `complete { id }` — Subscription ended
//! - `pong` — Pong response
//!
//! # Architecture
//!
//! ```text
//! Client ──ws──▶ /graphql/ws ──▶ graphql_ws_handler()
//!                                        │
//!                                  subscribe(query)
//!                                        │
//!                         EventBus ──broadcast──▶ filter ──▶ next { id, payload }
//! ```

use crate::core::events::{EntityEvent, EventBus, EventEnvelope, FrameworkEvent, LinkEvent};
use crate::events::sinks::in_app::{NotificationStore, StoredNotification};
use crate::server::host::ServerHost;
use axum::extract::ws::{Message, WebSocket};
use axum::extract::{Extension, WebSocketUpgrade};
use axum::response::IntoResponse;
use futures::SinkExt;
use futures::stream::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::Arc;
use tokio_stream::wrappers::BroadcastStream;

/// Axum handler for GraphQL WebSocket subscriptions
///
/// Upgrades the HTTP connection to WebSocket and starts the
/// `graphql-transport-ws` protocol handler.
pub async fn graphql_ws_handler(
    ws: WebSocketUpgrade,
    Extension(host): Extension<Arc<ServerHost>>,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_graphql_ws(socket, host))
}

/// Client-to-server protocol messages
#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ClientMsg {
    ConnectionInit {
        #[allow(dead_code)]
        payload: Option<Value>,
    },
    Subscribe {
        id: String,
        payload: SubscribePayload,
    },
    Complete {
        id: String,
    },
    Ping {
        #[allow(dead_code)]
        payload: Option<Value>,
    },
}

#[derive(Debug, Deserialize)]
struct SubscribePayload {
    query: String,
    #[allow(dead_code)]
    variables: Option<HashMap<String, Value>>,
}

/// Server-to-client protocol messages
#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ServerMsg {
    ConnectionAck,
    Next { id: String, payload: Value },
    Error { id: String, payload: Value },
    Complete { id: String },
    Pong,
}

/// Subscription filter parsed from the GraphQL query arguments
#[derive(Debug, Default)]
struct SubscriptionFilter {
    kind: Option<String>,
    entity_type: Option<String>,
    event_type: Option<String>,
    entity_id: Option<String>,
}

/// Handle a single GraphQL WebSocket connection
///
/// Uses a split read/write architecture with a shared outgoing channel:
/// 1. The write loop forwards all `ServerMsg` to the WebSocket
/// 2. The read loop processes client messages and spawns subscription tasks
/// 3. Each subscription task sends `ServerMsg` through the shared channel
async fn handle_graphql_ws(socket: WebSocket, host: Arc<ServerHost>) {
    let (mut ws_write, mut ws_read) = socket.split();

    // Shared channel for all outgoing messages (from any subscription)
    let (out_tx, mut out_rx) = tokio::sync::mpsc::unbounded_channel::<ServerMsg>();
    let mut active_subscriptions: HashMap<String, tokio::task::JoinHandle<()>> = HashMap::new();

    // Spawn the write loop: forwards ServerMsg → WebSocket
    let write_handle = tokio::spawn(async move {
        while let Some(msg) = out_rx.recv().await {
            match serde_json::to_string(&msg) {
                Ok(json) => {
                    if ws_write.send(Message::Text(json.into())).await.is_err() {
                        break;
                    }
                }
                Err(e) => {
                    tracing::error!(error = %e, "Failed to serialize ServerMsg");
                }
            }
        }
    });

    // Read loop: processes client messages
    while let Some(result) = ws_read.next().await {
        let text = match result {
            Ok(Message::Text(text)) => text,
            Ok(Message::Close(_)) => break,
            Ok(Message::Ping(_)) => continue, // axum handles pong automatically
            Ok(_) => continue,
            Err(_) => break,
        };

        let msg: ClientMsg = match serde_json::from_str(&text) {
            Ok(msg) => msg,
            Err(e) => {
                tracing::debug!(error = %e, "Invalid GraphQL-WS message");
                continue;
            }
        };

        match msg {
            ClientMsg::ConnectionInit { .. } => {
                let _ = out_tx.send(ServerMsg::ConnectionAck);
            }
            ClientMsg::Ping { .. } => {
                let _ = out_tx.send(ServerMsg::Pong);
            }
            ClientMsg::Subscribe { id, payload } => {
                let sub_type = detect_subscription_type(&payload.query);

                match sub_type {
                    SubscriptionType::OnEvent(filter) => {
                        let event_bus = match host.event_bus() {
                            Some(bus) => bus.clone(),
                            None => {
                                let _ = out_tx.send(ServerMsg::Error {
                                    id: id.clone(),
                                    payload: json!([{"message": "EventBus not configured"}]),
                                });
                                let _ = out_tx.send(ServerMsg::Complete { id });
                                continue;
                            }
                        };

                        let sub_tx = out_tx.clone();
                        let sub_id = id.clone();
                        let handle = tokio::spawn(async move {
                            run_subscription(event_bus, sub_id, filter, sub_tx).await;
                        });
                        active_subscriptions.insert(id, handle);
                    }
                    SubscriptionType::OnNotification(user_id) => {
                        let store = match host.notification_store() {
                            Some(s) => s.clone(),
                            None => {
                                let _ = out_tx.send(ServerMsg::Error {
                                    id: id.clone(),
                                    payload: json!([{"message": "NotificationStore not configured"}]),
                                });
                                let _ = out_tx.send(ServerMsg::Complete { id });
                                continue;
                            }
                        };

                        let sub_tx = out_tx.clone();
                        let sub_id = id.clone();
                        let handle = tokio::spawn(async move {
                            run_notification_subscription(store, sub_id, user_id, sub_tx).await;
                        });
                        active_subscriptions.insert(id, handle);
                    }
                    SubscriptionType::Unknown(field) => {
                        let _ = out_tx.send(ServerMsg::Error {
                            id: id.clone(),
                            payload: json!([{"message": format!("Unknown subscription field: {}", field)}]),
                        });
                        let _ = out_tx.send(ServerMsg::Complete { id });
                    }
                }
            }
            ClientMsg::Complete { id } => {
                if let Some(handle) = active_subscriptions.remove(&id) {
                    handle.abort();
                }
            }
        }
    }

    // Cleanup: abort all subscriptions and the write loop
    for (_, handle) in active_subscriptions {
        handle.abort();
    }
    write_handle.abort();
}

/// Run a single subscription, streaming filtered events from the EventBus
async fn run_subscription(
    event_bus: Arc<EventBus>,
    subscription_id: String,
    filter: SubscriptionFilter,
    tx: tokio::sync::mpsc::UnboundedSender<ServerMsg>,
) {
    let rx = event_bus.subscribe();
    let mut stream = BroadcastStream::new(rx);

    while let Some(result) = stream.next().await {
        match result {
            Ok(envelope) => {
                if matches_filter(&envelope, &filter) {
                    let payload = envelope_to_graphql_value(&envelope);
                    let msg = ServerMsg::Next {
                        id: subscription_id.clone(),
                        payload: json!({"data": {"onEvent": payload}}),
                    };
                    if tx.send(msg).is_err() {
                        break; // Receiver dropped
                    }
                }
            }
            Err(tokio_stream::wrappers::errors::BroadcastStreamRecvError::Lagged(n)) => {
                tracing::warn!(
                    subscription_id = %subscription_id,
                    missed = n,
                    "GraphQL subscription lagged"
                );
            }
        }
    }

    // Send complete when stream ends
    let _ = tx.send(ServerMsg::Complete {
        id: subscription_id,
    });
}

/// Check if an event envelope matches the subscription filter
fn matches_filter(envelope: &EventEnvelope, filter: &SubscriptionFilter) -> bool {
    // Filter by kind (entity / link)
    if let Some(ref kind) = filter.kind {
        if envelope.event.event_kind() != kind {
            return false;
        }
    }

    // Filter by entity_type
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
    if let Some(ref event_type) = filter.event_type {
        if envelope.event.action() != event_type {
            return false;
        }
    }

    // Filter by entity_id
    if let Some(ref entity_id) = filter.entity_id {
        if let Some(id) = envelope.event.entity_id() {
            if id.to_string() != *entity_id {
                return false;
            }
        } else {
            return false;
        }
    }

    true
}

/// Convert an EventEnvelope to a GraphQL-friendly JSON value
fn envelope_to_graphql_value(envelope: &EventEnvelope) -> Value {
    match &envelope.event {
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
                "id": envelope.id.to_string(),
                "timestamp": envelope.timestamp.to_rfc3339(),
                "kind": "entity",
                "action": envelope.event.action(),
                "entityType": entity_type,
                "entityId": entity_id.to_string(),
                "data": data,
            }),
            EntityEvent::Deleted {
                entity_type,
                entity_id,
            } => json!({
                "id": envelope.id.to_string(),
                "timestamp": envelope.timestamp.to_rfc3339(),
                "kind": "entity",
                "action": "deleted",
                "entityType": entity_type,
                "entityId": entity_id.to_string(),
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
                "id": envelope.id.to_string(),
                "timestamp": envelope.timestamp.to_rfc3339(),
                "kind": "link",
                "action": "created",
                "linkType": link_type,
                "linkId": link_id.to_string(),
                "sourceId": source_id.to_string(),
                "targetId": target_id.to_string(),
                "metadata": metadata,
            }),
            LinkEvent::Deleted {
                link_type,
                link_id,
                source_id,
                target_id,
            } => json!({
                "id": envelope.id.to_string(),
                "timestamp": envelope.timestamp.to_rfc3339(),
                "kind": "link",
                "action": "deleted",
                "linkType": link_type,
                "linkId": link_id.to_string(),
                "sourceId": source_id.to_string(),
                "targetId": target_id.to_string(),
            }),
        },
    }
}

/// Detected subscription type from the query
enum SubscriptionType {
    /// `subscription { onEvent(...) { ... } }`
    OnEvent(SubscriptionFilter),
    /// `subscription { onNotification(userId: "...") { ... } }`
    OnNotification(Option<String>),
    /// Unknown subscription field
    Unknown(String),
}

/// Detect the subscription type from the GraphQL query
fn detect_subscription_type(query: &str) -> SubscriptionType {
    use graphql_parser::query::parse_query;

    let doc = match parse_query::<String>(query) {
        Ok(doc) => doc,
        Err(_) => return SubscriptionType::Unknown("(parse error)".to_string()),
    };

    for def in &doc.definitions {
        if let graphql_parser::query::Definition::Operation(
            graphql_parser::query::OperationDefinition::Subscription(sub),
        ) = def
        {
            for sel in &sub.selection_set.items {
                if let graphql_parser::query::Selection::Field(field) = sel {
                    match field.name.as_str() {
                        "onEvent" => {
                            return SubscriptionType::OnEvent(parse_subscription_filter(query));
                        }
                        "onNotification" => {
                            let user_id = field
                                .arguments
                                .iter()
                                .find(|(name, _)| name == "userId")
                                .and_then(|(_, value)| {
                                    if let graphql_parser::query::Value::String(s) = value {
                                        Some(s.clone())
                                    } else {
                                        None
                                    }
                                });
                            return SubscriptionType::OnNotification(user_id);
                        }
                        other => return SubscriptionType::Unknown(other.to_string()),
                    }
                }
            }
        }
    }

    SubscriptionType::Unknown("(no subscription field)".to_string())
}

/// Run a notification subscription, streaming from the NotificationStore's broadcast channel
async fn run_notification_subscription(
    store: Arc<NotificationStore>,
    subscription_id: String,
    user_id_filter: Option<String>,
    tx: tokio::sync::mpsc::UnboundedSender<ServerMsg>,
) {
    let rx = store.subscribe();
    let mut stream = BroadcastStream::new(rx);

    while let Some(result) = stream.next().await {
        match result {
            Ok(notification) => {
                // Filter by userId if specified
                if let Some(ref uid) = user_id_filter {
                    if notification.recipient_id != *uid {
                        continue;
                    }
                }

                let payload = notification_to_graphql_value(&notification);
                let msg = ServerMsg::Next {
                    id: subscription_id.clone(),
                    payload: json!({"data": {"onNotification": payload}}),
                };
                if tx.send(msg).is_err() {
                    break;
                }
            }
            Err(tokio_stream::wrappers::errors::BroadcastStreamRecvError::Lagged(n)) => {
                tracing::warn!(
                    subscription_id = %subscription_id,
                    missed = n,
                    "Notification subscription lagged"
                );
            }
        }
    }

    let _ = tx.send(ServerMsg::Complete {
        id: subscription_id,
    });
}

/// Convert a StoredNotification to a GraphQL-friendly JSON value
fn notification_to_graphql_value(notification: &StoredNotification) -> Value {
    json!({
        "id": notification.id.to_string(),
        "recipientId": notification.recipient_id,
        "notificationType": notification.notification_type,
        "title": notification.title,
        "body": notification.body,
        "data": notification.data,
        "read": notification.read,
        "createdAt": notification.created_at.to_rfc3339(),
    })
}

/// Parse subscription filter arguments from a GraphQL subscription query
///
/// Extracts filter arguments from queries like:
/// `subscription { onEvent(kind: "entity", entityType: "order") { ... } }`
fn parse_subscription_filter(query: &str) -> SubscriptionFilter {
    use graphql_parser::query::parse_query;

    let mut filter = SubscriptionFilter::default();

    let doc = match parse_query::<String>(query) {
        Ok(doc) => doc,
        Err(_) => return filter,
    };

    // Find the subscription operation and its onEvent field arguments
    for def in &doc.definitions {
        if let graphql_parser::query::Definition::Operation(
            graphql_parser::query::OperationDefinition::Subscription(sub),
        ) = def
        {
            for sel in &sub.selection_set.items {
                if let graphql_parser::query::Selection::Field(field) = sel {
                    if field.name == "onEvent" {
                        for (arg_name, arg_value) in &field.arguments {
                            let value = match arg_value {
                                graphql_parser::query::Value::String(s) => s.clone(),
                                _ => continue,
                            };
                            match arg_name.as_str() {
                                "kind" => filter.kind = Some(value),
                                "entityType" => filter.entity_type = Some(value),
                                "eventType" => filter.event_type = Some(value),
                                "entityId" => filter.entity_id = Some(value),
                                _ => {}
                            }
                        }
                    }
                }
            }
        }
    }

    filter
}

#[cfg(test)]
#[cfg(feature = "graphql")]
mod tests {
    use super::*;
    use serde_json::json;
    use uuid::Uuid;

    // -----------------------------------------------------------------------
    // matches_filter tests
    // -----------------------------------------------------------------------

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
        let envelope = make_entity_envelope("order", "created");
        let filter = SubscriptionFilter::default();
        assert!(matches_filter(&envelope, &filter));
    }

    #[test]
    fn test_matches_filter_kind_entity() {
        let envelope = make_entity_envelope("order", "created");
        assert!(matches_filter(
            &envelope,
            &SubscriptionFilter {
                kind: Some("entity".to_string()),
                ..Default::default()
            }
        ));
        assert!(!matches_filter(
            &envelope,
            &SubscriptionFilter {
                kind: Some("link".to_string()),
                ..Default::default()
            }
        ));
    }

    #[test]
    fn test_matches_filter_kind_link() {
        let envelope = make_link_envelope("has_invoice", "created");
        assert!(matches_filter(
            &envelope,
            &SubscriptionFilter {
                kind: Some("link".to_string()),
                ..Default::default()
            }
        ));
        assert!(!matches_filter(
            &envelope,
            &SubscriptionFilter {
                kind: Some("entity".to_string()),
                ..Default::default()
            }
        ));
    }

    #[test]
    fn test_matches_filter_entity_type() {
        let envelope = make_entity_envelope("order", "created");
        assert!(matches_filter(
            &envelope,
            &SubscriptionFilter {
                entity_type: Some("order".to_string()),
                ..Default::default()
            }
        ));
        assert!(!matches_filter(
            &envelope,
            &SubscriptionFilter {
                entity_type: Some("invoice".to_string()),
                ..Default::default()
            }
        ));
    }

    #[test]
    fn test_matches_filter_event_type() {
        let envelope = make_entity_envelope("order", "created");
        assert!(matches_filter(
            &envelope,
            &SubscriptionFilter {
                event_type: Some("created".to_string()),
                ..Default::default()
            }
        ));
        assert!(!matches_filter(
            &envelope,
            &SubscriptionFilter {
                event_type: Some("deleted".to_string()),
                ..Default::default()
            }
        ));
    }

    #[test]
    fn test_matches_filter_entity_id() {
        let entity_id = Uuid::new_v4();
        let event = FrameworkEvent::Entity(EntityEvent::Created {
            entity_type: "order".to_string(),
            entity_id,
            data: json!({}),
        });
        let envelope = EventEnvelope::new(event);

        assert!(matches_filter(
            &envelope,
            &SubscriptionFilter {
                entity_id: Some(entity_id.to_string()),
                ..Default::default()
            }
        ));
        assert!(!matches_filter(
            &envelope,
            &SubscriptionFilter {
                entity_id: Some(Uuid::new_v4().to_string()),
                ..Default::default()
            }
        ));
    }

    #[test]
    fn test_matches_filter_combined() {
        let envelope = make_entity_envelope("order", "created");
        assert!(matches_filter(
            &envelope,
            &SubscriptionFilter {
                kind: Some("entity".to_string()),
                entity_type: Some("order".to_string()),
                event_type: Some("created".to_string()),
                ..Default::default()
            }
        ));

        // Wrong event_type
        assert!(!matches_filter(
            &envelope,
            &SubscriptionFilter {
                kind: Some("entity".to_string()),
                entity_type: Some("order".to_string()),
                event_type: Some("deleted".to_string()),
                ..Default::default()
            }
        ));
    }

    #[test]
    fn test_matches_filter_link_by_link_type() {
        let envelope = make_link_envelope("has_invoice", "created");
        assert!(matches_filter(
            &envelope,
            &SubscriptionFilter {
                entity_type: Some("has_invoice".to_string()),
                ..Default::default()
            }
        ));
        assert!(!matches_filter(
            &envelope,
            &SubscriptionFilter {
                entity_type: Some("other_type".to_string()),
                ..Default::default()
            }
        ));
    }

    // -----------------------------------------------------------------------
    // parse_subscription_filter tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_parse_subscription_filter_all_args() {
        let query = r#"subscription { onEvent(kind: "entity", entityType: "order", eventType: "created", entityId: "abc-123") { id kind } }"#;
        let filter = parse_subscription_filter(query);
        assert_eq!(filter.kind.as_deref(), Some("entity"));
        assert_eq!(filter.entity_type.as_deref(), Some("order"));
        assert_eq!(filter.event_type.as_deref(), Some("created"));
        assert_eq!(filter.entity_id.as_deref(), Some("abc-123"));
    }

    #[test]
    fn test_parse_subscription_filter_partial_args() {
        let query =
            r#"subscription { onEvent(entityType: "order") { id kind action entityType } }"#;
        let filter = parse_subscription_filter(query);
        assert_eq!(filter.kind, None);
        assert_eq!(filter.entity_type.as_deref(), Some("order"));
        assert_eq!(filter.event_type, None);
        assert_eq!(filter.entity_id, None);
    }

    #[test]
    fn test_parse_subscription_filter_no_args() {
        let query = r#"subscription { onEvent { id kind action } }"#;
        let filter = parse_subscription_filter(query);
        assert_eq!(filter.kind, None);
        assert_eq!(filter.entity_type, None);
        assert_eq!(filter.event_type, None);
        assert_eq!(filter.entity_id, None);
    }

    #[test]
    fn test_parse_subscription_filter_invalid_query() {
        let filter = parse_subscription_filter("not valid graphql {{{{");
        assert_eq!(filter.kind, None);
        assert_eq!(filter.entity_type, None);
    }

    // -----------------------------------------------------------------------
    // envelope_to_graphql_value tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_envelope_to_graphql_value_entity_created() {
        let entity_id = Uuid::new_v4();
        let event = FrameworkEvent::Entity(EntityEvent::Created {
            entity_type: "order".to_string(),
            entity_id,
            data: json!({"name": "Test"}),
        });
        let envelope = EventEnvelope::new(event);
        let value = envelope_to_graphql_value(&envelope);

        assert_eq!(value["kind"], "entity");
        assert_eq!(value["action"], "created");
        assert_eq!(value["entityType"], "order");
        assert_eq!(value["entityId"], entity_id.to_string());
        assert_eq!(value["data"]["name"], "Test");
        assert!(value["timestamp"].is_string());
    }

    #[test]
    fn test_envelope_to_graphql_value_entity_deleted() {
        let entity_id = Uuid::new_v4();
        let event = FrameworkEvent::Entity(EntityEvent::Deleted {
            entity_type: "order".to_string(),
            entity_id,
        });
        let envelope = EventEnvelope::new(event);
        let value = envelope_to_graphql_value(&envelope);

        assert_eq!(value["kind"], "entity");
        assert_eq!(value["action"], "deleted");
        assert_eq!(value["entityType"], "order");
        assert!(value.get("data").is_none());
    }

    #[test]
    fn test_envelope_to_graphql_value_link_created() {
        let link_id = Uuid::new_v4();
        let source_id = Uuid::new_v4();
        let target_id = Uuid::new_v4();
        let event = FrameworkEvent::Link(LinkEvent::Created {
            link_type: "has_invoice".to_string(),
            link_id,
            source_id,
            target_id,
            metadata: Some(json!({"priority": "high"})),
        });
        let envelope = EventEnvelope::new(event);
        let value = envelope_to_graphql_value(&envelope);

        assert_eq!(value["kind"], "link");
        assert_eq!(value["action"], "created");
        assert_eq!(value["linkType"], "has_invoice");
        assert_eq!(value["linkId"], link_id.to_string());
        assert_eq!(value["sourceId"], source_id.to_string());
        assert_eq!(value["targetId"], target_id.to_string());
        assert_eq!(value["metadata"]["priority"], "high");
    }

    #[test]
    fn test_envelope_to_graphql_value_link_deleted() {
        let event = FrameworkEvent::Link(LinkEvent::Deleted {
            link_type: "has_invoice".to_string(),
            link_id: Uuid::new_v4(),
            source_id: Uuid::new_v4(),
            target_id: Uuid::new_v4(),
        });
        let envelope = EventEnvelope::new(event);
        let value = envelope_to_graphql_value(&envelope);

        assert_eq!(value["kind"], "link");
        assert_eq!(value["action"], "deleted");
        assert!(value.get("metadata").is_none());
    }

    // -----------------------------------------------------------------------
    // Protocol message serialization tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_server_msg_connection_ack_serialization() {
        let msg = ServerMsg::ConnectionAck;
        let json = serde_json::to_string(&msg).unwrap();
        assert_eq!(json, r#"{"type":"connection_ack"}"#);
    }

    #[test]
    fn test_server_msg_next_serialization() {
        let msg = ServerMsg::Next {
            id: "sub-1".to_string(),
            payload: json!({"data": {"onEvent": {"kind": "entity"}}}),
        };
        let json: Value = serde_json::to_value(&msg).unwrap();
        assert_eq!(json["type"], "next");
        assert_eq!(json["id"], "sub-1");
        assert!(json["payload"]["data"]["onEvent"]["kind"].is_string());
    }

    #[test]
    fn test_server_msg_error_serialization() {
        let msg = ServerMsg::Error {
            id: "sub-1".to_string(),
            payload: json!([{"message": "something went wrong"}]),
        };
        let json: Value = serde_json::to_value(&msg).unwrap();
        assert_eq!(json["type"], "error");
        assert_eq!(json["id"], "sub-1");
    }

    #[test]
    fn test_server_msg_complete_serialization() {
        let msg = ServerMsg::Complete {
            id: "sub-1".to_string(),
        };
        let json = serde_json::to_string(&msg).unwrap();
        let parsed: Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["type"], "complete");
        assert_eq!(parsed["id"], "sub-1");
    }

    #[test]
    fn test_server_msg_pong_serialization() {
        let msg = ServerMsg::Pong;
        let json = serde_json::to_string(&msg).unwrap();
        assert_eq!(json, r#"{"type":"pong"}"#);
    }

    #[test]
    fn test_client_msg_connection_init_deserialization() {
        let json = r#"{"type":"connection_init"}"#;
        let msg: ClientMsg = serde_json::from_str(json).unwrap();
        assert!(matches!(msg, ClientMsg::ConnectionInit { .. }));
    }

    #[test]
    fn test_client_msg_subscribe_deserialization() {
        let json = r#"{"type":"subscribe","id":"1","payload":{"query":"subscription { onEvent { id } }"}}"#;
        let msg: ClientMsg = serde_json::from_str(json).unwrap();
        match msg {
            ClientMsg::Subscribe { id, payload } => {
                assert_eq!(id, "1");
                assert!(payload.query.contains("onEvent"));
            }
            other => panic!("expected Subscribe, got {:?}", other),
        }
    }

    #[test]
    fn test_client_msg_complete_deserialization() {
        let json = r#"{"type":"complete","id":"1"}"#;
        let msg: ClientMsg = serde_json::from_str(json).unwrap();
        match msg {
            ClientMsg::Complete { id } => assert_eq!(id, "1"),
            other => panic!("expected Complete, got {:?}", other),
        }
    }

    #[test]
    fn test_client_msg_ping_deserialization() {
        let json = r#"{"type":"ping"}"#;
        let msg: ClientMsg = serde_json::from_str(json).unwrap();
        assert!(matches!(msg, ClientMsg::Ping { .. }));
    }

    // -----------------------------------------------------------------------
    // run_subscription integration test
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_run_subscription_streams_matching_events() {
        let event_bus = Arc::new(EventBus::new(16));
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();

        let bus_clone = event_bus.clone();
        let handle = tokio::spawn(async move {
            run_subscription(
                bus_clone,
                "sub-1".to_string(),
                SubscriptionFilter {
                    entity_type: Some("order".to_string()),
                    ..Default::default()
                },
                tx,
            )
            .await;
        });

        // Give the subscription time to start
        tokio::task::yield_now().await;

        // Publish a matching event
        event_bus.publish(FrameworkEvent::Entity(EntityEvent::Created {
            entity_type: "order".to_string(),
            entity_id: Uuid::new_v4(),
            data: json!({"name": "Test Order"}),
        }));

        // Publish a non-matching event
        event_bus.publish(FrameworkEvent::Entity(EntityEvent::Created {
            entity_type: "invoice".to_string(),
            entity_id: Uuid::new_v4(),
            data: json!({"amount": 100}),
        }));

        // Should receive only the matching event
        let msg = tokio::time::timeout(std::time::Duration::from_millis(100), rx.recv())
            .await
            .expect("should receive within timeout")
            .expect("should have message");

        match msg {
            ServerMsg::Next { id, payload } => {
                assert_eq!(id, "sub-1");
                let on_event = &payload["data"]["onEvent"];
                assert_eq!(on_event["kind"], "entity");
                assert_eq!(on_event["action"], "created");
                assert_eq!(on_event["entityType"], "order");
            }
            other => panic!("expected Next, got {:?}", other),
        }

        // The invoice event should not have generated a message
        // (brief wait to confirm no extra messages)
        let no_msg = tokio::time::timeout(std::time::Duration::from_millis(50), rx.recv()).await;
        assert!(no_msg.is_err(), "should not receive non-matching event");

        handle.abort();
    }

    #[tokio::test]
    async fn test_run_subscription_no_filter_streams_all() {
        let event_bus = Arc::new(EventBus::new(16));
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();

        let bus_clone = event_bus.clone();
        let handle = tokio::spawn(async move {
            run_subscription(
                bus_clone,
                "sub-all".to_string(),
                SubscriptionFilter::default(),
                tx,
            )
            .await;
        });

        tokio::task::yield_now().await;

        event_bus.publish(FrameworkEvent::Entity(EntityEvent::Created {
            entity_type: "order".to_string(),
            entity_id: Uuid::new_v4(),
            data: json!({}),
        }));
        event_bus.publish(FrameworkEvent::Link(LinkEvent::Created {
            link_type: "has_invoice".to_string(),
            link_id: Uuid::new_v4(),
            source_id: Uuid::new_v4(),
            target_id: Uuid::new_v4(),
            metadata: None,
        }));

        // Should receive both events
        let msg1 = tokio::time::timeout(std::time::Duration::from_millis(100), rx.recv())
            .await
            .expect("timeout")
            .expect("msg");
        assert!(matches!(msg1, ServerMsg::Next { .. }));

        let msg2 = tokio::time::timeout(std::time::Duration::from_millis(100), rx.recv())
            .await
            .expect("timeout")
            .expect("msg");
        assert!(matches!(msg2, ServerMsg::Next { .. }));

        handle.abort();
    }

    // -----------------------------------------------------------------------
    // detect_subscription_type tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_detect_subscription_type_on_event() {
        let query = r#"subscription { onEvent(kind: "entity") { id kind } }"#;
        let sub_type = detect_subscription_type(query);
        assert!(
            matches!(sub_type, SubscriptionType::OnEvent(_)),
            "should detect onEvent"
        );
    }

    #[test]
    fn test_detect_subscription_type_on_notification() {
        let query = r#"subscription { onNotification(userId: "user-A") { id title } }"#;
        let sub_type = detect_subscription_type(query);
        match sub_type {
            SubscriptionType::OnNotification(user_id) => {
                assert_eq!(user_id.as_deref(), Some("user-A"));
            }
            other => panic!(
                "expected OnNotification, got {:?}",
                std::mem::discriminant(&other)
            ),
        }
    }

    #[test]
    fn test_detect_subscription_type_on_notification_no_user_id() {
        let query = r#"subscription { onNotification { id title } }"#;
        let sub_type = detect_subscription_type(query);
        match sub_type {
            SubscriptionType::OnNotification(user_id) => {
                assert_eq!(user_id, None);
            }
            other => panic!(
                "expected OnNotification, got {:?}",
                std::mem::discriminant(&other)
            ),
        }
    }

    #[test]
    fn test_detect_subscription_type_unknown() {
        let query = r#"subscription { unknownField { id } }"#;
        let sub_type = detect_subscription_type(query);
        assert!(
            matches!(sub_type, SubscriptionType::Unknown(_)),
            "should detect unknown"
        );
    }

    #[test]
    fn test_detect_subscription_type_parse_error() {
        let sub_type = detect_subscription_type("not valid {{{{");
        assert!(matches!(sub_type, SubscriptionType::Unknown(_)));
    }

    // -----------------------------------------------------------------------
    // notification_to_graphql_value tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_notification_to_graphql_value() {
        let notif_id = Uuid::new_v4();
        let notification = StoredNotification {
            id: notif_id,
            recipient_id: "user-A".to_string(),
            notification_type: "new_follower".to_string(),
            title: "New follower".to_string(),
            body: "Alice followed you".to_string(),
            data: json!({"follower": "Alice"}),
            read: false,
            created_at: chrono::Utc::now(),
        };

        let value = notification_to_graphql_value(&notification);
        assert_eq!(value["id"], notif_id.to_string());
        assert_eq!(value["recipientId"], "user-A");
        assert_eq!(value["notificationType"], "new_follower");
        assert_eq!(value["title"], "New follower");
        assert_eq!(value["body"], "Alice followed you");
        assert_eq!(value["data"]["follower"], "Alice");
        assert_eq!(value["read"], false);
        assert!(value["createdAt"].is_string());
    }

    // -----------------------------------------------------------------------
    // run_notification_subscription integration test
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_run_notification_subscription_filters_by_user() {
        let store = Arc::new(NotificationStore::new());
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();

        let store_clone = store.clone();
        let handle = tokio::spawn(async move {
            run_notification_subscription(
                store_clone,
                "notif-sub".to_string(),
                Some("user-A".to_string()),
                tx,
            )
            .await;
        });

        tokio::task::yield_now().await;

        // Insert notification for user-A (should match)
        store
            .insert(StoredNotification {
                id: Uuid::new_v4(),
                recipient_id: "user-A".to_string(),
                notification_type: "new_follower".to_string(),
                title: "New follower".to_string(),
                body: "Alice followed you".to_string(),
                data: json!({}),
                read: false,
                created_at: chrono::Utc::now(),
            })
            .await;

        // Insert notification for user-B (should NOT match)
        store
            .insert(StoredNotification {
                id: Uuid::new_v4(),
                recipient_id: "user-B".to_string(),
                notification_type: "test".to_string(),
                title: "For B".to_string(),
                body: String::new(),
                data: json!({}),
                read: false,
                created_at: chrono::Utc::now(),
            })
            .await;

        // Should receive only user-A's notification
        let msg = tokio::time::timeout(std::time::Duration::from_millis(100), rx.recv())
            .await
            .expect("timeout")
            .expect("msg");

        match msg {
            ServerMsg::Next { id, payload } => {
                assert_eq!(id, "notif-sub");
                let on_notif = &payload["data"]["onNotification"];
                assert_eq!(on_notif["recipientId"], "user-A");
                assert_eq!(on_notif["title"], "New follower");
            }
            other => panic!("expected Next, got {:?}", other),
        }

        // Should NOT receive user-B's notification
        let no_msg = tokio::time::timeout(std::time::Duration::from_millis(50), rx.recv()).await;
        assert!(no_msg.is_err(), "should not receive user-B notification");

        handle.abort();
    }

    #[tokio::test]
    async fn test_run_notification_subscription_no_filter_streams_all() {
        let store = Arc::new(NotificationStore::new());
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();

        let store_clone = store.clone();
        let handle = tokio::spawn(async move {
            run_notification_subscription(store_clone, "all-notif".to_string(), None, tx).await;
        });

        tokio::task::yield_now().await;

        store
            .insert(StoredNotification {
                id: Uuid::new_v4(),
                recipient_id: "user-A".to_string(),
                notification_type: "test".to_string(),
                title: "For A".to_string(),
                body: String::new(),
                data: json!({}),
                read: false,
                created_at: chrono::Utc::now(),
            })
            .await;

        store
            .insert(StoredNotification {
                id: Uuid::new_v4(),
                recipient_id: "user-B".to_string(),
                notification_type: "test".to_string(),
                title: "For B".to_string(),
                body: String::new(),
                data: json!({}),
                read: false,
                created_at: chrono::Utc::now(),
            })
            .await;

        // Should receive both notifications
        let msg1 = tokio::time::timeout(std::time::Duration::from_millis(100), rx.recv())
            .await
            .expect("timeout")
            .expect("msg");
        assert!(matches!(msg1, ServerMsg::Next { .. }));

        let msg2 = tokio::time::timeout(std::time::Duration::from_millis(100), rx.recv())
            .await
            .expect("timeout")
            .expect("msg");
        assert!(matches!(msg2, ServerMsg::Next { .. }));

        handle.abort();
    }
}
