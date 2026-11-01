//! Policy evaluation behind the synchronous `perm:` seam, on wami's documents.
//!
//! `this` asks its IAM a question through `PermissionFn = Fn(&str) -> Option<bool>`,
//! which is synchronous. wami's `Authorizer` is async and bound to a store, so this
//! module uses wami's pure per-document evaluator (`evaluate_policy_document`) and
//! reproduces the combination rules here (ADR D2):
//!
//! 1. Every identity document is read. An unreadable one refuses the decision.
//! 2. An explicit Deny anywhere wins (deny-overrides).
//! 3. Without an Allow from an identity document, the answer is an implicit Deny.
//! 4. A permissions boundary, when set, must also allow: it narrows, never grants.
//!
//! The mapping is fail-closed. `Some(true)` means allowed, `Some(false)` denied,
//! `None` unevaluable: the caller must refuse, never read `None` as allow.
//!
//! # Known limits
//!
//! - The seam carries no resource, so the evaluation resource is tenant-wide
//!   (`resource/*`). A statement naming one specific entity cannot match yet.
//!   Passing the resource through `PermissionFn` is a follow-up.
//! - Documents are registered in memory. Persistence is a follow-up.

use crate::core::auth::AuthContext;
use crate::core::auth::wami_bridge::{IdentityRegistry, RequestMeta, wami_context_for};
use anyhow::{Result, bail};
use std::collections::HashMap;
use wami::service::auth::decision::PolicySource;
use wami::service::auth::policy_evaluator::{
    PolicyEffect, evaluate_policy_document, parse_policy_doc,
};
use wami_core::WamiContext;
use wami_core::arn::{ScopedName, Service, WamiArn};
use wami_core::types::PolicyDocument;

/// Action namespace for `this` permissions: `perm:project.read` → `this:project:read`.
pub const ACTION_NAMESPACE: &str = "this";

/// Outcome of one decision. Never collapsed into a boolean before the caller sees it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    Allow,
    Deny,
    /// No decision could be made. The caller must refuse.
    Unevaluable,
}

impl Verdict {
    /// The `PermissionFn` answer: `None` keeps the unevaluable case distinct.
    pub fn as_permission(self) -> Option<bool> {
        match self {
            Verdict::Allow => Some(true),
            Verdict::Deny => Some(false),
            Verdict::Unevaluable => None,
        }
    }
}

/// Map a `this` permission string to a wami action.
///
/// Accepts `perm:a.b` or `a.b`. Each segment must be non-empty and made of
/// ASCII letters, digits, `_` or `-`, so no wildcard or separator can be smuggled
/// in from the permission name. Anything else maps to `None`.
pub fn wami_action_for(perm: &str) -> Option<String> {
    let name = perm.strip_prefix("perm:").unwrap_or(perm);
    let well_formed = !name.is_empty()
        && name.split('.').all(|seg| {
            !seg.is_empty()
                && seg
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        });
    well_formed.then(|| format!("{ACTION_NAMESPACE}:{}", name.replace('.', ":")))
}

/// The tenant-wide resource a permission check is evaluated against.
pub fn tenant_resource(ctx: &WamiContext) -> Result<WamiArn> {
    Ok(WamiArn::builder()
        .service(Service::Iam)
        .tenant_path(ctx.tenant_path().clone())
        .wami_instance(ctx.instance_id())
        .resource("resource", "*")
        .build()?)
}

