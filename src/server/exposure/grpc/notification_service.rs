//! gRPC Notification Service implementation
//!
//! Provides CRUD operations and real-time streaming for in-app notifications.
//! Mirrors the REST `/notifications` endpoints and GraphQL notification
//! queries/mutations/subscriptions, achieving full protocol parity.
//!
//! ## Architecture
//!
//! ```text
//! EventBus → deliver operator → InAppNotificationSink
//!                                        ↓
//!                                 NotificationStore
//!                                   ↓           ↓
//!                              REST/GraphQL   NotificationServiceImpl
//!                                              ↓ (broadcast channel)
//!                                           gRPC stream → client
//! ```

use super::convert::json_to_struct;
use super::proto::{
    DeleteNotificationRequest, DeleteNotificationResponse, GetUnreadCountRequest,
    GetUnreadCountResponse, ListNotificationsRequest, ListNotificationsResponse,
    MarkAllAsReadRequest, MarkAllAsReadResponse, MarkAsReadRequest, MarkAsReadResponse,
    NotificationResponse, SubscribeNotificationsRequest,
    notification_service_server::NotificationService,
};
use crate::server::host::ServerHost;
use std::pin::Pin;
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tonic::{Request, Response, Status};
use uuid::Uuid;

/// gRPC Notification Service implementation
///
/// Provides notification listing, mark-as-read, deletion, and real-time
/// streaming via the `NotificationStore` broadcast channel.
pub struct NotificationServiceImpl {
    host: Arc<ServerHost>,
}

impl NotificationServiceImpl {
    /// Create a new `NotificationServiceImpl` from a `ServerHost`
    pub fn new(host: Arc<ServerHost>) -> Self {
        Self { host }
    }

    /// Get the NotificationStore or return UNAVAILABLE
    fn store(&self) -> Result<&Arc<crate::events::sinks::in_app::NotificationStore>, Status> {
        self.host.notification_store().ok_or_else(|| {
            Status::unavailable(
                "NotificationStore not configured — notification features are not available",
            )
        })
    }
}

// ---------------------------------------------------------------------------
// StoredNotification → NotificationResponse conversion
// ---------------------------------------------------------------------------

/// Convert a `StoredNotification` into a proto `NotificationResponse`.
fn stored_to_response(
    notif: &crate::events::sinks::in_app::StoredNotification,
) -> NotificationResponse {
    let data = if notif.data.is_null() {
        None
    } else {
        Some(json_to_struct(&notif.data))
    };

    NotificationResponse {
        id: notif.id.to_string(),
        recipient_id: notif.recipient_id.clone(),
        notification_type: notif.notification_type.clone(),
        title: notif.title.clone(),
        body: notif.body.clone(),
        data,
        read: notif.read,
        created_at: notif.created_at.to_rfc3339(),
    }
}

// ---------------------------------------------------------------------------
// gRPC trait implementation
// ---------------------------------------------------------------------------

type NotificationStream =
    Pin<Box<dyn tokio_stream::Stream<Item = Result<NotificationResponse, Status>> + Send>>;

#[tonic::async_trait]
impl NotificationService for NotificationServiceImpl {
    type SubscribeNotificationsStream = NotificationStream;

    async fn list_notifications(
        &self,
        request: Request<ListNotificationsRequest>,
    ) -> Result<Response<ListNotificationsResponse>, Status> {
        let req = request.into_inner();
        let store = self.store()?;

        if req.user_id.is_empty() {
            return Err(Status::invalid_argument("user_id is required"));
        }

        let limit = if req.limit > 0 {
            (req.limit as usize).min(100)
        } else {
            20
        };
        let offset = req.offset.max(0) as usize;

        let notifications = store.list_by_user(&req.user_id, limit, offset).await;
        let total = store.total_count(&req.user_id).await;
        let unread = store.unread_count(&req.user_id).await;

        let items: Vec<NotificationResponse> =
            notifications.iter().map(stored_to_response).collect();

        Ok(Response::new(ListNotificationsResponse {
            notifications: items,
            total: total as i32,
            unread: unread as i32,
        }))
    }

