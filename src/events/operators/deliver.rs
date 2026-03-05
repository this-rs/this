//! Deliver operator — sends the payload to configured sinks
//!
//! The deliver operator is the terminal step of a pipeline. It reads the
//! `_payload` variable from the FlowContext (set by the `map` operator)
//! and dispatches it to one or more sinks.
//!
//! ```yaml
//! - deliver:
//!     sink: in_app
//! # or
//! - deliver:
//!     sinks: [in_app, push]
//! ```
//!
//! # Sink Registry
//!
//! Actual sink implementations are registered at runtime via the
//! `SinkRegistry` (see Plan 3). The deliver operator looks up sinks
//! by name and calls their `send()` method.
//!
//! For now, this is a structural placeholder that validates the
//! configuration and records which sinks should be called.

use crate::config::events::DeliverConfig;
use crate::events::context::FlowContext;
use crate::events::operators::{OpResult, PipelineOperator};
use anyhow::{anyhow, Result};
use async_trait::async_trait;

/// Compiled deliver operator
#[derive(Debug, Clone)]
pub struct DeliverOp {
    /// Sink names to deliver to
    pub sink_names: Vec<String>,
}

impl DeliverOp {
    /// Create a DeliverOp from a DeliverConfig
    pub fn from_config(config: &DeliverConfig) -> Result<Self> {
        let names: Vec<String> = config.sink_names().iter().map(|s| s.to_string()).collect();
        if names.is_empty() {
            return Err(anyhow!(
                "deliver: at least one sink must be specified (use 'sink' or 'sinks')"
            ));
        }
        Ok(Self { sink_names: names })
    }
}

#[async_trait]
impl PipelineOperator for DeliverOp {
    async fn execute(&self, ctx: &mut FlowContext) -> Result<OpResult> {
        // Verify that _payload exists
        let _payload = ctx
            .get_var("_payload")
            .ok_or_else(|| {
                anyhow!("deliver: no '_payload' variable in context. Did you forget a 'map' step before 'deliver'?")
            })?
            .clone();

        // Record which sinks were targeted (for debugging/tracing)
        let sinks_json: serde_json::Value = self.sink_names.clone().into();
        ctx.set_var("_delivered_to", sinks_json);

        // TODO(Plan 3): Look up sinks in SinkRegistry and call send()
        // For now, we just log the delivery intent
        tracing::debug!(
            sinks = ?self.sink_names,
            "deliver: dispatching payload to sinks"
        );

        Ok(OpResult::Continue)
    }

    fn name(&self) -> &str {
        "deliver"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::events::DeliverConfig;
    use crate::core::events::{EntityEvent, FrameworkEvent};
    use crate::core::service::LinkService;
    use serde_json::json;
    use std::collections::HashMap;
    use std::sync::Arc;
    use uuid::Uuid;

    struct MockLinkService;

    #[async_trait]
    impl LinkService for MockLinkService {
        async fn create(
            &self,
            _link: crate::core::link::LinkEntity,
        ) -> Result<crate::core::link::LinkEntity> {
            unimplemented!()
        }
        async fn get(&self, _id: &Uuid) -> Result<Option<crate::core::link::LinkEntity>> {
            unimplemented!()
        }
        async fn list(&self) -> Result<Vec<crate::core::link::LinkEntity>> {
            unimplemented!()
        }
        async fn find_by_source(
            &self,
            _source_id: &Uuid,
            _link_type: Option<&str>,
            _target_type: Option<&str>,
        ) -> Result<Vec<crate::core::link::LinkEntity>> {
            unimplemented!()
        }
        async fn find_by_target(
            &self,
            _target_id: &Uuid,
            _link_type: Option<&str>,
            _source_type: Option<&str>,
        ) -> Result<Vec<crate::core::link::LinkEntity>> {
            unimplemented!()
        }
        async fn update(
            &self,
            _id: &Uuid,
            _link: crate::core::link::LinkEntity,
        ) -> Result<crate::core::link::LinkEntity> {
            unimplemented!()
        }
        async fn delete(&self, _id: &Uuid) -> Result<()> {
            unimplemented!()
        }
        async fn delete_by_entity(&self, _entity_id: &Uuid) -> Result<()> {
            unimplemented!()
        }
    }

    fn make_context() -> FlowContext {
        let event = FrameworkEvent::Entity(EntityEvent::Created {
            entity_type: "user".to_string(),
            entity_id: Uuid::new_v4(),
            data: json!({}),
        });
        FlowContext::new(
            event,
            Arc::new(MockLinkService) as Arc<dyn LinkService>,
            HashMap::new(),
        )
    }

    #[tokio::test]
    async fn test_deliver_single_sink() {
        let mut ctx = make_context();
        ctx.set_var("_payload", json!({"title": "Hello"}));

        let op = DeliverOp::from_config(&DeliverConfig {
            sink: Some("in_app".to_string()),
            sinks: None,
        })
        .unwrap();

        let result = op.execute(&mut ctx).await.unwrap();
        assert!(matches!(result, OpResult::Continue));
        assert_eq!(
            ctx.get_var("_delivered_to"),
            Some(&json!(["in_app"]))
        );
    }

    #[tokio::test]
    async fn test_deliver_multiple_sinks() {
        let mut ctx = make_context();
        ctx.set_var("_payload", json!({"title": "Hello"}));

        let op = DeliverOp::from_config(&DeliverConfig {
            sink: None,
            sinks: Some(vec!["in_app".to_string(), "push".to_string()]),
        })
        .unwrap();

        let result = op.execute(&mut ctx).await.unwrap();
        assert!(matches!(result, OpResult::Continue));
        assert_eq!(
            ctx.get_var("_delivered_to"),
            Some(&json!(["in_app", "push"]))
        );
    }

    #[tokio::test]
    async fn test_deliver_no_payload_error() {
        let mut ctx = make_context();
        // No _payload set

        let op = DeliverOp::from_config(&DeliverConfig {
            sink: Some("in_app".to_string()),
            sinks: None,
        })
        .unwrap();

        let result = op.execute(&mut ctx).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("_payload"));
    }

    #[test]
    fn test_deliver_no_sink_error() {
        let result = DeliverOp::from_config(&DeliverConfig {
            sink: None,
            sinks: None,
        });
        assert!(result.is_err());
    }

    #[test]
    fn test_deliver_empty_sinks_error() {
        let result = DeliverOp::from_config(&DeliverConfig {
            sink: None,
            sinks: Some(vec![]),
        });
        assert!(result.is_err());
    }
}
