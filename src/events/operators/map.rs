//! Map operator — transforms the payload via Tera templates
//!
//! The map operator takes a JSON template where string values can contain
//! Tera expressions (e.g., `{{ owner.name }}`). At execution time, the
//! FlowContext variables are injected into the Tera context and each
//! string value is rendered as a template.
//!
//! The rendered result is stored as the `_payload` variable in the context,
//! which is then used by the `deliver` operator to send to sinks.
//!
//! ```yaml
//! - map:
//!     template:
//!       title: "{{ owner.name }} started following you"
//!       body: "You have a new follower!"
//!       icon: "follow"
//!       data:
//!         follower_id: "{{ source_id }}"
//! ```

use crate::config::events::MapConfig;
use crate::events::context::FlowContext;
use crate::events::operators::{OpResult, PipelineOperator};
use anyhow::{Result, anyhow};
use async_trait::async_trait;
use serde_json::Value;

/// Compiled map operator
#[derive(Debug, Clone)]
pub struct MapOp {
    /// The JSON template to render
    template: Value,
}

impl MapOp {
    /// Create a MapOp from a MapConfig
    pub fn from_config(config: &MapConfig) -> Self {
        Self {
            template: config.template.clone(),
        }
    }
}

#[async_trait]
impl PipelineOperator for MapOp {
    async fn execute(&self, ctx: &mut FlowContext) -> Result<OpResult> {
        let tera_ctx = build_tera_context(ctx);
        let rendered = render_value(&self.template, &tera_ctx)?;
        ctx.set_var("_payload", rendered);
        Ok(OpResult::Continue)
    }

    fn name(&self) -> &str {
        "map"
    }
}

/// Build a Tera context from FlowContext variables
fn build_tera_context(ctx: &FlowContext) -> tera::Context {
    let mut tera_ctx = tera::Context::new();
    for (key, value) in &ctx.variables {
        tera_ctx.insert(key, value);
    }
    tera_ctx
}

