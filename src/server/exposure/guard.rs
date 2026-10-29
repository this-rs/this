//! `SurfaceGuard`: per-operation authorization of the non-REST surfaces.
//!
//! One guard per request (GraphQL), per call (gRPC) or per connection (WebSocket
//! subscriptions). It binds the caller's [`AuthContext`] to a snapshot of the live
//! catalog ([`PolicyIndex`]: descriptors + YAML `auth:`) and to the host's evaluation
//! environment (named resolvers, materialized IAM for `perm:` with feature `obrain`),
//! then answers `check(entity, Operation)` with the single evaluator
//! (`core::auth::surface::authorize_op`).
//!
//! - Policies DISARMED (`ServerBuilder::without_authorization`) or `*_unenforced`
//!   routers: the guard is *unenforced* and allows everything (historical behavior).
//! - Report-only mode (`authorization_report_only`): a refusal is logged and allowed,
//!   exactly like the REST middleware.
//! - Identity: [`resolve_identity`] applies the rule of the REST `auth_middleware`
//!   (provider error → anonymous only if `auth.default_policy` is `public`, else
//!   `this.auth.unauthenticated`).

// `ContractError` is the cross-surface contract (same allowance as this-descriptor).
#![allow(clippy::result_large_err)]

use std::sync::Arc;

use axum::http::HeaderMap;
use uuid::Uuid;

#[cfg(feature = "obrain")]
use crate::core::auth::descriptor_policy::PermissionFn;
use crate::core::auth::descriptor_policy::RegistryEnv;
use crate::core::auth::surface::{PolicyIndex, authorize_link_op, authorize_op};
use crate::core::auth::{AuthContext, AuthResolverRegistry};
use crate::core::events::{EntityEvent, FrameworkEvent, LinkEvent};
use crate::core::{EntityCreator, EntityFetcher};
use crate::descriptor::framework::codes;
use crate::descriptor::{ContractError, Operation};
use crate::server::host::ServerHost;

/// Resolve the caller of a non-REST request from its HTTP headers (and the query
/// string of a WebSocket handshake). Same rule as the REST `auth_middleware`.
pub async fn resolve_identity(
    host: &ServerHost,
    headers: &HeaderMap,
    query: Option<&str>,
) -> Result<AuthContext, ContractError> {
    let Some(provider) = host.auth_provider.as_ref() else {
        return Ok(AuthContext::Anonymous);
    };
    match provider
        .extract_context_from_handshake(headers, query)
        .await
    {
        Ok(ctx) => Ok(ctx),
        Err(e) => identity_failure(host, &e),
    }
}

/// What a provider failure becomes (shared with the gRPC identity layer).
pub(crate) fn identity_failure(
    host: &ServerHost,
    err: &anyhow::Error,
) -> Result<AuthContext, ContractError> {
    let default_policy = host
        .config
        .auth
        .as_ref()
        .map(|a| a.default_policy.as_str())
        .unwrap_or("public");
    if default_policy == "public" {
        Ok(AuthContext::Anonymous)
    } else {
        tracing::debug!(error = %err, "non-REST surface: authentication failed");
        Err(ContractError::framework(codes::AUTH_UNAUTHENTICATED))
    }
}

impl Enforced {
    fn index(&self) -> PolicyIndex {
        let catalog = self.catalog.load();
        PolicyIndex::new(catalog.descriptors.clone(), catalog.links_config.clone())
    }
}

fn unknown_entity(entity: &str) -> ContractError {
    crate::core::errors::not_found(entity, format!("entity type '{entity}' not registered"))
}

/// Policies are read from the LIVE catalog at each check (not frozen at guard creation): a
/// long-lived WebSocket connection judges a module loaded after it connected, and a module
/// unloaded since is no longer served.
struct Enforced {
    catalog: Arc<crate::server::catalog::LiveCatalog>,
    resolvers: Arc<AuthResolverRegistry>,
    report_only: bool,
    surface: &'static str,
    #[cfg(feature = "obrain")]
    iam: Option<(
        crate::storage::iam::Iam,
        Option<Arc<crate::server::middleware::authorization::PrincipalMapping>>,
    )>,
}

/// Per-request guard (cheap to clone).
#[derive(Clone)]
pub struct SurfaceGuard {
    ctx: AuthContext,
    enforced: Option<Arc<Enforced>>,
}

impl std::fmt::Debug for SurfaceGuard {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SurfaceGuard")
            .field("enforced", &self.enforced.is_some())
            .finish()
    }
}

