//! Filter operator — evaluates a boolean condition against the FlowContext
//!
//! Drops events that don't match the condition. Supports simple expressions:
//!
//! - `field == "value"` — equality
//! - `field != "value"` — inequality
//! - `field exists` — field is present in context
//! - `field not_exists` — field is absent from context
//!
//! Fields support dotted access (e.g., `owner.name == "Alice"`).
//!
//! ```yaml
//! - filter:
//!     condition: "source_id != target_id"
//! ```

use crate::config::events::FilterConfig;
use crate::events::context::FlowContext;
use crate::events::operators::{OpResult, PipelineOperator};
use anyhow::{anyhow, Result};
use async_trait::async_trait;
use serde_json::Value;

/// Supported comparison operators
#[derive(Debug, Clone, PartialEq)]
enum CompareOp {
    /// `==` — equality
    Equal,
    /// `!=` — inequality
    NotEqual,
    /// `exists` — field is present
    Exists,
    /// `not_exists` — field is absent
    NotExists,
}

/// Right-hand side of a comparison: literal value or variable reference
#[derive(Debug, Clone)]
enum FilterValue {
    /// A literal value (quoted string, number, boolean, null)
    Literal(String),
    /// A variable reference (unquoted identifier, resolved from context)
    Variable(String),
}

/// Parsed filter expression
#[derive(Debug, Clone)]
struct FilterExpr {
    /// Left-hand side: variable name (supports dotted access)
    field: String,
    /// Comparison operator
    op: CompareOp,
    /// Right-hand side value (None for exists/not_exists)
    value: Option<FilterValue>,
}

/// Compiled filter operator
#[derive(Debug, Clone)]
pub struct FilterOp {
    /// The parsed expression to evaluate
    expr: FilterExpr,
    /// Original condition string for error messages
    condition: String,
}

impl FilterOp {
    /// Create a FilterOp from a FilterConfig
    ///
    /// Parses the condition string into a structured expression.
    pub fn from_config(config: &FilterConfig) -> Result<Self> {
        let expr = parse_condition(&config.condition)?;
        Ok(Self {
            expr,
            condition: config.condition.clone(),
        })
    }
}

#[async_trait]
impl PipelineOperator for FilterOp {
    async fn execute(&self, ctx: &mut FlowContext) -> Result<OpResult> {
        let result = evaluate(&self.expr, ctx);
        if result {
            Ok(OpResult::Continue)
        } else {
            Ok(OpResult::Drop)
        }
    }

    fn name(&self) -> &str {
        "filter"
    }
}

/// Parse a condition string into a FilterExpr
fn parse_condition(condition: &str) -> Result<FilterExpr> {
    let condition = condition.trim();

    // Try `field not_exists` (must be before `!=` check)
    if let Some(field) = condition.strip_suffix(" not_exists") {
        return Ok(FilterExpr {
            field: field.trim().to_string(),
            op: CompareOp::NotExists,
            value: None,
        });
    }

    // Try `field exists`
    if let Some(field) = condition.strip_suffix(" exists") {
        return Ok(FilterExpr {
            field: field.trim().to_string(),
            op: CompareOp::Exists,
            value: None,
        });
    }

    // Try `field != value`
    if let Some((left, right)) = condition.split_once(" != ") {
        return Ok(FilterExpr {
            field: left.trim().to_string(),
            op: CompareOp::NotEqual,
            value: Some(parse_rhs(right.trim())),
        });
    }

    // Try `field == value`
    if let Some((left, right)) = condition.split_once(" == ") {
        return Ok(FilterExpr {
            field: left.trim().to_string(),
            op: CompareOp::Equal,
            value: Some(parse_rhs(right.trim())),
        });
    }

    Err(anyhow!(
        "filter: cannot parse condition '{}'. Expected: 'field == value', 'field != value', 'field exists', or 'field not_exists'",
        condition
    ))
}

/// Remove surrounding quotes from a string value
fn unquote(s: &str) -> String {
    if (s.starts_with('"') && s.ends_with('"')) || (s.starts_with('\'') && s.ends_with('\'')) {
        s[1..s.len() - 1].to_string()
    } else {
        s.to_string()
    }
}

