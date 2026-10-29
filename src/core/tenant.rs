//! Multi-tenant context for request-scoped tenant isolation
//!
//! The `TenantContext` is extracted from the `AuthContext` by the tenant
//! resolver middleware and inserted into Axum request extensions. Downstream
//! handlers and services can use it for data scoping and event filtering.
//!
//! ## Usage
//!
//! ```ignore
//! use axum::Extension;
//! use this::core::tenant::TenantContext;
//!
//! async fn my_handler(Extension(tenant): Extension<TenantContext>) -> impl IntoResponse {
//!     let tenant_id = tenant.tenant_id();
//!     // ... filter data by tenant_id
//! }
//! ```

use uuid::Uuid;

/// Request-scoped tenant context
///
/// Extracted from `AuthContext` by the tenant resolver middleware.
/// Contains the tenant ID and optional metadata for the current request.
#[derive(Debug, Clone)]
pub struct TenantContext {
    /// The tenant ID for this request
    tenant_id: Uuid,
}

impl TenantContext {
    /// Create a new TenantContext
    pub fn new(tenant_id: Uuid) -> Self {
        Self { tenant_id }
    }

    /// Get the tenant ID
    pub fn tenant_id(&self) -> Uuid {
        self.tenant_id
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tenant_context_creation() {
        let tid = Uuid::new_v4();
        let ctx = TenantContext::new(tid);
        assert_eq!(ctx.tenant_id(), tid);
    }

    #[test]
    fn test_tenant_context_clone() {
        let tid = Uuid::new_v4();
        let ctx = TenantContext::new(tid);
        let cloned = ctx.clone();
        assert_eq!(ctx.tenant_id(), cloned.tenant_id());
    }
}
