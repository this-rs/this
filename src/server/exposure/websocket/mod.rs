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
//! - `{"type": "pong"}`
//! - `{"type": "error", "message": "..."}`

mod handler;
mod manager;
pub mod protocol;

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
    pub fn build_router(host: Arc<ServerHost>) -> Result<Router> {
        let connection_manager = Arc::new(manager::ConnectionManager::new(host.clone()));

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

        let router = Router::new()
            .route("/ws", get(handler::ws_handler))
            .with_state(connection_manager);

        Ok(router)
    }
}
