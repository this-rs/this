//! Configuration types for the declarative event flow system
//!
//! These structs are deserialized from the `events` section of `this.yaml`.
//! They define the event backend, declarative flows (trigger → pipeline → deliver),
//! and consumer groups with seek positions.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Top-level events configuration
///
/// ```yaml
/// events:
///   backend:
///     type: memory
///   flows:
///     - name: notify-new-follower
///       trigger: { kind: link.created, link_type: follows }
///       pipeline: [...]
///   consumers:
///     - name: mobile-feed
///       seek: last_acknowledged
/// ```
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct EventsConfig {
    /// Event backend configuration (memory, nats, kafka, redis)
    #[serde(default)]
    pub backend: BackendConfig,

    /// Declarative event flows
    #[serde(default)]
    pub flows: Vec<FlowConfig>,

    /// Consumer groups with seek positions
    #[serde(default)]
    pub consumers: Vec<ConsumerConfig>,
}

/// Event backend configuration
///
/// ```yaml
/// backend:
///   type: memory
///   config:
///     retention: 7d
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackendConfig {
    /// Backend type: "memory" (default), "nats", "kafka", "redis"
    #[serde(rename = "type", default = "default_backend_type")]
    pub backend_type: String,

    /// Backend-specific configuration (url, stream, retention, replicas, etc.)
    #[serde(default)]
    pub config: HashMap<String, serde_json::Value>,
}

fn default_backend_type() -> String {
    "memory".to_string()
}

impl Default for BackendConfig {
    fn default() -> Self {
        Self {
            backend_type: default_backend_type(),
            config: HashMap::new(),
        }
    }
}

/// A declarative event flow definition
///
/// ```yaml
/// flows:
///   - name: notify-new-follower
///     description: "Notify user when someone follows them"
///     trigger:
///       kind: link.created
///       link_type: follows
///     pipeline:
///       - resolve:
///           from: source_id
///           as: follower
///       - map:
///           template:
///             type: follow
///             message: "{{ follower.name }} started following you"
///       - deliver:
///           sinks: [push-notification, in-app-notification]
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FlowConfig {
    /// Unique name for this flow
    pub name: String,

    /// Human-readable description
    #[serde(default)]
    pub description: Option<String>,

    /// Event trigger (what events activate this flow)
    pub trigger: TriggerConfig,

    /// Pipeline of operators to apply
    pub pipeline: Vec<PipelineStep>,
}

/// Event trigger configuration — determines which events activate a flow
///
/// ```yaml
/// trigger:
///   kind: link.created      # link.created, link.deleted, entity.created, entity.updated, entity.deleted
///   link_type: follows       # optional: filter by link type
///   entity_type: user        # optional: filter by entity type
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TriggerConfig {
    /// Event kind to match: "link.created", "link.deleted", "entity.created", "entity.updated", "entity.deleted"
    pub kind: String,

    /// Optional link type filter (only for link events)
    #[serde(default)]
    pub link_type: Option<String>,

    /// Optional entity type filter (only for entity events)
    #[serde(default)]
    pub entity_type: Option<String>,
}

/// A single step in the pipeline — wraps a PipelineOp with its config
///
/// Each step is a single-key YAML map where the key names the operator:
/// ```yaml
/// - resolve:
///     from: target_id
///     as: owner
/// - filter:
///     condition: "source_id != owner.id"
/// ```
///
/// Uses a custom Serialize/Deserialize to produce clean YAML (map keys instead of YAML tags).
#[derive(Debug, Clone)]
pub enum PipelineStep {
    /// Resolve an entity by ID or by following a link
    Resolve(ResolveConfig),
    /// Filter events based on a condition (drop if false)
    Filter(FilterConfig),
    /// Fan out to multiple recipients via link resolution (1→N)
    FanOut(FanOutConfig),
    /// Batch events by key within a time window
    Batch(BatchConfig),
    /// Deduplicate events by key within a sliding window
    Deduplicate(DeduplicateConfig),
    /// Transform the payload via a Tera template
    Map(MapConfig),
    /// Rate limit the flow
    RateLimit(RateLimitConfig),
    /// Deliver to one or more sinks
    Deliver(DeliverConfig),
}

const PIPELINE_STEP_VARIANTS: &[&str] = &[
    "resolve",
    "filter",
    "fan_out",
    "batch",
    "deduplicate",
    "map",
    "rate_limit",
    "deliver",
];

