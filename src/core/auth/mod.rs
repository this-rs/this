//! Authorization system for this-rs
//!
//! Provides context-based authorization with multiple auth types:
//! - User authentication
//! - Owner-based access
//! - Service-to-service
//! - Admin access
//! - Custom resolvers (delegation, parent/child, group membership, etc.)
//!
//! ## Custom Resolvers
//!
//! Register named resolvers via `ServerBuilder::with_auth_resolver()` and reference
//! them in YAML config with the `resolver:` prefix:
//!
//! ```yaml
//! auth:
//!   create: "resolver:is_delegated"
//!   update: "resolver:is_parent"
//!   delete: "resolver:is_group_member"
//! ```
//!
//! ```rust,ignore
//! ServerBuilder::new()
//!     .with_auth_resolver("is_delegated", |ctx, resource_id| {
//!         // Check delegation logic using JWT claims
//!         ctx.has_claim("delegations", resource_id)
//!     })
//!     .with_auth_resolver("is_parent", IsDelegatedResolver::new(db.clone()))
//!     .build_host()?;
//! ```
//!
//! ## Feature gates
//! - `wami` — enables `WamiAuthProvider` (Ed25519 JWT verification)

#[cfg(feature = "wami")]
pub mod sts;
#[cfg(feature = "wami")]
pub mod wami_provider;

use anyhow::Result;
use async_trait::async_trait;
use axum::http::HeaderMap;
use std::collections::HashMap;
use std::sync::Arc;
use uuid::Uuid;

/// Authorization context extracted from a request
#[derive(Debug, Clone)]
pub enum AuthContext {
    /// Authenticated user
    User {
        user_id: Uuid,
        tenant_id: Uuid,
        roles: Vec<String>,
    },

    /// Owner of a specific resource
    Owner {
        user_id: Uuid,
        tenant_id: Uuid,
        resource_id: Uuid,
        resource_type: String,
    },

    /// Service-to-service communication
    Service {
        service_name: String,
        tenant_id: Option<Uuid>,
    },

    /// System administrator
    Admin { admin_id: Uuid },

    /// No authentication (public access)
    Anonymous,
}

impl AuthContext {
    /// Get tenant_id from context if available
    pub fn tenant_id(&self) -> Option<Uuid> {
        match self {
            AuthContext::User { tenant_id, .. } => Some(*tenant_id),
            AuthContext::Owner { tenant_id, .. } => Some(*tenant_id),
            AuthContext::Service { tenant_id, .. } => *tenant_id,
            AuthContext::Admin { .. } => None,
            AuthContext::Anonymous => None,
        }
    }

    /// Check if context represents an admin
    pub fn is_admin(&self) -> bool {
        matches!(self, AuthContext::Admin { .. })
    }

    /// Check if context represents a service
    pub fn is_service(&self) -> bool {
        matches!(self, AuthContext::Service { .. })
    }

    /// Get user_id if available
    pub fn user_id(&self) -> Option<Uuid> {
        match self {
            AuthContext::User { user_id, .. } => Some(*user_id),
            AuthContext::Owner { user_id, .. } => Some(*user_id),
            _ => None,
        }
    }

    /// Get roles if available
    pub fn roles(&self) -> &[String] {
        match self {
            AuthContext::User { roles, .. } => roles,
            _ => &[],
        }
    }

    /// Check if user has a specific role
    pub fn has_role(&self, role: &str) -> bool {
        self.roles().iter().any(|r| r == role)
    }
}

/// Trait for custom authorization resolvers
///
/// Implement this trait to create reusable authorization logic that can be
/// referenced from YAML config with the `resolver:` prefix.
///
/// # Examples
///
/// ```rust,ignore
/// use this_rs::core::auth::{AuthResolver, AuthContext};
/// use uuid::Uuid;
///
/// /// Check if user has delegation rights on the resource
/// struct IsDelegatedResolver {
///     db: Arc<DbPool>,
/// }
///
/// impl AuthResolver for IsDelegatedResolver {
///     fn check(&self, ctx: &AuthContext, resource_id: Option<&Uuid>) -> bool {
///         let Some(user_id) = ctx.user_id() else { return false };
///         let Some(res_id) = resource_id else { return false };
///         // Check delegation table...
///         self.db.has_delegation(&user_id, res_id)
///     }
///
///     fn description(&self) -> &str {
///         "Checks if user has delegation rights on the resource"
///     }
/// }
/// ```
///
/// Resolvers can also be created from closures via `FnResolver`:
///
/// ```rust,ignore
/// builder.with_auth_resolver("is_manager", |ctx: &AuthContext, _res: Option<&Uuid>| {
///     ctx.has_role("manager")
/// });
/// ```
pub trait AuthResolver: Send + Sync {
    /// Check if the auth context satisfies this resolver's policy
    ///
    /// # Arguments
    ///
    /// * `ctx` - The authentication context extracted from the request
    /// * `resource_id` - Optional resource ID being accessed (available for entity/link operations)
    fn check(&self, ctx: &AuthContext, resource_id: Option<&Uuid>) -> bool;

    /// Human-readable description of what this resolver checks
    fn description(&self) -> &str {
        "Custom auth resolver"
    }
}

/// Wrapper to use closures as `AuthResolver`
///
/// Created automatically by `ServerBuilder::with_auth_resolver()` when
/// passing a closure instead of a struct implementing `AuthResolver`.
pub struct FnResolver<F>
where
    F: Fn(&AuthContext, Option<&Uuid>) -> bool + Send + Sync,
{
    func: F,
    desc: String,
}

