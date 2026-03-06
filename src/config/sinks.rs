//! Configuration types for event sinks (notification destinations)
//!
//! These structs are deserialized from the `sinks` section of `this.yaml`.
//! Sinks define where processed events are delivered: push notifications,
//! in-app storage, WebSocket, webhook, counter updates, etc.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Sink configuration — a destination where events are delivered
///
/// ```yaml
/// sinks:
///   - name: push-notification
///     type: push
///     config:
///       provider: expo
///
///   - name: in-app-notification
///     type: in_app
///     config:
///       ttl: 30d
///
///   - name: analytics-webhook
///     type: webhook
///     config:
///       url: https://analytics.example.com/events
///       method: POST
///       headers:
///         Authorization: "Bearer {{ env.ANALYTICS_TOKEN }}"
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SinkConfig {
    /// Unique sink name (referenced by `deliver` operators in flows)
    pub name: String,

    /// Sink type
    #[serde(rename = "type")]
    pub sink_type: SinkType,

    /// Type-specific configuration
    #[serde(default)]
    pub config: HashMap<String, serde_json::Value>,
}

/// Available sink types
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum SinkType {
    /// Push notifications (Expo, APNs, FCM)
    Push,
    /// In-app notification store (list, mark_as_read, unread_count)
    InApp,
    /// Feed (ordered event stream per user)
    Feed,
    /// WebSocket dispatch to connected clients
    WebSocket,
    /// HTTP webhook (POST/PUT to external URL)
    Webhook,
    /// Counter update on an entity field
    Counter,
    /// Custom sink (user-provided implementation)
    Custom,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sink_config_push() {
        let yaml = r#"
name: push-notification
type: push
config:
  provider: expo
  retry_count: 3
"#;

        let config: SinkConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(config.name, "push-notification");
        assert_eq!(config.sink_type, SinkType::Push);
        assert_eq!(
            config.config.get("provider").unwrap(),
            &serde_json::Value::String("expo".to_string())
        );
        assert_eq!(
            config.config.get("retry_count").unwrap(),
            &serde_json::json!(3)
        );
    }

    #[test]
    fn test_sink_config_in_app() {
        let yaml = r#"
name: in-app-notification
type: in_app
config:
  ttl: 30d
"#;

        let config: SinkConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(config.name, "in-app-notification");
        assert_eq!(config.sink_type, SinkType::InApp);
    }

    #[test]
    fn test_sink_config_webhook() {
        let yaml = r#"
name: analytics-webhook
type: webhook
config:
  url: https://analytics.example.com/events
  method: POST
  headers:
    Authorization: "Bearer token123"
"#;

        let config: SinkConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(config.name, "analytics-webhook");
        assert_eq!(config.sink_type, SinkType::Webhook);
        assert!(config.config.contains_key("url"));
        assert!(config.config.contains_key("headers"));
    }

    #[test]
    fn test_sink_config_websocket() {
        let yaml = r#"
name: live-updates
type: web_socket
config:
  filter_by: recipient_id
"#;

        let config: SinkConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(config.name, "live-updates");
        assert_eq!(config.sink_type, SinkType::WebSocket);
    }

    #[test]
    fn test_sink_config_counter() {
        let yaml = r#"
name: like-counter
type: counter
config:
  field: like_count
  operation: increment
"#;

        let config: SinkConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(config.name, "like-counter");
        assert_eq!(config.sink_type, SinkType::Counter);
    }

    #[test]
    fn test_sink_config_no_config() {
        let yaml = r#"
name: simple-sink
type: in_app
"#;

        let config: SinkConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(config.name, "simple-sink");
        assert!(config.config.is_empty());
    }

    #[test]
    fn test_sink_type_serde_roundtrip() {
        let types = vec![
            SinkType::Push,
            SinkType::InApp,
            SinkType::Feed,
            SinkType::WebSocket,
            SinkType::Webhook,
            SinkType::Counter,
            SinkType::Custom,
        ];

        for t in &types {
            let json = serde_json::to_string(t).unwrap();
            let roundtrip: SinkType = serde_json::from_str(&json).unwrap();
            assert_eq!(*t, roundtrip);
        }
    }

    #[test]
    fn test_multiple_sinks_yaml() {
        let yaml = r#"
- name: push-notification
  type: push
  config:
    provider: expo

- name: in-app-notification
  type: in_app
  config:
    ttl: 30d

- name: websocket
  type: web_socket
  config:
    filter_by: recipient_id
"#;

        let sinks: Vec<SinkConfig> = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(sinks.len(), 3);
        assert_eq!(sinks[0].sink_type, SinkType::Push);
        assert_eq!(sinks[1].sink_type, SinkType::InApp);
        assert_eq!(sinks[2].sink_type, SinkType::WebSocket);
    }
}
