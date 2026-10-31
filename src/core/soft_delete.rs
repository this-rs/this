//! Soft delete as a *contract*, on top of any [`DataService`].
//!
//! ## Why this lives here and not in the nine backends
//!
//! The storage backends implement a primitive: `delete` removes the row,
//! `get`/`list`/`search` return what is stored. The README promises something
//! else — "built-in soft delete support". Those are two different things, and
//! the honest fix is not to rewrite the same semantics nine times, it is to
//! state them once: the nine backends differ in **how** they store, never in
//! **what a deletion means**.
//!
//! [`SoftDeleting`] wraps any `DataService<T>` and gives it that meaning:
//!
//! - `delete` marks `deleted_at` instead of removing the row;
//! - `get`, `list` and `search` hide what is marked;
//! - the mark is reversible and removable, but only on purpose:
//!   [`restore`](SoftDeleting::restore), [`purge`](SoftDeleting::purge),
//!   [`get_including_deleted`](SoftDeleting::get_including_deleted) and
//!   [`list_including_deleted`](SoftDeleting::list_including_deleted).
//!
//! ## What happens to a type that has no `deleted_at`
//!
//! [`Entity::set_deleted_at`](crate::core::Entity::set_deleted_at) answers
//! `false` for a type that cannot carry the mark — a hand-written `Entity`, for
//! instance. The decorator then performs the **hard** delete it always did, and
//! [`delete_reporting`](SoftDeleting::delete_reporting) says so. A delete that
//! silently does nothing would be the worst possible answer: the caller thinks
//! the data is gone and it is not.
//!
//! ## What this does not cover
//!
//! REST CRUD handlers are written by the application against its own store; the
//! framework never sees those calls. An application that wants the same
//! contract on its REST routes wraps its store the same way:
//! `SoftDeleting::new(InMemoryDataService::<Order>::new())`.

use anyhow::Result;
use async_trait::async_trait;
use chrono::Utc;
use uuid::Uuid;

use crate::core::{Data, DataService};

/// What a delete actually did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeleteOutcome {
    /// The entity was marked; the row is still stored.
    Soft,
    /// The type cannot carry the mark, so the row was really removed.
    Hard,
    /// There was no such entity.
    AlreadyGone,
}

/// A [`DataService`] whose deletions are reversible.
pub struct SoftDeleting<S> {
    inner: S,
}

impl<S> SoftDeleting<S> {
    pub fn new(inner: S) -> Self {
        Self { inner }
    }

    /// The wrapped service, for the operations that must bypass the contract.
    pub fn inner(&self) -> &S {
        &self.inner
    }

    pub fn into_inner(self) -> S {
        self.inner
    }

    /// Delete and say how. `DataService::delete` returns `()`, so the honest
    /// answer has to be reachable another way.
    pub async fn delete_reporting<T>(&self, id: &Uuid) -> Result<DeleteOutcome>
    where
        T: Data,
        S: DataService<T>,
    {
        let Some(mut entity) = self.inner.get(id).await? else {
            return Ok(DeleteOutcome::AlreadyGone);
        };

        // An entity that is already marked is not marked twice: the timestamp
        // of the first deletion is the one that counts.
        if entity.is_deleted() {
            return Ok(DeleteOutcome::Soft);
        }

        if entity.set_deleted_at(Some(Utc::now())) {
            self.inner.update(id, entity).await?;
            Ok(DeleteOutcome::Soft)
        } else {
            self.inner.delete(id).await?;
            Ok(DeleteOutcome::Hard)
        }
    }

    /// Undo a soft delete. `Ok(None)` when there is no such entity.
    pub async fn restore<T>(&self, id: &Uuid) -> Result<Option<T>>
    where
        T: Data,
        S: DataService<T>,
    {
        let Some(mut entity) = self.inner.get(id).await? else {
            return Ok(None);
        };
        if !entity.set_deleted_at(None) {
            anyhow::bail!(
                "restore: this entity type has no `deleted_at` field, so nothing was ever marked"
            );
        }
        Ok(Some(self.inner.update(id, entity).await?))
    }

    /// Remove the row for good. This is the only way back to a real delete, and
    /// it is deliberately its own name.
    pub async fn purge<T>(&self, id: &Uuid) -> Result<()>
    where
        T: Data,
        S: DataService<T>,
    {
        self.inner.delete(id).await
    }

    /// Read one entity even when it is marked deleted.
    pub async fn get_including_deleted<T>(&self, id: &Uuid) -> Result<Option<T>>
    where
        T: Data,
        S: DataService<T>,
    {
        self.inner.get(id).await
    }

