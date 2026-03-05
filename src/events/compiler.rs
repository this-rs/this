//! Flow compiler — compiles FlowConfig YAML into executable CompiledFlow
//!
//! The compiler transforms declarative flow configurations into a list of
//! executable pipeline operators. Each CompiledFlow contains:
//! - An `EventMatcher` for deciding which events trigger the flow
//! - A `Vec<Box<dyn PipelineOperator>>` for the pipeline steps
//!
//! # Usage
//!
//! ```ignore
//! let config = FlowConfig { ... };
//! let compiled = compile_flow(&config)?;
//! // compiled.matcher.matches(&event) → true/false
//! // for op in &compiled.operators { op.execute(&mut ctx).await? }
//! ```

use crate::config::events::{FlowConfig, PipelineStep};
use crate::events::matcher::EventMatcher;
use crate::events::operators::*;
use anyhow::{Context, Result};

/// A compiled flow ready for execution
#[derive(Debug)]
pub struct CompiledFlow {
    /// Flow name (from config)
    pub name: String,

    /// Compiled event matcher (from trigger config)
    pub matcher: EventMatcher,

    /// Compiled pipeline operators (in execution order)
    pub operators: Vec<Box<dyn PipelineOperator>>,
}

/// Compile a FlowConfig into a CompiledFlow
///
/// Validates the configuration and creates executable operator instances.
///
/// # Errors
///
/// Returns an error if:
/// - The trigger has an unknown event kind
/// - A filter condition cannot be parsed
/// - A deliver step references no sinks
/// - A duration string cannot be parsed
pub fn compile_flow(config: &FlowConfig) -> Result<CompiledFlow> {
    let matcher = EventMatcher::compile(&config.trigger)
        .with_context(|| format!("flow '{}': invalid trigger", config.name))?;

    let mut operators: Vec<Box<dyn PipelineOperator>> = Vec::new();

    for (i, step) in config.pipeline.iter().enumerate() {
        let op: Box<dyn PipelineOperator> = compile_step(step)
            .with_context(|| format!("flow '{}': step {} failed to compile", config.name, i))?;
        operators.push(op);
    }

    Ok(CompiledFlow {
        name: config.name.clone(),
        matcher,
        operators,
    })
}

/// Compile a single PipelineStep into a PipelineOperator
fn compile_step(step: &PipelineStep) -> Result<Box<dyn PipelineOperator>> {
    match step {
        PipelineStep::Resolve(config) => Ok(Box::new(ResolveOp::from_config(config))),
        PipelineStep::Filter(config) => {
            Ok(Box::new(FilterOp::from_config(config)?))
        }
        PipelineStep::FanOut(config) => Ok(Box::new(FanOutOp::from_config(config))),
        PipelineStep::Batch(config) => Ok(Box::new(BatchOp::from_config(config)?)),
        PipelineStep::Deduplicate(config) => {
            Ok(Box::new(DeduplicateOp::from_config(config)?))
        }
        PipelineStep::Map(config) => Ok(Box::new(MapOp::from_config(config))),
        PipelineStep::RateLimit(config) => {
            Ok(Box::new(RateLimitOp::from_config(config)?))
        }
        PipelineStep::Deliver(config) => Ok(Box::new(DeliverOp::from_config(config)?)),
    }
}

