//! FlowContext — the context bag passed through the pipeline
//!
//! The FlowContext carries the original event, resolved variables, and
//! access to services (LinkService, EntityFetchers) needed by operators.

use crate::core::events::FrameworkEvent;
use crate::core::module::EntityFetcher;
use crate::core::service::LinkService;
use crate::events::sinks::SinkRegistry;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;

/// Context passed through each operator in the pipeline
///
/// Accumulates variables as operators resolve entities and fan out.
/// Each operator can read/write variables via `get_var`/`set_var`.
///
/// # Variables
///
/// Variables are stored as `serde_json::Value` and named by the `as` field
/// of operators. For example, `resolve(from: source_id, as: follower)` stores
/// the resolved entity as `follower` in the context.
///
/// Special variables set from the trigger event:
/// - `source_id` — Source entity ID (for link events)
/// - `target_id` — Target entity ID (for link events)
/// - `link_type` — Link type (for link events)
/// - `entity_type` — Entity type (for entity events)
/// - `entity_id` — Entity ID (for entity events)
/// - `metadata` — Link metadata (for link events)
/// - `data` — Entity data (for entity events)
#[derive(Clone)]
pub struct FlowContext {
    /// The original framework event that triggered this flow
    pub event: FrameworkEvent,

    /// Accumulated variables from pipeline operators
    pub variables: HashMap<String, Value>,

    /// Access to the link service for resolve/fan_out operators
    pub link_service: Arc<dyn LinkService>,

    /// Access to entity fetchers, keyed by entity type
    pub entity_fetchers: HashMap<String, Arc<dyn EntityFetcher>>,

    /// Access to the sink registry for deliver operators
    pub sink_registry: Option<Arc<SinkRegistry>>,
}

impl std::fmt::Debug for FlowContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FlowContext")
            .field("event", &self.event)
            .field("variables", &self.variables)
            .field(
                "entity_fetchers",
                &self.entity_fetchers.keys().collect::<Vec<_>>(),
            )
            .finish()
    }
}

impl FlowContext {
    /// Create a new FlowContext from a framework event
    ///
    /// Automatically extracts event fields into variables.
    pub fn new(
        event: FrameworkEvent,
        link_service: Arc<dyn LinkService>,
        entity_fetchers: HashMap<String, Arc<dyn EntityFetcher>>,
    ) -> Self {
        let mut variables = HashMap::new();

        // Extract event-specific variables
        match &event {
            FrameworkEvent::Entity(entity_event) => {
                use crate::core::events::EntityEvent;
                match entity_event {
                    EntityEvent::Created {
                        entity_type,
                        entity_id,
                        data,
                    } => {
                        variables.insert(
                            "entity_type".to_string(),
                            Value::String(entity_type.clone()),
                        );
                        variables.insert(
                            "entity_id".to_string(),
                            Value::String(entity_id.to_string()),
                        );
                        variables.insert("data".to_string(), data.clone());
                    }
                    EntityEvent::Updated {
                        entity_type,
                        entity_id,
                        data,
                    } => {
                        variables.insert(
                            "entity_type".to_string(),
                            Value::String(entity_type.clone()),
                        );
                        variables.insert(
                            "entity_id".to_string(),
                            Value::String(entity_id.to_string()),
                        );
                        variables.insert("data".to_string(), data.clone());
                    }
                    EntityEvent::Deleted {
                        entity_type,
                        entity_id,
                    } => {
                        variables.insert(
                            "entity_type".to_string(),
                            Value::String(entity_type.clone()),
                        );
                        variables.insert(
                            "entity_id".to_string(),
                            Value::String(entity_id.to_string()),
                        );
                    }
                }
            }
            FrameworkEvent::Link(link_event) => {
                use crate::core::events::LinkEvent;
                match link_event {
                    LinkEvent::Created {
                        link_type,
                        link_id,
                        source_id,
                        target_id,
                        metadata,
                    } => {
                        variables.insert("link_type".to_string(), Value::String(link_type.clone()));
                        variables.insert("link_id".to_string(), Value::String(link_id.to_string()));
                        variables.insert(
                            "source_id".to_string(),
                            Value::String(source_id.to_string()),
                        );
                        variables.insert(
                            "target_id".to_string(),
                            Value::String(target_id.to_string()),
                        );
                        if let Some(meta) = metadata {
                            variables.insert("metadata".to_string(), meta.clone());
                        }
                    }
                    LinkEvent::Deleted {
                        link_type,
                        link_id,
                        source_id,
                        target_id,
                    } => {
                        variables.insert("link_type".to_string(), Value::String(link_type.clone()));
                        variables.insert("link_id".to_string(), Value::String(link_id.to_string()));
                        variables.insert(
                            "source_id".to_string(),
                            Value::String(source_id.to_string()),
                        );
                        variables.insert(
                            "target_id".to_string(),
                            Value::String(target_id.to_string()),
                        );
                    }
                }
            }
        }

        Self {
            event,
            variables,
            link_service,
            entity_fetchers,
            sink_registry: None,
        }
    }

