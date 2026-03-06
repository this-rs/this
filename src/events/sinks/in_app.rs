//! In-app notification sink — stores notifications per user
//!
//! This is the primary sink for the notification system. It stores
//! structured notifications in memory (extensible to a database),
//! supporting list, mark_as_read, and unread_count operations.
//!
//! # Payload format
//!
//! The `map` operator should produce a payload with these fields:
//!
//! ```json
//! {
//!     "title": "New follower",
//!     "body": "Alice started following you",
//!     "notification_type": "new_follower",
//!     "recipient_id": "user-uuid",
//!     "data": { ... }  // optional extra data
//! }
//! ```
//!
//! # Preferences
//!
//! If a `NotificationPreferencesStore` is attached, the sink checks
//! user preferences before storing. Disabled notification types are
//! silently dropped.

use crate::config::sinks::SinkType;
use crate::events::sinks::Sink;
use crate::events::sinks::preferences::NotificationPreferencesStore;
use anyhow::{Result, anyhow};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{RwLock, broadcast};
use uuid::Uuid;

/// A stored notification
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredNotification {
    /// Unique notification ID
    pub id: Uuid,

    /// Recipient user ID
    pub recipient_id: String,

    /// Notification type (e.g., "new_follower", "new_like", "new_comment")
    pub notification_type: String,

    /// Human-readable title
    pub title: String,

    /// Human-readable body
    pub body: String,

    /// Additional payload data (optional)
    #[serde(default)]
    pub data: Value,

    /// Whether the notification has been read
    pub read: bool,

    /// Creation timestamp
    pub created_at: DateTime<Utc>,
}

/// Maximum notifications stored per user before eviction
const MAX_PER_USER: usize = 1000;

/// Broadcast channel capacity for real-time notification streaming.
///
/// Subscribers that fall behind by more than this many notifications
/// will receive a `Lagged` error and miss intermediate events.
const BROADCAST_CAPACITY: usize = 256;

/// In-memory notification store
///
/// Thread-safe store for notifications, keyed by recipient_id.
/// Each recipient has their own ordered list stored in chronological order
/// (oldest first, newest last). Retrieval returns newest first.
///
/// When a user exceeds `MAX_PER_USER` notifications, the oldest are evicted.
///
/// ## Real-time streaming
///
/// A `tokio::sync::broadcast` channel broadcasts every inserted notification
/// to subscribers (e.g., gRPC server-streaming RPCs). The broadcast is
/// fire-and-forget: if no subscriber is listening, the notification is simply
/// not delivered to the channel (no error, no panic).
#[derive(Debug)]
pub struct NotificationStore {
    /// Notifications keyed by recipient_id (chronological order: oldest first)
    notifications: RwLock<HashMap<String, Vec<StoredNotification>>>,

    /// Broadcast channel for real-time notification streaming.
    /// Every `insert()` sends a clone on this channel.
    broadcast: broadcast::Sender<StoredNotification>,
}

impl NotificationStore {
    /// Create an empty store with a broadcast channel
    pub fn new() -> Self {
        let (broadcast, _) = broadcast::channel(BROADCAST_CAPACITY);
        Self {
            notifications: RwLock::new(HashMap::new()),
            broadcast,
        }
    }

    /// Subscribe to real-time notifications.
    ///
    /// Returns a receiver that will get every notification inserted after
    /// this call. Notifications inserted before are NOT replayed.
    ///
    /// The subscriber should filter by `recipient_id` if it only wants
    /// notifications for a specific user.
    pub fn subscribe(&self) -> broadcast::Receiver<StoredNotification> {
        self.broadcast.subscribe()
    }

    /// Store a notification and broadcast it to real-time subscribers.
    ///
    /// Notifications are stored in chronological order (oldest first).
    /// If the user exceeds `MAX_PER_USER`, the oldest notifications are evicted.
    ///
    /// The broadcast is fire-and-forget: if no subscriber is listening,
    /// `send()` returns `Err` which is silently ignored.
    pub async fn insert(&self, notification: StoredNotification) {
        // Broadcast to real-time subscribers (fire-and-forget)
        let _ = self.broadcast.send(notification.clone());

        let mut store = self.notifications.write().await;
        let user_notifs = store.entry(notification.recipient_id.clone()).or_default();
        user_notifs.push(notification);

        // Evict oldest if over capacity
        if user_notifs.len() > MAX_PER_USER {
            let excess = user_notifs.len() - MAX_PER_USER;
            user_notifs.drain(0..excess);
        }
    }

