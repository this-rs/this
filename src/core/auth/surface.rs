//! Per-operation authorization for every surface (REST, GraphQL, gRPC, WebSocket).
//!
//! [`PolicyIndex`] resolves the policy of `(entity, Operation)` — or of a link
//! operation — from the published descriptors AND the YAML `auth:` tables, in ONE
//! place; [`authorize_op`] evaluates it with the single three-valued evaluator
//! (`engine::policy`). No dependency on `server/` (RFC A16): the host-bound guard
//! (`server::exposure::SurfaceGuard`) builds the index from the live catalog.
//!
//! # Precedence (entity)
//! 1. `EntityDesc.permissions` explicit for the operation (descriptor);
//! 2. the YAML `auth:` entry of the entity (by singular, then plural) — what the REST
//!    middleware applies — parsed fail-closed (unknown token = refusal);
//! 3. `PermDesc::default_policy` (`authenticated`, `deny` for business operations).
//!
//! # Precedence (link)
//! 1. `LinkDesc.permissions` explicit; 2. YAML `LinkDefinition.auth`; 3. the scope
//! entity (source in forward direction), as above. A partial link table does not
//! complete itself with `authenticated` (RFC §3.6).
//!
//! # Owner
//! `owner` is satisfied by an `AuthContext::Owner` (same rule as the REST middleware),
//! except for `List`, where ownership is unknown before rows are read: `owner` is then
//! unevaluable and refuses (RFC §3.6), unless the caller passes an explicit verdict.
//!
//! # Errors
//! A refusal is `this.auth.forbidden` (403 / `PERMISSION_DENIED` / `FORBIDDEN`) for
//! every caller, anonymous included — the same status as the REST middleware. An
//! unreadable declared policy is also a refusal (`reason` names it).

use std::collections::BTreeMap;
use std::sync::Arc;

use super::AuthContext;
use super::descriptor_policy::{Principal, entity_auth_field, parse_declared};
use crate::config::LinksConfig;
use crate::descriptor::engine::policy::{PolicyEnv, evaluate};
use crate::descriptor::framework::codes;
use crate::descriptor::{ContractError, DescriptorSet, Operation, PermDesc, PolicyExpr};

/// Where a resolved policy comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PolicySource {
    Descriptor,
    Config,
    LinkDescriptor,
    LinkConfig,
    Default,
}

/// A policy resolved for one `(scope, operation)`.
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedPolicy {
    /// `Entity · operation` or `link_type(source→target) · operation`.
    pub scope: String,
    pub operation: Operation,
    /// `Err(raw)`: the declared text does not parse — refusal.
    pub policy: Result<PolicyExpr, String>,
    pub source: PolicySource,
}

/// Snapshot of every declared policy (descriptors + YAML), cheap to clone.
#[derive(Clone)]
pub struct PolicyIndex {
    descriptors: Arc<DescriptorSet>,
    config: Arc<LinksConfig>,
}

impl PolicyIndex {
    pub fn new(descriptors: Arc<DescriptorSet>, config: Arc<LinksConfig>) -> Self {
        Self {
            descriptors,
            config,
        }
    }

    fn declared(raw: &str) -> Result<PolicyExpr, String> {
        parse_declared(raw).map_err(|_| raw.to_string())
    }

    /// Policy of an entity operation.
    pub fn entity_policy(&self, entity: &str, op: &Operation) -> ResolvedPolicy {
        let scope = format!("{entity} · {op}");
        let desc = self.descriptors.entity(entity).map(|(_, e)| e).or_else(|| {
            self.descriptors
                .entity_by_plural(entity)
                .and_then(|s| self.descriptors.entity(s))
                .map(|(_, e)| e)
        });
        if let Some(p) = desc.and_then(|d| d.permissions.policy(op)) {
            return ResolvedPolicy {
                scope,
                operation: op.clone(),
                policy: Ok(p.clone()),
                source: PolicySource::Descriptor,
            };
        }
        let yaml = self
            .config
            .entities
            .iter()
            .find(|e| e.singular == entity)
            .or_else(|| self.config.entities.iter().find(|e| e.plural == entity));
        if let Some(raw) = yaml.and_then(|e| entity_auth_field(&e.auth, op)) {
            return ResolvedPolicy {
                scope,
                operation: op.clone(),
                policy: Self::declared(raw),
                source: PolicySource::Config,
            };
        }
        ResolvedPolicy {
            scope,
            operation: op.clone(),
            policy: Ok(PermDesc::default_policy(op)),
            source: PolicySource::Default,
        }
    }