impl SurfaceGuard {
    /// Guard that allows everything (policies disarmed or explicit `*_unenforced`).
    pub fn unenforced(ctx: AuthContext) -> Self {
        Self {
            ctx,
            enforced: None,
        }
    }

    /// Guard of `host` for `ctx`: enforced iff the host's policies are armed.
    pub fn for_host(host: &ServerHost, ctx: AuthContext, surface: &'static str) -> Self {
        if !host.authorize {
            return Self::unenforced(ctx);
        }
        Self {
            ctx,
            enforced: Some(Arc::new(Enforced {
                catalog: host.catalog.clone(),
                resolvers: host.resolver_registry.clone(),
                report_only: host.authorize_report_only,
                surface,
                #[cfg(feature = "obrain")]
                iam: host
                    .graph_store
                    .as_ref()
                    .map(|s| (s.iam(), host.principal_mapping.clone())),
            })),
        }
    }

    pub fn is_enforced(&self) -> bool {
        self.enforced.is_some()
    }

    pub fn context(&self) -> &AuthContext {
        &self.ctx
    }

    fn outcome(&self, e: &Enforced, res: Result<(), ContractError>) -> Result<(), ContractError> {
        match res {
            Ok(()) => Ok(()),
            Err(err) if e.report_only => {
                tracing::warn!(
                    surface = e.surface,
                    code = %err.code,
                    params = ?err.params,
                    "authorization: refusal OBSERVED (report-only mode, the call proceeds)"
                );
                Ok(())
            }
            Err(err) => {
                tracing::debug!(surface = e.surface, params = ?err.params, "authorization refused");
                Err(err)
            }
        }
    }