/// Recursively render a JSON value, treating string values as Tera templates
fn render_value(template: &Value, tera_ctx: &tera::Context) -> Result<Value> {
    match template {
        Value::String(s) => {
            // Render the string as a Tera template
            let rendered = tera::Tera::one_off(s, tera_ctx, false)
                .map_err(|e| anyhow!("map: template rendering failed for '{}': {}", s, e))?;
            Ok(Value::String(rendered))
        }
        Value::Object(map) => {
            // Recursively render each value in the object
            let mut result = serde_json::Map::new();
            for (key, value) in map {
                result.insert(key.clone(), render_value(value, tera_ctx)?);
            }
            Ok(Value::Object(result))
        }
        Value::Array(arr) => {
            // Recursively render each element in the array
            let result: Result<Vec<Value>> =
                arr.iter().map(|v| render_value(v, tera_ctx)).collect();
            Ok(Value::Array(result?))
        }
        // Numbers, booleans, null — pass through unchanged
        other => Ok(other.clone()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::events::MapConfig;
    use crate::core::events::{EntityEvent, FrameworkEvent, LinkEvent};
    use crate::core::service::LinkService;
    use serde_json::json;
    use std::collections::HashMap;
    use std::sync::Arc;
    use uuid::Uuid;

    // ── Mock LinkService ─────────────────────────────────────────────

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

    fn mock_link_service() -> Arc<dyn LinkService> {
        Arc::new(MockLinkService)
    }

    fn make_link_context() -> FlowContext {
        let source_id = Uuid::new_v4();
        let target_id = Uuid::new_v4();
        let event = FrameworkEvent::Link(LinkEvent::Created {
            link_type: "follows".to_string(),
            link_id: Uuid::new_v4(),
            source_id,
            target_id,
            metadata: None,
        });
        FlowContext::new(event, mock_link_service(), HashMap::new())
    }

    fn make_entity_context() -> FlowContext {
        let event = FrameworkEvent::Entity(EntityEvent::Created {
            entity_type: "user".to_string(),
            entity_id: Uuid::new_v4(),
            data: json!({"name": "Alice"}),
        });
        FlowContext::new(event, mock_link_service(), HashMap::new())
    }

    // ── Tests ────────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_map_simple_string() {
        let mut ctx = make_entity_context();
        ctx.set_var("owner", json!({"name": "Alice"}));

        let op = MapOp::from_config(&MapConfig {
            template: json!({
                "title": "Hello {{ owner.name }}!",
                "body": "Welcome"
            }),
        });

        let result = op.execute(&mut ctx).await.unwrap();
        assert!(matches!(result, OpResult::Continue));

        let payload = ctx.get_var("_payload").unwrap();
        assert_eq!(payload["title"], "Hello Alice!");
        assert_eq!(payload["body"], "Welcome");
    }

    #[tokio::test]
    async fn test_map_with_context_variables() {
        let mut ctx = make_link_context();
        ctx.set_var("owner", json!({"name": "Alice", "id": "abc-123"}));
        ctx.set_var("follower", json!({"name": "Bob", "id": "def-456"}));

        let op = MapOp::from_config(&MapConfig {
            template: json!({
                "title": "{{ follower.name }} started following {{ owner.name }}",
                "icon": "follow",
                "data": {
                    "follower_id": "{{ follower.id }}",
                    "owner_id": "{{ owner.id }}"
                }
            }),
        });

        let result = op.execute(&mut ctx).await.unwrap();
        assert!(matches!(result, OpResult::Continue));

        let payload = ctx.get_var("_payload").unwrap();
        assert_eq!(payload["title"], "Bob started following Alice");
        assert_eq!(payload["icon"], "follow");
        assert_eq!(payload["data"]["follower_id"], "def-456");
        assert_eq!(payload["data"]["owner_id"], "abc-123");
    }

    #[tokio::test]
    async fn test_map_preserves_non_string_values() {
        let mut ctx = make_entity_context();

        let op = MapOp::from_config(&MapConfig {
            template: json!({
                "count": 42,
                "active": true,
                "tags": ["a", "b"],
                "title": "Static title"
            }),
        });

        let result = op.execute(&mut ctx).await.unwrap();
        assert!(matches!(result, OpResult::Continue));

        let payload = ctx.get_var("_payload").unwrap();
        assert_eq!(payload["count"], 42);
        assert_eq!(payload["active"], true);
        assert_eq!(payload["tags"][0], "a");
        assert_eq!(payload["title"], "Static title");
    }

    #[tokio::test]
    async fn test_map_with_tera_conditionals() {
        let mut ctx = make_entity_context();
        ctx.set_var("owner", json!({"name": "Alice", "vip": true}));

        let op = MapOp::from_config(&MapConfig {
            template: json!({
                "title": "{% if owner.vip %}VIP: {% endif %}{{ owner.name }}"
            }),
        });

        let result = op.execute(&mut ctx).await.unwrap();
        assert!(matches!(result, OpResult::Continue));

        let payload = ctx.get_var("_payload").unwrap();
        assert_eq!(payload["title"], "VIP: Alice");
    }

    #[tokio::test]
    async fn test_map_with_array_template() {
        let mut ctx = make_entity_context();
        ctx.set_var("user", json!({"name": "Alice"}));

        let op = MapOp::from_config(&MapConfig {
            template: json!(["Hello {{ user.name }}", "static"]),
        });

        let result = op.execute(&mut ctx).await.unwrap();
        assert!(matches!(result, OpResult::Continue));

        let payload = ctx.get_var("_payload").unwrap();
        assert_eq!(payload[0], "Hello Alice");
        assert_eq!(payload[1], "static");
    }

    #[tokio::test]
    async fn test_map_invalid_template() {
        let mut ctx = make_entity_context();

        let op = MapOp::from_config(&MapConfig {
            template: json!({
                "title": "{{ unclosed"
            }),
        });

        let result = op.execute(&mut ctx).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_map_uses_event_variables() {
        let mut ctx = make_entity_context();

        // entity_type and entity_id are auto-extracted by FlowContext
        let op = MapOp::from_config(&MapConfig {
            template: json!({
                "message": "New {{ entity_type }} created"
            }),
        });

        let result = op.execute(&mut ctx).await.unwrap();
        assert!(matches!(result, OpResult::Continue));

        let payload = ctx.get_var("_payload").unwrap();
        assert_eq!(payload["message"], "New user created");
    }
}