/// Check if a string represents a literal value (quoted, number, boolean, null)
/// rather than a variable reference.
fn is_literal_value(s: &str) -> bool {
    // Quoted strings
    if (s.starts_with('"') && s.ends_with('"'))
        || (s.starts_with('\'') && s.ends_with('\''))
    {
        return true;
    }
    // Numbers (int or float)
    if s.parse::<f64>().is_ok() {
        return true;
    }
    // Booleans
    if s == "true" || s == "false" {
        return true;
    }
    // Null
    if s == "null" {
        return true;
    }
    false
}

/// Parse the right-hand side of a comparison into a FilterValue
fn parse_rhs(s: &str) -> FilterValue {
    if is_literal_value(s) {
        FilterValue::Literal(unquote(s))
    } else {
        FilterValue::Variable(s.to_string())
    }
}

/// Resolve a FilterValue to a string for comparison
fn resolve_filter_value(value: &FilterValue, ctx: &FlowContext) -> Option<String> {
    match value {
        FilterValue::Literal(s) => Some(s.clone()),
        FilterValue::Variable(var_name) => ctx.get_var(var_name).map(value_to_string),
    }
}

/// Convert a JSON Value to its string representation for comparison
fn value_to_string(val: &Value) -> String {
    match val {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        Value::Bool(b) => b.to_string(),
        Value::Null => "null".to_string(),
        _ => val.to_string(),
    }
}

/// Evaluate a filter expression against the context
fn evaluate(expr: &FilterExpr, ctx: &FlowContext) -> bool {
    let var = ctx.get_var(&expr.field);

    match expr.op {
        CompareOp::Exists => var.is_some(),
        CompareOp::NotExists => var.is_none(),
        CompareOp::Equal => match (var, &expr.value) {
            (Some(val), Some(filter_val)) => {
                match resolve_filter_value(filter_val, ctx) {
                    Some(resolved) => value_matches(val, &resolved),
                    None => false, // RHS variable not found → not equal
                }
            }
            _ => false,
        },
        CompareOp::NotEqual => match (var, &expr.value) {
            (Some(val), Some(filter_val)) => {
                match resolve_filter_value(filter_val, ctx) {
                    Some(resolved) => !value_matches(val, &resolved),
                    None => true, // RHS variable not found → not equal
                }
            }
            (None, _) => true, // Missing field != anything is true
            _ => true,
        },
    }
}

