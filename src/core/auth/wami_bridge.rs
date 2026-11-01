//! Bridge from this-rs identities to wami's identity vocabulary.
//!
//! `this` identifies tenants and users by `Uuid`; wami identifies a tenant by
//! a [`TenantPath`] of `u64` segments and a principal by an ARN / a
//! [`ScopedName`] `(tenant, name)`. This module is the only place that
//! translates between the two.
//!
//! # Decisions (see `docs/wami-core-migration.md`)
//!
//! - **D1 — identity registry.** A tenant `Uuid` is mapped to a `u64` segment
//!   by an [`IdentityRegistry`] that *allocates* segments and refuses any reuse.
//!   Hash-derived segments are not used: a `u64` collision between two tenants
//!   would let one cross into the other.
//! - **Trust boundary.** [`wami_context_for`] builds a [`WamiContext`] from an
//!   [`AuthContext`] that the caller has already verified (JWT signature,
//!   expiry, audience). It never builds one from unverified input.
//! - **Fail-closed by absence.** `Anonymous`, `Service` and `Admin` produce
//!   `None`: they have no WAMI caller identity. `None` means *no WAMI decision
//!   was made*, not *allowed*. Callers must not treat it as an allow.
//!
//! # Known limit
//!
//! The registry is in memory. Segments must be persisted and restored with
//! [`IdentityRegistry::load`] before a multi-process or restart-safe
//! deployment, or an ARN minted before a restart may point at another tenant.

use crate::core::auth::AuthContext;
use anyhow::{Result, anyhow, bail};
use axum::http::HeaderMap;
use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::Mutex;
use uuid::Uuid;
use wami_core::WamiContext;
use wami_core::arn::{ScopedName, Service, TenantPath, WamiArn};

/// Segment reserved for the wami root tenant (`TenantPath` `[0]`). Never allocated.
const ROOT_SEGMENT: u64 = 0;

#[derive(Debug, Default)]
struct RegistryState {
    by_uuid: HashMap<Uuid, u64>,
    by_segment: HashMap<u64, Uuid>,
    next: u64,
}

/// Maps tenant `Uuid`s to wami tenant segments without ever reusing one.
///
/// Segments are allocated monotonically from `1`; `0` is the wami root tenant.
#[derive(Debug)]
pub struct IdentityRegistry {
    state: Mutex<RegistryState>,
}

impl Default for IdentityRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl IdentityRegistry {
    /// An empty registry. The first allocated segment is `1`.
    pub fn new() -> Self {
        Self {
            state: Mutex::new(RegistryState {
                next: ROOT_SEGMENT + 1,
                ..RegistryState::default()
            }),
        }
    }

    fn lock(&self) -> Result<std::sync::MutexGuard<'_, RegistryState>> {
        self.state
            .lock()
            .map_err(|_| anyhow!("identity registry lock poisoned"))
    }

    /// The segment of `tenant`, allocating one on first use.
    pub fn tenant_segment(&self, tenant: Uuid) -> Result<u64> {
        let mut state = self.lock()?;
        if let Some(segment) = state.by_uuid.get(&tenant) {
            return Ok(*segment);
        }
        let segment = state.next;
        state.next = segment
            .checked_add(1)
            .ok_or_else(|| anyhow!("identity registry exhausted its u64 segment space"))?;
        if state.by_segment.contains_key(&segment) {
            // Unreachable while allocation is monotonic; kept as a hard refusal.
            bail!("segment {segment} already allocated: refusing reuse");
        }
        state.by_uuid.insert(tenant, segment);
        state.by_segment.insert(segment, tenant);
        Ok(segment)
    }

    /// The [`TenantPath`] of `tenant`, allocating a segment on first use.
    pub fn tenant_path(&self, tenant: Uuid) -> Result<TenantPath> {
        Ok(TenantPath::new(vec![self.tenant_segment(tenant)?]))
    }

    /// Restore persisted `(tenant, segment)` pairs.
    ///
    /// Refuses the whole batch if any pair reuses a segment for another tenant,
    /// assigns two segments to one tenant, or uses the root segment. A registry
    /// that loaded a conflicting table would hand out a tenant's ARNs to someone else.
    pub fn load(&self, entries: impl IntoIterator<Item = (Uuid, u64)>) -> Result<()> {
        let mut state = self.lock()?;
        let mut staged_uuid: HashMap<Uuid, u64> = HashMap::new();
        let mut staged_segment: HashMap<u64, Uuid> = HashMap::new();
        for (tenant, segment) in entries {
            if segment == ROOT_SEGMENT {
                bail!("tenant {tenant} mapped to the root segment 0");
            }
            if let Some(existing) = staged_uuid.insert(tenant, segment)
                && existing != segment
            {
                bail!("tenant {tenant} mapped to two segments");
            }
            if let Some(other) = staged_segment.insert(segment, tenant)
                && other != tenant
            {
                bail!("segment {segment} mapped to tenants {other} and {tenant}");
            }
            if let Some(known) = state.by_uuid.get(&tenant)
                && *known != segment
            {
                bail!("tenant {tenant} already registered with another segment");
            }
            if let Some(known) = state.by_segment.get(&segment)
                && *known != tenant
            {
                bail!("segment {segment} already registered to tenant {known}");
            }
        }
        for (tenant, segment) in staged_uuid {
            state.by_uuid.insert(tenant, segment);
            state.by_segment.insert(segment, tenant);
            if segment >= state.next {
                state.next = segment + 1;
            }
        }
        Ok(())
    }
}