impl Serialize for PipelineStep {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeMap;
        let mut map = serializer.serialize_map(Some(1))?;
        match self {
            PipelineStep::Resolve(c) => map.serialize_entry("resolve", c)?,
            PipelineStep::Filter(c) => map.serialize_entry("filter", c)?,
            PipelineStep::FanOut(c) => map.serialize_entry("fan_out", c)?,
            PipelineStep::Batch(c) => map.serialize_entry("batch", c)?,
            PipelineStep::Deduplicate(c) => map.serialize_entry("deduplicate", c)?,
            PipelineStep::Map(c) => map.serialize_entry("map", c)?,
            PipelineStep::RateLimit(c) => map.serialize_entry("rate_limit", c)?,
            PipelineStep::Deliver(c) => map.serialize_entry("deliver", c)?,
        }
        map.end()
    }
}

impl<'de> Deserialize<'de> for PipelineStep {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        use serde::de::{self, MapAccess, Visitor};

        struct PipelineStepVisitor;

        impl<'de> Visitor<'de> for PipelineStepVisitor {
            type Value = PipelineStep;

            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                write!(
                    f,
                    "a map with a single key naming the pipeline operator (resolve, filter, fan_out, batch, deduplicate, map, rate_limit, deliver)"
                )
            }

            fn visit_map<M>(self, mut map: M) -> Result<Self::Value, M::Error>
            where
                M: MapAccess<'de>,
            {
                let key: String = map
                    .next_key()?
                    .ok_or_else(|| de::Error::custom("empty map, expected a pipeline operator"))?;
                let step = match key.as_str() {
                    "resolve" => PipelineStep::Resolve(map.next_value()?),
                    "filter" => PipelineStep::Filter(map.next_value()?),
                    "fan_out" => PipelineStep::FanOut(map.next_value()?),
                    "batch" => PipelineStep::Batch(map.next_value()?),
                    "deduplicate" => PipelineStep::Deduplicate(map.next_value()?),
                    "map" => PipelineStep::Map(map.next_value()?),
                    "rate_limit" => PipelineStep::RateLimit(map.next_value()?),
                    "deliver" => PipelineStep::Deliver(map.next_value()?),
                    _ => {
                        return Err(de::Error::unknown_variant(&key, PIPELINE_STEP_VARIANTS));
                    }
                };
                Ok(step)
            }
        }

        deserializer.deserialize_map(PipelineStepVisitor)
    }
}

/// Configuration for the `resolve` operator
///
/// Resolves an entity via its ID or by following a link through the LinkService.
///
/// ```yaml
/// - resolve:
///     from: target_id       # field containing the entity ID
///     via: owns              # optional: link type to follow
///     direction: reverse     # forward (default) or reverse
///     as: owner              # variable name to store the resolved entity
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResolveConfig {
    /// Field in the event/context containing the entity ID to resolve from
    pub from: String,

    /// Optional link type to follow (if absent, resolves the entity directly by ID)
    #[serde(default)]
    pub via: Option<String>,

    /// Direction to follow the link: "forward" (default) or "reverse"
    #[serde(default = "default_direction")]
    pub direction: String,

    /// Variable name to store the resolved entity in the FlowContext
    #[serde(rename = "as")]
    pub output_var: String,
}

fn default_direction() -> String {
    "forward".to_string()
}

/// Configuration for the `filter` operator
///
/// Drops the event if the condition evaluates to false.
///
/// ```yaml
/// - filter:
///     condition: "source_id != owner.id"
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FilterConfig {
    /// Boolean expression to evaluate against the FlowContext variables
    /// Supports: ==, !=, >, <, in, not_in, exists, not_exists
    pub condition: String,
}

/// Configuration for the `fan_out` operator
///
/// Multiplies the event for each entity linked via the specified link type.
///
/// ```yaml
/// - fan_out:
///     from: source_id
///     via: follows
///     direction: reverse
///     as: follower
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FanOutConfig {
    /// Field containing the entity ID to fan out from
    pub from: String,

    /// Link type to follow for fan-out
    pub via: String,

    /// Direction to follow the link: "forward" or "reverse"
    #[serde(default = "default_direction")]
    pub direction: String,

    /// Variable name for each iterated entity
    #[serde(rename = "as")]
    pub output_var: String,
}

/// Configuration for the `batch` operator
///
/// Accumulates events by key within a time window, emitting a single batched event
/// when the window expires.
///
/// ```yaml
/// - batch:
///     key: target_id
///     window: 5m
///     min_count: 1
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BatchConfig {
    /// Field to group events by
    pub key: String,

    /// Time window duration (e.g., "5m", "1h", "30s")
    pub window: String,

    /// Minimum number of events before emitting (default: 1)
    #[serde(default = "default_min_count")]
    pub min_count: u32,
}