    /// List notifications for a user with pagination
    ///
    /// Returns notifications ordered by creation time (newest first).
    /// No sorting needed — stored in chronological order, iterated in reverse.
    pub async fn list_by_user(
        &self,
        recipient_id: &str,
        limit: usize,
        offset: usize,
    ) -> Vec<StoredNotification> {
        let store = self.notifications.read().await;
        let Some(user_notifications) = store.get(recipient_id) else {
            return Vec::new();
        };

        // Iterate in reverse (newest first) — no clone+sort needed
        user_notifications
            .iter()
            .rev()
            .skip(offset)
            .take(limit)
            .cloned()
            .collect()
    }

    /// Mark notifications as read by their IDs
    ///
    /// If `recipient_id` is provided, only searches that user's notifications
    /// (avoiding a full scan). Otherwise scans all users.
    ///
    /// Returns the number of notifications actually marked as read.
    pub async fn mark_as_read(
        &self,
        notification_ids: &[Uuid],
        recipient_id: Option<&str>,
    ) -> usize {
        let mut store = self.notifications.write().await;
        let mut count = 0;

        let values: Box<dyn Iterator<Item = &mut Vec<StoredNotification>>> =
            if let Some(rid) = recipient_id {
                // Scoped search: only this user's notifications
                Box::new(store.get_mut(rid).into_iter())
            } else {
                // Global scan (fallback)
                Box::new(store.values_mut())
            };

        for notifications in values {
            for notif in notifications.iter_mut() {
                if notification_ids.contains(&notif.id) && !notif.read {
                    notif.read = true;
                    count += 1;
                }
            }
        }

        count
    }

    /// Mark all notifications for a user as read
    pub async fn mark_all_as_read(&self, recipient_id: &str) -> usize {
        let mut store = self.notifications.write().await;
        let Some(notifications) = store.get_mut(recipient_id) else {
            return 0;
        };

        let mut count = 0;
        for notif in notifications.iter_mut() {
            if !notif.read {
                notif.read = true;
                count += 1;
            }
        }
        count
    }

    /// Count unread notifications for a user
    pub async fn unread_count(&self, recipient_id: &str) -> usize {
        let store = self.notifications.read().await;
        store
            .get(recipient_id)
            .map(|notifs| notifs.iter().filter(|n| !n.read).count())
            .unwrap_or(0)
    }

    /// Total notification count for a user
    pub async fn total_count(&self, recipient_id: &str) -> usize {
        let store = self.notifications.read().await;
        store.get(recipient_id).map(|n| n.len()).unwrap_or(0)
    }

    /// Delete a notification by ID
    pub async fn delete(&self, notification_id: &Uuid) -> bool {
        let mut store = self.notifications.write().await;
        for notifications in store.values_mut() {
            if let Some(pos) = notifications.iter().position(|n| n.id == *notification_id) {
                notifications.remove(pos);
                return true;
            }
        }
        false
    }
}

impl Default for NotificationStore {
    fn default() -> Self {
        Self::new()
    }
}

/// In-app notification sink
///
/// Receives payloads from the `deliver` operator and stores them
/// as structured notifications in the `NotificationStore`.
///
/// Optionally checks user notification preferences before storing.
#[derive(Debug)]
pub struct InAppNotificationSink {
    /// The notification store
    store: Arc<NotificationStore>,

    /// Optional preferences store (checks before delivering)
    preferences: Option<Arc<NotificationPreferencesStore>>,
}

impl InAppNotificationSink {
    /// Create a new InAppNotificationSink
    pub fn new(store: Arc<NotificationStore>) -> Self {
        Self {
            store,
            preferences: None,
        }
    }

    /// Create with a preferences store
    pub fn with_preferences(
        store: Arc<NotificationStore>,
        preferences: Arc<NotificationPreferencesStore>,
    ) -> Self {
        Self {
            store,
            preferences: Some(preferences),
        }
    }

    /// Access the underlying notification store
    pub fn store(&self) -> &Arc<NotificationStore> {
        &self.store
    }
}