    async fn get_unread_count(
        &self,
        request: Request<GetUnreadCountRequest>,
    ) -> Result<Response<GetUnreadCountResponse>, Status> {
        let req = request.into_inner();
        let store = self.store()?;

        if req.user_id.is_empty() {
            return Err(Status::invalid_argument("user_id is required"));
        }

        let count = store.unread_count(&req.user_id).await;

        Ok(Response::new(GetUnreadCountResponse {
            count: count as i32,
        }))
    }

    async fn mark_as_read(
        &self,
        request: Request<MarkAsReadRequest>,
    ) -> Result<Response<MarkAsReadResponse>, Status> {
        let req = request.into_inner();
        let store = self.store()?;

        if req.notification_ids.is_empty() {
            return Err(Status::invalid_argument(
                "At least one notification_id is required",
            ));
        }

        let ids: Vec<Uuid> = req
            .notification_ids
            .iter()
            .map(|s| {
                Uuid::parse_str(s)
                    .map_err(|_| Status::invalid_argument(format!("Invalid UUID: {}", s)))
            })
            .collect::<Result<Vec<_>, _>>()?;

        let recipient = req.user_id.as_deref().filter(|s| !s.is_empty());
        let marked = store.mark_as_read(&ids, recipient).await;

        Ok(Response::new(MarkAsReadResponse {
            marked_count: marked as i32,
        }))
    }

    async fn mark_all_as_read(
        &self,
        request: Request<MarkAllAsReadRequest>,
    ) -> Result<Response<MarkAllAsReadResponse>, Status> {
        let req = request.into_inner();
        let store = self.store()?;

        if req.user_id.is_empty() {
            return Err(Status::invalid_argument("user_id is required"));
        }

        let marked = store.mark_all_as_read(&req.user_id).await;

        Ok(Response::new(MarkAllAsReadResponse {
            marked_count: marked as i32,
        }))
    }

    async fn delete_notification(
        &self,
        request: Request<DeleteNotificationRequest>,
    ) -> Result<Response<DeleteNotificationResponse>, Status> {
        let req = request.into_inner();
        let store = self.store()?;

        let id = Uuid::parse_str(&req.notification_id)
            .map_err(|_| Status::invalid_argument("Invalid notification_id UUID"))?;

        let success = store.delete(&id).await;

        Ok(Response::new(DeleteNotificationResponse { success }))
    }

