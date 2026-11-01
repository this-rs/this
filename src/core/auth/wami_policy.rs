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
//! # Resources
//!
//! The seam's closure is built where the target is known, so the target is bound
//! into it as a [`ResourceRef`] and becomes the ARN the statements match:
//!
//! - entity `orders`, one instance → `…:orders/<uuid>`; the whole type → `…:orders/*`
//! - link `owns` from `users` to `orders` → `…:link/owns/users/orders`
//! - no particular target → `…:resource/*` (only a `Resource: "*"` statement matches)
//!
//! # Roles
//!
//! `this` carries roles in the token (`AuthContext::User.roles`); wami attaches
//! policies to groups in its store. Here a role is a named set of documents
//! ([`PolicyStore::register_role`]) read beside the principal's own: a Deny in a
//! role document overrides, exactly as a group policy would.
//!
//! # Ownership
//!
//! `owner` stays with `PolicyExpr`: without a store, wami cannot know who owns a
//! row. wami answers `perm:` only.
//!
//! # Known limit
//!
//! Documents and the identity registry live in memory; see [`WamiEnforcement`]
//! for the snapshot/restore hooks a host persists with.

use crate::core::auth::AuthContext;
use crate::core::auth::wami_bridge::{IdentityRegistry, RequestMeta, wami_context_for};
use anyhow::{Result, anyhow, bail};
use std::collections::HashMap;
use uuid::Uuid;
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
/// `this` descriptors write permissions as `perm:orders:write` (the historical
/// form) or `perm:orders.write`; both separators are accepted and become
/// `this:orders:write`. Each segment must be non-empty and made of ASCII
/// letters, digits, `_` or `-`, so no wildcard can be smuggled in from the
/// permission name: a `*` in a request would otherwise match a policy's
/// wildcard statements. Anything else maps to `None`.
pub fn wami_action_for(perm: &str) -> Option<String> {
    let name = perm.strip_prefix("perm:").unwrap_or(perm);
    let segments: Vec<&str> = name.split(['.', ':']).collect();
    let well_formed = !name.is_empty()
        && segments.iter().all(|seg| {
            !seg.is_empty()
                && seg
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        });
    well_formed.then(|| format!("{ACTION_NAMESPACE}:{}", segments.join(":")))
}

pub use crate::core::auth::ResourceRef;

