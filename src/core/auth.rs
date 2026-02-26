//! Authorization system for this-rs
//!
//! Provides context-based authorization with multiple auth types:
//! - User authentication
//! - Owner-based access
//! - Service-to-service
//! - Admin access

use anyhow::Result;
use async_trait::async_trait;
use axum::http::Request;
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
}

impl AuthPolicy {
    /// Check if auth context satisfies this policy
    pub fn check(&self, context: &AuthContext) -> bool {
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

            AuthPolicy::And(policies) => policies.iter().all(|p| p.check(context)),

            AuthPolicy::Or(policies) => policies.iter().any(|p| p.check(context)),

            AuthPolicy::Custom(f) => f(context),
        }
    }

    /// Parse policy from string (for YAML config)
    pub fn parse_policy(s: &str) -> Self {
        match s {
            "public" => AuthPolicy::Public,
            "authenticated" => AuthPolicy::Authenticated,
            "owner" => AuthPolicy::Owner,
            "service_only" => AuthPolicy::ServiceOnly,
            "admin_only" => AuthPolicy::AdminOnly,
            s if s.starts_with("role:") => {
                let role = s.strip_prefix("role:").unwrap().to_string();
                AuthPolicy::HasRole(vec![role])
            }
            s if s.starts_with("owner_or_role:") => {
                let role = s.strip_prefix("owner_or_role:").unwrap().to_string();
                AuthPolicy::Or(vec![AuthPolicy::Owner, AuthPolicy::HasRole(vec![role])])
            }
            _ => AuthPolicy::Authenticated, // Default
        }
    }
}

/// Trait for auth providers
#[async_trait]
pub trait AuthProvider: Send + Sync {
    /// Extract auth context from HTTP request
    async fn extract_context<B>(&self, req: &Request<B>) -> Result<AuthContext>;

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
    async fn extract_context<B>(&self, _req: &Request<B>) -> Result<AuthContext> {
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
        let req = Request::builder()
            .body(())
            .expect("failed to build request");
        let ctx = provider
            .extract_context(&req)
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
}