    async fn subscribe_notifications(
        &self,
        request: Request<SubscribeNotificationsRequest>,
    ) -> Result<Response<Self::SubscribeNotificationsStream>, Status> {
        let req = request.into_inner();
        let store = self.store()?.clone();

        let user_id_filter = req.user_id.filter(|s| !s.is_empty());

        // Subscribe to the broadcast channel
        let mut rx = store.subscribe();

        // Channel to stream notifications to the gRPC response
        let (tx, client_rx) = mpsc::channel::<Result<NotificationResponse, Status>>(64);

        // Spawn background task: receive from broadcast → filter → send to gRPC stream
        tokio::spawn(async move {
            loop {
                match rx.recv().await {
                    Ok(notification) => {
                        // Filter by user_id if specified
                        if let Some(ref uid) = user_id_filter {
                            if notification.recipient_id != *uid {
                                continue;
                            }
                        }

                        let response = stored_to_response(&notification);
                        // If the client disconnected, tx.send() returns Err → break
                        if tx.send(Ok(response)).await.is_err() {
                            tracing::debug!(
                                "gRPC notification stream: client disconnected, closing"
                            );
                            break;
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(count)) => {
                        tracing::warn!(
                            "gRPC notification stream: lagged by {} notifications, skipping",
                            count
                        );
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                        tracing::info!(
                            "gRPC notification stream: NotificationStore closed, ending stream"
                        );
                        break;
                    }
                }
            }
        });

        let stream = ReceiverStream::new(client_rx);
        Ok(Response::new(
            Box::pin(stream) as Self::SubscribeNotificationsStream
        ))
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::sinks::in_app::{NotificationStore, StoredNotification};
    use chrono::Utc;
    use serde_json::json;

    // === Conversion tests ===

    #[test]
    fn test_stored_to_response_with_data() {
        let notif = StoredNotification {
            id: Uuid::new_v4(),
            recipient_id: "user-A".to_string(),
            notification_type: "new_follower".to_string(),
            title: "New follower".to_string(),
            body: "Alice followed you".to_string(),
            data: json!({"follower_name": "Alice"}),
            read: false,
            created_at: Utc::now(),
        };

        let resp = stored_to_response(&notif);

        assert_eq!(resp.id, notif.id.to_string());
        assert_eq!(resp.recipient_id, "user-A");
        assert_eq!(resp.notification_type, "new_follower");
        assert_eq!(resp.title, "New follower");
        assert_eq!(resp.body, "Alice followed you");
        assert!(!resp.read);
        assert!(resp.data.is_some());
        assert!(!resp.created_at.is_empty());
    }

    #[test]
    fn test_stored_to_response_null_data() {
        let notif = StoredNotification {
            id: Uuid::new_v4(),
            recipient_id: "user-B".to_string(),
            notification_type: "test".to_string(),
            title: "Test".to_string(),
            body: String::new(),
            data: serde_json::Value::Null,
            read: true,
            created_at: Utc::now(),
        };

        let resp = stored_to_response(&notif);

        assert_eq!(resp.recipient_id, "user-B");
        assert!(resp.read);
        assert!(resp.data.is_none(), "null data should map to None");
    }

    // === ListNotifications tests ===

    #[tokio::test]
    async fn test_list_notifications_empty() {
        let store = Arc::new(NotificationStore::new());
        let host = Arc::new(ServerHost::minimal_for_test().with_notification_store(store));
        let svc = NotificationServiceImpl::new(host);

        let request = Request::new(ListNotificationsRequest {
            user_id: "user-A".to_string(),
            limit: 10,
            offset: 0,
        });

        let response = svc.list_notifications(request).await.unwrap();
        let inner = response.into_inner();

        assert!(inner.notifications.is_empty());
        assert_eq!(inner.total, 0);
        assert_eq!(inner.unread, 0);
    }

    #[tokio::test]
    async fn test_list_notifications_with_data() {
        let store = Arc::new(NotificationStore::new());
        for i in 0..3 {
            store
                .insert(StoredNotification {
                    id: Uuid::new_v4(),
                    recipient_id: "user-A".to_string(),
                    notification_type: "test".to_string(),
                    title: format!("Notif {}", i),
                    body: String::new(),
                    data: serde_json::Value::Null,
                    read: false,
                    created_at: Utc::now() + chrono::Duration::seconds(i as i64),
                })
                .await;
        }

        let host = Arc::new(ServerHost::minimal_for_test().with_notification_store(store));
        let svc = NotificationServiceImpl::new(host);

        let request = Request::new(ListNotificationsRequest {
            user_id: "user-A".to_string(),
            limit: 10,
            offset: 0,
        });

        let response = svc.list_notifications(request).await.unwrap();
        let inner = response.into_inner();

        assert_eq!(inner.notifications.len(), 3);
        assert_eq!(inner.total, 3);
        assert_eq!(inner.unread, 3);
        // Newest first
        assert_eq!(inner.notifications[0].title, "Notif 2");
    }

    #[tokio::test]
    async fn test_list_notifications_missing_user_id() {
        let store = Arc::new(NotificationStore::new());
        let host = Arc::new(ServerHost::minimal_for_test().with_notification_store(store));
        let svc = NotificationServiceImpl::new(host);

        let request = Request::new(ListNotificationsRequest {
            user_id: String::new(),
            limit: 10,
            offset: 0,
        });

        let result = svc.list_notifications(request).await;
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().code(), tonic::Code::InvalidArgument);
    }

    // === GetUnreadCount tests ===

    #[tokio::test]
    async fn test_get_unread_count() {
        let store = Arc::new(NotificationStore::new());
        for i in 0..5 {
            store
                .insert(StoredNotification {
                    id: Uuid::new_v4(),
                    recipient_id: "user-A".to_string(),
                    notification_type: "test".to_string(),
                    title: format!("Notif {}", i),
                    body: String::new(),
                    data: serde_json::Value::Null,
                    read: false,
                    created_at: Utc::now(),
                })
                .await;
        }
        // Mark 2 as read
        let notifs = store.list_by_user("user-A", 2, 0).await;
        let ids: Vec<Uuid> = notifs.iter().map(|n| n.id).collect();
        store.mark_as_read(&ids, Some("user-A")).await;

        let host = Arc::new(ServerHost::minimal_for_test().with_notification_store(store));
        let svc = NotificationServiceImpl::new(host);

        let request = Request::new(GetUnreadCountRequest {
            user_id: "user-A".to_string(),
        });

        let response = svc.get_unread_count(request).await.unwrap();
        assert_eq!(response.into_inner().count, 3);
    }

    // === MarkAsRead tests ===

    #[tokio::test]
    async fn test_mark_as_read() {
        let store = Arc::new(NotificationStore::new());
        let id1 = Uuid::new_v4();
        let id2 = Uuid::new_v4();

        for id in [id1, id2] {
            store
                .insert(StoredNotification {
                    id,
                    recipient_id: "user-A".to_string(),
                    notification_type: "test".to_string(),
                    title: "Test".to_string(),
                    body: String::new(),
                    data: serde_json::Value::Null,
                    read: false,
                    created_at: Utc::now(),
                })
                .await;
        }

        let host = Arc::new(ServerHost::minimal_for_test().with_notification_store(store));
        let svc = NotificationServiceImpl::new(host);

        let request = Request::new(MarkAsReadRequest {
            notification_ids: vec![id1.to_string()],
            user_id: Some("user-A".to_string()),
        });

        let response = svc.mark_as_read(request).await.unwrap();
        assert_eq!(response.into_inner().marked_count, 1);
    }

    #[tokio::test]
    async fn test_mark_as_read_invalid_uuid() {
        let store = Arc::new(NotificationStore::new());
        let host = Arc::new(ServerHost::minimal_for_test().with_notification_store(store));
        let svc = NotificationServiceImpl::new(host);

        let request = Request::new(MarkAsReadRequest {
            notification_ids: vec!["not-a-uuid".to_string()],
            user_id: None,
        });

        let result = svc.mark_as_read(request).await;
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().code(), tonic::Code::InvalidArgument);
    }