/// A name usable as an ARN segment: letters, digits, `_`, `-`. Anything else
/// (a `*`, a `/`, a `:`) would change what a policy's pattern matches.
fn arn_safe(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// The ARN a permission check is evaluated against. `None` when a name is not
/// ARN-safe: the check is then unevaluable rather than matched against a
/// pattern it was not written for.
pub fn resource_arn(ctx: &WamiContext, target: &ResourceRef<'_>) -> Option<WamiArn> {
    let (resource_type, resource_id) = match target {
        ResourceRef::Entity { entity, id } => {
            if !arn_safe(entity) {
                return None;
            }
            let id = id.map(|u| u.to_string()).unwrap_or_else(|| "*".to_string());
            ((*entity).to_string(), id)
        }
        ResourceRef::Link {
            link_type,
            source,
            target,
        } => {
            if !arn_safe(link_type) || !source.is_none_or(arn_safe) || !target.is_none_or(arn_safe)
            {
                return None;
            }
            // An unknown endpoint is a literal `*` on the REQUEST side: it matches
            // only a pattern that is itself a wildcard there (`link/owns/**`,
            // `link/owns/*/*`), never a statement on one specific endpoint.
            let source = source.unwrap_or("*");
            let target = target.unwrap_or("*");
            ("link".to_string(), format!("{link_type}/{source}/{target}"))
        }
        ResourceRef::Any => ("resource".to_string(), "*".to_string()),
    };
    WamiArn::builder()
        .service(Service::Iam)
        .tenant_path(ctx.tenant_path().clone())
        .wami_instance(ctx.instance_id())
        .resource(resource_type, resource_id)
        .build()
        .ok()
}

/// The ARN of "no particular target". Kept for callers that have nothing better.
pub fn tenant_resource(ctx: &WamiContext) -> Result<WamiArn> {
    resource_arn(ctx, &ResourceRef::Any).ok_or_else(|| anyhow!("cannot build the tenant ARN"))
}

/// Borrowed grants for one decision: identity documents and an optional boundary.
pub struct Grants<'a> {
    pub identity: Vec<(PolicySource, &'a PolicyDocument)>,
    pub boundary: Option<(PolicySource, &'a PolicyDocument)>,
}

/// Condition operators that hold when the IP is UNKNOWN (AWS semantics:
/// `NotIpAddress` on an absent key is true, `…IfExists` passes). A statement
/// using one would turn unconditional on a host that cannot learn the IP, so
/// such a document is unevaluable without a known IP (fail closed).
fn needs_known_ip(document: &PolicyDocument) -> bool {
    document.statement.iter().any(|statement| {
        statement
            .condition
            .as_ref()
            .and_then(|c| c.as_object())
            .is_some_and(|ops| {
                ops.keys()
                    .any(|op| op.starts_with("NotIpAddress") || op.ends_with("IfExists"))
            })
    })
}

/// Pure combination of the grants for one action on one resource.
///
/// Errors from the per-document evaluator become [`Verdict::Unevaluable`].
pub fn decide(grants: &Grants<'_>, action: &str, resource: &WamiArn, ctx: &WamiContext) -> Verdict {
    if ctx.source_ip().is_none() {
        let documents = grants
            .identity
            .iter()
            .map(|(_, d)| *d)
            .chain(grants.boundary.iter().map(|(_, d)| *d));
        if documents.into_iter().any(needs_known_ip) {
            return Verdict::Unevaluable;
        }
    }
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
    /// Documents attached to a role name, read for every caller carrying the role.
    roles: HashMap<String, Vec<(PolicySource, PolicyDocument)>>,
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

    /// Register the documents of a role. Every caller whose token carries the
    /// role reads them beside its own (wami's group policies). Replaces any
    /// previous set for that role; an unreadable document fails the whole call.
    pub fn register_role(&mut self, role: &str, documents: &[(&str, &str)]) -> Result<()> {
        let mut parsed = Vec::with_capacity(documents.len());
        for (name, json) in documents {
            let source = PolicySource::GroupInline {
                group: role.to_string(),
                name: (*name).to_string(),
            };
            let document = parse_policy_doc(json, &source)
                .map_err(|e| anyhow!("role `{role}` policy `{name}` refused: {e}"))?;
            parsed.push((source, document));
        }
        self.roles.insert(role.to_string(), parsed);
        Ok(())
    }

    /// Whether `role` has documents registered.
    pub fn has_role(&self, role: &str) -> bool {
        self.roles.contains_key(role)
    }

    /// The verdict for `perm` on `target`, for the caller of `ctx` carrying `roles`.
    ///
    /// Unevaluable when the permission maps to no action, when the target is not
    /// ARN-safe, or when the context names no caller. A caller with neither its
    /// own grants nor a known role has nothing to read: `Deny`.
    pub fn verdict_on(
        &self,
        ctx: &WamiContext,
        roles: &[String],
        perm: &str,
        target: &ResourceRef<'_>,
    ) -> Verdict {
        let Some(action) = wami_action_for(perm) else {
            return Verdict::Unevaluable;
        };
        let Some(resource) = resource_arn(ctx, target) else {
            return Verdict::Unevaluable;
        };
        let Some(key) = ctx.caller_key() else {
            return Verdict::Unevaluable;
        };
        let owned = self.principals.get(key);
        let mut identity: Vec<(PolicySource, &PolicyDocument)> = owned
            .map(|o| o.identity.iter().map(|(s, d)| (s.clone(), d)).collect())
            .unwrap_or_default();
        for role in roles {
            if let Some(docs) = self.roles.get(role) {
                identity.extend(docs.iter().map(|(s, d)| (s.clone(), d)));
            }
        }
        if identity.is_empty() && owned.is_none() {
            return Verdict::Deny;
        }
        let grants = Grants {
            identity,
            boundary: owned
                .and_then(|o| o.boundary.as_ref())
                .map(|(s, d)| (s.clone(), d)),
        };
        decide(&grants, &action, &resource, ctx)
    }

    /// [`PolicyStore::verdict_on`] with no roles and no particular target.
    pub fn verdict(&self, ctx: &WamiContext, perm: &str) -> Verdict {
        self.verdict_on(ctx, &[], perm, &ResourceRef::Any)
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

    /// The WAMI caller of a verified `AuthContext`, if it has one, with the
    /// request facts conditions read (IP, transport, MFA). Without them, an IP
    /// or TLS condition cannot be met and fails closed.
    pub fn caller_context(
        &self,
        auth: &AuthContext,
        meta: &RequestMeta,
    ) -> Result<Option<WamiContext>> {
        wami_context_for(auth, meta, &self.registry, &self.instance_id)
    }

    /// The verdict for `perm` on `target`, for a verified `AuthContext`.
    ///
    /// `None` when the caller has no WAMI identity or its context cannot be
    /// built: unknown, so refused by every caller of this function.
    pub fn verdict_for(
        &self,
        auth: &AuthContext,
        meta: &RequestMeta,
        perm: &str,
        target: &ResourceRef<'_>,
    ) -> Option<Verdict> {
        let ctx = self.caller_context(auth, meta).ok().flatten()?;
        Some(self.store.verdict_on(&ctx, auth.roles(), perm, target))
    }
}

/// A policy document as a definition file carries it: inline JSON.
pub type DocumentJson = serde_json::Value;

/// One principal's grants, keyed by the `this` identifiers (tenant and user
/// UUIDs), not by wami segments: the file stays valid across registries.
///
/// Unknown fields are refused: a misspelt `boundary` would otherwise load a
/// principal WITHOUT its boundary, silently widening its grants.
#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub struct PrincipalDefinition {
    pub tenant: Uuid,
    pub user: Uuid,
    /// Inline policies by name.
    #[serde(default)]
    pub policies: std::collections::BTreeMap<String, DocumentJson>,
    /// Permissions boundary, if any.
    #[serde(default)]
    pub boundary: Option<BoundaryDefinition>,
}

#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub struct BoundaryDefinition {
    pub arn: String,
    pub document: DocumentJson,
}

/// The whole IAM a host loads: principals and roles. Loaded atomically: one
/// unreadable document refuses the whole definition and leaves the enforcement
/// as it was.
#[derive(Debug, Clone, Default, serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub struct PolicyDefinition {
    #[serde(default)]
    pub principals: Vec<PrincipalDefinition>,
    /// Role name → inline policies by name.
    #[serde(default)]
    pub roles: std::collections::BTreeMap<String, std::collections::BTreeMap<String, DocumentJson>>,
}

