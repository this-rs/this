# Custom Routes Guide

This guide explains how to add custom routes to your `this-rs` application for endpoints that don't fit the standard CRUD pattern.

## Overview

While `this-rs` automatically generates CRUD routes for entities and link management routes, you often need custom endpoints for:

- **Authentication** (`/login`, `/logout`, `/register`)
- **OAuth flows** (`/oauth/token`, `/oauth/authorize`)
- **Webhooks** (`/webhooks/stripe`, `/webhooks/github`)
- **Custom business logic** (`/reports`, `/analytics`, `/export`)

The `ServerBuilder::with_custom_routes()` method allows you to add any Axum router to your application.

## Basic Usage

```rust
use axum::{Router, routing::post, Json};
use serde_json::json;
use this::prelude::*;

#[tokio::main]
async fn main() -> Result<()> {
    let link_service = InMemoryLinkService::new();
    
    // Define your custom routes
    let custom_routes = Router::new()
        .route("/login", post(login_handler))
        .route("/logout", post(logout_handler));
    
    // Add them to the server
    ServerBuilder::new()
        .with_link_service(link_service)
        .with_custom_routes(custom_routes)
        .serve("127.0.0.1:3000")
        .await?;
    
    Ok(())
}

async fn login_handler(Json(payload): Json<LoginRequest>) -> Json<Value> {
    // Your login logic here
    Json(json!({
        "token": "jwt_token_here",
        "user_id": "123"
    }))
}
```

## Multiple Custom Route Groups

You can call `with_custom_routes()` multiple times to organize routes by domain:

```rust
let auth_routes = Router::new()
    .route("/auth/login", post(login_handler))
    .route("/auth/logout", post(logout_handler))
    .route("/auth/register", post(register_handler));

let oauth_routes = Router::new()
    .route("/oauth/token", post(oauth_token_handler))
    .route("/oauth/authorize", get(oauth_authorize_handler));

let webhook_routes = Router::new()
    .route("/webhooks/stripe", post(stripe_webhook))
    .route("/webhooks/github", post(github_webhook));

ServerBuilder::new()
    .with_link_service(link_service)
    .with_custom_routes(auth_routes)
    .with_custom_routes(oauth_routes)
    .with_custom_routes(webhook_routes)
    .register_module(my_module)?
    .serve("127.0.0.1:3000")
    .await?;
```

## Route Precedence

Routes are evaluated in this order:

1. **Health check routes** (`/health`, `/healthz`) - Always first
2. **Entity CRUD routes** - Auto-generated from modules
3. **Custom routes** - Added via `with_custom_routes()`
4. **Link routes** - Auto-generated, with fallback handler (always last)

This ensures your custom routes won't be overridden by the link route fallback handler.

## Example: Authentication Routes

```rust
use axum::{Router, routing::{post, get}, Json};
use serde::{Deserialize, Serialize};

#[derive(Deserialize)]
struct LoginRequest {
    username: String,
    password: String,
}

#[derive(Serialize)]
struct AuthResponse {
    token: String,
    user_id: String,
}

async fn login_handler(
    Json(req): Json<LoginRequest>
) -> Json<AuthResponse> {
    // Validate credentials (use your auth service)
    let token = generate_jwt(&req.username);
    
    Json(AuthResponse {
        token,
        user_id: get_user_id(&req.username),
    })
}

async fn logout_handler() -> Json<Value> {
    // Invalidate token logic
    Json(json!({ "message": "Logged out successfully" }))
}

async fn me_handler() -> Json<User> {
    // Extract user from JWT token (use middleware)
    Json(get_current_user())
}

let auth_routes = Router::new()
    .route("/auth/login", post(login_handler))
    .route("/auth/logout", post(logout_handler))
    .route("/auth/me", get(me_handler));
```

## Example: OAuth Flow

```rust
#[derive(Deserialize)]
struct TokenRequest {
    grant_type: String,
    code: Option<String>,
    refresh_token: Option<String>,
}

#[derive(Serialize)]
struct TokenResponse {
    access_token: String,
    token_type: String,
    expires_in: u64,
    refresh_token: String,
}

async fn oauth_token_handler(
    Json(req): Json<TokenRequest>
) -> Json<TokenResponse> {
    match req.grant_type.as_str() {
        "authorization_code" => {
            // Exchange code for token
            let token = exchange_code(req.code.unwrap());
            Json(TokenResponse { /* ... */ })
        }
        "refresh_token" => {
            // Refresh the token
            let token = refresh_access_token(req.refresh_token.unwrap());
            Json(TokenResponse { /* ... */ })
        }
        _ => panic!("Unsupported grant type")
    }
}

let oauth_routes = Router::new()
    .route("/oauth/token", post(oauth_token_handler))
    .route("/oauth/authorize", get(oauth_authorize_handler))
    .route("/oauth/callback", get(oauth_callback_handler));
```