/// Compile multiple flows from a list of FlowConfigs
///
/// Returns all successfully compiled flows and logs errors for any that fail.
pub fn compile_flows(configs: &[FlowConfig]) -> Result<Vec<CompiledFlow>> {
    let mut compiled = Vec::new();
    let mut errors = Vec::new();

    for config in configs {
        match compile_flow(config) {
            Ok(flow) => compiled.push(flow),
            Err(e) => {
                tracing::error!(flow = %config.name, error = %e, "failed to compile flow");
                errors.push(format!("{}: {}", config.name, e));
            }
        }
    }

    if !errors.is_empty() && compiled.is_empty() {
        anyhow::bail!("all flows failed to compile: {}", errors.join("; "));
    }

    Ok(compiled)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::events::*;
    use serde_json::json;

    fn make_flow(name: &str, kind: &str, steps: Vec<PipelineStep>) -> FlowConfig {
        FlowConfig {
            name: name.to_string(),
            description: None,
            trigger: TriggerConfig {
                kind: kind.to_string(),
                link_type: None,
                entity_type: None,
            },
            pipeline: steps,
        }
    }

    #[test]
    fn test_compile_empty_pipeline() {
        let flow = make_flow("test", "link.created", vec![]);
        let compiled = compile_flow(&flow).unwrap();
        assert_eq!(compiled.name, "test");
        assert!(compiled.operators.is_empty());
    }

    #[test]
    fn test_compile_full_pipeline() {
        let flow = make_flow(
            "follow_notification",
            "link.created",
            vec![
                PipelineStep::Resolve(ResolveConfig {
                    from: "source_id".to_string(),
                    via: None,
                    direction: "forward".to_string(),
                    output_var: "source".to_string(),
                }),
                PipelineStep::Filter(FilterConfig {
                    condition: "source_id != target_id".to_string(),
                }),
                PipelineStep::Map(MapConfig {
                    template: json!({
                        "title": "{{ source.name }} started following you"
                    }),
                }),
                PipelineStep::Deliver(DeliverConfig {
                    sink: Some("in_app".to_string()),
                    sinks: None,
                }),
            ],
        );

        let compiled = compile_flow(&flow).unwrap();
        assert_eq!(compiled.name, "follow_notification");
        assert_eq!(compiled.operators.len(), 4);
        assert_eq!(compiled.operators[0].name(), "resolve");
        assert_eq!(compiled.operators[1].name(), "filter");
        assert_eq!(compiled.operators[2].name(), "map");
        assert_eq!(compiled.operators[3].name(), "deliver");
    }

    #[test]
    fn test_compile_with_stateful_operators() {
        let flow = make_flow(
            "like_batch",
            "link.created",
            vec![
                PipelineStep::Deduplicate(DeduplicateConfig {
                    key: "source_id".to_string(),
                    window: "1h".to_string(),
                }),
                PipelineStep::Batch(BatchConfig {
                    key: "target_id".to_string(),
                    window: "5m".to_string(),
                    min_count: 1,
                }),
                PipelineStep::RateLimit(RateLimitConfig {
                    max: 100,
                    per: "1m".to_string(),
                    strategy: "drop".to_string(),
                }),
                PipelineStep::Map(MapConfig {
                    template: json!({"title": "batch"}),
                }),
                PipelineStep::Deliver(DeliverConfig {
                    sink: Some("push".to_string()),
                    sinks: None,
                }),
            ],
        );

        let compiled = compile_flow(&flow).unwrap();
        assert_eq!(compiled.operators.len(), 5);
        assert_eq!(compiled.operators[0].name(), "deduplicate");
        assert_eq!(compiled.operators[1].name(), "batch");
        assert_eq!(compiled.operators[2].name(), "rate_limit");
    }

    #[test]
    fn test_compile_with_fan_out() {
        let flow = make_flow(
            "broadcast",
            "entity.created",
            vec![
                PipelineStep::FanOut(FanOutConfig {
                    from: "entity_id".to_string(),
                    via: "follows".to_string(),
                    direction: "reverse".to_string(),
                    output_var: "follower".to_string(),
                }),
                PipelineStep::Map(MapConfig {
                    template: json!({"title": "new content"}),
                }),
                PipelineStep::Deliver(DeliverConfig {
                    sink: Some("in_app".to_string()),
                    sinks: None,
                }),
            ],
        );

        let compiled = compile_flow(&flow).unwrap();
        assert_eq!(compiled.operators.len(), 3);
        assert_eq!(compiled.operators[0].name(), "fan_out");
    }

    #[test]
    fn test_compile_invalid_trigger() {
        let flow = make_flow("bad", "invalid.kind", vec![]);
        let result = compile_flow(&flow);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("invalid trigger"));
    }

    #[test]
    fn test_compile_invalid_filter_condition() {
        let flow = make_flow(
            "bad_filter",
            "link.created",
            vec![PipelineStep::Filter(FilterConfig {
                condition: "no operator here".to_string(),
            })],
        );

        let result = compile_flow(&flow);
        assert!(result.is_err());
    }

    #[test]
    fn test_compile_deliver_no_sink() {
        let flow = make_flow(
            "bad_deliver",
            "link.created",
            vec![PipelineStep::Deliver(DeliverConfig {
                sink: None,
                sinks: None,
            })],
        );

        let result = compile_flow(&flow);
        assert!(result.is_err());
    }

    #[test]
    fn test_compile_invalid_duration() {
        let flow = make_flow(
            "bad_batch",
            "link.created",
            vec![PipelineStep::Batch(BatchConfig {
                key: "target_id".to_string(),
                window: "invalid".to_string(),
                min_count: 1,
            })],
        );

        let result = compile_flow(&flow);
        assert!(result.is_err());
    }

    #[test]
    fn test_compile_flows_partial_failure() {
        let good = make_flow("good", "link.created", vec![]);
        let bad = make_flow("bad", "invalid.kind", vec![]);

        let compiled = compile_flows(&[good, bad]).unwrap();
        assert_eq!(compiled.len(), 1);
        assert_eq!(compiled[0].name, "good");
    }

    #[test]
    fn test_compile_flows_all_fail() {
        let bad1 = make_flow("bad1", "invalid.kind", vec![]);
        let bad2 = make_flow("bad2", "also.invalid", vec![]);

        let result = compile_flows(&[bad1, bad2]);
        assert!(result.is_err());
    }
}
