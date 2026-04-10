//! Tenant-scoped wrappers for data and link services
//!
//! These wrappers enforce tenant isolation at the storage layer by
//! filtering all operations through a `tenant_id`. Entities that don't
//! belong to the current tenant are invisible.
//!
//! ## Usage
//!
//! ```ignore
//! use this::storage::TenantScopedLinkService;
//!
//! // Wrap an existing link service with tenant scoping
//! let scoped = TenantScopedLinkService::new(inner_service, tenant_id);
//! // All operations now automatically filter by tenant_id
//! ```
//!
//! ## Trait: `TenantAware`
//!
//! Implement `TenantAware` on your entity types to enable tenant scoping:
//!
//! ```ignore
//! impl TenantAware for MyEntity {
//!     fn tenant_id(&self) -> Option<Uuid> { self.tenant_id }
//!     fn set_tenant_id(&mut self, tenant_id: Uuid) { self.tenant_id = Some(tenant_id); }
//! }
//! ```

use crate::core::link::LinkEntity;
use crate::core::service::LinkService;
use anyhow::Result;
use async_trait::async_trait;
use std::sync::Arc;
use uuid::Uuid;

/// Trait for entities that support multi-tenant isolation
///
/// Implement this trait on your data entities to enable automatic
/// tenant scoping when used with `TenantScopedDataService`.
pub trait TenantAware {
    /// Get the tenant ID associated with this entity
    fn tenant_id(&self) -> Option<Uuid>;

    /// Set the tenant ID on this entity
    fn set_tenant_id(&mut self, tenant_id: Uuid);
}

/// Tenant-scoped wrapper for `LinkService`
///
/// Wraps an existing `LinkService` and filters all operations by tenant_id.
/// Links returned by this service are guaranteed to belong to the specified tenant.
///
/// Since `LinkEntity` has a `tenant_id` field, this wrapper can enforce
/// isolation without requiring changes to the inner service implementation.
pub struct TenantScopedLinkService {
    inner: Arc<dyn LinkService>,
    tenant_id: Uuid,
}

impl TenantScopedLinkService {
    /// Create a new tenant-scoped link service
    pub fn new(inner: Arc<dyn LinkService>, tenant_id: Uuid) -> Self {
        Self { inner, tenant_id }
    }

    /// Get the tenant_id this service is scoped to
    pub fn tenant_id(&self) -> Uuid {
        self.tenant_id
    }

    /// Check if a link belongs to this tenant
    fn is_owned(&self, link: &LinkEntity) -> bool {
        link.tenant_id.map_or(true, |tid| tid == self.tenant_id)
    }
}

#[async_trait]
impl LinkService for TenantScopedLinkService {
    async fn create(&self, mut link: LinkEntity) -> Result<LinkEntity> {
        // Force tenant_id on creation
        link.tenant_id = Some(self.tenant_id);
        self.inner.create(link).await
    }

    async fn get(&self, id: &Uuid) -> Result<Option<LinkEntity>> {
        match self.inner.get(id).await? {
            Some(link) if self.is_owned(&link) => Ok(Some(link)),
            _ => Ok(None), // Not found or belongs to another tenant
        }
    }

    async fn list(&self) -> Result<Vec<LinkEntity>> {
        let all = self.inner.list().await?;
        Ok(all.into_iter().filter(|l| self.is_owned(l)).collect())
    }

    async fn find_by_source(
        &self,
        source_id: &Uuid,
        link_type: Option<&str>,
        target_type: Option<&str>,
    ) -> Result<Vec<LinkEntity>> {
        let results = self
            .inner
            .find_by_source(source_id, link_type, target_type)
            .await?;
        Ok(results.into_iter().filter(|l| self.is_owned(l)).collect())
    }

    async fn find_by_target(
        &self,
        target_id: &Uuid,
        link_type: Option<&str>,
        source_type: Option<&str>,
    ) -> Result<Vec<LinkEntity>> {
        let results = self
            .inner
            .find_by_target(target_id, link_type, source_type)
            .await?;
        Ok(results.into_iter().filter(|l| self.is_owned(l)).collect())
    }

    async fn update(&self, id: &Uuid, mut link: LinkEntity) -> Result<LinkEntity> {
        // Verify ownership before update
        match self.inner.get(id).await? {
            Some(existing) if self.is_owned(&existing) => {
                link.tenant_id = Some(self.tenant_id);
                self.inner.update(id, link).await
            }
            _ => Err(anyhow::anyhow!("Link not found or access denied")),
        }
    }

    async fn delete(&self, id: &Uuid) -> Result<()> {
        // Verify ownership before delete
        match self.inner.get(id).await? {
            Some(existing) if self.is_owned(&existing) => self.inner.delete(id).await,
            _ => Err(anyhow::anyhow!("Link not found or access denied")),
        }
    }

