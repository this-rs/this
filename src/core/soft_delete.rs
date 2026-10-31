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
//! [`crate::core::Entity::set_deleted_at`] answers
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

// ─────────────────────────────────────────────────────────────────────────────
// The JSON surfaces: GraphQL, gRPC and the link routes do not touch
// `DataService` at all. They go through `EntityFetcher` / `EntityCreator`, which
// speak `serde_json::Value`. The same contract is therefore stated again here,
// once, over those two traits.
// ─────────────────────────────────────────────────────────────────────────────

use std::sync::Arc;

use crate::core::{EntityCreator, EntityFetcher};
use crate::descriptor::{FilterExpr, FilterOp, Query};

/// A stored document is hidden when it carries a non-null `deleted_at`.
fn is_hidden(document: &serde_json::Value) -> bool {
    document
        .get("deleted_at")
        .map(|v| !v.is_null())
        .unwrap_or(false)
}

/// `EntityFetcher` whose reads hide soft-deleted rows.
pub struct SoftDeleteFetcher {
    inner: Arc<dyn EntityFetcher>,
}

impl SoftDeleteFetcher {
    pub fn new(inner: Arc<dyn EntityFetcher>) -> Self {
        Self { inner }
    }
}

/// Add `deleted_at IS NULL` to a query, once. A caller that already filters on
/// `deleted_at` keeps its own condition: this is an AND, never a decision taken
/// in the caller's place.
fn with_hidden_filter(query: &Query) -> Query {
    // Built through the public builder: `Query`'s `_entity` marker is private and
    // must not be set by hand.
    let mut q = Query::new();
    if let Some(filter) = &query.filter {
        q = q.filter(filter.clone());
    }
    for spec in &query.sort {
        q = q.sort_by(&spec.field, spec.dir);
    }
    q.page = query.page.clone();
    q.filter(FilterExpr::Cmp {
        field: "deleted_at".into(),
        op: FilterOp::IsNull,
        value: serde_json::Value::Null,
    })
}

#[async_trait]
impl EntityFetcher for SoftDeleteFetcher {
    async fn fetch_as_json(&self, entity_id: &Uuid) -> Result<serde_json::Value> {
        let document = self.inner.fetch_as_json(entity_id).await?;
        if is_hidden(&document) {
            // Indistinguishable from "no such row": that is what a soft delete
            // means to a reader, and telling the two apart would leak the fact
            // that something was deleted.
            anyhow::bail!("{entity_id}: not found");
        }
        Ok(document)
    }

    async fn get_sample_entity(&self) -> Result<serde_json::Value> {
        self.inner.get_sample_entity().await
    }

    async fn list_as_json(
        &self,
        limit: Option<i32>,
        offset: Option<i32>,
    ) -> Result<Vec<serde_json::Value>> {
        let Some(want) = limit else {
            return Ok(self
                .inner
                .list_as_json(None, offset)
                .await?
                .into_iter()
                .filter(|d| !is_hidden(d))
                .collect());
        };

        // Filtering after the source paginated would return short pages. So the
        // decorator reads in growing batches until it has `want` visible rows or
        // the source is exhausted.
        let want = want.max(0) as usize;
        let mut skip = offset.unwrap_or(0).max(0) as usize;
        let mut batch = (want.saturating_mul(2)).max(32);
        let mut visible = Vec::with_capacity(want);

        loop {
            let page = self
                .inner
                .list_as_json(Some(batch as i32), Some(skip as i32))
                .await?;
            let exhausted = page.len() < batch;
            for document in page {
                if is_hidden(&document) {
                    continue;
                }
                visible.push(document);
                if visible.len() == want {
                    return Ok(visible);
                }
            }
            if exhausted {
                return Ok(visible);
            }
            skip += batch;
            batch = batch.saturating_mul(2);
        }
    }

    fn list_query_max_rows(&self) -> usize {
        self.inner.list_query_max_rows()
    }

