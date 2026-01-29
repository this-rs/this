//! In-memory implementation of LinkService for testing and development

use crate::core::error::{LinkError, ThisError, ThisResult};
use crate::core::{LinkService, link::LinkEntity};
use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use uuid::Uuid;

/// In-memory link service implementation
///
/// Useful for testing and development. Uses tokio's async RwLock for
/// non-blocking access in async contexts.
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
    async fn create(&self, link: LinkEntity) -> ThisResult<LinkEntity> {
        let mut links = self.links.write().await;
        links.insert(link.id, link.clone());
        Ok(link)
    }

    async fn get(&self, id: &Uuid) -> ThisResult<Option<LinkEntity>> {
        let links = self.links.read().await;
        Ok(links.get(id).cloned())
    }

    async fn list(&self) -> ThisResult<Vec<LinkEntity>> {
        let links = self.links.read().await;
        Ok(links.values().cloned().collect())
    }

    async fn find_by_source(
        &self,
        source_id: &Uuid,
        link_type: Option<&str>,
        target_type: Option<&str>,
    ) -> ThisResult<Vec<LinkEntity>> {
        let links = self.links.read().await;

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
    ) -> ThisResult<Vec<LinkEntity>> {
        let links = self.links.read().await;

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

    async fn update(&self, id: &Uuid, updated_link: LinkEntity) -> ThisResult<LinkEntity> {
        let mut links = self.links.write().await;

        if !links.contains_key(id) {
            return Err(ThisError::Link(LinkError::NotFoundById { id: *id }));
        }

        links.insert(*id, updated_link.clone());
        Ok(updated_link)
    }

    async fn delete(&self, id: &Uuid) -> ThisResult<()> {
        let mut links = self.links.write().await;
        links.remove(id);
        Ok(())
    }

    async fn delete_by_entity(&self, entity_id: &Uuid) -> ThisResult<()> {
        let mut links = self.links.write().await;
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

    /// Test concurrent access with tokio's async RwLock
    /// This test verifies that the service handles multiple concurrent operations correctly
    #[tokio::test]
    async fn test_concurrent_writes() {
        use std::sync::Arc;

        let service = Arc::new(InMemoryLinkService::new());
        let mut handles = vec![];

        // Spawn 100 concurrent write tasks
        for i in 0..100 {
            let service_clone = Arc::clone(&service);
            let handle = tokio::spawn(async move {
                let link = LinkEntity::new(
                    &format!("link_type_{}", i),
                    Uuid::new_v4(),
                    Uuid::new_v4(),
                    None,
                );
                service_clone.create(link).await.unwrap()
            });
            handles.push(handle);
        }

        // Wait for all tasks to complete
        for handle in handles {
            handle.await.unwrap();
        }

        // Verify all links were created
        let links = service.list().await.unwrap();
        assert_eq!(links.len(), 100);
    }

    /// Test concurrent reads don't block each other
    #[tokio::test]
    async fn test_concurrent_reads() {
        use std::sync::Arc;

        let service = Arc::new(InMemoryLinkService::new());

        // Create some links first
        for i in 0..10 {
            let link = LinkEntity::new(
                &format!("link_type_{}", i),
                Uuid::new_v4(),
                Uuid::new_v4(),
                None,
            );
            service.create(link).await.unwrap();
        }

        let mut handles = vec![];

        // Spawn 50 concurrent read tasks
        for _ in 0..50 {
            let service_clone = Arc::clone(&service);
            let handle = tokio::spawn(async move {
                let links = service_clone.list().await.unwrap();
                assert_eq!(links.len(), 10);
                links.len()
            });
            handles.push(handle);
        }

        // All reads should complete successfully
        for handle in handles {
            let count = handle.await.unwrap();
            assert_eq!(count, 10);
        }
    }

    /// Test mixed concurrent reads and writes
    #[tokio::test]
    async fn test_concurrent_read_write() {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicUsize, Ordering};

        let service = Arc::new(InMemoryLinkService::new());
        let write_count = Arc::new(AtomicUsize::new(0));

        let mut handles = vec![];

        // Spawn writer tasks
        for i in 0..20 {
            let service_clone = Arc::clone(&service);
            let write_count_clone = Arc::clone(&write_count);
            let handle = tokio::spawn(async move {
                let link = LinkEntity::new(
                    &format!("concurrent_{}", i),
                    Uuid::new_v4(),
                    Uuid::new_v4(),
                    None,
                );
                service_clone.create(link).await.unwrap();
                write_count_clone.fetch_add(1, Ordering::SeqCst);
            });
            handles.push(handle);
        }

        // Spawn reader tasks
        for _ in 0..30 {
            let service_clone = Arc::clone(&service);
            let handle = tokio::spawn(async move {
                // Just read, the count may vary as writes are happening
                let _ = service_clone.list().await.unwrap();
            });
            handles.push(handle);
        }

        // Wait for all tasks
        for handle in handles {
            handle.await.unwrap();
        }

        // Verify final state
        let final_links = service.list().await.unwrap();
        assert_eq!(final_links.len(), 20);
        assert_eq!(write_count.load(Ordering::SeqCst), 20);
    }

    /// Test that update on non-existent link returns error
    #[tokio::test]
    async fn test_update_nonexistent_link() {
        use crate::core::error::{LinkError, ThisError};

        let service = InMemoryLinkService::new();
        let fake_id = Uuid::new_v4();
        let link = LinkEntity::new("test", Uuid::new_v4(), Uuid::new_v4(), None);

        let result = service.update(&fake_id, link).await;
        assert!(result.is_err());

        // Verify it's the correct typed error
        match result.unwrap_err() {
            ThisError::Link(LinkError::NotFoundById { id }) => {
                assert_eq!(id, fake_id);
            }
            other => panic!("Expected LinkError::NotFoundById, got {:?}", other),
        }
    }
}