    /// Set a variable in the context
    pub fn set_var(&mut self, name: impl Into<String>, value: Value) {
        self.variables.insert(name.into(), value);
    }

    /// Get a variable from the context
    pub fn get_var(&self, name: &str) -> Option<&Value> {
        // Support dotted access: "owner.id" -> variables["owner"]["id"]
        if let Some(dot_pos) = name.find('.') {
            let (root, rest) = name.split_at(dot_pos);
            let rest = &rest[1..]; // skip the dot
            if let Some(root_val) = self.variables.get(root) {
                return get_nested(root_val, rest);
            }
            return None;
        }
        self.variables.get(name)
    }

    /// Set the sink registry for deliver operators
    pub fn with_sink_registry(mut self, registry: Arc<SinkRegistry>) -> Self {
        self.sink_registry = Some(registry);
        self
    }

    /// Get a variable as a string (convenience)
    pub fn get_var_str(&self, name: &str) -> Option<&str> {
        self.get_var(name).and_then(|v| v.as_str())
    }
}

/// Navigate into nested JSON values via dotted path
fn get_nested<'a>(value: &'a Value, path: &str) -> Option<&'a Value> {
    if let Some(dot_pos) = path.find('.') {
        let (key, rest) = path.split_at(dot_pos);
        let rest = &rest[1..];
        match value {
            Value::Object(map) => map.get(key).and_then(|v| get_nested(v, rest)),
            _ => None,
        }
    } else {
        match value {
            Value::Object(map) => map.get(path),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::events::{EntityEvent, LinkEvent};
    use serde_json::json;
    use uuid::Uuid;

    // Minimal mock for tests
    struct MockLinkService;

    #[async_trait::async_trait]
    impl LinkService for MockLinkService {
        async fn create(
            &self,
            _link: crate::core::link::LinkEntity,
        ) -> anyhow::Result<crate::core::link::LinkEntity> {
            unimplemented!()
        }
        async fn get(&self, _id: &Uuid) -> anyhow::Result<Option<crate::core::link::LinkEntity>> {
            unimplemented!()
        }
        async fn list(&self) -> anyhow::Result<Vec<crate::core::link::LinkEntity>> {
            unimplemented!()
        }
        async fn find_by_source(
            &self,
            _source_id: &Uuid,
            _link_type: Option<&str>,
            _target_type: Option<&str>,
        ) -> anyhow::Result<Vec<crate::core::link::LinkEntity>> {
            unimplemented!()
        }
        async fn find_by_target(
            &self,
            _target_id: &Uuid,
            _link_type: Option<&str>,
            _source_type: Option<&str>,
        ) -> anyhow::Result<Vec<crate::core::link::LinkEntity>> {
            unimplemented!()
        }
        async fn update(
            &self,
            _id: &Uuid,
            _link: crate::core::link::LinkEntity,
        ) -> anyhow::Result<crate::core::link::LinkEntity> {
            unimplemented!()
        }
        async fn delete(&self, _id: &Uuid) -> anyhow::Result<()> {
            unimplemented!()
        }
        async fn delete_by_entity(&self, _entity_id: &Uuid) -> anyhow::Result<()> {
            unimplemented!()
        }
    }

    fn mock_link_service() -> Arc<dyn LinkService> {
        Arc::new(MockLinkService)
    }

    #[test]
    fn test_context_from_link_created() {
        let source_id = Uuid::new_v4();
        let target_id = Uuid::new_v4();
        let event = FrameworkEvent::Link(LinkEvent::Created {
            link_type: "follows".to_string(),
            link_id: Uuid::new_v4(),
            source_id,
            target_id,
            metadata: Some(json!({"note": "hello"})),
        });

        let ctx = FlowContext::new(event, mock_link_service(), HashMap::new());

        assert_eq!(ctx.get_var_str("link_type"), Some("follows"));
        assert_eq!(
            ctx.get_var_str("source_id"),
            Some(source_id.to_string().as_str())
        );
        assert_eq!(
            ctx.get_var_str("target_id"),
            Some(target_id.to_string().as_str())
        );
        assert_eq!(ctx.get_var("metadata"), Some(&json!({"note": "hello"})));
    }

    #[test]
    fn test_context_from_entity_created() {
        let entity_id = Uuid::new_v4();
        let event = FrameworkEvent::Entity(EntityEvent::Created {
            entity_type: "user".to_string(),
            entity_id,
            data: json!({"name": "Alice"}),
        });

        let ctx = FlowContext::new(event, mock_link_service(), HashMap::new());

        assert_eq!(ctx.get_var_str("entity_type"), Some("user"));
        assert_eq!(
            ctx.get_var_str("entity_id"),
            Some(entity_id.to_string().as_str())
        );
        assert_eq!(ctx.get_var("data"), Some(&json!({"name": "Alice"})));
    }

    #[test]
    fn test_set_and_get_var() {
        let event = FrameworkEvent::Entity(EntityEvent::Created {
            entity_type: "user".to_string(),
            entity_id: Uuid::new_v4(),
            data: json!({}),
        });

        let mut ctx = FlowContext::new(event, mock_link_service(), HashMap::new());
        ctx.set_var("owner", json!({"id": "abc", "name": "Bob"}));

        assert_eq!(
            ctx.get_var("owner"),
            Some(&json!({"id": "abc", "name": "Bob"}))
        );
    }

    #[test]
    fn test_dotted_access() {
        let event = FrameworkEvent::Entity(EntityEvent::Created {
            entity_type: "user".to_string(),
            entity_id: Uuid::new_v4(),
            data: json!({}),
        });

        let mut ctx = FlowContext::new(event, mock_link_service(), HashMap::new());
        ctx.set_var(
            "owner",
            json!({"id": "abc", "profile": {"name": "Bob", "age": 30}}),
        );

        assert_eq!(ctx.get_var_str("owner.id"), Some("abc"));
        assert_eq!(ctx.get_var_str("owner.profile.name"), Some("Bob"));
        assert_eq!(ctx.get_var("owner.profile.age"), Some(&json!(30)));
        assert_eq!(ctx.get_var("owner.nonexistent"), None);
        assert_eq!(ctx.get_var("nonexistent.field"), None);
    }

    #[test]
    fn test_link_deleted_context() {
        let source_id = Uuid::new_v4();
        let target_id = Uuid::new_v4();
        let event = FrameworkEvent::Link(LinkEvent::Deleted {
            link_type: "follows".to_string(),
            link_id: Uuid::new_v4(),
            source_id,
            target_id,
        });

        let ctx = FlowContext::new(event, mock_link_service(), HashMap::new());
        assert_eq!(ctx.get_var_str("link_type"), Some("follows"));
        assert_eq!(ctx.get_var("metadata"), None); // Deleted has no metadata
    }
}