#[async_trait]
impl Sink for InAppNotificationSink {
    async fn deliver(
        &self,
        payload: Value,
        recipient_id: Option<&str>,
        context_vars: &HashMap<String, Value>,
    ) -> Result<()> {
        // Determine recipient: explicit parameter > payload field > context variable
        let recipient =
            super::resolve_recipient(recipient_id, &payload, context_vars).ok_or_else(|| {
                anyhow!(
                    "in_app sink: recipient_id not found. \
                     Provide it as a parameter, in the payload, or as a context variable."
                )
            })?;

        // Extract notification fields from payload
        let title = payload
            .get("title")
            .and_then(|v| v.as_str())
            .unwrap_or("Notification")
            .to_string();

        let body = payload
            .get("body")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        let notification_type = payload
            .get("notification_type")
            .and_then(|v| v.as_str())
            .unwrap_or("generic")
            .to_string();

        let data = payload.get("data").cloned().unwrap_or(Value::Null);

        // Check preferences if available
        if let Some(prefs_store) = &self.preferences
            && !prefs_store.is_enabled(&recipient, &notification_type).await
        {
            tracing::debug!(
                recipient = %recipient,
                notification_type = %notification_type,
                "in_app sink: notification type disabled by user preferences, skipping"
            );
            return Ok(());
        }

        // Create and store the notification
        let notification = StoredNotification {
            id: Uuid::new_v4(),
            recipient_id: recipient,
            notification_type,
            title,
            body,
            data,
            read: false,
            created_at: Utc::now(),
        };

        self.store.insert(notification).await;
        Ok(())
    }

    fn name(&self) -> &str {
        "in_app"
    }

