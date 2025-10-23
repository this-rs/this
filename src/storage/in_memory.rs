//! In-memory implementation of LinkService

use crate::core::{Link, LinkService, EntityReference};
use anyhow::Result;
use async_trait::async_trait;
use std::collections::HashMap;
use uuid::Uuid;

/// In-memory implementation of LinkService
#[derive(Clone)]
pub struct InMemoryLinkService {
    links: std::sync::Arc<tokio::sync::RwLock<HashMap<Uuid, Link>>>,
}

impl InMemoryLinkService {
    pub fn new() -> Self {
        Self {
            links: std::sync::Arc::new(tokio::sync::RwLock::new(HashMap::new())),
        }
    }
}

#[async_trait]
impl LinkService for InMemoryLinkService {
    async fn create(
        &self,
        tenant_id: &Uuid,
        link_type: &str,
        source: EntityReference,
        target: EntityReference,
        metadata: Option<serde_json::Value>,
    ) -> Result<Link> {
        let link = Link::new(*tenant_id, link_type, source, target, metadata);
        let mut links = self.links.write().await;
        links.insert(link.id, link.clone());
        Ok(link)
    }

    async fn get(&self, _tenant_id: &Uuid, id: &Uuid) -> Result<Option<Link>> {
        let links = self.links.read().await;
        Ok(links.get(id).cloned())
    }

    async fn list(&self, tenant_id: &Uuid) -> Result<Vec<Link>> {
        let links = self.links.read().await;
        Ok(links.values().filter(|link| link.tenant_id == *tenant_id).cloned().collect())
    }

    async fn find_by_source(
        &self,
        tenant_id: &Uuid,
        source_id: &Uuid,
        source_type: &str,
        link_type: Option<&str>,
        target_type: Option<&str>,
    ) -> Result<Vec<Link>> {
        let links = self.links.read().await;
        Ok(links
            .values()
            .filter(|link| {
                link.tenant_id == *tenant_id
                    && link.source.id == *source_id
                    && link.source.entity_type == source_type
                    && link_type.map_or(true, |lt| link.link_type == lt)
                    && target_type.map_or(true, |tt| link.target.entity_type == tt)
            })
            .cloned()
            .collect())
    }

    async fn find_by_target(
        &self,
        tenant_id: &Uuid,
        target_id: &Uuid,
        target_type: &str,
        link_type: Option<&str>,
        source_type: Option<&str>,
    ) -> Result<Vec<Link>> {
        let links = self.links.read().await;
        Ok(links
            .values()
            .filter(|link| {
                link.tenant_id == *tenant_id
                    && link.target.id == *target_id
                    && link.target.entity_type == target_type
                    && link_type.map_or(true, |lt| link.link_type == lt)
                    && source_type.map_or(true, |st| link.source.entity_type == st)
            })
            .cloned()
            .collect())
    }

    async fn update(
        &self,
        _tenant_id: &Uuid,
        id: &Uuid,
        metadata: Option<serde_json::Value>,
    ) -> Result<Link> {
        let mut links = self.links.write().await;
        if let Some(link) = links.get_mut(id) {
            link.metadata = metadata;
            link.updated_at = chrono::Utc::now();
            Ok(link.clone())
        } else {
            anyhow::bail!("Link not found")
        }
    }

    async fn delete(&self, _tenant_id: &Uuid, id: &Uuid) -> Result<()> {
        let mut links = self.links.write().await;
        links.remove(id);
        Ok(())
    }

    async fn delete_by_entity(
        &self,
        tenant_id: &Uuid,
        entity_id: &Uuid,
        entity_type: &str,
    ) -> Result<()> {
        let mut links = self.links.write().await;
        links.retain(|_, link| {
            !(link.tenant_id == *tenant_id
                && ((link.source.id == *entity_id && link.source.entity_type == entity_type)
                    || (link.target.id == *entity_id && link.target.entity_type == entity_type)))
        });
        Ok(())
    }
}
