# this-rs 🦀

> A framework for building **complex multi-entity APIs** with **many relationships** in Rust.
> Four protocols, nine storage backends, cognitive intelligence — from one codebase.
>
> **Designed for APIs with 5+ entities and complex relationships.**  
> For simple CRUD APIs, consider using Axum directly.

[![CI](https://github.com/this-rs/this/actions/workflows/ci.yml/badge.svg)](https://github.com/this-rs/this/actions/workflows/ci.yml)
[![Documentation](https://github.com/this-rs/this/actions/workflows/docs.yml/badge.svg)](https://github.com/this-rs/this/actions/workflows/docs.yml)
[![Crates.io](https://img.shields.io/crates/v/this-rs.svg)](https://crates.io/crates/this-rs)
[![docs.rs](https://docs.rs/this-rs/badge.svg)](https://docs.rs/this-rs)
[![License](https://img.shields.io/crates/l/this-rs.svg)](LICENSE-MIT)
[![codecov](https://codecov.io/gh/this-rs/this/graph/badge.svg)](https://codecov.io/gh/this-rs/this)

**Website**: [this-rs.dev](https://this-rs.dev)

---

## ✨ Highlights

### 🚀 Core Features
- 🔌 **Generic Entity System** — Add entities without modifying framework code
- 🤖 **Auto-Generated Routes** — Declare a module, routes are created automatically
- 🏗️ **Macro-Driven Entities** — Define entities with zero boilerplate using macros
- 🔒 **Type-Safe** — Full Rust compile-time guarantees

### 🔗 Relationship Management
- 🔗 **Flexible Relationships** — Multiple link types between same entities
- ↔️ **Bidirectional Navigation** — Query relationships from both directions
- ✨ **Auto-Enriched Links** — Full entities in responses, no N+1 queries
- 🎯 **Smart Entity Creation** — Create new entities + links in one API call

### 🌐 Multi-Protocol Support
- 🌍 **REST API** — Traditional RESTful endpoints with auto-routing
- 📊 **GraphQL API** — Dynamic schema generation with full CRUD, relations, and subscriptions
- ⚡ **gRPC** — Auto-generated Protocol Buffers with EntityService, LinkService, EventService, and NotificationService
- 🔌 **WebSocket** — Real-time bidirectional communication with subscribe/unsubscribe filters
- 📡 **SSE** — Server-Sent Events for lightweight real-time streaming

### 💾 Storage Backends (9)
- **In-Memory** — Zero-config for development and testing
- **PostgreSQL** — Production-ready relational backend
- **MongoDB** — Document store backend
- **Neo4j** — Graph database for deep relationship traversals
- **ScyllaDB** — Distributed wide-column store (CQL compatible)
- **MySQL** — Traditional relational backend
- **DynamoDB** — AWS serverless storage
- **LMDB** — Embedded key-value store (zero-copy reads)
- **Obrain** — Cognitive graph database with hybrid intelligence

### 📣 Event System
- 🔔 **EventBus** — Built-in non-blocking broadcast on all CRUD operations
- 🔄 **Event Flows** — Declarative YAML pipelines with 8 operators (filter, map, batch, deduplicate, rate_limit, fan_out, resolve, deliver)
- 📬 **Notifications** — In-app NotificationStore with read/unread tracking across REST, GraphQL, and gRPC
- 🪝 **Webhook Sinks** — Deliver events to external webhooks with retry logic
- 📱 **Push Notifications** — Expo push notification delivery via Event Flows

### 🧠 Cognitive Signals
- 🚨 **AnomalyDetected** — Unusual patterns in data mutations (threshold-based)
- 🔄 **CoChangeDetected** — Entities that always change together (correlation tracking)
- 🕸️ **StigmergyLockIn** — Emergent behavioral lock-ins
- 🩹 **ScarCreated** — Failure patterns to avoid repeating
- 📖 **EpisodeLearned** — Learned behavioral sequences
- 🌉 **CognitiveNotificationBridge** — Subscribes to EventBus, evaluates thresholds, routes to SinkRegistry

### 🔐 Authentication & Authorization
- 🔑 **WAMI Auth STS** — Built-in JWT authentication with Ed25519 key pairs
- 🚀 **Bootstrap Mode** — Auto-generates keys at startup for zero-config development
- 🛡️ **RBAC Policies** — Per-entity authorization policies in YAML (authenticated, owner, admin_only, service_only)
- 🧩 **Custom Resolvers** — Reusable authorization logic referenced with `resolver:name` in YAML
- 🏢 **Multi-Tenant** — Opt-in tenant isolation via `impl_entity_multi_tenant!` macro
- 🗑️ **GDPR Erasure** — One-call tenant data deletion: `DELETE /tenants/:tenant_id/data`

### ⚡ Developer Experience
- ✅ **Automatic Validation & Filtering** — Zero-boilerplate data validation with declarative rules
- 📄 **Generic Pagination & Query Filtering** — Automatic pagination for all list endpoints
- 📝 **Auto-Pluralization** — Smart plural forms (company → companies)
- ⚙️ **YAML Configuration** — Declarative entity, link, event flow, auth, and cognitive definitions
- 🛠️ **CLI Scaffolding** — `this-cli` generates entire projects in seconds

---

## 🎯 Is this-rs Right for You?

### ✅ **Perfect Fit** - You Should Use this-rs

- **Many entities** (5+ entities with CRUD operations)
- **Complex relationships** (multiple link types between entities)
- **Bidirectional navigation** (need to query relationships from both directions)
- **Multi-protocol APIs** (want both REST and GraphQL from same codebase)
- **Microservices architecture** (building multiple interconnected services)
- **Rapid iteration** (adding entities frequently, need consistency)

**Example use cases**: CMS, ERP, e-commerce platforms, social networks, project management tools

### ⚠️ **Probably Overkill** - Consider Alternatives

- **Simple CRUD** (< 5 entities with basic operations)
- **No relationships** (entities are independent)
- **Single small API** (not planning to scale)
- **Learning Rust/Axum** (start with Axum directly, add this-rs later if needed)
- **Maximum performance critical** (framework adds small overhead)

**For simple projects, use [Axum](https://github.com/tokio-rs/axum) + [utoipa](https://github.com/juhaku/utoipa) directly.**

**📖 See [Alternatives Comparison](docs/ALTERNATIVES.md) for detailed analysis of when to use what.**

### 📊 ROI by Project Size

| Entities | Relationships | Recommended | Time Saved |
|----------|---------------|-------------|------------|
| 1-3 | Few | ❌ Axum directly | - |
| 3-5 | Some | ⚠️ Consider this-rs | ~20% |
| 5-10 | Many | ✅ this-rs recommended | ~40% |
| 10+ | Complex | ✅✅ this-rs highly recommended | ~60% |

---

## 💡 What this-rs Actually Saves

### Without this-rs (Pure Axum)
```rust
// For each entity, you write:
// 1. Entity definition (✓ same in both)
// 2. CRUD handlers (✓ same in both - you still write business logic)
// 3. Routes registration (❌ REPETITIVE - 30+ lines per entity)
// 4. Link routes (❌ REPETITIVE - 50+ lines per relationship)
// 5. Link enrichment (❌ MANUAL - N+1 queries if not careful)
// 6. GraphQL schema (❌ MANUAL - duplicate type definitions)

// Example: 10 entities with 15 relationships
// = ~500 lines of repetitive routing code
```

### With this-rs (✅)
```rust
// 1. Entity definition (✓ with macro helpers)
impl_data_entity!(Product, "product", ["name", "sku"], {
    sku: String,
    price: f64,
});

// 2. CRUD handlers (✓ you still write these - it's your business logic)
// 3. Routes registration (✅ AUTO-GENERATED)
// 4. Link routes (✅ AUTO-GENERATED from YAML)
// 5. Link enrichment (✅ AUTOMATIC - no N+1 queries)
// 6. GraphQL schema (✅ AUTO-GENERATED from entities)

// Main.rs for 10 entities with 15 relationships
let app = ServerBuilder::new()
    .register_module(module)?  // ← ~40 lines total
    .build()?;
```

**What you save**: Routing boilerplate, link management, GraphQL schema duplication.  
**What you still write**: Business logic handlers (as you should!).

---

## 🚀 Quick Example

### 1. Define Your Entity with Macros

```rust
use this::prelude::*;

// Macro generates full entity with all base fields
impl_data_entity!(Product, "product", ["name", "sku"], {
    sku: String,
    price: f64,
    description: Option<String>,
    stock: i32,
});

// Automatically includes:
// - id: Uuid (auto-generated)
// - type: String (auto-set to "product")
// - name: String (required)
// - created_at: DateTime<Utc> (auto-generated)
// - updated_at: DateTime<Utc> (auto-managed)
// - deleted_at: Option<DateTime<Utc>> (soft delete)
// - status: String (required)
```

### 2. Create Entity Store with EntityCreator

```rust
use this::prelude::*;

#[derive(Clone)]
pub struct ProductStore {
    data: Arc<RwLock<HashMap<Uuid, Product>>>,
}

// Implement EntityFetcher for link enrichment
#[async_trait]
impl EntityFetcher for ProductStore {
    async fn fetch_as_json(&self, entity_id: &Uuid) -> Result<serde_json::Value> {
        let product = self.get(entity_id)
            .ok_or_else(|| anyhow::anyhow!("Product not found"))?;
        Ok(serde_json::to_value(product)?)
    }
}

// Implement EntityCreator for automatic entity creation
#[async_trait]
impl EntityCreator for ProductStore {
    async fn create_from_json(&self, entity_data: serde_json::Value) -> Result<serde_json::Value> {
        let product = Product::new(
            entity_data["name"].as_str().unwrap_or("").to_string(),
            entity_data["status"].as_str().unwrap_or("active").to_string(),
            entity_data["sku"].as_str().unwrap_or("").to_string(),
            entity_data["price"].as_f64().unwrap_or(0.0),
            entity_data["description"].as_str().map(String::from),
            entity_data["stock"].as_i64().unwrap_or(0) as i32,
        );
        self.add(product.clone());
        Ok(serde_json::to_value(product)?)
    }
}
```

### 3. Create Module

```rust
impl Module for CatalogModule {
    fn name(&self) -> &str { "catalog-service" }
    fn entity_types(&self) -> Vec<&str> { vec!["product"] }
    
    fn links_config(&self) -> Result<LinksConfig> {
        LinksConfig::from_file("config/links.yaml")
    }
    
    fn register_entities(&self, registry: &mut EntityRegistry) {
        registry.register(Box::new(ProductDescriptor::new(self.store.clone())));
    }
    
    fn get_entity_fetcher(&self, entity_type: &str) -> Option<Arc<dyn EntityFetcher>> {
        match entity_type {
            "product" => Some(Arc::new(self.store.clone()) as Arc<dyn EntityFetcher>),
            _ => None,
        }
    }
    
    fn get_entity_creator(&self, entity_type: &str) -> Option<Arc<dyn EntityCreator>> {
        match entity_type {
            "product" => Some(Arc::new(self.store.clone()) as Arc<dyn EntityCreator>),
            _ => None,
        }
    }
}
```

### 4. Launch Server (Auto-Generated Routes!)

#### REST-only (simplest)
```rust
#[tokio::main]
async fn main() -> Result<()> {
    let app = ServerBuilder::new()
        .with_link_service(InMemoryLinkService::new())
        .register_module(CatalogModule::new(store))?
        .build()?;  // ← All REST routes created automatically!
    
    let listener = tokio::net::TcpListener::bind("0.0.0.0:3000").await?;
    axum::serve(listener, app).await?;
    Ok(())
}
```

#### Multi-Protocol with Events, Auth & Cognitive Signals
```rust
use this::prelude::*;

#[tokio::main]
async fn main() -> Result<()> {
    let host = ServerBuilder::new()
        .with_link_service(InMemoryLinkService::new())
        .register_module(CatalogModule::new(store))?
        .build_host()?                                          // transport-agnostic host
        .with_event_bus(EventBus::new(256))                     // event broadcasting
        .with_notification_store(InMemoryNotificationStore::new())
        .with_auth_config_file("config/auth.yaml")?;            // WAMI Auth STS

    let host = Arc::new(host);
    let app = Router::new()
        .merge(RestExposure::build_router(host.clone(), vec![])?)
        .merge(GraphQLExposure::build_router(host.clone())?)
        .merge(GrpcExposure::build_router(host.clone())?)
        .merge(WebSocketExposure::build_router(host)?);

    let listener = tokio::net::TcpListener::bind("0.0.0.0:3000").await?;
    axum::serve(listener, app).await?;
    Ok(())
}
```

**That's it!** Routes are auto-generated:

**REST API:**
- ✅ `GET /products` — List all (paginated)
- ✅ `POST /products` — Create new product
- ✅ `GET /products/:id` — Get by ID
- ✅ `PUT /products/:id` — Update product
- ✅ `DELETE /products/:id` — Delete product
- ✅ `GET /products/:id/links` — Introspection
- ✅ Link routes (auto-generated from YAML)

**GraphQL API:**
- ✅ `POST /graphql` — Full CRUD + subscriptions
- ✅ `GET /graphql/playground` — Interactive playground
- ✅ Auto-generated types, queries, mutations, and relations

**gRPC:**
- ✅ `EntityService` — List, Get, Create, Update, Delete
- ✅ `LinkService` — CRUD for relationships
- ✅ `EventService` — `StreamEvents` (server-streaming)
- ✅ `NotificationService` — GetNotifications, MarkAsRead
- ✅ `GET /grpc/proto` — Proto export for client generation

**WebSocket & SSE:**
- ✅ `/ws` — Subscribe/unsubscribe with entity_type/event_type filters
- ✅ `/events/stream` — SSE endpoint with query filters

**Auth (WAMI STS):**
- ✅ `POST /auth/token` — JWT token issuance
- ✅ `GET /auth/keys` — Public key endpoint (Ed25519)
- ✅ `POST /auth/refresh` — Token refresh
- ✅ `POST /auth/revoke` — Token revocation
- ✅ `DELETE /tenants/:tenant_id/data` — GDPR erasure

---

## 🔗 Advanced Link Features

### Two Ways to Create Links

#### 1. Link Existing Entities
```bash
# POST /orders/{order_id}/invoices/{invoice_id}
curl -X POST http://localhost:3000/orders/abc-123/invoices/inv-456 \
  -H 'Content-Type: application/json' \
  -d '{"metadata": {"priority": "high"}}'
```

#### 2. Create New Entity + Link Automatically
```bash
# POST /orders/{order_id}/invoices
curl -X POST http://localhost:3000/orders/abc-123/invoices \
  -H 'Content-Type: application/json' \
  -d '{
    "entity": {
      "number": "INV-999",
      "amount": 1500.00,
      "status": "active"
    },
    "metadata": {"priority": "high"}
  }'

# Response includes both created entity AND link!
{
  "entity": {
    "id": "inv-999-uuid",
    "type": "invoice",
    "name": "INV-999",
    "amount": 1500.00,
    ...
  },
  "link": {
    "id": "link-uuid",
    "source_id": "abc-123",
    "target_id": "inv-999-uuid",
    ...
  }
}
```

### Auto-Enriched Link Responses

When you query links, you automatically get full entity data:

```bash
# GET /orders/{id}/invoices
{
  "links": [
    {
      "id": "link-123",
      "source_id": "order-abc",
      "target_id": "invoice-xyz",
      "target": {
        "id": "invoice-xyz",
        "type": "invoice",
        "name": "INV-001",
        "amount": 1500.00,
        ...
      }
    }
  ]
}
```

**No N+1 queries!** Entities are fetched efficiently in the background.

---

## 📚 Examples

### [Microservice Example](examples/microservice/)

Complete billing microservice with **auto-generated routes** for both REST and GraphQL:

#### REST API
```bash
cargo run --example microservice
```

Output:
```
🚀 Starting billing-service v1.0.0
📦 Entities: ["order", "invoice", "payment"]
🌐 Server running on http://127.0.0.1:3000

📚 Entity Routes (CRUD - Auto-generated):
  GET    /orders                        - List all orders
  POST   /orders                        - Create a new order
  GET    /orders/{id}                   - Get a specific order
  PUT    /orders/{id}                   - Update an order
  DELETE /orders/{id}                   - Delete an order

🔗 Link Routes (Auto-generated):
  GET    /orders/{id}/invoices          - List invoices for an order
  POST   /orders/{id}/invoices          - Create new invoice + link automatically
  POST   /orders/{id}/invoices/{inv_id} - Link existing order & invoice
  PUT    /orders/{id}/invoices/{inv_id} - Update link metadata
  DELETE /orders/{id}/invoices/{inv_id} - Delete link
```

See [examples/microservice/README.md](examples/microservice/README.md) for full REST API details.

#### GraphQL API
```bash
cargo run --example microservice_graphql --features graphql
```

The same entities are exposed via GraphQL with:
- **Dynamic Schema Generation** - Types (`Order`, `Invoice`, `Payment`) auto-generated
- **Full CRUD** - Queries and mutations for all entities
- **Automatic Relations** - Navigate `order.invoices`, `invoice.payments` automatically
- **Interactive Playground** - Test queries at `http://127.0.0.1:3000/graphql/playground`

Example query:
```graphql
query {
  orders {
    id
    number
    customerName
    amount
    invoices {
      id
      number
      amount
      payments {
        id
        amount
        method
      }
    }
  }
}
```

See [examples/microservice/README_GRAPHQL.md](examples/microservice/README_GRAPHQL.md) for full GraphQL details.

---

## 🏗️ Architecture

### Entity Hierarchy

```
Entity (Base Trait)
├── id: Uuid
├── type: String
├── created_at: DateTime<Utc>
├── updated_at: DateTime<Utc>
├── deleted_at: Option<DateTime<Utc>>
└── status: String

    ├─► Data (Inherits Entity)
    │   └── name: String
    │       + indexed_fields()
    │       + field_value()
    │
    └─► Link (Inherits Entity)
        ├── source_id: Uuid
        ├── target_id: Uuid
        └── link_type: String
```

### Core Concepts

1. **ServerBuilder** — Fluent API for building HTTP servers
2. **ServerHost** — Transport-agnostic host holding all registries and services
3. **EntityDescriptor** — Describes how to generate routes for an entity
4. **EntityRegistry** — Collects and builds all entity routes
5. **Module** — Groups related entities with configuration
6. **LinkService** — Generic relationship management (9 backend implementations)
7. **EntityFetcher** — Dynamically fetch entities for link enrichment
8. **EntityCreator** — Dynamically create entities with automatic linking
9. **EventBus** — Non-blocking broadcast channel for entity lifecycle events
10. **Event Flows** — Declarative YAML pipelines (filter → map → batch → deduplicate → deliver)
11. **CognitiveNotificationBridge** — Monitors EventBus for anomalies, co-changes, scars, stigmergy, episodes
12. **SinkRegistry** — Routes signals to webhooks, in-app notifications, push, SSE
13. **Exposures** — Protocol adapters: RestExposure, GraphQLExposure, GrpcExposure, WebSocketExposure

### Macro System

- `impl_data_entity!` - Generate a complete Data entity
- `impl_link_entity!` - Generate a custom Link entity
- `entity_fields!` - Inject base Entity fields
- `data_fields!` - Inject Entity + name fields
- `link_fields!` - Inject Entity + link fields

---

## 📖 Documentation

### 🚀 Getting Started
- **[Quick Start](docs/guides/QUICK_START.md)** - Fast introduction
- **[Getting Started](docs/guides/GETTING_STARTED.md)** - Step-by-step tutorial

### 🌐 API Exposure
- **[GraphQL Guide](docs/guides/GRAPHQL.md)** - 🆕 Dynamic GraphQL API with auto-generated schema
- **[Custom Routes](docs/guides/CUSTOM_ROUTES.md)** - Adding custom endpoints alongside auto-routes

### 🔗 Features
- **[Validation & Filtering](docs/guides/VALIDATION_AND_FILTERING.md)** - Automatic data validation
- **[Pagination & Filtering](docs/guides/PAGINATION_AND_FILTERING.md)** - Generic pagination and query filtering
- **[Enriched Links](docs/guides/ENRICHED_LINKS.md)** - Auto-enrichment & performance
- **[Link Authorization](docs/guides/LINK_AUTHORIZATION.md)** - Securing relationships

### 🏗️ Architecture
- **[Architecture](docs/architecture/ARCHITECTURE.md)** - Technical deep dive
- **[ServerBuilder](docs/architecture/SERVER_BUILDER_IMPLEMENTATION.md)** - Auto-routing details
- **[GraphQL Implementation](docs/architecture/GRAPHQL_IMPLEMENTATION.md)** - 🆕 Custom executor design
- **[Full Documentation](docs/)** - Complete documentation index

---

## 🎁 Key Benefits

### For Developers

✅ **-88% less routing boilerplate** (340 → 40 lines in microservice example)  
✅ **Add entity quickly** - Macro helpers + module registration  
✅ **Consistent patterns** - Same structure for all entities  
✅ **Type-safe** - Full Rust compile-time checks  
✅ **Scales well** - Adding the 10th entity is as easy as the 1st  
✅ **Multi-protocol** - 🆕 Same entities exposed via REST and GraphQL  
⚠️ **Learning curve** - Framework abstractions to understand (traits, registry)  

### For Teams

✅ **Faster development** - Less code to write and maintain  
✅ **Easier onboarding** - Clear patterns and conventions  
✅ **Reduced errors** - Less manual work = fewer mistakes  
✅ **Better consistency** - Framework enforces best practices  
✅ **Flexible APIs** - 🆕 Choose REST, GraphQL, or both  

### For Production

✅ **WAMI Auth STS** — JWT authentication with Ed25519, bootstrap mode, RBAC policies  
✅ **Custom Auth Resolvers** — Reusable authorization closures (`resolver:name` in YAML)  
✅ **Multi-Tenant** — Opt-in tenant isolation per entity  
✅ **GDPR Erasure** — One-call cascade delete across entities and links  
✅ **9 Storage Backends** — In-Memory, PostgreSQL, MongoDB, Neo4j, ScyllaDB, MySQL, DynamoDB, LMDB, Obrain  
✅ **Cognitive Signals** — Anomaly detection, co-change tracking, threshold-based routing  
✅ **Event Flows** — Declarative pipelines with webhooks, push notifications, and SSE  
✅ **Notifications** — In-app NotificationStore with read/unread tracking  
✅ **Soft Deletes** — Built-in soft delete support  
✅ **Dynamic Schema** — GraphQL schema auto-generated from entities  

---

## 🤝 Contributing

Contributions are welcome! Please feel free to submit a Pull Request.

---

## 📄 License

This project is licensed under the MIT License - see the [LICENSE-MIT](LICENSE-MIT) file for details.

---

## 🌟 Why this-rs?

> "The best code is the code you don't have to write... *if you're writing it 50 times.*"

this-rs eliminates **repetitive routing and relationship boilerplate** while maintaining type safety. 

**Perfect for**:
- 🏢 **Microservices architectures** with many entities
- 🔗 **Complex relationship graphs** (many-to-many, bidirectional)
- 🔮 **Four protocols from one codebase** (REST + GraphQL + gRPC + WebSocket)
- 🧠 **Cognitive APIs** with anomaly detection, co-change tracking, and intelligent signals
- 🔐 **Secure multi-tenant platforms** with WAMI Auth, RBAC, and GDPR compliance
- 🚀 **Rapidly evolving domains** (adding entities frequently)
- 📊 **Hybrid backends** (PostgreSQL for CRUD + Neo4j/Obrain for graph traversals)

**NOT ideal for**:
- ❌ Simple CRUD APIs (< 5 entities)
- ❌ Maximum performance critical paths (framework adds overhead)
- ❌ Learning projects (start with Axum first)

### 🆕 What's New in v0.0.9

- 🔔 **EventBus** — Built-in event system with non-blocking broadcast on all CRUD operations
- 🔄 **Event Flows** — Declarative YAML pipelines: filter → map → batch → deduplicate → deliver
- 📬 **Notifications** — In-app NotificationStore with read/unread tracking (REST + GraphQL + gRPC)
- 🪝 **Webhook & Push Sinks** — Deliver events externally via webhooks or Expo push notifications
- 📡 **SSE Streaming** — `GET /events/stream` for lightweight real-time event streaming
- 🧠 **Cognitive Signals** — CognitiveNotificationBridge detects anomalies, co-changes, scars, stigmergy, episodes
- 🔐 **WAMI Auth STS** — JWT authentication with Ed25519 keys, bootstrap mode, per-entity RBAC policies
- 🧩 **Custom Auth Resolvers** — Reusable authorization closures referenced with `resolver:name` in YAML
- 🗑️ **GDPR Erasure** — `DELETE /tenants/:tenant_id/data` cascades across all entities and links
- 💾 **3 New Backends** — MySQL, LMDB, and Obrain storage backends (total: 9)
- ⚡ **gRPC** — Full gRPC support with EntityService, LinkService, EventService, NotificationService
- 🔌 **WebSocket** — Real-time bidirectional events with subscribe/unsubscribe filters

---

## 🤔 Honest Trade-offs

### What this-rs Adds ✅
- Auto-generated routing for entities and links (REST + GraphQL + gRPC + WebSocket)
- Bidirectional relationship navigation with auto-enrichment
- 9 storage backends — same API, any database
- EventBus + Event Flows with declarative YAML pipelines
- Cognitive signals (anomaly detection, co-changes, stigmergy, scars, episodes)
- WAMI Auth STS with JWT, RBAC, custom resolvers, multi-tenant, GDPR erasure
- Notifications (in-app + webhook + push + SSE)
- YAML-driven configuration for everything

### What You Still Write ✍️
- Entity definitions (with macro helpers)
- Business logic handlers (create, update, delete, custom queries)
- Validation rules (with `Validated<T>` extractor)
- Custom auth resolvers (closures or trait impls)

### The Cost ⚠️
- Learning curve (framework patterns and traits)
- Some abstraction overhead (dynamic dispatch, registry lookups)
- YAML configuration to maintain
- Smaller ecosystem than pure Axum

**Built with Rust. Four protocols. Nine backends. Cognitive intelligence. One codebase.** 🦀✨

---

<p align="center">
  Made with ❤️ and 🦀 by the this-rs community
</p>
