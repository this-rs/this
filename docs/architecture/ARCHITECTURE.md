# this-rs Framework Architecture

## 📐 Overview

```
┌─────────────────────────────────────────────────────────────┐
│                     USER APPLICATION                        │
│                                                             │
│  ┌──────────┐  ┌──────────┐  ┌──────────┐                │
│  │   User   │  │ Company  │  │   Car    │  ... Entities  │
│  └────┬─────┘  └────┬─────┘  └────┬─────┘                │
│       │             │              │                        │
│       └─────────────┴──────────────┘                       │
│                     │                                       │
└─────────────────────┼───────────────────────────────────────┘
                      │
                      ▼
┌─────────────────────────────────────────────────────────────┐
│                   THIS-RS FRAMEWORK                         │
│                                                             │
│  ┌───────────────────────────────────────────────────┐    │
│  │              CORE MODULE (Generic)                 │    │
│  │                                                    │    │
│  │  ┌──────────┐  ┌──────────┐  ┌──────────┐      │    │
│  │  │  Entity  │  │  Link    │  │  Field   │      │    │
│  │  │  Traits  │  │  System  │  │  System  │      │    │
│  │  └──────────┘  └──────────┘  └──────────┘      │    │
│  │                                                    │    │
│  │  ┌──────────┐  ┌──────────┐  ┌──────────┐      │    │
│  │  │ Service  │  │Pluralize │  │  Module  │      │    │
│  │  │  Traits  │  │  System  │  │  System  │      │    │
│  │  └──────────┘  └──────────┘  └──────────┘      │    │
│  └───────────────────────────────────────────────────┘    │
│                           │                                 │
│       ┌───────────────────┼───────────────────┐           │
│       ▼                   ▼                   ▼           │
│  ┌─────────┐      ┌─────────────┐      ┌─────────┐      │
│  │ LINKS   │      │   CONFIG    │      │ENTITIES │      │
│  │ MODULE  │◄─────┤   MODULE    │─────►│ MODULE  │      │
│  │         │      │             │      │         │      │
│  │ Service │      │ YAML Loader │      │ Macros  │      │
│  │Registry │      │   Parser    │      │         │      │
│  └─────────┘      └─────────────┘      └─────────┘      │
│                                                             │
└─────────────────────────────────────────────────────────────┘
                      │
                      ▼
            ┌──────────────────┐
            │   STORAGE LAYER  │
            │                  │
            │  ┌────────────┐  │
            │  │  In-Memory │  │
            │  └────────────┘  │
            │  ┌────────────┐  │
            │  │  DynamoDB  │  │
            │  └────────────┘  │
            └──────────────────┘
```

## 🏗️ Detailed Modules

### 1. Core Module (Generic)

The heart of the framework, completely agnostic of concrete entity types.

```
src/core/
├── entity.rs       ← Fundamental traits
│   ├── Entity      : Base trait (id, type, timestamps, status)
│   ├── Data        : Inherits Entity, adds name + indexed fields
│   └── Link        : Inherits Entity, adds source_id + target_id
│
├── link.rs         ← Polymorphic link system
│   ├── LinkEntity     : Concrete link implementation
│   └── LinkDefinition : Link type configuration
│
├── field.rs        ← Types and validation
│   ├── FieldValue     : Polymorphic value (String, Int, UUID, etc.)
│   └── FieldFormat    : Validators (Email, URL, Phone, Custom)
│
├── service.rs      ← Service traits
│   ├── DataService<T> : CRUD for entities
│   └── LinkService    : CRUD for links
│
├── module.rs       ← Module system
│   ├── Module         : Groups entities + config
│   ├── EntityFetcher  : Fetch entities for link enrichment
│   └── EntityCreator  : Create entities dynamically
│
├── pluralize.rs    ← Pluralization
│   └── Pluralizer     : company → companies
│
├── auth.rs         ← Authorization
│   ├── AuthProvider   : Auth policy provider
│   └── AuthContext    : User auth context
│
└── extractors.rs   ← HTTP extractors (Axum)
    ├── LinkExtractor      : Extract link info from URL
    └── DirectLinkExtractor: Extract specific link from URL
```

**Key Principle**: No reference to concrete types (User, Car, etc.) in core.

## 🎯 Entity Hierarchy

The framework provides a 3-level entity hierarchy:

