//! SSE (Server-Sent Events) exposure of the framework `EventBus`.
//!
//! Same events, same JSON as the WebSocket exposure: every `FrameworkEvent`
//! published on the bus becomes one SSE message whose `data` is the
//! [`ServerMessage::Event`] the WebSocket sends (`{"type":"event",
//! "subscription_id":…, "data":<EventEnvelope>}`), built from the same types.
//!
//! ```text
//! GET /sse[?entity_type=order&entity_id=…&event_type=created&kind=entity]
//! ```
//!
//! The query parameters are the WebSocket `SubscriptionFilter` fields. The same
//! isolation rules apply as on `/ws`: tenant, user (`data.user_id`) and, on an
//! armed host, the per-entity `get` / `get_link` policy (`SurfaceGuard`).
//! A lagging subscriber skips the missed events; the stream stays open.
//! Without an `EventBus` on the host, the endpoint answers `503` with a JSON body.

use super::guard::{SurfaceGuard, resolve_identity};
use super::websocket::ConnectionManager;
use super::websocket::protocol::{ServerMessage, SubscriptionFilter};
use crate::core::auth::AuthContext;
use crate::core::events::EventEnvelope;
use crate::server::host::ServerHost;
use anyhow::Result;
use axum::extract::{Extension, Query, RawQuery, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::{Router, routing::get};
use futures::stream::{Stream, StreamExt};
use std::convert::Infallible;
use std::sync::Arc;
use std::time::Duration;
use tokio_stream::wrappers::BroadcastStream;
use uuid::Uuid;

/// Keep-alive interval of the SSE stream.
const KEEP_ALIVE: Duration = Duration::from_secs(15);

#[derive(Clone)]
struct SseState {
    host: Arc<ServerHost>,
    enforce: bool,
}

/// SSE API exposure.
pub struct SseExposure;

impl SseExposure {
    /// Router with the `GET /sse` endpoint. Policies are applied when the host
    /// has them armed (same rule as `WebSocketExposure::build_router`).
    pub fn build_router(host: Arc<ServerHost>) -> Result<Router> {
        Ok(Self::build(host, true))
    }

    /// Like [`SseExposure::build_router`], without applying `auth:` policies.
    pub fn build_router_unenforced(host: Arc<ServerHost>) -> Result<Router> {
        crate::server::exposure::warn_unenforced_surface(
            "SSE",
            "SseExposure::build_router_unenforced",
        );
        Ok(Self::build(host, false))
    }

    fn build(host: Arc<ServerHost>, enforce: bool) -> Router {
        if host.event_bus().is_none() {
            tracing::warn!("SseExposure: no EventBus on the ServerHost; /sse will answer 503");
        }
        Router::new()
            .route("/sse", get(sse_handler))
            .with_state(SseState { host, enforce })
    }
}

/// Wire form of one event for one SSE stream: the WebSocket `event` message.
pub fn event_message(subscription_id: &str, envelope: &EventEnvelope) -> ServerMessage {
    ServerMessage::Event {
        subscription_id: subscription_id.to_string(),
        data: envelope.clone(),
    }
}

/// Whether `envelope` is delivered to this stream (filter, tenant, user, policy),
/// with the rules of `ConnectionManager::dispatch_event`.
fn deliver(
    envelope: &EventEnvelope,
    filter: &SubscriptionFilter,
    tenant: Option<Uuid>,
    user: Option<&str>,
    guard: Option<&SurfaceGuard>,
) -> bool {
    if let (Some(c), Some(e)) = (tenant, envelope.tenant_id())
        && c != e
    {
        return false;
    }
    if let (Some(e), Some(c)) = (
        ConnectionManager::extract_event_user_id(&envelope.event),
        user,
    ) && e != c
    {
        return false;
    }
    if let Some(g) = guard
        && !g.allows_event(&envelope.event)
    {
        return false;
    }
    filter.matches(&envelope.event)
}

/// SSE message of an envelope: `id` = envelope id, `event` = `event`, `data` = JSON.
fn to_sse(subscription_id: &str, envelope: &EventEnvelope) -> Option<Event> {
    let json = serde_json::to_string(&event_message(subscription_id, envelope)).ok()?;
    Some(
        Event::default()
            .event("event")
            .id(envelope.id.to_string())
            .data(json),
    )
}

async fn sse_handler(
    State(state): State<SseState>,
    auth: Option<Extension<AuthContext>>,
    headers: HeaderMap,
    RawQuery(raw): RawQuery,
    Query(filter): Query<SubscriptionFilter>,
) -> Response {
    let Some(bus) = state.host.event_bus() else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            axum::Json(serde_json::json!({
                "error": "no_event_bus",
                "message": "no EventBus configured on this host"
            })),
        )
            .into_response();
    };

    let guard = if state.enforce {
        let ctx = match auth.as_ref() {
            Some(Extension(ctx)) => ctx.clone(),
            None => match resolve_identity(&state.host, &headers, raw.as_deref()).await {
                Ok(ctx) => ctx,
                Err(e) => return (e.http_status_code(), axum::Json(e.to_json())).into_response(),
            },
        };
        Some(SurfaceGuard::for_request(&state.host, ctx, "SSE", &headers))
    } else {
        None
    };
    let ctx = match &guard {
        Some(g) => Some(g.context().clone()),
        None => auth.map(|Extension(c)| c),
    };
    let tenant = ctx.as_ref().and_then(|c| c.tenant_id());
    let user = ctx
        .as_ref()
        .and_then(|c| c.user_id())
        .map(|u| u.to_string());

    let sub_id = format!("sub_{}", Uuid::new_v4().simple());
    let stream = event_stream(bus.subscribe(), sub_id, filter, tenant, user, guard);
    Sse::new(stream)
        .keep_alive(KeepAlive::new().interval(KEEP_ALIVE))
        .into_response()
}

