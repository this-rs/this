//! Authentication and authorization configuration
//!
//! Parsed from the `auth:` section of the YAML configuration.
//! This module defines the contract between the YAML config and all auth-related
//! modules (middleware, event scoping, tenant resolver, GDPR).
//!
//! ## Feature gates
//! - Base structs (provider, default_policy, per-entity policies) are always available
//! - WAMI-specific fields (public_key, issuer, audience, mode) require `feature = "wami"`
//! - obrain event scoping fields require `feature = "obrain"`

use serde::{Deserialize, Serialize};

// ─── Auth Provider ───────────────────────────────────────────────────────────

/// Which authentication provider to use
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthProviderType {
    /// No authentication (development mode)
    #[default]
    None,

    /// WAMI STS provider (Ed25519 JWT via wami-token)
    ///
    /// The variant is always available for YAML deserialization,
    /// but the actual provider is only instantiated when `feature = "wami"`.
    Wami,

    /// Standard OIDC provider (external IdP)
    Oidc,
}

// ─── Auth Mode (WAMI-specific) ──────────────────────────────────────────────

/// How WAMI authentication is bootstrapped and operated
#[cfg(feature = "wami")]
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthMode {
    /// Full STS: WAMI runs as a separate service, this.rs only verifies tokens
    #[default]
    Sts,

    /// Embedded: WAMI STS is embedded in this.rs (login + verify endpoints)
    Embedded,

    /// Bootstrap: minimal in-memory WAMI for development/testing
    Bootstrap,
}

// ─── Tenant Configuration ───────────────────────────────────────────────────

/// How tenant_id is resolved from requests
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TenantResolver {
    /// Extract from JWT claim (default)
    #[default]
    JwtClaim,

    /// Extract from X-Tenant-Id header
    Header,

    /// Extract from subdomain (tenant.api.example.com)
    Subdomain,
}

/// Multi-tenant configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TenantConfig {
    /// Enable multi-tenant isolation
    #[serde(default)]
    pub enabled: bool,

    /// How to resolve tenant_id from requests
    #[serde(default)]
    pub resolver: TenantResolver,

    /// JWT claim field containing tenant_id (default: "tenant_id")
    #[serde(default = "default_tenant_claim_field")]
    pub claim_field: String,

    /// Auto-provision unknown tenants on first request
    #[serde(default)]
    pub auto_provision: bool,
}

fn default_tenant_claim_field() -> String {
    "tenant_id".to_string()
}

impl Default for TenantConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            resolver: TenantResolver::default(),
            claim_field: default_tenant_claim_field(),
            auto_provision: false,
        }
    }
}

// ─── Event Scoping ──────────────────────────────────────────────────────────

/// Event system scoping configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventScopingConfig {
    /// Filter events by tenant_id in dispatch (WS, SSE, GraphQL, gRPC)
    #[serde(default = "default_true")]
    pub tenant_isolation: bool,

    /// Enable bidirectional FrameworkEvent <-> MutationBus bridge
    #[cfg(feature = "obrain")]
    #[serde(default)]
    pub cognitive_bridge: bool,

    /// Enable cognitive signal notifications (stigmergy, co-change, episodes)
    #[cfg(feature = "obrain")]
    #[serde(default)]
    pub cognitive_notifications: bool,
}

fn default_true() -> bool {
    true
}

impl Default for EventScopingConfig {
    fn default() -> Self {
        Self {
            tenant_isolation: true,
            #[cfg(feature = "obrain")]
            cognitive_bridge: false,
            #[cfg(feature = "obrain")]
            cognitive_notifications: false,
        }
    }
}

// ─── GDPR Configuration ────────────────────────────────────────────────────

/// GDPR compliance configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GdprConfig {
    /// Enable DELETE /tenants/:id/data cascade endpoint
    #[serde(default)]
    pub erasure_cascade: bool,

    /// Sink name for immutable audit logging (must exist in sinks config)
    #[serde(default)]
    pub audit_sink: Option<String>,
}

impl Default for GdprConfig {
    fn default() -> Self {
        Self {
            erasure_cascade: false,
            audit_sink: None,
        }
    }
}

// ─── WAMI-specific Configuration ────────────────────────────────────────────