```
┌─────────────────────────────────────────┐
│              Entity (Base)              │
│  - id: Uuid (auto-generated)            │
│  - type: String (auto-set)              │
│  - created_at: DateTime<Utc>            │
│  - updated_at: DateTime<Utc>            │
│  - deleted_at: Option<DateTime<Utc>>    │
│  - status: String                       │
└─────────────────┬───────────────────────┘
                  │
         ┌────────┴────────┐
         │                 │
         ▼                 ▼
┌─────────────────┐  ┌─────────────────┐
│  Data Entity    │  │  Link Entity    │
│  (Inherits)     │  │  (Inherits)     │
│                 │  │                 │
│  + name: String │  │  + source_id    │
│  + custom fields│  │  + target_id    │
│                 │  │  + link_type    │
└─────────────────┘  └─────────────────┘
```

### Entity (Base Trait)

All entities inherit these fields:
```rust
pub trait Entity {
    fn id(&self) -> Uuid;
    fn entity_type(&self) -> &str;
    fn created_at(&self) -> DateTime<Utc>;
    fn updated_at(&self) -> DateTime<Utc>;
    fn deleted_at(&self) -> Option<DateTime<Utc>>;
    fn status(&self) -> &str;
    
    // Utility methods
    fn is_deleted(&self) -> bool;
    fn is_active(&self) -> bool;
}
```

### Data (Inherits Entity)

Data entities represent domain objects:
```rust
pub trait Data: Entity {
    fn name(&self) -> &str;
    fn indexed_fields(&self) -> Vec<&str>;
    fn field_value(&self, field_name: &str) -> Option<FieldValue>;
}
```

### Link (Inherits Entity)

Link entities represent relationships:
```rust
pub trait Link: Entity {
    fn source_id(&self) -> Uuid;
    fn target_id(&self) -> Uuid;
    fn link_type(&self) -> &str;
}
```

## 🔧 Macro System

The framework provides powerful macros to eliminate boilerplate:

### `impl_data_entity!`

Generates a complete Data entity:

```rust
impl_data_entity!(Order, "order", ["name", "number"], {
    number: String,
    amount: f64,
    customer_name: Option<String>,
    notes: Option<String>,
});
```

**Generates**:
```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Order {
    // Base Entity fields (auto-injected)
    pub id: Uuid,
    pub entity_type: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub deleted_at: Option<DateTime<Utc>>,
    pub status: String,
    
    // Data field (auto-injected)
    pub name: String,
    
    // Custom fields
    pub number: String,
    pub amount: f64,
    pub customer_name: Option<String>,
    pub notes: Option<String>,
}

impl Entity for Order { /* auto-implemented */ }
impl Data for Order { /* auto-implemented */ }

impl Order {
    pub fn new(...) -> Self { /* auto-generated constructor */ }
    pub fn soft_delete(&mut self) { /* soft delete support */ }
    pub fn touch(&mut self) { /* update timestamp */ }
    pub fn set_status(&mut self, status: String) { /* status update */ }
    pub fn restore(&mut self) { /* restore from soft delete */ }
}
```

`soft_delete()` and `restore()` only set and clear `deleted_at` on the entity value. They do not change the DELETE routes: a REST `DELETE` removes the row by default (`src/server/module_runtime/json_entity.rs`). The contract applies to the JSON surfaces only after `ServerBuilder::with_soft_delete()`, and to REST handlers you own through `SoftDeleting::new(service)` (`src/core/soft_delete.rs`).

### `impl_link_entity!`

Generates a custom Link entity:

```rust
impl_link_entity!(CustomLink, "custom_link", {
    metadata_field: String,
});
```

### Field Injection Macros

- `entity_fields!()` - Inject base Entity fields
- `data_fields!()` - Inject Entity + name fields
- `link_fields!()` - Inject Entity + link fields

## 🔗 Links Module (Agnostic)

Manages relationships between entities without knowing their types.

```
src/links/
├── handlers.rs     ← HTTP handlers (Axum)
│   ├── list_links          : GET /{entity}/{id}/{route_name}
│   ├── get_link            : GET /links/{link_id}
│   ├── get_link_by_route   : GET /{source}/{sid}/{route}/{tid}
│   ├── create_link         : POST /{source}/{sid}/{route}/{tid}
│   ├── create_linked_entity: POST /{source}/{sid}/{route}
│   ├── update_link         : PUT /{source}/{sid}/{route}/{tid}
│   └── delete_link         : DELETE /{source}/{sid}/{route}/{tid}
│
├── registry.rs     ← Route resolution
│   └── LinkRouteRegistry : URL → LinkDefinition
│
└── service.rs      ← Link service implementation
    └── (moved to storage/)
```

### Generated Routes

From YAML configuration:
```yaml
links:
  - link_type: owner
    source_type: user
    target_type: car
    forward_route_name: cars-owned
    reverse_route_name: owner
```