impl<F> FnResolver<F>
where
    F: Fn(&AuthContext, Option<&Uuid>) -> bool + Send + Sync,
{
    /// Create a new FnResolver with a description
    pub fn new(func: F, description: impl Into<String>) -> Self {
        Self {
            func,
            desc: description.into(),
        }
    }
}

impl<F> AuthResolver for FnResolver<F>
where
    F: Fn(&AuthContext, Option<&Uuid>) -> bool + Send + Sync,
{
    fn check(&self, ctx: &AuthContext, resource_id: Option<&Uuid>) -> bool {
        (self.func)(ctx, resource_id)
    }

    fn description(&self) -> &str {
        &self.desc
    }
}

/// Registry of named auth resolvers
///
/// Stores custom resolvers that can be referenced from YAML config.
/// Thread-safe (uses Arc internally) and cheaply cloneable.
///
/// # YAML reference
///
/// ```yaml
/// entities:
///   order:
///     auth:
///       update: "resolver:is_delegated"
///       delete: "resolver:is_parent_or_admin"
///
/// links:
///   - link_type: order_items
///     auth:
///       create: "resolver:is_group_member"
/// ```
#[derive(Clone, Default)]
pub struct AuthResolverRegistry {
    resolvers: HashMap<String, Arc<dyn AuthResolver>>,
}

impl AuthResolverRegistry {
    /// Create an empty registry
    pub fn new() -> Self {
        Self {
            resolvers: HashMap::new(),
        }
    }

    /// Register a named resolver
    ///
    /// The name is used in YAML config with the `resolver:` prefix.
    /// For example, registering "is_delegated" allows YAML `auth: "resolver:is_delegated"`.
    pub fn register(&mut self, name: impl Into<String>, resolver: Arc<dyn AuthResolver>) {
        self.resolvers.insert(name.into(), resolver);
    }

    /// Get a resolver by name
    pub fn get(&self, name: &str) -> Option<&Arc<dyn AuthResolver>> {
        self.resolvers.get(name)
    }

    /// Check if a named resolver exists
    pub fn contains(&self, name: &str) -> bool {
        self.resolvers.contains_key(name)
    }

    /// List all registered resolver names
    pub fn names(&self) -> Vec<&str> {
        self.resolvers.keys().map(|k| k.as_str()).collect()
    }

    /// Number of registered resolvers
    pub fn len(&self) -> usize {
        self.resolvers.len()
    }

    /// Check if registry is empty
    pub fn is_empty(&self) -> bool {
        self.resolvers.is_empty()
    }
}

impl std::fmt::Debug for AuthResolverRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AuthResolverRegistry")
            .field("resolvers", &self.resolvers.keys().collect::<Vec<_>>())
            .finish()
    }
}

/// Authorization policy for an operation
#[derive(Debug, Clone)]
pub enum AuthPolicy {
    /// Public access (no auth required)
    Public,

    /// Any authenticated user
    Authenticated,

    /// Owner of the resource only
    Owner,

    /// User must have one of these roles
    HasRole(Vec<String>),

    /// Service-to-service only
    ServiceOnly,

    /// Admin only
    AdminOnly,

    /// Combination of policies (AND)
    And(Vec<AuthPolicy>),

    /// Combination of policies (OR)
    Or(Vec<AuthPolicy>),

    /// Custom policy function
    Custom(fn(&AuthContext) -> bool),

    /// Negation of a policy (parsed from `not:policy` in YAML)
    ///
    /// Inverts the inner policy result. Useful for deny lists:
    /// ```yaml
    /// auth:
    ///   update: "all:authenticated,not:resolver:is_readonly_mode"
    /// ```
    Not(Box<AuthPolicy>),

    /// Named custom resolver (referenced from YAML as `resolver:name`)
    ///
    /// The resolver is looked up in the `AuthResolverRegistry` at check time.
    /// If the resolver is not found, the policy **denies** access (fail-closed).
    ///
    /// # YAML syntax
    ///
    /// ```yaml
    /// auth:
    ///   create: "resolver:is_delegated"
    ///   update: "resolver:is_parent"
    ///   delete: "resolver:is_group_member"
    /// ```
    ///
    /// Combinable with OR:
    /// ```yaml
    /// auth:
    ///   update: "owner_or_resolver:is_delegated"
    /// ```
    Resolver(String),
}

impl AuthPolicy {
    /// Check if auth context satisfies this policy
    pub fn check(&self, context: &AuthContext) -> bool {
        self.check_with_resolver(context, None, None)
    }