    /// List everything, marked or not.
    pub async fn list_including_deleted<T>(&self) -> Result<Vec<T>>
    where
        T: Data,
        S: DataService<T>,
    {
        self.inner.list().await
    }
}

impl<S: Clone> Clone for SoftDeleting<S> {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
        }
    }
}

#[async_trait]
impl<T, S> DataService<T> for SoftDeleting<S>
where
    T: Data,
    S: DataService<T>,
{
    async fn create(&self, entity: T) -> Result<T> {
        self.inner.create(entity).await
    }

    async fn get(&self, id: &Uuid) -> Result<Option<T>> {
        Ok(self.inner.get(id).await?.filter(|e| !e.is_deleted()))
    }

    async fn list(&self) -> Result<Vec<T>> {
        Ok(self
            .inner
            .list()
            .await?
            .into_iter()
            .filter(|e| !e.is_deleted())
            .collect())
    }

    async fn update(&self, id: &Uuid, entity: T) -> Result<T> {
        self.inner.update(id, entity).await
    }

    /// Delete, softly where the type allows it. See [`Self::delete_reporting`]
    /// when the caller needs to know which happened.
    async fn delete(&self, id: &Uuid) -> Result<()> {
        self.delete_reporting::<T>(id).await.map(|_| ())
    }

    async fn search(&self, field: &str, value: &str) -> Result<Vec<T>> {
        Ok(self
            .inner
            .search(field, value)
            .await?
            .into_iter()
            .filter(|e| !e.is_deleted())
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::Entity;
    use crate::core::field::FieldValue;
    use crate::storage::in_memory::InMemoryDataService;
    use chrono::{DateTime, Utc};
    use std::sync::Arc;

    /// An entity that carries the mark — what the macros generate.
    #[derive(Clone, Debug)]
    struct Markable {
        id: Uuid,
        name: String,
        deleted_at: Option<DateTime<Utc>>,
        updated_at: DateTime<Utc>,
    }

    impl Markable {
        fn new(name: &str) -> Self {
            Self {
                id: Uuid::new_v4(),
                name: name.into(),
                deleted_at: None,
                updated_at: Utc::now(),
            }
        }
    }

    impl Entity for Markable {
        type Service = ();

        fn resource_name() -> &'static str {
            "markables"
        }
        fn resource_name_singular() -> &'static str {
            "markable"
        }
        fn service_from_host(
            _: &Arc<dyn std::any::Any + Send + Sync>,
        ) -> Result<Arc<Self::Service>> {
            Ok(Arc::new(()))
        }
        fn id(&self) -> Uuid {
            self.id
        }
        fn entity_type(&self) -> &str {
            "markable"
        }
        fn created_at(&self) -> DateTime<Utc> {
            self.updated_at
        }
        fn updated_at(&self) -> DateTime<Utc> {
            self.updated_at
        }
        fn deleted_at(&self) -> Option<DateTime<Utc>> {
            self.deleted_at
        }
        fn status(&self) -> &str {
            "active"
        }
        fn set_deleted_at(&mut self, at: Option<DateTime<Utc>>) -> bool {
            self.deleted_at = at;
            self.updated_at = Utc::now();
            true
        }
    }

    impl Data for Markable {
        fn name(&self) -> &str {
            &self.name
        }
        fn indexed_fields() -> &'static [&'static str] {
            &["name"]
        }
        fn field_value(&self, field: &str) -> Option<FieldValue> {
            match field {
                "name" => Some(FieldValue::String(self.name.clone())),
                _ => None,
            }
        }
    }

    /// An entity that does **not** carry the mark — a hand-written `Entity`.
    /// The decorator must fall back to a real delete and say so.
    #[derive(Clone, Debug)]
    struct Unmarkable {
        id: Uuid,
        name: String,
        updated_at: DateTime<Utc>,
    }

    impl Unmarkable {
        fn new(name: &str) -> Self {
            Self {
                id: Uuid::new_v4(),
                name: name.into(),
                updated_at: Utc::now(),
            }
        }
    }

    impl Entity for Unmarkable {
        type Service = ();

        fn resource_name() -> &'static str {
            "unmarkables"
        }
        fn resource_name_singular() -> &'static str {
            "unmarkable"
        }
        fn service_from_host(
            _: &Arc<dyn std::any::Any + Send + Sync>,
        ) -> Result<Arc<Self::Service>> {
            Ok(Arc::new(()))
        }
        fn id(&self) -> Uuid {
            self.id
        }
        fn entity_type(&self) -> &str {
            "unmarkable"
        }
        fn created_at(&self) -> DateTime<Utc> {
            self.updated_at
        }
        fn updated_at(&self) -> DateTime<Utc> {
            self.updated_at
        }
        fn deleted_at(&self) -> Option<DateTime<Utc>> {
            None
        }
        fn status(&self) -> &str {
            "active"
        }
        // `set_deleted_at` is deliberately NOT overridden: this is the default.
    }

    impl Data for Unmarkable {
        fn name(&self) -> &str {
            &self.name
        }
        fn indexed_fields() -> &'static [&'static str] {
            &[]
        }
        fn field_value(&self, _field: &str) -> Option<FieldValue> {
            None
        }
    }

    fn service() -> SoftDeleting<InMemoryDataService<Markable>> {
        SoftDeleting::new(InMemoryDataService::new())
    }

    #[tokio::test]
    async fn delete_marks_instead_of_removing() {
        let svc = service();
        let e = Markable::new("widget");
        let id = e.id;
        svc.create(e).await.unwrap();

        assert_eq!(
            svc.delete_reporting::<Markable>(&id).await.unwrap(),
            DeleteOutcome::Soft
        );

        // Hidden from the contract...
        assert!(svc.get(&id).await.unwrap().is_none());
        assert!(svc.list().await.unwrap().is_empty());
        assert!(svc.search("name", "widget").await.unwrap().is_empty());
        // ...but still there, and the row was never removed.
        let raw = svc.get_including_deleted::<Markable>(&id).await.unwrap();
        assert!(raw.is_some(), "a soft delete must not remove the row");
        assert!(raw.unwrap().is_deleted());
    }

    #[tokio::test]
    async fn restore_brings_it_back() {
        let svc = service();
        let e = Markable::new("widget");
        let id = e.id;
        svc.create(e).await.unwrap();
        svc.delete(&id).await.unwrap();

        let restored = svc
            .restore::<Markable>(&id)
            .await
            .unwrap()
            .expect("restored");
        assert!(!restored.is_deleted());
        assert!(svc.get(&id).await.unwrap().is_some());
        assert_eq!(svc.list().await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn purge_removes_for_good() {
        let svc = service();
        let e = Markable::new("widget");
        let id = e.id;
        svc.create(e).await.unwrap();
        svc.delete(&id).await.unwrap();
        svc.purge::<Markable>(&id).await.unwrap();

        assert!(
            svc.get_including_deleted::<Markable>(&id)
                .await
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn deleting_twice_keeps_the_first_timestamp() {
        let svc = service();
        let e = Markable::new("widget");
        let id = e.id;
        svc.create(e).await.unwrap();

        svc.delete(&id).await.unwrap();
        let first = svc
            .get_including_deleted::<Markable>(&id)
            .await
            .unwrap()
            .unwrap()
            .deleted_at
            .expect("marked");

        assert_eq!(
            svc.delete_reporting::<Markable>(&id).await.unwrap(),
            DeleteOutcome::Soft
        );
        let second = svc
            .get_including_deleted::<Markable>(&id)
            .await
            .unwrap()
            .unwrap()
            .deleted_at
            .expect("still marked");
        assert_eq!(first, second, "a second delete must not move the timestamp");
    }

    #[tokio::test]
    async fn delete_of_unknown_id_says_so() {
        let svc = service();
        let outcome = svc
            .delete_reporting::<Markable>(&Uuid::new_v4())
            .await
            .unwrap();
        assert_eq!(outcome, DeleteOutcome::AlreadyGone);
    }

    #[tokio::test]
    async fn a_type_without_the_field_falls_back_to_a_real_delete() {
        // The important half of the contract: no silent no-op.
        let svc = SoftDeleting::new(InMemoryDataService::<Unmarkable>::new());
        let e = Unmarkable::new("thing");
        let id = e.id;
        svc.create(e).await.unwrap();

        assert_eq!(
            svc.delete_reporting::<Unmarkable>(&id).await.unwrap(),
            DeleteOutcome::Hard
        );
        assert!(
            svc.get(&id).await.unwrap().is_none(),
            "the row must be gone"
        );
        assert!(
            svc.get_including_deleted::<Unmarkable>(&id)
                .await
                .unwrap()
                .is_none(),
            "a hard delete removes the row, even when asked to include deleted"
        );
    }
}