fn default_min_count() -> u32 {
    1
}

/// Configuration for the `deduplicate` operator
///
/// Eliminates duplicate events within a sliding time window.
///
/// ```yaml
/// - deduplicate:
///     key: source_id
///     window: 1h
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeduplicateConfig {
    /// Field to use as deduplication key
    pub key: String,

    /// Sliding window duration (e.g., "1h", "30m")
    pub window: String,
}

/// Configuration for the `map` operator
///
/// Transforms the payload using a Tera template.
///
/// ```yaml
/// - map:
///     template:
///       type: like
///       recipient_id: "{{ owner.id }}"
///       message: "{{ source.name }} liked your trace"
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MapConfig {
    /// Template to render — each value can contain Tera expressions
    pub template: serde_json::Value,
}

/// Configuration for the `rate_limit` operator
///
/// Limits the throughput of the flow using a token bucket algorithm.
///
/// ```yaml
/// - rate_limit:
///     max: 100
///     per: 1s
///     strategy: drop
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RateLimitConfig {
    /// Maximum number of events allowed
    pub max: u32,

    /// Time period (e.g., "1s", "1m", "1h")
    pub per: String,

    /// Strategy when limit is exceeded: "drop" (default) or "queue"
    #[serde(default = "default_rate_limit_strategy")]
    pub strategy: String,
}

fn default_rate_limit_strategy() -> String {
    "drop".to_string()
}

/// Configuration for the `deliver` operator
///
/// Sends the processed event to one or more sinks.
///
/// ```yaml
/// # Single sink
/// - deliver:
///     sink: push-notification
///
/// # Multiple sinks
/// - deliver:
///     sinks: [push-notification, in-app-notification, websocket]
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeliverConfig {
    /// Single sink name (mutually exclusive with `sinks`)
    #[serde(default)]
    pub sink: Option<String>,

    /// Multiple sink names (mutually exclusive with `sink`)
    #[serde(default)]
    pub sinks: Option<Vec<String>>,
}

impl DeliverConfig {
    /// Get all sink names this deliver step targets
    ///
    /// If both `sink` and `sinks` are present, they are merged (with a warning).
    pub fn sink_names(&self) -> Vec<&str> {
        let mut names = Vec::new();

        // Include the singular `sink` if present
        if let Some(sink) = &self.sink {
            names.push(sink.as_str());
        }

        // Include all `sinks` if present
        if let Some(sinks) = &self.sinks {
            for s in sinks {
                let name = s.as_str();
                if !names.contains(&name) {
                    names.push(name);
                }
            }
        }

        if self.sink.is_some() && self.sinks.is_some() {
            tracing::warn!(
                "deliver: both 'sink' and 'sinks' are defined — merging them. \
                 Prefer using only 'sinks' for clarity."
            );
        }

        names
    }
}

/// Consumer group configuration
///
/// ```yaml
/// consumers:
///   - name: mobile-feed
///     seek: last_acknowledged
///   - name: web-dashboard
///     seek: latest
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConsumerConfig {
    /// Unique consumer group name
    pub name: String,

    /// Initial seek position
    #[serde(default)]
    pub seek: SeekMode,
}

