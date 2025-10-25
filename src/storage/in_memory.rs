//! In-memory implementation of LinkService for testing and development

use crate::core::{LinkService, link::LinkEntity};
use anyhow::{Result, anyhow};
use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use uuid::Uuid;

/// In-memory link service implementation
///
/// Useful for testing and development. Uses RwLock for thread-safe access.
#[derive(Clone)]
pub struct InMemoryLinkService {
    links: Arc<RwLock<HashMap<Uuid, LinkEntity>>>,
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
    async fn create(&self, link: LinkEntity) -> Result<LinkEntity> {
        let mut links = self
            .links
            .write()
            .map_err(|e| anyhow!("Failed to acquire write lock: {}", e))?;

        links.insert(link.id, link.clone());

        Ok(link)
    }

    async fn get(&self, id: &Uuid) -> Result<Option<LinkEntity>> {
        let links = self
            .links
            .read()
            .map_err(|e| anyhow!("Failed to acquire read lock: {}", e))?;

        Ok(links.get(id).cloned())
    }

    async fn list(&self) -> Result<Vec<LinkEntity>> {
        let links = self
            .links
            .read()
            .map_err(|e| anyhow!("Failed to acquire read lock: {}", e))?;

        Ok(links.values().cloned().collect())
    }

    async fn find_by_source(
        &self,
        source_id: &Uuid,
        link_type: Option<&str>,
        target_type: Option<&str>,
    ) -> Result<Vec<LinkEntity>> {
        let links = self
            .links
            .read()
            .map_err(|e| anyhow!("Failed to acquire read lock: {}", e))?;

        Ok(links
            .values()
            .filter(|link| {
                &link.source_id == source_id
                    && link_type.is_none_or(|lt| link.link_type == lt)
                    && target_type.is_none_or(|_tt| true) // TODO: Add target type to Link if needed
            })
            .cloned()
            .collect())
    }

    async fn find_by_target(
        &self,
        target_id: &Uuid,
        link_type: Option<&str>,
        source_type: Option<&str>,
    ) -> Result<Vec<LinkEntity>> {
        let links = self
            .links
            .read()
            .map_err(|e| anyhow!("Failed to acquire read lock: {}", e))?;

        Ok(links
            .values()
            .filter(|link| {
                &link.target_id == target_id
                    && link_type.is_none_or(|lt| link.link_type == lt)
                    && source_type.is_none_or(|_st| true) // TODO: Add source type to Link if needed
            })
            .cloned()
            .collect())
    }

    async fn update(&self, id: &Uuid, updated_link: LinkEntity) -> Result<LinkEntity> {
        let mut links = self
            .links
            .write()
            .map_err(|e| anyhow!("Failed to acquire write lock: {}", e))?;

        links.get_mut(id).ok_or_else(|| anyhow!("Link not found"))?;

        links.insert(*id, updated_link.clone());

        Ok(updated_link)
    }

    async fn delete(&self, id: &Uuid) -> Result<()> {
        let mut links = self
            .links
            .write()
            .map_err(|e| anyhow!("Failed to acquire write lock: {}", e))?;

        links.remove(id);

        Ok(())
    }