    async fn list_query(&self, query: &Query) -> Result<(Vec<serde_json::Value>, usize)> {
        // Delegated with the filter injected, so a backend that pushes the query
        // down (SQL `WHERE`, Mongo filter) keeps an exact `total` and correct
        // pages. Filtering the result here instead would shrink a page after the
        // fact and lie about the count.
        self.inner.list_query(&with_hidden_filter(query)).await
    }
}

/// `EntityCreator` whose `delete` marks instead of removing.
pub struct SoftDeleteCreator {
    inner: Arc<dyn EntityCreator>,
    fetcher: Arc<dyn EntityFetcher>,
}

impl SoftDeleteCreator {
    pub fn new(inner: Arc<dyn EntityCreator>, fetcher: Arc<dyn EntityFetcher>) -> Self {
        Self { inner, fetcher }
    }
}

#[async_trait]
impl EntityCreator for SoftDeleteCreator {
    async fn create_from_json(&self, entity_data: serde_json::Value) -> Result<serde_json::Value> {
        self.inner.create_from_json(entity_data).await
    }

    async fn update_from_json(
        &self,
        entity_id: &Uuid,
        entity_data: serde_json::Value,
    ) -> Result<serde_json::Value> {
        self.inner.update_from_json(entity_id, entity_data).await
    }

    async fn delete(&self, entity_id: &Uuid) -> Result<()> {
        // Read the whole document first: `update_from_json` is a replacement in
        // most backends, so sending only `{"deleted_at": ...}` would erase the
        // rest of the row.
        let Ok(mut document) = self.fetcher.fetch_as_json(entity_id).await else {
            // Already gone (or hidden): nothing to mark.
            return Ok(());
        };
        document["deleted_at"] = serde_json::Value::String(Utc::now().to_rfc3339());

        match self.inner.update_from_json(entity_id, document).await {
            Ok(_) => Ok(()),
            // The type or the store cannot carry the mark: fall back to the real
            // delete rather than pretending it happened.
            Err(_) => self.inner.delete(entity_id).await,
        }
    }
}

#[cfg(test)]
mod json_tests {
    use super::*;

    /// A store of plain documents, enough to exercise the decorator.
    struct StubFetcher {
        docs: Vec<serde_json::Value>,
    }

    #[async_trait]
    impl EntityFetcher for StubFetcher {
        async fn fetch_as_json(&self, id: &Uuid) -> Result<serde_json::Value> {
            self.docs
                .iter()
                .find(|d| d["id"] == id.to_string())
                .cloned()
                .ok_or_else(|| anyhow::anyhow!("not found"))
        }

        async fn list_as_json(
            &self,
            limit: Option<i32>,
            offset: Option<i32>,
        ) -> Result<Vec<serde_json::Value>> {
            let skip = offset.unwrap_or(0).max(0) as usize;
            let take = limit.unwrap_or(i32::MAX).max(0) as usize;
            Ok(self.docs.iter().skip(skip).take(take).cloned().collect())
        }
    }

    fn doc(n: usize, deleted: bool) -> serde_json::Value {
        serde_json::json!({
            "id": Uuid::from_u128(n as u128).to_string(),
            "name": format!("row-{n}"),
            "deleted_at": if deleted { serde_json::json!("2026-10-31T00:00:00Z") } else { serde_json::Value::Null },
        })
    }

    #[tokio::test]
    async fn fetch_hides_a_marked_row_and_says_not_found() {
        let fetcher = SoftDeleteFetcher::new(Arc::new(StubFetcher {
            docs: vec![doc(1, false), doc(2, true)],
        }));

        let alive = fetcher
            .fetch_as_json(&Uuid::from_u128(1))
            .await
            .expect("row 1 is alive");
        assert_eq!(alive["name"], "row-1");

        let gone = fetcher.fetch_as_json(&Uuid::from_u128(2)).await;
        assert!(gone.is_err(), "a marked row must read as absent");
        assert_eq!(
            gone.unwrap_err().to_string(),
            format!("{}: not found", Uuid::from_u128(2))
        );
    }