/// Compare a JSON value against a string representation
fn value_matches(val: &Value, expected: &str) -> bool {
    match val {
        Value::String(s) => s == expected,
        Value::Number(n) => n.to_string() == expected,
        Value::Bool(b) => b.to_string() == expected,
        Value::Null => expected == "null",
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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

    fn make_link_context(source_id: Uuid, target_id: Uuid) -> FlowContext {
        let event = FrameworkEvent::Link(LinkEvent::Created {
            link_type: "follows".to_string(),
            link_id: Uuid::new_v4(),
            source_id,
            target_id,
            metadata: None,
        });
        FlowContext::new(event, mock_link_service(), HashMap::new())
    }

    fn make_entity_context(entity_type: &str) -> FlowContext {
        let event = FrameworkEvent::Entity(EntityEvent::Created {
            entity_type: entity_type.to_string(),
            entity_id: Uuid::new_v4(),
            data: json!({"name": "test"}),
        });
        FlowContext::new(event, mock_link_service(), HashMap::new())
    }

    // ── Tests: equality ──────────────────────────────────────────────

    #[tokio::test]
    async fn test_filter_equal_pass() {
        let mut ctx = make_entity_context("user");
        let op = FilterOp::from_config(&FilterConfig {
            condition: "entity_type == \"user\"".to_string(),
        })
        .unwrap();

        let result = op.execute(&mut ctx).await.unwrap();
        assert!(matches!(result, OpResult::Continue));
    }

    #[tokio::test]
    async fn test_filter_equal_drop() {
        let mut ctx = make_entity_context("user");
        let op = FilterOp::from_config(&FilterConfig {
            condition: "entity_type == \"post\"".to_string(),
        })
        .unwrap();

        let result = op.execute(&mut ctx).await.unwrap();
        assert!(matches!(result, OpResult::Drop));
    }

    // ── Tests: inequality ────────────────────────────────────────────

    #[tokio::test]
    async fn test_filter_not_equal_pass() {
        let source_id = Uuid::new_v4();
        let target_id = Uuid::new_v4();
        let mut ctx = make_link_context(source_id, target_id);

        let op = FilterOp::from_config(&FilterConfig {
            condition: "source_id != target_id".to_string(),
        })
        .unwrap();

        // source_id != target_id evaluates by comparing the string values
        // Since they're different UUIDs, this should pass
        let result = op.execute(&mut ctx).await.unwrap();
        assert!(matches!(result, OpResult::Continue));
    }

    #[tokio::test]
    async fn test_filter_not_equal_drop() {
        let mut ctx = make_entity_context("user");
        let op = FilterOp::from_config(&FilterConfig {
            condition: "entity_type != \"user\"".to_string(),
        })
        .unwrap();

        let result = op.execute(&mut ctx).await.unwrap();
        assert!(matches!(result, OpResult::Drop));
    }

    // ── Tests: exists / not_exists ───────────────────────────────────

    #[tokio::test]
    async fn test_filter_exists_pass() {
        let mut ctx = make_entity_context("user");
        let op = FilterOp::from_config(&FilterConfig {
            condition: "entity_type exists".to_string(),
        })
        .unwrap();

        let result = op.execute(&mut ctx).await.unwrap();
        assert!(matches!(result, OpResult::Continue));
    }

    #[tokio::test]
    async fn test_filter_exists_drop() {
        let mut ctx = make_entity_context("user");
        let op = FilterOp::from_config(&FilterConfig {
            condition: "nonexistent exists".to_string(),
        })
        .unwrap();

        let result = op.execute(&mut ctx).await.unwrap();
        assert!(matches!(result, OpResult::Drop));
    }

    #[tokio::test]
    async fn test_filter_not_exists_pass() {
        let mut ctx = make_entity_context("user");
        let op = FilterOp::from_config(&FilterConfig {
            condition: "nonexistent not_exists".to_string(),
        })
        .unwrap();

        let result = op.execute(&mut ctx).await.unwrap();
        assert!(matches!(result, OpResult::Continue));
    }

    #[tokio::test]
    async fn test_filter_not_exists_drop() {
        let mut ctx = make_entity_context("user");
        let op = FilterOp::from_config(&FilterConfig {
            condition: "entity_type not_exists".to_string(),
        })
        .unwrap();

        let result = op.execute(&mut ctx).await.unwrap();
        assert!(matches!(result, OpResult::Drop));
    }

    // ── Tests: dotted access ─────────────────────────────────────────

    #[tokio::test]
    async fn test_filter_dotted_access() {
        let mut ctx = make_entity_context("user");
        ctx.set_var("owner", json!({"name": "Alice", "role": "admin"}));

        let op = FilterOp::from_config(&FilterConfig {
            condition: "owner.role == \"admin\"".to_string(),
        })
        .unwrap();

        let result = op.execute(&mut ctx).await.unwrap();
        assert!(matches!(result, OpResult::Continue));
    }

    #[tokio::test]
    async fn test_filter_dotted_access_missing() {
        let mut ctx = make_entity_context("user");
        ctx.set_var("owner", json!({"name": "Alice"}));

        let op = FilterOp::from_config(&FilterConfig {
            condition: "owner.role exists".to_string(),
        })
        .unwrap();

        let result = op.execute(&mut ctx).await.unwrap();
        assert!(matches!(result, OpResult::Drop));
    }

    // ── Tests: parse errors ──────────────────────────────────────────

    #[test]
    fn test_filter_parse_error() {
        let result = FilterOp::from_config(&FilterConfig {
            condition: "invalid condition without operator".to_string(),
        });
        assert!(result.is_err());
    }

    // ── Tests: value types ───────────────────────────────────────────

    #[tokio::test]
    async fn test_filter_number_comparison() {
        let mut ctx = make_entity_context("user");
        ctx.set_var("count", json!(42));

        let op = FilterOp::from_config(&FilterConfig {
            condition: "count == 42".to_string(),
        })
        .unwrap();

        let result = op.execute(&mut ctx).await.unwrap();
        assert!(matches!(result, OpResult::Continue));
    }

    #[tokio::test]
    async fn test_filter_boolean_comparison() {
        let mut ctx = make_entity_context("user");
        ctx.set_var("active", json!(true));

        let op = FilterOp::from_config(&FilterConfig {
            condition: "active == true".to_string(),
        })
        .unwrap();

        let result = op.execute(&mut ctx).await.unwrap();
        assert!(matches!(result, OpResult::Continue));
    }

    // ── Tests: unquoted strings ──────────────────────────────────────

    #[tokio::test]
    async fn test_filter_single_quotes() {
        let mut ctx = make_entity_context("user");
        let op = FilterOp::from_config(&FilterConfig {
            condition: "entity_type == 'user'".to_string(),
        })
        .unwrap();

        let result = op.execute(&mut ctx).await.unwrap();
        assert!(matches!(result, OpResult::Continue));
    }

    // ── Tests: variable-to-variable comparison ──────────────────────

    #[tokio::test]
    async fn test_filter_var_to_var_not_equal_same_uuid_drops() {
        // source_id == target_id (same UUID) → source_id != target_id should Drop
        let same_id = Uuid::new_v4();
        let mut ctx = make_link_context(same_id, same_id);

        let op = FilterOp::from_config(&FilterConfig {
            condition: "source_id != target_id".to_string(),
        })
        .unwrap();

        let result = op.execute(&mut ctx).await.unwrap();
        assert!(
            matches!(result, OpResult::Drop),
            "self-link (source_id == target_id) should be dropped by != filter"
        );
    }

    #[tokio::test]
    async fn test_filter_var_to_var_equal_same_uuid_passes() {
        // source_id == target_id (same UUID) → source_id == target_id should Continue
        let same_id = Uuid::new_v4();
        let mut ctx = make_link_context(same_id, same_id);

        let op = FilterOp::from_config(&FilterConfig {
            condition: "source_id == target_id".to_string(),
        })
        .unwrap();

        let result = op.execute(&mut ctx).await.unwrap();
        assert!(matches!(result, OpResult::Continue));
    }

    #[tokio::test]
    async fn test_filter_var_to_var_equal_different_uuids_drops() {
        // source_id != target_id (different UUIDs) → source_id == target_id should Drop
        let mut ctx = make_link_context(Uuid::new_v4(), Uuid::new_v4());

        let op = FilterOp::from_config(&FilterConfig {
            condition: "source_id == target_id".to_string(),
        })
        .unwrap();

        let result = op.execute(&mut ctx).await.unwrap();
        assert!(matches!(result, OpResult::Drop));
    }

    #[tokio::test]
    async fn test_filter_quoted_stays_literal() {
        // Quoted "target_id" should NOT be resolved as a variable
        let mut ctx = make_link_context(Uuid::new_v4(), Uuid::new_v4());

        let op = FilterOp::from_config(&FilterConfig {
            condition: "source_id != \"target_id\"".to_string(),
        })
        .unwrap();

        // source_id is a UUID, "target_id" is the literal string → always not equal
        let result = op.execute(&mut ctx).await.unwrap();
        assert!(matches!(result, OpResult::Continue));
    }

    #[tokio::test]
    async fn test_filter_unknown_var_fallback() {
        // Unknown variable on RHS: source_id != unknown_var
        // unknown_var doesn't exist → resolve returns None → treated as not equal
        let mut ctx = make_link_context(Uuid::new_v4(), Uuid::new_v4());

        let op = FilterOp::from_config(&FilterConfig {
            condition: "source_id != unknown_var".to_string(),
        })
        .unwrap();

        let result = op.execute(&mut ctx).await.unwrap();
        assert!(
            matches!(result, OpResult::Continue),
            "comparison with unknown variable should be 'not equal'"
        );
    }
}