/// Seek mode for consumers — determines where to start reading from
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
pub enum SeekMode {
    /// Start from the very beginning (replay all events)
    Beginning,
    /// Resume from the last acknowledged position
    LastAcknowledged,
    /// Start from now (only receive future events)
    #[default]
    Latest,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_events_config_full_yaml() {
        let yaml = r#"
backend:
  type: memory
  config:
    retention: 7d
flows:
  - name: notify-new-follower
    description: "Notify user when someone follows them"
    trigger:
      kind: link.created
      link_type: follows
    pipeline:
      - resolve:
          from: source_id
          as: follower
      - resolve:
          from: target_id
          as: recipient
      - map:
          template:
            type: follow
            recipient_id: "{{ recipient.id }}"
            message: "{{ follower.name }} started following you"
      - deliver:
          sinks: [push-notification, in-app-notification]
  - name: notify-like
    trigger:
      kind: link.created
      link_type: likes
    pipeline:
      - resolve:
          from: target_id
          via: owns
          direction: reverse
          as: owner
      - filter:
          condition: "source_id != owner.id"
      - batch:
          key: target_id
          window: 5m
      - deduplicate:
          key: source_id
          window: 1h
      - map:
          template:
            type: like
            recipient_id: "{{ owner.id }}"
            message: "{{ batch.count }} people liked your trace"
      - deliver:
          sink: push-notification
consumers:
  - name: mobile-feed
    seek: last_acknowledged
  - name: web-dashboard
    seek: latest
"#;

        let config: EventsConfig = serde_yaml::from_str(yaml).unwrap();

        // Backend
        assert_eq!(config.backend.backend_type, "memory");
        assert_eq!(
            config.backend.config.get("retention").unwrap(),
            &serde_json::Value::String("7d".to_string())
        );

        // Flows
        assert_eq!(config.flows.len(), 2);
        assert_eq!(config.flows[0].name, "notify-new-follower");
        assert_eq!(
            config.flows[0].description.as_deref(),
            Some("Notify user when someone follows them")
        );
        assert_eq!(config.flows[0].trigger.kind, "link.created");
        assert_eq!(
            config.flows[0].trigger.link_type.as_deref(),
            Some("follows")
        );
        assert_eq!(config.flows[0].pipeline.len(), 4);

        // Check pipeline operators
        assert!(
            matches!(&config.flows[0].pipeline[0], PipelineStep::Resolve(r) if r.from == "source_id")
        );
        assert!(
            matches!(&config.flows[0].pipeline[1], PipelineStep::Resolve(r) if r.from == "target_id")
        );
        assert!(matches!(&config.flows[0].pipeline[2], PipelineStep::Map(_)));
        assert!(
            matches!(&config.flows[0].pipeline[3], PipelineStep::Deliver(d) if d.sink_names().len() == 2)
        );

        // Second flow with advanced operators
        assert_eq!(config.flows[1].name, "notify-like");
        assert_eq!(config.flows[1].pipeline.len(), 6);
        assert!(
            matches!(&config.flows[1].pipeline[0], PipelineStep::Resolve(r) if r.via.as_deref() == Some("owns"))
        );
        assert!(
            matches!(&config.flows[1].pipeline[1], PipelineStep::Filter(f) if f.condition == "source_id != owner.id")
        );
        assert!(matches!(&config.flows[1].pipeline[2], PipelineStep::Batch(b) if b.window == "5m"));
        assert!(
            matches!(&config.flows[1].pipeline[3], PipelineStep::Deduplicate(d) if d.window == "1h")
        );
        assert!(
            matches!(&config.flows[1].pipeline[5], PipelineStep::Deliver(d) if d.sink.as_deref() == Some("push-notification"))
        );

        // Consumers
        assert_eq!(config.consumers.len(), 2);
        assert_eq!(config.consumers[0].name, "mobile-feed");
        assert_eq!(config.consumers[0].seek, SeekMode::LastAcknowledged);
        assert_eq!(config.consumers[1].name, "web-dashboard");
        assert_eq!(config.consumers[1].seek, SeekMode::Latest);
    }

    #[test]
    fn test_events_config_minimal() {
        let yaml = r#"
flows: []
"#;
        let config: EventsConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(config.backend.backend_type, "memory");
        assert!(config.flows.is_empty());
        assert!(config.consumers.is_empty());
    }

    #[test]
    fn test_pipeline_step_serde_roundtrip() {
        let steps = vec![
            PipelineStep::Resolve(ResolveConfig {
                from: "source_id".to_string(),
                via: Some("follows".to_string()),
                direction: "reverse".to_string(),
                output_var: "follower".to_string(),
            }),
            PipelineStep::Filter(FilterConfig {
                condition: "source_id != owner.id".to_string(),
            }),
            PipelineStep::FanOut(FanOutConfig {
                from: "source_id".to_string(),
                via: "follows".to_string(),
                direction: "reverse".to_string(),
                output_var: "followers".to_string(),
            }),
            PipelineStep::Batch(BatchConfig {
                key: "target_id".to_string(),
                window: "5m".to_string(),
                min_count: 2,
            }),
            PipelineStep::Deduplicate(DeduplicateConfig {
                key: "source_id".to_string(),
                window: "1h".to_string(),
            }),
            PipelineStep::Map(MapConfig {
                template: serde_json::json!({
                    "type": "notification",
                    "message": "{{ follower.name }} followed you"
                }),
            }),
            PipelineStep::RateLimit(RateLimitConfig {
                max: 100,
                per: "1m".to_string(),
                strategy: "drop".to_string(),
            }),
            PipelineStep::Deliver(DeliverConfig {
                sink: None,
                sinks: Some(vec![
                    "push-notification".to_string(),
                    "in-app-notification".to_string(),
                ]),
            }),
        ];

        for step in &steps {
            let yaml = serde_yaml::to_string(step).unwrap();
            let roundtrip: PipelineStep = serde_yaml::from_str(&yaml).unwrap();
            // Verify the variant matches
            assert_eq!(
                std::mem::discriminant(step),
                std::mem::discriminant(&roundtrip)
            );
        }
    }

