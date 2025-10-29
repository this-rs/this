# Getting Started with This-RS

## 🎯 Overview

This-RS is a framework for building **complex multi-entity REST and GraphQL APIs** with **many relationships**. This guide will walk you through building your first API.

> **⚠️ Is This Guide for You?**
> 
> This-RS is designed for APIs with **5+ entities and complex relationships**.  
> If you're building a simple CRUD API (< 5 entities, few relationships), you might be better served by using [Axum](https://github.com/tokio-rs/axum) directly.
>
> **What This-RS actually saves:**
> - ✅ Routing boilerplate (auto-generated routes)
> - ✅ Link management (bidirectional navigation, enrichment)
> - ✅ GraphQL schema (auto-generated from entities)
> 
> **What you still write:**
> - ✍️ Business logic handlers
> - ✍️ Entity definitions (with macro helpers)
> - ✍️ Validation rules
>
> See [Is This-RS Right for You?](../../README.md#is-this-rs-right-for-you) for a detailed comparison.

## 📋 Prerequisites

- Rust 1.70+ installed
- Basic knowledge of Rust and async programming
- Familiarity with REST APIs

## 🚀 Quick Setup

### 1. Add This-RS to Your Project

```toml
[dependencies]
this-rs = "0.0.2"
tokio = { version = "1", features = ["full"] }
axum = "0.7"
serde = { version = "1", features = ["derive"] }
serde_json = "1"
uuid = { version = "1", features = ["v4", "serde"] }
chrono = { version = "0.4", features = ["serde"] }
anyhow = "1"
```

### 2. Create Project Structure

```
your-project/
├── Cargo.toml
├── config/
│   └── links.yaml
└── src/
    ├── main.rs
    └── entities/
        ├── mod.rs
        └── user/
            ├── mod.rs
            ├── model.rs
            ├── store.rs
            ├── handlers.rs
            └── descriptor.rs
```

## 📝 Step-by-Step Tutorial

### Step 1: Define Your Entity with Macros

Create `src/entities/user/model.rs`:

```rust
use this::prelude::*;

// Macro generates complete entity with automatic validation!
impl_data_entity_validated!(
    User, 
    "user", 
    ["name", "email"], 
    {
        email: String,
        age: Option<i32>,
    },
    // Validation rules
    validate: {
        create: {
            name: [required string_length(2, 100)],
            email: [required],
            age: [optional positive],
        },
        update: {
            name: [optional string_length(2, 100)],
            email: [optional],
            age: [optional positive],
        },
    },
    // Filters (data transformation)
    filters: {
        create: {
            name: [trim],
            email: [trim lowercase],
        },
        update: {
            name: [trim],
            email: [trim lowercase],
        },
    }
);

// That's it! You now have:
// - id: Uuid (auto-generated)
// - type: String (auto-set to "user")
// - name: String (required)
// - created_at: DateTime<Utc> (auto-generated)
// - updated_at: DateTime<Utc> (auto-managed)
// - deleted_at: Option<DateTime<Utc>> (soft delete support)
// - status: String (required)
// - email: String (your custom field)
// - age: Option<i32> (your custom field)
//
// Plus automatic validation and filtering before handlers receive data!
// - Constructor: User::new(name, status, email, age)
// - Methods: soft_delete(), touch(), set_status(), restore()
// - Trait implementations: Entity, Data, Clone, Serialize, Deserialize
```

### Step 2: Create Entity Store

Create `src/entities/user/store.rs`:

```rust
use super::model::User;
use anyhow::Result;
use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use this::prelude::*;
use uuid::Uuid;

#[derive(Clone)]
pub struct UserStore {
    data: Arc<RwLock<HashMap<Uuid, User>>>,
}

impl UserStore {
    pub fn new() -> Self {
        Self {
            data: Arc::new(RwLock::new(HashMap::new())),
        }
    }
    
    pub fn get(&self, id: &Uuid) -> Option<User> {
        self.data.read().unwrap().get(id).cloned()
    }
    
    pub fn list(&self) -> Vec<User> {
        self.data.read().unwrap().values().cloned().collect()
    }
    
    pub fn add(&self, user: User) {
        self.data.write().unwrap().insert(user.id, user);
    }
    
    pub fn update(&self, user: User) {
        self.data.write().unwrap().insert(user.id, user);
    }
    
    pub fn delete(&self, id: &Uuid) -> Option<User> {
        self.data.write().unwrap().remove(id)
    }
}

// Implement EntityFetcher for link enrichment
#[async_trait::async_trait]
impl EntityFetcher for UserStore {
    async fn fetch_as_json(&self, entity_id: &Uuid) -> Result<serde_json::Value> {
        let user = self.get(entity_id)
            .ok_or_else(|| anyhow::anyhow!("User not found: {}", entity_id))?;
        Ok(serde_json::to_value(user)?)
    }
}

// Implement EntityCreator for automatic entity creation with links
#[async_trait::async_trait]
impl EntityCreator for UserStore {
    async fn create_from_json(&self, entity_data: serde_json::Value) -> Result<serde_json::Value> {
        let user = User::new(
            entity_data["name"].as_str().unwrap_or("").to_string(),
            entity_data["status"].as_str().unwrap_or("active").to_string(),
            entity_data["email"].as_str().unwrap_or("").to_string(),
            entity_data["age"].as_i64().map(|a| a as i32),
        );
        
        self.add(user.clone());
        Ok(serde_json::to_value(user)?)
    }
}
```

### Step 3: Create HTTP Handlers

Create `src/entities/user/handlers.rs`:

```rust
use super::{model::User, store::UserStore};
use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    Json,
};
use serde_json::Value;
use this::prelude::{Validated, QueryParams, PaginatedResponse, PaginationMeta};
use uuid::Uuid;

#[derive(Clone)]
pub struct UserAppState {
    pub store: UserStore,
}

pub async fn list_users(
    State(state): State<UserAppState>,
    Query(params): Query<QueryParams>,
) -> Json<PaginatedResponse<Value>> {
    let page = params.page();
    let limit = params.limit();
    
    // Get all users
    let mut users = state.store.list();
    
    // Apply filters if provided
    if let Some(filter) = params.filter_value() {
        users = state.store.apply_filters(users, &filter);
    }
    
    let total = users.len();
    
    // ALWAYS paginate
    let start = (page - 1) * limit;
    let paginated: Vec<Value> = users
        .into_iter()
        .skip(start)
        .take(limit)
        .map(|user| serde_json::to_value(user).unwrap())
        .collect();
    
    Json(PaginatedResponse {
        data: paginated,
        pagination: PaginationMeta::new(page, limit, total),
    })
}

pub async fn get_user(
    State(state): State<UserAppState>,
    Path(id): Path<String>,
) -> Result<Json<User>, StatusCode> {
    let id = Uuid::parse_str(&id).map_err(|_| StatusCode::BAD_REQUEST)?;
    state.store.get(&id).map(Json).ok_or(StatusCode::NOT_FOUND)
}

pub async fn create_user(
    State(state): State<UserAppState>,
    validated: Validated<User>,  // ← Data already validated!
) -> Result<Json<User>, StatusCode> {
    let payload = &*validated;
    
    let user = User::new(
        payload["name"].as_str().unwrap().to_string(),
        payload["status"].as_str().unwrap_or("active").to_string(),
        payload["email"].as_str().unwrap().to_string(),
        payload["age"].as_i64().map(|a| a as i32),
    );
    
    state.store.add(user.clone());
    Ok(Json(user))
}

pub async fn update_user(
    State(state): State<UserAppState>,
    Path(id): Path<String>,
    validated: Validated<User>,  // ← Data already validated!
) -> Result<Json<User>, StatusCode> {
    let id = Uuid::parse_str(&id).map_err(|_| StatusCode::BAD_REQUEST)?;
    let mut user = state.store.get(&id).ok_or(StatusCode::NOT_FOUND)?;
    let payload = &*validated;
    
    if let Some(name) = payload["name"].as_str() {
        user.name = name.to_string();
    }
    if let Some(email) = payload["email"].as_str() {
        user.email = email.to_string();
    }
    if let Some(age) = payload["age"].as_i64() {
        user.age = Some(age as i32);
    }
    
    user.touch(); // Updates updated_at timestamp
    state.store.update(user.clone());
    Ok(Json(user))
}

pub async fn delete_user(
    State(state): State<UserAppState>,
    Path(id): Path<String>,
) -> Result<StatusCode, StatusCode> {
    let id = Uuid::parse_str(&id).map_err(|_| StatusCode::BAD_REQUEST)?;
    state.store.delete(&id).ok_or(StatusCode::NOT_FOUND)?;
    Ok(StatusCode::NO_CONTENT)
}
```

### Step 4: Create Entity Descriptor

Create `src/entities/user/descriptor.rs`:

```rust
use super::{handlers::*, store::UserStore};
use axum::{routing::get, Router};
use this::prelude::*;

pub struct UserDescriptor {
    store: UserStore,
}

impl UserDescriptor {
    pub fn new(store: UserStore) -> Self {
        Self { store }
    }
}

impl EntityDescriptor for UserDescriptor {
    fn entity_type(&self) -> &str {
        "user"
    }
    
    fn plural(&self) -> &str {
        "users"
    }
    
    fn build_routes(&self) -> Router {
        let state = UserAppState {
            store: self.store.clone(),
        };
        
        Router::new()
            .route("/users", get(list_users).post(create_user))
            .route(
                "/users/{id}",
                get(get_user).put(update_user).delete(delete_user),
            )
            .with_state(state)
    }
}
```

### Step 5: Create Module

Create `src/entities/mod.rs`:

```rust
pub mod user;

use anyhow::Result;
use std::sync::Arc;
use this::prelude::*;
use user::{descriptor::UserDescriptor, store::UserStore};

pub struct AppModule {
    user_store: Arc<UserStore>,
}

impl AppModule {
    pub fn new(user_store: Arc<UserStore>) -> Self {
        Self { user_store }
    }
}

impl Module for AppModule {
    fn name(&self) -> &str {
        "app-service"
    }
    
    fn entity_types(&self) -> Vec<&str> {
        vec!["user"]
    }
    
    fn links_config(&self) -> Result<LinksConfig> {
        LinksConfig::from_file("config/links.yaml")
    }
    
    fn register_entities(&self, registry: &mut EntityRegistry) {
        registry.register(Box::new(UserDescriptor::new((*self.user_store).clone())));
    }
    
    fn get_entity_fetcher(&self, entity_type: &str) -> Option<Arc<dyn EntityFetcher>> {
        match entity_type {
            "user" => Some(Arc::new((*self.user_store).clone()) as Arc<dyn EntityFetcher>),
            _ => None,
        }
    }
    
    fn get_entity_creator(&self, entity_type: &str) -> Option<Arc<dyn EntityCreator>> {
        match entity_type {
            "user" => Some(Arc::new((*self.user_store).clone()) as Arc<dyn EntityCreator>),
            _ => None,
        }
    }
}
```

### Step 6: Configure Links

Create `config/links.yaml`:

```yaml
entities:
  - singular: user
    plural: users
  - singular: car
    plural: cars

links:
  - link_type: owner
    source_type: user
    target_type: car
    forward_route_name: cars-owned
    reverse_route_name: owner
    description: "User owns a car"
```

### Step 7: Create Main Server

Create `src/main.rs`:

```rust
use anyhow::Result;
use std::sync::Arc;
use this::prelude::*;

mod entities;
use entities::{user::store::UserStore, AppModule};

#[tokio::main]
async fn main() -> Result<()> {
    // Create stores
    let user_store = Arc::new(UserStore::new());
    
    // Create module
    let module = AppModule::new(user_store);
    
    // Build server - all routes auto-generated!
    let app = ServerBuilder::new()
        .with_link_service(InMemoryLinkService::new())
        .register_module(module)?
        .build()?;
    
    // Start server
    let listener = tokio::net::TcpListener::bind("127.0.0.1:3000").await?;
    println!("🚀 Server running on http://127.0.0.1:3000");
    println!("\n✨ Auto-generated routes:");
    println!("  GET    /users              - List all users");
    println!("  POST   /users              - Create a new user");
    println!("  GET    /users/{{id}}          - Get a specific user");
    println!("  PUT    /users/{{id}}          - Update a user");
    println!("  DELETE /users/{{id}}          - Delete a user");
    println!("  GET    /users/{{id}}/links    - List available link types");
    
    axum::serve(listener, app).await?;
    Ok(())
}
```

### Step 8: Run Your Server!

```bash
cargo run
```

## 🧪 Testing Your API

### Create a User

```bash
curl -X POST http://localhost:3000/users \
  -H 'Content-Type: application/json' \
  -d '{
    "name": "Alice",
    "email": "alice@example.com",
    "age": 30,
    "status": "active"
  }'
```

### Get All Users

```bash
curl http://localhost:3000/users | jq .
```

### Update a User

```bash
curl -X PUT http://localhost:3000/users/{user_id} \
  -H 'Content-Type: application/json' \
  -d '{
    "name": "Alice Smith",
    "age": 31
  }'
```

## 🔗 Adding Relationships

### Add a Second Entity (Car)

Follow the same steps to create:
- `entities/car/model.rs`
- `entities/car/store.rs`
- `entities/car/handlers.rs`
- `entities/car/descriptor.rs`

   ```rust
// entities/car/model.rs
impl_data_entity!(Car, "car", ["name", "brand", "model"], {
    brand: String,
    model: String,
    year: i32,
});
```

### Create Links

#### Method 1: Link Existing Entities
   ```bash
curl -X POST http://localhost:3000/users/{user_id}/cars-owned/{car_id} \
  -H 'Content-Type: application/json' \
  -d '{
    "metadata": {
      "purchase_date": "2024-01-15",
      "price": 45000
    }
  }'
```

#### Method 2: Create New Entity + Link
   ```bash
curl -X POST http://localhost:3000/users/{user_id}/cars-owned \
  -H 'Content-Type: application/json' \
  -d '{
    "entity": {
      "name": "Tesla Model 3",
      "brand": "Tesla",
      "model": "Model 3",
      "year": 2023,
      "status": "active"
    },
    "metadata": {
      "purchase_date": "2024-03-20",
      "price": 55000
    }
  }'

# Returns BOTH the created car AND the link!
```

### Query Links (Auto-Enriched!)

```bash
# List cars owned by user (includes full car data!)
curl http://localhost:3000/users/{user_id}/cars-owned | jq .

# Response includes enriched entities:
{
  "links": [
    {
      "id": "link-uuid",
      "source_id": "user-uuid",
      "target_id": "car-uuid",
      "target": {
        "id": "car-uuid",
        "type": "car",
        "name": "Tesla Model 3",
        "brand": "Tesla",
        "model": "Model 3",
        "year": 2023,
        ...
      },
      "metadata": {
        "purchase_date": "2024-01-15",
        "price": 45000
      }
    }
  ]
}

# Reverse navigation: Get owner of a car
curl http://localhost:3000/cars/{car_id}/owner | jq .
```

## 🎯 Key Concepts

### 1. Entity Hierarchy

```
Entity (Base)
  ├─► Data (Business objects: User, Car, Order, etc.)
  └─► Link (Relationships between entities)
```

### 2. Macros Eliminate Boilerplate

```rust
// Just 4 lines
impl_data_entity!(User, "user", ["name", "email"], {
    email: String,
});

// Generates 100+ lines of code!
```

### 3. Module System

- Groups related entities
- Provides EntityFetcher for link enrichment
- Provides EntityCreator for auto-creation
- Registers routes automatically

### 4. Auto-Generated Routes

- CRUD routes for entities
- Generic link routes
- Bidirectional navigation
- Auto-enriched responses

## 📚 Next Steps

- [Quick Start Guide](QUICK_START.md) - Fast intro
- [Validation & Filtering](VALIDATION_AND_FILTERING.md) - Automatic data validation
- [Pagination & Filtering](PAGINATION_AND_FILTERING.md) - 🆕 Generic pagination and query filtering
- [Enriched Links](ENRICHED_LINKS.md) - Link enrichment details
- [Multi-Level Navigation](MULTI_LEVEL_NAVIGATION.md) - Complex relationships
- [Architecture](../architecture/ARCHITECTURE.md) - Technical deep dive
- [Microservice Example](../../examples/microservice/README.md) - Production patterns

## 💡 Tips & Best Practices

### Use Macros for All Entities

```rust
// ✅ Do this with validation
impl_data_entity_validated!(
    Order, 
    "order", 
    ["name"], 
    { amount: f64 },
    validate: {
        create: { amount: [required positive] },
    },
    filters: {
        create: { amount: [round_decimals(2)] },
    }
);

// ❌ Don't manually define entities
```

### Implement Both EntityFetcher and EntityCreator

```rust
// ✅ Enables link enrichment AND auto-creation
impl EntityFetcher for OrderStore { /* ... */ }
impl EntityCreator for OrderStore { /* ... */ }
```

### Keep Module Configuration in YAML

```yaml
# ✅ Easy to change, no recompilation needed
links:
  - link_type: owner
    source_type: user
    target_type: car
```

### Use Soft Deletes

```rust
// ✅ Never lose data
user.soft_delete();  // Sets deleted_at timestamp

// ✅ Can be restored later
user.restore();  // Clears deleted_at
```

## 🎉 Congratulations!

You've built a complete RESTful API with:
- ✅ Auto-generated CRUD routes
- ✅ Auto-generated link routes
- ✅ Bidirectional navigation
- ✅ Link enrichment (no N+1 queries!)
- ✅ Automatic entity creation with linking
- ✅ Zero boilerplate code

**Welcome to the This-RS community!** 🚀🦀✨