impl WamiEnforcement {
    /// Replace the policy store with `definition`.
    ///
    /// The store is swapped only once every document has been read: on any
    /// error the previous grants stay in force, never a mix of two files. The
    /// identity registry is NOT rolled back: a tenant first seen while reading
    /// keeps its segment (allocation is monotonic, and the same segment would be
    /// allocated on that tenant's first request anyway).
    pub fn load_definition(&mut self, definition: &PolicyDefinition) -> Result<()> {
        let mut store = PolicyStore::new();
        for (role, policies) in &definition.roles {
            let rendered: Vec<(String, String)> = policies
                .iter()
                .map(|(name, doc)| Ok((name.clone(), serde_json::to_string(doc)?)))
                .collect::<Result<_>>()?;
            let borrowed: Vec<(&str, &str)> = rendered
                .iter()
                .map(|(n, j)| (n.as_str(), j.as_str()))
                .collect();
            store.register_role(role, &borrowed)?;
        }
        for principal in &definition.principals {
            let tenant = self.registry.tenant_path(principal.tenant)?;
            let key = ScopedName::new(tenant, principal.user.to_string());
            let rendered: Vec<(String, String)> = principal
                .policies
                .iter()
                .map(|(name, doc)| Ok((name.clone(), serde_json::to_string(doc)?)))
                .collect::<Result<_>>()?;
            let borrowed: Vec<(&str, &str)> = rendered
                .iter()
                .map(|(n, j)| (n.as_str(), j.as_str()))
                .collect();
            let boundary_json = principal
                .boundary
                .as_ref()
                .map(|b| {
                    Ok::<_, anyhow::Error>((b.arn.clone(), serde_json::to_string(&b.document)?))
                })
                .transpose()?;
            let boundary = boundary_json
                .as_ref()
                .map(|(arn, json)| (arn.as_str(), json.as_str()));
            store.register(key, &borrowed, boundary)?;
        }
        self.store = store;
        Ok(())
    }

    /// The enforcement a `wami` config section describes, or `None` when it sets
    /// no `policies_path` (WAMI stays unarmed; `perm:` is then unevaluable).
    ///
    /// Outside `bootstrap` mode, `identity_path` is required with
    /// `policies_path`: an in-memory registry would reallocate tenant segments at
    /// every restart. An unreadable file is an error: the host must not start on
    /// grants it could not read.
    pub fn from_config(config: &crate::config::auth::WamiConfig) -> Result<Option<Self>> {
        use crate::config::auth::AuthMode;
        let Some(policies_path) = &config.policies_path else {
            return Ok(None);
        };
        let registry = match &config.identity_path {
            Some(path) => IdentityRegistry::with_journal(path)?,
            None if matches!(config.mode, AuthMode::Bootstrap) => IdentityRegistry::new(),
            None => bail!(
                "wami.identity_path is required with wami.policies_path outside bootstrap mode: \
                 an in-memory identity registry reallocates tenant segments at every restart"
            ),
        };
        let instance = config
            .issuer
            .clone()
            .unwrap_or_else(|| "this-rs".to_string());
        let mut enforcement = Self::new(registry, PolicyStore::new(), instance);
        let text = std::fs::read_to_string(policies_path)
            .map_err(|e| anyhow!("reading wami policies {}: {e}", policies_path.display()))?;
        enforcement
            .load_definition_json(&text)
            .map_err(|e| anyhow!("wami policies {}: {e}", policies_path.display()))?;
        Ok(Some(enforcement))
    }

