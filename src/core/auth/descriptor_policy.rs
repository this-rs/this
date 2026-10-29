//! Bridge between the descriptor's policy AST (`this_descriptor::perm`) and the
//! historical `AuthPolicy` / `AuthContext` of this-rs.
//!
//! - [`Principal`]: an owned projection of an [`AuthContext`], viewed as a
//!   [`PrincipalView`] for the pure evaluator (`engine::policy`).
//! - [`RegistryEnv`]: the [`PolicyEnv`] of the host: named resolvers come from the
//!   [`AuthResolverRegistry`], `perm:` from an optional IAM callback.
//! - `TryFrom<&PolicyExpr> for AuthPolicy`: direct AST conversion (no detour through
//!   the 9 strings of `EntityAuthConfig`, RFC §3.6). `perm:` has no `AuthPolicy`
//!   counterpart: it is refused, never approximated.
//! - [`conflicting_declarations`]: equality check YAML `auth:` vs descriptor
//!   `permissions`, compared on the AST, not on strings.
//!
//! Semantics shared with the REST middleware: `owner` is satisfied by an
//! `AuthContext::Owner` (the context, not a row lookup), roles only exist on
//! `AuthContext::User`, `admin_only` / `service_only` follow the variant.

use super::{AuthContext, AuthPolicy, AuthResolverRegistry};
use crate::config::EntityAuthConfig;
use crate::descriptor::engine::policy::PolicyEnv;
use crate::descriptor::framework::codes;
use crate::descriptor::{ContractError, Operation, PermDesc, PolicyExpr, PrincipalView};
use uuid::Uuid;

/// Owned projection of an [`AuthContext`] for the pure evaluator.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Principal {
    pub authenticated: bool,
    pub user_id: Option<String>,
    pub roles: Vec<String>,
    pub admin: bool,
    pub service: bool,
    /// `Some(true)` for an `AuthContext::Owner` (same rule as `AuthPolicy::Owner`).
    pub owner_context: bool,
}

impl Principal {
    pub fn of(ctx: &AuthContext) -> Self {
        Self {
            authenticated: !matches!(ctx, AuthContext::Anonymous),
            user_id: ctx.user_id().map(|u| u.to_string()),
            roles: ctx.roles().to_vec(),
            admin: ctx.is_admin(),
            service: ctx.is_service(),
            owner_context: matches!(ctx, AuthContext::Owner { .. }),
        }
    }

    /// View for the evaluator. `owner`: `Some(b)` when the caller knows ownership,
    /// `None` when it does not (then `owner` is UNEVALUABLE, never true).
    pub fn view(&self, owner: Option<bool>) -> PrincipalView<'_> {
        PrincipalView {
            authenticated: self.authenticated,
            user_id: self.user_id.as_deref(),
            roles: &self.roles,
            admin: self.admin,
            service: self.service,
            owner,
        }
    }

    /// Ownership as the REST middleware sees it: the context variant.
    pub fn context_ownership(&self) -> Option<bool> {
        Some(self.owner_context)
    }
}

/// IAM callback: `Some(verdict)` or `None` (no plan / no principal = unevaluable).
pub type PermissionFn<'a> = dyn Fn(&str) -> Option<bool> + Send + Sync + 'a;

/// The host's policy environment: named resolvers and (optional) IAM.
pub struct RegistryEnv<'a> {
    pub resolvers: Option<&'a AuthResolverRegistry>,
    pub ctx: &'a AuthContext,
    pub resource_id: Option<&'a Uuid>,
    pub permission: Option<&'a PermissionFn<'a>>,
}

impl<'a> RegistryEnv<'a> {
    pub fn new(resolvers: Option<&'a AuthResolverRegistry>, ctx: &'a AuthContext) -> Self {
        Self {
            resolvers,
            ctx,
            resource_id: None,
            permission: None,
        }
    }
    pub fn with_resource(mut self, id: Option<&'a Uuid>) -> Self {
        self.resource_id = id;
        self
    }
    pub fn with_permission(mut self, f: &'a PermissionFn<'a>) -> Self {
        self.permission = Some(f);
        self
    }
}

impl PolicyEnv for RegistryEnv<'_> {
    fn resolver(&self, name: &str) -> Option<bool> {
        // Unknown resolver = unevaluable (stricter than `check_with_resolver`: a
        // negation of it cannot allow).
        let r = self.resolvers?.get(name)?;
        Some(r.check(self.ctx, self.resource_id))
    }
    fn permission(&self, permission: &str) -> Option<bool> {
        self.permission.and_then(|f| f(permission))
    }
}

/// `PolicyExpr` that `AuthPolicy` cannot express.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("policy `{0}` has no AuthPolicy equivalent (perm: needs the IAM evaluator)")]
pub struct NotExpressible(pub String);

fn deny_all(_: &AuthContext) -> bool {
    false
}

