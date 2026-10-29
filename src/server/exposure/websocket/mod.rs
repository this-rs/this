//! WebSocket API exposure for the framework
//!
//! This module provides WebSocket-specific routing and real-time event handling.
//! It is completely separate from the core framework logic and follows the same
//! pattern as RestExposure and GraphQLExposure.
//!
//! # Architecture
//!
//! ```text
//! Client ──ws──▶ /ws ──▶ ws_handler() ──▶ ConnectionManager
//!                                              │
//!                                     subscribe(filter)
//!                                              │
//!                           EventBus ──broadcast──▶ filter ──▶ Client
//! ```
//!
//! # Sink Integration
//!
//! When a `SinkRegistry` is configured on the host, `build_router()` automatically
//! wires a `WebSocketSink` for every sink of type `WebSocket` declared in the YAML
//! config. The sink dispatches notifications through the `ConnectionManagerDispatcher`.
//!
//! ```text
//! FlowRuntime → DeliverOp → SinkRegistry → WebSocketSink
//!                                              └─ ConnectionManagerDispatcher
//!                                                   └─ ConnectionManager
//!                                                        └─ send_to_user() / broadcast_payload()
//! ```
//!
//! # Protocol
//!
//! Client → Server (JSON):
//! - `{"type": "subscribe", "filter": {"entity_type": "order"}}`
//! - `{"type": "unsubscribe", "subscription_id": "..."}`
//! - `{"type": "ping"}`
//!
//! Server → Client (JSON):
//! - `{"type": "event", "data": {...}}`
//! - `{"type": "subscribed", "subscription_id": "..."}`
//! - `{"type": "unsubscribed", "subscription_id": "..."}`
//! - `{"type": "notification", "data": {...}}`
//! - `{"type": "pong"}`
//! - `{"type": "error", "message": "..."}`

mod dispatcher;
mod handler;
mod manager;
pub mod protocol;

// Re-export ConnectionManager and CustomMessageHandler for host applications
// that need to register custom message handlers or access connection state.
pub use manager::{ConnectionManager, CustomMessageHandler};

use crate::config::sinks::SinkType;
use crate::events::sinks::Sink;
use crate::events::sinks::websocket::WebSocketSink;
use crate::server::host::ServerHost;
use anyhow::Result;
use axum::{Router, routing::get};
use std::sync::Arc;

/// WebSocket API exposure implementation
///
/// This struct encapsulates all WebSocket-specific logic for exposing real-time
/// events from the framework. It consumes a `ServerHost` and produces an Axum
/// router with a `/ws` endpoint.
///
/// # Requirements
///
/// The `ServerHost` must have an `EventBus` configured (via `ServerBuilder::with_event_bus()`)
/// for the WebSocket exposure to function. Without an EventBus, the WebSocket endpoint
/// will still accept connections but no events will be broadcast.
///
/// # Sink Auto-wiring
///
/// If the host has a `SinkRegistry` and YAML config declares sinks of type `websocket`,
/// `build_router()` will automatically register a `WebSocketSink` backed by the
/// `ConnectionManager` for each such sink. This enables the `deliver` operator in
/// event pipelines to dispatch payloads to connected WebSocket clients.
///
/// # Example
///
/// ```rust,ignore
/// use this::server::{ServerBuilder, WebSocketExposure, RestExposure};
/// use this::storage::InMemoryLinkService;
/// use std::sync::Arc;
///
/// let host = Arc::new(
///     ServerBuilder::new()
///         .with_link_service(InMemoryLinkService::new())
///         .with_event_bus(1024)
///         .register_module(my_module)?
///         .build_host()?
/// );
///
/// let rest_router = RestExposure::build_router(host.clone(), vec![])?;
/// let ws_router = WebSocketExposure::build_router(host)?;
///
/// let app = rest_router.merge(ws_router);
/// ```
pub struct WebSocketExposure;

impl WebSocketExposure {
    /// Build the WebSocket router from a host
    ///
    /// Creates a `ConnectionManager` that subscribes to the host's `EventBus`,
    /// spawns the event dispatch loop, and returns a router with the `/ws` endpoint.
    ///
    /// If the host has a `SinkRegistry`, this also registers a `WebSocketSink` for
    /// every YAML-declared sink of type `websocket`, enabling the event pipeline
    /// to dispatch notifications to connected clients.
    pub fn build_router(host: Arc<ServerHost>) -> Result<Router> {
        let (router, _cm) = Self::build_router_with_manager(host)?;
        Ok(router)
    }

    /// Comme [`WebSocketExposure::build_router`], en **acceptant explicitement**
    /// que les politiques `auth:` ne s'appliquent pas au WebSocket.
    pub fn build_router_unenforced(host: Arc<ServerHost>) -> Result<Router> {
        crate::server::exposure::warn_unenforced_surface(
            "WebSocket",
            "WebSocketExposure::build_router_unenforced",
        );
        let (router, _cm) = Self::build_router_with_manager_unenforced(host)?;
        Ok(router)
    }