**Auto-generated routes**:
```
GET    /users/{id}/cars-owned              → List cars owned by user
POST   /users/{id}/cars-owned              → Create new car + link
GET    /users/{id}/cars-owned/{car_id}     → Get specific link
POST   /users/{id}/cars-owned/{car_id}     → Link existing car
PUT    /users/{id}/cars-owned/{car_id}     → Update link metadata
DELETE /users/{id}/cars-owned/{car_id}     → Delete link

GET    /cars/{id}/owner                    → Get owner of car (reverse)
```

## 🎨 Module System

Modules group related entities and provide services:

```rust
pub trait Module: Send + Sync {
    fn name(&self) -> &str;
    fn entity_types(&self) -> Vec<&str>;
    fn links_config(&self) -> Result<LinksConfig>;
    fn register_entities(&self, registry: &mut EntityRegistry);
    fn get_entity_fetcher(&self, entity_type: &str) -> Option<Arc<dyn EntityFetcher>>;
    fn get_entity_creator(&self, entity_type: &str) -> Option<Arc<dyn EntityCreator>>;
}
```

### EntityFetcher

Dynamically fetches entities for link enrichment:

```rust
#[async_trait]
pub trait EntityFetcher: Send + Sync {
    async fn fetch_as_json(&self, entity_id: &Uuid) -> Result<serde_json::Value>;
}
```

**Implementation example**:
```rust
#[async_trait]
impl EntityFetcher for OrderStore {
    async fn fetch_as_json(&self, entity_id: &Uuid) -> Result<serde_json::Value> {
        let order = self.get(entity_id)
            .ok_or_else(|| anyhow::anyhow!("Order not found"))?;
        Ok(serde_json::to_value(order)?)
    }
}
```

### EntityCreator

Dynamically creates entities with automatic linking:

```rust
#[async_trait]
pub trait EntityCreator: Send + Sync {
    async fn create_from_json(&self, entity_data: serde_json::Value) -> Result<serde_json::Value>;
}
```

**Implementation example**:
```rust
#[async_trait]
impl EntityCreator for OrderStore {
    async fn create_from_json(&self, entity_data: serde_json::Value) -> Result<serde_json::Value> {
        let order = Order::new(
            entity_data["number"].as_str().unwrap_or("").to_string(),
            entity_data["status"].as_str().unwrap_or("active").to_string(),
            // ... other fields
        );
        self.add(order.clone());
        Ok(serde_json::to_value(order)?)
    }
}
```

## 🔄 Data Flow

### Creating a Link Between Existing Entities

```
1. HTTP Request
   POST /users/{user_id}/cars-owned/{car_id}
   Body: { "metadata": { "purchase_date": "2024-01-15" } }

2. Axum Handler (create_link)
   ↓ DirectLinkExtractor parses URL
   ↓ Extracts: source_id, target_id, link_type

3. LinkService.create()
   ↓ Creates LinkEntity
   ↓ link_type: "owner"
   ↓ source_id: user_id (UUID)
   ↓ target_id: car_id (UUID)

4. Storage Layer
   ↓ Insert LinkEntity

5. Response
   ← LinkEntity { id, link_type, source_id, target_id, ... }
```

### Creating New Entity + Link Automatically

```
1. HTTP Request
   POST /users/{user_id}/cars-owned
   Body: {
     "entity": { "name": "Tesla Model 3", "brand": "Tesla", ... },
     "metadata": { "purchase_date": "2024-01-15" }
   }

2. Axum Handler (create_linked_entity)
   ↓ LinkExtractor parses URL
   ↓ Extracts: source_id, entity_type, route_name

3. Get EntityCreator from Module
   ↓ module.get_entity_creator("car")

4. EntityCreator.create_from_json()
   ↓ Creates new Car entity
   ↓ Stores in database
   ↓ Returns created entity with ID

5. LinkService.create()
   ↓ Creates link: user_id → car_id

6. Response
   ← {
       "entity": { /* created car */ },
       "link": { /* created link */ }
     }
```

### Querying Links (Auto-Enriched)

```
1. HTTP Request
   GET /users/{user_id}/cars-owned

2. LinkRouteRegistry.resolve_route()
   ↓ "user" + "cars-owned"
   ↓ → LinkDefinition { link_type: "owner", ... }
   ↓ → Direction: Forward

3. LinkService.find_by_source()
   ↓ source_id: user_id
   ↓ source_type: "user"
   ↓ link_type: Some("owner")
   ↓ target_type: Some("car")

4. Enrich Links (enrich_links_with_entities)
   ↓ For each link:
   ↓   - Get EntityFetcher for "car"
   ↓   - Fetch full car entity by target_id
   ↓   - Embed in link response

5. Response
   ← {
       "links": [
         {
           "id": "link-123",
           "source_id": "user-uuid",
           "target_id": "car-uuid",
           "target": { /* FULL car entity */ },
           "metadata": { /* link metadata */ }
         }
       ]
     }
```