    #[tokio::test]
    async fn list_returns_full_pages_when_some_rows_are_hidden() {
        // Hidden rows sit inside the first page; a naive filter-after-paginate
        // would return a short page.
        let mut docs = Vec::new();
        for n in 1..=12 {
            docs.push(doc(n, n % 2 == 0));
        }
        let fetcher = SoftDeleteFetcher::new(Arc::new(StubFetcher { docs }));

        let page = fetcher.list_as_json(Some(3), Some(0)).await.unwrap();
        assert_eq!(page.len(), 3, "the page must be full, not filtered short");
        assert!(page.iter().all(|d| !is_hidden(d)));

        let all = fetcher.list_as_json(None, None).await.unwrap();
        assert_eq!(all.len(), 6, "only the 6 alive rows");
    }

    #[tokio::test]
    async fn list_query_carries_the_hidden_filter_to_the_backend() {
        // The inner fetcher sees the query the decorator builds; it must contain
        // `deleted_at IS NULL`, and must keep the caller's own filter.
        let q = Query::new().filter(FilterExpr::Cmp {
            field: "status".into(),
            op: FilterOp::Eq,
            value: serde_json::json!("active"),
        });
        let injected = with_hidden_filter(&q);

        match injected.filter {
            Some(FilterExpr::And(parts)) => {
                assert_eq!(parts.len(), 2, "caller filter + hidden filter");
                assert!(parts.iter().any(|p| matches!(
                    p,
                    FilterExpr::Cmp { field, op: FilterOp::IsNull, .. } if field == "deleted_at"
                )));
            }
            other => panic!("expected an AND, got {other:?}"),
        }
    }
}

#[cfg(test)]
mod creator_tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::Mutex;

    /// A tiny document store with interior mutability: enough to exercise the
    /// creator path that GraphQL and gRPC mutations actually use.
    struct Store {
        docs: Mutex<HashMap<Uuid, serde_json::Value>>,
        updates_supported: bool,
    }

    impl Store {
        fn new(updates_supported: bool) -> Arc<Self> {
            Arc::new(Self {
                docs: Mutex::new(HashMap::new()),
                updates_supported,
            })
        }
        fn insert(&self, id: Uuid, name: &str) {
            self.docs.lock().unwrap().insert(
                id,
                serde_json::json!({ "id": id.to_string(), "name": name, "deleted_at": null }),
            );
        }
        fn snapshot(&self, id: &Uuid) -> Option<serde_json::Value> {
            self.docs.lock().unwrap().get(id).cloned()
        }
    }

    #[async_trait]
    impl EntityFetcher for Store {
        async fn fetch_as_json(&self, id: &Uuid) -> Result<serde_json::Value> {
            self.snapshot(id)
                .ok_or_else(|| anyhow::anyhow!("not found"))
        }
    }

    #[async_trait]
    impl EntityCreator for Store {
        async fn create_from_json(&self, data: serde_json::Value) -> Result<serde_json::Value> {
            Ok(data)
        }

        async fn update_from_json(
            &self,
            id: &Uuid,
            data: serde_json::Value,
        ) -> Result<serde_json::Value> {
            if !self.updates_supported {
                anyhow::bail!("Update operation not implemented for this entity type");
            }
            let mut docs = self.docs.lock().unwrap();
            let slot = docs
                .get_mut(id)
                .ok_or_else(|| anyhow::anyhow!("not found"))?;
            *slot = data.clone();
            Ok(data)
        }

        async fn delete(&self, id: &Uuid) -> Result<()> {
            self.docs.lock().unwrap().remove(id);
            Ok(())
        }
    }

    #[tokio::test]
    async fn delete_marks_the_document_and_keeps_the_rest_of_it() {
        let store = Store::new(true);
        let id = Uuid::new_v4();
        store.insert(id, "widget");

        let creator = SoftDeleteCreator::new(store.clone(), store.clone());
        creator.delete(&id).await.unwrap();

        let row = store.snapshot(&id).expect("the row must still be there");
        assert!(
            !row["deleted_at"].is_null(),
            "delete must mark the row: {row}"
        );
        assert_eq!(
            row["name"], "widget",
            "the mark must not erase the other fields -- update_from_json replaces the document"
        );
    }

    #[tokio::test]
    async fn delete_falls_back_to_a_real_delete_when_update_is_refused() {
        // No silent no-op: a store that cannot update gets the delete it always got.
        let store = Store::new(false);
        let id = Uuid::new_v4();
        store.insert(id, "widget");

        let creator = SoftDeleteCreator::new(store.clone(), store.clone());
        creator.delete(&id).await.unwrap();

        assert!(
            store.snapshot(&id).is_none(),
            "a store that cannot update must still delete"
        );
    }

    #[tokio::test]
    async fn delete_of_an_absent_row_is_not_an_error() {
        let store = Store::new(true);
        let creator = SoftDeleteCreator::new(store.clone(), store.clone());
        creator.delete(&Uuid::new_v4()).await.unwrap();
    }
}

