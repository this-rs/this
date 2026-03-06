//! Resolve operator — resolves an entity by ID or by following a link
//!
//! # Direct resolution (no `via`)
//!
//! Reads the entity ID from `from` in the context, fetches the entity
//! via the appropriate `EntityFetcher`, and stores it in `output_var`.
//!
//! # Link resolution (with `via`)
//!
//! Reads the entity ID from `from`, follows links of type `via` in the
//! specified `direction` (forward or reverse) via `LinkService`, takes
//! the first result, fetches the linked entity, and stores it.
//!
//! ```yaml
//! - resolve:
//!     from: source_id
//!     via: follows
//!     direction: reverse
//!     as: follower
//! ```

use crate::config::events::ResolveConfig;
use crate::events::context::FlowContext;
use crate::events::operators::{OpResult, PipelineOperator};
use anyhow::{Result, anyhow};
use async_trait::async_trait;
use serde_json::Value;
use uuid::Uuid;

/// Compiled resolve operator
#[derive(Debug, Clone)]
pub struct ResolveOp {
    /// Field name in context to read the entity ID from
    pub from: String,

    /// Optional link type to follow (None = direct ID resolution)
    pub via: Option<String>,

    /// Direction: "forward" or "reverse"
    pub direction: String,

    /// Variable name to store the resolved entity
    pub output_var: String,
}

impl ResolveOp {
    /// Create a ResolveOp from a ResolveConfig
    pub fn from_config(config: &ResolveConfig) -> Self {
        Self {
            from: config.from.clone(),
            via: config.via.clone(),
            direction: config.direction.clone(),
            output_var: config.output_var.clone(),
        }
    }
}

#[async_trait]
impl PipelineOperator for ResolveOp {
    async fn execute(&self, ctx: &mut FlowContext) -> Result<OpResult> {
        // Read the source ID from context
        let from_value = ctx
            .get_var(&self.from)
            .ok_or_else(|| anyhow!("resolve: variable '{}' not found in context", self.from))?
            .clone();

        let from_id = parse_uuid(&from_value, &self.from)?;

        match &self.via {
            None => {
                // Direct resolution: fetch entity by ID
                // We need to know the entity type to pick the right fetcher.
                // Convention: if `from` ends with `_id`, the entity type is
                // derived from the prefix (e.g., "source_id" → look at entity_type var).
                // For now, we try each fetcher until one succeeds.
                let entity = self.fetch_entity_by_id(ctx, &from_id).await?;
                ctx.set_var(&self.output_var, entity);
            }
            Some(link_type) => {
                // Link resolution: follow the link, then fetch the entity
                let target_id = self.follow_link(ctx, &from_id, link_type).await?;
                let entity = self.fetch_entity_by_id(ctx, &target_id).await?;
                ctx.set_var(&self.output_var, entity);
            }
        }

        Ok(OpResult::Continue)
    }

    fn name(&self) -> &str {
        "resolve"
    }
}

impl ResolveOp {
    /// Follow a link from `from_id` via `link_type` and return the target entity ID
    async fn follow_link(
        &self,
        ctx: &FlowContext,
        from_id: &Uuid,
        link_type: &str,
    ) -> Result<Uuid> {
        let links = match self.direction.as_str() {
            "forward" => {
                ctx.link_service
                    .find_by_source(from_id, Some(link_type), None)
                    .await?
            }
            "reverse" => {
                ctx.link_service
                    .find_by_target(from_id, Some(link_type), None)
                    .await?
            }
            other => {
                return Err(anyhow!(
                    "resolve: invalid direction '{}', expected 'forward' or 'reverse'",
                    other
                ));
            }
        };

        let link = links.first().ok_or_else(|| {
            anyhow!(
                "resolve: no '{}' link found from {} (direction: {})",
                link_type,
                from_id,
                self.direction
            )
        })?;

        // Return the other end of the link
        match self.direction.as_str() {
            "forward" => Ok(link.target_id),
            _ => Ok(link.source_id),
        }
    }

    /// Fetch an entity by ID using available entity fetchers
    ///
    /// Tries each fetcher until one returns a result.
    async fn fetch_entity_by_id(&self, ctx: &FlowContext, id: &Uuid) -> Result<Value> {
        for (_entity_type, fetcher) in &ctx.entity_fetchers {
            if let Ok(entity) = fetcher.fetch_as_json(id).await {
                return Ok(entity);
            }
        }

        Err(anyhow!(
            "resolve: entity {} not found in any registered fetcher",
            id
        ))
    }
}