    /// Check if auth context satisfies this policy, with resolver registry and resource context
    ///
    /// This is the full check method that supports custom resolvers.
    /// The simpler `check()` method delegates here with `None` values.
    pub fn check_with_resolver(
        &self,
        context: &AuthContext,
        registry: Option<&AuthResolverRegistry>,
        resource_id: Option<&Uuid>,
    ) -> bool {
        match self {
            AuthPolicy::Public => true,

            AuthPolicy::Authenticated => !matches!(context, AuthContext::Anonymous),

            AuthPolicy::Owner => matches!(context, AuthContext::Owner { .. }),

            AuthPolicy::HasRole(required_roles) => match context {
                AuthContext::User { roles, .. } => required_roles.iter().any(|r| roles.contains(r)),
                _ => false,
            },

            AuthPolicy::ServiceOnly => context.is_service(),

            AuthPolicy::AdminOnly => context.is_admin(),

            AuthPolicy::And(policies) => policies
                .iter()
                .all(|p| p.check_with_resolver(context, registry, resource_id)),

            AuthPolicy::Or(policies) => policies
                .iter()
                .any(|p| p.check_with_resolver(context, registry, resource_id)),

            AuthPolicy::Custom(f) => f(context),

            AuthPolicy::Not(inner) => !inner.check_with_resolver(context, registry, resource_id),

            AuthPolicy::Resolver(name) => {
                if let Some(reg) = registry {
                    if let Some(resolver) = reg.get(name) {
                        resolver.check(context, resource_id)
                    } else {
                        tracing::warn!(
                            resolver = %name,
                            "Auth resolver not found in registry — denying access (fail-closed)"
                        );
                        false // Fail-closed: unknown resolver denies access
                    }
                } else {
                    tracing::warn!(
                        resolver = %name,
                        "No resolver registry available — denying access (fail-closed)"
                    );
                    false
                }
            }
        }
    }

    /// Parse policy from string (for YAML config)
    ///
    /// ## Supported formats
    ///
    /// **Simple policies:**
    /// - `"public"` → `AuthPolicy::Public`
    /// - `"authenticated"` → `AuthPolicy::Authenticated`
    /// - `"owner"` → `AuthPolicy::Owner`
    /// - `"service_only"` → `AuthPolicy::ServiceOnly`
    /// - `"admin_only"` → `AuthPolicy::AdminOnly`
    ///
    /// **Role-based:**
    /// - `"role:admin"` → `AuthPolicy::HasRole(["admin"])`
    ///
    /// **Custom resolvers:**
    /// - `"resolver:is_delegated"` → `AuthPolicy::Resolver("is_delegated")`
    ///
    /// **Shortcuts (OR with owner):**
    /// - `"owner_or_role:admin"` → `Or(Owner, HasRole(["admin"]))`
    /// - `"owner_or_service"` → `Or(Owner, ServiceOnly)`
    /// - `"owner_or_resolver:is_delegated"` → `Or(Owner, Resolver("is_delegated"))`
    ///
    /// **Statements (composable):**
    /// - `"any:owner,resolver:is_delegated,role:admin"` → `Or([Owner, Resolver, HasRole])`
    /// - `"all:authenticated,resolver:is_group_member"` → `And([Authenticated, Resolver])`
    /// - `"not:resolver:is_blacklisted"` → inverted resolver (deny list)
    ///
    /// ## Examples
    ///
    /// ```yaml
    /// auth:
    ///   list: "public"
    ///   get: "authenticated"
    ///   create: "all:authenticated,resolver:is_group_member"
    ///   update: "any:owner,resolver:is_delegated,role:admin"
    ///   delete: "all:admin_only,not:resolver:is_readonly_mode"
    /// ```
    pub fn parse_policy(s: &str) -> Self {
        match s {
            "public" => AuthPolicy::Public,
            "authenticated" => AuthPolicy::Authenticated,
            "owner" => AuthPolicy::Owner,
            "service_only" => AuthPolicy::ServiceOnly,
            "admin_only" => AuthPolicy::AdminOnly,
            "owner_or_service" => AuthPolicy::Or(vec![AuthPolicy::Owner, AuthPolicy::ServiceOnly]),
            s if s.starts_with("all:") => {
                let parts = s.strip_prefix("all:").unwrap();
                let policies: Vec<AuthPolicy> = parts
                    .split(',')
                    .map(|p| AuthPolicy::parse_policy(p.trim()))
                    .collect();
                if policies.len() == 1 {
                    policies.into_iter().next().unwrap()
                } else {
                    AuthPolicy::And(policies)
                }
            }
            s if s.starts_with("any:") => {
                let parts = s.strip_prefix("any:").unwrap();
                let policies: Vec<AuthPolicy> = parts
                    .split(',')
                    .map(|p| AuthPolicy::parse_policy(p.trim()))
                    .collect();
                if policies.len() == 1 {
                    policies.into_iter().next().unwrap()
                } else {
                    AuthPolicy::Or(policies)
                }
            }
            s if s.starts_with("not:") => {
                let inner = s.strip_prefix("not:").unwrap();
                let inner_policy = AuthPolicy::parse_policy(inner);
                // not:X is equivalent to a custom function that inverts the inner policy
                // We implement it as And with a negation wrapper
                AuthPolicy::Not(Box::new(inner_policy))
            }
            s if s.starts_with("role:") => {
                let role = s.strip_prefix("role:").unwrap().to_string();
                AuthPolicy::HasRole(vec![role])
            }
            s if s.starts_with("owner_or_role:") => {
                let role = s.strip_prefix("owner_or_role:").unwrap().to_string();
                AuthPolicy::Or(vec![AuthPolicy::Owner, AuthPolicy::HasRole(vec![role])])
            }
            s if s.starts_with("resolver:") => {
                let name = s.strip_prefix("resolver:").unwrap().to_string();
                AuthPolicy::Resolver(name)
            }
            s if s.starts_with("owner_or_resolver:") => {
                let name = s.strip_prefix("owner_or_resolver:").unwrap().to_string();
                AuthPolicy::Or(vec![AuthPolicy::Owner, AuthPolicy::Resolver(name)])
            }
            _ => AuthPolicy::Authenticated, // Default
        }
    }
}

