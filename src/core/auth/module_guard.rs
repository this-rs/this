//! What a MODULE may do, checked beside what the CALLER may do.
//!
//! A route served for a module is governed by two independent questions, and both must be
//! answered `yes`:
//!
//! 1. the **caller** satisfies the route's policy (the historical check, unchanged);
//! 2. the **module** is itself granted every `perm:` permission that policy names
//!    ([`ModuleGuard`]).
//!
//! Neither implies the other. An authorized caller on a route whose module lacks the permission
//! is refused (`this.auth.module_forbidden`); a module that holds the permission serving a caller
//! who does not is refused (`this.auth.forbidden`). Merely being loaded grants nothing: the
//! module's grants come from the [`ModulePermissionSource`] (host policy rules), never from the
//! catalog, and a module never sees another module's grants.
//!
//! # Scope of the module check
//! A module is held to the permissions its routes' policies NAME (`perm:billing.invoice.read`).
//! A route whose policy names no `perm:` (`authenticated`, `role:..`) requires nothing of the
//! module, so a guard changes nothing for it. A route not declared by a module descriptor
//! (YAML `auth:` table, default policy) has no module and is left to the caller check.
//!
//! # No guard, no change
//! Every function here takes the guard as an `Option`/is only reached when one is wired: an
//! application that declares no module policy keeps its current behavior.
//!
//! # Traceability
//! Every refusal built here carries, as parameters, the `module`, the `permission`, the `caller`
//! (kind and id, see [`caller_label`]), the `resource` and the `route` (`entity · operation`),
//! and is logged with the same fields.

// `ContractError` is the cross-surface contract (same allowance as this-descriptor).
#![allow(clippy::result_large_err)]

use std::sync::Arc;

use super::AuthContext;
use super::surface::{PolicyIndex, ResolvedPolicy, authorize_link_op, decide};
use crate::descriptor::engine::policy::PolicyEnv;
use crate::descriptor::perm::{ModuleGrants, ModulePermissionSource, check_module_call};
use crate::descriptor::{ContractError, Operation};
use uuid::Uuid;

/// Who is calling, for an operator: `user:<id>`, `owner:<id>`, `service:<name>`, `admin:<id>`
/// or `anonymous`. Never a secret: only identifiers the audit trail already carries.
pub fn caller_label(ctx: &AuthContext) -> String {
    match ctx {
        AuthContext::User { user_id, .. } => format!("user:{user_id}"),
        AuthContext::Owner { user_id, .. } => format!("owner:{user_id}"),
        AuthContext::Service { service_name, .. } => format!("service:{service_name}"),
        AuthContext::Admin { admin_id } => format!("admin:{admin_id}"),
        AuthContext::Anonymous => "anonymous".to_string(),
    }
}

/// Add the four facts an operator needs to a refusal: module, caller, resource, route.
/// Existing parameters (`permission`, `cause`...) are kept.
fn trace(
    err: ContractError,
    module: Option<&str>,
    route: &str,
    ctx: &AuthContext,
    resource: Option<&Uuid>,
) -> ContractError {
    let caller = caller_label(ctx);
    let resource = resource.map_or_else(|| "*".to_string(), |r| r.to_string());
    tracing::warn!(
        code = %err.code,
        module = module.unwrap_or("-"),
        route,
        %caller,
        %resource,
        permission = err.params.get("permission").and_then(|p| p.as_str()).unwrap_or("-"),
        "module authorization refused"
    );
    let mut err = err
        .with_param("route", route)
        .with_param("caller", &caller)
        .with_param("resource", &resource);
    if let Some(m) = module {
        err = err.with_param("module", m);
    }
    err
}

/// The module side of the double check.
#[derive(Clone)]
pub struct ModuleGuard {
    source: Arc<dyn ModulePermissionSource>,
}

impl std::fmt::Debug for ModuleGuard {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ModuleGuard { .. }")
    }
}

impl ModuleGuard {
    /// Guard backed by any grant source (host policy rules, a wami adapter, a closure).
    pub fn new(source: Arc<dyn ModulePermissionSource>) -> Self {
        Self { source }
    }

    /// Guard backed by an in-memory rule table.
    pub fn from_grants(grants: ModuleGrants) -> Self {
        Self::new(Arc::new(grants))
    }

    /// Does the MODULE hold every permission the resolved policies name? `Ok(())` when none
    /// names one (or none has a module). The first missing permission refuses.
    pub fn authorize(
        &self,
        resolved: &[ResolvedPolicy],
        ctx: &AuthContext,
        resource: Option<&Uuid>,
    ) -> Result<(), ContractError> {
        for r in resolved {
            let (Some(module), Ok(expr)) = (r.module.as_deref(), &r.policy) else {
                continue;
            };
            for permission in expr.permissions() {
                check_module_call(module, permission, self.source.as_ref())
                    .map_err(|e| trace(e, Some(module), &r.scope, ctx, resource))?;
            }
        }
        Ok(())
    }
}

