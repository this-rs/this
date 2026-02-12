//! WebSocket upgrade handler and message loop
//!
//! This module handles the HTTP → WebSocket upgrade and runs the per-connection
//! message loop. Each connection gets:
//!
//! 1. A welcome message with its unique connection ID
//! 2. A read loop that processes client messages (subscribe, unsubscribe, ping)
//! 3. A write loop that forwards server messages to the WebSocket

use super::manager::ConnectionManager;
use super::protocol::{ClientMessage, ServerMessage};
use axum::extract::ws::{Message, WebSocket};
use axum::extract::{State, WebSocketUpgrade};
use axum::response::IntoResponse;
use futures::SinkExt;
use futures::stream::StreamExt;
use std::sync::Arc;

/// WebSocket upgrade handler
///
/// This is the axum handler for GET /ws. It upgrades the HTTP connection
/// to a WebSocket connection and spawns the message loop.
pub async fn ws_handler(
    ws: WebSocketUpgrade,
    State(manager): State<Arc<ConnectionManager>>,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_socket(socket, manager))
}

/// Handle a single WebSocket connection
///
/// This function:
/// 1. Registers the connection with the ConnectionManager
/// 2. Sends a Welcome message with the connection ID
/// 3. Spawns a write loop that forwards ServerMessages to the WebSocket
/// 4. Runs the read loop that processes client messages
/// 5. Cleans up on disconnect
async fn handle_socket(socket: WebSocket, manager: Arc<ConnectionManager>) {
    let (conn_id, mut server_rx) = manager.connect().await;

    // Split the WebSocket into read and write halves
    let (mut ws_write, mut ws_read) = socket.split();

    // Send welcome message
    let welcome = ServerMessage::Welcome {
        connection_id: conn_id.clone(),
    };
    if let Ok(json) = serde_json::to_string(&welcome)
        && ws_write.send(Message::Text(json.into())).await.is_err()
    {
        manager.disconnect(&conn_id).await;
        return;
    }

    let conn_id_write = conn_id.clone();
    let conn_id_read = conn_id.clone();
    let manager_read = manager.clone();

    // Spawn write loop: forward ServerMessages from manager to WebSocket
    let write_handle = tokio::spawn(async move {
        while let Some(msg) = server_rx.recv().await {
            match serde_json::to_string(&msg) {
                Ok(json) => {
                    if ws_write.send(Message::Text(json.into())).await.is_err() {
                        tracing::debug!(
                            connection_id = %conn_id_write,
                            "WebSocket write failed, closing"
                        );
                        break;
                    }
                }
                Err(e) => {
                    tracing::error!(
                        connection_id = %conn_id_write,
                        error = %e,
                        "Failed to serialize ServerMessage"
                    );
                }
            }
        }
    });

    // Read loop: process client messages
    while let Some(result) = ws_read.next().await {
        match result {
            Ok(Message::Text(text)) => {
                handle_client_message(&manager_read, &conn_id_read, &text).await;
            }
            Ok(Message::Close(_)) => {
                tracing::debug!(connection_id = %conn_id_read, "Client sent close frame");
                break;
            }
            Ok(Message::Ping(_)) => {
                // axum handles pong automatically
            }
            Ok(_) => {
                // Ignore binary and other message types
            }
            Err(e) => {
                tracing::debug!(
                    connection_id = %conn_id_read,
                    error = %e,
                    "WebSocket read error"
                );
                break;
            }
        }
    }

    // Cleanup
    write_handle.abort();
    manager.disconnect(&conn_id).await;
}

/// Process a single client message
async fn handle_client_message(manager: &ConnectionManager, connection_id: &str, text: &str) {
    let msg: ClientMessage = match serde_json::from_str(text) {
        Ok(msg) => msg,
        Err(e) => {
            let error_msg = ServerMessage::Error {
                message: format!("Invalid message: {}", e),
            };
            manager.send_to(connection_id, error_msg).await;
            return;
        }
    };

    match msg {
        ClientMessage::Subscribe { filter } => {
            match manager.subscribe(connection_id, filter.clone()).await {
                Ok(sub_id) => {
                    let response = ServerMessage::Subscribed {
                        subscription_id: sub_id,
                        filter,
                    };
                    manager.send_to(connection_id, response).await;
                }
                Err(e) => {
                    let error_msg = ServerMessage::Error { message: e };
                    manager.send_to(connection_id, error_msg).await;
                }
            }
        }
        ClientMessage::Unsubscribe { subscription_id } => {
            match manager.unsubscribe(connection_id, &subscription_id).await {
                Ok(removed) => {
                    if removed {
                        let response = ServerMessage::Unsubscribed { subscription_id };
                        manager.send_to(connection_id, response).await;
                    } else {
                        let error_msg = ServerMessage::Error {
                            message: format!("Subscription {} not found", subscription_id),
                        };
                        manager.send_to(connection_id, error_msg).await;
                    }
                }
                Err(e) => {
                    let error_msg = ServerMessage::Error { message: e };
                    manager.send_to(connection_id, error_msg).await;
                }
            }
        }
        ClientMessage::Ping => {
            manager.send_to(connection_id, ServerMessage::Pong).await;
        }
    }
}