    fn sink_type(&self) -> SinkType {
        SinkType::InApp
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[tokio::test]
    async fn test_store_insert_and_list() {
        let store = NotificationStore::new();

        for i in 0..5 {
            store
                .insert(StoredNotification {
                    id: Uuid::new_v4(),
                    recipient_id: "user-A".to_string(),
                    notification_type: "new_follower".to_string(),
                    title: format!("Follower {}", i),
                    body: format!("User {} followed you", i),
                    data: Value::Null,
                    read: false,
                    created_at: Utc::now() + chrono::Duration::seconds(i as i64),
                })
                .await;
        }

        // List with limit
        let page = store.list_by_user("user-A", 3, 0).await;
        assert_eq!(page.len(), 3);
        // Newest first
        assert_eq!(page[0].title, "Follower 4");
        assert_eq!(page[1].title, "Follower 3");
        assert_eq!(page[2].title, "Follower 2");
    }

    #[tokio::test]
    async fn test_store_pagination() {
        let store = NotificationStore::new();

        for i in 0..5 {
            store
                .insert(StoredNotification {
                    id: Uuid::new_v4(),
                    recipient_id: "user-A".to_string(),
                    notification_type: "test".to_string(),
                    title: format!("Notif {}", i),
                    body: String::new(),
                    data: Value::Null,
                    read: false,
                    created_at: Utc::now() + chrono::Duration::seconds(i as i64),
                })
                .await;
        }

        // Page 2 (offset=3, limit=3) → should get 2 items
        let page2 = store.list_by_user("user-A", 3, 3).await;
        assert_eq!(page2.len(), 2);
    }

    #[tokio::test]
    async fn test_store_mark_as_read() {
        let store = NotificationStore::new();
        let id1 = Uuid::new_v4();
        let id2 = Uuid::new_v4();
        let id3 = Uuid::new_v4();

        for (id, i) in [(id1, 0), (id2, 1), (id3, 2)] {
            store
                .insert(StoredNotification {
                    id,
                    recipient_id: "user-A".to_string(),
                    notification_type: "test".to_string(),
                    title: format!("Notif {}", i),
                    body: String::new(),
                    data: Value::Null,
                    read: false,
                    created_at: Utc::now(),
                })
                .await;
        }

        assert_eq!(store.unread_count("user-A").await, 3);

        // Mark 2 as read (scoped to user-A)
        let marked = store.mark_as_read(&[id1, id2], Some("user-A")).await;
        assert_eq!(marked, 2);
        assert_eq!(store.unread_count("user-A").await, 1);
    }

    #[tokio::test]
    async fn test_store_mark_all_as_read() {
        let store = NotificationStore::new();

        for i in 0..5 {
            store
                .insert(StoredNotification {
                    id: Uuid::new_v4(),
                    recipient_id: "user-A".to_string(),
                    notification_type: "test".to_string(),
                    title: format!("Notif {}", i),
                    body: String::new(),
                    data: Value::Null,
                    read: false,
                    created_at: Utc::now(),
                })
                .await;
        }

        assert_eq!(store.unread_count("user-A").await, 5);

        let marked = store.mark_all_as_read("user-A").await;
        assert_eq!(marked, 5);
        assert_eq!(store.unread_count("user-A").await, 0);
    }

    #[tokio::test]
    async fn test_store_separate_users() {
        let store = NotificationStore::new();

        store
            .insert(StoredNotification {
                id: Uuid::new_v4(),
                recipient_id: "user-A".to_string(),
                notification_type: "test".to_string(),
                title: "For A".to_string(),
                body: String::new(),
                data: Value::Null,
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
                data: Value::Null,
                read: false,
                created_at: Utc::now(),
            })
            .await;

        assert_eq!(store.unread_count("user-A").await, 1);
        assert_eq!(store.unread_count("user-B").await, 1);
        assert_eq!(store.total_count("user-A").await, 1);
    }

    #[tokio::test]
    async fn test_store_delete() {
        let store = NotificationStore::new();
        let id = Uuid::new_v4();

        store
            .insert(StoredNotification {
                id,
                recipient_id: "user-A".to_string(),
                notification_type: "test".to_string(),
                title: "Will be deleted".to_string(),
                body: String::new(),
                data: Value::Null,
                read: false,
                created_at: Utc::now(),
            })
            .await;

        assert_eq!(store.total_count("user-A").await, 1);
        assert!(store.delete(&id).await);
        assert_eq!(store.total_count("user-A").await, 0);
        assert!(!store.delete(&id).await); // Already deleted
    }

    #[tokio::test]
    async fn test_store_empty_user() {
        let store = NotificationStore::new();
        assert_eq!(store.unread_count("nobody").await, 0);
        assert_eq!(store.list_by_user("nobody", 10, 0).await.len(), 0);
    }

    // ── Sink trait tests ────────────────────────────────────────────

    #[tokio::test]
    async fn test_sink_deliver_from_payload() {
        let store = Arc::new(NotificationStore::new());
        let sink = InAppNotificationSink::new(store.clone());

        let payload = json!({
            "title": "New follower",
            "body": "Alice followed you",
            "notification_type": "new_follower",
            "recipient_id": "user-A",
            "data": {"follower_name": "Alice"}
        });

        sink.deliver(payload, None, &HashMap::new()).await.unwrap();

        let notifs = store.list_by_user("user-A", 10, 0).await;
        assert_eq!(notifs.len(), 1);
        assert_eq!(notifs[0].title, "New follower");
        assert_eq!(notifs[0].body, "Alice followed you");
        assert_eq!(notifs[0].notification_type, "new_follower");
        assert!(!notifs[0].read);
        assert_eq!(notifs[0].data, json!({"follower_name": "Alice"}));
    }

    #[tokio::test]
    async fn test_sink_deliver_explicit_recipient() {
        let store = Arc::new(NotificationStore::new());
        let sink = InAppNotificationSink::new(store.clone());

        let payload = json!({
            "title": "Hello",
            "body": "World",
            "notification_type": "test"
        });

        // Explicit recipient_id parameter overrides payload
        sink.deliver(payload, Some("user-B"), &HashMap::new())
            .await
            .unwrap();

        assert_eq!(store.unread_count("user-B").await, 1);
    }

    #[tokio::test]
    async fn test_sink_deliver_recipient_from_context() {
        let store = Arc::new(NotificationStore::new());
        let sink = InAppNotificationSink::new(store.clone());

        let payload = json!({
            "title": "Hello",
            "notification_type": "test"
        });

        let mut vars = HashMap::new();
        vars.insert(
            "recipient_id".to_string(),
            Value::String("user-C".to_string()),
        );

        sink.deliver(payload, None, &vars).await.unwrap();
        assert_eq!(store.unread_count("user-C").await, 1);
    }

    #[tokio::test]
    async fn test_sink_deliver_no_recipient_error() {
        let store = Arc::new(NotificationStore::new());
        let sink = InAppNotificationSink::new(store);

        let payload = json!({
            "title": "Hello",
            "notification_type": "test"
        });

        let result = sink.deliver(payload, None, &HashMap::new()).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("recipient_id"));
    }