/// Per-request facts that wami conditions read.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RequestMeta {
    /// Client address used for IP conditions.
    pub source_ip: Option<IpAddr>,
    /// Whether the request arrived over TLS, as far as it is known.
    pub secure_transport: Option<bool>,
    /// Whether MFA was used. Unknown until the token carries an `amr` claim.
    pub mfa_present: Option<bool>,
}

/// Whether `X-Forwarded-*` headers may be read.
///
/// `Ignore` is the default: a client can set those headers itself.
/// Choose `Trust` only when the edge proxy overwrites them on every request.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ForwardedHeaders {
    #[default]
    Ignore,
    Trust,
}

impl RequestMeta {
    /// Extract the request facts from headers and the peer address.
    pub fn from_request(
        headers: &HeaderMap,
        peer: Option<IpAddr>,
        forwarded: ForwardedHeaders,
    ) -> Self {
        match forwarded {
            ForwardedHeaders::Ignore => Self {
                source_ip: peer,
                secure_transport: None,
                mfa_present: None,
            },
            ForwardedHeaders::Trust => {
                let header = |name: &str| headers.get(name).and_then(|v| v.to_str().ok());
                let source_ip = header("x-forwarded-for")
                    .and_then(|list| list.split(',').next())
                    .and_then(|first| first.trim().parse::<IpAddr>().ok())
                    .or(peer);
                let secure_transport = header("x-forwarded-proto")
                    .map(|proto| proto.trim().eq_ignore_ascii_case("https"));
                Self {
                    source_ip,
                    secure_transport,
                    mfa_present: None,
                }
            }
        }
    }
}