    /// Comme [`WebSocketExposure::build_router`], en rendant aussi le
    /// `ConnectionManager`.
    ///
    /// # Policies (armed host)
    ///
    /// The caller is resolved at the handshake (header, `this_token` cookie,
    /// `access_token` query; 401 on a bad token with a non-public default policy)
    /// and every event is delivered only if it may `get` its entity / `get_link`
    /// its link (`SurfaceGuard`, same resolution as REST). Still **refused** when
    /// `websocket` sinks are wired: their `broadcast` reaches every connection
    /// without any policy (fail-closed).
    pub fn build_router_with_manager(
        host: Arc<ServerHost>,
    ) -> Result<(Router, Arc<ConnectionManager>)> {
        crate::server::exposure::refuse_uncovered_surface(
            &host,
            "WebSocket",
            &uncovered_parts(&host),
            "WebSocketExposure::build_router_with_manager_unenforced",
        )?;
        Ok(Self::build(host, true))
    }

    /// Build the WebSocket router and return both the router and the `ConnectionManager`.
    ///
    /// Use this variant when the host application needs access to the `ConnectionManager`
    /// to register a custom message handler (e.g., for multiplexing chat or other
    /// protocols over the same `/ws` connection).
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// let (ws_router, ws_manager) = WebSocketExposure::build_router_with_manager(host)?;
    /// ws_manager.set_custom_handler(Arc::new(MyChatHandler { ... })).await;
    /// let app = rest_router.merge(ws_router);
    /// ```
    pub fn build_router_with_manager_unenforced(
        host: Arc<ServerHost>,
    ) -> Result<(Router, Arc<manager::ConnectionManager>)> {
        Ok(Self::build(host, false))
    }

    fn build(host: Arc<ServerHost>, enforce: bool) -> (Router, Arc<manager::ConnectionManager>) {
        let mut cm = manager::ConnectionManager::new(host.clone());
        if enforce {
            cm = cm.enforced();
        }
        let connection_manager = Arc::new(cm);

        // Spawn the event dispatch loop if there's an event bus
        if let Some(event_bus) = host.event_bus() {
            let cm = connection_manager.clone();
            let rx = event_bus.subscribe();
            tokio::spawn(async move {
                cm.run_dispatch_loop(rx).await;
            });
        } else {
            tracing::warn!(
                "WebSocketExposure: No EventBus configured on ServerHost. \
                 WebSocket connections will work but no events will be broadcast. \
                 Use ServerBuilder::with_event_bus() to enable real-time events."
            );
        }

        // Auto-wire WebSocket sinks into the SinkRegistry
        Self::wire_websocket_sinks(&host, &connection_manager);

        let router = Router::new()
            .route("/ws", get(handler::ws_handler))
            .with_state(connection_manager.clone());

        (router, connection_manager)
    }

    /// Wire WebSocket sinks into the SinkRegistry
    ///
    /// For each sink declared as `type: websocket` in the YAML config, register
    /// a `WebSocketSink` backed by the `ConnectionManager` in the `SinkRegistry`.
    fn wire_websocket_sinks(
        host: &ServerHost,
        connection_manager: &Arc<manager::ConnectionManager>,
    ) {
        let sink_registry = match &host.sink_registry {
            Some(registry) => registry,
            None => return,
        };

        let sink_configs = match &host.config.sinks {
            Some(configs) => configs,
            None => return,
        };

        // Create the dispatcher (shared by all WebSocket sinks)
        let ws_dispatcher = Arc::new(dispatcher::ConnectionManagerDispatcher::new(
            connection_manager.clone(),
        ));
        let ws_sink: Arc<dyn Sink> = Arc::new(WebSocketSink::new(ws_dispatcher));

        for config in sink_configs {
            if config.sink_type == SinkType::WebSocket {
                sink_registry.register(&config.name, ws_sink.clone());
                tracing::info!(
                    sink = %config.name,
                    "auto-wired WebSocket sink to ConnectionManager"
                );
            }
        }
    }
}

/// Parts of the WebSocket surface that do not apply the policies yet.
fn uncovered_parts(host: &ServerHost) -> Vec<&'static str> {
    let wired_sinks = host.sink_registry.is_some()
        && host
            .config
            .sinks
            .as_ref()
            .is_some_and(|s| s.iter().any(|c| c.sink_type == SinkType::WebSocket));
    if wired_sinks {
        // `WebSocketSink::broadcast` reaches every connection, whatever its caller.
        vec!["websocket sinks (broadcast)"]
    } else {
        vec![]
    }
}
