//! Push notification sink — delivers via Expo, APNs, or FCM
//!
//! Uses the `PushProvider` trait to abstract the push notification backend.
//! The default implementation is `ExpoPushProvider` which sends via the
//! Expo Push API (https://exp.host/--/api/v2/push/send).
//!
//! # Retry strategy
//!
//! Failed sends are retried up to 3 times with exponential backoff:
//! - Attempt 1: immediate
//! - Attempt 2: after 100ms
//! - Attempt 3: after 500ms
//! - Attempt 4: after 2s
//!
//! Only server errors (5xx) and network errors are retried.
//! Client errors (4xx) fail immediately.

use crate::config::sinks::SinkType;
use crate::events::sinks::Sink;
use crate::events::sinks::device_tokens::DeviceTokenStore;
use crate::events::sinks::preferences::NotificationPreferencesStore;
use anyhow::{Result, anyhow};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

#[cfg(feature = "push")]
use reqwest;

/// Push message to send to a provider
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PushMessage {
    /// Device push token
    pub to: String,

    /// Notification title
    pub title: String,

    /// Notification body
    pub body: String,

    /// Extra data payload (passed to the app when notification is tapped)
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub data: Value,

    /// Sound to play (default: "default")
    #[serde(default = "default_sound")]
    pub sound: String,
}

fn default_sound() -> String {
    "default".to_string()
}

/// Result of a push send attempt
#[derive(Debug, Clone)]
pub enum PushResult {
    /// Successfully sent
    Success,
    /// Failed with retriable error (server error, network issue)
    RetriableError(String),
    /// Failed with non-retriable error (invalid token, etc.)
    PermanentError(String),
}

/// Trait for push notification providers
///
/// Abstracts the backend used to send push notifications.
/// Implementations: `ExpoPushProvider` (default), future: `ApnsPushProvider`, `FcmPushProvider`
#[async_trait]
pub trait PushProvider: Send + Sync + std::fmt::Debug {
    /// Send a batch of push messages
    ///
    /// Returns one `PushResult` per message, in the same order.
    async fn send_batch(&self, messages: Vec<PushMessage>) -> Vec<PushResult>;

    /// Provider name for logging
    fn name(&self) -> &str;
}

/// Expo Push API provider
///
/// Sends push notifications via the Expo Push API.
/// Works with Expo push tokens (format: "ExponentPushToken[xxx]").
///
/// Requires the `push` feature to be enabled.
#[cfg(feature = "push")]
#[derive(Debug)]
pub struct ExpoPushProvider {
    client: reqwest::Client,
    api_url: String,
}

#[cfg(feature = "push")]
impl ExpoPushProvider {
    /// Create with default Expo API URL
    pub fn new() -> Self {
        Self {
            client: reqwest::Client::new(),
            api_url: "https://exp.host/--/api/v2/push/send".to_string(),
        }
    }

    /// Create with a custom API URL (for testing)
    pub fn with_url(url: String) -> Self {
        Self {
            client: reqwest::Client::new(),
            api_url: url,
        }
    }
}