    /// Policies of a link operation: one per link definition matching `link_type` (and
    /// `source` / `target` when known). The caller must satisfy ALL of them (an
    /// ambiguous link type is judged by its strictest declaration). No definition at
    /// all: the policy of `scope` (the entity the request is anchored on) if given,
    /// else an EMPTY list, which the guard refuses (undeclared link type).
    pub fn link_policies(
        &self,
        link_type: Option<&str>,
        source: Option<&str>,
        target: Option<&str>,
        scope: Option<&str>,
        op: &Operation,
    ) -> Vec<ResolvedPolicy> {
        let matches = |lt: &str, s: &str, t: &str| {
            link_type.is_none_or(|x| x == lt)
                && source.is_none_or(|x| x == s)
                && target.is_none_or(|x| x == t)
        };
        // (link_type, source, target) → resolved, descriptor first, deduplicated.
        let mut out: BTreeMap<(String, String, String), ResolvedPolicy> = BTreeMap::new();
        for m in self.descriptors.modules() {
            for l in &m.links {
                if !matches(&l.link_type, &l.source, &l.target) {
                    continue;
                }
                let key = (l.link_type.clone(), l.source.clone(), l.target.clone());
                let scope_name = format!("{}({}→{}) · {op}", l.link_type, l.source, l.target);
                let resolved = match l.permissions.policy(op) {
                    Some(p) => ResolvedPolicy {
                        scope: scope_name,
                        operation: op.clone(),
                        policy: Ok(p.clone()),
                        source: PolicySource::LinkDescriptor,
                    },
                    None => self.link_fallback(&l.source, op, scope_name, &key),
                };
                out.entry(key).or_insert(resolved);
            }
        }
        for d in &self.config.links {
            if !matches(&d.link_type, &d.source_type, &d.target_type) {
                continue;
            }
            let key = (
                d.link_type.clone(),
                d.source_type.clone(),
                d.target_type.clone(),
            );
            if out.contains_key(&key) {
                continue;
            }
            let scope_name = format!(
                "{}({}→{}) · {op}",
                d.link_type, d.source_type, d.target_type
            );
            let resolved = match d.auth.as_ref().and_then(|a| a.policy_for(op.as_str())) {
                Some(raw) => ResolvedPolicy {
                    scope: scope_name,
                    operation: op.clone(),
                    policy: Self::declared(raw),
                    source: PolicySource::LinkConfig,
                },
                None => self.link_fallback(&d.source_type, op, scope_name, &key),
            };
            out.insert(key, resolved);
        }
        if out.is_empty() {
            return scope
                .map(|s| vec![self.entity_policy(s, op)])
                .unwrap_or_default();
        }
        out.into_values().collect()
    }

    fn link_fallback(
        &self,
        source_entity: &str,
        op: &Operation,
        scope_name: String,
        key: &(String, String, String),
    ) -> ResolvedPolicy {
        // A YAML link table declared for the same triple still wins over the entity.
        if let Some(raw) = self
            .config
            .links
            .iter()
            .find(|d| d.link_type == key.0 && d.source_type == key.1 && d.target_type == key.2)
            .and_then(|d| d.auth.as_ref())
            .and_then(|a| a.policy_for(op.as_str()))
        {
            return ResolvedPolicy {
                scope: scope_name,
                operation: op.clone(),
                policy: Self::declared(raw),
                source: PolicySource::LinkConfig,
            };
        }
        let mut r = self.entity_policy(source_entity, op);
        r.scope = scope_name;
        r
    }

