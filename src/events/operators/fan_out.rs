//! Fan-out operator — multiplies events across linked entities
//!
//! For each link found (e.g., all followers of a user), the fan-out operator
//! clones the FlowContext and injects the linked entity into a named variable.
//! This turns 1 event into N events (one per linked entity).
//!
//! ```yaml
//! - fan_out:
//!     from: target_id
//!     via: follows
//!     direction: reverse
//!     as: follower
//! ```

use crate::config::events::FanOutConfig;
use crate::events::context::FlowContext;
use crate::events::operators::{OpResult, PipelineOperator};
use anyhow::{Result, anyhow};
use async_trait::async_trait;
use serde_json::Value;
use uuid::Uuid;

/// Compiled fan-out operator
#[derive(Debug, Clone)]
pub struct FanOutOp {
    /// Field name in context to read the entity ID from
    pub from: String,

    /// Link type to follow
    pub via: String,

    /// Direction: "forward" or "reverse"
    pub direction: String,

    /// Variable name for each iterated entity
    pub output_var: String,
}

impl FanOutOp {
    /// Create a FanOutOp from a FanOutConfig
    pub fn from_config(config: &FanOutConfig) -> Self {
        Self {
            from: config.from.clone(),
            via: config.via.clone(),
            direction: config.direction.clone(),
            output_var: config.output_var.clone(),
        }
    }
}

#[async_trait]
impl PipelineOperator for FanOutOp {
    async fn execute(&self, ctx: &mut FlowContext) -> Result<OpResult> {
        // Read the source ID from context
        let from_value = ctx
            .get_var(&self.from)
            .ok_or_else(|| anyhow!("fan_out: variable '{}' not found in context", self.from))?
            .clone();

        let from_id = parse_uuid(&from_value, &self.from)?;

        // Find all links
        let links = match self.direction.as_str() {
            "forward" => {
                ctx.link_service
                    .find_by_source(&from_id, Some(&self.via), None)
                    .await?
            }
            "reverse" => {
                ctx.link_service
                    .find_by_target(&from_id, Some(&self.via), None)
                    .await?
            }
            other => {
                return Err(anyhow!(
                    "fan_out: invalid direction '{}', expected 'forward' or 'reverse'",
                    other
                ));
            }
        };

        if links.is_empty() {
            return Ok(OpResult::Drop);
        }

        // For each link, clone the context and inject the linked entity ID
        let mut contexts = Vec::with_capacity(links.len());
        for link in &links {
            let mut new_ctx = ctx.clone();
            let entity_id = match self.direction.as_str() {
                "forward" => link.target_id,
                _ => link.source_id,
            };

            // Try to fetch the entity data via entity fetchers
            let entity_value = fetch_entity(&new_ctx, &entity_id).await;
            match entity_value {
                Some(data) => {
                    new_ctx.set_var(&self.output_var, data);
                }
                None => {
                    // If no fetcher found, store just the ID
                    new_ctx.set_var(&self.output_var, Value::String(entity_id.to_string()));
                }
            }

            // Always set the entity ID as a sub-variable for convenience
            new_ctx.set_var(
                &format!("{}_id", self.output_var),
                Value::String(entity_id.to_string()),
            );

            contexts.push(new_ctx);
        }

        Ok(OpResult::FanOut(contexts))
    }

    fn name(&self) -> &str {
        "fan_out"
    }
}

/// Try to fetch an entity by ID from any registered fetcher
async fn fetch_entity(ctx: &FlowContext, id: &Uuid) -> Option<Value> {
    for (_entity_type, fetcher) in &ctx.entity_fetchers {
        if let Ok(entity) = fetcher.fetch_as_json(id).await {
            return Some(entity);
        }
    }
    None
}