fn event_stream(
    rx: tokio::sync::broadcast::Receiver<EventEnvelope>,
    sub_id: String,
    filter: SubscriptionFilter,
    tenant: Option<Uuid>,
    user: Option<String>,
    guard: Option<SurfaceGuard>,
) -> impl Stream<Item = Result<Event, Infallible>> {
    BroadcastStream::new(rx).filter_map(move |item| {
        // `Err(Lagged)`: skip the missed events, keep the stream open.
        let out = match item {
            Ok(env) if deliver(&env, &filter, tenant, user.as_deref(), guard.as_ref()) => {
                to_sse(&sub_id, &env).map(Ok)
            }
            Ok(_) => None,
            Err(e) => {
                tracing::warn!("SSE subscriber lagged: {e}");
                None
            }
        };
        std::future::ready(out)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::LinksConfig;
    use crate::core::events::{EntityEvent, EventBus, FrameworkEvent};
    use crate::server::entity_registry::EntityRegistry;
    use crate::storage::InMemoryLinkService;
    use serde_json::json;
    use std::collections::HashMap;
    use tokio::io::{AsyncBufReadExt, BufReader};

    fn host(bus: Option<EventBus>) -> Arc<ServerHost> {
        let mut h = ServerHost::from_builder_components(
            Arc::new(InMemoryLinkService::new()),
            LinksConfig::default_config(),
            EntityRegistry::new(),
            HashMap::new(),
            HashMap::new(),
        )
        .unwrap();
        if let Some(b) = bus {
            h = h.with_event_bus(b);
        }
        Arc::new(h)
    }

    fn created(entity_type: &str) -> FrameworkEvent {
        FrameworkEvent::Entity(EntityEvent::Created {
            entity_type: entity_type.into(),
            entity_id: Uuid::new_v4(),
            data: json!({"amount": 100}),
        })
    }

    #[test]
    fn message_is_the_websocket_event_message() {
        let env = EventEnvelope::new(created("order"));
        let ws = serde_json::to_value(event_message("sub_x", &env)).unwrap();
        assert_eq!(ws["type"], "event");
        assert_eq!(ws["subscription_id"], "sub_x");
        assert_eq!(ws["data"]["event"]["data"]["amount"], 100);
        assert_eq!(ws["data"]["id"], env.id.to_string());
        assert!(to_sse("sub_x", &env).is_some());
    }

    #[tokio::test]
    async fn lagged_subscriber_keeps_streaming() {
        let bus = EventBus::new(2);
        let rx = bus.subscribe();
        for _ in 0..5 {
            bus.publish(created("order"));
        }
        let mut s = Box::pin(event_stream(
            rx,
            "s".into(),
            SubscriptionFilter::default(),
            None,
            None,
            None,
        ));
        // Lag is skipped, the remaining buffered events still arrive.
        assert!(s.next().await.is_some());
    }

    #[tokio::test]
    async fn no_event_bus_answers_503() {
        let app = SseExposure::build_router_unenforced(host(None)).unwrap();
        let server = axum_test::TestServer::new(app);
        let r = server.get("/sse").await;
        assert_eq!(r.status_code(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(r.json::<serde_json::Value>()["error"], "no_event_bus");
    }

    /// Real HTTP: bind a socket, open `/sse?entity_type=order`, publish, read frames.
    #[tokio::test]
    async fn http_stream_carries_published_event_filtered() {
        let bus = EventBus::new(16);
        let h = host(Some(bus.clone()));
        let app = SseExposure::build_router_unenforced(h).unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

        let mut sock = tokio::net::TcpStream::connect(addr).await.unwrap();
        tokio::io::AsyncWriteExt::write_all(
            &mut sock,
            b"GET /sse?entity_type=order HTTP/1.1\r\nHost: x\r\nAccept: text/event-stream\r\n\r\n",
        )
        .await
        .unwrap();
        let mut lines = BufReader::new(sock).lines();
        // Headers.
        let status = lines.next_line().await.unwrap().unwrap();
        assert!(status.contains("200"), "{status}");
        let mut ct = false;
        while let Some(l) = lines.next_line().await.unwrap() {
            if l.to_lowercase().starts_with("content-type") {
                ct = l.contains("text/event-stream");
            }
            if l.is_empty() {
                break;
            }
        }
        assert!(ct, "content-type must be text/event-stream");

        // Give the handler time to subscribe, then publish: filtered-out first.
        tokio::time::sleep(Duration::from_millis(200)).await;
        bus.publish(created("invoice"));
        bus.publish(created("order"));

        let data = tokio::time::timeout(Duration::from_secs(5), async {
            while let Some(l) = lines.next_line().await.unwrap() {
                if let Some(d) = l.strip_prefix("data:") {
                    return d.trim().to_string();
                }
            }
            panic!("stream ended");
        })
        .await
        .expect("no SSE data within 5s");
        let v: serde_json::Value = serde_json::from_str(&data).unwrap();
        assert_eq!(v["type"], "event");
        assert_eq!(v["data"]["event"]["entity_type"], "order");
        assert_eq!(v["data"]["event"]["data"]["amount"], 100);
    }
}