    // === MarkAllAsRead tests ===

    #[tokio::test]
    async fn test_mark_all_as_read() {
        let store = Arc::new(NotificationStore::new());
        for _ in 0..3 {
            store
                .insert(StoredNotification {
                    id: Uuid::new_v4(),
                    recipient_id: "user-A".to_string(),
                    notification_type: "test".to_string(),
                    title: "Test".to_string(),
                    body: String::new(),
                    data: serde_json::Value::Null,
                    read: false,
                    created_at: Utc::now(),
                })
                .await;
        }

        let host = Arc::new(ServerHost::minimal_for_test().with_notification_store(store));
        let svc = NotificationServiceImpl::new(host);

        let request = Request::new(MarkAllAsReadRequest {
            user_id: "user-A".to_string(),
        });

        let response = svc.mark_all_as_read(request).await.unwrap();
        assert_eq!(response.into_inner().marked_count, 3);
    }

    // === DeleteNotification tests ===

    #[tokio::test]
    async fn test_delete_notification() {
        let store = Arc::new(NotificationStore::new());
        let id = Uuid::new_v4();

        store
            .insert(StoredNotification {
                id,
                recipient_id: "user-A".to_string(),
                notification_type: "test".to_string(),
                title: "To delete".to_string(),
                body: String::new(),
                data: serde_json::Value::Null,
                read: false,
                created_at: Utc::now(),
            })
            .await;

        let host = Arc::new(ServerHost::minimal_for_test().with_notification_store(store));
        let svc = NotificationServiceImpl::new(host);

        let request = Request::new(DeleteNotificationRequest {
            notification_id: id.to_string(),
        });

        let response = svc.delete_notification(request).await.unwrap();
        assert!(response.into_inner().success);
    }

    #[tokio::test]
    async fn test_delete_notification_not_found() {
        let store = Arc::new(NotificationStore::new());
        let host = Arc::new(ServerHost::minimal_for_test().with_notification_store(store));
        let svc = NotificationServiceImpl::new(host);

        let request = Request::new(DeleteNotificationRequest {
            notification_id: Uuid::new_v4().to_string(),
        });

        let response = svc.delete_notification(request).await.unwrap();
        assert!(!response.into_inner().success);
    }

    // === SubscribeNotifications tests ===