/// Name the module and caller on a CALLER refusal too: `this.auth.forbidden` alone says neither.
fn trace_caller_refusal(
    err: ContractError,
    resolved: &[ResolvedPolicy],
    ctx: &AuthContext,
    resource: Option<&Uuid>,
) -> ContractError {
    let module = resolved.iter().find_map(|r| r.module.as_deref());
    let route = resolved.first().map(|r| r.scope.as_str()).unwrap_or("link");
    let err = trace(err, module, route, ctx, resource);
    // The caller was refused: say it, so it is not mistaken for a module refusal.
    err.with_param("refused_party", "caller")
}

/// [`super::surface::authorize_op`] plus the module check. Caller first (unchanged), then module.
/// `guard == None` is exactly `authorize_op` (plus the traceability parameters on a refusal).
#[allow(clippy::too_many_arguments)]
pub fn authorize_op_as_module(
    index: &PolicyIndex,
    guard: Option<&ModuleGuard>,
    entity: &str,
    op: &Operation,
    ctx: &AuthContext,
    owner: Option<bool>,
    resource: Option<&Uuid>,
    env: &dyn PolicyEnv,
) -> Result<(), ContractError> {
    let resolved = [index.entity_policy(entity, op)];
    decide(&resolved[0], ctx, owner, resource, env)
        .map_err(|e| trace_caller_refusal(e, &resolved, ctx, resource))?;
    match guard {
        Some(g) => g.authorize(&resolved, ctx, resource),
        None => Ok(()),
    }
}