    async fn delete_by_entity(&self, entity_id: &Uuid) -> Result<()> {
        let mut links = self
            .links
            .write()
            .map_err(|e| anyhow!("Failed to acquire write lock: {}", e))?;

        links.retain(|_, link| &link.source_id != entity_id && &link.target_id != entity_id);

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_create_link() {
        let service = InMemoryLinkService::new();
        let user_id = Uuid::new_v4();
        let car_id = Uuid::new_v4();

        let link = LinkEntity::new("owner", user_id, car_id, None);

        let created = service.create(link.clone()).await.unwrap();

        assert_eq!(created.link_type, "owner");
        assert_eq!(created.source_id, user_id);
        assert_eq!(created.target_id, car_id);
    }

    #[tokio::test]
    async fn test_get_link() {
        let service = InMemoryLinkService::new();
        let link = LinkEntity::new("owner", Uuid::new_v4(), Uuid::new_v4(), None);

        service.create(link.clone()).await.unwrap();

        let retrieved = service.get(&link.id).await.unwrap();
        assert!(retrieved.is_some());
        assert_eq!(retrieved.unwrap().id, link.id);
    }

    #[tokio::test]
    async fn test_list_links() {
        let service = InMemoryLinkService::new();

        let link1 = LinkEntity::new("owner", Uuid::new_v4(), Uuid::new_v4(), None);
        let link2 = LinkEntity::new("driver", Uuid::new_v4(), Uuid::new_v4(), None);

        service.create(link1).await.unwrap();
        service.create(link2).await.unwrap();

        let links = service.list().await.unwrap();
        assert_eq!(links.len(), 2);
    }

    #[tokio::test]
    async fn test_find_by_source() {
        let service = InMemoryLinkService::new();
        let user_id = Uuid::new_v4();
        let car1_id = Uuid::new_v4();
        let car2_id = Uuid::new_v4();

        // User owns car1
        service
            .create(LinkEntity::new("owner", user_id, car1_id, None))
            .await
            .unwrap();

        // User drives car2
        service
            .create(LinkEntity::new("driver", user_id, car2_id, None))
            .await
            .unwrap();

        // Find all links from user
        let links = service.find_by_source(&user_id, None, None).await.unwrap();
        assert_eq!(links.len(), 2);

        // Find only owner links
        let owner_links = service
            .find_by_source(&user_id, Some("owner"), None)
            .await
            .unwrap();
        assert_eq!(owner_links.len(), 1);
        assert_eq!(owner_links[0].link_type, "owner");
    }

    #[tokio::test]
    async fn test_find_by_target() {
        let service = InMemoryLinkService::new();
        let user1_id = Uuid::new_v4();
        let user2_id = Uuid::new_v4();
        let car_id = Uuid::new_v4();

        // User1 owns car
        service
            .create(LinkEntity::new("owner", user1_id, car_id, None))
            .await
            .unwrap();

        // User2 drives car
        service
            .create(LinkEntity::new("driver", user2_id, car_id, None))
            .await
            .unwrap();

        // Find all links to car
        let links = service.find_by_target(&car_id, None, None).await.unwrap();
        assert_eq!(links.len(), 2);

        // Find only driver links
        let driver_links = service
            .find_by_target(&car_id, Some("driver"), None)
            .await
            .unwrap();
        assert_eq!(driver_links.len(), 1);
        assert_eq!(driver_links[0].link_type, "driver");
    }

    #[tokio::test]
    async fn test_update_link() {
        let service = InMemoryLinkService::new();
        let user_id = Uuid::new_v4();
        let company_id = Uuid::new_v4();

        let mut link = LinkEntity::new(
            "worker",
            user_id,
            company_id,
            Some(serde_json::json!({"role": "Developer"})),
        );

        service.create(link.clone()).await.unwrap();

        // Update metadata
        link.metadata = Some(serde_json::json!({"role": "Senior Developer"}));
        link.touch();

        let updated = service.update(&link.id, link.clone()).await.unwrap();
        assert_eq!(
            updated.metadata,
            Some(serde_json::json!({"role": "Senior Developer"}))
        );
    }

    #[tokio::test]
    async fn test_delete_link() {
        let service = InMemoryLinkService::new();
        let link = LinkEntity::new("owner", Uuid::new_v4(), Uuid::new_v4(), None);

        service.create(link.clone()).await.unwrap();

        let retrieved = service.get(&link.id).await.unwrap();
        assert!(retrieved.is_some());

        service.delete(&link.id).await.unwrap();

        let retrieved = service.get(&link.id).await.unwrap();
        assert!(retrieved.is_none());
    }

    #[tokio::test]
    async fn test_delete_by_entity() {
        let service = InMemoryLinkService::new();
        let user_id = Uuid::new_v4();
        let car1_id = Uuid::new_v4();
        let car2_id = Uuid::new_v4();

        service
            .create(LinkEntity::new("owner", user_id, car1_id, None))
            .await
            .unwrap();
        service
            .create(LinkEntity::new("driver", user_id, car2_id, None))
            .await
            .unwrap();
        service
            .create(LinkEntity::new("owner", Uuid::new_v4(), car1_id, None))
            .await
            .unwrap();

        let links = service.list().await.unwrap();
        assert_eq!(links.len(), 3);

        // Delete all links involving user_id
        service.delete_by_entity(&user_id).await.unwrap();

        let remaining = service.list().await.unwrap();
        assert_eq!(remaining.len(), 1);
        assert_ne!(remaining[0].source_id, user_id);
        assert_ne!(remaining[0].target_id, user_id);
    }
}