// `guarded_creator` only exists when one of the JSON surfaces is compiled in.
#[cfg(all(
    test,
    any(feature = "graphql", feature = "grpc", feature = "websocket")
))]
mod host_tests {
    use super::*;

    /// The guard judges policies; a deletion needs a credential that satisfies
    /// them, and this test is about the wiring, not about the policy engine.
    fn admin() -> AuthContext {
        AuthContext::Admin {
            admin_id: Uuid::new_v4(),
        }
    }
    use crate::config::LinksConfig;
    use crate::core::Module;
    use crate::core::auth::AuthContext;
    use crate::descriptor::Operation;
    use crate::server::builder::ServerBuilder;
    use crate::server::entity_registry::EntityRegistry;
    use std::collections::HashMap;
    use std::sync::Mutex;

    /// One document store, served as both fetcher and creator.
    struct Store {
        docs: Mutex<HashMap<Uuid, serde_json::Value>>,
    }

    impl Store {
        fn new() -> Arc<Self> {
            Arc::new(Self {
                docs: Mutex::new(HashMap::new()),
            })
        }
        fn insert(&self, id: Uuid, name: &str) {
            self.docs.lock().unwrap().insert(
                id,
                serde_json::json!({ "id": id.to_string(), "name": name, "deleted_at": null }),
            );
        }
        fn snapshot(&self, id: &Uuid) -> Option<serde_json::Value> {
            self.docs.lock().unwrap().get(id).cloned()
        }
    }

    #[async_trait]
    impl EntityFetcher for Store {
        async fn fetch_as_json(&self, id: &Uuid) -> Result<serde_json::Value> {
            self.snapshot(id)
                .ok_or_else(|| anyhow::anyhow!("not found"))
        }
        async fn get_sample_entity(&self) -> Result<serde_json::Value> {
            Ok(serde_json::json!({ "id": "", "name": "", "deleted_at": null }))
        }
        async fn list_as_json(
            &self,
            limit: Option<i32>,
            offset: Option<i32>,
        ) -> Result<Vec<serde_json::Value>> {
            let skip = offset.unwrap_or(0).max(0) as usize;
            let take = limit.unwrap_or(i32::MAX).max(0) as usize;
            Ok(self
                .docs
                .lock()
                .unwrap()
                .values()
                .skip(skip)
                .take(take)
                .cloned()
                .collect())
        }
    }