/// WAMI STS-specific configuration (only available with feature "wami")
#[cfg(feature = "wami")]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WamiConfig {
    /// Ed25519 public key (PEM or base64) for JWT verification
    ///
    /// Required in `sts` and `embedded` modes.
    /// In `bootstrap` mode, if absent, a key pair is auto-generated at startup.
    #[serde(default)]
    pub public_key: Option<String>,

    /// Ed25519 private key (PEM or base64) for JWT signing
    ///
    /// Required for `embedded` and `bootstrap` modes (STS token issuance).
    /// Not needed in `sts` mode (verification only).
    /// In `bootstrap` mode, if absent, a key pair is auto-generated at startup.
    #[serde(default)]
    pub private_key: Option<String>,

    /// Expected JWT issuer (optional, validates `iss` claim)
    #[serde(default)]
    pub issuer: Option<String>,

    /// Expected JWT audience (optional, validates `aud` claim)
    #[serde(default)]
    pub audience: Option<String>,

    /// Authentication mode
    #[serde(default)]
    pub mode: AuthMode,

    /// Token TTL in seconds (default: 3600 = 1 hour)
    #[serde(default = "default_token_ttl")]
    pub token_ttl_secs: u64,

    /// Refresh token TTL in seconds (default: 86400 = 24 hours)
    #[serde(default = "default_refresh_ttl")]
    pub refresh_ttl_secs: u64,
}

#[cfg(feature = "wami")]
fn default_token_ttl() -> u64 {
    3600
}

#[cfg(feature = "wami")]
fn default_refresh_ttl() -> u64 {
    86400
}

// ─── OIDC Configuration ────────────────────────────────────────────────────

/// OIDC provider configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OidcConfig {
    /// OIDC discovery URL (e.g., https://auth.example.com/.well-known/openid-configuration)
    pub discovery_url: String,

    /// Expected audience
    #[serde(default)]
    pub audience: Option<String>,

    /// Claim field for user_id (default: "sub")
    #[serde(default = "default_oidc_user_claim")]
    pub user_claim: String,

    /// Claim field for roles (default: "roles")
    #[serde(default = "default_oidc_roles_claim")]
    pub roles_claim: String,
}

fn default_oidc_user_claim() -> String {
    "sub".to_string()
}

fn default_oidc_roles_claim() -> String {
    "roles".to_string()
}

// ─── Main AuthConfig ────────────────────────────────────────────────────────

/// Top-level authentication configuration
///
/// Parsed from the `auth:` section of the YAML config file.
///
/// # Examples
///
/// ## Minimal (no auth)
/// ```yaml
/// # No auth: section → AuthConfig::default() → provider: none
/// ```
///
/// ## WAMI STS
/// ```yaml
/// auth:
///   provider: wami
///   default_policy: authenticated
///   wami:
///     public_key: "base64-encoded-ed25519-public-key"
///     issuer: "https://sts.example.com"
///     mode: sts
///   tenant:
///     enabled: true
///     resolver: jwt_claim
///     claim_field: tenant_id
/// ```
///
/// ## OIDC
/// ```yaml
/// auth:
///   provider: oidc
///   default_policy: authenticated
///   oidc:
///     discovery_url: "https://auth.example.com/.well-known/openid-configuration"
///     audience: "my-api"
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthConfig {
    /// Authentication provider type
    #[serde(default)]
    pub provider: AuthProviderType,

    /// Default authorization policy for all routes (overridable per-entity)
    #[serde(default = "default_policy")]
    pub default_policy: String,

    /// WAMI-specific configuration (requires feature "wami")
    #[cfg(feature = "wami")]
    #[serde(default)]
    pub wami: Option<WamiConfig>,

    /// OIDC-specific configuration
    #[serde(default)]
    pub oidc: Option<OidcConfig>,

    /// Multi-tenant configuration
    #[serde(default)]
    pub tenant: TenantConfig,

    /// Event scoping configuration
    #[serde(default)]
    pub events: EventScopingConfig,

    /// GDPR compliance configuration
    #[serde(default)]
    pub gdpr: GdprConfig,
}

fn default_policy() -> String {
    "public".to_string()
}