#[cfg(feature = "push")]
impl Default for ExpoPushProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(feature = "push")]
#[async_trait]
impl PushProvider for ExpoPushProvider {
    async fn send_batch(&self, messages: Vec<PushMessage>) -> Vec<PushResult> {
        if messages.is_empty() {
            return Vec::new();
        }

        // Expo API accepts an array of messages
        let response = self.client.post(&self.api_url).json(&messages).send().await;

        match response {
            Ok(resp) => {
                let status = resp.status();
                if status.is_success() {
                    // Parse Expo's response to check per-ticket status
                    match resp.json::<ExpoResponse>().await {
                        Ok(expo_resp) => expo_resp
                            .data
                            .into_iter()
                            .map(|ticket| match ticket.status.as_str() {
                                "ok" => PushResult::Success,
                                "error" => {
                                    let msg = ticket
                                        .message
                                        .unwrap_or_else(|| "unknown error".to_string());
                                    // DeviceNotRegistered → permanent error
                                    if ticket.details.as_ref().is_some_and(|d| {
                                        d.get("error")
                                            .and_then(|e| e.as_str())
                                            .is_some_and(|e| e == "DeviceNotRegistered")
                                    }) {
                                        PushResult::PermanentError(msg)
                                    } else {
                                        PushResult::RetriableError(msg)
                                    }
                                }
                                _ => PushResult::RetriableError(format!(
                                    "unexpected status: {}",
                                    ticket.status
                                )),
                            })
                            .collect(),
                        Err(e) => {
                            // Couldn't parse response — treat all as retriable
                            vec![
                                PushResult::RetriableError(format!(
                                    "failed to parse Expo response: {}",
                                    e
                                ));
                                messages.len()
                            ]
                        }
                    }
                } else if status.is_server_error() {
                    vec![
                        PushResult::RetriableError(format!("server error: {}", status));
                        messages.len()
                    ]
                } else {
                    // 4xx → permanent error
                    let body = resp.text().await.unwrap_or_default();
                    vec![
                        PushResult::PermanentError(format!("client error {}: {}", status, body));
                        messages.len()
                    ]
                }
            }
            Err(e) => {
                // Network error → retriable
                vec![PushResult::RetriableError(format!("network error: {}", e)); messages.len()]
            }
        }
    }

    fn name(&self) -> &str {
        "expo"
    }
}

/// Expo Push API response format
#[cfg(feature = "push")]
#[derive(Debug, Deserialize)]
struct ExpoResponse {
    data: Vec<ExpoTicket>,
}

/// Individual push ticket from Expo
#[cfg(feature = "push")]
#[derive(Debug, Deserialize)]
struct ExpoTicket {
    status: String,
    #[serde(default)]
    message: Option<String>,
    #[serde(default)]
    details: Option<Value>,
}

/// Retry configuration for push delivery
#[derive(Debug, Clone)]
pub struct RetryConfig {
    /// Maximum number of retry attempts (excluding the first attempt)
    pub max_retries: u32,
    /// Backoff durations for each retry attempt
    pub backoff: Vec<Duration>,
}

impl Default for RetryConfig {
    fn default() -> Self {
        Self {
            max_retries: 3,
            backoff: vec![
                Duration::from_millis(100),
                Duration::from_millis(500),
                Duration::from_secs(2),
            ],
        }
    }
}

/// Push notification sink
///
/// Receives payloads from the `deliver` operator and sends push
/// notifications to all registered device tokens for the recipient.
///
/// # Preferences
///
/// If a `NotificationPreferencesStore` is attached via `with_preferences`,
/// the sink checks user preferences before sending. Disabled notification
/// types are silently dropped (same pattern as `InAppNotificationSink`).
///
/// # Stale token cleanup
///
/// When a push provider returns `PermanentError` (e.g., `DeviceNotRegistered`),
/// the corresponding device token is automatically unregistered from the store.
#[derive(Debug)]
pub struct PushNotificationSink {
    /// Device token store
    device_tokens: Arc<DeviceTokenStore>,

    /// Push provider (Expo by default)
    provider: Arc<dyn PushProvider>,

    /// Retry configuration
    retry_config: RetryConfig,

    /// Optional preferences store (checks before delivering)
    preferences: Option<Arc<NotificationPreferencesStore>>,
}

impl PushNotificationSink {
    /// Create with default Expo provider and retry config
    ///
    /// Requires the `push` feature to be enabled.
    #[cfg(feature = "push")]
    pub fn new(device_tokens: Arc<DeviceTokenStore>) -> Self {
        Self {
            device_tokens,
            provider: Arc::new(ExpoPushProvider::new()),
            retry_config: RetryConfig::default(),
            preferences: None,
        }
    }

    /// Create with a custom push provider
    pub fn with_provider(
        device_tokens: Arc<DeviceTokenStore>,
        provider: Arc<dyn PushProvider>,
    ) -> Self {
        Self {
            device_tokens,
            provider,
            retry_config: RetryConfig::default(),
            preferences: None,
        }
    }

