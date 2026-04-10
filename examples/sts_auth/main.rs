//! STS Authentication example
//!
//! Demonstrates the full WAMI STS flow with multi-tenant isolation.
//! This example shows the server configuration for embedded STS mode.
//!
//! # Running
//!
//! ```sh
//! cargo run --example sts_auth --features wami
//! ```

fn main() {
    println!("=== this-rs STS Auth Example ===\n");

    #[cfg(feature = "wami")]
    {
        use this::config::auth::*;

        // Demonstrate config parsing for embedded STS
        let yaml = r#"
provider: wami
default_policy: authenticated
wami:
  public_key: "MCowBQYDK2VwAyEA..."
  mode: bootstrap
  token_ttl_secs: 900
  refresh_ttl_secs: 3600
tenant:
  enabled: true
  resolver: jwt_claim
  claim_field: tenant_id
gdpr:
  erasure_cascade: true
"#;

        let config: AuthConfig = serde_yaml::from_str(yaml).expect("parse auth config");
        println!("[OK] AuthConfig parsed:");
        println!("     Provider:  {:?}", config.provider);
        println!("     Policy:    {}", config.default_policy);
        println!("     Tenant:    enabled={}", config.tenant.enabled);
        println!("     GDPR:      erasure_cascade={}", config.gdpr.erasure_cascade);

        let wami = config.wami.as_ref().expect("wami config");
        println!("     WAMI mode: {:?}", wami.mode);
        println!("     Token TTL: {}s", wami.token_ttl_secs);
        println!("     Refresh:   {}s", wami.refresh_ttl_secs);

        // Demonstrate STS token issuance
        use this::core::auth::sts::*;
        use ed25519_dalek::SigningKey;
        use ed25519_dalek::pkcs8::{EncodePrivateKey, EncodePublicKey};
        use ed25519_dalek::pkcs8::spki::der::pem::LineEnding;

        // Generate bootstrap keys
        let secret = [42u8; 32]; // deterministic for demo
        let signing_key = SigningKey::from_bytes(&secret);
        let verifying_key = signing_key.verifying_key();

        let priv_pem = signing_key.to_pkcs8_pem(LineEnding::LF).expect("priv pem").to_string();
        let pub_pem = verifying_key.to_public_key_pem(LineEnding::LF).expect("pub pem");

        let sts = StsService::new(&priv_pem, "this-rs-example".to_string())
            .expect("create STS");

        // Register demo users
        let tenant_a = uuid::Uuid::new_v4();
        let tenant_b = uuid::Uuid::new_v4();

        sts.register_user("alice".into(), "secret".into(), uuid::Uuid::new_v4(), tenant_a, vec!["admin".into()]);
        sts.register_user("bob".into(), "secret".into(), uuid::Uuid::new_v4(), tenant_b, vec!["viewer".into()]);
        println!("\n[OK] Registered 2 users in 2 tenants");

        // Login Alice
        let pair = sts.login(&LoginCredentials {
            username: "alice".into(),
            password: "secret".into(),
        }).expect("login");
        println!("\n[OK] Alice logged in:");
        println!("     Token type: {}", pair.token_type);
        println!("     Expires in: {}s", pair.expires_in);
        println!("     Access:     {}...", &pair.access_token[..40]);

        // Verify token
        let dk = jsonwebtoken::DecodingKey::from_ed_pem(pub_pem.as_bytes()).expect("dk");
        let mut validation = jsonwebtoken::Validation::new(jsonwebtoken::Algorithm::EdDSA);
        validation.set_issuer(&["this-rs-example"]);
        validation.set_audience::<&str>(&[]);

        let data = jsonwebtoken::decode::<StsClaims>(&pair.access_token, &dk, &validation)
            .expect("verify token");
        println!("\n[OK] Token verified:");
        println!("     Subject:   {}", data.claims.sub);
        println!("     Tenant:    {}", data.claims.tenant_id);
        println!("     Roles:     {:?}", data.claims.roles);
        println!("     Issuer:    {}", data.claims.iss);

        // JWKS
        let jwks = StsService::jwks_response(&pub_pem).expect("jwks");
        println!("\n[OK] JWKS response:");
        println!("     {}", serde_json::to_string_pretty(&jwks).unwrap());

        // Refresh
        let new_pair = sts.refresh(&pair.refresh_token, &dk).expect("refresh");
        println!("\n[OK] Token refreshed (old token rotated)");
        println!("     New access: {}...", &new_pair.access_token[..40]);

        println!("\n=== Endpoints (when server runs) ===\n");
        println!("POST   /auth/token    Login (credentials -> JWT)");
        println!("GET    /auth/keys     JWKS public key distribution");
        println!("POST   /auth/refresh  Refresh access token");
        println!("POST   /auth/revoke   Revoke token by JTI");
        println!("DELETE /tenants/:id/data  GDPR erasure cascade");
        println!("\n=== Multi-Tenant Isolation ===\n");
        println!("- JWT contains tenant_id claim");
        println!("- REST/SSE/WS/GraphQL/gRPC events filtered by tenant");
        println!("- TenantScopedLinkService enforces storage isolation");
        println!("- Notifications are tenant-scoped");
        println!("\n[OK] Example complete.");
    }

    #[cfg(not(feature = "wami"))]
    {
        println!("This example requires the 'wami' feature.");
        println!("Run with: cargo run --example sts_auth --features wami");
    }
}