/// Parse a UUID from a serde_json::Value
fn parse_uuid(value: &Value, field_name: &str) -> Result<Uuid> {
    match value {
        Value::String(s) => Uuid::parse_str(s)
            .map_err(|e| anyhow!("fan_out: '{}' is not a valid UUID: {}", field_name, e)),
        _ => Err(anyhow!(
            "fan_out: '{}' expected a string UUID, got {:?}",
            field_name,
            value
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::events::FanOutConfig;
    use crate::core::events::{FrameworkEvent, LinkEvent};
    use crate::core::link::LinkEntity;
    use crate::core::module::EntityFetcher;
    use crate::core::service::LinkService;
    use serde_json::json;
    use std::collections::HashMap;
    use std::sync::Arc;

    // ── Mocks ────────────────────────────────────────────────────────

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

    struct MockEntityFetcher {
        entities: HashMap<Uuid, Value>,
    }

    #[async_trait]
    impl EntityFetcher for MockEntityFetcher {
        async fn fetch_as_json(&self, entity_id: &Uuid) -> Result<Value> {
            self.entities
                .get(entity_id)
                .cloned()
                .ok_or_else(|| anyhow!("not found"))
        }
    }

    fn make_link(source_id: Uuid, target_id: Uuid) -> LinkEntity {
        LinkEntity {
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
        }
    }

    fn make_context(
        target_id: Uuid,
        link_service: Arc<dyn LinkService>,
        entity_fetchers: HashMap<String, Arc<dyn EntityFetcher>>,
    ) -> FlowContext {
        let event = FrameworkEvent::Link(LinkEvent::Created {
            link_type: "follows".to_string(),
            link_id: Uuid::new_v4(),
            source_id: Uuid::new_v4(),
            target_id,
            metadata: None,
        });
        FlowContext::new(event, link_service, entity_fetchers)
    }

    // ── Tests ────────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_fan_out_zero_followers_drops() {
        let target_id = Uuid::new_v4();
        let link_service = Arc::new(MockLinkService { links: vec![] }) as Arc<dyn LinkService>;
        let mut ctx = make_context(target_id, link_service, HashMap::new());

        let op = FanOutOp::from_config(&FanOutConfig {
            from: "target_id".to_string(),
            via: "follows".to_string(),
            direction: "reverse".to_string(),
            output_var: "follower".to_string(),
        });

        let result = op.execute(&mut ctx).await.unwrap();
        assert!(matches!(result, OpResult::Drop));
    }

    #[tokio::test]
    async fn test_fan_out_one_follower() {
        let target_id = Uuid::new_v4();
        let follower_id = Uuid::new_v4();

        let links = vec![make_link(follower_id, target_id)];
        let link_service = Arc::new(MockLinkService { links }) as Arc<dyn LinkService>;

        let mut entities = HashMap::new();
        entities.insert(follower_id, json!({"name": "Alice"}));
        let fetcher = Arc::new(MockEntityFetcher { entities }) as Arc<dyn EntityFetcher>;
        let mut fetchers = HashMap::new();
        fetchers.insert("user".to_string(), fetcher);

        let mut ctx = make_context(target_id, link_service, fetchers);

        let op = FanOutOp::from_config(&FanOutConfig {
            from: "target_id".to_string(),
            via: "follows".to_string(),
            direction: "reverse".to_string(),
            output_var: "follower".to_string(),
        });

        let result = op.execute(&mut ctx).await.unwrap();
        match result {
            OpResult::FanOut(contexts) => {
                assert_eq!(contexts.len(), 1);
                assert_eq!(
                    contexts[0].get_var("follower"),
                    Some(&json!({"name": "Alice"}))
                );
                assert!(contexts[0].get_var("follower_id").is_some());
            }
            other => panic!("expected FanOut, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_fan_out_five_followers() {
        let target_id = Uuid::new_v4();
        let follower_ids: Vec<Uuid> = (0..5).map(|_| Uuid::new_v4()).collect();

        let links: Vec<LinkEntity> = follower_ids
            .iter()
            .map(|fid| make_link(*fid, target_id))
            .collect();
        let link_service = Arc::new(MockLinkService { links }) as Arc<dyn LinkService>;

        // No entity fetchers — should still work with just IDs
        let mut ctx = make_context(target_id, link_service, HashMap::new());

        let op = FanOutOp::from_config(&FanOutConfig {
            from: "target_id".to_string(),
            via: "follows".to_string(),
            direction: "reverse".to_string(),
            output_var: "follower".to_string(),
        });

        let result = op.execute(&mut ctx).await.unwrap();
        match result {
            OpResult::FanOut(contexts) => {
                assert_eq!(contexts.len(), 5);
                // Each context should have a follower_id
                for fctx in &contexts {
                    assert!(fctx.get_var("follower_id").is_some());
                    // Without fetcher, follower is stored as ID string
                    assert!(fctx.get_var("follower").is_some());
                }
            }
            other => panic!("expected FanOut, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_fan_out_forward_direction() {
        let source_id = Uuid::new_v4();
        let target_ids: Vec<Uuid> = (0..3).map(|_| Uuid::new_v4()).collect();

        let links: Vec<LinkEntity> = target_ids
            .iter()
            .map(|tid| make_link(source_id, *tid))
            .collect();
        let link_service = Arc::new(MockLinkService { links }) as Arc<dyn LinkService>;

        let event = FrameworkEvent::Link(LinkEvent::Created {
            link_type: "follows".to_string(),
            link_id: Uuid::new_v4(),
            source_id,
            target_id: target_ids[0],
            metadata: None,
        });
        let mut ctx = FlowContext::new(event, link_service, HashMap::new());

        let op = FanOutOp::from_config(&FanOutConfig {
            from: "source_id".to_string(),
            via: "follows".to_string(),
            direction: "forward".to_string(),
            output_var: "followed".to_string(),
        });

        let result = op.execute(&mut ctx).await.unwrap();
        match result {
            OpResult::FanOut(contexts) => {
                assert_eq!(contexts.len(), 3);
            }
            other => panic!("expected FanOut, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_fan_out_missing_variable() {
        let link_service = Arc::new(MockLinkService { links: vec![] }) as Arc<dyn LinkService>;
        let mut ctx = make_context(Uuid::new_v4(), link_service, HashMap::new());

        let op = FanOutOp::from_config(&FanOutConfig {
            from: "nonexistent".to_string(),
            via: "follows".to_string(),
            direction: "reverse".to_string(),
            output_var: "follower".to_string(),
        });

        let result = op.execute(&mut ctx).await;
        assert!(result.is_err());
    }
}