impl Default for AuthConfig {
    fn default() -> Self {
        Self {
            provider: AuthProviderType::default(),
            default_policy: default_policy(),
            #[cfg(feature = "wami")]
            wami: None,
            oidc: None,
            tenant: TenantConfig::default(),
            events: EventScopingConfig::default(),
            gdpr: GdprConfig::default(),
        }
    }
}

impl AuthConfig {
    /// Check if authentication is enabled (provider != none)
    pub fn is_enabled(&self) -> bool {
        self.provider != AuthProviderType::None
    }

    /// Check if multi-tenant mode is active
    pub fn is_multi_tenant(&self) -> bool {
        self.tenant.enabled
    }

    /// Check if GDPR erasure cascade is enabled
    pub fn has_erasure_cascade(&self) -> bool {
        self.gdpr.erasure_cascade
    }
}

// ─── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_auth_config_default() {
        let config = AuthConfig::default();
        assert_eq!(config.provider, AuthProviderType::None);
        assert_eq!(config.default_policy, "public");
        assert!(!config.is_enabled());
        assert!(!config.is_multi_tenant());
        assert!(!config.has_erasure_cascade());
    }

    #[test]
    fn test_auth_config_default_roundtrip() {
        let config = AuthConfig::default();
        let yaml = serde_yaml::to_string(&config).unwrap();
        let parsed: AuthConfig = serde_yaml::from_str(&yaml).unwrap();
        assert_eq!(parsed.provider, AuthProviderType::None);
        assert_eq!(parsed.default_policy, "public");
    }

    #[test]
    fn test_auth_config_oidc_yaml() {
        let yaml = r#"
provider: oidc
default_policy: authenticated
oidc:
  discovery_url: "https://auth.example.com/.well-known/openid-configuration"
  audience: "my-api"
tenant:
  enabled: true
  resolver: jwt_claim
  claim_field: org_id
  auto_provision: true
"#;
        let config: AuthConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(config.provider, AuthProviderType::Oidc);
        assert_eq!(config.default_policy, "authenticated");
        assert!(config.is_enabled());
        assert!(config.is_multi_tenant());

        let oidc = config.oidc.unwrap();
        assert_eq!(
            oidc.discovery_url,
            "https://auth.example.com/.well-known/openid-configuration"
        );
        assert_eq!(oidc.audience.unwrap(), "my-api");

        assert!(config.tenant.enabled);
        assert_eq!(config.tenant.resolver, TenantResolver::JwtClaim);
        assert_eq!(config.tenant.claim_field, "org_id");
        assert!(config.tenant.auto_provision);
    }

    #[test]
    fn test_tenant_config_default() {
        let config = TenantConfig::default();
        assert!(!config.enabled);
        assert_eq!(config.resolver, TenantResolver::default());
        assert_eq!(config.claim_field, "tenant_id");
        assert!(!config.auto_provision);
    }

    #[test]
    fn test_tenant_resolver_variants() {
        let yaml_jwt = "\"jwt_claim\"";
        let parsed: TenantResolver = serde_yaml::from_str(yaml_jwt).unwrap();
        assert_eq!(parsed, TenantResolver::JwtClaim);

        let yaml_header = "\"header\"";
        let parsed: TenantResolver = serde_yaml::from_str(yaml_header).unwrap();
        assert_eq!(parsed, TenantResolver::Header);

        let yaml_subdomain = "\"subdomain\"";
        let parsed: TenantResolver = serde_yaml::from_str(yaml_subdomain).unwrap();
        assert_eq!(parsed, TenantResolver::Subdomain);
    }

    #[test]
    fn test_event_scoping_default() {
        let config = EventScopingConfig::default();
        assert!(config.tenant_isolation);
    }

    #[test]
    fn test_gdpr_config_default() {
        let config = GdprConfig::default();
        assert!(!config.erasure_cascade);
        assert!(config.audit_sink.is_none());
    }

    #[test]
    fn test_gdpr_config_yaml() {
        let yaml = r#"
erasure_cascade: true
audit_sink: "compliance-webhook"
"#;
        let config: GdprConfig = serde_yaml::from_str(yaml).unwrap();
        assert!(config.erasure_cascade);
        assert_eq!(config.audit_sink.unwrap(), "compliance-webhook");
    }

    #[test]
    fn test_auth_config_partial_yaml_defaults() {
        // Only provider specified — everything else should default
        let yaml = r#"
provider: oidc
"#;
        let config: AuthConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(config.provider, AuthProviderType::Oidc);
        assert_eq!(config.default_policy, "public"); // default
        assert!(!config.tenant.enabled); // default
        assert!(config.events.tenant_isolation); // default true
        assert!(!config.gdpr.erasure_cascade); // default
    }

    #[test]
    fn test_auth_config_empty_yaml() {
        // Empty YAML object → all defaults
        let yaml = "{}";
        let config: AuthConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(config.provider, AuthProviderType::None);
        assert!(!config.is_enabled());
    }

    #[test]
    fn test_auth_config_full_roundtrip() {
        let config = AuthConfig {
            provider: AuthProviderType::Oidc,
            default_policy: "authenticated".to_string(),
            #[cfg(feature = "wami")]
            wami: None,
            oidc: Some(OidcConfig {
                discovery_url: "https://auth.example.com/.well-known/openid-configuration"
                    .to_string(),
                audience: Some("my-api".to_string()),
                user_claim: "sub".to_string(),
                roles_claim: "roles".to_string(),
            }),
            tenant: TenantConfig {
                enabled: true,
                resolver: TenantResolver::Header,
                claim_field: "org_id".to_string(),
                auto_provision: true,
            },
            events: EventScopingConfig {
                tenant_isolation: true,
                #[cfg(feature = "obrain")]
                cognitive_bridge: true,
                #[cfg(feature = "obrain")]
                cognitive_notifications: false,
            },
            gdpr: GdprConfig {
                erasure_cascade: true,
                audit_sink: Some("audit-webhook".to_string()),
            },
        };

        let yaml = serde_yaml::to_string(&config).unwrap();
        let parsed: AuthConfig = serde_yaml::from_str(&yaml).unwrap();

        assert_eq!(parsed.provider, AuthProviderType::Oidc);
        assert_eq!(parsed.default_policy, "authenticated");
        assert!(parsed.is_enabled());
        assert!(parsed.is_multi_tenant());
        assert!(parsed.has_erasure_cascade());
        assert!(parsed.oidc.is_some());
        assert_eq!(parsed.tenant.resolver, TenantResolver::Header);
        assert_eq!(parsed.tenant.claim_field, "org_id");
    }

    #[cfg(feature = "wami")]
    #[test]
    fn test_auth_config_wami_yaml() {
        let yaml = r#"
provider: wami
default_policy: authenticated
wami:
  public_key: "MCowBQYDK2VwAyEA..."
  issuer: "https://sts.example.com"
  audience: "my-api"
  mode: sts
tenant:
  enabled: true
"#;
        let config: AuthConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(config.provider, AuthProviderType::Wami);
        assert!(config.is_enabled());

        let wami = config.wami.unwrap();
        assert_eq!(wami.public_key.as_deref(), Some("MCowBQYDK2VwAyEA..."));
        assert_eq!(wami.issuer.unwrap(), "https://sts.example.com");
        assert_eq!(wami.audience.unwrap(), "my-api");
        assert_eq!(wami.mode, AuthMode::Sts);
    }

    #[cfg(feature = "wami")]
    #[test]
    fn test_auth_mode_variants() {
        let sts: AuthMode = serde_yaml::from_str("\"sts\"").unwrap();
        assert_eq!(sts, AuthMode::Sts);

        let embedded: AuthMode = serde_yaml::from_str("\"embedded\"").unwrap();
        assert_eq!(embedded, AuthMode::Embedded);

        let bootstrap: AuthMode = serde_yaml::from_str("\"bootstrap\"").unwrap();
        assert_eq!(bootstrap, AuthMode::Bootstrap);
    }

    #[cfg(feature = "wami")]
    #[test]
    fn test_wami_config_without_optional_fields() {
        let yaml = r#"
public_key: "MCowBQYDK2VwAyEA..."
"#;
        let config: WamiConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(config.public_key.as_deref(), Some("MCowBQYDK2VwAyEA..."));
        assert!(config.issuer.is_none());
        assert!(config.audience.is_none());
        assert_eq!(config.mode, AuthMode::Sts); // default
    }
}