/// [`super::surface::authorize_link_op`] plus the module check on every matching declaration.
pub fn authorize_link_op_as_module(
    policies: &[ResolvedPolicy],
    guard: Option<&ModuleGuard>,
    ctx: &AuthContext,
    env: &dyn PolicyEnv,
) -> Result<(), ContractError> {
    authorize_link_op(policies, ctx, env)
        .map_err(|e| trace_caller_refusal(e, policies, ctx, None))?;
    match guard {
        Some(g) => g.authorize(policies, ctx, None),
        None => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::LinksConfig;
    use crate::core::auth::descriptor_policy::RegistryEnv;
    use crate::descriptor::perm::{
        LoadPosture, admit_module, check_load, check_module_permissions, permission_codes,
    };
    use crate::descriptor::{
        DescriptorSet, EntityDesc, ErrorKind, LinkDesc, ModuleDescriptor, PolicyExpr,
    };

    const CALLER: Uuid = Uuid::from_u128(1);
    const INVOICE: Uuid = Uuid::from_u128(77);

    fn module(name: &str, entity: &str, policies: &[(&str, &str)]) -> ModuleDescriptor {
        let mut m = ModuleDescriptor::new(name, "1.0.0");
        let mut e = EntityDesc::new(entity);
        for (op, pol) in policies {
            e.permissions
                .ops
                .insert(op.parse().unwrap(), PolicyExpr::parse(pol).unwrap());
        }
        m.entities.push(e);
        m
    }

    fn billing() -> ModuleDescriptor {
        module(
            "billing",
            "invoice",
            &[
                ("get", "perm:billing.invoice.read"),
                ("create", "perm:billing.invoice.write"),
                ("list", "authenticated"),
            ],
        )
    }

    fn catalog() -> ModuleDescriptor {
        module(
            "catalog",
            "product",
            &[("get", "perm:catalog.product.read")],
        )
    }

    fn index(modules: Vec<ModuleDescriptor>) -> PolicyIndex {
        let mut set = DescriptorSet::new();
        for m in modules {
            set.insert(m).unwrap();
        }
        let config = LinksConfig {
            entities: vec![],
            links: vec![],
            validation_rules: None,
            events: None,
            sinks: None,
            auth: None,
        };
        PolicyIndex::new(Arc::new(set), Arc::new(config))
    }

    fn user() -> AuthContext {
        AuthContext::User {
            user_id: CALLER,
            tenant_id: Uuid::from_u128(2),
            roles: vec![],
            scopes: vec![],
        }
    }

    /// The in-memory CALLER policy: the caller holds exactly `held`.
    fn run(
        idx: &PolicyIndex,
        guard: Option<&ModuleGuard>,
        entity: &str,
        op: &str,
        held: &[&str],
    ) -> Result<(), ContractError> {
        let ctx = user();
        let held: Vec<String> = held.iter().map(|s| s.to_string()).collect();
        let perm = move |p: &str| Some(held.iter().any(|h| h == p));
        let env = RegistryEnv::new(None, &ctx).with_permission(&perm);
        authorize_op_as_module(
            idx,
            guard,
            entity,
            &op.parse().unwrap(),
            &ctx,
            None,
            Some(&INVOICE),
            &env,
        )
    }

    fn guard(g: ModuleGrants) -> ModuleGuard {
        ModuleGuard::from_grants(g)
    }

    #[test]
    fn granted_when_both_caller_and_module_hold_the_permission() {
        let idx = index(vec![billing()]);
        let g = guard(ModuleGrants::new().allow("billing", "billing.invoice.read"));
        assert!(run(&idx, Some(&g), "invoice", "get", &["billing.invoice.read"]).is_ok());
    }

    #[test]
    fn authorized_caller_is_refused_when_the_module_lacks_the_permission() {
        let idx = index(vec![billing()]);
        // The module is granted `write`, not `read`.
        let g = guard(ModuleGrants::new().allow("billing", "billing.invoice.write"));
        let e = run(&idx, Some(&g), "invoice", "get", &["billing.invoice.read"]).unwrap_err();
        assert_eq!(e.code.as_str(), permission_codes::MODULE_FORBIDDEN);
        assert_eq!(e.kind, ErrorKind::PermissionDenied);
        assert_eq!((e.kind.http_status(), e.kind.grpc_code()), (403, 7));
        // Traceability: module, permission, caller, resource, route.
        assert_eq!(e.params["module"], "billing");
        assert_eq!(e.params["permission"], "billing.invoice.read");
        assert_eq!(e.params["caller"], format!("user:{CALLER}"));
        assert_eq!(e.params["resource"], INVOICE.to_string());
        assert_eq!(e.params["route"], "invoice · get");
        assert!(e.message.contains("billing") && e.message.contains("billing.invoice.read"));
    }

    #[test]
    fn module_that_holds_the_permission_does_not_lend_it_to_an_unauthorized_caller() {
        let idx = index(vec![billing()]);
        let g = guard(ModuleGrants::new().allow("billing", "billing.invoice.*"));
        let e = run(&idx, Some(&g), "invoice", "get", &[]).unwrap_err();
        assert_eq!(e.code.as_str(), "this.auth.forbidden");
        assert_eq!(e.kind, ErrorKind::PermissionDenied);
        // The refusal still names the module, caller and resource, and who was refused.
        assert_eq!(e.params["module"], "billing");
        assert_eq!(e.params["refused_party"], "caller");
        assert_eq!(e.params["caller"], format!("user:{CALLER}"));
        assert_eq!(e.params["resource"], INVOICE.to_string());
    }

    #[test]
    fn grants_are_not_inherited_between_modules() {
        let idx = index(vec![billing(), catalog()]);
        // Only `catalog` has rules. `billing` is loaded, its caller is authorized: refused.
        let g = guard(ModuleGrants::new().allow("catalog", "catalog.*"));
        assert!(run(&idx, Some(&g), "product", "get", &["catalog.product.read"]).is_ok());
        let e = run(&idx, Some(&g), "invoice", "get", &["billing.invoice.read"]).unwrap_err();
        assert_eq!(e.code.as_str(), permission_codes::MODULE_FORBIDDEN);
        assert_eq!(e.params["module"], "billing");
    }

    #[test]
    fn a_loaded_module_with_no_rule_is_not_granted_by_being_loaded() {
        let idx = index(vec![billing()]);
        let g = guard(ModuleGrants::new());
        let e = run(&idx, Some(&g), "invoice", "get", &["billing.invoice.read"]).unwrap_err();
        assert_eq!(e.code.as_str(), permission_codes::MODULE_FORBIDDEN);
        assert_eq!(e.params["cause"], "not_granted");
    }

    #[test]
    fn a_descriptor_that_names_a_foreign_permission_cannot_borrow_its_grant() {
        // A tampered store: `billing` names a catalog permission, and catalog IS granted it.
        let thief = module(
            "billing",
            "invoice",
            &[("get", "perm:catalog.product.read")],
        );
        let idx = index(vec![thief.clone()]);
        let g = guard(ModuleGrants::new().allow("catalog", "catalog.*"));
        let e = run(&idx, Some(&g), "invoice", "get", &["catalog.product.read"]).unwrap_err();
        assert_eq!(e.code.as_str(), permission_codes::MODULE_FORBIDDEN);
        assert_eq!(e.params["cause"], permission_codes::FOREIGN_NAMESPACE);
        // And the install check would have refused it before.
        let errs = check_module_permissions(&thief);
        assert_eq!(errs[0].code.as_str(), permission_codes::FOREIGN_NAMESPACE);
    }

    #[test]
    fn an_administration_permission_is_refused_at_install_and_never_served_at_call() {
        let admin = module("billing", "invoice", &[("delete", "perm:billing.admin")]);
        let errs = admit_module(&admin, &ModuleGrants::new(), LoadPosture::Degraded).unwrap_err();
        assert_eq!(errs[0].code.as_str(), permission_codes::ADMIN);
        // Even with a rule granting it and a caller holding it.
        let idx = index(vec![admin]);
        let g = guard(ModuleGrants::new().allow("billing", "billing.admin"));
        let e = run(&idx, Some(&g), "invoice", "delete", &["billing.admin"]).unwrap_err();
        assert_eq!(e.params["cause"], permission_codes::ADMIN);
    }

    #[test]
    fn load_is_refused_when_no_rule_grants_a_required_permission() {
        let g = ModuleGrants::new().allow("billing", "billing.invoice.read");
        let e = admit_module(&billing(), &g, LoadPosture::Strict).unwrap_err();
        assert_eq!(e[0].code.as_str(), permission_codes::UNGRANTED);
        assert_eq!(e[0].params["permissions"][0], "billing.invoice.write");
        // Degraded: loads, says so, and the ungranted route is refused at call time.
        let v = check_load(&billing(), &g, LoadPosture::Degraded).unwrap();
        assert_eq!(v.ungranted, vec!["billing.invoice.write"]);
        let idx = index(vec![billing()]);
        let guard = guard(g);
        assert!(
            run(
                &idx,
                Some(&guard),
                "invoice",
                "get",
                &["billing.invoice.read"]
            )
            .is_ok()
        );
        let e = run(
            &idx,
            Some(&guard),
            "invoice",
            "create",
            &["billing.invoice.write"],
        )
        .unwrap_err();
        assert_eq!(e.code.as_str(), permission_codes::MODULE_FORBIDDEN);
    }

    #[test]
    fn without_a_guard_the_historical_behaviour_is_unchanged() {
        let idx = index(vec![billing()]);
        assert!(run(&idx, None, "invoice", "get", &["billing.invoice.read"]).is_ok());
        assert!(run(&idx, None, "invoice", "get", &[]).is_err());
    }

    #[test]
    fn a_route_that_names_no_permission_asks_nothing_of_the_module() {
        let idx = index(vec![billing()]);
        let g = guard(ModuleGrants::new());
        assert!(run(&idx, Some(&g), "invoice", "list", &[]).is_ok());
    }

    #[test]
    fn an_unevaluable_module_grant_refuses() {
        let idx = index(vec![billing()]);
        let g = ModuleGuard::new(Arc::new(|_: &str, _: &str| -> Option<bool> { None }));
        let e = run(&idx, Some(&g), "invoice", "get", &["billing.invoice.read"]).unwrap_err();
        assert_eq!(e.code.as_str(), permission_codes::MODULE_FORBIDDEN);
        assert_eq!(e.params["cause"], "unevaluable");
    }

    #[test]
    fn link_routes_check_the_module_that_declared_the_link() {
        let mut m = billing();
        let mut l = LinkDesc::new("billed_to", "invoice", "customer");
        l.permissions.ops.insert(
            "create_link".parse().unwrap(),
            PolicyExpr::parse("perm:billing.invoice.link").unwrap(),
        );
        m.links.push(l);
        let idx = index(vec![m]);
        let policies = idx.link_policies(
            Some("billed_to"),
            None,
            None,
            None,
            &"create_link".parse().unwrap(),
        );
        let ctx = user();
        let perm = |p: &str| Some(p == "billing.invoice.link");
        let env = RegistryEnv::new(None, &ctx).with_permission(&perm);

        let none = guard(ModuleGrants::new());
        let e = authorize_link_op_as_module(&policies, Some(&none), &ctx, &env).unwrap_err();
        assert_eq!(e.code.as_str(), permission_codes::MODULE_FORBIDDEN);
        assert_eq!(e.params["module"], "billing");

        let some = guard(ModuleGrants::new().allow("billing", "billing.invoice.link"));
        assert!(authorize_link_op_as_module(&policies, Some(&some), &ctx, &env).is_ok());
    }

    #[test]
    fn caller_labels_name_the_kind_of_caller() {
        assert_eq!(caller_label(&AuthContext::Anonymous), "anonymous");
        assert_eq!(
            caller_label(&AuthContext::Service {
                service_name: "billing-worker".into(),
                tenant_id: None
            }),
            "service:billing-worker"
        );
        assert_eq!(
            caller_label(&AuthContext::Admin { admin_id: CALLER }),
            format!("admin:{CALLER}")
        );
    }
}