    /// Parse and load a JSON definition. See [`WamiEnforcement::load_definition`].
    pub fn load_definition_json(&mut self, json: &str) -> Result<()> {
        let definition: PolicyDefinition =
            serde_json::from_str(json).map_err(|e| anyhow!("policy definition unreadable: {e}"))?;
        self.load_definition(&definition)
    }

    /// The identity registry's `(tenant, segment)` pairs, to persist.
    pub fn identity_snapshot(&self) -> Result<Vec<(Uuid, u64)>> {
        self.registry.snapshot()
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
            scopes: Vec::new(),
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
        // The historical descriptor form uses `:`; both separators are one action.
        assert_eq!(
            wami_action_for("perm:orders:write").as_deref(),
            Some("this:orders:write")
        );
        assert_eq!(
            wami_action_for("perm:orders.write"),
            wami_action_for("perm:orders:write")
        );
        for bad in [
            "", "perm:", "a..b", "a::b", "a b", "../x", "a.*", "a:*", ".a", "a.", ":a",
        ] {
            assert_eq!(wami_action_for(bad), None, "must refuse {bad:?}");
        }
    }

    /// The `perm:` strings this repository's own descriptors and tests use must
    /// all map, or a policy that works today stops working the day wami is armed.
    #[test]
    fn real_descriptor_permissions_all_map() {
        for perm in [
            "perm:orders:write",
            "perm:orders:read",
            "perm:graph:read",
            "perm:invoices.read",
            "perm:project.read",
        ] {
            assert!(
                wami_action_for(perm).is_some(),
                "{perm} must map to an action"
            );
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

    const ALLOW_ONE_ORDER: &str = r#"{
        "Version": "2012-10-17",
        "Statement": [
            {"Effect": "Allow", "Action": "this:orders:read",
             "Resource": "arn:wami:iam:${tenant}:wami:this:orders/11111111-1111-1111-1111-111111111111"}
        ]
    }"#;

    const ALLOW_ALL_ORDERS: &str = r#"{
        "Version": "2012-10-17",
        "Statement": [
            {"Effect": "Allow", "Action": "this:orders:*",
             "Resource": "arn:wami:iam:${tenant}:wami:this:orders/*"}
        ]
    }"#;

