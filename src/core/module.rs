//! Module system for this-rs
//!
//! Defines traits for microservice modules

use crate::config::LinksConfig;
use crate::server::entity_registry::EntityRegistry;
use anyhow::Result;
use async_trait::async_trait;
use std::sync::Arc;
use uuid::Uuid;

/// Default cap of the in-memory `EntityFetcher::list_query` (rows).
pub const DEFAULT_LIST_QUERY_MAX_ROWS: usize = 10_000;

/// `THIS_LIST_QUERY_MAX_ROWS` when set and numeric, else [`DEFAULT_LIST_QUERY_MAX_ROWS`].
fn default_list_query_max_rows() -> usize {
    static CAP: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
    *CAP.get_or_init(|| {
        std::env::var("THIS_LIST_QUERY_MAX_ROWS")
            .ok()
            .and_then(|v| v.trim().parse().ok())
            .unwrap_or(DEFAULT_LIST_QUERY_MAX_ROWS)
    })
}

/// Trait for fetching entities dynamically
///
/// This allows the link system to enrich links with full entity data
/// without knowing the concrete entity types at compile time.
#[async_trait]
pub trait EntityFetcher: Send + Sync {
    /// Fetch an entity by ID and return it as JSON
    ///
    /// # Arguments
    /// * `entity_id` - The unique ID of the entity to fetch
    ///
    /// # Returns
    /// The entity serialized as JSON, or an error if not found
    async fn fetch_as_json(&self, entity_id: &Uuid) -> Result<serde_json::Value>;

    /// Get a sample entity for schema introspection
    ///
    /// This method returns an entity with all fields populated (can be dummy data)
    /// to allow the GraphQL schema generator to discover the entity structure.
    ///
    /// Default implementation returns an empty object.
    async fn get_sample_entity(&self) -> Result<serde_json::Value> {
        Ok(serde_json::json!({}))
    }

    /// List entities with pagination
    ///
    /// # Arguments
    /// * `limit` - Maximum number of entities to return
    /// * `offset` - Number of entities to skip
    ///
    /// # Returns
    /// A vector of entities serialized as JSON
    ///
    /// Default implementation returns an empty list.
    async fn list_as_json(
        &self,
        _limit: Option<i32>,
        _offset: Option<i32>,
    ) -> Result<Vec<serde_json::Value>> {
        Ok(vec![])
    }

    /// Hard cap on the rows the DEFAULT [`list_query`](Self::list_query) will load in
    /// memory. Above it the query fails with `this.query.too_large` (a typed 400 /
    /// `INVALID_ARGUMENT` / `BAD_USER_INPUT`) instead of reading a whole table.
    ///
    /// Default: [`DEFAULT_LIST_QUERY_MAX_ROWS`] (10 000), or the value of the
    /// `THIS_LIST_QUERY_MAX_ROWS` environment variable when it parses as a number (read
    /// once per process). Override this method to set a per-fetcher limit. It has no effect
    /// on a fetcher that overrides `list_query` itself.
    fn list_query_max_rows(&self) -> usize {
        default_list_query_max_rows()
    }

    /// List with a typed query (filter, sort, page), already checked against the
    /// entity's `QuerySchema` by the caller.
    ///
    /// Default: **O(n), bounded** — fetch at most [`list_query_max_rows`](Self::list_query_max_rows)
    /// rows with `list_as_json(Some(max + 1), None)`, then filter, sort and paginate in
    /// memory (filter BEFORE pagination, so pages are correct). When the source holds more
    /// than the cap, the query fails with the typed error `this.query.too_large` (a
    /// `ContractError`) rather than silently answering from a truncated table. Returns
    /// `(page, total)`.
    ///
    /// **Native overrides.** A backend that can push the query down overrides this method
    /// and translates the typed [`Query`](crate::descriptor::Query) itself: SQL (Postgres,
    /// MySQL): `WHERE` / `ORDER BY` / `LIMIT .. OFFSET` built from `query.filter`,
    /// `query.sort` and `query.page`, with bound parameters only (field names come from the
    /// already-checked `QuerySchema`, never from the client string); MongoDB: a filter
    /// document plus `sort` / `skip` / `limit`; Neo4j / Cypher: `WHERE` and `ORDER BY` /
    /// `SKIP` / `LIMIT`; ScyllaDB / DynamoDB / LMDB (no ad-hoc secondary index): keep this
    /// default, or query an index you own. Return `total` as the number of rows matching
    /// the filter BEFORE pagination.
    async fn list_query(
        &self,
        query: &crate::descriptor::Query,
    ) -> Result<(Vec<serde_json::Value>, usize)> {
        let max = self.list_query_max_rows();
        let probe = i32::try_from(max.saturating_add(1)).unwrap_or(i32::MAX);
        let all = self.list_as_json(Some(probe), None).await?;
        if all.len() > max {
            return Err(anyhow::Error::new(
                crate::descriptor::ContractError::framework(
                    crate::descriptor::framework::codes::QUERY_TOO_LARGE,
                )
                .with_param("max", max),
            ));
        }
        Ok(crate::descriptor::engine::query::apply_in_memory(
            query, all,
        ))
    }
}