    #[tokio::test]
    async fn test_sink_deliver_defaults() {
        let store = Arc::new(NotificationStore::new());
        let sink = InAppNotificationSink::new(store.clone());

        // Minimal payload — no title, body, notification_type
        let payload = json!({
            "recipient_id": "user-A"
        });

        sink.deliver(payload, None, &HashMap::new()).await.unwrap();

        let notifs = store.list_by_user("user-A", 10, 0).await;
        assert_eq!(notifs[0].title, "Notification");
        assert_eq!(notifs[0].body, "");
        assert_eq!(notifs[0].notification_type, "generic");
    }

    #[tokio::test]
    async fn test_sink_name_and_type() {
        let sink = InAppNotificationSink::new(Arc::new(NotificationStore::new()));
        assert_eq!(sink.name(), "in_app");
        assert_eq!(sink.sink_type(), SinkType::InApp);
    }

    // ── Preferences integration tests ───────────────────────────────

    #[tokio::test]
    async fn test_sink_with_preferences_disabled_type_skipped() {
        let store = Arc::new(NotificationStore::new());
        let prefs = Arc::new(NotificationPreferencesStore::new());
        prefs.disable_type("user-A", "new_like").await;

        let sink = InAppNotificationSink::with_preferences(store.clone(), prefs);

        // Deliver a "new_like" notification — should be skipped
        let payload = json!({
            "title": "New like",
            "notification_type": "new_like",
            "recipient_id": "user-A"
        });
        sink.deliver(payload, None, &HashMap::new()).await.unwrap();
        assert_eq!(store.unread_count("user-A").await, 0);

        // Deliver a "new_follower" notification — should be stored
        let payload = json!({
            "title": "New follower",
            "notification_type": "new_follower",
            "recipient_id": "user-A"
        });
        sink.deliver(payload, None, &HashMap::new()).await.unwrap();
        assert_eq!(store.unread_count("user-A").await, 1);
    }

    #[tokio::test]
    async fn test_sink_with_preferences_muted_user() {
        let store = Arc::new(NotificationStore::new());
        let prefs = Arc::new(NotificationPreferencesStore::new());
        prefs.mute("user-A").await;

        let sink = InAppNotificationSink::with_preferences(store.clone(), prefs);

        // All notification types should be skipped when muted
        for notif_type in &["new_follower", "new_like", "new_comment"] {
            let payload = json!({
                "title": "Test",
                "notification_type": notif_type,
                "recipient_id": "user-A"
            });
            sink.deliver(payload, None, &HashMap::new()).await.unwrap();
        }

        assert_eq!(store.unread_count("user-A").await, 0);
    }

    #[tokio::test]
    async fn test_sink_without_preferences_delivers_all() {
        let store = Arc::new(NotificationStore::new());
        // No preferences store → all types delivered
        let sink = InAppNotificationSink::new(store.clone());

        for notif_type in &["new_follower", "new_like", "new_comment"] {
            let payload = json!({
                "title": "Test",
                "notification_type": notif_type,
                "recipient_id": "user-A"
            });
            sink.deliver(payload, None, &HashMap::new()).await.unwrap();
        }

        assert_eq!(store.unread_count("user-A").await, 3);
    }

    // ── Eviction + mark_as_read scoped tests ──────────────────────────

    #[tokio::test]
    async fn test_store_evicts_oldest_beyond_max() {
        let store = NotificationStore::new();

        // Insert MAX_PER_USER + 50 notifications
        let total = MAX_PER_USER + 50;
        for i in 0..total {
            store
                .insert(StoredNotification {
                    id: Uuid::new_v4(),
                    recipient_id: "user-A".to_string(),
                    notification_type: "test".to_string(),
                    title: format!("Notif {}", i),
                    body: String::new(),
                    data: Value::Null,
                    read: false,
                    created_at: Utc::now() + chrono::Duration::seconds(i as i64),
                })
                .await;
        }

        // Should be capped at MAX_PER_USER
        assert_eq!(store.total_count("user-A").await, MAX_PER_USER);

        // Newest should still be present (last inserted)
        let latest = store.list_by_user("user-A", 1, 0).await;
        assert_eq!(latest[0].title, format!("Notif {}", total - 1));

        // Oldest 50 should have been evicted — first kept is Notif 50
        let oldest = store.list_by_user("user-A", 1, MAX_PER_USER - 1).await;
        assert_eq!(oldest[0].title, "Notif 50");
    }