/// Trait for auth providers
#[async_trait]
pub trait AuthProvider: Send + Sync {
    /// Extract auth context from HTTP request headers
    ///
    /// Takes a `&HeaderMap` — only headers are needed for auth (Bearer token).
    /// This keeps the trait dyn-compatible and Send-safe.
    async fn extract_context(&self, headers: &HeaderMap) -> Result<AuthContext>;

    /// Check if user is owner of a resource
    async fn is_owner(
        &self,
        user_id: &Uuid,
        resource_id: &Uuid,
        resource_type: &str,
    ) -> Result<bool>;

    /// Check if user has a role
    async fn has_role(&self, user_id: &Uuid, role: &str) -> Result<bool>;
}

/// Default no-auth provider (for development)
pub struct NoAuthProvider;

#[async_trait]
impl AuthProvider for NoAuthProvider {
    async fn extract_context(&self, _headers: &HeaderMap) -> Result<AuthContext> {
        Ok(AuthContext::Anonymous)
    }

    async fn is_owner(&self, _: &Uuid, _: &Uuid, _: &str) -> Result<bool> {
        Ok(true)
    }

    async fn has_role(&self, _: &Uuid, _: &str) -> Result<bool> {
        Ok(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_policy_check() {
        let user_context = AuthContext::User {
            user_id: Uuid::new_v4(),
            tenant_id: Uuid::new_v4(),
            roles: vec!["admin".to_string()],
        };

        assert!(AuthPolicy::Authenticated.check(&user_context));
        assert!(AuthPolicy::HasRole(vec!["admin".into()]).check(&user_context));
        assert!(!AuthPolicy::Owner.check(&user_context));

        let anon_context = AuthContext::Anonymous;
        assert!(AuthPolicy::Public.check(&anon_context));
        assert!(!AuthPolicy::Authenticated.check(&anon_context));
    }

    #[test]
    fn test_policy_from_str() {
        match AuthPolicy::parse_policy("public") {
            AuthPolicy::Public => (),
            _ => panic!("Expected Public"),
        }

        match AuthPolicy::parse_policy("role:admin") {
            AuthPolicy::HasRole(roles) => assert_eq!(roles, vec!["admin"]),
            _ => panic!("Expected HasRole"),
        }
    }

    // --- AuthPolicy::check ---

    #[test]
    fn test_policy_check_and_both_pass() {
        let ctx = AuthContext::User {
            user_id: Uuid::new_v4(),
            tenant_id: Uuid::new_v4(),
            roles: vec!["editor".to_string()],
        };
        let policy = AuthPolicy::And(vec![
            AuthPolicy::Authenticated,
            AuthPolicy::HasRole(vec!["editor".into()]),
        ]);
        assert!(policy.check(&ctx));
    }

    #[test]
    fn test_policy_check_and_one_fails() {
        let ctx = AuthContext::User {
            user_id: Uuid::new_v4(),
            tenant_id: Uuid::new_v4(),
            roles: vec!["viewer".to_string()],
        };
        let policy = AuthPolicy::And(vec![
            AuthPolicy::Authenticated,
            AuthPolicy::HasRole(vec!["admin".into()]),
        ]);
        assert!(!policy.check(&ctx));
    }

    #[test]
    fn test_policy_check_or_one_passes() {
        let ctx = AuthContext::Admin {
            admin_id: Uuid::new_v4(),
        };
        let policy = AuthPolicy::Or(vec![AuthPolicy::ServiceOnly, AuthPolicy::AdminOnly]);
        assert!(policy.check(&ctx));
    }

    #[test]
    fn test_policy_check_or_both_fail() {
        let ctx = AuthContext::User {
            user_id: Uuid::new_v4(),
            tenant_id: Uuid::new_v4(),
            roles: vec![],
        };
        let policy = AuthPolicy::Or(vec![AuthPolicy::ServiceOnly, AuthPolicy::AdminOnly]);
        assert!(!policy.check(&ctx));
    }

    #[test]
    fn test_policy_check_custom_true() {
        fn always_true(_ctx: &AuthContext) -> bool {
            true
        }
        let policy = AuthPolicy::Custom(always_true);
        assert!(policy.check(&AuthContext::Anonymous));
    }

    #[test]
    fn test_policy_check_custom_false() {
        fn always_false(_ctx: &AuthContext) -> bool {
            false
        }
        let policy = AuthPolicy::Custom(always_false);
        assert!(!policy.check(&AuthContext::Anonymous));
    }

    #[test]
    fn test_policy_check_service_only() {
        let service_ctx = AuthContext::Service {
            service_name: "billing".to_string(),
            tenant_id: None,
        };
        assert!(AuthPolicy::ServiceOnly.check(&service_ctx));

        let user_ctx = AuthContext::User {
            user_id: Uuid::new_v4(),
            tenant_id: Uuid::new_v4(),
            roles: vec![],
        };
        assert!(!AuthPolicy::ServiceOnly.check(&user_ctx));
    }

    #[test]
    fn test_policy_check_admin_only() {
        let admin_ctx = AuthContext::Admin {
            admin_id: Uuid::new_v4(),
        };
        assert!(AuthPolicy::AdminOnly.check(&admin_ctx));

        let user_ctx = AuthContext::User {
            user_id: Uuid::new_v4(),
            tenant_id: Uuid::new_v4(),
            roles: vec![],
        };
        assert!(!AuthPolicy::AdminOnly.check(&user_ctx));
    }

    #[test]
    fn test_policy_check_owner() {
        let owner_ctx = AuthContext::Owner {
            user_id: Uuid::new_v4(),
            tenant_id: Uuid::new_v4(),
            resource_id: Uuid::new_v4(),
            resource_type: "document".to_string(),
        };
        assert!(AuthPolicy::Owner.check(&owner_ctx));

        let user_ctx = AuthContext::User {
            user_id: Uuid::new_v4(),
            tenant_id: Uuid::new_v4(),
            roles: vec![],
        };
        assert!(!AuthPolicy::Owner.check(&user_ctx));
    }

    // --- parse_policy ---

    #[test]
    fn test_parse_policy_authenticated() {
        assert!(matches!(
            AuthPolicy::parse_policy("authenticated"),
            AuthPolicy::Authenticated
        ));
    }

    #[test]
    fn test_parse_policy_service_only() {
        assert!(matches!(
            AuthPolicy::parse_policy("service_only"),
            AuthPolicy::ServiceOnly
        ));
    }

    #[test]
    fn test_parse_policy_admin_only() {
        assert!(matches!(
            AuthPolicy::parse_policy("admin_only"),
            AuthPolicy::AdminOnly
        ));
    }

    #[test]
    fn test_parse_policy_owner() {
        assert!(matches!(
            AuthPolicy::parse_policy("owner"),
            AuthPolicy::Owner
        ));
    }

    #[test]
    fn test_parse_policy_owner_or_role() {
        match AuthPolicy::parse_policy("owner_or_role:manager") {
            AuthPolicy::Or(policies) => {
                assert_eq!(policies.len(), 2);
                assert!(matches!(policies[0], AuthPolicy::Owner));
                match &policies[1] {
                    AuthPolicy::HasRole(roles) => assert_eq!(roles, &vec!["manager".to_string()]),
                    other => panic!("Expected HasRole, got {:?}", other),
                }
            }
            other => panic!("Expected Or policy, got {:?}", other),
        }
    }

    #[test]
    fn test_parse_policy_unknown_defaults_to_authenticated() {
        assert!(matches!(
            AuthPolicy::parse_policy("something_unknown"),
            AuthPolicy::Authenticated
        ));
    }

    // --- AuthContext accessors ---

    #[test]
    fn test_auth_context_tenant_id_user() {
        let tid = Uuid::new_v4();
        let ctx = AuthContext::User {
            user_id: Uuid::new_v4(),
            tenant_id: tid,
            roles: vec![],
        };
        assert_eq!(ctx.tenant_id(), Some(tid));
    }

    #[test]
    fn test_auth_context_tenant_id_owner() {
        let tid = Uuid::new_v4();
        let ctx = AuthContext::Owner {
            user_id: Uuid::new_v4(),
            tenant_id: tid,
            resource_id: Uuid::new_v4(),
            resource_type: "item".to_string(),
        };
        assert_eq!(ctx.tenant_id(), Some(tid));
    }

    #[test]
    fn test_auth_context_tenant_id_service_with_tenant() {
        let tid = Uuid::new_v4();
        let ctx = AuthContext::Service {
            service_name: "svc".to_string(),
            tenant_id: Some(tid),
        };
        assert_eq!(ctx.tenant_id(), Some(tid));
    }

    #[test]
    fn test_auth_context_tenant_id_service_without_tenant() {
        let ctx = AuthContext::Service {
            service_name: "svc".to_string(),
            tenant_id: None,
        };
        assert_eq!(ctx.tenant_id(), None);
    }

    #[test]
    fn test_auth_context_tenant_id_admin() {
        let ctx = AuthContext::Admin {
            admin_id: Uuid::new_v4(),
        };
        assert_eq!(ctx.tenant_id(), None);
    }

    #[test]
    fn test_auth_context_tenant_id_anonymous() {
        assert_eq!(AuthContext::Anonymous.tenant_id(), None);
    }

    #[test]
    fn test_auth_context_user_id() {
        let uid = Uuid::new_v4();
        let user_ctx = AuthContext::User {
            user_id: uid,
            tenant_id: Uuid::new_v4(),
            roles: vec![],
        };
        assert_eq!(user_ctx.user_id(), Some(uid));

        let owner_uid = Uuid::new_v4();
        let owner_ctx = AuthContext::Owner {
            user_id: owner_uid,
            tenant_id: Uuid::new_v4(),
            resource_id: Uuid::new_v4(),
            resource_type: "doc".to_string(),
        };
        assert_eq!(owner_ctx.user_id(), Some(owner_uid));

        assert_eq!(AuthContext::Anonymous.user_id(), None);
        assert_eq!(
            AuthContext::Admin {
                admin_id: Uuid::new_v4()
            }
            .user_id(),
            None
        );
        assert_eq!(
            AuthContext::Service {
                service_name: "x".to_string(),
                tenant_id: None
            }
            .user_id(),
            None
        );
    }

    #[test]
    fn test_auth_context_is_admin() {
        assert!(
            AuthContext::Admin {
                admin_id: Uuid::new_v4()
            }
            .is_admin()
        );
        assert!(!AuthContext::Anonymous.is_admin());
    }

    #[test]
    fn test_auth_context_is_service() {
        assert!(
            AuthContext::Service {
                service_name: "svc".to_string(),
                tenant_id: None
            }
            .is_service()
        );
        assert!(!AuthContext::Anonymous.is_service());
    }

    // --- NoAuthProvider ---

    #[tokio::test]
    async fn test_no_auth_provider_extract_context() {
        let provider = NoAuthProvider;
        let headers = HeaderMap::new();
        let ctx = provider
            .extract_context(&headers)
            .await
            .expect("extract_context should succeed");
        assert!(matches!(ctx, AuthContext::Anonymous));
    }

    #[tokio::test]
    async fn test_no_auth_provider_is_owner() {
        let provider = NoAuthProvider;
        let result = provider
            .is_owner(&Uuid::new_v4(), &Uuid::new_v4(), "resource")
            .await
            .expect("is_owner should succeed");
        assert!(result);
    }

    #[tokio::test]
    async fn test_no_auth_provider_has_role() {
        let provider = NoAuthProvider;
        let result = provider
            .has_role(&Uuid::new_v4(), "admin")
            .await
            .expect("has_role should succeed");
        assert!(!result);
    }

    // --- AuthContext helpers ---

    #[test]
    fn test_auth_context_roles() {
        let ctx = AuthContext::User {
            user_id: Uuid::new_v4(),
            tenant_id: Uuid::new_v4(),
            roles: vec!["admin".into(), "editor".into()],
        };
        assert_eq!(ctx.roles(), &["admin", "editor"]);
        assert!(ctx.has_role("admin"));
        assert!(ctx.has_role("editor"));
        assert!(!ctx.has_role("viewer"));
    }

    #[test]
    fn test_auth_context_roles_anonymous() {
        assert!(AuthContext::Anonymous.roles().is_empty());
        assert!(!AuthContext::Anonymous.has_role("admin"));
    }

    // --- parse_policy: resolver ---

    #[test]
    fn test_parse_policy_resolver() {
        match AuthPolicy::parse_policy("resolver:is_delegated") {
            AuthPolicy::Resolver(name) => assert_eq!(name, "is_delegated"),
            other => panic!("Expected Resolver, got {:?}", other),
        }
    }

    #[test]
    fn test_parse_policy_owner_or_resolver() {
        match AuthPolicy::parse_policy("owner_or_resolver:is_parent") {
            AuthPolicy::Or(policies) => {
                assert_eq!(policies.len(), 2);
                assert!(matches!(policies[0], AuthPolicy::Owner));
                match &policies[1] {
                    AuthPolicy::Resolver(name) => assert_eq!(name, "is_parent"),
                    other => panic!("Expected Resolver, got {:?}", other),
                }
            }
            other => panic!("Expected Or policy, got {:?}", other),
        }
    }

    #[test]
    fn test_parse_policy_owner_or_service() {
        match AuthPolicy::parse_policy("owner_or_service") {
            AuthPolicy::Or(policies) => {
                assert_eq!(policies.len(), 2);
                assert!(matches!(policies[0], AuthPolicy::Owner));
                assert!(matches!(policies[1], AuthPolicy::ServiceOnly));
            }
            other => panic!("Expected Or policy, got {:?}", other),
        }
    }

    // --- AuthResolver + Registry ---

    #[test]
    fn test_fn_resolver_check() {
        let resolver = FnResolver::new(
            |ctx: &AuthContext, _res: Option<&Uuid>| ctx.has_role("manager"),
            "Check manager role",
        );

        let manager_ctx = AuthContext::User {
            user_id: Uuid::new_v4(),
            tenant_id: Uuid::new_v4(),
            roles: vec!["manager".into()],
        };
        assert!(resolver.check(&manager_ctx, None));

        let viewer_ctx = AuthContext::User {
            user_id: Uuid::new_v4(),
            tenant_id: Uuid::new_v4(),
            roles: vec!["viewer".into()],
        };
        assert!(!resolver.check(&viewer_ctx, None));
    }

    #[test]
    fn test_fn_resolver_with_resource_id() {
        let target_id = Uuid::new_v4();
        let resolver = FnResolver::new(
            move |ctx: &AuthContext, res: Option<&Uuid>| {
                ctx.user_id().is_some() && res == Some(&target_id)
            },
            "Check resource match",
        );

        let ctx = AuthContext::User {
            user_id: Uuid::new_v4(),
            tenant_id: Uuid::new_v4(),
            roles: vec![],
        };
        assert!(resolver.check(&ctx, Some(&target_id)));
        assert!(!resolver.check(&ctx, Some(&Uuid::new_v4())));
        assert!(!resolver.check(&ctx, None));
    }

    #[test]
    fn test_resolver_registry() {
        let mut registry = AuthResolverRegistry::new();
        assert!(registry.is_empty());

        registry.register(
            "is_manager",
            Arc::new(FnResolver::new(
                |ctx: &AuthContext, _: Option<&Uuid>| ctx.has_role("manager"),
                "Manager check",
            )),
        );
        assert_eq!(registry.len(), 1);
        assert!(registry.contains("is_manager"));
        assert!(!registry.contains("unknown"));

        let names = registry.names();
        assert!(names.contains(&"is_manager"));
    }

    #[test]
    fn test_resolver_policy_check_with_registry() {
        let mut registry = AuthResolverRegistry::new();
        registry.register(
            "is_manager",
            Arc::new(FnResolver::new(
                |ctx: &AuthContext, _: Option<&Uuid>| ctx.has_role("manager"),
                "Manager check",
            )),
        );

        let policy = AuthPolicy::Resolver("is_manager".into());

        let manager = AuthContext::User {
            user_id: Uuid::new_v4(),
            tenant_id: Uuid::new_v4(),
            roles: vec!["manager".into()],
        };
        assert!(policy.check_with_resolver(&manager, Some(&registry), None));

        let viewer = AuthContext::User {
            user_id: Uuid::new_v4(),
            tenant_id: Uuid::new_v4(),
            roles: vec!["viewer".into()],
        };
        assert!(!policy.check_with_resolver(&viewer, Some(&registry), None));
    }

    #[test]
    fn test_resolver_policy_fail_closed_missing_resolver() {
        let registry = AuthResolverRegistry::new();
        let policy = AuthPolicy::Resolver("nonexistent".into());

        let ctx = AuthContext::User {
            user_id: Uuid::new_v4(),
            tenant_id: Uuid::new_v4(),
            roles: vec!["admin".into()],
        };
        // Fail-closed: unknown resolver denies access
        assert!(!policy.check_with_resolver(&ctx, Some(&registry), None));
    }

    #[test]
    fn test_resolver_policy_fail_closed_no_registry() {
        let policy = AuthPolicy::Resolver("any".into());

        let ctx = AuthContext::User {
            user_id: Uuid::new_v4(),
            tenant_id: Uuid::new_v4(),
            roles: vec![],
        };
        // Fail-closed: no registry denies access
        assert!(!policy.check_with_resolver(&ctx, None, None));
    }

    #[test]
    fn test_owner_or_resolver_policy() {
        let mut registry = AuthResolverRegistry::new();
        registry.register(
            "is_delegated",
            Arc::new(FnResolver::new(
                |ctx: &AuthContext, _: Option<&Uuid>| ctx.has_role("delegate"),
                "Delegation check",
            )),
        );

        let policy = AuthPolicy::parse_policy("owner_or_resolver:is_delegated");

        // Owner passes
        let owner = AuthContext::Owner {
            user_id: Uuid::new_v4(),
            tenant_id: Uuid::new_v4(),
            resource_id: Uuid::new_v4(),
            resource_type: "order".into(),
        };
        assert!(policy.check_with_resolver(&owner, Some(&registry), None));

        // Delegate passes
        let delegate = AuthContext::User {
            user_id: Uuid::new_v4(),
            tenant_id: Uuid::new_v4(),
            roles: vec!["delegate".into()],
        };
        assert!(policy.check_with_resolver(&delegate, Some(&registry), None));

        // Neither owner nor delegate fails
        let random = AuthContext::User {
            user_id: Uuid::new_v4(),
            tenant_id: Uuid::new_v4(),
            roles: vec!["viewer".into()],
        };
        assert!(!policy.check_with_resolver(&random, Some(&registry), None));
    }

    #[test]
    fn test_resolver_description() {
        let resolver = FnResolver::new(|_: &AuthContext, _: Option<&Uuid>| true, "Always allow");
        assert_eq!(resolver.description(), "Always allow");
    }

    #[test]
    fn test_resolver_registry_debug() {
        let mut registry = AuthResolverRegistry::new();
        registry.register(
            "test",
            Arc::new(FnResolver::new(
                |_: &AuthContext, _: Option<&Uuid>| true,
                "test",
            )),
        );
        let debug = format!("{:?}", registry);
        assert!(debug.contains("test"));
    }

    // --- Statements: all, any, not ---

    #[test]
    fn test_parse_policy_all_statement() {
        match AuthPolicy::parse_policy("all:authenticated,owner") {
            AuthPolicy::And(policies) => {
                assert_eq!(policies.len(), 2);
                assert!(matches!(policies[0], AuthPolicy::Authenticated));
                assert!(matches!(policies[1], AuthPolicy::Owner));
            }
            other => panic!("Expected And policy, got {:?}", other),
        }
    }

    #[test]
    fn test_parse_policy_any_statement() {
        match AuthPolicy::parse_policy("any:owner,service_only,admin_only") {
            AuthPolicy::Or(policies) => {
                assert_eq!(policies.len(), 3);
                assert!(matches!(policies[0], AuthPolicy::Owner));
                assert!(matches!(policies[1], AuthPolicy::ServiceOnly));
                assert!(matches!(policies[2], AuthPolicy::AdminOnly));
            }
            other => panic!("Expected Or policy, got {:?}", other),
        }
    }

    #[test]
    fn test_parse_policy_any_with_resolvers() {
        match AuthPolicy::parse_policy("any:owner,resolver:is_delegated,role:admin") {
            AuthPolicy::Or(policies) => {
                assert_eq!(policies.len(), 3);
                assert!(matches!(policies[0], AuthPolicy::Owner));
                match &policies[1] {
                    AuthPolicy::Resolver(name) => assert_eq!(name, "is_delegated"),
                    other => panic!("Expected Resolver, got {:?}", other),
                }
                match &policies[2] {
                    AuthPolicy::HasRole(roles) => assert_eq!(roles, &vec!["admin".to_string()]),
                    other => panic!("Expected HasRole, got {:?}", other),
                }
            }
            other => panic!("Expected Or policy, got {:?}", other),
        }
    }

    #[test]
    fn test_parse_policy_all_with_resolver() {
        match AuthPolicy::parse_policy("all:authenticated,resolver:is_group_member") {
            AuthPolicy::And(policies) => {
                assert_eq!(policies.len(), 2);
                assert!(matches!(policies[0], AuthPolicy::Authenticated));
                match &policies[1] {
                    AuthPolicy::Resolver(name) => assert_eq!(name, "is_group_member"),
                    other => panic!("Expected Resolver, got {:?}", other),
                }
            }
            other => panic!("Expected And policy, got {:?}", other),
        }
    }

    #[test]
    fn test_parse_policy_not() {
        match AuthPolicy::parse_policy("not:service_only") {
            AuthPolicy::Not(inner) => {
                assert!(matches!(*inner, AuthPolicy::ServiceOnly));
            }
            other => panic!("Expected Not policy, got {:?}", other),
        }
    }

    #[test]
    fn test_parse_policy_not_resolver() {
        match AuthPolicy::parse_policy("not:resolver:is_blacklisted") {
            AuthPolicy::Not(inner) => match *inner {
                AuthPolicy::Resolver(name) => assert_eq!(name, "is_blacklisted"),
                other => panic!("Expected Resolver, got {:?}", other),
            },
            other => panic!("Expected Not policy, got {:?}", other),
        }
    }

    #[test]
    fn test_parse_policy_single_all_unwraps() {
        // all: with a single element should unwrap
        match AuthPolicy::parse_policy("all:authenticated") {
            AuthPolicy::Authenticated => (),
            other => panic!("Expected Authenticated (unwrapped), got {:?}", other),
        }
    }

    #[test]
    fn test_statement_all_check() {
        let policy = AuthPolicy::parse_policy("all:authenticated,role:editor");

        // User with editor role — passes both
        let editor = AuthContext::User {
            user_id: Uuid::new_v4(),
            tenant_id: Uuid::new_v4(),
            roles: vec!["editor".into()],
        };
        assert!(policy.check(&editor));

        // User without editor role — fails role check
        let viewer = AuthContext::User {
            user_id: Uuid::new_v4(),
            tenant_id: Uuid::new_v4(),
            roles: vec!["viewer".into()],
        };
        assert!(!policy.check(&viewer));

        // Anonymous — fails authenticated check
        assert!(!policy.check(&AuthContext::Anonymous));
    }

    #[test]
    fn test_statement_any_check() {
        let policy = AuthPolicy::parse_policy("any:owner,admin_only,service_only");

        // Admin passes
        let admin = AuthContext::Admin {
            admin_id: Uuid::new_v4(),
        };
        assert!(policy.check(&admin));

        // Service passes
        let service = AuthContext::Service {
            service_name: "billing".into(),
            tenant_id: None,
        };
        assert!(policy.check(&service));

        // Regular user fails all three
        let user = AuthContext::User {
            user_id: Uuid::new_v4(),
            tenant_id: Uuid::new_v4(),
            roles: vec![],
        };
        assert!(!policy.check(&user));
    }

    #[test]
    fn test_statement_not_check() {
        let policy = AuthPolicy::parse_policy("not:service_only");

        // Regular user — not service → true
        let user = AuthContext::User {
            user_id: Uuid::new_v4(),
            tenant_id: Uuid::new_v4(),
            roles: vec![],
        };
        assert!(policy.check(&user));

        // Service — is service → false (negated)
        let service = AuthContext::Service {
            service_name: "svc".into(),
            tenant_id: None,
        };
        assert!(!policy.check(&service));
    }

    #[test]
    fn test_statement_all_with_not_resolver() {
        let mut registry = AuthResolverRegistry::new();
        registry.register(
            "is_readonly",
            Arc::new(FnResolver::new(
                |ctx: &AuthContext, _: Option<&Uuid>| ctx.has_role("readonly"),
                "Readonly check",
            )),
        );

        // "must be authenticated AND not in readonly mode"
        let policy = AuthPolicy::parse_policy("all:authenticated,not:resolver:is_readonly");

        // Normal user → authenticated=true, not readonly=true → pass
        let normal = AuthContext::User {
            user_id: Uuid::new_v4(),
            tenant_id: Uuid::new_v4(),
            roles: vec!["editor".into()],
        };
        assert!(policy.check_with_resolver(&normal, Some(&registry), None));

        // Readonly user → authenticated=true, not readonly=false → fail
        let readonly = AuthContext::User {
            user_id: Uuid::new_v4(),
            tenant_id: Uuid::new_v4(),
            roles: vec!["readonly".into()],
        };
        assert!(!policy.check_with_resolver(&readonly, Some(&registry), None));

        // Anonymous → authenticated=false → fail
        assert!(!policy.check_with_resolver(&AuthContext::Anonymous, Some(&registry), None));
    }

    #[test]
    fn test_statement_any_with_resolver() {
        let mut registry = AuthResolverRegistry::new();
        registry.register(
            "is_delegated",
            Arc::new(FnResolver::new(
                |ctx: &AuthContext, _: Option<&Uuid>| ctx.has_role("delegate"),
                "Delegation check",
            )),
        );

        let policy = AuthPolicy::parse_policy("any:owner,resolver:is_delegated,role:admin");

        // Delegate passes via resolver
        let delegate = AuthContext::User {
            user_id: Uuid::new_v4(),
            tenant_id: Uuid::new_v4(),
            roles: vec!["delegate".into()],
        };
        assert!(policy.check_with_resolver(&delegate, Some(&registry), None));

        // Admin passes via role
        let admin = AuthContext::User {
            user_id: Uuid::new_v4(),
            tenant_id: Uuid::new_v4(),
            roles: vec!["admin".into()],
        };
        assert!(policy.check_with_resolver(&admin, Some(&registry), None));

        // Nobody passes
        let nobody = AuthContext::User {
            user_id: Uuid::new_v4(),
            tenant_id: Uuid::new_v4(),
            roles: vec!["viewer".into()],
        };
        assert!(!policy.check_with_resolver(&nobody, Some(&registry), None));
    }
}
