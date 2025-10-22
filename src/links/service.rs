//! In-memory implementation of LinkService for testing and development

use crate::core::{EntityReference, Link, LinkService};
use anyhow::{anyhow, Result};
use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use uuid::Uuid;

/// In-memory link service implementation
///
/// Useful for testing and development. Uses RwLock for thread-safe access.
#[derive(Clone)]
pub struct InMemoryLinkService {
    links: Arc<RwLock<HashMap<Uuid, Link>>>,
}

impl InMemoryLinkService {
    /// Create a new in-memory link service
    pub fn new() -> Self {
        Self {
            links: Arc::new(RwLock::new(HashMap::new())),
        }
    }
}

impl Default for InMemoryLinkService {
    fn default() -> Self {
        Self::new()
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

        let mut links = self
            .links
            .write()
            .map_err(|e| anyhow!("Failed to acquire write lock: {}", e))?;

        links.insert(link.id, link.clone());

        Ok(link)
    }

    async fn get(&self, tenant_id: &Uuid, id: &Uuid) -> Result<Option<Link>> {
        let links = self
            .links
            .read()
            .map_err(|e| anyhow!("Failed to acquire read lock: {}", e))?;

        Ok(links
            .get(id)
            .filter(|link| &link.tenant_id == tenant_id)
            .cloned())
    }

    async fn list(&self, tenant_id: &Uuid) -> Result<Vec<Link>> {
        let links = self
            .links
            .read()
            .map_err(|e| anyhow!("Failed to acquire read lock: {}", e))?;

        Ok(links
            .values()
            .filter(|link| &link.tenant_id == tenant_id)
            .cloned()
            .collect())
    }

    async fn find_by_source(
        &self,
        tenant_id: &Uuid,
        source_id: &Uuid,
        source_type: &str,
        link_type: Option<&str>,
        target_type: Option<&str>,
    ) -> Result<Vec<Link>> {
        let links = self
            .links
            .read()
            .map_err(|e| anyhow!("Failed to acquire read lock: {}", e))?;

        Ok(links
            .values()
            .filter(|link| {
                &link.tenant_id == tenant_id
                    && &link.source.id == source_id
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
        let links = self
            .links
            .read()
            .map_err(|e| anyhow!("Failed to acquire read lock: {}", e))?;

        Ok(links
            .values()
            .filter(|link| {
                &link.tenant_id == tenant_id
                    && &link.target.id == target_id
                    && link.target.entity_type == target_type
                    && link_type.map_or(true, |lt| link.link_type == lt)
                    && source_type.map_or(true, |st| link.source.entity_type == st)
            })
            .cloned()
            .collect())
    }

    async fn delete(&self, tenant_id: &Uuid, id: &Uuid) -> Result<()> {
        let mut links = self
            .links
            .write()
            .map_err(|e| anyhow!("Failed to acquire write lock: {}", e))?;

        if let Some(link) = links.get(id) {
            if &link.tenant_id != tenant_id {
                return Err(anyhow!("Link not found or access denied"));
            }
            links.remove(id);
        }

        Ok(())
    }

    async fn delete_by_entity(
        &self,
        tenant_id: &Uuid,
        entity_id: &Uuid,
        entity_type: &str,
    ) -> Result<()> {
        let mut links = self
            .links
            .write()
            .map_err(|e| anyhow!("Failed to acquire write lock: {}", e))?;

        links.retain(|_, link| {
            &link.tenant_id != tenant_id
                || (&link.source.id != entity_id || link.source.entity_type != entity_type)
                    && (&link.target.id != entity_id || link.target.entity_type != entity_type)
        });

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_create_link() {
        let service = InMemoryLinkService::new();
        let tenant_id = Uuid::new_v4();
        let user_id = Uuid::new_v4();
        let car_id = Uuid::new_v4();

        let link = service
            .create(
                &tenant_id,
                "owner",
                EntityReference::new(user_id, "user"),
                EntityReference::new(car_id, "car"),
                None,
            )
            .await
            .unwrap();

        assert_eq!(link.tenant_id, tenant_id);
        assert_eq!(link.link_type, "owner");
        assert_eq!(link.source.id, user_id);
        assert_eq!(link.target.id, car_id);
    }

    #[tokio::test]
    async fn test_find_by_source() {
        let service = InMemoryLinkService::new();
        let tenant_id = Uuid::new_v4();
        let user_id = Uuid::new_v4();
        let car1_id = Uuid::new_v4();
        let car2_id = Uuid::new_v4();

        // User owns car1
        service
            .create(
                &tenant_id,
                "owner",
                EntityReference::new(user_id, "user"),
                EntityReference::new(car1_id, "car"),
                None,
            )
            .await
            .unwrap();

        // User drives car2
        service
            .create(
                &tenant_id,
                "driver",
                EntityReference::new(user_id, "user"),
                EntityReference::new(car2_id, "car"),
                None,
            )
            .await
            .unwrap();

        // Find all links from user
        let links = service
            .find_by_source(&tenant_id, &user_id, "user", None, None)
            .await
            .unwrap();

        assert_eq!(links.len(), 2);

        // Find only owner links
        let owner_links = service
            .find_by_source(&tenant_id, &user_id, "user", Some("owner"), None)
            .await
            .unwrap();

        assert_eq!(owner_links.len(), 1);
        assert_eq!(owner_links[0].link_type, "owner");
    }

    #[tokio::test]
    async fn test_tenant_isolation() {
        let service = InMemoryLinkService::new();
        let tenant1_id = Uuid::new_v4();
        let tenant2_id = Uuid::new_v4();
        let user_id = Uuid::new_v4();
        let car_id = Uuid::new_v4();

        // Create link for tenant1
        let link = service
            .create(
                &tenant1_id,
                "owner",
                EntityReference::new(user_id, "user"),
                EntityReference::new(car_id, "car"),
                None,
            )
            .await
            .unwrap();

        // Tenant1 can see it
        let result = service.get(&tenant1_id, &link.id).await.unwrap();
        assert!(result.is_some());

        // Tenant2 cannot see it
        let result = service.get(&tenant2_id, &link.id).await.unwrap();
        assert!(result.is_none());
    }
}