/// Build the WAMI caller context for a verified `AuthContext`.
///
/// Returns `Ok(None)` for `Anonymous`, `Service` and `Admin`: they have no WAMI
/// caller identity (see the module docs). For `User` and `Owner`, the caller is
/// the user in the tenant the token names; the principal ARN and the caller key
/// both sit in that tenant, so a context cannot point at another tenant's user.
pub fn wami_context_for(
    auth: &AuthContext,
    meta: &RequestMeta,
    registry: &IdentityRegistry,
    instance_id: &str,
) -> Result<Option<WamiContext>> {
    let (user_id, tenant_id) = match auth {
        AuthContext::User {
            user_id, tenant_id, ..
        }
        | AuthContext::Owner {
            user_id, tenant_id, ..
        } => (*user_id, *tenant_id),
        AuthContext::Anonymous | AuthContext::Service { .. } | AuthContext::Admin { .. } => {
            return Ok(None);
        }
    };

    let tenant = registry.tenant_path(tenant_id)?;
    let principal = user_id.to_string();

    let caller_arn = WamiArn::builder()
        .service(Service::Iam)
        .tenant_path(tenant.clone())
        .wami_instance(instance_id)
        .resource("user", principal.as_str())
        .build()?;

    let mut builder = WamiContext::builder()
        .tenant_path(tenant.clone())
        .instance_id(instance_id)
        .caller_arn(caller_arn)
        .caller_key(ScopedName::new(tenant, principal));

    if let Some(ip) = meta.source_ip {
        builder = builder.source_ip(ip.to_string());
    }
    if let Some(mfa) = meta.mfa_present {
        builder = builder.mfa_present(mfa);
    }
    if let Some(secure) = meta.secure_transport {
        builder = builder.secure_transport(secure);
    }

    Ok(Some(builder.build()?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;
    use std::net::Ipv4Addr;

    fn user(tenant: Uuid) -> AuthContext {
        AuthContext::User {
            user_id: Uuid::new_v4(),
            tenant_id: tenant,
            roles: vec![],
        }
    }

    #[test]
    fn same_tenant_gets_same_segment_and_segments_are_never_root() {
        let reg = IdentityRegistry::new();
        let t = Uuid::new_v4();
        let a = reg.tenant_segment(t).unwrap();
        assert_eq!(a, reg.tenant_segment(t).unwrap());
        assert_ne!(a, ROOT_SEGMENT);
    }

    #[test]
    fn distinct_tenants_get_distinct_segments() {
        let reg = IdentityRegistry::new();
        let segments: std::collections::HashSet<u64> = (0..200)
            .map(|_| reg.tenant_segment(Uuid::new_v4()).unwrap())
            .collect();
        assert_eq!(segments.len(), 200);
    }

    #[test]
    fn load_refuses_segment_reused_by_another_tenant() {
        let reg = IdentityRegistry::new();
        let (a, b) = (Uuid::new_v4(), Uuid::new_v4());
        assert!(reg.load([(a, 7), (b, 7)]).is_err());
    }

    #[test]
    fn load_refuses_tenant_with_two_segments_and_root_segment() {
        let reg = IdentityRegistry::new();
        let a = Uuid::new_v4();
        assert!(reg.load([(a, 3), (a, 4)]).is_err());
        assert!(reg.load([(Uuid::new_v4(), ROOT_SEGMENT)]).is_err());
    }

    #[test]
    fn load_refuses_conflict_with_existing_allocation_and_keeps_state() {
        let reg = IdentityRegistry::new();
        let known = Uuid::new_v4();
        let seg = reg.tenant_segment(known).unwrap();
        let intruder = Uuid::new_v4();
        assert!(reg.load([(intruder, seg)]).is_err());
        // The refused batch left the mapping untouched.
        assert_eq!(reg.tenant_segment(known).unwrap(), seg);
    }

    #[test]
    fn load_then_allocate_does_not_reuse_loaded_segments() {
        let reg = IdentityRegistry::new();
        let restored = Uuid::new_v4();
        reg.load([(restored, 50)]).unwrap();
        let fresh = reg.tenant_segment(Uuid::new_v4()).unwrap();
        assert!(fresh > 50);
    }

    #[test]
    fn anonymous_service_and_admin_have_no_wami_caller() {
        let reg = IdentityRegistry::new();
        let meta = RequestMeta::default();
        for auth in [
            AuthContext::Anonymous,
            AuthContext::Service {
                service_name: "svc".into(),
                tenant_id: None,
            },
            AuthContext::Admin {
                admin_id: Uuid::new_v4(),
            },
        ] {
            assert!(
                wami_context_for(&auth, &meta, &reg, "this")
                    .unwrap()
                    .is_none()
            );
        }
    }

    #[test]
    fn user_context_stays_inside_the_token_tenant() {
        let reg = IdentityRegistry::new();
        let tenant = Uuid::new_v4();
        let auth = user(tenant);
        let ctx = wami_context_for(&auth, &RequestMeta::default(), &reg, "this")
            .unwrap()
            .expect("user has a caller");
        let expected = reg.tenant_path(tenant).unwrap();
        assert_eq!(ctx.tenant_path(), &expected);
        assert_eq!(ctx.caller_key().unwrap().tenant(), &expected);
        assert_eq!(ctx.caller_arn().tenant_path, expected);
        assert!(!ctx.is_root());
    }

    #[test]
    fn two_tenants_never_share_a_caller_tenant() {
        let reg = IdentityRegistry::new();
        let (ta, tb) = (Uuid::new_v4(), Uuid::new_v4());
        let meta = RequestMeta::default();
        let a = wami_context_for(&user(ta), &meta, &reg, "this")
            .unwrap()
            .unwrap();
        let b = wami_context_for(&user(tb), &meta, &reg, "this")
            .unwrap()
            .unwrap();
        assert_ne!(a.tenant_path(), b.tenant_path());
    }

    #[test]
    fn request_facts_flow_into_context() {
        let reg = IdentityRegistry::new();
        let meta = RequestMeta {
            source_ip: Some(IpAddr::V4(Ipv4Addr::new(10, 0, 0, 9))),
            secure_transport: Some(true),
            mfa_present: Some(false),
        };
        let ctx = wami_context_for(&user(Uuid::new_v4()), &meta, &reg, "this")
            .unwrap()
            .unwrap();
        assert_eq!(ctx.source_ip(), Some("10.0.0.9"));
        assert_eq!(ctx.secure_transport(), Some(true));
        assert_eq!(ctx.mfa_present(), Some(false));
    }

    #[test]
    fn ignore_mode_never_reads_forwarded_headers() {
        let mut h = HeaderMap::new();
        h.insert("x-forwarded-for", HeaderValue::from_static("6.6.6.6"));
        h.insert("x-forwarded-proto", HeaderValue::from_static("https"));
        let peer = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1));
        let m = RequestMeta::from_request(&h, Some(peer), ForwardedHeaders::Ignore);
        assert_eq!(m.source_ip, Some(peer));
        assert_eq!(m.secure_transport, None);
    }

    #[test]
    fn trust_mode_reads_leftmost_forwarded_for_and_proto() {
        let mut h = HeaderMap::new();
        h.insert(
            "x-forwarded-for",
            HeaderValue::from_static("203.0.113.5, 10.0.0.2"),
        );
        h.insert("x-forwarded-proto", HeaderValue::from_static("HTTPS"));
        let m = RequestMeta::from_request(&h, None, ForwardedHeaders::Trust);
        assert_eq!(m.source_ip, Some(IpAddr::V4(Ipv4Addr::new(203, 0, 113, 5))));
        assert_eq!(m.secure_transport, Some(true));
    }
}