    #[async_trait]
    impl EntityCreator for Store {
        async fn create_from_json(&self, data: serde_json::Value) -> Result<serde_json::Value> {
            Ok(data)
        }
        async fn update_from_json(
            &self,
            id: &Uuid,
            data: serde_json::Value,
        ) -> Result<serde_json::Value> {
            let mut docs = self.docs.lock().unwrap();
            let slot = docs
                .get_mut(id)
                .ok_or_else(|| anyhow::anyhow!("not found"))?;
            *slot = data.clone();
            Ok(data)
        }
        async fn delete(&self, id: &Uuid) -> Result<()> {
            self.docs.lock().unwrap().remove(id);
            Ok(())
        }
    }

    struct MemModule {
        store: Arc<Store>,
    }

    impl Module for MemModule {
        fn name(&self) -> &str {
            "mem"
        }
        fn entity_types(&self) -> Vec<&str> {
            vec!["widget"]
        }
        fn links_config(&self) -> anyhow::Result<LinksConfig> {
            LinksConfig::from_yaml_str("entities: []\nlinks: []\n")
        }
        fn register_entities(&self, _registry: &mut EntityRegistry) {}
        fn get_entity_fetcher(&self, entity_type: &str) -> Option<Arc<dyn EntityFetcher>> {
            (entity_type == "widget").then(|| self.store.clone() as Arc<dyn EntityFetcher>)
        }
        fn get_entity_creator(&self, entity_type: &str) -> Option<Arc<dyn EntityCreator>> {
            (entity_type == "widget").then(|| self.store.clone() as Arc<dyn EntityCreator>)
        }
    }

    fn host_with(store: Arc<Store>) -> crate::server::host::ServerHost {
        ServerBuilder::new()
            .with_link_service(crate::storage::InMemoryLinkService::new())
            .register_module(MemModule { store })
            .expect("register module")
            .with_soft_delete()
            .build_host()
            .expect("build host")
    }

    #[tokio::test]
    async fn the_host_marks_on_delete_and_hides_on_read() {
        // The whole point of the feature, exercised through the host the way
        // GraphQL, gRPC and the link routes reach it.
        let store = Store::new();
        let id = Uuid::new_v4();
        store.insert(id, "widget");
        let host = host_with(store.clone());

        // Through the guarded accessors -- the path GraphQL, gRPC and the link
        // routes actually take, and the only one the structural test in
        // `exposure::guard` allows outside the guard itself.
        let creator = host
            .guarded_creator("widget", Operation::Delete, &admin())
            .expect("guarded creator");
        creator.delete(&id).await.expect("delete through the host");

        let row = store.snapshot(&id).expect("the row must still be stored");
        assert!(!row["deleted_at"].is_null(), "delete must mark: {row}");
        assert_eq!(row["name"], "widget", "the rest of the row is intact");

        let fetcher = host
            .guarded_fetcher("widget", Operation::Get, &admin())
            .expect("guarded fetcher");
        assert!(
            fetcher.fetch_as_json(&id).await.is_err(),
            "a marked row must read as absent through the host"
        );
        let listed = fetcher.list_as_json(None, None).await.unwrap();
        assert!(
            listed.is_empty(),
            "a marked row must not be listed: {listed:?}"
        );
    }

    #[tokio::test]
    async fn without_the_option_the_host_behaves_as_before() {
        // Opt-in means opt-in: the same host without `with_soft_delete()` must
        // keep the old, hard delete. Otherwise the "opt-in" claim is a lie.
        let store = Store::new();
        let id = Uuid::new_v4();
        store.insert(id, "widget");

        let host = ServerBuilder::new()
            .with_link_service(crate::storage::InMemoryLinkService::new())
            .register_module(MemModule {
                store: store.clone(),
            })
            .expect("register module")
            .build_host()
            .expect("build host");

        let creator = host
            .guarded_creator("widget", Operation::Delete, &admin())
            .expect("guarded creator");
        creator.delete(&id).await.unwrap();
        assert!(
            store.snapshot(&id).is_none(),
            "without the option, delete still removes the row"
        );
    }
}
