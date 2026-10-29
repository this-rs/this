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
    // Cookie identity is ambient: only from a listed `Origin` (cross-site WebSocket
    // hijacking). Explicit tokens (header, subprotocol, query) are not affected.
    if let Some((crate::core::auth::HandshakeSource::Cookie, _)) =
        crate::core::auth::handshake_token(headers, query)
        && !crate::core::auth::origin_allowed(headers, &host.security.websocket_origins)
    {
        tracing::warn!(
            "WebSocket handshake: session cookie from an unlisted or absent Origin: refused"
        );
        return Err(ContractError::framework(codes::AUTH_FORBIDDEN)
            .with_param("operation", "websocket")
            .with_param("reason", "origin not allowed for cookie identity"));
    }
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

struct Enforced {
    index: PolicyIndex,
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
        let catalog = host.catalog.load();
        let index = PolicyIndex::new(catalog.descriptors.clone(), catalog.links_config.clone());
        Self {
            ctx,
            enforced: Some(Arc::new(Enforced {
                index,
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
            authorize_op(&e.index, entity, &op, &self.ctx, None, resource, env)
        });
        self.outcome(e, res)
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
        let policies = e.index.link_policies(link_type, source, target, scope, &op);
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
}
