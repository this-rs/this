//! Notification preferences store
//!
//! Per-user notification preferences that control which notification
//! types are enabled/disabled. The `InAppNotificationSink` consults
//! this store before storing a notification.
//!
//! # Default behavior
//!
//! All notification types are enabled by default. Users can disable
//! specific types (e.g., "new_follower", "new_like"). Unknown types
//! are treated as enabled.

use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use tokio::sync::RwLock;

/// Per-user notification preferences
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserPreferences {
    /// Notification types that are explicitly disabled
    ///
    /// Any type not in this set is considered enabled (opt-out model).
    pub disabled_types: HashSet<String>,

    /// Global mute — when true, ALL notifications are suppressed
    #[serde(default)]
    pub muted: bool,
}

impl UserPreferences {
    /// Create default preferences (everything enabled)
    pub fn new() -> Self {
        Self {
            disabled_types: HashSet::new(),
            muted: false,
        }
    }

    /// Check if a notification type is enabled
    pub fn is_type_enabled(&self, notification_type: &str) -> bool {
        if self.muted {
            return false;
        }
        !self.disabled_types.contains(notification_type)
    }

    /// Disable a notification type
    pub fn disable_type(&mut self, notification_type: impl Into<String>) {
        self.disabled_types.insert(notification_type.into());
    }

    /// Enable a notification type (remove from disabled set)
    pub fn enable_type(&mut self, notification_type: &str) {
        self.disabled_types.remove(notification_type);
    }
}

impl Default for UserPreferences {
    fn default() -> Self {
        Self::new()
    }
}

/// In-memory notification preferences store
///
/// Thread-safe store mapping user IDs to their notification preferences.
#[derive(Debug)]
pub struct NotificationPreferencesStore {
    preferences: RwLock<HashMap<String, UserPreferences>>,
}

impl NotificationPreferencesStore {
    /// Create an empty preferences store
    pub fn new() -> Self {
        Self {
            preferences: RwLock::new(HashMap::new()),
        }
    }

    /// Get preferences for a user (returns defaults if not set)
    pub async fn get(&self, user_id: &str) -> UserPreferences {
        let store = self.preferences.read().await;
        store
            .get(user_id)
            .cloned()
            .unwrap_or_default()
    }

    /// Update preferences for a user
    pub async fn update(&self, user_id: impl Into<String>, prefs: UserPreferences) {
        let mut store = self.preferences.write().await;
        store.insert(user_id.into(), prefs);
    }

    /// Check if a notification type is enabled for a user
    ///
    /// Convenience method that combines get + is_type_enabled.
    /// Returns true if the user has no preferences set (default = all enabled).
    pub async fn is_enabled(&self, user_id: &str, notification_type: &str) -> bool {
        let store = self.preferences.read().await;
        match store.get(user_id) {
            Some(prefs) => prefs.is_type_enabled(notification_type),
            None => true, // Default: all enabled
        }
    }

    /// Disable a specific notification type for a user
    pub async fn disable_type(&self, user_id: &str, notification_type: &str) {
        let mut store = self.preferences.write().await;
        let prefs = store
            .entry(user_id.to_string())
            .or_insert_with(UserPreferences::new);
        prefs.disable_type(notification_type);
    }

    /// Enable a specific notification type for a user
    pub async fn enable_type(&self, user_id: &str, notification_type: &str) {
        let mut store = self.preferences.write().await;
        if let Some(prefs) = store.get_mut(user_id) {
            prefs.enable_type(notification_type);
        }
    }

    /// Mute all notifications for a user
    pub async fn mute(&self, user_id: &str) {
        let mut store = self.preferences.write().await;
        let prefs = store
            .entry(user_id.to_string())
            .or_insert_with(UserPreferences::new);
        prefs.muted = true;
    }

    /// Unmute all notifications for a user
    pub async fn unmute(&self, user_id: &str) {
        let mut store = self.preferences.write().await;
        if let Some(prefs) = store.get_mut(user_id) {
            prefs.muted = false;
        }
    }
}

impl Default for NotificationPreferencesStore {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_user_prefs_default_all_enabled() {
        let prefs = UserPreferences::new();
        assert!(prefs.is_type_enabled("new_follower"));
        assert!(prefs.is_type_enabled("new_like"));
        assert!(prefs.is_type_enabled("anything"));
        assert!(!prefs.muted);
    }

    #[test]
    fn test_user_prefs_disable_type() {
        let mut prefs = UserPreferences::new();
        prefs.disable_type("new_like");

        assert!(!prefs.is_type_enabled("new_like"));
        assert!(prefs.is_type_enabled("new_follower"));
    }

    #[test]
    fn test_user_prefs_enable_type() {
        let mut prefs = UserPreferences::new();
        prefs.disable_type("new_like");
        assert!(!prefs.is_type_enabled("new_like"));

        prefs.enable_type("new_like");
        assert!(prefs.is_type_enabled("new_like"));
    }

    #[test]
    fn test_user_prefs_muted() {
        let mut prefs = UserPreferences::new();
        prefs.muted = true;

        assert!(!prefs.is_type_enabled("new_follower"));
        assert!(!prefs.is_type_enabled("new_like"));
        assert!(!prefs.is_type_enabled("anything"));
    }

    #[tokio::test]
    async fn test_store_default_all_enabled() {
        let store = NotificationPreferencesStore::new();
        assert!(store.is_enabled("user-A", "new_follower").await);
        assert!(store.is_enabled("user-A", "new_like").await);
    }

    #[tokio::test]
    async fn test_store_disable_type() {
        let store = NotificationPreferencesStore::new();
        store.disable_type("user-A", "new_like").await;

        assert!(!store.is_enabled("user-A", "new_like").await);
        assert!(store.is_enabled("user-A", "new_follower").await);
        // Other users not affected
        assert!(store.is_enabled("user-B", "new_like").await);
    }

    #[tokio::test]
    async fn test_store_enable_type() {
        let store = NotificationPreferencesStore::new();
        store.disable_type("user-A", "new_like").await;
        store.enable_type("user-A", "new_like").await;

        assert!(store.is_enabled("user-A", "new_like").await);
    }

    #[tokio::test]
    async fn test_store_mute_unmute() {
        let store = NotificationPreferencesStore::new();
        store.mute("user-A").await;

        assert!(!store.is_enabled("user-A", "new_follower").await);
        assert!(!store.is_enabled("user-A", "new_like").await);

        store.unmute("user-A").await;
        assert!(store.is_enabled("user-A", "new_follower").await);
    }

    #[tokio::test]
    async fn test_store_update_full_preferences() {
        let store = NotificationPreferencesStore::new();

        let mut prefs = UserPreferences::new();
        prefs.disable_type("new_follower");
        prefs.disable_type("new_comment");
        store.update("user-A", prefs).await;

        assert!(!store.is_enabled("user-A", "new_follower").await);
        assert!(!store.is_enabled("user-A", "new_comment").await);
        assert!(store.is_enabled("user-A", "new_like").await);
    }

    #[tokio::test]
    async fn test_store_get_returns_defaults() {
        let store = NotificationPreferencesStore::new();
        let prefs = store.get("nonexistent").await;
        assert!(prefs.disabled_types.is_empty());
        assert!(!prefs.muted);
    }

    #[tokio::test]
    async fn test_store_get_returns_updated() {
        let store = NotificationPreferencesStore::new();
        store.disable_type("user-A", "new_like").await;

        let prefs = store.get("user-A").await;
        assert!(prefs.disabled_types.contains("new_like"));
    }
}