**Key**: linked entities are included in the response. They are fetched one link at a time, not batched (see `src/links/handlers.rs`).

## 📦 Config Module

Loads and parses YAML configuration:

```
src/config/
└── mod.rs
    ├── LinksConfig    : Complete configuration
    └── EntityConfig   : Entity configuration
```

**YAML structure**:
```yaml
entities:
  - singular: order
    plural: orders
  - singular: invoice
    plural: invoices

links:
  - link_type: has_invoice
    source_type: order
    target_type: invoice
    forward_route_name: invoices
    reverse_route_name: order
    description: "Order has invoices"
    auth:
      create:
        policy: AllowOwner
        roles: ["admin", "user"]
```

## 🗄️ Storage Layer

```
src/storage/
├── in_memory.rs    ← In-memory implementation
│   └── InMemoryLinkService
│
└── dynamodb.rs     ← DynamoDB implementation
    ├── DynamoDbDataService<T>
    └── DynamoDbLinkService
```

Both implement the same traits:
- `DataService<T>` for entity CRUD
- `LinkService` for link CRUD

## 🏛️ Server Architecture

### ServerBuilder

Fluent API for building HTTP servers:

```rust
let app = ServerBuilder::new()
    .with_link_service(InMemoryLinkService::new())
    .register_module(billing_module)?
    .register_module(catalog_module)?
    .build()?;
```

**What it does**:
1. Collects all modules
2. Merges YAML configurations
3. Registers all entity descriptors
4. Builds entity CRUD routes
5. Builds generic link routes
6. Collects EntityFetchers
7. Collects EntityCreators
8. Creates AppState
9. Returns complete Axum Router

### EntityRegistry

Collects and builds entity routes:

```rust
pub struct EntityRegistry {
    descriptors: Vec<Box<dyn EntityDescriptor>>,
}

impl EntityRegistry {
    pub fn register(&mut self, descriptor: Box<dyn EntityDescriptor>);
    pub fn build_routes(&self) -> Router;
}
```

## 🎭 Key Design Patterns

### 1. Type Erasure

Core framework never knows concrete types:
```rust
// ✅ Framework uses trait objects
Arc<dyn EntityFetcher>
Arc<dyn EntityCreator>
Arc<dyn DataService<T>>

// ❌ Framework never does this
Arc<OrderStore>
Arc<UserService>
```

### 2. Dynamic Dispatch

Entities are fetched/created dynamically at runtime:
```rust
let fetcher = module.get_entity_fetcher("order")?;
let entity = fetcher.fetch_as_json(&order_id).await?;
```

### 3. Macro-Driven Code Generation

Eliminate boilerplate with compile-time generation:
```rust
// Input: 4 lines
impl_data_entity!(Order, "order", ["name"], {
    amount: f64,
});

// Output: 100+ lines of generated code
```

### 4. Configuration-Driven Behavior

Routes and validation defined in YAML:
```yaml
links:
  - link_type: owner
    source_type: user
    target_type: car
    forward_route_name: cars-owned
```

### 5. Dependency Injection

Modules provide services through trait methods:
```rust
impl Module for BillingModule {
    fn get_entity_fetcher(&self, entity_type: &str) -> Option<Arc<dyn EntityFetcher>> {
        match entity_type {
            "order" => Some(Arc::new(self.store.orders.clone())),
            _ => None,
        }
    }
}
```

## 🚀 Benefits

### For Framework Users

✅ **Zero Boilerplate**: Define entities in 4 lines  
✅ **Auto-Generated Routes**: No manual routing code  
✅ **Type Safety**: Full Rust compile-time checks  
✅ **Consistent Patterns**: Same structure everywhere  
✅ **Link Enrichment**: linked entities included in responses (one fetch per entity, not batched)  

### For Framework Developers

✅ **Extensibility**: Easy to add new storage backends  
✅ **Testability**: Trait-based design allows mocking  
✅ **Modularity**: Clear separation of concerns  
✅ **Maintainability**: Generic core never changes  

## 📚 Next Steps

- [Server Builder Implementation](SERVER_BUILDER_IMPLEMENTATION.md)
- [Routing Explanation](ROUTING_EXPLANATION.md)
- [Link Authorization](LINK_AUTH_IMPLEMENTATION.md)
- [Getting Started Guide](../guides/GETTING_STARTED.md)

---

**The architecture is designed for maximum productivity with zero compromise on type safety.** 🚀🦀✨
