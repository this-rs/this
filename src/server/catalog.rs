//! Live host catalog: everything that a runtime module load can change, in ONE immutable
//! snapshot, published with one atomic store.
//!
//! # Why (RFC module-descriptor, objection "the host is a frozen snapshot")
//!
//! `ServerHost::entity_fetchers`, `entity_creators`, `config` and the plurals frozen by
//! `Authorization::new` are built once. A module loaded at runtime would only reach the
//! surfaces that read something rebuilt (REST routes), while GraphQL, gRPC, link handlers
//! and the authorization middleware keep the old view: a 403 on every route of the new
//! module with armed policies, `NOT_FOUND` on GraphQL/gRPC, and two diverging sources of
//! truth.
//!
//! The foundation therefore provides `HostCatalog` (immutable) and `LiveCatalog`
//! (`ArcSwap`): readers call `LiveCatalog::load` once per request and keep a consistent
//! view (no lock, no guard held across an `.await`); `ModuleRuntime` builds the next
//! snapshot (creators already wrapped in `ValidatingCreator`) and publishes it with
//! `LiveCatalog::publish`.
//!
//! # Migration contract (who reads it)
//!
//! The legacy fields of `ServerHost` stay and equal the INITIAL snapshot, so nothing
//! changes in this batch for hosts without runtime modules. The streams migrate readers:
//! `auth` (Authorization, GraphQL/gRPC guards through `guarded_fetcher`), `validation`
//! (wrapping at snapshot build), `modules-runtime` (link handlers, publication). Until
//! then, runtime loading is limited to what reads the catalog, and the RFC says so.

use crate::config::LinksConfig;
use crate::core::module::{EntityCreator, EntityFetcher};
use crate::descriptor::DescriptorSet;
use arc_swap::ArcSwap;
use std::collections::HashMap;
use std::sync::Arc;

/// One consistent view of the host's modules.
#[derive(Clone)]
pub struct HostCatalog {
    /// Active descriptors (empty when no module provides one).
    pub descriptors: Arc<DescriptorSet>,
    /// Entity type -> fetcher.
    pub fetchers: Arc<HashMap<String, Arc<dyn EntityFetcher>>>,
    /// Entity type -> creator (wrapped in `ValidatingCreator` when the entity has a
    /// descriptor: validation stream).
    pub creators: Arc<HashMap<String, Arc<dyn EntityCreator>>>,
    /// Effective (merged) links configuration.
    pub links_config: Arc<LinksConfig>,
    /// `(entity_type, plural)` of every served entity.
    pub plurals: Arc<Vec<(String, String)>>,
    /// Incremented on every publication: key of derived caches (SDL, proto).
    pub generation: u64,
}

impl std::fmt::Debug for HostCatalog {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HostCatalog")
            .field(
                "modules",
                &self
                    .descriptors
                    .modules()
                    .map(|m| m.name.clone())
                    .collect::<Vec<_>>(),
            )
            .field("fetchers", &self.fetchers.keys().collect::<Vec<_>>())
            .field("creators", &self.creators.keys().collect::<Vec<_>>())
            .field("plurals", &self.plurals)
            .field("generation", &self.generation)
            .finish()
    }
}

impl HostCatalog {
    /// Plural of an entity type.
    pub fn plural_of(&self, entity: &str) -> Option<&str> {
        self.plurals
            .iter()
            .find(|(e, _)| e == entity)
            .map(|(_, p)| p.as_str())
    }
    /// Every served entity type (sorted), hot-loaded modules included. The legacy
    /// `ServerHost::entity_types` reads the frozen registry and misses them.
    pub fn entity_types(&self) -> Vec<&str> {
        self.plurals.iter().map(|(e, _)| e.as_str()).collect()
    }
    /// Entity type whose plural is `plural` (first path segment dispatch).
    pub fn entity_of_plural(&self, plural: &str) -> Option<&str> {
        self.plurals
            .iter()
            .find(|(_, p)| p == plural)
            .map(|(e, _)| e.as_str())
    }
}

/// The published snapshot. Cheap to read (`load` = one atomic load).
pub struct LiveCatalog {
    current: ArcSwap<HostCatalog>,
    /// Texts derived from a snapshot (SDL, proto), keyed by name and valid for ONE
    /// generation: a publication invalidates them by construction.
    derived: std::sync::Mutex<HashMap<&'static str, (u64, Arc<String>)>>,
}

impl std::fmt::Debug for LiveCatalog {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Debug::fmt(&**self.current.load(), f)
    }
}

impl LiveCatalog {
    pub fn new(initial: HostCatalog) -> Self {
        Self {
            current: ArcSwap::from_pointee(initial),
            derived: std::sync::Mutex::new(HashMap::new()),
        }
    }

    /// Current snapshot. Keep it for the whole request.
    pub fn load(&self) -> Arc<HostCatalog> {
        self.current.load_full()
    }

    /// Derived text of `key` computed for exactly `generation`, if cached.
    pub fn derived(&self, key: &'static str, generation: u64) -> Option<Arc<String>> {
        let map = self.derived.lock().unwrap_or_else(|e| e.into_inner());
        map.get(key)
            .filter(|(g, _)| *g == generation)
            .map(|(_, v)| v.clone())
    }

    /// Cache the derived text of `key` for `generation` (a newer generation wins).
    pub fn store_derived(&self, key: &'static str, generation: u64, value: Arc<String>) {
        let mut map = self.derived.lock().unwrap_or_else(|e| e.into_inner());
        if map.get(key).is_none_or(|(g, _)| *g <= generation) {
            map.insert(key, (generation, value));
        }
    }

    /// Publish the next snapshot, built from the current one. `generation` is set here.
    /// Concurrent publishers: last-writer-wins on the WHOLE snapshot, so the runtime
    /// serializes its loads (one `ModuleRuntime` per host).
    pub fn publish(&self, mut next: HostCatalog) -> Arc<HostCatalog> {
        next.generation = self.current.load().generation + 1;
        let next = Arc::new(next);
        self.current.store(next.clone());
        next
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn empty() -> HostCatalog {
        HostCatalog {
            descriptors: Arc::new(DescriptorSet::new()),
            fetchers: Arc::new(HashMap::new()),
            creators: Arc::new(HashMap::new()),
            links_config: Arc::new(LinksConfig {
                entities: vec![],
                links: vec![],
                validation_rules: None,
                events: None,
                sinks: None,
                auth: None,
            }),
            plurals: Arc::new(vec![("invoice".into(), "invoices".into())]),
            generation: 0,
        }
    }

    #[test]
    fn readers_keep_their_snapshot_across_publication() {
        let live = LiveCatalog::new(empty());
        let before = live.load();
        let mut next = (*before).clone();
        next.plurals = Arc::new(vec![("order".into(), "orders".into())]);
        live.publish(next);
        assert_eq!(before.entity_of_plural("invoices"), Some("invoice"));
        let after = live.load();
        assert_eq!(after.generation, 1);
        assert_eq!(after.entity_of_plural("orders"), Some("order"));
        assert_eq!(after.plural_of("invoice"), None);
    }
}