    /// Permissions aggregated per module: `module → entity → PermDesc` (explicit
    /// declarations only), links under `link:<type>(<source>→<target>)`.
    pub fn module_permissions(&self) -> BTreeMap<String, BTreeMap<String, PermDesc>> {
        let mut out = BTreeMap::new();
        for m in self.descriptors.modules() {
            let entry: &mut BTreeMap<String, PermDesc> = out.entry(m.name.clone()).or_default();
            for e in &m.entities {
                entry.insert(e.name.clone(), e.permissions.clone());
            }
            for l in &m.links {
                if !l.permissions.is_empty() {
                    entry.insert(
                        format!("link:{}({}→{})", l.link_type, l.source, l.target),
                        l.permissions.clone(),
                    );
                }
            }
        }
        out
    }
}

/// Ownership seen by the evaluator for `op`: explicit verdict, else the context rule,
/// except `List` (unknown before reading rows: unevaluable).
pub fn ownership(principal: &Principal, op: &Operation, explicit: Option<bool>) -> Option<bool> {
    match (explicit, op) {
        (Some(b), _) => Some(b),
        (None, Operation::List) => None,
        (None, _) => principal.context_ownership(),
    }
}

fn forbidden(scope: &str, reason: &str) -> ContractError {
    ContractError::framework(codes::AUTH_FORBIDDEN)
        .with_param("operation", scope)
        .with_param("reason", reason)
}

/// Decide one resolved policy. `Ok(())` only on an evaluable `true`.
pub fn decide(
    resolved: &ResolvedPolicy,
    ctx: &AuthContext,
    owner: Option<bool>,
    env: &dyn PolicyEnv,
) -> Result<(), ContractError> {
    let expr = match &resolved.policy {
        Ok(e) => e,
        Err(raw) => {
            tracing::warn!(scope = %resolved.scope, policy = %raw, "unreadable policy: refusal");
            return Err(forbidden(&resolved.scope, "unreadable policy"));
        }
    };
    let principal = Principal::of(ctx);
    let view = principal.view(ownership(&principal, &resolved.operation, owner));
    match evaluate(expr, &view, env) {
        Some(true) => Ok(()),
        Some(false) => Err(forbidden(&resolved.scope, "policy not satisfied")),
        None => Err(forbidden(&resolved.scope, "policy unevaluable (fail-closed)")),
    }
}

/// The single entry point of every non-REST surface: may `ctx` run `op` on `entity`?
pub fn authorize_op(
    index: &PolicyIndex,
    entity: &str,
    op: &Operation,
    ctx: &AuthContext,
    owner: Option<bool>,
    env: &dyn PolicyEnv,
) -> Result<(), ContractError> {
    decide(&index.entity_policy(entity, op), ctx, owner, env)
}