    const ALLOW_OWNS_LINK: &str = r#"{
        "Version": "2012-10-17",
        "Statement": [
            {"Effect": "Allow", "Action": "this:link:create",
             "Resource": "arn:wami:iam:${tenant}:wami:this:link/owns/**"}
        ]
    }"#;

    #[test]
    fn resource_arn_names_rows_types_and_links() {
        let reg = IdentityRegistry::new();
        let ctx = ctx_for(Uuid::new_v4(), Uuid::new_v4(), &reg);
        let id = Uuid::nil();
        let row = resource_arn(
            &ctx,
            &ResourceRef::Entity {
                entity: "orders",
                id: Some(&id),
            },
        )
        .unwrap();
        assert!(
            row.to_string()
                .ends_with(":orders/00000000-0000-0000-0000-000000000000")
        );
        let whole = resource_arn(
            &ctx,
            &ResourceRef::Entity {
                entity: "orders",
                id: None,
            },
        )
        .unwrap();
        assert!(whole.to_string().ends_with(":orders/*"));
        let link = resource_arn(
            &ctx,
            &ResourceRef::Link {
                link_type: "owns",
                source: Some("users"),
                target: Some("orders"),
            },
        )
        .unwrap();
        assert!(link.to_string().ends_with(":link/owns/users/orders"));
        let partial = resource_arn(
            &ctx,
            &ResourceRef::Link {
                link_type: "owns",
                source: None,
                target: None,
            },
        )
        .unwrap();
        assert!(partial.to_string().ends_with(":link/owns/*/*"));
        // A name that would alter the pattern is refused, not matched.
        assert!(
            resource_arn(
                &ctx,
                &ResourceRef::Entity {
                    entity: "orders/*",
                    id: None,
                },
            )
            .is_none()
        );
    }

    #[test]
    fn a_statement_on_one_row_matches_that_row_only() {
        let reg = IdentityRegistry::new();
        let ctx = ctx_for(Uuid::new_v4(), Uuid::new_v4(), &reg);
        let mut store = PolicyStore::new();
        store
            .register(key_of(&ctx), &[("one", ALLOW_ONE_ORDER)], None)
            .unwrap();
        let mine = Uuid::parse_str("11111111-1111-1111-1111-111111111111").unwrap();
        let other = Uuid::parse_str("22222222-2222-2222-2222-222222222222").unwrap();
        let on = |id: Option<&Uuid>| {
            store.verdict_on(
                &ctx,
                &[],
                "perm:orders:read",
                &ResourceRef::Entity {
                    entity: "orders",
                    id,
                },
            )
        };
        assert_eq!(on(Some(&mine)), Verdict::Allow);
        assert_eq!(on(Some(&other)), Verdict::Deny);
        // The whole type (a list) is not one row: refused.
        assert_eq!(on(None), Verdict::Deny);
        // And "no particular target" is not that row either.
        assert_eq!(
            store.verdict_on(&ctx, &[], "perm:orders:read", &ResourceRef::Any),
            Verdict::Deny
        );
    }

    #[test]
    fn a_statement_on_the_type_covers_rows_and_lists_but_not_other_types() {
        let reg = IdentityRegistry::new();
        let ctx = ctx_for(Uuid::new_v4(), Uuid::new_v4(), &reg);
        let mut store = PolicyStore::new();
        store
            .register(key_of(&ctx), &[("orders", ALLOW_ALL_ORDERS)], None)
            .unwrap();
        let id = Uuid::new_v4();
        let orders_row = ResourceRef::Entity {
            entity: "orders",
            id: Some(&id),
        };
        let orders_all = ResourceRef::Entity {
            entity: "orders",
            id: None,
        };
        let invoices = ResourceRef::Entity {
            entity: "invoices",
            id: None,
        };
        assert_eq!(
            store.verdict_on(&ctx, &[], "perm:orders:read", &orders_row),
            Verdict::Allow
        );
        assert_eq!(
            store.verdict_on(&ctx, &[], "perm:orders:list", &orders_all),
            Verdict::Allow
        );
        assert_eq!(
            store.verdict_on(&ctx, &[], "perm:orders:read", &invoices),
            Verdict::Deny
        );
    }

    #[test]
    fn link_statements_match_the_link_type() {
        let reg = IdentityRegistry::new();
        let ctx = ctx_for(Uuid::new_v4(), Uuid::new_v4(), &reg);
        let mut store = PolicyStore::new();
        store
            .register(key_of(&ctx), &[("owns", ALLOW_OWNS_LINK)], None)
            .unwrap();
        let owns = ResourceRef::Link {
            link_type: "owns",
            source: Some("users"),
            target: Some("orders"),
        };
        let manages = ResourceRef::Link {
            link_type: "manages",
            source: Some("users"),
            target: Some("orders"),
        };
        assert_eq!(
            store.verdict_on(&ctx, &[], "perm:link:create", &owns),
            Verdict::Allow
        );
        assert_eq!(
            store.verdict_on(&ctx, &[], "perm:link:create", &manages),
            Verdict::Deny
        );
    }

    /// A Deny written on a link type must bite even when the surface only knows
    /// the type (gRPC CreateLink, delete by id): the partial link is `owns/*/*`,
    /// which `link/owns/**` covers. And `*` on the request side is literal: it
    /// does not match a statement on one specific endpoint pair.
    #[test]
    fn a_partially_named_link_still_meets_a_deny_on_its_type() {
        let reg = IdentityRegistry::new();
        let ctx = ctx_for(Uuid::new_v4(), Uuid::new_v4(), &reg);
        let deny_owns = r#"{
            "Version": "2012-10-17",
            "Statement": [
                {"Effect": "Allow", "Action": "this:*", "Resource": "*"},
                {"Effect": "Deny", "Action": "this:link:create",
                 "Resource": "arn:wami:iam:${tenant}:wami:this:link/owns/**"}
            ]
        }"#;
        let mut store = PolicyStore::new();
        store
            .register(key_of(&ctx), &[("d", deny_owns)], None)
            .unwrap();
        let only_type = ResourceRef::Link {
            link_type: "owns",
            source: None,
            target: None,
        };
        assert_eq!(
            store.verdict_on(&ctx, &[], "perm:link:create", &only_type),
            Verdict::Deny
        );
        // Another link type, partially named, is still allowed by the first statement.
        let other = ResourceRef::Link {
            link_type: "manages",
            source: None,
            target: None,
        };
        assert_eq!(
            store.verdict_on(&ctx, &[], "perm:link:create", &other),
            Verdict::Allow
        );
        // A request-side `*` does not match a statement on one specific pair.
        let mut specific = PolicyStore::new();
        specific
            .register(key_of(&ctx), &[("owns", ALLOW_OWNS_LINK_SPECIFIC)], None)
            .unwrap();
        assert_eq!(
            specific.verdict_on(&ctx, &[], "perm:link:create", &only_type),
            Verdict::Deny
        );
    }

    const ALLOW_OWNS_LINK_SPECIFIC: &str = r#"{
        "Version": "2012-10-17",
        "Statement": [
            {"Effect": "Allow", "Action": "this:link:create",
             "Resource": "arn:wami:iam:${tenant}:wami:this:link/owns/users/orders"}
        ]
    }"#;

    /// `NotIpAddress` holds on an absent IP (AWS semantics), which would make a
    /// conditional Allow unconditional on a host that cannot learn the IP. Such a
    /// document is unevaluable without a known IP, and decided normally with one.
    #[test]
    fn negative_ip_conditions_are_unevaluable_without_a_known_ip() {
        use std::net::{IpAddr, Ipv4Addr};
        let mut enf = WamiEnforcement::new(IdentityRegistry::new(), PolicyStore::new(), "this");
        let auth = AuthContext::User {
            user_id: Uuid::new_v4(),
            tenant_id: Uuid::new_v4(),
            roles: vec![],
            scopes: Vec::new(),
        };
        let key = enf
            .caller_context(&auth, &RequestMeta::default())
            .unwrap()
            .unwrap()
            .caller_key()
            .unwrap()
            .clone();
        let all_but_banned = r#"{
            "Version": "2012-10-17",
            "Statement": [{
                "Effect": "Allow", "Action": "this:project:read", "Resource": "*",
                "Condition": {"NotIpAddress": {"aws:SourceIp": "203.0.113.0/24"}}
            }]
        }"#;
        enf.store
            .register(key, &[("banned", all_but_banned)], None)
            .unwrap();
        let p = "perm:project.read";
        assert_eq!(
            enf.verdict_for(&auth, &RequestMeta::default(), p, &ResourceRef::Any),
            Some(Verdict::Unevaluable)
        );
        let banned = RequestMeta {
            source_ip: Some(IpAddr::V4(Ipv4Addr::new(203, 0, 113, 7))),
            ..RequestMeta::default()
        };
        let elsewhere = RequestMeta {
            source_ip: Some(IpAddr::V4(Ipv4Addr::new(198, 51, 100, 7))),
            ..RequestMeta::default()
        };
        assert_eq!(
            enf.verdict_for(&auth, &banned, p, &ResourceRef::Any),
            Some(Verdict::Deny)
        );
        assert_eq!(
            enf.verdict_for(&auth, &elsewhere, p, &ResourceRef::Any),
            Some(Verdict::Allow)
        );
    }

    fn wami_config(
        mode: crate::config::auth::AuthMode,
        policies: Option<std::path::PathBuf>,
        identity: Option<std::path::PathBuf>,
    ) -> crate::config::auth::WamiConfig {
        crate::config::auth::WamiConfig {
            public_key: None,
            private_key: None,
            issuer: Some("this-test".to_string()),
            audience: Some("this-rs".to_string()),
            mode,
            token_ttl_secs: 60,
            refresh_ttl_secs: 600,
            accept_legacy_tokens: None,
            policies_path: policies,
            identity_path: identity,
        }
    }

    #[test]
    fn from_config_arms_from_files_and_requires_a_journal_outside_bootstrap() {
        use crate::config::auth::AuthMode;
        let dir = tempfile::tempdir().unwrap();
        let policies = dir.path().join("policies.json");
        let identity = dir.path().join("identity.json");
        let tenant = Uuid::new_v4();
        let alice = Uuid::new_v4();
        std::fs::write(
            &policies,
            serde_json::json!({
                "principals": [{"tenant": tenant, "user": alice,
                    "policies": {"all": serde_json::from_str::<serde_json::Value>(ALLOW_ALL).unwrap()}}]
            })
            .to_string(),
        )
        .unwrap();

        // No policies_path: unarmed.
        assert!(
            WamiEnforcement::from_config(&wami_config(AuthMode::Sts, None, None))
                .unwrap()
                .is_none()
        );
        // Sts mode without a journal: refused.
        assert!(
            WamiEnforcement::from_config(&wami_config(AuthMode::Sts, Some(policies.clone()), None))
                .is_err()
        );
        // Bootstrap tolerates an in-memory registry.
        assert!(
            WamiEnforcement::from_config(&wami_config(
                AuthMode::Bootstrap,
                Some(policies.clone()),
                None
            ))
            .unwrap()
            .is_some()
        );
        // Sts with both files: armed, and the grants answer.
        let enf = WamiEnforcement::from_config(&wami_config(
            AuthMode::Sts,
            Some(policies.clone()),
            Some(identity.clone()),
        ))
        .unwrap()
        .unwrap();
        assert_eq!(enf.instance_id, "this-test");
        let auth = AuthContext::User {
            user_id: alice,
            tenant_id: tenant,
            roles: vec![],
            scopes: Vec::new(),
        };
        assert_eq!(
            enf.verdict_for(
                &auth,
                &RequestMeta::default(),
                "perm:project.read",
                &ResourceRef::Any
            ),
            Some(Verdict::Allow)
        );
        assert!(
            identity.exists(),
            "the tenant seen while loading was journaled"
        );

        // An unreadable policies file refuses to build.
        std::fs::write(&policies, "{ not json").unwrap();
        assert!(
            WamiEnforcement::from_config(&wami_config(
                AuthMode::Sts,
                Some(policies),
                Some(identity)
            ))
            .is_err()
        );
    }

    #[test]
    fn definition_refuses_unknown_fields() {
        let mut enf = WamiEnforcement::new(IdentityRegistry::new(), PolicyStore::new(), "this");
        let misspelt_boundary = serde_json::json!({
            "principals": [{"tenant": Uuid::new_v4(), "user": Uuid::new_v4(),
                "policies": {"all": serde_json::from_str::<serde_json::Value>(ALLOW_ALL).unwrap()},
                "boundry": {"arn": "x", "document": serde_json::from_str::<serde_json::Value>(BOUNDARY_READ_ONLY).unwrap()}}]
        });
        assert!(
            enf.load_definition_json(&misspelt_boundary.to_string())
                .is_err()
        );
    }

    #[test]
    fn roles_grant_like_groups_and_a_role_deny_overrides() {
        let reg = IdentityRegistry::new();
        let ctx = ctx_for(Uuid::new_v4(), Uuid::new_v4(), &reg);
        let mut store = PolicyStore::new();
        store
            .register_role("reader", &[("read", ALLOW_PROJECT_READ)])
            .unwrap();
        store
            .register_role("banned", &[("deny", DENY_PROJECT_READ)])
            .unwrap();
        let reader = vec!["reader".to_string()];
        let both = vec!["reader".to_string(), "banned".to_string()];
        let unknown = vec!["ghost".to_string()];

        // A caller with no own grants reads its roles' documents.
        assert_eq!(
            store.verdict_on(&ctx, &reader, "perm:project.read", &ResourceRef::Any),
            Verdict::Allow
        );
        // A Deny in another role overrides the Allow.
        assert_eq!(
            store.verdict_on(&ctx, &both, "perm:project.read", &ResourceRef::Any),
            Verdict::Deny
        );
        // A role nobody registered grants nothing.
        assert_eq!(
            store.verdict_on(&ctx, &unknown, "perm:project.read", &ResourceRef::Any),
            Verdict::Deny
        );
        // The principal's boundary still narrows what a role grants.
        let mut bounded = PolicyStore::new();
        bounded.register_role("all", &[("all", ALLOW_ALL)]).unwrap();
        bounded
            .register(
                key_of(&ctx),
                &[],
                Some(("arn:boundary/read-only", BOUNDARY_READ_ONLY)),
            )
            .unwrap();
        let all = vec!["all".to_string()];
        assert_eq!(
            bounded.verdict_on(&ctx, &all, "perm:project.read", &ResourceRef::Any),
            Verdict::Allow
        );
        assert_eq!(
            bounded.verdict_on(&ctx, &all, "perm:project.write", &ResourceRef::Any),
            Verdict::Deny
        );
    }

    #[test]
    fn request_facts_reach_the_conditions() {
        use std::net::{IpAddr, Ipv4Addr};
        let mut enf = WamiEnforcement::new(IdentityRegistry::new(), PolicyStore::new(), "this");
        let auth = AuthContext::User {
            user_id: Uuid::new_v4(),
            tenant_id: Uuid::new_v4(),
            roles: vec![],
            scopes: Vec::new(),
        };
        let key = enf
            .caller_context(&auth, &RequestMeta::default())
            .unwrap()
            .unwrap()
            .caller_key()
            .unwrap()
            .clone();
        let office_only = r#"{
            "Version": "2012-10-17",
            "Statement": [{
                "Effect": "Allow", "Action": "this:project:read", "Resource": "*",
                "Condition": {"IpAddress": {"aws:SourceIp": "10.0.0.0/8"}}
            }]
        }"#;
        enf.store
            .register(key, &[("office", office_only)], None)
            .unwrap();
        let from_office = RequestMeta {
            source_ip: Some(IpAddr::V4(Ipv4Addr::new(10, 1, 2, 3))),
            ..RequestMeta::default()
        };
        let from_outside = RequestMeta {
            source_ip: Some(IpAddr::V4(Ipv4Addr::new(203, 0, 113, 9))),
            ..RequestMeta::default()
        };
        let p = "perm:project.read";
        assert_eq!(
            enf.verdict_for(&auth, &from_office, p, &ResourceRef::Any),
            Some(Verdict::Allow)
        );
        assert_eq!(
            enf.verdict_for(&auth, &from_outside, p, &ResourceRef::Any),
            Some(Verdict::Deny)
        );
        // No IP known: the condition cannot hold, the statement does not apply.
        assert_eq!(
            enf.verdict_for(&auth, &RequestMeta::default(), p, &ResourceRef::Any),
            Some(Verdict::Deny)
        );
    }

    #[test]
    fn definition_loads_principals_roles_and_boundaries_atomically() {
        let tenant = Uuid::new_v4();
        let alice = Uuid::new_v4();
        let bob = Uuid::new_v4();
        let definition = serde_json::json!({
            "roles": {
                "reader": {"read": serde_json::from_str::<serde_json::Value>(ALLOW_PROJECT_READ).unwrap()}
            },
            "principals": [
                {"tenant": tenant, "user": alice,
                 "policies": {"all": serde_json::from_str::<serde_json::Value>(ALLOW_ALL).unwrap()},
                 "boundary": {"arn": "arn:boundary/read-only",
                              "document": serde_json::from_str::<serde_json::Value>(BOUNDARY_READ_ONLY).unwrap()}},
                {"tenant": tenant, "user": bob}
            ]
        });
        let mut enf = WamiEnforcement::new(IdentityRegistry::new(), PolicyStore::new(), "this");
        enf.load_definition_json(&definition.to_string()).unwrap();

        let user = |id: Uuid, roles: &[&str]| AuthContext::User {
            user_id: id,
            tenant_id: tenant,
            roles: roles.iter().map(|r| r.to_string()).collect(),
            scopes: Vec::new(),
        };
        let meta = RequestMeta::default();
        let any = ResourceRef::Any;
        // Alice: everything, narrowed by her boundary.
        assert_eq!(
            enf.verdict_for(&user(alice, &[]), &meta, "perm:project.read", &any),
            Some(Verdict::Allow)
        );
        assert_eq!(
            enf.verdict_for(&user(alice, &[]), &meta, "perm:project.write", &any),
            Some(Verdict::Deny)
        );
        // Bob: nothing of his own, the reader role when his token carries it.
        assert_eq!(
            enf.verdict_for(&user(bob, &[]), &meta, "perm:project.read", &any),
            Some(Verdict::Deny)
        );
        assert_eq!(
            enf.verdict_for(&user(bob, &["reader"]), &meta, "perm:project.read", &any),
            Some(Verdict::Allow)
        );

        // A broken file changes nothing: the previous store stays in force.
        let broken = serde_json::json!({
            "principals": [{"tenant": tenant, "user": bob,
                "policies": {"bad": {"Version": "2012-10-17", "Statement": [{"Effect": "Allow", "Action": "this:*", "Resource": "*", "Typo": 1}]}}}]
        });
        assert!(enf.load_definition_json(&broken.to_string()).is_err());
        assert_eq!(
            enf.verdict_for(&user(alice, &[]), &meta, "perm:project.read", &any),
            Some(Verdict::Allow)
        );
        assert!(enf.load_definition_json("not json").is_err());

        // The registry allocated the tenant once; the snapshot restores it.
        let snapshot = enf.identity_snapshot().unwrap();
        assert_eq!(snapshot.len(), 1);
        assert_eq!(snapshot[0].0, tenant);
        let restored = IdentityRegistry::new();
        restored.load(snapshot.iter().copied()).unwrap();
        assert_eq!(
            restored.tenant_segment(tenant).unwrap(),
            enf.registry.tenant_segment(tenant).unwrap()
        );
    }

    #[test]
    fn enforcement_answers_only_for_named_callers() {
        let mut enf = WamiEnforcement::new(IdentityRegistry::new(), PolicyStore::new(), "this");
        let auth = AuthContext::User {
            user_id: Uuid::new_v4(),
            tenant_id: Uuid::new_v4(),
            roles: vec![],
            scopes: Vec::new(),
        };
        let ctx = enf
            .caller_context(&auth, &RequestMeta::default())
            .unwrap()
            .unwrap();
        enf.store
            .register(key_of(&ctx), &[("write", ALLOW_PROJECT_WRITE)], None)
            .unwrap();

        // The named caller is answered from its own grants.
        let verdict = enf
            .caller_context(&auth, &RequestMeta::default())
            .unwrap()
            .map(|c| enf.store.verdict(&c, "perm:project.write"));
        assert_eq!(verdict, Some(Verdict::Allow));

        // A caller WAMI cannot name has no WAMI identity at all: no answer, so none
        // can be read as an allow.
        assert!(
            enf.caller_context(&AuthContext::Anonymous, &RequestMeta::default())
                .unwrap()
                .is_none()
        );
        assert!(
            enf.caller_context(
                &AuthContext::Admin {
                    admin_id: Uuid::new_v4()
                },
                &RequestMeta::default()
            )
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