/// Borrowed grants for one decision: identity documents and an optional boundary.
pub struct Grants<'a> {
    pub identity: Vec<(PolicySource, &'a PolicyDocument)>,
    pub boundary: Option<(PolicySource, &'a PolicyDocument)>,
}

/// Pure combination of the grants for one action on one resource.
///
/// Errors from the per-document evaluator become [`Verdict::Unevaluable`].
pub fn decide(grants: &Grants<'_>, action: &str, resource: &WamiArn, ctx: &WamiContext) -> Verdict {
    let mut allowed = false;
    for (source, document) in &grants.identity {
        match evaluate_policy_document(document, action, resource, ctx, source) {
            Ok(Some(hit)) => match hit.effect {
                PolicyEffect::Deny => return Verdict::Deny,
                PolicyEffect::Allow => allowed = true,
                PolicyEffect::NoMatch => {}
            },
            Ok(None) => {}
            Err(_) => return Verdict::Unevaluable,
        }
    }
    if !allowed {
        return Verdict::Deny;
    }

    if let Some((source, document)) = &grants.boundary {
        match evaluate_policy_document(document, action, resource, ctx, source) {
            Ok(Some(hit)) if hit.effect == PolicyEffect::Allow => {}
            Ok(_) => return Verdict::Deny,
            Err(_) => return Verdict::Unevaluable,
        }
    }
    Verdict::Allow
}

#[derive(Debug)]
struct OwnedGrants {
    identity: Vec<(PolicySource, PolicyDocument)>,
    boundary: Option<(PolicySource, PolicyDocument)>,
}

/// Policy documents per principal, parsed once at registration.
///
/// A document that does not parse (unknown field, malformed JSON) is refused at
/// registration. It is never stored half-read.
#[derive(Debug, Default)]
pub struct PolicyStore {
    principals: HashMap<ScopedName, OwnedGrants>,
}

impl PolicyStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register the identity documents and optional boundary of one principal.
    ///
    /// `identity` pairs an inline-policy name with its JSON; `boundary` pairs the
    /// boundary's ARN with its JSON. Any unreadable document fails the whole call.
    pub fn register(
        &mut self,
        principal: ScopedName,
        identity: &[(&str, &str)],
        boundary: Option<(&str, &str)>,
    ) -> Result<()> {
        let mut parsed_identity = Vec::with_capacity(identity.len());
        for (name, json) in identity {
            let source = PolicySource::UserInline {
                name: (*name).to_string(),
            };
            let document = parse_policy_doc(json, &source)
                .map_err(|e| anyhow::anyhow!("identity policy `{name}` refused: {e}"))?;
            parsed_identity.push((source, document));
        }
        let parsed_boundary = match boundary {
            Some((arn, json)) => {
                let source = PolicySource::UserManaged {
                    arn: arn.to_string(),
                };
                let document = parse_policy_doc(json, &source)
                    .map_err(|e| anyhow::anyhow!("boundary `{arn}` refused: {e}"))?;
                Some((source, document))
            }
            None => None,
        };
        self.principals.insert(
            principal,
            OwnedGrants {
                identity: parsed_identity,
                boundary: parsed_boundary,
            },
        );
        Ok(())
    }

    /// The verdict for `perm` for the caller of `ctx`.
    ///
    /// A permission string that maps to no action is `Unevaluable`. A caller with
    /// no registered principal has no grants, so the answer is `Deny`.
    pub fn verdict(&self, ctx: &WamiContext, perm: &str) -> Verdict {
        let Some(action) = wami_action_for(perm) else {
            return Verdict::Unevaluable;
        };
        let Ok(resource) = tenant_resource(ctx) else {
            return Verdict::Unevaluable;
        };
        let Some(key) = ctx.caller_key() else {
            return Verdict::Unevaluable;
        };
        let Some(owned) = self.principals.get(key) else {
            return Verdict::Deny;
        };
        let grants = Grants {
            identity: owned.identity.iter().map(|(s, d)| (s.clone(), d)).collect(),
            boundary: owned.boundary.as_ref().map(|(s, d)| (s.clone(), d)),
        };
        decide(&grants, &action, &resource, ctx)
    }

    /// The `PermissionFn` for one caller, ready for `RegistryEnv::with_permission`.
    pub fn permission<'a>(
        &'a self,
        ctx: &'a WamiContext,
    ) -> impl Fn(&str) -> Option<bool> + Send + Sync + 'a {
        move |perm: &str| self.verdict(ctx, perm).as_permission()
    }
}

/// Refuse a context that names no caller. Used by hosts before building a permission fn.
pub fn require_caller(ctx: &WamiContext) -> Result<()> {
    if ctx.caller_key().is_none() {
        bail!("WAMI context carries no caller key: no policy can be read for it");
    }
    Ok(())
}

/// What a host carries to answer `perm:` from wami for its callers.
///
/// When a host sets one, wami is the sole source for `perm:`: an IAM elsewhere is
/// not consulted, so two answers can never disagree about the same permission.
#[derive(Debug)]
pub struct WamiEnforcement {
    pub registry: IdentityRegistry,
    pub store: PolicyStore,
    pub instance_id: String,
}

impl WamiEnforcement {
    pub fn new(
        registry: IdentityRegistry,
        store: PolicyStore,
        instance_id: impl Into<String>,
    ) -> Self {
        Self {
            registry,
            store,
            instance_id: instance_id.into(),
        }
    }