impl TryFrom<&PolicyExpr> for AuthPolicy {
    type Error = NotExpressible;
    fn try_from(e: &PolicyExpr) -> Result<Self, Self::Error> {
        let all = |v: &[PolicyExpr]| -> Result<Vec<AuthPolicy>, NotExpressible> {
            v.iter().map(AuthPolicy::try_from).collect()
        };
        Ok(match e {
            PolicyExpr::Public => AuthPolicy::Public,
            PolicyExpr::Authenticated => AuthPolicy::Authenticated,
            PolicyExpr::Owner => AuthPolicy::Owner,
            PolicyExpr::ServiceOnly => AuthPolicy::ServiceOnly,
            PolicyExpr::AdminOnly => AuthPolicy::AdminOnly,
            PolicyExpr::Deny => AuthPolicy::Custom(deny_all),
            PolicyExpr::HasRole(r) => AuthPolicy::HasRole(r.clone()),
            PolicyExpr::Resolver(n) => AuthPolicy::Resolver(n.clone()),
            PolicyExpr::Perm(_) => return Err(NotExpressible(e.to_string())),
            PolicyExpr::And(v) => AuthPolicy::And(all(v)?),
            PolicyExpr::Or(v) => AuthPolicy::Or(all(v)?),
            PolicyExpr::Not(inner) => AuthPolicy::Not(Box::new(AuthPolicy::try_from(&**inner)?)),
        })
    }
}

impl TryFrom<PolicyExpr> for AuthPolicy {
    type Error = NotExpressible;
    fn try_from(e: PolicyExpr) -> Result<Self, Self::Error> {
        AuthPolicy::try_from(&e)
    }
}

/// Parse a declared policy string (YAML `auth:`), fail-closed: an unknown token is a
/// `this.auth.policy_invalid` error, never the legacy `authenticated` fallback.
pub fn parse_declared(policy: &str) -> Result<PolicyExpr, ContractError> {
    PolicyExpr::parse(policy).map_err(|e| {
        ContractError::framework(codes::AUTH_POLICY_INVALID)
            .with_param("policy", policy)
            .with_param("reason", e.reason)
    })
}

/// The YAML `EntityAuthConfig` field of a standard operation. `GetLink` reads
/// `list_links` (reading one link is navigating links); business operations have none.
pub fn entity_auth_field<'a>(auth: &'a EntityAuthConfig, op: &Operation) -> Option<&'a str> {
    Some(match op {
        Operation::List => &auth.list,
        Operation::Get => &auth.get,
        Operation::Create => &auth.create,
        Operation::Update => &auth.update,
        Operation::Delete => &auth.delete,
        Operation::ListLinks | Operation::GetLink => &auth.list_links,
        Operation::CreateLink => &auth.create_link,
        Operation::UpdateLink => &auth.update_link,
        Operation::DeleteLink => &auth.delete_link,
        Operation::Custom(_) => return None,
    })
}

/// Operations whose YAML policy and descriptor policy BOTH exist and DIFFER on the AST.
/// Only operations explicitly declared in the descriptor are compared (the YAML table
/// fills every field with `authenticated`, so absence cannot be told from default).
/// An unparsable YAML string is a conflict (reported with its raw text).
pub fn conflicting_declarations(
    yaml: &EntityAuthConfig,
    desc: &PermDesc,
) -> Vec<(Operation, String, PolicyExpr)> {
    let mut out = Vec::new();
    for (op, expr) in &desc.ops {
        let Some(raw) = entity_auth_field(yaml, op) else {
            continue;
        };
        match PolicyExpr::parse(raw) {
            Ok(parsed) if &parsed == expr => {}
            _ => out.push((op.clone(), raw.to_string(), expr.clone())),
        }
    }
    out
}