    /// Create with custom provider and retry config
    pub fn with_config(
        device_tokens: Arc<DeviceTokenStore>,
        provider: Arc<dyn PushProvider>,
        retry_config: RetryConfig,
    ) -> Self {
        Self {
            device_tokens,
            provider,
            retry_config,
            preferences: None,
        }
    }

    /// Attach a preferences store to check before sending
    ///
    /// When set, the sink checks `is_enabled(recipient, notification_type)`
    /// before sending. Disabled types are silently dropped.
    pub fn with_preferences(mut self, preferences: Arc<NotificationPreferencesStore>) -> Self {
        self.preferences = Some(preferences);
        self
    }

    /// Send messages with retry logic
    ///
    /// Returns the list of tokens that had permanent errors (e.g., `DeviceNotRegistered`).
    /// The caller should unregister these tokens from the store.
    async fn send_with_retry(&self, messages: Vec<PushMessage>) -> Result<Vec<String>> {
        let mut pending = messages;
        let mut attempt = 0;
        let mut permanently_failed_tokens: Vec<String> = Vec::new();

        loop {
            let results = self.provider.send_batch(pending.clone()).await;

            let mut failed: Vec<PushMessage> = Vec::new();
            let mut permanent_errors: Vec<String> = Vec::new();

            for (msg, result) in pending.iter().zip(results.iter()) {
                match result {
                    PushResult::Success => {}
                    PushResult::RetriableError(err) => {
                        tracing::warn!(
                            token = %msg.to,
                            error = %err,
                            attempt = attempt + 1,
                            "push: retriable error"
                        );
                        failed.push(msg.clone());
                    }
                    PushResult::PermanentError(err) => {
                        tracing::error!(
                            token = %msg.to,
                            error = %err,
                            "push: permanent error (will not retry)"
                        );
                        permanently_failed_tokens.push(msg.to.clone());
                        permanent_errors.push(err.clone());
                    }
                }
            }

            if failed.is_empty() {
                if permanent_errors.is_empty() {
                    return Ok(permanently_failed_tokens);
                } else {
                    // All retriable sent, but some had permanent errors
                    // Still return the failed tokens for cleanup
                    return Ok(permanently_failed_tokens);
                }
            }

            attempt += 1;
            if attempt > self.retry_config.max_retries {
                return Err(anyhow!(
                    "push: {} message(s) failed after {} retries",
                    failed.len(),
                    self.retry_config.max_retries
                ));
            }

            // Backoff before retry
            let backoff_idx = (attempt as usize - 1).min(self.retry_config.backoff.len() - 1);
            let delay = self.retry_config.backoff[backoff_idx];
            tracing::debug!(
                attempt = attempt,
                delay_ms = delay.as_millis(),
                remaining = failed.len(),
                "push: retrying after backoff"
            );
            tokio::time::sleep(delay).await;

            pending = failed;
        }
    }
}

