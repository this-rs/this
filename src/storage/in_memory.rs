//! In-memory implementations of DataService and LinkService for testing and development

use crate::core::field::FieldValue;
use crate::core::{Data, DataService, LinkService, link::LinkEntity};
use anyhow::{Result, anyhow};
use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use uuid::Uuid;

/// In-memory implementation of DataService
///
/// Provides a generic in-memory store for any entity type implementing `Data`.
/// Useful for testing, development, and prototyping without external dependencies.
///
/// Uses `Arc<RwLock<HashMap>>` for thread-safe concurrent access.
///
/// # Example
///
/// ```rust,ignore
/// use this::storage::InMemoryDataService;
///
/// let service = InMemoryDataService::<MyEntity>::new();
/// let entity = service.create(my_entity).await?;
/// let found = service.get(&entity.id()).await?;
/// ```
pub struct InMemoryDataService<T: Data> {
    data: Arc<RwLock<HashMap<Uuid, T>>>,
}

impl<T: Data> InMemoryDataService<T> {
    /// Create a new empty in-memory data service
    pub fn new() -> Self {
        Self {
            data: Arc::new(RwLock::new(HashMap::new())),
        }
    }
}

impl<T: Data> Clone for InMemoryDataService<T> {
    fn clone(&self) -> Self {
        Self {
            data: Arc::clone(&self.data),
        }
    }
}

impl<T: Data> Default for InMemoryDataService<T> {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl<T: Data> DataService<T> for InMemoryDataService<T> {
    async fn create(&self, entity: T) -> Result<T> {
        let mut data = self
            .data
            .write()
            .map_err(|e| anyhow!("Failed to acquire write lock: {}", e))?;

        data.insert(entity.id(), entity.clone());

        Ok(entity)
    }

    async fn get(&self, id: &Uuid) -> Result<Option<T>> {
        let data = self
            .data
            .read()
            .map_err(|e| anyhow!("Failed to acquire read lock: {}", e))?;

        Ok(data.get(id).cloned())
    }

    async fn list(&self) -> Result<Vec<T>> {
        let data = self
            .data
            .read()
            .map_err(|e| anyhow!("Failed to acquire read lock: {}", e))?;

        Ok(data.values().cloned().collect())
    }

    async fn update(&self, id: &Uuid, entity: T) -> Result<T> {
        let mut data = self
            .data
            .write()
            .map_err(|e| anyhow!("Failed to acquire write lock: {}", e))?;

        data.get(id)
            .ok_or_else(|| anyhow!("Entity not found: {}", id))?;

        data.insert(*id, entity.clone());

        Ok(entity)
    }

    async fn delete(&self, id: &Uuid) -> Result<()> {
        let mut data = self
            .data
            .write()
            .map_err(|e| anyhow!("Failed to acquire write lock: {}", e))?;

        data.remove(id);

        Ok(())
    }

    async fn search(&self, field: &str, value: &str) -> Result<Vec<T>> {
        let data = self
            .data
            .read()
            .map_err(|e| anyhow!("Failed to acquire read lock: {}", e))?;

        Ok(data
            .values()
            .filter(|entity| {
                entity.field_value(field).is_some_and(|fv| match &fv {
                    FieldValue::String(s) => s == value,
                    FieldValue::Integer(i) => i.to_string() == value,
                    FieldValue::Float(f) => f.to_string() == value,
                    FieldValue::Boolean(b) => b.to_string() == value,
                    FieldValue::Uuid(u) => u.to_string() == value,
                    FieldValue::DateTime(dt) => dt.to_rfc3339() == value,
                    FieldValue::Null => false,
                })
            })
            .cloned()
            .collect())
    }
}

// ---------------------------------------------------------------------------
// LinkService
// ---------------------------------------------------------------------------

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
    use crate::core::entity::Entity;
    use crate::core::field::FieldValue;
    use chrono::{DateTime, Utc};

    // -----------------------------------------------------------------------
    // Test entity for InMemoryDataService tests
    // -----------------------------------------------------------------------

    #[derive(Clone, Debug, PartialEq)]
    struct TestDataEntity {
        id: Uuid,
        entity_name: String,
        status: String,
        created_at: DateTime<Utc>,
        updated_at: DateTime<Utc>,
    }

    impl TestDataEntity {
        fn new(name: &str) -> Self {
            let now = Utc::now();
            Self {
                id: Uuid::new_v4(),
                entity_name: name.to_string(),
                status: "active".to_string(),
                created_at: now,
                updated_at: now,
            }
        }
    }

    impl Entity for TestDataEntity {
        type Service = ();

        fn resource_name() -> &'static str {
            "test_data_entities"
        }