    #[tokio::test]
    async fn test_mark_as_read_scoped_to_recipient() {
        let store = NotificationStore::new();
        let id_a = Uuid::new_v4();
        let id_b = Uuid::new_v4();

        store
            .insert(StoredNotification {
                id: id_a,
                recipient_id: "user-A".to_string(),
                notification_type: "test".to_string(),
                title: "For A".to_string(),
                body: String::new(),
                data: Value::Null,
                read: false,
                created_at: Utc::now(),
            })
            .await;

        store
            .insert(StoredNotification {
                id: id_b,
                recipient_id: "user-B".to_string(),
                notification_type: "test".to_string(),
                title: "For B".to_string(),
                body: String::new(),
                data: Value::Null,
                read: false,
                created_at: Utc::now(),
            })
            .await;

        // Scoped mark_as_read: try to mark id_b but scoped to user-A → should find 0
        let marked = store.mark_as_read(&[id_b], Some("user-A")).await;
        assert_eq!(marked, 0);
        assert_eq!(store.unread_count("user-B").await, 1); // Still unread

        // Scoped mark_as_read: mark id_a scoped to user-A → should find 1
        let marked = store.mark_as_read(&[id_a], Some("user-A")).await;
        assert_eq!(marked, 1);
        assert_eq!(store.unread_count("user-A").await, 0);
    }

    #[tokio::test]
    async fn test_mark_as_read_global_fallback() {
        let store = NotificationStore::new();
        let id = Uuid::new_v4();

        store
            .insert(StoredNotification {
                id,
                recipient_id: "user-A".to_string(),
                notification_type: "test".to_string(),
                title: "Test".to_string(),
                body: String::new(),
                data: Value::Null,
                read: false,
                created_at: Utc::now(),
            })
            .await;

        // Without recipient_id — global scan fallback
        let marked = store.mark_as_read(&[id], None).await;
        assert_eq!(marked, 1);
        assert_eq!(store.unread_count("user-A").await, 0);
    }

    // ── Broadcast channel tests ─────────────────────────────────────

    #[tokio::test]
    async fn test_notification_broadcast_on_insert() {
        let store = NotificationStore::new();
        let mut rx = store.subscribe();

        let notif_id = Uuid::new_v4();
        store
            .insert(StoredNotification {
                id: notif_id,
                recipient_id: "user-A".to_string(),
                notification_type: "new_follower".to_string(),
                title: "New follower".to_string(),
                body: "Alice followed you".to_string(),
                data: json!({"follower_name": "Alice"}),
                read: false,
                created_at: Utc::now(),
            })
            .await;

        let received = rx.recv().await.expect("should receive broadcast");
        assert_eq!(received.id, notif_id);
        assert_eq!(received.recipient_id, "user-A");
        assert_eq!(received.notification_type, "new_follower");
        assert_eq!(received.title, "New follower");

        // Also stored in the store
        assert_eq!(store.total_count("user-A").await, 1);
    }

    #[tokio::test]
    async fn test_broadcast_without_subscriber() {
        let store = NotificationStore::new();

        // Insert without any subscriber — should NOT panic
        store
            .insert(StoredNotification {
                id: Uuid::new_v4(),
                recipient_id: "user-A".to_string(),
                notification_type: "test".to_string(),
                title: "No one listening".to_string(),
                body: String::new(),
                data: Value::Null,
                read: false,
                created_at: Utc::now(),
            })
            .await;

        // Still stored
        assert_eq!(store.total_count("user-A").await, 1);
    }

    #[tokio::test]
    async fn test_broadcast_multiple_subscribers() {
        let store = NotificationStore::new();
        let mut rx1 = store.subscribe();
        let mut rx2 = store.subscribe();

        let notif_id = Uuid::new_v4();
        store
            .insert(StoredNotification {
                id: notif_id,
                recipient_id: "user-A".to_string(),
                notification_type: "test".to_string(),
                title: "For everyone".to_string(),
                body: String::new(),
                data: Value::Null,
                read: false,
                created_at: Utc::now(),
            })
            .await;

        let r1 = rx1.recv().await.expect("rx1 should receive");
        let r2 = rx2.recv().await.expect("rx2 should receive");

        assert_eq!(r1.id, notif_id);
        assert_eq!(r2.id, notif_id);
    }
}