#[async_trait]
impl Sink for PushNotificationSink {
    async fn deliver(
        &self,
        payload: Value,
        recipient_id: Option<&str>,
        context_vars: &HashMap<String, Value>,
    ) -> Result<()> {
        // Determine recipient
        let recipient = super::resolve_recipient(recipient_id, &payload, context_vars)
            .ok_or_else(|| anyhow!("push sink: recipient_id not found"))?;

        // Check preferences before sending (same pattern as InAppNotificationSink)
        if let Some(prefs_store) = &self.preferences {
            let notification_type = payload
                .get("notification_type")
                .and_then(|v| v.as_str())
                .unwrap_or("generic");

            if !prefs_store.is_enabled(&recipient, notification_type).await {
                tracing::debug!(
                    recipient = %recipient,
                    notification_type = %notification_type,
                    "push sink: notification type disabled by user preferences, skipping"
                );
                return Ok(());
            }
        }

        // Get device tokens
        let tokens = self.device_tokens.get_tokens(&recipient).await;
        if tokens.is_empty() {
            tracing::debug!(
                recipient = %recipient,
                "push sink: no device tokens registered, skipping"
            );
            return Ok(());
        }

        // Extract notification fields
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

        let data = payload.get("data").cloned().unwrap_or(Value::Null);

        // Build messages — one per device token
        let messages: Vec<PushMessage> = tokens
            .into_iter()
            .map(|dt| PushMessage {
                to: dt.token,
                title: title.clone(),
                body: body.clone(),
                data: data.clone(),
                sound: "default".to_string(),
            })
            .collect();

        tracing::debug!(
            recipient = %recipient,
            token_count = messages.len(),
            provider = self.provider.name(),
            "push sink: sending notifications"
        );

        // Send with retry; collect permanently failed tokens for cleanup
        let stale_tokens = self.send_with_retry(messages).await?;

        // Unregister stale tokens (e.g., DeviceNotRegistered)
        for token in &stale_tokens {
            tracing::info!(
                recipient = %recipient,
                token = %token,
                "push sink: unregistering stale device token"
            );
            self.device_tokens.unregister(&recipient, token).await;
        }

        Ok(())
    }

    fn name(&self) -> &str {
        "push"
    }

