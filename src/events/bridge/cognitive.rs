//! Cognitive notification bridge (T10)
//!
//! Listens for `FrameworkEvent::Cognitive` signals on the `EventBus` and
//! routes them to the `SinkRegistry` as rich notifications. Each signal
//! type has configurable threshold rules that determine whether a
//! notification should be sent.
//!
//! **Not feature-gated** — `CognitiveSignal` lives in `core/events.rs`
//! and can be published by any backend (obrain, postgres, custom, etc.).
//! The bridge is backend-agnostic: it only depends on `EventBus` + `SinkRegistry`.

use crate::core::events::{CognitiveSignal, EventBus, FrameworkEvent};
use crate::events::sinks::SinkRegistry;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::broadcast;

/// Configuration for cognitive notification rules
///
/// Each rule maps a signal type to a threshold and a list of sink names
/// to deliver notifications to.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CognitiveNotificationConfig {
    /// Rules keyed by signal type (e.g. "stigmergy_lock_in", "co_change_detected")
    pub rules: HashMap<String, CognitiveRule>,
    /// Default sink names when no rule-specific sinks are configured
    pub default_sinks: Vec<String>,
}

impl Default for CognitiveNotificationConfig {
    fn default() -> Self {
        let mut rules = HashMap::new();

        // StigmergyLockIn: notify when intensity > 0.8
        rules.insert(
            "stigmergy_lock_in".to_string(),
            CognitiveRule {
                threshold: Some(0.8),
                sinks: vec![],
                enabled: true,
            },
        );

        // CoChangeDetected: notify when strength > 0.7
        rules.insert(
            "co_change_detected".to_string(),
            CognitiveRule {
                threshold: Some(0.7),
                sinks: vec![],
                enabled: true,
            },
        );

        // AnomalyDetected: notify when score > 0.5
        rules.insert(
            "anomaly_detected".to_string(),
            CognitiveRule {
                threshold: Some(0.5),
                sinks: vec![],
                enabled: true,
            },
        );

        // ScarCreated: always notify (no threshold)
        rules.insert(
            "scar_created".to_string(),
            CognitiveRule {
                threshold: None,
                sinks: vec![],
                enabled: true,
            },
        );

        // EpisodeLearned: always notify (no threshold)
        rules.insert(
            "episode_learned".to_string(),
            CognitiveRule {
                threshold: None,
                sinks: vec![],
                enabled: true,
            },
        );

        Self {
            rules,
            default_sinks: vec!["in_app".to_string()],
        }
    }
}

/// A single cognitive notification rule
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CognitiveRule {
    /// Score threshold — signal is only forwarded if its score/intensity/strength
    /// exceeds this value. `None` means always forward.
    pub threshold: Option<f64>,
    /// Sink names to deliver to. If empty, uses `default_sinks` from config.
    pub sinks: Vec<String>,
    /// Whether this rule is active
    pub enabled: bool,
}

/// Bridge that listens for cognitive signals and routes them to notification sinks
pub struct CognitiveNotificationBridge {
    event_bus: Arc<EventBus>,
    sink_registry: Arc<SinkRegistry>,
    config: CognitiveNotificationConfig,
}

impl CognitiveNotificationBridge {
    /// Create a new cognitive notification bridge
    pub fn new(
        event_bus: Arc<EventBus>,
        sink_registry: Arc<SinkRegistry>,
        config: CognitiveNotificationConfig,
    ) -> Self {
        Self {
            event_bus,
            sink_registry,
            config,
        }
    }

    /// Create with default configuration
    pub fn with_defaults(event_bus: Arc<EventBus>, sink_registry: Arc<SinkRegistry>) -> Self {
        Self::new(event_bus, sink_registry, CognitiveNotificationConfig::default())
    }

