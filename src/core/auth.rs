//! Authorization system for This-RS
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
    pub fn from_str(s: &str) -> Self {
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
        match AuthPolicy::from_str("public") {
            AuthPolicy::Public => (),
            _ => panic!("Expected Public"),
        }

        match AuthPolicy::from_str("role:admin") {
            AuthPolicy::HasRole(roles) => assert_eq!(roles, vec!["admin"]),
            _ => panic!("Expected HasRole"),
        }
    }
}