    #[tokio::test]
    async fn test_subscribe_receives_notifications() {
        use tokio_stream::StreamExt;

        let store = Arc::new(NotificationStore::new());
        let host = Arc::new(ServerHost::minimal_for_test().with_notification_store(store.clone()));
        let svc = NotificationServiceImpl::new(host);

        // Subscribe with user filter
        let request = Request::new(SubscribeNotificationsRequest {
            user_id: Some("user-A".to_string()),
        });

        let response = svc.subscribe_notifications(request).await.unwrap();
        let mut stream = response.into_inner();

        // Insert a notification for user-A (should match)
        let notif_id = Uuid::new_v4();
        store
            .insert(StoredNotification {
                id: notif_id,
                recipient_id: "user-A".to_string(),
                notification_type: "new_follower".to_string(),
                title: "New follower".to_string(),
                body: "Alice followed you".to_string(),
                data: json!({"follower": "alice"}),
                read: false,
                created_at: Utc::now(),
            })
            .await;

        // Insert a notification for user-B (should NOT match)
        store
            .insert(StoredNotification {
                id: Uuid::new_v4(),
                recipient_id: "user-B".to_string(),
                notification_type: "test".to_string(),
                title: "For B".to_string(),
                body: String::new(),
                data: serde_json::Value::Null,
                read: false,
                created_at: Utc::now(),
            })
            .await;

        // Should receive exactly 1 notification
        let msg = tokio::time::timeout(std::time::Duration::from_millis(100), stream.next())
            .await
            .expect("timed out waiting for notification")
            .expect("stream ended unexpectedly")
            .expect("received error");

        assert_eq!(msg.id, notif_id.to_string());
        assert_eq!(msg.recipient_id, "user-A");
        assert_eq!(msg.notification_type, "new_follower");
        assert_eq!(msg.title, "New follower");
        assert!(!msg.read);

        // No more matching notifications should arrive
        let timeout_result =
            tokio::time::timeout(std::time::Duration::from_millis(50), stream.next()).await;
        assert!(
            timeout_result.is_err(),
            "should time out — no more matching notifications"
        );
    }

    #[tokio::test]
    async fn test_subscribe_wildcard_receives_all() {
        use tokio_stream::StreamExt;

        let store = Arc::new(NotificationStore::new());
        let host = Arc::new(ServerHost::minimal_for_test().with_notification_store(store.clone()));
        let svc = NotificationServiceImpl::new(host);

        // Subscribe without user filter (wildcard)
        let request = Request::new(SubscribeNotificationsRequest { user_id: None });

        let response = svc.subscribe_notifications(request).await.unwrap();
        let mut stream = response.into_inner();

        // Insert notifications for different users
        store
            .insert(StoredNotification {
                id: Uuid::new_v4(),
                recipient_id: "user-A".to_string(),
                notification_type: "test".to_string(),
                title: "For A".to_string(),
                body: String::new(),
                data: serde_json::Value::Null,
                read: false,
                created_at: Utc::now(),
            })
            .await;

        store
            .insert(StoredNotification {
                id: Uuid::new_v4(),
                recipient_id: "user-B".to_string(),
                notification_type: "test".to_string(),
                title: "For B".to_string(),
                body: String::new(),
                data: serde_json::Value::Null,
                read: false,
                created_at: Utc::now(),
            })
            .await;

        // Should receive both
        let msg1 = tokio::time::timeout(std::time::Duration::from_millis(100), stream.next())
            .await
            .expect("timed out")
            .expect("stream ended")
            .expect("error");
        assert_eq!(msg1.recipient_id, "user-A");

        let msg2 = tokio::time::timeout(std::time::Duration::from_millis(100), stream.next())
            .await
            .expect("timed out")
            .expect("stream ended")
            .expect("error");
        assert_eq!(msg2.recipient_id, "user-B");
    }

    // === No store configured tests ===

    #[tokio::test]
    async fn test_service_without_store_returns_unavailable() {
        let host = Arc::new(ServerHost::minimal_for_test());
        let svc = NotificationServiceImpl::new(host);

        let result = svc
            .list_notifications(Request::new(ListNotificationsRequest {
                user_id: "user-A".to_string(),
                limit: 10,
                offset: 0,
            }))
            .await;

        assert!(result.is_err());
        assert_eq!(result.unwrap_err().code(), tonic::Code::Unavailable);
    }
}