    /// Start the bridge as a background task
    ///
    /// Returns a `JoinHandle` for the spawned task.
    pub fn start(self) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            let mut rx = self.event_bus.subscribe();
            loop {
                match rx.recv().await {
                    Ok(envelope) => {
                        if let FrameworkEvent::Cognitive(ref signal) = envelope.event {
                            if let Some(notification) =
                                self.evaluate_signal(signal)
                            {
                                let sinks = self.resolve_sinks(&notification.signal_type);
                                let tenant_id = envelope.tenant_id();

                                for sink_name in &sinks {
                                    let payload = match serde_json::to_value(&notification) {
                                        Ok(p) => p,
                                        Err(e) => {
                                            tracing::warn!(
                                                error = %e,
                                                "CognitiveBridge: failed to serialize notification"
                                            );
                                            continue;
                                        }
                                    };

                                    let mut context = HashMap::new();
                                    if let Some(tid) = tenant_id {
                                        context.insert(
                                            "tenant_id".to_string(),
                                            serde_json::Value::String(tid.to_string()),
                                        );
                                    }
                                    context.insert(
                                        "signal_type".to_string(),
                                        serde_json::Value::String(
                                            notification.signal_type.clone(),
                                        ),
                                    );

                                    if let Err(e) = self
                                        .sink_registry
                                        .deliver(sink_name, payload, None, &context)
                                        .await
                                    {
                                        tracing::warn!(
                                            sink = %sink_name,
                                            signal_type = %notification.signal_type,
                                            error = %e,
                                            "CognitiveBridge: failed to deliver to sink"
                                        );
                                    }
                                }

                                tracing::debug!(
                                    signal_type = %notification.signal_type,
                                    tenant_id = ?tenant_id,
                                    sinks_count = sinks.len(),
                                    "CognitiveBridge: notification delivered"
                                );
                            }
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        tracing::warn!(
                            lagged = n,
                            "CognitiveBridge lagged, {} events dropped",
                            n
                        );
                    }
                    Err(broadcast::error::RecvError::Closed) => {
                        tracing::info!("CognitiveBridge: EventBus closed, shutting down");
                        break;
                    }
                }
            }
        })
    }

    /// Evaluate a cognitive signal against the configured rules
    ///
    /// Returns a `CognitiveNotification` if the signal passes the threshold,
    /// or `None` if it should be dropped.
    fn evaluate_signal(&self, signal: &CognitiveSignal) -> Option<CognitiveNotification> {
        let signal_type = signal_type_name(signal);
        let rule = self.config.rules.get(signal_type);

        // If no rule exists for this signal type, drop it
        let rule = rule?;

        // If the rule is disabled, drop it
        if !rule.enabled {
            return None;
        }

        // Check threshold
        let score = signal_score(signal);
        if let Some(threshold) = rule.threshold {
            if score < threshold {
                return None;
            }
        }

        Some(CognitiveNotification {
            signal_type: signal_type.to_string(),
            title: signal_title(signal),
            body: signal_body(signal),
            score,
            signal: signal.clone(),
        })
    }

    /// Resolve which sinks to deliver to for a given signal type
    fn resolve_sinks(&self, signal_type: &str) -> Vec<String> {
        if let Some(rule) = self.config.rules.get(signal_type) {
            if !rule.sinks.is_empty() {
                return rule.sinks.clone();
            }
        }
        self.config.default_sinks.clone()
    }
}

/// A cognitive notification ready for delivery
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CognitiveNotification {
    pub signal_type: String,
    pub title: String,
    pub body: String,
    pub score: f64,
    pub signal: CognitiveSignal,
}

/// Extract the signal type name for rule matching
fn signal_type_name(signal: &CognitiveSignal) -> &'static str {
    match signal {
        CognitiveSignal::ScarCreated { .. } => "scar_created",
        CognitiveSignal::CoChangeDetected { .. } => "co_change_detected",
        CognitiveSignal::EpisodeLearned { .. } => "episode_learned",
        CognitiveSignal::AnomalyDetected { .. } => "anomaly_detected",
        CognitiveSignal::StigmergyLockIn { .. } => "stigmergy_lock_in",
    }
}

/// Extract the numeric score/intensity/strength from a signal
fn signal_score(signal: &CognitiveSignal) -> f64 {
    match signal {
        CognitiveSignal::StigmergyLockIn { intensity, .. } => *intensity,
        CognitiveSignal::CoChangeDetected { strength, .. } => *strength,
        CognitiveSignal::AnomalyDetected { score, .. } => *score,
        // Signals without numeric scores get 1.0 (always above any threshold)
        CognitiveSignal::ScarCreated { .. } => 1.0,
        CognitiveSignal::EpisodeLearned { .. } => 1.0,
    }
}

/// Generate a human-readable title for a cognitive signal
fn signal_title(signal: &CognitiveSignal) -> String {
    match signal {
        CognitiveSignal::ScarCreated { scar_type, .. } => {
            format!("Neural scar detected: {}", scar_type)
        }
        CognitiveSignal::CoChangeDetected { nodes, strength, .. } => {
            format!(
                "Co-change pattern ({:.0}%) across {} nodes",
                strength * 100.0,
                nodes.len()
            )
        }
        CognitiveSignal::EpisodeLearned { lesson, .. } => {
            format!("Episode learned: {}", truncate(lesson, 60))
        }
        CognitiveSignal::AnomalyDetected { anomaly_type, score, .. } => {
            format!("Anomaly detected: {} (score: {:.2})", anomaly_type, score)
        }
        CognitiveSignal::StigmergyLockIn { path, intensity, .. } => {
            format!(
                "Stigmergic lock-in ({:.0}%) on path of {} nodes",
                intensity * 100.0,
                path.len()
            )
        }
    }
}