/// Parse a UUID from a serde_json::Value
fn parse_uuid(value: &Value, field_name: &str) -> Result<Uuid> {
    match value {
        Value::String(s) => Uuid::parse_str(s)
            .map_err(|e| anyhow!("resolve: '{}' is not a valid UUID: {}", field_name, e)),
        _ => Err(anyhow!(
            "resolve: '{}' expected a string UUID, got {:?}",
            field_name,
            value
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::events::ResolveConfig;
    use crate::core::events::{FrameworkEvent, LinkEvent};
    use crate::core::link::LinkEntity;
    use crate::core::module::EntityFetcher;
    use crate::core::service::LinkService;
    use serde_json::json;
    use std::collections::HashMap;
    use std::sync::Arc;

    // ── Mock LinkService ─────────────────────────────────────────────

    struct MockLinkService {
        links: Vec<LinkEntity>,
    }

    #[async_trait]
    impl LinkService for MockLinkService {
        async fn create(&self, _link: LinkEntity) -> Result<LinkEntity> {
            unimplemented!()
        }
        async fn get(&self, _id: &Uuid) -> Result<Option<LinkEntity>> {
            unimplemented!()
        }
        async fn list(&self) -> Result<Vec<LinkEntity>> {
            Ok(self.links.clone())
        }
        async fn find_by_source(
            &self,
            source_id: &Uuid,
            link_type: Option<&str>,
            _target_type: Option<&str>,
        ) -> Result<Vec<LinkEntity>> {
            Ok(self
                .links
                .iter()
                .filter(|l| {
                    l.source_id == *source_id && link_type.map_or(true, |lt| l.link_type == lt)
                })
                .cloned()
                .collect())
        }
        async fn find_by_target(
            &self,
            target_id: &Uuid,
            link_type: Option<&str>,
            _source_type: Option<&str>,
        ) -> Result<Vec<LinkEntity>> {
            Ok(self
                .links
                .iter()
                .filter(|l| {
                    l.target_id == *target_id && link_type.map_or(true, |lt| l.link_type == lt)
                })
                .cloned()
                .collect())
        }
        async fn update(&self, _id: &Uuid, _link: LinkEntity) -> Result<LinkEntity> {
            unimplemented!()
        }
        async fn delete(&self, _id: &Uuid) -> Result<()> {
            unimplemented!()
        }
        async fn delete_by_entity(&self, _entity_id: &Uuid) -> Result<()> {
            unimplemented!()
        }
    }

    // ── Mock EntityFetcher ───────────────────────────────────────────

    struct MockEntityFetcher {
        entities: HashMap<Uuid, Value>,
    }

    #[async_trait]
    impl EntityFetcher for MockEntityFetcher {
        async fn fetch_as_json(&self, entity_id: &Uuid) -> Result<Value> {
            self.entities
                .get(entity_id)
                .cloned()
                .ok_or_else(|| anyhow!("entity not found"))
        }
    }

    // ── Helpers ──────────────────────────────────────────────────────

    fn make_context(
        source_id: Uuid,
        target_id: Uuid,
        link_service: Arc<dyn LinkService>,
        entity_fetchers: HashMap<String, Arc<dyn EntityFetcher>>,
    ) -> FlowContext {
        let event = FrameworkEvent::Link(LinkEvent::Created {
            link_type: "follows".to_string(),
            link_id: Uuid::new_v4(),
            source_id,
            target_id,
            metadata: None,
        });
        FlowContext::new(event, link_service, entity_fetchers)
    }

    // ── Tests ────────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_resolve_direct_by_id() {
        let entity_id = Uuid::new_v4();
        let entity_data = json!({"name": "Alice", "email": "alice@example.com"});

        let mut entities = HashMap::new();
        entities.insert(entity_id, entity_data.clone());

        let fetcher = Arc::new(MockEntityFetcher { entities }) as Arc<dyn EntityFetcher>;
        let mut fetchers = HashMap::new();
        fetchers.insert("user".to_string(), fetcher);

        let link_service = Arc::new(MockLinkService { links: vec![] }) as Arc<dyn LinkService>;

        let mut ctx = make_context(entity_id, Uuid::new_v4(), link_service, fetchers);

        let op = ResolveOp::from_config(&ResolveConfig {
            from: "source_id".to_string(),
            via: None,
            direction: "forward".to_string(),
            output_var: "owner".to_string(),
        });

        let result = op.execute(&mut ctx).await.unwrap();
        assert!(matches!(result, OpResult::Continue));
        assert_eq!(ctx.get_var("owner"), Some(&entity_data));
    }

    #[tokio::test]
    async fn test_resolve_via_link_forward() {
        let source_id = Uuid::new_v4();
        let target_id = Uuid::new_v4();
        let target_data = json!({"name": "Bob"});

        let link = LinkEntity {
            id: Uuid::new_v4(),
            entity_type: "link".to_string(),
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
            deleted_at: None,
            status: "active".to_string(),
            tenant_id: None,
            link_type: "follows".to_string(),
            source_id,
            target_id,
            metadata: None,
        };

        let mut entities = HashMap::new();
        entities.insert(target_id, target_data.clone());

        let fetcher = Arc::new(MockEntityFetcher { entities }) as Arc<dyn EntityFetcher>;
        let mut fetchers = HashMap::new();
        fetchers.insert("user".to_string(), fetcher);

        let link_service = Arc::new(MockLinkService { links: vec![link] }) as Arc<dyn LinkService>;

        let mut ctx = make_context(source_id, target_id, link_service, fetchers);

        let op = ResolveOp::from_config(&ResolveConfig {
            from: "source_id".to_string(),
            via: Some("follows".to_string()),
            direction: "forward".to_string(),
            output_var: "followed_user".to_string(),
        });

        let result = op.execute(&mut ctx).await.unwrap();
        assert!(matches!(result, OpResult::Continue));
        assert_eq!(ctx.get_var("followed_user"), Some(&target_data));
    }

    #[tokio::test]
    async fn test_resolve_via_link_reverse() {
        let source_id = Uuid::new_v4();
        let target_id = Uuid::new_v4();
        let source_data = json!({"name": "Alice"});

        let link = LinkEntity {
            id: Uuid::new_v4(),
            entity_type: "link".to_string(),
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
            deleted_at: None,
            status: "active".to_string(),
            tenant_id: None,
            link_type: "follows".to_string(),
            source_id,
            target_id,
            metadata: None,
        };

        let mut entities = HashMap::new();
        entities.insert(source_id, source_data.clone());

        let fetcher = Arc::new(MockEntityFetcher { entities }) as Arc<dyn EntityFetcher>;
        let mut fetchers = HashMap::new();
        fetchers.insert("user".to_string(), fetcher);

        let link_service = Arc::new(MockLinkService { links: vec![link] }) as Arc<dyn LinkService>;

        let mut ctx = make_context(source_id, target_id, link_service, fetchers);

        let op = ResolveOp::from_config(&ResolveConfig {
            from: "target_id".to_string(),
            via: Some("follows".to_string()),
            direction: "reverse".to_string(),
            output_var: "follower".to_string(),
        });

        let result = op.execute(&mut ctx).await.unwrap();
        assert!(matches!(result, OpResult::Continue));
        assert_eq!(ctx.get_var("follower"), Some(&source_data));
    }

    #[tokio::test]
    async fn test_resolve_missing_variable() {
        let link_service = Arc::new(MockLinkService { links: vec![] }) as Arc<dyn LinkService>;

        let mut ctx = make_context(Uuid::new_v4(), Uuid::new_v4(), link_service, HashMap::new());

        let op = ResolveOp::from_config(&ResolveConfig {
            from: "nonexistent_var".to_string(),
            via: None,
            direction: "forward".to_string(),
            output_var: "result".to_string(),
        });

        let result = op.execute(&mut ctx).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("nonexistent_var"));
    }

    #[tokio::test]
    async fn test_resolve_no_link_found() {
        let source_id = Uuid::new_v4();

        let link_service = Arc::new(MockLinkService { links: vec![] }) as Arc<dyn LinkService>;

        let mut ctx = make_context(source_id, Uuid::new_v4(), link_service, HashMap::new());

        let op = ResolveOp::from_config(&ResolveConfig {
            from: "source_id".to_string(),
            via: Some("follows".to_string()),
            direction: "forward".to_string(),
            output_var: "result".to_string(),
        });

        let result = op.execute(&mut ctx).await;
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("no 'follows' link found")
        );
    }

    #[tokio::test]
    async fn test_resolve_entity_not_found() {
        let entity_id = Uuid::new_v4();
        // Empty fetcher — entity won't be found
        let fetcher = Arc::new(MockEntityFetcher {
            entities: HashMap::new(),
        }) as Arc<dyn EntityFetcher>;
        let mut fetchers = HashMap::new();
        fetchers.insert("user".to_string(), fetcher);

        let link_service = Arc::new(MockLinkService { links: vec![] }) as Arc<dyn LinkService>;

        let mut ctx = make_context(entity_id, Uuid::new_v4(), link_service, fetchers);

        let op = ResolveOp::from_config(&ResolveConfig {
            from: "source_id".to_string(),
            via: None,
            direction: "forward".to_string(),
            output_var: "owner".to_string(),
        });

        let result = op.execute(&mut ctx).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("not found"));
    }
}