/// Link operation: ALL matching declarations must allow; none = refusal.
pub fn authorize_link_op(
    policies: &[ResolvedPolicy],
    ctx: &AuthContext,
    env: &dyn PolicyEnv,
) -> Result<(), ContractError> {
    if policies.is_empty() {
        return Err(forbidden("link", "undeclared link type"));
    }
    for p in policies {
        decide(p, ctx, None, env)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{EntityAuthConfig, EntityConfig};
    use crate::core::auth::descriptor_policy::RegistryEnv;
    use crate::core::link::{LinkAuthConfig, LinkDefinition};
    use crate::descriptor::{EntityDesc, ErrorKind, LinkDesc, ModuleDescriptor};
    use uuid::Uuid;

    fn config() -> LinksConfig {
        LinksConfig {
            entities: vec![EntityConfig {
                singular: "order".into(),
                plural: "orders".into(),
                auth: EntityAuthConfig {
                    create: "role:admin".into(),
                    delete: "admin_onyl".into(),
                    list: "owner".into(),
                    ..Default::default()
                },
            }],
            links: vec![LinkDefinition {
                link_type: "has_invoice".into(),
                source_type: "order".into(),
                target_type: "invoice".into(),
                forward_route_name: "invoices".into(),
                reverse_route_name: "order".into(),
                description: None,
                required_fields: None,
                auth: Some(LinkAuthConfig {
                    create: "role:billing".into(),
                    ..Default::default()
                }),
            }],
            validation_rules: None,
            events: None,
            sinks: None,
            auth: None,
        }
    }

    fn index_with(desc: Option<ModuleDescriptor>) -> PolicyIndex {
        let mut set = DescriptorSet::new();
        if let Some(d) = desc {
            set.insert(d).unwrap();
        }
        PolicyIndex::new(Arc::new(set), Arc::new(config()))
    }

    fn user(roles: &[&str]) -> AuthContext {
        AuthContext::User {
            user_id: Uuid::from_u128(1),
            tenant_id: Uuid::from_u128(2),
            roles: roles.iter().map(|r| r.to_string()).collect(),
        }
    }

    #[test]
    fn yaml_policy_applies_and_typo_refuses() {
        let idx = index_with(None);
        let ctx = user(&["admin"]);
        let env = RegistryEnv::new(None, &ctx);
        assert!(authorize_op(&idx, "order", &Operation::Create, &ctx, None, &env).is_ok());
        let plain = user(&[]);
        let env2 = RegistryEnv::new(None, &plain);
        let err = authorize_op(&idx, "order", &Operation::Create, &plain, None, &env2).unwrap_err();
        assert_eq!(err.code.as_str(), codes::AUTH_FORBIDDEN);
        assert_eq!(err.kind, ErrorKind::PermissionDenied);
        // `admin_onyl` is never read as `authenticated`.
        assert!(authorize_op(&idx, "order", &Operation::Delete, &ctx, None, &env).is_err());
        // Business operation: deny by default.
        let pay = Operation::Custom("order.pay".into());
        assert!(authorize_op(&idx, "order", &pay, &ctx, None, &env).is_err());
        // Plural lookup reaches the same entry.
        assert!(authorize_op(&idx, "orders", &Operation::Create, &ctx, None, &env).is_ok());
    }

    #[test]
    fn owner_on_list_is_unevaluable() {
        let idx = index_with(None);
        let owner = AuthContext::Owner {
            user_id: Uuid::from_u128(1),
            tenant_id: Uuid::from_u128(2),
            resource_id: Uuid::from_u128(3),
            resource_type: "order".into(),
        };
        let env = RegistryEnv::new(None, &owner);
        assert!(authorize_op(&idx, "order", &Operation::List, &owner, None, &env).is_err());
        assert!(authorize_op(&idx, "order", &Operation::List, &owner, Some(true), &env).is_ok());
    }

    #[test]
    fn descriptor_wins_over_yaml_and_links_follow_precedence() {
        let mut m = ModuleDescriptor::new("shop", "1.0.0");
        let mut e = EntityDesc::new("order");
        e.permissions
            .ops
            .insert(Operation::Create, PolicyExpr::Public);
        m.entities.push(e);
        let mut l = LinkDesc::new("has_invoice", "order", "invoice");
        l.permissions
            .ops
            .insert(Operation::DeleteLink, PolicyExpr::Deny);
        m.links.push(l);
        let idx = index_with(Some(m));
        let anon = AuthContext::Anonymous;
        let env = RegistryEnv::new(None, &anon);
        let r = idx.entity_policy("order", &Operation::Create);
        assert_eq!(r.source, PolicySource::Descriptor);
        assert!(authorize_op(&idx, "order", &Operation::Create, &anon, None, &env).is_ok());
        // Not declared in the descriptor: the YAML table still applies.
        assert_eq!(idx.entity_policy("order", &Operation::List).source, PolicySource::Config);

        // Link: descriptor explicit, then YAML link table, then entity.
        let del = idx.link_policies(Some("has_invoice"), None, None, None, &Operation::DeleteLink);
        assert_eq!(del.len(), 1);
        assert_eq!(del[0].source, PolicySource::LinkDescriptor);
        let create = idx.link_policies(Some("has_invoice"), None, None, None, &Operation::CreateLink);
        assert_eq!(create[0].source, PolicySource::LinkConfig);
        assert_eq!(create[0].policy, Ok(PolicyExpr::HasRole(vec!["billing".into()])));
        // Undeclared link type without scope: refused.
        let none = idx.link_policies(Some("unknown"), None, None, None, &Operation::CreateLink);
        let admin = AuthContext::Admin {
            admin_id: Uuid::from_u128(9),
        };
        let env_admin = RegistryEnv::new(None, &admin);
        assert!(authorize_link_op(&none, &admin, &env_admin).is_err());
        // Aggregation per module.
        let agg = idx.module_permissions();
        assert!(agg["shop"].contains_key("order"));
        assert!(agg["shop"].contains_key("link:has_invoice(order→invoice)"));
    }
}