/// Trait for creating entities dynamically
///
/// This allows the link system to create new entities with automatic linking
/// without knowing the concrete entity types at compile time.
#[async_trait]
pub trait EntityCreator: Send + Sync {
    /// Create a new entity from JSON data
    ///
    /// # Arguments
    /// * `entity_data` - The entity data as JSON
    ///
    /// # Returns
    /// The created entity serialized as JSON (with generated ID, timestamps, etc.)
    async fn create_from_json(&self, entity_data: serde_json::Value) -> Result<serde_json::Value>;

    /// Update an existing entity from JSON data
    ///
    /// # Arguments
    /// * `entity_id` - The ID of the entity to update
    /// * `entity_data` - The updated entity data as JSON
    ///
    /// # Returns
    /// The updated entity serialized as JSON
    ///
    /// Default implementation returns an error.
    async fn update_from_json(
        &self,
        _entity_id: &Uuid,
        _entity_data: serde_json::Value,
    ) -> Result<serde_json::Value> {
        Err(anyhow::anyhow!(
            "Update operation not implemented for this entity type"
        ))
    }

    /// Delete an entity by ID
    ///
    /// # Arguments
    /// * `entity_id` - The ID of the entity to delete
    ///
    /// # Returns
    /// Ok(()) if successful, error otherwise
    ///
    /// Default implementation returns an error.
    async fn delete(&self, _entity_id: &Uuid) -> Result<()> {
        Err(anyhow::anyhow!(
            "Delete operation not implemented for this entity type"
        ))
    }
}

/// Trait for a microservice module
pub trait Module: Send + Sync {
    /// Unique module name
    fn name(&self) -> &str;

    /// Module version
    fn version(&self) -> &str {
        "1.0.0"
    }

    /// List of entity types managed by this module
    fn entity_types(&self) -> Vec<&str>;

    /// Load links configuration
    fn links_config(&self) -> Result<LinksConfig>;

    /// Register entities with the entity registry
    ///
    /// This method should register all entity descriptors for the module.
    /// Each entity descriptor provides the CRUD routes for that entity.
    fn register_entities(&self, registry: &mut EntityRegistry);

    /// Get an entity fetcher for a specific entity type
    ///
    /// This allows the framework to dynamically load entities when enriching links.
    ///
    /// # Arguments
    /// * `entity_type` - The type of entity (e.g., "order", "invoice")
    ///
    /// # Returns
    /// An `EntityFetcher` implementation, or `None` if the entity type is not managed by this module
    ///
    /// **Défaut : `None`.** Ces deux accesseurs ne servent qu'à la surface
    /// GraphQL et à la résolution de liens par identifiant : un module qui
    /// n'expose pas ces chemins n'a rien à fournir. Les rendre obligatoires
    /// forçait chaque module — et donc chaque projet qui adopte this-rs — à
    /// écrire deux méthodes vides pour compiler.
    fn get_entity_fetcher(&self, _entity_type: &str) -> Option<Arc<dyn EntityFetcher>> {
        None
    }

    /// Get an entity creator for a specific entity type
    ///
    /// This allows the framework to create new entities dynamically when creating linked entities.
    ///
    /// # Arguments
    /// * `entity_type` - The type of entity (e.g., "order", "invoice")
    ///
    /// # Returns
    /// An `EntityCreator` implementation, or `None` if the entity type is not managed by this module
    ///
    /// **Défaut : `None`** — voir [`Module::get_entity_fetcher`].
    fn get_entity_creator(&self, _entity_type: &str) -> Option<Arc<dyn EntityCreator>> {
        None
    }