/// Build a `PermDesc` from a YAML table (every standard entity operation, explicit).
pub fn perm_desc_from_entity_auth(auth: &EntityAuthConfig) -> Result<PermDesc, ContractError> {
    let mut p = PermDesc::default();
    for op in Operation::STANDARD {
        if op == Operation::GetLink {
            continue; // no YAML field of its own
        }
        if let Some(raw) = entity_auth_field(auth, &op) {
            p.ops.insert(op, parse_declared(raw)?);
        }
    }
    Ok(p)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::auth::FnResolver;
    use crate::descriptor::engine::policy::is_allowed;
    use std::sync::Arc;

    fn uid(n: u128) -> Uuid {
        Uuid::from_u128(n)
    }

    fn contexts() -> Vec<(&'static str, AuthContext)> {
        vec![
            ("anonymous", AuthContext::Anonymous),
            (
                "user",
                AuthContext::User {
                    user_id: uid(1),
                    tenant_id: uid(2),
                    roles: vec!["editor".into()],
                },
            ),
            (
                "user_admin_role",
                AuthContext::User {
                    user_id: uid(1),
                    tenant_id: uid(2),
                    roles: vec!["admin".into(), "editor".into()],
                },
            ),
            (
                "owner",
                AuthContext::Owner {
                    user_id: uid(1),
                    tenant_id: uid(2),
                    resource_id: uid(3),
                    resource_type: "order".into(),
                },
            ),
            (
                "service",
                AuthContext::Service {
                    service_name: "billing".into(),
                    tenant_id: None,
                },
            ),
            ("admin", AuthContext::Admin { admin_id: uid(9) }),
        ]
    }

    fn registry() -> AuthResolverRegistry {
        let mut r = AuthResolverRegistry::new();
        r.register("yes", Arc::new(FnResolver::new(|_: &AuthContext, _| true, "yes")));
        r.register("no", Arc::new(FnResolver::new(|_: &AuthContext, _| false, "no")));
        r.register(
            "is_user",
            Arc::new(FnResolver::new(
                |c: &AuthContext, _| matches!(c, AuthContext::User { .. }),
                "user",
            )),
        );
        r
    }

    /// Every policy of a `PermDesc`, converted to `AuthPolicy`, gives the same verdict as
    /// the pure evaluator, for every context kind, and as the legacy string parser on the
    /// shared vocabulary. Unknown resolvers under a negation are the one documented
    /// difference (tested in `unknown_resolver_denies`), so they are not in this table.
    #[test]
    fn perm_desc_to_auth_policy_equivalent() {
        let policies = [
            "public",
            "authenticated",
            "owner",
            "service_only",
            "admin_only",
            "owner_or_service",
            "role:admin",
            "role:admin|editor",
            "owner_or_role:admin",
            "resolver:yes",
            "resolver:no",
            "resolver:is_user",
            "owner_or_resolver:is_user",
            "all:authenticated,resolver:is_user",
            "any:owner,resolver:no,role:admin",
            "all:admin_only,not:resolver:no",
            "not:role:admin",
            "deny",
            "all(any(owner, role:admin), not(resolver:no))",
            "any(service_only, all(authenticated, resolver:is_user))",
        ];
        let reg = registry();
        let mut perm = PermDesc::default();
        let ops: Vec<Operation> = Operation::STANDARD.to_vec();
        for (i, s) in policies.iter().enumerate() {
            perm.ops
                .insert(ops[i % ops.len()].clone(), PolicyExpr::parse(s).unwrap());
            for (name, ctx) in contexts() {
                let expr = PolicyExpr::parse(s).unwrap();
                let legacy = AuthPolicy::try_from(&expr).unwrap();
                let p = Principal::of(&ctx);
                let env = RegistryEnv::new(Some(&reg), &ctx);
                let engine = is_allowed(&expr, &p.view(p.context_ownership()), &env);
                let converted = legacy.check_with_resolver(&ctx, Some(&reg), None);
                assert_eq!(engine, converted, "`{s}` for {name}: engine vs AuthPolicy");
                // The legacy flat grammar (no function form, no `|`, no deny) must agree too.
                if !s.contains('(') && !s.contains('|') && *s != "deny" {
                    let parsed = AuthPolicy::parse_policy(s).check_with_resolver(&ctx, Some(&reg), None);
                    assert_eq!(engine, parsed, "`{s}` for {name}: engine vs parse_policy");
                }
            }
        }
        // Every op of the aggregated PermDesc converts (no perm: in it).
        for op in Operation::STANDARD {
            assert!(AuthPolicy::try_from(perm.effective(&op)).is_ok());
        }
        // perm: is refused, never approximated.
        assert!(AuthPolicy::try_from(PolicyExpr::parse("perm:invoices.read").unwrap()).is_err());
        assert!(
            AuthPolicy::try_from(PolicyExpr::parse("any(public, perm:x)").unwrap()).is_err()
        );
    }

    #[test]
    fn yaml_and_descriptor_compared_on_the_ast() {
        let yaml = EntityAuthConfig {
            delete: "all:admin_only,not:resolver:ro".into(),
            create: "admin_onyl".into(),
            ..Default::default()
        };
        let mut desc = PermDesc::default();
        desc.ops.insert(
            Operation::Delete,
            PolicyExpr::parse("all(admin_only, not(resolver:ro))").unwrap(),
        );
        desc.ops.insert(Operation::Create, PolicyExpr::AdminOnly);
        desc.ops.insert(Operation::Get, PolicyExpr::Public);
        let c = conflicting_declarations(&yaml, &desc);
        let ops: Vec<_> = c.iter().map(|(o, ..)| o.clone()).collect();
        // Same AST in two syntaxes: no conflict. Typo and real difference: conflicts.
        assert_eq!(ops, vec![Operation::Get, Operation::Create]);
        assert!(perm_desc_from_entity_auth(&yaml).is_err());
        let ok = perm_desc_from_entity_auth(&EntityAuthConfig::default()).unwrap();
        assert_eq!(ok.effective(&Operation::Delete), PolicyExpr::Authenticated);
    }
}
