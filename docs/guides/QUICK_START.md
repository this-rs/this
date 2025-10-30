# 🚀 Quick Start Guide - this-rs

> **⚠️ Before You Start**: this-rs is designed for **complex APIs with 5+ entities and many relationships**.  
> If you're building a simple CRUD API (< 5 entities), consider using [Axum](https://github.com/tokio-rs/axum) directly.  
> See [Is this-rs Right for You?](../../README.md#is-this-rs-right-for-you) in the main README.

---

## Quick Installation

```bash
# Clone the project
cd this-rs

# Verify everything compiles
cargo check

# Run tests
cargo test

# Run the complete example
cargo run --example microservice
```

## 📖 Your First Server

### 1. Create Your `links.yaml` Configuration

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

### 2. Define Your Entities with Macros

```rust
use this::prelude::*;

// Macro automatically generates complete entity with automatic validation
impl_data_entity_validated!(
    User, 
    "user", 
    ["name", "email"], 
    { email: String, },
    validate: {
        create: {
            name: [required string_length(2, 100)],
            email: [required],
        },
    },
    filters: {
        create: {
            name: [trim],
            email: [trim lowercase],
        },
    }
);

impl_data_entity_validated!(
    Car, 
    "car", 
    ["name", "brand", "model"], 
    { 
        brand: String, 
        model: String, 
        year: i32, 
    },
    validate: {
        create: {
            name: [required string_length(2, 100)],
            brand: [required],
            model: [required],
            year: [required positive],
        },
    },
    filters: {
        create: {
            name: [trim],
            brand: [trim],
            model: [trim],
        },
    }
);

// Each entity automatically includes:
// - id: Uuid (auto-generated)
// - type: String (auto-set to entity type)
// - name: String (required)
// - created_at: DateTime<Utc> (auto-generated)
// - updated_at: DateTime<Utc> (auto-managed)
// - deleted_at: Option<DateTime<Utc>> (soft delete support)
// - status: String (required)
//
// Plus automatic validation and filtering before handlers receive data!
```

### 3. Create Entity Stores with EntityFetcher & EntityCreator

```rust
use this::prelude::*;

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
    
    pub fn add(&self, user: User) {
        self.data.write().unwrap().insert(user.id, user);
    }
}

// Implement EntityFetcher for link enrichment
#[async_trait]
impl EntityFetcher for UserStore {
    async fn fetch_as_json(&self, entity_id: &Uuid) -> Result<serde_json::Value> {
        let user = self.get(entity_id)
            .ok_or_else(|| anyhow::anyhow!("User not found: {}", entity_id))?;
        Ok(serde_json::to_value(user)?)
    }
}

// Implement EntityCreator for automatic entity creation with links
#[async_trait]
impl EntityCreator for UserStore {
    async fn create_from_json(&self, entity_data: serde_json::Value) -> Result<serde_json::Value> {
        let user = User::new(
            entity_data["name"].as_str().unwrap_or("").to_string(),
            entity_data["status"].as_str().unwrap_or("active").to_string(),
            entity_data["email"].as_str().unwrap_or("").to_string(),
        );
        self.add(user.clone());
        Ok(serde_json::to_value(user)?)
    }
}
```

### 4. Create Your Module

```rust
use this::prelude::*;

pub struct AppModule {
    user_store: Arc<UserStore>,
    car_store: Arc<CarStore>,
}

impl Module for AppModule {
    fn name(&self) -> &str {
        "app-service"
    }
    
    fn entity_types(&self) -> Vec<&str> {
        vec!["user", "car"]
    }
    
    fn links_config(&self) -> Result<LinksConfig> {
        LinksConfig::from_file("config/links.yaml")
    }
    
    fn register_entities(&self, registry: &mut EntityRegistry) {
        registry.register(Box::new(UserDescriptor::new(self.user_store.clone())));
        registry.register(Box::new(CarDescriptor::new(self.car_store.clone())));
    }
    
    fn get_entity_fetcher(&self, entity_type: &str) -> Option<Arc<dyn EntityFetcher>> {
        match entity_type {
            "user" => Some(Arc::new(self.user_store.clone()) as Arc<dyn EntityFetcher>),
            "car" => Some(Arc::new(self.car_store.clone()) as Arc<dyn EntityFetcher>),
            _ => None,
        }
    }
    
    fn get_entity_creator(&self, entity_type: &str) -> Option<Arc<dyn EntityCreator>> {
        match entity_type {
            "user" => Some(Arc::new(self.user_store.clone()) as Arc<dyn EntityCreator>),
            "car" => Some(Arc::new(self.car_store.clone()) as Arc<dyn EntityCreator>),
            _ => None,
        }
    }
}
```

### 5. Build Your Server

```rust
use this::prelude::*;

#[tokio::main]
async fn main() -> Result<()> {
    // Create stores
    let user_store = Arc::new(UserStore::new());
    let car_store = Arc::new(CarStore::new());
    
    // Create module
    let module = AppModule::new(user_store, car_store);
    
    // Build server - all routes auto-generated!
    let app = ServerBuilder::new()
        .with_link_service(InMemoryLinkService::new())
        .register_module(module)?
        .build()?;
    
    // Start server
    let listener = tokio::net::TcpListener::bind("127.0.0.1:3000").await?;
    println!("🚀 Server running on http://127.0.0.1:3000");
    
    axum::serve(listener, app).await?;
    Ok(())
}
```

### 6. Test Your API

#### Create Entities
```bash
# Create a user
curl -X POST http://localhost:3000/users \
  -H "Content-Type: application/json" \
  -d '{
    "name": "Alice",
    "email": "alice@example.com",
    "status": "active"
  }'

# Create a car
curl -X POST http://localhost:3000/cars \
  -H "Content-Type: application/json" \
  -d '{
    "name": "Tesla Model 3",
    "brand": "Tesla",
    "model": "Model 3",
    "year": 2023,
    "status": "active"
  }'
```

#### Two Ways to Create Links

**Method 1: Link Existing Entities**
```bash
# Link existing user and car
curl -X POST http://localhost:3000/users/{user_id}/cars-owned/{car_id} \
  -H "Content-Type: application/json" \
  -d '{
    "metadata": {
      "purchase_date": "2024-01-15",
      "price": 45000
    }
  }'
```

**Method 2: Create New Entity + Link Automatically**
```bash
# Create a new car AND link it to the user in one call
curl -X POST http://localhost:3000/users/{user_id}/cars-owned \
  -H "Content-Type: application/json" \
  -d '{
    "entity": {
      "name": "BMW X5",
      "brand": "BMW",
      "model": "X5",
      "year": 2024,
      "status": "active"
    },
    "metadata": {
      "purchase_date": "2024-03-20",
      "price": 65000
    }
  }'

# Response includes both the created car AND the link!
{
  "entity": {
    "id": "car-uuid",
    "type": "car",
    "name": "BMW X5",
    "brand": "BMW",
    "model": "X5",
    "year": 2024,
    ...
  },
  "link": {
    "id": "link-uuid",
    "source_id": "user-uuid",
    "target_id": "car-uuid",
    "link_type": "owner",
    ...
  }
}
```

#### Query Links (Auto-Enriched!)
```bash
# List cars owned by a user (includes full car data!)
curl http://localhost:3000/users/{user_id}/cars-owned | jq .

# Response with enriched entities:
{
  "links": [
    {
      "id": "link-123",
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

# Get owner of a car (reverse navigation)
curl http://localhost:3000/cars/{car_id}/owner | jq .

# Discover all available link routes for an entity
curl http://localhost:3000/users/{user_id}/links | jq .
```

## 🎯 Advanced Use Cases

### Multiple Link Types

You can have multiple link types between the same entities:

```yaml
links:
  # User owns a Car
  - link_type: owner
    source_type: user
    target_type: car
    forward_route_name: cars-owned
    reverse_route_name: owner
  
  # User drives a Car (different from owning!)
  - link_type: driver
    source_type: user
    target_type: car
    forward_route_name: cars-driven
    reverse_route_name: drivers
```

This automatically generates:
- `GET /users/{id}/cars-owned` - cars owned by user
- `GET /users/{id}/cars-driven` - cars driven by user
- `GET /cars/{id}/owner` - owner of the car
- `GET /cars/{id}/drivers` - drivers of the car

### Link Metadata

```rust
// Links can carry rich metadata
let metadata = serde_json::json!({
    "role": "Senior Developer",
    "start_date": "2024-01-01",
    "salary": 75000,
    "department": "Engineering"
});

// Metadata is returned with link queries
```

### Custom Validation Rules

```yaml
validation_rules:
  owner:
    - source: user
      targets: [car, house, company]
    - source: company
      targets: [car, building]
  
  driver:
    - source: user
      targets: [car, truck]
```

If you attempt to create an invalid link (e.g., `company` driving `car`), the API returns an error.

## 📚 Complete Examples

The project includes functional examples:

```bash
# Simple example with in-memory data
cargo run --example simple_api

# Complete microservice with auto-generated routes
cargo run --example microservice

# Full API example with Axum server
cargo run --example full_api
```

## 🔧 Advanced Configuration

### Irregular Plurals

The system automatically handles irregular plurals:
- `company` → `companies` ✅
- `address` → `addresses` ✅
- `knife` → `knives` ✅

But you can also specify them manually:

```yaml
entities:
  - singular: person
    plural: people  # Manually specified
  
  - singular: datum
    plural: data    # Manually specified
```

### Authorization Policies

```yaml
links:
  - link_type: owner
    source_type: user
    target_type: car
    forward_route_name: cars-owned
    reverse_route_name: owner
    auth:
      create:
        policy: AllowOwner
        roles: ["admin", "user"]
      delete:
        policy: RequireRole
        roles: ["admin"]
```

## 🎓 Architecture

```
┌─────────────────────────────────────────┐
│         Your Application                │
│  (User, Car, Company, etc.)             │
└────────────┬────────────────────────────┘
             │
             ▼
┌─────────────────────────────────────────┐
│         this-rs Framework               │
│                                         │
│  ┌──────────┐  ┌──────────────┐       │
│  │  Core    │  │   Links      │       │
│  │ (Generic)│  │  (Agnostic)  │       │
│  └──────────┘  └──────────────┘       │
│                                         │
│  ┌──────────────────────────────┐     │
│  │    HTTP Handlers (Axum)      │     │
│  └──────────────────────────────┘     │
└─────────────────────────────────────────┘
             │
             ▼
┌─────────────────────────────────────────┐
│      Storage (InMemory / DynamoDB)      │
└─────────────────────────────────────────┘
```

## 📖 Complete Documentation

For more details, see:
- [README.md](../../README.md) - Complete overview
- [ARCHITECTURE.md](../architecture/ARCHITECTURE.md) - Detailed architecture
- [VALIDATION_AND_FILTERING.md](VALIDATION_AND_FILTERING.md) - 🆕 Automatic data validation
- [ENRICHED_LINKS.md](ENRICHED_LINKS.md) - Link enrichment guide
- [GETTING_STARTED.md](GETTING_STARTED.md) - Step-by-step tutorial

## 💡 Help and Support

- 📝 Documentation: [docs.rs/this-rs](https://docs.rs/this-rs)
- 🐛 Issues: GitHub Issues
- 💬 Discussions: GitHub Discussions

---

**Ready to build your API?** 🚀

```bash
cargo run --example microservice
```