## Example: Webhook Handlers

```rust
use axum::http::StatusCode;

async fn stripe_webhook_handler(
    Json(payload): Json<Value>
) -> StatusCode {
    // Verify webhook signature (important!)
    if !verify_stripe_signature(&payload) {
        return StatusCode::UNAUTHORIZED;
    }
    
    // Process webhook event
    process_stripe_event(payload).await;
    
    StatusCode::OK
}

async fn github_webhook_handler(
    Json(payload): Json<Value>
) -> StatusCode {
    // Verify webhook signature
    if !verify_github_signature(&payload) {
        return StatusCode::UNAUTHORIZED;
    }
    
    // Process GitHub event
    process_github_event(payload).await;
    
    StatusCode::OK
}

let webhook_routes = Router::new()
    .route("/webhooks/stripe", post(stripe_webhook_handler))
    .route("/webhooks/github", post(github_webhook_handler));
```

## Sharing State with Custom Routes

If your custom routes need access to services (database, caches, etc.), use Axum's state management:

```rust
use axum::extract::State;
use std::sync::Arc;

#[derive(Clone)]
struct AppState {
    db: Arc<Database>,
    redis: Arc<RedisPool>,
}

async fn login_handler(
    State(state): State<AppState>,
    Json(req): Json<LoginRequest>
) -> Json<AuthResponse> {
    // Access shared state
    let user = state.db.find_user(&req.username).await?;
    // ... rest of login logic
}

let app_state = AppState {
    db: Arc::new(database),
    redis: Arc::new(redis_pool),
};

let auth_routes = Router::new()
    .route("/auth/login", post(login_handler))
    .with_state(app_state);

ServerBuilder::new()
    .with_link_service(link_service)
    .with_custom_routes(auth_routes)
    .serve("127.0.0.1:3000")
    .await?;
```

## Middleware for Custom Routes

You can add middleware (authentication, logging, etc.) to your custom routes:

```rust
use axum::middleware;
use tower_http::cors::CorsLayer;

async fn auth_middleware(
    req: Request<Body>,
    next: Next<Body>
) -> Result<Response, StatusCode> {
    // Verify JWT token
    let token = extract_token(&req)?;
    verify_jwt(token)?;
    
    Ok(next.run(req).await)
}

let protected_routes = Router::new()
    .route("/admin/users", get(list_users))
    .route("/admin/settings", get(get_settings))
    .layer(middleware::from_fn(auth_middleware));

let public_routes = Router::new()
    .route("/auth/login", post(login_handler))
    .layer(CorsLayer::permissive());

ServerBuilder::new()
    .with_link_service(link_service)
    .with_custom_routes(public_routes)
    .with_custom_routes(protected_routes)
    .serve("127.0.0.1:3000")
    .await?;
```

## Best Practices

### 1. Organize Routes by Domain

Group related routes together:

```rust
// ✅ Good
let auth_routes = Router::new()
    .route("/auth/login", post(login))
    .route("/auth/logout", post(logout))
    .route("/auth/register", post(register));

// ❌ Bad
let routes = Router::new()
    .route("/login", post(login))
    .route("/export", get(export))
    .route("/webhook", post(webhook));
```

### 2. Use Consistent URL Patterns

```rust
// ✅ Good - namespaced
/auth/login
/auth/logout
/oauth/token
/webhooks/stripe

// ❌ Bad - no namespace
/login
/logout
/token
/stripe
```

### 3. Add Error Handling

```rust
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};

async fn login_handler(
    Json(req): Json<LoginRequest>
) -> Result<Json<AuthResponse>, AppError> {
    let user = validate_credentials(&req)
        .await
        .map_err(|_| AppError::InvalidCredentials)?;
    
    Ok(Json(AuthResponse { /* ... */ }))
}

// Custom error type
struct AppError {
    message: String,
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        (StatusCode::UNAUTHORIZED, self.message).into_response()
    }
}
```

### 4. Document Your Custom Routes

Add route documentation in your startup logs:

```rust
println!("🔐 Authentication Routes:");
println!("  POST   /auth/login");
println!("  POST   /auth/logout");
println!("  GET    /auth/me");
```

## Testing Custom Routes

```rust
use axum_test::TestServer;

#[tokio::test]
async fn test_login() {
    let app = ServerBuilder::new()
        .with_link_service(InMemoryLinkService::new())
        .with_custom_routes(auth_routes())
        .build()?;
    
    let server = TestServer::new(app)?;
    
    let response = server
        .post("/auth/login")
        .json(&json!({
            "username": "test",
            "password": "password"
        }))
        .await;
    
    response.assert_status_ok();
    assert!(response.json::<AuthResponse>().token.len() > 0);
}
```

## See Also

- [Axum Documentation](https://docs.rs/axum/latest/axum/)
- [Quick Start Guide](./QUICK_START.md)
- [Link Authorization](./LINK_AUTHORIZATION.md)