    /// The WAMI caller of a verified `AuthContext`, if it has one.
    ///
    /// `RequestMeta::default()`: the surface guard does not carry the request's
    /// IP or transport yet, so an IP or TLS condition cannot be met and fails
    /// closed. Threading `RequestMeta` through the guard is a follow-up.
    pub fn caller_context(&self, auth: &AuthContext) -> Result<Option<WamiContext>> {
        wami_context_for(
            auth,
            &RequestMeta::default(),
            &self.registry,
            &self.instance_id,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::auth::wami_bridge::{IdentityRegistry, RequestMeta, wami_context_for};
    use uuid::Uuid;

    const ALLOW_PROJECT_READ: &str = r#"{
        "Version": "2012-10-17",
        "Statement": [
            {"Sid": "read", "Effect": "Allow", "Action": "this:project:read", "Resource": "*"}
        ]
    }"#;

    const DENY_PROJECT_READ: &str = r#"{
        "Version": "2012-10-17",
        "Statement": [
            {"Effect": "Deny", "Action": "this:project:read", "Resource": "*"}
        ]
    }"#;

    const ALLOW_ALL: &str = r#"{
        "Version": "2012-10-17",
        "Statement": [
            {"Effect": "Allow", "Action": "this:*", "Resource": "*"}
        ]
    }"#;

    const ALLOW_PROJECT_WRITE: &str = r#"{
        "Version": "2012-10-17",
        "Statement": [
            {"Effect": "Allow", "Action": "this:project:write", "Resource": "*"}
        ]
    }"#;

    const BOUNDARY_READ_ONLY: &str = r#"{
        "Version": "2012-10-17",
        "Statement": [
            {"Effect": "Allow", "Action": "this:project:read", "Resource": "*"}
        ]
    }"#;

    fn ctx_for(tenant: Uuid, user: Uuid, reg: &IdentityRegistry) -> WamiContext {
        let auth = AuthContext::User {
            user_id: user,
            tenant_id: tenant,
            roles: vec![],
        };
        wami_context_for(&auth, &RequestMeta::default(), reg, "this")
            .unwrap()
            .unwrap()
    }

    fn key_of(ctx: &WamiContext) -> ScopedName {
        ctx.caller_key().unwrap().clone()
    }

    #[test]
    fn permission_names_map_to_wami_actions() {
        assert_eq!(
            wami_action_for("perm:project.read").as_deref(),
            Some("this:project:read")
        );
        assert_eq!(
            wami_action_for("project.read").as_deref(),
            Some("this:project:read")
        );
        for bad in ["", "perm:", "a..b", "a b", "../x", "a.*", "a:b", ".a", "a."] {
            assert_eq!(wami_action_for(bad), None, "must refuse {bad:?}");
        }
    }

    #[test]
    fn allow_grants_and_absence_denies() {
        let reg = IdentityRegistry::new();
        let (t, u) = (Uuid::new_v4(), Uuid::new_v4());
        let ctx = ctx_for(t, u, &reg);
        let mut store = PolicyStore::new();
        store
            .register(key_of(&ctx), &[("read", ALLOW_PROJECT_READ)], None)
            .unwrap();
        assert_eq!(store.verdict(&ctx, "perm:project.read"), Verdict::Allow);
        assert_eq!(store.verdict(&ctx, "perm:project.write"), Verdict::Deny);
    }

    #[test]
    fn explicit_deny_overrides_an_allow_in_another_document() {
        let reg = IdentityRegistry::new();
        let (t, u) = (Uuid::new_v4(), Uuid::new_v4());
        let ctx = ctx_for(t, u, &reg);
        let mut store = PolicyStore::new();
        store
            .register(
                key_of(&ctx),
                &[("all", ALLOW_ALL), ("no-read", DENY_PROJECT_READ)],
                None,
            )
            .unwrap();
        assert_eq!(store.verdict(&ctx, "perm:project.read"), Verdict::Deny);
        assert_eq!(store.verdict(&ctx, "perm:project.write"), Verdict::Allow);
    }

    #[test]
    fn boundary_narrows_identity_and_never_grants() {
        let reg = IdentityRegistry::new();
        let (t, u) = (Uuid::new_v4(), Uuid::new_v4());
        let ctx = ctx_for(t, u, &reg);
        let mut store = PolicyStore::new();
        store
            .register(
                key_of(&ctx),
                &[("all", ALLOW_ALL)],
                Some(("arn:boundary/read-only", BOUNDARY_READ_ONLY)),
            )
            .unwrap();
        assert_eq!(store.verdict(&ctx, "perm:project.read"), Verdict::Allow);
        // Identity allows it, the boundary does not: refused.
        assert_eq!(store.verdict(&ctx, "perm:project.write"), Verdict::Deny);

        // A boundary alone grants nothing.
        let mut only_boundary = PolicyStore::new();
        only_boundary
            .register(
                key_of(&ctx),
                &[],
                Some(("arn:boundary/read-only", BOUNDARY_READ_ONLY)),
            )
            .unwrap();
        assert_eq!(
            only_boundary.verdict(&ctx, "perm:project.read"),
            Verdict::Deny
        );
    }

    #[test]
    fn unreadable_document_is_refused_at_registration() {
        let reg = IdentityRegistry::new();
        let ctx = ctx_for(Uuid::new_v4(), Uuid::new_v4(), &reg);
        let mut store = PolicyStore::new();
        let typo = r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":"this:*","Resource":"*","Typo":1}]}"#;
        assert!(
            store
                .register(key_of(&ctx), &[("bad", typo)], None)
                .is_err()
        );
        // Nothing half-read was stored: the caller still has no grants.
        assert_eq!(store.verdict(&ctx, "perm:project.read"), Verdict::Deny);
    }

    #[test]
    fn unknown_condition_operator_is_refused_at_registration() {
        // wami rejects an operator it does not know when the document is read,
        // so the statement can never silently fail to apply at decision time.
        let reg = IdentityRegistry::new();
        let ctx = ctx_for(Uuid::new_v4(), Uuid::new_v4(), &reg);
        let mut store = PolicyStore::new();
        let bogus_operator = r#"{
            "Version": "2012-10-17",
            "Statement": [{
                "Effect": "Allow", "Action": "this:project:read", "Resource": "*",
                "Condition": {"NoSuchOperator": {"aws:SourceIp": "10.0.0.0/8"}}
            }]
        }"#;
        assert!(
            store
                .register(key_of(&ctx), &[("cond", bogus_operator)], None)
                .is_err()
        );
        assert_eq!(store.verdict(&ctx, "perm:project.read"), Verdict::Deny);
    }

    #[test]
    fn malformed_permission_is_unevaluable() {
        let reg = IdentityRegistry::new();
        let ctx = ctx_for(Uuid::new_v4(), Uuid::new_v4(), &reg);
        let store = PolicyStore::new();
        assert_eq!(store.verdict(&ctx, "perm:*"), Verdict::Unevaluable);
    }

    #[test]
    fn grants_do_not_cross_tenants() {
        let reg = IdentityRegistry::new();
        let user = Uuid::new_v4();
        let (ta, tb) = (Uuid::new_v4(), Uuid::new_v4());
        let ctx_a = ctx_for(ta, user, &reg);
        let ctx_b = ctx_for(tb, user, &reg);
        let mut store = PolicyStore::new();
        store
            .register(key_of(&ctx_a), &[("all", ALLOW_ALL)], None)
            .unwrap();
        assert_eq!(store.verdict(&ctx_a, "perm:project.read"), Verdict::Allow);
        // The same user id in another tenant is another principal: no grants.
        assert_eq!(store.verdict(&ctx_b, "perm:project.read"), Verdict::Deny);
    }

    #[test]
    fn enforcement_answers_only_for_named_callers() {
        let mut enf = WamiEnforcement::new(IdentityRegistry::new(), PolicyStore::new(), "this");
        let auth = AuthContext::User {
            user_id: Uuid::new_v4(),
            tenant_id: Uuid::new_v4(),
            roles: vec![],
        };
        let ctx = enf.caller_context(&auth).unwrap().unwrap();
        enf.store
            .register(key_of(&ctx), &[("write", ALLOW_PROJECT_WRITE)], None)
            .unwrap();

        // The named caller is answered from its own grants.
        let verdict = enf
            .caller_context(&auth)
            .unwrap()
            .map(|c| enf.store.verdict(&c, "perm:project.write"));
        assert_eq!(verdict, Some(Verdict::Allow));

        // A caller WAMI cannot name has no WAMI identity at all: no answer, so none
        // can be read as an allow.
        assert!(
            enf.caller_context(&AuthContext::Anonymous)
                .unwrap()
                .is_none()
        );
        assert!(
            enf.caller_context(&AuthContext::Admin {
                admin_id: Uuid::new_v4()
            })
            .unwrap()
            .is_none()
        );
    }

    #[test]
    fn permission_closure_answers_the_seam() {
        let reg = IdentityRegistry::new();
        let ctx = ctx_for(Uuid::new_v4(), Uuid::new_v4(), &reg);
        let mut store = PolicyStore::new();
        store
            .register(key_of(&ctx), &[("write", ALLOW_PROJECT_WRITE)], None)
            .unwrap();
        let perm = store.permission(&ctx);
        assert_eq!(perm("perm:project.write"), Some(true));
        assert_eq!(perm("perm:project.read"), Some(false));
        assert_eq!(perm("perm:*"), None);
        require_caller(&ctx).unwrap();
    }
}