    fn with_env<R>(
        &self,
        e: &Enforced,
        resource: Option<&Uuid>,
        f: impl FnOnce(&RegistryEnv<'_>) -> R,
    ) -> R {
        #[cfg(feature = "obrain")]
        {
            if let Some((iam, mapping)) = &e.iam {
                let identity = match mapping {
                    Some(m) => m(&self.ctx),
                    None => crate::server::middleware::authorization::Authorization::default_iam_identity(
                        &self.ctx,
                    ),
                };
                let perm = move |p: &str| -> Option<bool> {
                    let (principal, scope) = identity.as_ref()?;
                    // A failed IAM read denies (same as the REST middleware).
                    Some(iam.allows(principal, scope, p).unwrap_or(false))
                };
                let perm_ref: &PermissionFn<'_> = &perm;
                let env = RegistryEnv::new(Some(&e.resolvers), &self.ctx)
                    .with_resource(resource)
                    .with_permission(perm_ref);
                return f(&env);
            }
        }
        let env = RegistryEnv::new(Some(&e.resolvers), &self.ctx).with_resource(resource);
        f(&env)
    }

    /// May the caller run `op` on `entity` (optionally on one resource)?
    pub fn check(
        &self,
        entity: &str,
        op: Operation,
        resource: Option<&Uuid>,
    ) -> Result<(), ContractError> {
        let Some(e) = &self.enforced else {
            return Ok(());
        };
        let res = self.with_env(e, resource, |env| {
            authorize_op(&e.index(), entity, &op, &self.ctx, None, env)
        });
        self.outcome(e, res)
    }

    /// THE access to the fetcher of `entity` for a caller-driven operation: the policy of
    /// `op` is judged first (descriptor, then YAML `auth:`), then the fetcher is read from
    /// the LIVE catalog (hot-loaded modules included). Unknown entity: `ENTITY_NOT_FOUND`.
    pub fn fetcher(
        &self,
        host: &ServerHost,
        entity: &str,
        op: Operation,
        resource: Option<&Uuid>,
    ) -> Result<Arc<dyn EntityFetcher>, ContractError> {
        self.check(entity, op, resource)?;
        host.catalog
            .load()
            .fetchers
            .get(entity)
            .cloned()
            .ok_or_else(|| unknown_entity(entity))
    }

    /// Creator counterpart of [`SurfaceGuard::fetcher`].
    pub fn creator(
        &self,
        host: &ServerHost,
        entity: &str,
        op: Operation,
        resource: Option<&Uuid>,
    ) -> Result<Arc<dyn EntityCreator>, ContractError> {
        self.check(entity, op, resource)?;
        host.catalog
            .load()
            .creators
            .get(entity)
            .cloned()
            .ok_or_else(|| unknown_entity(entity))
    }

    /// Fetchers (from the live catalog) of every entity type whose `get` policy allows
    /// the caller to read `resource`: enrichment of a link by id across types.
    pub fn readable_fetchers(
        &self,
        host: &ServerHost,
        resource: &Uuid,
    ) -> Vec<Arc<dyn EntityFetcher>> {
        let catalog = host.catalog.load();
        catalog
            .fetchers
            .iter()
            .filter(|(entity, _)| self.check(entity, Operation::Get, Some(resource)).is_ok())
            .map(|(_, f)| f.clone())
            .collect()
    }

    /// May the caller run the link operation `op`? Every matching declaration must
    /// allow; an undeclared link type (no definition, no `scope`) is refused.
    pub fn check_link(
        &self,
        link_type: Option<&str>,
        source: Option<&str>,
        target: Option<&str>,
        scope: Option<&str>,
        op: Operation,
    ) -> Result<(), ContractError> {
        let Some(e) = &self.enforced else {
            return Ok(());
        };
        let policies = e
            .index()
            .link_policies(link_type, source, target, scope, &op);
        let res = self.with_env(e, None, |env| authorize_link_op(&policies, &self.ctx, env));
        self.outcome(e, res)
    }

    /// A part of the surface that does not know how to apply policies: refused when
    /// enforced (fail-closed), allowed otherwise.
    pub fn deny_uncovered(&self, what: &str) -> Result<(), ContractError> {
        let Some(e) = &self.enforced else {
            return Ok(());
        };
        let res = Err(ContractError::framework(codes::AUTH_FORBIDDEN)
            .with_param("operation", what)
            .with_param("reason", "not covered by policies on this surface"));
        self.outcome(e, res)
    }

    /// May this event be delivered to the caller? Entity events need `get` on their
    /// entity, link events `get_link` on their link type; other events (cognitive
    /// signals, GDPR erasure) are not entity data and keep their tenant scoping.
    pub fn allows_event(&self, event: &FrameworkEvent) -> bool {
        if self.enforced.is_none() {
            return true;
        }
        match event {
            FrameworkEvent::Entity(
                EntityEvent::Created {
                    entity_type,
                    entity_id,
                    ..
                }
                | EntityEvent::Updated {
                    entity_type,
                    entity_id,
                    ..
                }
                | EntityEvent::Deleted {
                    entity_type,
                    entity_id,
                },
            ) => self
                .check(entity_type, Operation::Get, Some(entity_id))
                .is_ok(),
            FrameworkEvent::Link(
                LinkEvent::Created { link_type, .. } | LinkEvent::Deleted { link_type, .. },
            ) => self
                .check_link(Some(link_type), None, None, None, Operation::GetLink)
                .is_ok(),
            _ => true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{EntityAuthConfig, EntityConfig, LinksConfig};
    use crate::server::entity_registry::EntityRegistry;
    use crate::storage::InMemoryLinkService;
    use std::collections::HashMap;

    pub(crate) fn armed_host(create: &str) -> ServerHost {
        let config = LinksConfig {
            entities: vec![EntityConfig {
                singular: "order".into(),
                plural: "orders".into(),
                auth: EntityAuthConfig {
                    create: create.into(),
                    ..Default::default()
                },
            }],
            links: vec![],
            validation_rules: None,
            events: None,
            sinks: None,
            auth: None,
        };
        ServerHost::from_builder_components(
            Arc::new(InMemoryLinkService::new()),
            config,
            EntityRegistry::new(),
            HashMap::new(),
            HashMap::new(),
        )
        .unwrap()
    }

    #[test]
    fn guard_follows_arming_and_report_only() {
        let host = armed_host("role:admin");
        let anon = SurfaceGuard::for_host(&host, AuthContext::Anonymous, "test");
        assert!(anon.is_enforced());
        let err = anon.check("order", Operation::Create, None).unwrap_err();
        assert_eq!(err.code.as_str(), codes::AUTH_FORBIDDEN);

        let mut observed = armed_host("role:admin");
        observed.authorize_report_only = true;
        let g = SurfaceGuard::for_host(&observed, AuthContext::Anonymous, "test");
        assert!(g.check("order", Operation::Create, None).is_ok());

        let mut disarmed = armed_host("role:admin");
        disarmed.authorize = false;
        let g = SurfaceGuard::for_host(&disarmed, AuthContext::Anonymous, "test");
        assert!(!g.is_enforced());
        assert!(g.check("order", Operation::Create, None).is_ok());
    }

    #[test]
    fn events_are_filtered_by_get_policy() {
        let mut host = armed_host("public");
        let mut cfg = (*host.config).clone();
        cfg.entities[0].auth.get = "role:reader".into();
        host.config = Arc::new(cfg.clone());
        host.catalog.publish({
            let mut c = (*host.catalog.load()).clone();
            c.links_config = Arc::new(cfg);
            c
        });
        let ev = FrameworkEvent::Entity(EntityEvent::Deleted {
            entity_type: "order".into(),
            entity_id: Uuid::nil(),
        });
        let anon = SurfaceGuard::for_host(&host, AuthContext::Anonymous, "test");
        assert!(!anon.allows_event(&ev));
        let reader = SurfaceGuard::for_host(
            &host,
            AuthContext::User {
                user_id: Uuid::nil(),
                tenant_id: Uuid::nil(),
                roles: vec!["reader".into()],
            },
            "test",
        );
        assert!(reader.allows_event(&ev));
    }

    // ── coverage: the guard is the ONLY way to a fetcher / creator ─────────────

    /// Files that may read a fetcher / creator map directly, each with the reason. Any other
    /// file under `src/` must go through `ServerHost::guarded_fetcher` /
    /// `guarded_creator` or `SurfaceGuard::{fetcher, creator, readable_fetchers}`.
    const DIRECT_ACCESS_ALLOWED: &[(&str, &str)] = &[
        (
            "src/server/exposure/guard.rs",
            "the guard itself: policy check, then lookup in the live catalog",
        ),
        (
            "src/server/host.rs",
            "defines the maps and the guarded accessors; `schema_fetcher` (schema derivation \
             for SDL / proto: no caller, only field names and types are inferred); unit tests",
        ),
        (
            "src/core/validation/gate.rs",
            "`wrap_creators` re-wraps every creator in `ValidatingCreator` when a snapshot is \
             built (no caller, no data read or written)",
        ),
        (
            "src/links/handlers.rs",
            "REST link handlers: the link policy is judged by the authorization middleware \
             BEFORE the handler runs, and `AppState` is rebuilt from the published catalog by \
             the module runtime (REST is not a SurfaceGuard surface)",
        ),
        (
            "src/events/operators/resolve.rs",
            "in-process flow operator (event pipeline): no caller, the event is already \
             emitted; reads the fetchers the FlowRuntime was built with",
        ),
        (
            "src/events/operators/fan_out.rs",
            "in-process flow operator (event pipeline): same as `resolve`",
        ),
    ];

    fn rust_files(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                rust_files(&path, out);
            } else if path.extension().is_some_and(|e| e == "rs") {
                out.push(path);
            }
        }
    }

    /// Offending `(file, pattern)` pairs among `files` (path, content).
    fn direct_accesses(files: &[(String, String)]) -> Vec<(String, String)> {
        let patterns: Vec<String> = ["fetchers", "creators"]
            .iter()
            .flat_map(|m| [".get(", ".iter(", ".values(", "["].map(|a| format!("{m}{a}")))
            .collect();
        let mut found = vec![];
        for (path, content) in files {
            if DIRECT_ACCESS_ALLOWED.iter().any(|(p, _)| path.ends_with(p)) {
                continue;
            }
            // Comments dropped, whitespace removed: catches `.entity_fetchers\n    .get(`.
            let code: String = content
                .lines()
                .filter(|l| !l.trim_start().starts_with("//"))
                .flat_map(|l| l.chars().filter(|c| !c.is_whitespace()))
                .collect();
            for p in &patterns {
                if code.contains(p.as_str()) {
                    found.push((path.clone(), p.clone()));
                }
            }
        }
        found
    }

    fn load_sources() -> Vec<(String, String)> {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut paths = vec![];
        rust_files(&root, &mut paths);
        paths
            .into_iter()
            .map(|p| {
                let content = std::fs::read_to_string(&p).unwrap();
                (p.to_string_lossy().replace('\\', "/"), content)
            })
            .collect()
    }

    #[test]
    fn no_entity_fetchers_access_outside_guard() {
        let sources = load_sources();
        assert!(sources.len() > 50, "the scan must see the source tree");
        let found = direct_accesses(&sources);
        assert!(
            found.is_empty(),
            "direct fetcher/creator access outside the guard and its allow-list \
             (use ServerHost::guarded_fetcher / SurfaceGuard::fetcher): {found:#?}"
        );
        // The allow-list stays honest: every entry exists.
        for (p, _) in DIRECT_ACCESS_ALLOWED {
            assert!(sources.iter().any(|(path, _)| path.ends_with(p)), "{p}");
        }
    }

    #[test]
    fn coverage_scan_detects_a_direct_access() {
        // The scanner itself is checked: single line, multi-line (rustfmt layout), indexing.
        let files = |code: &str| vec![("src/server/exposure/x.rs".to_string(), code.to_string())];
        assert!(!direct_accesses(&files("host.entity_fetchers.get(t)")).is_empty());
        assert!(!direct_accesses(&files("host\n    .entity_creators\n    .get(t)")).is_empty());
        assert!(!direct_accesses(&files("host.catalog.load().fetchers[\"a\"]")).is_empty());
        assert!(direct_accesses(&files("// host.entity_fetchers.get(t)")).is_empty());
        assert!(direct_accesses(&files("guard.fetcher(host, t, op, None)")).is_empty());
    }

    // ── single guard: policy first, then the LIVE catalog ──────────────────────

    #[tokio::test]
    async fn guarded_fetcher_judges_policy_then_reads_live_catalog() {
        use crate::core::EntityFetcher;
        struct F;
        #[async_trait::async_trait]
        impl EntityFetcher for F {
            async fn fetch_as_json(&self, _: &Uuid) -> anyhow::Result<serde_json::Value> {
                Ok(serde_json::json!({}))
            }
        }
        let mut host = armed_host("public");
        let mut cfg = (*host.config).clone();
        cfg.entities[0].auth.get = "role:reader".into();
        host.config = Arc::new(cfg.clone());
        host.catalog.publish({
            let mut c = (*host.catalog.load()).clone();
            c.links_config = Arc::new(cfg);
            c
        });
        let reader = AuthContext::User {
            user_id: Uuid::nil(),
            tenant_id: Uuid::nil(),
            roles: vec!["reader".into()],
        };
        // Refused for anonymous BEFORE any lookup (even though no fetcher exists yet).
        let err = host
            .guarded_fetcher("order", Operation::Get, &AuthContext::Anonymous)
            .err()
            .expect("refused");
        assert_eq!(err.code.as_str(), codes::AUTH_FORBIDDEN);
        // Allowed caller, entity not in the catalog yet: not found.
        let err = host
            .guarded_fetcher("order", Operation::Get, &reader)
            .err()
            .expect("unknown");
        assert_eq!(err.code.as_str(), codes::ENTITY_NOT_FOUND);
        // Published afterwards (hot load): served, with no change to `host.entity_fetchers`.
        host.catalog.publish({
            let mut c = (*host.catalog.load()).clone();
            let mut m = (*c.fetchers).clone();
            m.insert("order".into(), Arc::new(F));
            c.fetchers = Arc::new(m);
            c
        });
        assert!(host.entity_fetchers.is_empty(), "frozen field unchanged");
        assert!(
            host.guarded_fetcher("order", Operation::Get, &reader)
                .is_ok()
        );
        assert!(
            host.guarded_fetcher("order", Operation::Get, &AuthContext::Anonymous)
                .is_err()
        );
    }

    #[test]
    fn long_lived_guard_judges_a_module_published_after_it_was_built() {
        // WebSocket / GraphQL subscription: ONE guard per connection, built at connect time.
        let host = armed_host("public");
        let guard = SurfaceGuard::for_host(&host, AuthContext::Anonymous, "WebSocket");
        let ev = FrameworkEvent::Entity(EntityEvent::Deleted {
            entity_type: "vault".into(),
            entity_id: Uuid::nil(),
        });
        let base = (*host.catalog.load().links_config).clone();
        let publish = |get: Option<&str>| {
            let mut cfg = base.clone();
            if let Some(get) = get {
                cfg.entities.push(EntityConfig {
                    singular: "vault".into(),
                    plural: "vaults".into(),
                    auth: EntityAuthConfig {
                        get: get.into(),
                        ..Default::default()
                    },
                });
            }
            host.catalog.publish({
                let mut c = (*host.catalog.load()).clone();
                c.links_config = Arc::new(cfg);
                c
            });
        };
        // Module published AFTER the connection, `get` public: its events are delivered
        // (a guard frozen at connect time would not know `vault`).
        publish(Some("public"));
        assert!(
            guard.allows_event(&ev),
            "the connection's guard must see the module loaded after it connected"
        );
        // Module replaced by a stricter version: refused from the next event on.
        publish(Some("role:admin"));
        assert!(
            !guard.allows_event(&ev),
            "new policy applies to the open connection"
        );
        // Module unloaded: the entity is undeclared again, events are not delivered.
        publish(None);
        assert!(!guard.allows_event(&ev), "unloaded module no longer served");
    }
}