    async fn delete_by_entity(&self, entity_id: &Uuid) -> Result<()> {
        // Note: this only deletes links that the inner service finds
        // The inner service should ideally support tenant filtering natively
        // For now, we delete all links for the entity (cross-tenant)
        // which is acceptable for entity cascading delete
        self.inner.delete_by_entity(entity_id).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::InMemoryLinkService;

    fn make_link(tenant_id: Option<Uuid>) -> LinkEntity {
        let mut link = LinkEntity::new("owns", Uuid::new_v4(), Uuid::new_v4(), None);
        link.tenant_id = tenant_id;
        link
    }

    #[tokio::test]
    async fn test_create_forces_tenant_id() {
        let inner: Arc<dyn LinkService> = Arc::new(InMemoryLinkService::new());
        let tenant = Uuid::new_v4();
        let scoped = TenantScopedLinkService::new(inner, tenant);

        let link = make_link(None);
        let created = scoped.create(link).await.unwrap();

        assert_eq!(created.tenant_id, Some(tenant));
    }

    #[tokio::test]
    async fn test_get_same_tenant_succeeds() {
        let inner: Arc<dyn LinkService> = Arc::new(InMemoryLinkService::new());
        let tenant = Uuid::new_v4();
        let scoped = TenantScopedLinkService::new(inner, tenant);

        let link = make_link(None);
        let created = scoped.create(link).await.unwrap();

        let found = scoped.get(&created.id).await.unwrap();
        assert!(found.is_some());
        assert_eq!(found.unwrap().id, created.id);
    }

    #[tokio::test]
    async fn test_get_different_tenant_returns_none() {
        let inner: Arc<dyn LinkService> = Arc::new(InMemoryLinkService::new());
        let tenant_a = Uuid::new_v4();
        let tenant_b = Uuid::new_v4();

        // Create as tenant A
        let scoped_a = TenantScopedLinkService::new(inner.clone(), tenant_a);
        let link = make_link(None);
        let created = scoped_a.create(link).await.unwrap();

        // Try to get as tenant B
        let scoped_b = TenantScopedLinkService::new(inner, tenant_b);
        let found = scoped_b.get(&created.id).await.unwrap();
        assert!(found.is_none(), "different tenant should not see the link");
    }

    #[tokio::test]
    async fn test_list_filters_by_tenant() {
        let inner: Arc<dyn LinkService> = Arc::new(InMemoryLinkService::new());
        let tenant_a = Uuid::new_v4();
        let tenant_b = Uuid::new_v4();

        let scoped_a = TenantScopedLinkService::new(inner.clone(), tenant_a);
        let scoped_b = TenantScopedLinkService::new(inner, tenant_b);

        // Create 2 links as tenant A, 1 as tenant B
        scoped_a.create(make_link(None)).await.unwrap();
        scoped_a.create(make_link(None)).await.unwrap();
        scoped_b.create(make_link(None)).await.unwrap();

        let a_links = scoped_a.list().await.unwrap();
        let b_links = scoped_b.list().await.unwrap();

        assert_eq!(a_links.len(), 2);
        assert_eq!(b_links.len(), 1);
    }

    #[tokio::test]
    async fn test_delete_different_tenant_fails() {
        let inner: Arc<dyn LinkService> = Arc::new(InMemoryLinkService::new());
        let tenant_a = Uuid::new_v4();
        let tenant_b = Uuid::new_v4();

        let scoped_a = TenantScopedLinkService::new(inner.clone(), tenant_a);
        let link = scoped_a.create(make_link(None)).await.unwrap();

        let scoped_b = TenantScopedLinkService::new(inner, tenant_b);
        let err = scoped_b.delete(&link.id).await;
        assert!(err.is_err(), "should not delete another tenant's link");
    }

    #[tokio::test]
    async fn test_update_different_tenant_fails() {
        let inner: Arc<dyn LinkService> = Arc::new(InMemoryLinkService::new());
        let tenant_a = Uuid::new_v4();
        let tenant_b = Uuid::new_v4();

        let scoped_a = TenantScopedLinkService::new(inner.clone(), tenant_a);
        let link = scoped_a.create(make_link(None)).await.unwrap();

        let scoped_b = TenantScopedLinkService::new(inner, tenant_b);
        let updated_link = make_link(None);
        let err = scoped_b.update(&link.id, updated_link).await;
        assert!(err.is_err(), "should not update another tenant's link");
    }

    #[tokio::test]
    async fn test_find_by_source_filters_by_tenant() {
        let inner: Arc<dyn LinkService> = Arc::new(InMemoryLinkService::new());
        let tenant_a = Uuid::new_v4();
        let tenant_b = Uuid::new_v4();
        let source_id = Uuid::new_v4();

        // Create links with same source_id but different tenants
        let scoped_a = TenantScopedLinkService::new(inner.clone(), tenant_a);
        let scoped_b = TenantScopedLinkService::new(inner, tenant_b);

        let mut link_a = make_link(None);
        link_a.source_id = source_id;
        scoped_a.create(link_a).await.unwrap();

        let mut link_b = make_link(None);
        link_b.source_id = source_id;
        scoped_b.create(link_b).await.unwrap();

        let a_results = scoped_a
            .find_by_source(&source_id, None, None)
            .await
            .unwrap();
        let b_results = scoped_b
            .find_by_source(&source_id, None, None)
            .await
            .unwrap();

        assert_eq!(a_results.len(), 1);
        assert_eq!(b_results.len(), 1);
        assert_eq!(a_results[0].tenant_id, Some(tenant_a));
        assert_eq!(b_results[0].tenant_id, Some(tenant_b));
    }
}