    #[test]
    fn test_seek_mode_variants() {
        let yaml_beginning = "\"beginning\"";
        let yaml_last = "\"last_acknowledged\"";
        let yaml_latest = "\"latest\"";

        assert_eq!(
            serde_json::from_str::<SeekMode>(yaml_beginning).unwrap(),
            SeekMode::Beginning
        );
        assert_eq!(
            serde_json::from_str::<SeekMode>(yaml_last).unwrap(),
            SeekMode::LastAcknowledged
        );
        assert_eq!(
            serde_json::from_str::<SeekMode>(yaml_latest).unwrap(),
            SeekMode::Latest
        );
    }

    #[test]
    fn test_deliver_config_single_sink() {
        let config = DeliverConfig {
            sink: Some("push".to_string()),
            sinks: None,
        };
        assert_eq!(config.sink_names(), vec!["push"]);
    }

    #[test]
    fn test_deliver_config_multiple_sinks() {
        let config = DeliverConfig {
            sink: None,
            sinks: Some(vec!["push".to_string(), "in-app".to_string()]),
        };
        assert_eq!(config.sink_names(), vec!["push", "in-app"]);
    }

    #[test]
    fn test_deliver_config_empty() {
        let config = DeliverConfig {
            sink: None,
            sinks: None,
        };
        assert!(config.sink_names().is_empty());
    }

    #[test]
    fn test_deliver_config_both_sink_and_sinks_merged() {
        let config = DeliverConfig {
            sink: Some("push".to_string()),
            sinks: Some(vec!["in-app".to_string(), "websocket".to_string()]),
        };
        let names = config.sink_names();
        assert_eq!(names.len(), 3);
        assert!(names.contains(&"push"));
        assert!(names.contains(&"in-app"));
        assert!(names.contains(&"websocket"));
    }

    #[test]
    fn test_deliver_config_both_with_duplicate_deduped() {
        let config = DeliverConfig {
            sink: Some("push".to_string()),
            sinks: Some(vec!["push".to_string(), "in-app".to_string()]),
        };
        let names = config.sink_names();
        // "push" should appear only once
        assert_eq!(names.len(), 2);
        assert_eq!(names, vec!["push", "in-app"]);
    }

    #[test]
    fn test_resolve_direction_defaults() {
        let yaml = r#"
from: target_id
as: owner
"#;
        let config: ResolveConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(config.direction, "forward");
        assert!(config.via.is_none());
    }

    #[test]
    fn test_rate_limit_strategy_default() {
        let yaml = r#"
max: 50
per: 1s
"#;
        let config: RateLimitConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(config.strategy, "drop");
    }

    #[test]
    fn test_batch_min_count_default() {
        let yaml = r#"
key: target_id
window: 5m
"#;
        let config: BatchConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(config.min_count, 1);
    }

    #[test]
    fn test_flow_with_fan_out_pipeline() {
        let yaml = r#"
name: feed-update
trigger:
  kind: link.created
  link_type: owns
pipeline:
  - resolve:
      from: source_id
      as: creator
  - fan_out:
      from: source_id
      via: follows
      direction: reverse
      as: follower
  - map:
      template:
        type: feed_update
        recipient_id: "{{ follower.id }}"
        message: "{{ creator.name }} posted a new trace"
  - deliver:
      sinks: [in-app-notification, websocket]
"#;

        let flow: FlowConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(flow.name, "feed-update");
        assert_eq!(flow.pipeline.len(), 4);
        assert!(
            matches!(&flow.pipeline[1], PipelineStep::FanOut(f) if f.via == "follows" && f.direction == "reverse")
        );
    }

    #[test]
    fn test_trigger_entity_event() {
        let yaml = r#"
kind: entity.created
entity_type: user
"#;
        let trigger: TriggerConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(trigger.kind, "entity.created");
        assert_eq!(trigger.entity_type.as_deref(), Some("user"));
        assert!(trigger.link_type.is_none());
    }

    #[test]
    fn test_trigger_wildcard() {
        let yaml = r#"
kind: link.created
"#;
        let trigger: TriggerConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(trigger.kind, "link.created");
        assert!(trigger.link_type.is_none());
        assert!(trigger.entity_type.is_none());
    }
}