/// Generate a human-readable body for a cognitive signal
fn signal_body(signal: &CognitiveSignal) -> String {
    match signal {
        CognitiveSignal::ScarCreated {
            node_id,
            scar_type,
            description,
        } => {
            format!(
                "A {} scar was created on node {}. {}",
                scar_type, node_id, description
            )
        }
        CognitiveSignal::CoChangeDetected { nodes, strength } => {
            let ids: Vec<String> = nodes.iter().map(|id| id.to_string()).collect();
            format!(
                "Temporal coupling detected (strength: {:.2}) between nodes: {}",
                strength,
                ids.join(", ")
            )
        }
        CognitiveSignal::EpisodeLearned { episode_id, lesson } => {
            format!("Episode {}: {}", episode_id, lesson)
        }
        CognitiveSignal::AnomalyDetected {
            node_id,
            anomaly_type,
            score,
        } => {
            format!(
                "Anomaly '{}' on node {} with score {:.2}",
                anomaly_type, node_id, score
            )
        }
        CognitiveSignal::StigmergyLockIn { path, intensity } => {
            let ids: Vec<String> = path.iter().map(|id| id.to_string()).collect();
            format!(
                "Pheromone intensity {:.2} on path: {}",
                intensity,
                ids.join(" → ")
            )
        }
    }
}

