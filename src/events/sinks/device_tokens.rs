//! Device token store for push notifications
//!
//! Stores device push tokens (Expo, APNs, FCM) per user, enabling
//! the `PushNotificationSink` to look up where to send push notifications.
//!
//! # Token lifecycle
//!
//! 1. Client registers a token: `register(user_id, token, platform)`
//! 2. Push sink delivers to all user tokens: `get_tokens(user_id)`
//! 3. Client unregisters on logout: `unregister(user_id, token)`

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use tokio::sync::RwLock;

/// Supported push notification platforms
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Platform {
    /// iOS (APNs or Expo)
    Ios,
    /// Android (FCM or Expo)
    Android,
    /// Web (Web Push or Expo)
    Web,
}

impl std::fmt::Display for Platform {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Platform::Ios => write!(f, "ios"),
            Platform::Android => write!(f, "android"),
            Platform::Web => write!(f, "web"),
        }
    }
}

/// A registered device push token
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceToken {
    /// The push token string (e.g., Expo push token "ExponentPushToken\[xxx\]")
    pub token: String,

    /// Platform this token belongs to
    pub platform: Platform,

    /// When this token was registered
    pub registered_at: DateTime<Utc>,
}

/// In-memory device token store
///
/// Thread-safe store for device tokens, keyed by user ID.
/// Each user can have multiple tokens (multiple devices).
#[derive(Debug)]
pub struct DeviceTokenStore {
    tokens: RwLock<HashMap<String, Vec<DeviceToken>>>,
}

impl DeviceTokenStore {
    /// Create an empty device token store
    pub fn new() -> Self {
        Self {
            tokens: RwLock::new(HashMap::new()),
        }
    }

    /// Register a device token for a user
    ///
    /// If the same token already exists for this user, it is updated
    /// (platform and registered_at are refreshed).
    pub async fn register(&self, user_id: &str, token: String, platform: Platform) {
        let mut store = self.tokens.write().await;
        let user_tokens = store.entry(user_id.to_string()).or_default();

        // Update existing token or add new
        if let Some(existing) = user_tokens.iter_mut().find(|t| t.token == token) {
            existing.platform = platform;
            existing.registered_at = Utc::now();
        } else {
            user_tokens.push(DeviceToken {
                token,
                platform,
                registered_at: Utc::now(),
            });
        }
    }

    /// Unregister a device token for a user
    ///
    /// Returns true if the token was found and removed.
    pub async fn unregister(&self, user_id: &str, token: &str) -> bool {
        let mut store = self.tokens.write().await;
        if let Some(user_tokens) = store.get_mut(user_id) {
            let len_before = user_tokens.len();
            user_tokens.retain(|t| t.token != token);
            return user_tokens.len() < len_before;
        }
        false
    }

    /// Get all device tokens for a user
    pub async fn get_tokens(&self, user_id: &str) -> Vec<DeviceToken> {
        let store = self.tokens.read().await;
        store.get(user_id).cloned().unwrap_or_default()
    }

    /// Get token count for a user
    pub async fn token_count(&self, user_id: &str) -> usize {
        let store = self.tokens.read().await;
        store.get(user_id).map(|t| t.len()).unwrap_or(0)
    }

    /// Remove all tokens for a user (e.g., on account deletion)
    pub async fn remove_all(&self, user_id: &str) -> usize {
        let mut store = self.tokens.write().await;
        store.remove(user_id).map(|t| t.len()).unwrap_or(0)
    }
}

impl Default for DeviceTokenStore {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_register_and_get_tokens() {
        let store = DeviceTokenStore::new();

        store
            .register("user-A", "token-1".to_string(), Platform::Ios)
            .await;
        store
            .register("user-A", "token-2".to_string(), Platform::Android)
            .await;

        let tokens = store.get_tokens("user-A").await;
        assert_eq!(tokens.len(), 2);
        assert_eq!(tokens[0].token, "token-1");
        assert_eq!(tokens[0].platform, Platform::Ios);
        assert_eq!(tokens[1].token, "token-2");
        assert_eq!(tokens[1].platform, Platform::Android);
    }

    #[tokio::test]
    async fn test_unregister() {
        let store = DeviceTokenStore::new();

        store
            .register("user-A", "token-1".to_string(), Platform::Ios)
            .await;
        store
            .register("user-A", "token-2".to_string(), Platform::Android)
            .await;

        assert_eq!(store.token_count("user-A").await, 2);

        let removed = store.unregister("user-A", "token-1").await;
        assert!(removed);
        assert_eq!(store.token_count("user-A").await, 1);

        let tokens = store.get_tokens("user-A").await;
        assert_eq!(tokens[0].token, "token-2");
    }

    #[tokio::test]
    async fn test_unregister_nonexistent() {
        let store = DeviceTokenStore::new();
        assert!(!store.unregister("user-A", "nonexistent").await);
    }

    #[tokio::test]
    async fn test_register_duplicate_updates() {
        let store = DeviceTokenStore::new();

        store
            .register("user-A", "token-1".to_string(), Platform::Ios)
            .await;
        // Re-register same token but different platform
        store
            .register("user-A", "token-1".to_string(), Platform::Android)
            .await;

        let tokens = store.get_tokens("user-A").await;
        assert_eq!(tokens.len(), 1); // No duplicate
        assert_eq!(tokens[0].platform, Platform::Android); // Updated
    }

    #[tokio::test]
    async fn test_get_tokens_empty() {
        let store = DeviceTokenStore::new();
        assert!(store.get_tokens("nonexistent").await.is_empty());
        assert_eq!(store.token_count("nonexistent").await, 0);
    }

    #[tokio::test]
    async fn test_remove_all() {
        let store = DeviceTokenStore::new();

        store
            .register("user-A", "token-1".to_string(), Platform::Ios)
            .await;
        store
            .register("user-A", "token-2".to_string(), Platform::Android)
            .await;

        let removed = store.remove_all("user-A").await;
        assert_eq!(removed, 2);
        assert!(store.get_tokens("user-A").await.is_empty());
    }

    #[tokio::test]
    async fn test_separate_users() {
        let store = DeviceTokenStore::new();

        store
            .register("user-A", "token-a".to_string(), Platform::Ios)
            .await;
        store
            .register("user-B", "token-b".to_string(), Platform::Android)
            .await;

        assert_eq!(store.token_count("user-A").await, 1);
        assert_eq!(store.token_count("user-B").await, 1);

        // Unregistering from one user doesn't affect another
        store.unregister("user-A", "token-a").await;
        assert_eq!(store.token_count("user-A").await, 0);
        assert_eq!(store.token_count("user-B").await, 1);
    }
}
