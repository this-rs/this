//! The single seam that publishes [`LinkEvent`]s.
//!
//! Entities have `SurfaceGuard::creator`; links have this decorator. It wraps the host's
//! [`LinkService`] **once** (in `ServerHost::with_event_bus`), so every surface that writes
//! links through `host.link_service` — REST, gRPC, GraphQL, `createAndLink` — emits the same
//! events without publishing by hand.
//!
//! Contract:
//! - `create` publishes one `Created` after the inner write succeeded;
//! - `delete` reads the link first (`get`) so the payload is exact; a link that does not
//!   exist publishes **nothing** (no phantom `Deleted`); a failed delete publishes nothing;
//! - `delete_by_entity` publishes one `Deleted` per link actually removed;
//! - `update` publishes nothing: [`LinkEvent`] has no `Updated` variant (by design, a link
//!   metadata change is silent on the event bus);
//! - `delete_by_tenant` (GDPR erasure) is delegated untouched: the erasure route emits its
//!   own tenant-level event, and one event per link would flood subscribers.

use std::sync::Arc;

use anyhow::Result;
use async_trait::async_trait;
use uuid::Uuid;

use crate::core::events::{EventBus, FrameworkEvent, LinkEvent};
use crate::core::link::LinkEntity;
use crate::core::service::LinkService;

/// [`LinkService`] decorator that publishes [`LinkEvent`]s after successful writes.
pub struct EventedLinkService {
    inner: Arc<dyn LinkService>,
    bus: Arc<EventBus>,
}

impl EventedLinkService {
    pub fn new(inner: Arc<dyn LinkService>, bus: Arc<EventBus>) -> Self {
        Self { inner, bus }
    }

    fn deleted(&self, link: &LinkEntity) {
        self.bus.publish(FrameworkEvent::Link(LinkEvent::Deleted {
            link_type: link.link_type.clone(),
            link_id: link.id,
            source_id: link.source_id,
            target_id: link.target_id,
        }));
    }
}

#[async_trait]
impl LinkService for EventedLinkService {
    async fn create(&self, link: LinkEntity) -> Result<LinkEntity> {
        let created = self.inner.create(link).await?;
        self.bus.publish(FrameworkEvent::Link(LinkEvent::Created {
            link_type: created.link_type.clone(),
            link_id: created.id,
            source_id: created.source_id,
            target_id: created.target_id,
            metadata: created.metadata.clone(),
        }));
        Ok(created)
    }

    async fn get(&self, id: &Uuid) -> Result<Option<LinkEntity>> {
        self.inner.get(id).await
    }

    async fn list(&self) -> Result<Vec<LinkEntity>> {
        self.inner.list().await
    }

    async fn find_by_source(
        &self,
        source_id: &Uuid,
        link_type: Option<&str>,
        target_type: Option<&str>,
    ) -> Result<Vec<LinkEntity>> {
        self.inner
            .find_by_source(source_id, link_type, target_type)
            .await
    }

    async fn find_by_target(
        &self,
        target_id: &Uuid,
        link_type: Option<&str>,
        source_type: Option<&str>,
    ) -> Result<Vec<LinkEntity>> {
        self.inner
            .find_by_target(target_id, link_type, source_type)
            .await
    }

    async fn update(&self, id: &Uuid, link: LinkEntity) -> Result<LinkEntity> {
        // No `LinkEvent::Updated` exists: a link update is silent on the bus.
        self.inner.update(id, link).await
    }

    async fn delete(&self, id: &Uuid) -> Result<()> {
        let existing = self.inner.get(id).await?;
        self.inner.delete(id).await?;
        if let Some(link) = existing {
            self.deleted(&link);
        }
        Ok(())
    }

    async fn delete_by_entity(&self, entity_id: &Uuid) -> Result<()> {
        let mut links = self.inner.find_by_source(entity_id, None, None).await?;
        for link in self.inner.find_by_target(entity_id, None, None).await? {
            if !links.iter().any(|l| l.id == link.id) {
                links.push(link);
            }
        }
        self.inner.delete_by_entity(entity_id).await?;
        for link in &links {
            self.deleted(link);
        }
        Ok(())
    }

    async fn delete_by_tenant(&self, tenant_id: &Uuid) -> Result<usize> {
        self.inner.delete_by_tenant(tenant_id).await
    }
}
