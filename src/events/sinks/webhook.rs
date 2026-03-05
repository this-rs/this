//! Webhook sink — delivers events to external HTTP endpoints
//!
//! Sends processed event payloads to configured webhook URLs via
//! HTTP POST (or PUT). Supports custom headers, retry with exponential
//! backoff, and configurable timeouts.
//!
//! ```yaml
//! sinks:
//!   - name: analytics-webhook
//!     type: webhook
//!     config:
//!       url: https://analytics.example.com/events
//!       method: POST
//!       headers:
//!         Authorization: "Bearer {{ env.ANALYTICS_TOKEN }}"
//! ```

use crate::config::sinks::SinkType;
use crate::events::sinks::Sink;
use anyhow::{anyhow, Result};
use async_trait::async_trait;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

/// Trait for sending HTTP requests (abstracts reqwest for testability)
#[async_trait]
pub trait HttpSender: Send + Sync + std::fmt::Debug {
    /// Send an HTTP request and return the status code
    async fn send(
        &self,
        method: &str,
        url: &str,
        headers: &HashMap<String, String>,
        body: Value,
    ) -> Result<u16>;
}

/// Webhook sink configuration
#[derive(Debug, Clone)]
pub struct WebhookConfig {
    /// Target URL
    pub url: String,

    /// HTTP method (POST or PUT)
    pub method: String,

    /// Custom headers to include
    pub headers: HashMap<String, String>,

    /// Maximum retry attempts
    pub max_retries: u32,

    /// Backoff durations for retries
    pub backoff: Vec<Duration>,

    /// Request timeout
    pub timeout: Duration,
}

impl Default for WebhookConfig {
    fn default() -> Self {
        Self {
            url: String::new(),
            method: "POST".to_string(),
            headers: HashMap::new(),
            max_retries: 3,
            backoff: vec![
                Duration::from_millis(100),
                Duration::from_millis(500),
                Duration::from_secs(2),
            ],
            timeout: Duration::from_secs(10),
        }
    }
}

/// Webhook notification sink
///
/// Sends event payloads to an HTTP endpoint. Supports retry with
/// exponential backoff on server errors and network failures.
#[derive(Debug)]
pub struct WebhookSink {
    /// Webhook configuration
    config: WebhookConfig,

    /// HTTP sender (abstract for testing)
    sender: Arc<dyn HttpSender>,
}

impl WebhookSink {
    /// Create a new WebhookSink with a sender and config
    pub fn new(sender: Arc<dyn HttpSender>, config: WebhookConfig) -> Self {
        Self { config, sender }
    }

    /// Send with retry logic
    async fn send_with_retry(&self, payload: Value) -> Result<()> {
        let mut last_error = String::new();

        for attempt in 0..=self.config.max_retries {
            if attempt > 0 {
                let backoff_idx =
                    (attempt as usize - 1).min(self.config.backoff.len() - 1);
                let delay = self.config.backoff[backoff_idx];
                tracing::debug!(
                    attempt = attempt,
                    delay_ms = delay.as_millis(),
                    "webhook: retrying after backoff"
                );
                tokio::time::sleep(delay).await;
            }

            match self
                .sender
                .send(
                    &self.config.method,
                    &self.config.url,
                    &self.config.headers,
                    payload.clone(),
                )
                .await
            {
                Ok(status) if (200..300).contains(&status) => {
                    tracing::debug!(
                        url = %self.config.url,
                        status = status,
                        "webhook: delivered successfully"
                    );
                    return Ok(());
                }
                Ok(status) if (400..500).contains(&status) => {
                    // Client error — don't retry
                    return Err(anyhow!(
                        "webhook: client error {} from {}",
                        status,
                        self.config.url
                    ));
                }
                Ok(status) => {
                    // Server error — retry
                    last_error = format!("server error {} from {}", status, self.config.url);
                    tracing::warn!(
                        url = %self.config.url,
                        status = status,
                        attempt = attempt + 1,
                        "webhook: server error, will retry"
                    );
                }
                Err(e) => {
                    // Network error — retry
                    last_error = format!("network error: {}", e);
                    tracing::warn!(
                        url = %self.config.url,
                        error = %e,
                        attempt = attempt + 1,
                        "webhook: network error, will retry"
                    );
                }
            }
        }

        Err(anyhow!(
            "webhook: failed after {} retries: {}",
            self.config.max_retries,
            last_error
        ))
    }
}

#[async_trait]
impl Sink for WebhookSink {
    async fn deliver(
        &self,
        payload: Value,
        _recipient_id: Option<&str>,
        _context_vars: &HashMap<String, Value>,
    ) -> Result<()> {
        if self.config.url.is_empty() {
            return Err(anyhow!("webhook: URL not configured"));
        }

        self.send_with_retry(payload).await
    }

    fn name(&self) -> &str {
        "webhook"
    }