/// Truncate a string to max_len chars, appending "…" if truncated
fn truncate(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.to_string()
    } else {
        format!("{}…", &s[..max_len])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    #[test]
    fn test_signal_type_name() {
        assert_eq!(
            signal_type_name(&CognitiveSignal::ScarCreated {
                node_id: Uuid::nil(),
                scar_type: "test".into(),
                description: "test".into(),
            }),
            "scar_created"
        );
        assert_eq!(
            signal_type_name(&CognitiveSignal::StigmergyLockIn {
                path: vec![],
                intensity: 0.9,
            }),
            "stigmergy_lock_in"
        );
    }

    #[test]
    fn test_signal_score() {
        assert_eq!(
            signal_score(&CognitiveSignal::StigmergyLockIn {
                path: vec![],
                intensity: 0.85,
            }),
            0.85
        );
        assert_eq!(
            signal_score(&CognitiveSignal::CoChangeDetected {
                nodes: vec![],
                strength: 0.72,
            }),
            0.72
        );
        assert_eq!(
            signal_score(&CognitiveSignal::AnomalyDetected {
                node_id: Uuid::nil(),
                anomaly_type: "test".into(),
                score: 0.6,
            }),
            0.6
        );
        // No-score signals return 1.0
        assert_eq!(
            signal_score(&CognitiveSignal::ScarCreated {
                node_id: Uuid::nil(),
                scar_type: "test".into(),
                description: "test".into(),
            }),
            1.0
        );
    }

    #[test]
    fn test_evaluate_signal_passes_threshold() {
        let config = CognitiveNotificationConfig::default();
        let bridge = CognitiveNotificationBridge::new(
            Arc::new(EventBus::new(16)),
            Arc::new(SinkRegistry::new()),
            config,
        );

        // StigmergyLockIn with intensity 0.9 > threshold 0.8 → should pass
        let signal = CognitiveSignal::StigmergyLockIn {
            path: vec![Uuid::new_v4()],
            intensity: 0.9,
        };
        let notif = bridge.evaluate_signal(&signal);
        assert!(notif.is_some());
        assert_eq!(notif.unwrap().signal_type, "stigmergy_lock_in");
    }

    #[test]
    fn test_evaluate_signal_below_threshold() {
        let config = CognitiveNotificationConfig::default();
        let bridge = CognitiveNotificationBridge::new(
            Arc::new(EventBus::new(16)),
            Arc::new(SinkRegistry::new()),
            config,
        );

        // CoChangeDetected with strength 0.5 < threshold 0.7 → should be dropped
        let signal = CognitiveSignal::CoChangeDetected {
            nodes: vec![Uuid::new_v4(), Uuid::new_v4()],
            strength: 0.5,
        };
        assert!(bridge.evaluate_signal(&signal).is_none());
    }

    #[test]
    fn test_evaluate_signal_no_threshold_always_passes() {
        let config = CognitiveNotificationConfig::default();
        let bridge = CognitiveNotificationBridge::new(
            Arc::new(EventBus::new(16)),
            Arc::new(SinkRegistry::new()),
            config,
        );

        // ScarCreated has no threshold → always passes
        let signal = CognitiveSignal::ScarCreated {
            node_id: Uuid::new_v4(),
            scar_type: "repeated_failure".to_string(),
            description: "3 consecutive failures".to_string(),
        };
        let notif = bridge.evaluate_signal(&signal).unwrap();
        assert_eq!(notif.signal_type, "scar_created");
        assert!(notif.title.contains("repeated_failure"));
    }

    #[test]
    fn test_evaluate_signal_disabled_rule() {
        let mut config = CognitiveNotificationConfig::default();
        config
            .rules
            .get_mut("scar_created")
            .unwrap()
            .enabled = false;

        let bridge = CognitiveNotificationBridge::new(
            Arc::new(EventBus::new(16)),
            Arc::new(SinkRegistry::new()),
            config,
        );

        let signal = CognitiveSignal::ScarCreated {
            node_id: Uuid::new_v4(),
            scar_type: "test".to_string(),
            description: "test".to_string(),
        };
        assert!(bridge.evaluate_signal(&signal).is_none());
    }

    #[test]
    fn test_resolve_sinks_rule_specific() {
        let mut config = CognitiveNotificationConfig::default();
        config
            .rules
            .get_mut("anomaly_detected")
            .unwrap()
            .sinks = vec!["webhook".to_string(), "slack".to_string()];

        let bridge = CognitiveNotificationBridge::new(
            Arc::new(EventBus::new(16)),
            Arc::new(SinkRegistry::new()),
            config,
        );

        let sinks = bridge.resolve_sinks("anomaly_detected");
        assert_eq!(sinks, vec!["webhook", "slack"]);
    }

    #[test]
    fn test_resolve_sinks_falls_back_to_default() {
        let config = CognitiveNotificationConfig::default();
        let bridge = CognitiveNotificationBridge::new(
            Arc::new(EventBus::new(16)),
            Arc::new(SinkRegistry::new()),
            config,
        );

        // scar_created has empty sinks → should use default_sinks
        let sinks = bridge.resolve_sinks("scar_created");
        assert_eq!(sinks, vec!["in_app"]);
    }

    #[test]
    fn test_signal_title_formatting() {
        let title = signal_title(&CognitiveSignal::CoChangeDetected {
            nodes: vec![Uuid::nil(), Uuid::nil(), Uuid::nil()],
            strength: 0.85,
        });
        assert!(title.contains("85%"));
        assert!(title.contains("3 nodes"));
    }

    #[test]
    fn test_truncate() {
        assert_eq!(truncate("hello", 10), "hello");
        assert_eq!(truncate("hello world this is long", 10), "hello worl…");
    }

    #[test]
    fn test_cognitive_notification_serialization() {
        let notif = CognitiveNotification {
            signal_type: "scar_created".to_string(),
            title: "Test title".to_string(),
            body: "Test body".to_string(),
            score: 1.0,
            signal: CognitiveSignal::ScarCreated {
                node_id: Uuid::nil(),
                scar_type: "test".to_string(),
                description: "desc".to_string(),
            },
        };

        let json = serde_json::to_value(&notif).unwrap();
        assert_eq!(json["signal_type"], "scar_created");
        assert_eq!(json["title"], "Test title");
        assert_eq!(json["score"], 1.0);
    }

    #[tokio::test]
    async fn test_bridge_delivers_to_sink() {
        use std::sync::Mutex;

        // Mock sink that records deliveries
        #[derive(Debug)]
        struct MockSink {
            delivered: Mutex<Vec<serde_json::Value>>,
        }

        #[async_trait::async_trait]
        impl crate::events::sinks::Sink for MockSink {
            async fn deliver(
                &self,
                payload: serde_json::Value,
                _recipient_id: Option<&str>,
                _context_vars: &HashMap<String, serde_json::Value>,
            ) -> anyhow::Result<()> {
                self.delivered.lock().unwrap().push(payload);
                Ok(())
            }

            fn name(&self) -> &str {
                "mock"
            }

            fn sink_type(&self) -> crate::config::sinks::SinkType {
                crate::config::sinks::SinkType::InApp
            }
        }

        let event_bus = Arc::new(EventBus::new(16));
        let registry = Arc::new(SinkRegistry::new());
        let mock_sink = Arc::new(MockSink {
            delivered: Mutex::new(Vec::new()),
        });
        registry.register("in_app", mock_sink.clone());

        let bridge = CognitiveNotificationBridge::with_defaults(
            event_bus.clone(),
            registry,
        );
        let handle = bridge.start();

        // Yield to let the bridge subscribe
        tokio::task::yield_now().await;

        // Publish a cognitive signal above threshold
        event_bus.publish(FrameworkEvent::Cognitive(
            CognitiveSignal::StigmergyLockIn {
                path: vec![Uuid::new_v4(), Uuid::new_v4()],
                intensity: 0.95,
            },
        ));

        // Give it time to process
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let delivered = mock_sink.delivered.lock().unwrap();
        assert_eq!(delivered.len(), 1);
        assert_eq!(delivered[0]["signal_type"], "stigmergy_lock_in");
        assert!(delivered[0]["title"]
            .as_str()
            .unwrap()
            .contains("95%"));

        handle.abort();
    }

    #[tokio::test]
    async fn test_bridge_skips_below_threshold() {
        use std::sync::Mutex;

        #[derive(Debug)]
        struct MockSink {
            delivered: Mutex<Vec<serde_json::Value>>,
        }

        #[async_trait::async_trait]
        impl crate::events::sinks::Sink for MockSink {
            async fn deliver(
                &self,
                payload: serde_json::Value,
                _recipient_id: Option<&str>,
                _context_vars: &HashMap<String, serde_json::Value>,
            ) -> anyhow::Result<()> {
                self.delivered.lock().unwrap().push(payload);
                Ok(())
            }

            fn name(&self) -> &str {
                "mock"
            }

            fn sink_type(&self) -> crate::config::sinks::SinkType {
                crate::config::sinks::SinkType::InApp
            }
        }

        let event_bus = Arc::new(EventBus::new(16));
        let registry = Arc::new(SinkRegistry::new());
        let mock_sink = Arc::new(MockSink {
            delivered: Mutex::new(Vec::new()),
        });
        registry.register("in_app", mock_sink.clone());

        let bridge = CognitiveNotificationBridge::with_defaults(event_bus.clone(), registry);
        let handle = bridge.start();

        tokio::task::yield_now().await;

        // Publish a signal BELOW threshold (CoChange at 0.3 < 0.7)
        event_bus.publish(FrameworkEvent::Cognitive(
            CognitiveSignal::CoChangeDetected {
                nodes: vec![Uuid::new_v4()],
                strength: 0.3,
            },
        ));

        // And a non-cognitive event (should be ignored entirely)
        use crate::core::events::EntityEvent;
        event_bus.publish(FrameworkEvent::Entity(EntityEvent::Created {
            entity_type: "test".to_string(),
            entity_id: Uuid::new_v4(),
            data: serde_json::json!({}),
        }));

        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let delivered = mock_sink.delivered.lock().unwrap();
        assert!(delivered.is_empty(), "No notifications should be delivered");

        handle.abort();
    }

    #[tokio::test]
    async fn test_bridge_tenant_propagation() {
        use std::sync::Mutex;

        #[derive(Debug)]
        struct ContextCaptureSink {
            contexts: Mutex<Vec<HashMap<String, serde_json::Value>>>,
        }

        #[async_trait::async_trait]
        impl crate::events::sinks::Sink for ContextCaptureSink {
            async fn deliver(
                &self,
                _payload: serde_json::Value,
                _recipient_id: Option<&str>,
                context_vars: &HashMap<String, serde_json::Value>,
            ) -> anyhow::Result<()> {
                self.contexts.lock().unwrap().push(context_vars.clone());
                Ok(())
            }

            fn name(&self) -> &str {
                "ctx_capture"
            }

            fn sink_type(&self) -> crate::config::sinks::SinkType {
                crate::config::sinks::SinkType::InApp
            }
        }

        let event_bus = Arc::new(EventBus::new(16));
        let registry = Arc::new(SinkRegistry::new());
        let ctx_sink = Arc::new(ContextCaptureSink {
            contexts: Mutex::new(Vec::new()),
        });
        registry.register("in_app", ctx_sink.clone());

        let bridge = CognitiveNotificationBridge::with_defaults(event_bus.clone(), registry);
        let handle = bridge.start();

        tokio::task::yield_now().await;

        let tenant_id = Uuid::new_v4();
        event_bus.publish_for_tenant(
            FrameworkEvent::Cognitive(CognitiveSignal::ScarCreated {
                node_id: Uuid::new_v4(),
                scar_type: "failure".to_string(),
                description: "test".to_string(),
            }),
            tenant_id,
        );

        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let contexts = ctx_sink.contexts.lock().unwrap();
        assert_eq!(contexts.len(), 1);
        assert_eq!(
            contexts[0]["tenant_id"],
            serde_json::Value::String(tenant_id.to_string())
        );

        handle.abort();
    }
}