    /// Descriptor of the module (entities, fields, rules, errors, permissions, messages,
    /// dependencies), declared once.
    ///
    /// **Default: `None`** — an existing module compiles and behaves exactly as before.
    /// When `Some`, `ServerBuilder::build_host` resolves dependencies (topological,
    /// semver, cycles, duplicates), checks that `name`, `version` and `entity_types` match the
    /// descriptor, and that the plural agrees with `EntityDescriptor::plural` and
    /// `EntityConfig::plural`.
    fn descriptor(&self) -> Option<crate::descriptor::ModuleDescriptor> {
        None
    }

    /// Message bundles of the module (translations of its error codes and keys, per locale),
    /// added to `ServerHost::locales` by `ServerBuilder::build_host` so that every surface
    /// (REST, gRPC, GraphQL) renders the module's errors in the request locale, exactly as
    /// for a runtime-loaded module.
    ///
    /// **Default: empty** — an existing module is unchanged. A bundle the catalog refuses
    /// (unknown locale tag, placeholder mismatch...) fails `build_host` with the contract
    /// error, instead of being silently dropped.
    fn bundles(&self) -> Vec<crate::descriptor::MessageBundle> {
        Vec::new()
    }
}

#[cfg(test)]
mod list_query_tests {
    use super::*;
    use crate::descriptor::{Page, Query};
    use serde_json::json;

    /// Source of `rows` rows whose `list_as_json` honours `limit`; cap set per fetcher.
    struct Rows {
        rows: usize,
        cap: usize,
    }

    #[async_trait]
    impl EntityFetcher for Rows {
        async fn fetch_as_json(&self, _: &Uuid) -> Result<serde_json::Value> {
            Ok(json!({}))
        }
        async fn list_as_json(
            &self,
            limit: Option<i32>,
            _: Option<i32>,
        ) -> Result<Vec<serde_json::Value>> {
            let take = limit.map_or(self.rows, |l| (l.max(0) as usize).min(self.rows));
            Ok((0..take).map(|i| json!({ "n": i })).collect())
        }
        fn list_query_max_rows(&self) -> usize {
            self.cap
        }
    }

    #[tokio::test]
    async fn list_query_default_refuses_a_table_above_the_cap_with_a_typed_error() {
        let f = Rows { rows: 11, cap: 10 };
        let err = f.list_query(&Query::new()).await.expect_err("too large");
        let c = crate::core::errors::from_anyhow(&err);
        assert_eq!(c.code.as_str(), "this.query.too_large");
        assert_eq!(c.kind, crate::descriptor::ErrorKind::BadRequest);
    }

    #[tokio::test]
    async fn list_query_default_serves_a_table_at_the_cap_and_paginates_after_filtering() {
        let f = Rows { rows: 10, cap: 10 };
        let mut q = Query::new();
        q.page = Page::Cursor {
            after: Some("8".into()),
            limit: 5,
        };
        let (page, total) = f.list_query(&q).await.expect("at the cap is served");
        assert_eq!((page.len(), total), (2, 10));
    }

    #[tokio::test]
    async fn list_query_default_asks_the_source_for_at_most_cap_plus_one_rows() {
        // The source would hold a million rows: the default must not load them all.
        let f = Rows {
            rows: 1_000_000,
            cap: 5,
        };
        let err = f.list_query(&Query::new()).await.expect_err("too large");
        assert_eq!(
            crate::core::errors::from_anyhow(&err).code.as_str(),
            "this.query.too_large"
        );
        assert_eq!(f.list_as_json(Some(6), None).await.unwrap().len(), 6);
    }

    #[test]
    fn list_query_default_cap_is_ten_thousand_without_override() {
        struct Plain;
        #[async_trait]
        impl EntityFetcher for Plain {
            async fn fetch_as_json(&self, _: &Uuid) -> Result<serde_json::Value> {
                Ok(json!({}))
            }
        }
        // Honours the documented env override only when it is set in this process.
        if std::env::var("THIS_LIST_QUERY_MAX_ROWS").is_err() {
            assert_eq!(Plain.list_query_max_rows(), 10_000);
        }
    }
}