    fn sink_type(&self) -> SinkType {
        SinkType::Webhook
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio::sync::Mutex;

    /// Recorded HTTP request
    #[derive(Debug, Clone)]
    struct RecordedRequest {
        method: String,
        url: String,
        headers: HashMap<String, String>,
        body: Value,
    }

    /// Mock HTTP sender
    #[derive(Debug)]
    struct MockHttpSender {
        /// Status codes to return on successive calls
        responses: Mutex<Vec<Result<u16>>>,
        /// Recorded requests
        requests: Mutex<Vec<RecordedRequest>>,
        /// Call count
        call_count: AtomicUsize,
    }

    impl MockHttpSender {
        fn with_responses(responses: Vec<Result<u16>>) -> Self {
            Self {
                responses: Mutex::new(responses),
                requests: Mutex::new(Vec::new()),
                call_count: AtomicUsize::new(0),
            }
        }

        fn always_ok() -> Self {
            Self::with_responses(vec![])
        }
    }

    #[async_trait]
    impl HttpSender for MockHttpSender {
        async fn send(
            &self,
            method: &str,
            url: &str,
            headers: &HashMap<String, String>,
            body: Value,
        ) -> Result<u16> {
            let idx = self.call_count.fetch_add(1, Ordering::SeqCst);
            self.requests.lock().await.push(RecordedRequest {
                method: method.to_string(),
                url: url.to_string(),
                headers: headers.clone(),
                body,
            });

            let mut responses = self.responses.lock().await;
            if idx < responses.len() {
                // Use a placeholder to avoid shifting
                let response = std::mem::replace(&mut responses[idx], Ok(0));
                response
            } else {
                Ok(200) // Default: success
            }
        }
    }

    fn fast_config(url: &str) -> WebhookConfig {
        WebhookConfig {
            url: url.to_string(),
            method: "POST".to_string(),
            headers: HashMap::new(),
            max_retries: 3,
            backoff: vec![
                Duration::from_millis(1),
                Duration::from_millis(1),
                Duration::from_millis(1),
            ],
            timeout: Duration::from_secs(5),
        }
    }

    #[tokio::test]
    async fn test_webhook_success() {
        let sender = Arc::new(MockHttpSender::always_ok());
        let sink = WebhookSink::new(sender.clone(), fast_config("https://example.com/hook"));

        let payload = json!({"event": "user.created", "user_id": "123"});
        sink.deliver(payload.clone(), None, &HashMap::new())
            .await
            .unwrap();

        let requests = sender.requests.lock().await;
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].method, "POST");
        assert_eq!(requests[0].url, "https://example.com/hook");
        assert_eq!(requests[0].body, payload);
    }

    #[tokio::test]
    async fn test_webhook_custom_headers() {
        let sender = Arc::new(MockHttpSender::always_ok());
        let mut config = fast_config("https://example.com/hook");
        config.headers.insert(
            "Authorization".to_string(),
            "Bearer token123".to_string(),
        );
        config.method = "PUT".to_string();

        let sink = WebhookSink::new(sender.clone(), config);
        sink.deliver(json!({}), None, &HashMap::new())
            .await
            .unwrap();

        let requests = sender.requests.lock().await;
        assert_eq!(requests[0].method, "PUT");
        assert_eq!(
            requests[0].headers.get("Authorization").unwrap(),
            "Bearer token123"
        );
    }

    #[tokio::test]
    async fn test_webhook_retry_on_server_error() {
        let sender = Arc::new(MockHttpSender::with_responses(vec![
            Ok(500), // First: server error
            Ok(200), // Second: success
        ]));

        let sink = WebhookSink::new(sender.clone(), fast_config("https://example.com"));
        sink.deliver(json!({}), None, &HashMap::new())
            .await
            .unwrap();

        assert_eq!(sender.call_count.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn test_webhook_no_retry_on_client_error() {
        let sender = Arc::new(MockHttpSender::with_responses(vec![
            Ok(400), // Client error — don't retry
        ]));

        let sink = WebhookSink::new(sender.clone(), fast_config("https://example.com"));
        let result = sink.deliver(json!({}), None, &HashMap::new()).await;

        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("client error 400"));
        assert_eq!(sender.call_count.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn test_webhook_retry_on_network_error() {
        let sender = Arc::new(MockHttpSender::with_responses(vec![
            Err(anyhow!("connection refused")),
            Ok(200),
        ]));

        let sink = WebhookSink::new(sender.clone(), fast_config("https://example.com"));
        sink.deliver(json!({}), None, &HashMap::new())
            .await
            .unwrap();

        assert_eq!(sender.call_count.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn test_webhook_max_retries_exceeded() {
        let sender = Arc::new(MockHttpSender::with_responses(vec![
            Ok(503),
            Ok(503),
            Ok(503),
            Ok(503),
        ]));

        let sink = WebhookSink::new(sender.clone(), fast_config("https://example.com"));
        let result = sink.deliver(json!({}), None, &HashMap::new()).await;

        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("after 3 retries"));
        assert_eq!(sender.call_count.load(Ordering::SeqCst), 4); // 1 + 3 retries
    }

    #[tokio::test]
    async fn test_webhook_empty_url_error() {
        let sender = Arc::new(MockHttpSender::always_ok());
        let sink = WebhookSink::new(sender, fast_config(""));

        let result = sink.deliver(json!({}), None, &HashMap::new()).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("URL not configured"));
    }

    #[test]
    fn test_webhook_sink_name_and_type() {
        let sender = Arc::new(MockHttpSender::always_ok());
        let sink = WebhookSink::new(sender, fast_config("https://example.com"));
        assert_eq!(sink.name(), "webhook");
        assert_eq!(sink.sink_type(), SinkType::Webhook);
    }
}