    fn sink_type(&self) -> SinkType {
        SinkType::Push
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::sinks::device_tokens::Platform;
    use serde_json::json;
    use std::sync::atomic::{AtomicUsize, Ordering};

    // ── Mock push provider ──────────────────────────────────────────

    /// Shared state for the mock push provider
    #[derive(Debug, Clone)]
    struct MockState {
        results: Arc<tokio::sync::Mutex<Vec<Vec<PushResult>>>>,
        call_count: Arc<AtomicUsize>,
        received: Arc<tokio::sync::Mutex<Vec<Vec<PushMessage>>>>,
    }

    /// A mock push provider that records calls and returns configurable results
    #[derive(Debug)]
    struct MockPushProvider {
        state: MockState,
    }

    impl MockPushProvider {
        fn new(results: Vec<Vec<PushResult>>) -> (Self, MockState) {
            let state = MockState {
                results: Arc::new(tokio::sync::Mutex::new(results)),
                call_count: Arc::new(AtomicUsize::new(0)),
                received: Arc::new(tokio::sync::Mutex::new(Vec::new())),
            };
            (
                Self {
                    state: state.clone(),
                },
                state,
            )
        }

        /// Provider that always succeeds
        fn always_success() -> (Self, MockState) {
            Self::new(Vec::new())
        }
    }

    #[async_trait]
    impl PushProvider for MockPushProvider {
        async fn send_batch(&self, messages: Vec<PushMessage>) -> Vec<PushResult> {
            let call_idx = self.state.call_count.fetch_add(1, Ordering::SeqCst);
            self.state.received.lock().await.push(messages.clone());

            let mut results = self.state.results.lock().await;
            if call_idx < results.len() {
                results[call_idx].drain(..).collect()
            } else {
                // Default: all success
                vec![PushResult::Success; messages.len()]
            }
        }

        fn name(&self) -> &str {
            "mock"
        }
    }

    fn fast_retry_config() -> RetryConfig {
        RetryConfig {
            max_retries: 3,
            backoff: vec![
                Duration::from_millis(1),
                Duration::from_millis(1),
                Duration::from_millis(1),
            ],
        }
    }

    // ── Tests ────────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_push_deliver_success() {
        let tokens = Arc::new(DeviceTokenStore::new());
        tokens
            .register(
                "user-A",
                "ExponentPushToken[abc]".to_string(),
                Platform::Ios,
            )
            .await;

        let (provider, state) = MockPushProvider::always_success();
        let sink = PushNotificationSink::with_provider(tokens, Arc::new(provider));

        let payload = json!({
            "title": "New follower",
            "body": "Alice followed you",
            "recipient_id": "user-A",
            "data": {"screen": "profile"}
        });

        sink.deliver(payload, None, &HashMap::new()).await.unwrap();

        let calls = state.received.lock().await;
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].len(), 1);
        assert_eq!(calls[0][0].to, "ExponentPushToken[abc]");
        assert_eq!(calls[0][0].title, "New follower");
        assert_eq!(calls[0][0].body, "Alice followed you");
        assert_eq!(calls[0][0].data, json!({"screen": "profile"}));
    }

    #[tokio::test]
    async fn test_push_deliver_multiple_tokens() {
        let tokens = Arc::new(DeviceTokenStore::new());
        tokens
            .register("user-A", "token-1".to_string(), Platform::Ios)
            .await;
        tokens
            .register("user-A", "token-2".to_string(), Platform::Android)
            .await;

        let (provider, state) = MockPushProvider::always_success();
        let sink = PushNotificationSink::with_provider(tokens, Arc::new(provider));

        let payload = json!({
            "title": "Test",
            "body": "Hello",
            "recipient_id": "user-A"
        });

        sink.deliver(payload, None, &HashMap::new()).await.unwrap();

        let calls = state.received.lock().await;
        assert_eq!(calls[0].len(), 2);
        assert_eq!(calls[0][0].to, "token-1");
        assert_eq!(calls[0][1].to, "token-2");
    }

    #[tokio::test]
    async fn test_push_deliver_no_tokens_skips() {
        let tokens = Arc::new(DeviceTokenStore::new());
        let (provider, state) = MockPushProvider::always_success();
        let sink = PushNotificationSink::with_provider(tokens, Arc::new(provider));

        let payload = json!({
            "title": "Test",
            "recipient_id": "user-A"
        });

        // Should succeed silently (no tokens registered)
        sink.deliver(payload, None, &HashMap::new()).await.unwrap();

        // Provider should not have been called
        assert_eq!(state.call_count.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn test_push_deliver_no_recipient_error() {
        let tokens = Arc::new(DeviceTokenStore::new());
        let (provider, _state) = MockPushProvider::always_success();
        let sink = PushNotificationSink::with_provider(tokens, Arc::new(provider));

        let payload = json!({"title": "Test"});
        let result = sink.deliver(payload, None, &HashMap::new()).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("recipient_id"));
    }

    #[tokio::test]
    async fn test_push_retry_on_server_error() {
        let tokens = Arc::new(DeviceTokenStore::new());
        tokens
            .register("user-A", "token-1".to_string(), Platform::Ios)
            .await;

        // First call: retriable error, second call: success
        let (provider, state) = MockPushProvider::new(vec![
            vec![PushResult::RetriableError("server error: 500".to_string())],
            vec![PushResult::Success],
        ]);

        let sink =
            PushNotificationSink::with_config(tokens, Arc::new(provider), fast_retry_config());

        let payload = json!({
            "title": "Test",
            "recipient_id": "user-A"
        });

        sink.deliver(payload, None, &HashMap::new()).await.unwrap();

        // Should have been called twice (initial + 1 retry)
        assert_eq!(state.call_count.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn test_push_no_retry_on_permanent_error() {
        let tokens = Arc::new(DeviceTokenStore::new());
        tokens
            .register("user-A", "token-1".to_string(), Platform::Ios)
            .await;

        let (provider, state) = MockPushProvider::new(vec![vec![PushResult::PermanentError(
            "DeviceNotRegistered".to_string(),
        )]]);

        let sink = PushNotificationSink::with_config(
            tokens.clone(),
            Arc::new(provider),
            fast_retry_config(),
        );

        let payload = json!({
            "title": "Test",
            "recipient_id": "user-A"
        });

        // Permanent errors are now handled gracefully: token cleaned up, no error
        let result = sink.deliver(payload, None, &HashMap::new()).await;
        assert!(result.is_ok());

        // Should only have been called once (no retry)
        assert_eq!(state.call_count.load(Ordering::SeqCst), 1);

        // Stale token should have been cleaned up
        assert_eq!(tokens.token_count("user-A").await, 0);
    }

    #[tokio::test]
    async fn test_push_max_retries_exceeded() {
        let tokens = Arc::new(DeviceTokenStore::new());
        tokens
            .register("user-A", "token-1".to_string(), Platform::Ios)
            .await;

        // Always returns retriable error
        let (provider, state) = MockPushProvider::new(vec![
            vec![PushResult::RetriableError("error 1".to_string())],
            vec![PushResult::RetriableError("error 2".to_string())],
            vec![PushResult::RetriableError("error 3".to_string())],
            vec![PushResult::RetriableError("error 4".to_string())],
        ]);

        let sink =
            PushNotificationSink::with_config(tokens, Arc::new(provider), fast_retry_config());

        let payload = json!({
            "title": "Test",
            "recipient_id": "user-A"
        });

        let result = sink.deliver(payload, None, &HashMap::new()).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("after 3 retries"));

        // 1 initial + 3 retries = 4 calls
        assert_eq!(state.call_count.load(Ordering::SeqCst), 4);
    }

    #[tokio::test]
    async fn test_push_explicit_recipient_overrides_payload() {
        let tokens = Arc::new(DeviceTokenStore::new());
        tokens
            .register("user-B", "token-B".to_string(), Platform::Ios)
            .await;

        let (provider, state) = MockPushProvider::always_success();
        let sink = PushNotificationSink::with_provider(tokens, Arc::new(provider));

        // Payload says user-A, but explicit param says user-B
        let payload = json!({
            "title": "Test",
            "recipient_id": "user-A"
        });

        sink.deliver(payload, Some("user-B"), &HashMap::new())
            .await
            .unwrap();

        let calls = state.received.lock().await;
        assert_eq!(calls[0][0].to, "token-B");
    }

    #[tokio::test]
    async fn test_push_message_serialization() {
        let msg = PushMessage {
            to: "ExponentPushToken[abc]".to_string(),
            title: "Hello".to_string(),
            body: "World".to_string(),
            data: json!({"screen": "home"}),
            sound: "default".to_string(),
        };

        let json = serde_json::to_value(&msg).unwrap();
        assert_eq!(json["to"], "ExponentPushToken[abc]");
        assert_eq!(json["title"], "Hello");
        assert_eq!(json["body"], "World");
        assert_eq!(json["data"]["screen"], "home");
        assert_eq!(json["sound"], "default");
    }

    #[tokio::test]
    async fn test_push_message_null_data_omitted() {
        let msg = PushMessage {
            to: "token".to_string(),
            title: "Test".to_string(),
            body: "Body".to_string(),
            data: Value::Null,
            sound: "default".to_string(),
        };

        let json = serde_json::to_value(&msg).unwrap();
        assert!(!json.as_object().unwrap().contains_key("data"));
    }

    #[test]
    fn test_sink_name_and_type() {
        let tokens = Arc::new(DeviceTokenStore::new());
        let (provider, _state) = MockPushProvider::always_success();
        let sink = PushNotificationSink::with_provider(tokens, Arc::new(provider));
        assert_eq!(sink.name(), "push");
        assert_eq!(sink.sink_type(), SinkType::Push);
    }

    // ── Preferences integration tests ─────────────────────────────

    #[tokio::test]
    async fn test_push_with_preferences_disabled_type_skipped() {
        let tokens = Arc::new(DeviceTokenStore::new());
        tokens
            .register("user-A", "token-1".to_string(), Platform::Ios)
            .await;

        let prefs = Arc::new(NotificationPreferencesStore::new());
        prefs.disable_type("user-A", "new_like").await;

        let (provider, state) = MockPushProvider::always_success();
        let sink =
            PushNotificationSink::with_provider(tokens, Arc::new(provider)).with_preferences(prefs);

        // Deliver a disabled type — should be skipped
        let payload = json!({
            "title": "New like",
            "notification_type": "new_like",
            "recipient_id": "user-A"
        });
        sink.deliver(payload, None, &HashMap::new()).await.unwrap();
        assert_eq!(state.call_count.load(Ordering::SeqCst), 0);

        // Deliver an enabled type — should send
        let payload = json!({
            "title": "New follower",
            "notification_type": "new_follower",
            "recipient_id": "user-A"
        });
        sink.deliver(payload, None, &HashMap::new()).await.unwrap();
        assert_eq!(state.call_count.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn test_push_with_preferences_muted_user_skipped() {
        let tokens = Arc::new(DeviceTokenStore::new());
        tokens
            .register("user-A", "token-1".to_string(), Platform::Ios)
            .await;

        let prefs = Arc::new(NotificationPreferencesStore::new());
        prefs.mute("user-A").await;

        let (provider, state) = MockPushProvider::always_success();
        let sink =
            PushNotificationSink::with_provider(tokens, Arc::new(provider)).with_preferences(prefs);

        let payload = json!({
            "title": "Test",
            "notification_type": "new_follower",
            "recipient_id": "user-A"
        });
        sink.deliver(payload, None, &HashMap::new()).await.unwrap();
        assert_eq!(state.call_count.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn test_push_without_preferences_delivers_all() {
        let tokens = Arc::new(DeviceTokenStore::new());
        tokens
            .register("user-A", "token-1".to_string(), Platform::Ios)
            .await;

        let (provider, state) = MockPushProvider::always_success();
        // No preferences store
        let sink = PushNotificationSink::with_provider(tokens, Arc::new(provider));

        let payload = json!({
            "title": "Test",
            "notification_type": "new_like",
            "recipient_id": "user-A"
        });
        sink.deliver(payload, None, &HashMap::new()).await.unwrap();
        assert_eq!(state.call_count.load(Ordering::SeqCst), 1);
    }

    // ── Stale token cleanup tests ─────────────────────────────────

    #[tokio::test]
    async fn test_push_permanent_error_unregisters_stale_token() {
        let tokens = Arc::new(DeviceTokenStore::new());
        tokens
            .register("user-A", "good-token".to_string(), Platform::Ios)
            .await;
        tokens
            .register("user-A", "stale-token".to_string(), Platform::Android)
            .await;
        assert_eq!(tokens.token_count("user-A").await, 2);

        // First token succeeds, second gets DeviceNotRegistered
        let (provider, _state) = MockPushProvider::new(vec![vec![
            PushResult::Success,
            PushResult::PermanentError("DeviceNotRegistered".to_string()),
        ]]);

        let sink = PushNotificationSink::with_config(
            tokens.clone(),
            Arc::new(provider),
            fast_retry_config(),
        );

        let payload = json!({
            "title": "Test",
            "recipient_id": "user-A"
        });

        // Should succeed (stale token cleaned up silently)
        sink.deliver(payload, None, &HashMap::new()).await.unwrap();

        // Stale token should be unregistered
        assert_eq!(tokens.token_count("user-A").await, 1);
        let remaining = tokens.get_tokens("user-A").await;
        assert_eq!(remaining[0].token, "good-token");
    }

    #[tokio::test]
    async fn test_push_all_tokens_permanent_error_cleans_all() {
        let tokens = Arc::new(DeviceTokenStore::new());
        tokens
            .register("user-A", "dead-1".to_string(), Platform::Ios)
            .await;
        tokens
            .register("user-A", "dead-2".to_string(), Platform::Android)
            .await;

        let (provider, _state) = MockPushProvider::new(vec![vec![
            PushResult::PermanentError("DeviceNotRegistered".to_string()),
            PushResult::PermanentError("DeviceNotRegistered".to_string()),
        ]]);

        let sink = PushNotificationSink::with_config(
            tokens.clone(),
            Arc::new(provider),
            fast_retry_config(),
        );

        let payload = json!({
            "title": "Test",
            "recipient_id": "user-A"
        });

        sink.deliver(payload, None, &HashMap::new()).await.unwrap();

        // All tokens cleaned up
        assert_eq!(tokens.token_count("user-A").await, 0);
    }
}