        fn resource_name_singular() -> &'static str {
            "test_data_entity"
        }

        fn service_from_host(
            _: &std::sync::Arc<dyn std::any::Any + Send + Sync>,
        ) -> anyhow::Result<std::sync::Arc<Self::Service>> {
            Ok(std::sync::Arc::new(()))
        }

        fn id(&self) -> Uuid {
            self.id
        }

        fn entity_type(&self) -> &str {
            "test_data"
        }

        fn created_at(&self) -> DateTime<Utc> {
            self.created_at
        }

        fn updated_at(&self) -> DateTime<Utc> {
            self.updated_at
        }

        fn deleted_at(&self) -> Option<DateTime<Utc>> {
            None
        }

        fn status(&self) -> &str {
            &self.status
        }
    }

    impl crate::core::Data for TestDataEntity {
        fn name(&self) -> &str {
            &self.entity_name
        }

        fn indexed_fields() -> &'static [&'static str] {
            &["entity_name", "status"]
        }

        fn field_value(&self, field: &str) -> Option<FieldValue> {
            match field {
                "entity_name" => Some(FieldValue::String(self.entity_name.clone())),
                "status" => Some(FieldValue::String(self.status.clone())),
                _ => None,
            }
        }
    }

    // -----------------------------------------------------------------------
    // InMemoryDataService CRUD tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_data_create_entity() {
        let service = InMemoryDataService::<TestDataEntity>::new();
        let entity = TestDataEntity::new("Alice");

        let created = service.create(entity.clone()).await.unwrap();
        assert_eq!(created.id, entity.id);
        assert_eq!(created.entity_name, "Alice");
    }

    #[tokio::test]
    async fn test_data_get_entity() {
        let service = InMemoryDataService::<TestDataEntity>::new();
        let entity = TestDataEntity::new("Bob");

        service.create(entity.clone()).await.unwrap();

        let retrieved = service.get(&entity.id).await.unwrap();
        assert!(retrieved.is_some());
        assert_eq!(retrieved.unwrap().entity_name, "Bob");
    }

    #[tokio::test]
    async fn test_data_get_nonexistent() {
        let service = InMemoryDataService::<TestDataEntity>::new();

        let retrieved = service.get(&Uuid::new_v4()).await.unwrap();
        assert!(retrieved.is_none());
    }

    #[tokio::test]
    async fn test_data_list_entities() {
        let service = InMemoryDataService::<TestDataEntity>::new();

        service.create(TestDataEntity::new("Alice")).await.unwrap();
        service.create(TestDataEntity::new("Bob")).await.unwrap();
        service
            .create(TestDataEntity::new("Charlie"))
            .await
            .unwrap();

        let all = service.list().await.unwrap();
        assert_eq!(all.len(), 3);
    }

    #[tokio::test]
    async fn test_data_list_empty() {
        let service = InMemoryDataService::<TestDataEntity>::new();

        let all = service.list().await.unwrap();
        assert!(all.is_empty());
    }

    #[tokio::test]
    async fn test_data_update_entity() {
        let service = InMemoryDataService::<TestDataEntity>::new();
        let mut entity = TestDataEntity::new("Alice");

        service.create(entity.clone()).await.unwrap();

        entity.entity_name = "Alice Updated".to_string();
        let updated = service.update(&entity.id, entity.clone()).await.unwrap();

        assert_eq!(updated.entity_name, "Alice Updated");

        // Verify persisted
        let retrieved = service.get(&entity.id).await.unwrap().unwrap();
        assert_eq!(retrieved.entity_name, "Alice Updated");
    }

    #[tokio::test]
    async fn test_data_update_nonexistent() {
        let service = InMemoryDataService::<TestDataEntity>::new();
        let entity = TestDataEntity::new("Ghost");
        let id = entity.id;

        let result = service.update(&id, entity).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("not found"));
    }

    #[tokio::test]
    async fn test_data_delete_entity() {
        let service = InMemoryDataService::<TestDataEntity>::new();
        let entity = TestDataEntity::new("Alice");

        service.create(entity.clone()).await.unwrap();
        assert!(service.get(&entity.id).await.unwrap().is_some());

        service.delete(&entity.id).await.unwrap();
        assert!(service.get(&entity.id).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn test_data_delete_nonexistent() {
        let service = InMemoryDataService::<TestDataEntity>::new();

        // Deleting a nonexistent entity should succeed silently
        let result = service.delete(&Uuid::new_v4()).await;
        assert!(result.is_ok());
    }

    // -----------------------------------------------------------------------
    // InMemoryDataService search tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_data_search_by_indexed_field() {
        let service = InMemoryDataService::<TestDataEntity>::new();

        service.create(TestDataEntity::new("Alice")).await.unwrap();
        service.create(TestDataEntity::new("Bob")).await.unwrap();
        service.create(TestDataEntity::new("Alice")).await.unwrap();

        let results = service.search("entity_name", "Alice").await.unwrap();
        assert_eq!(results.len(), 2);
        assert!(results.iter().all(|e| e.entity_name == "Alice"));
    }

    #[tokio::test]
    async fn test_data_search_no_results() {
        let service = InMemoryDataService::<TestDataEntity>::new();

        service.create(TestDataEntity::new("Alice")).await.unwrap();

        let results = service.search("entity_name", "Zara").await.unwrap();
        assert!(results.is_empty());
    }

    #[tokio::test]
    async fn test_data_search_by_status() {
        let service = InMemoryDataService::<TestDataEntity>::new();

        let mut inactive = TestDataEntity::new("Inactive");
        inactive.status = "inactive".to_string();

        service.create(TestDataEntity::new("Active")).await.unwrap();
        service.create(inactive).await.unwrap();

        let results = service.search("status", "inactive").await.unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].entity_name, "Inactive");
    }

    #[tokio::test]
    async fn test_data_search_unknown_field() {
        let service = InMemoryDataService::<TestDataEntity>::new();

        service.create(TestDataEntity::new("Alice")).await.unwrap();

        // Unknown field returns no results (field_value returns None)
        let results = service.search("nonexistent_field", "anything").await.unwrap();
        assert!(results.is_empty());
    }

    #[tokio::test]
    async fn test_data_clone_shares_state() {
        let service = InMemoryDataService::<TestDataEntity>::new();
        let cloned = service.clone();

        service.create(TestDataEntity::new("Alice")).await.unwrap();

        // Clone shares the same backing store
        let all = cloned.list().await.unwrap();
        assert_eq!(all.len(), 1);
    }

    // -----------------------------------------------------------------------
    // InMemoryLinkService tests (existing)
    // -----------------------------------------------------------------------

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
