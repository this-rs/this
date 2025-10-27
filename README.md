# This-RS 🦀

> A generic entity and relationship management framework for building RESTful APIs in Rust with **zero boilerplate**.

[![CI](https://github.com/this-rs/this/actions/workflows/ci.yml/badge.svg)](https://github.com/this-rs/this/actions/workflows/ci.yml)
[![Documentation](https://github.com/this-rs/this/actions/workflows/docs.yml/badge.svg)](https://github.com/this-rs/this/actions/workflows/docs.yml)
[![Crates.io](https://img.shields.io/crates/v/this-rs.svg)](https://crates.io/crates/this-rs)
[![docs.rs](https://docs.rs/this-rs/badge.svg)](https://docs.rs/this-rs)
[![License](https://img.shields.io/crates/l/this-rs.svg)](LICENSE-MIT)

---

## ✨ Highlights

- 🔌 **Generic Entity System** - Add entities without modifying framework code
- 🤖 **Auto-Generated Routes** - Declare a module, routes are created automatically
- ✅ **Automatic Validation & Filtering** - 🆕 Zero-boilerplate data validation with declarative rules
- 🔗 **Flexible Relationships** - Multiple link types between same entities
- ↔️ **Bidirectional Navigation** - Query relationships from both directions
- ✨ **Auto-Enriched Links** - Full entities in responses, no N+1 queries
- 🏗️ **Macro-Driven Entities** - Define entities with zero boilerplate using macros
- 🎯 **Smart Entity Creation** - Create new entities + links in one API call
- 📝 **Auto-Pluralization** - Smart plural forms (company → companies)
- ⚙️ **YAML Configuration** - Declarative entity and link definitions
- 🔒 **Type-Safe** - Full Rust compile-time guarantees

---

## 🎯 The Vision

### Traditional Framework (❌)
```rust
// Add new entity = Modify 10+ files
// - Update routing module (30+ lines)
// - Modify link handlers
// - Update entity registry
// - Write CRUD boilerplate
// - Maintain consistency manually
```

### This-RS (✅)
```rust
// Add new entity = Just use a macro!
impl_data_entity!(Product, "product", ["name", "sku"], {
    sku: String,
    price: f64,
    description: Option<String>,
});

// Main.rs stays unchanged!
let app = ServerBuilder::new()
    .register_module(module)?  // ← Everything auto-generated
    .build()?;
```

**Result**: Zero boilerplate, maximum productivity.

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

```rust
#[tokio::main]
async fn main() -> Result<()> {
    let app = ServerBuilder::new()
        .with_link_service(InMemoryLinkService::new())
        .register_module(CatalogModule::new(store))?
        .build()?;  // ← All routes created automatically!
    
    let listener = tokio::net::TcpListener::bind("0.0.0.0:3000").await?;
    axum::serve(listener, app).await?;
    Ok(())
}
```

**That's it!** Routes are auto-generated:
- ✅ `GET /products` - List all
- ✅ `POST /products` - Create new product
- ✅ `GET /products/:id` - Get by ID
- ✅ `PUT /products/:id` - Update product
- ✅ `DELETE /products/:id` - Delete product
- ✅ `GET /products/:id/links` - Introspection
- ✅ Link routes (if configured in YAML)

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

Complete billing microservice with **auto-generated routes**:

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

See [examples/microservice/README.md](examples/microservice/README.md) for full details.

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

1. **ServerBuilder** - Fluent API for building HTTP servers
2. **EntityDescriptor** - Describes how to generate routes for an entity
3. **EntityRegistry** - Collects and builds all entity routes
4. **Module** - Groups related entities with configuration
5. **LinkService** - Generic relationship management
6. **EntityFetcher** - Dynamically fetch entities for link enrichment
7. **EntityCreator** - Dynamically create entities with automatic linking

### Macro System

- `impl_data_entity!` - Generate a complete Data entity
- `impl_link_entity!` - Generate a custom Link entity
- `entity_fields!` - Inject base Entity fields
- `data_fields!` - Inject Entity + name fields
- `link_fields!` - Inject Entity + link fields

---

## 📖 Documentation

- **[Quick Start](docs/guides/QUICK_START.md)** - Fast introduction
- **[Getting Started](docs/guides/GETTING_STARTED.md)** - Step-by-step tutorial
- **[Validation & Filtering](docs/guides/VALIDATION_AND_FILTERING.md)** - 🆕 Automatic data validation
- **[Enriched Links](docs/guides/ENRICHED_LINKS.md)** - Auto-enrichment & performance
- **[Architecture](docs/architecture/ARCHITECTURE.md)** - Technical deep dive
- **[ServerBuilder](docs/architecture/SERVER_BUILDER_IMPLEMENTATION.md)** - Auto-routing details
- **[Full Documentation](docs/)** - Complete documentation index

---

## 🎁 Key Benefits

### For Developers

✅ **-88% less boilerplate** (340 → 40 lines in main.rs)  
✅ **Add entity in minutes** - Just one macro call  
✅ **Consistent patterns** - Same structure for all entities  
✅ **Type-safe** - Full Rust compile-time checks  
✅ **Scalable** - 3 or 300 entities = same simplicity  

### For Teams

✅ **Faster development** - Less code to write and maintain  
✅ **Easier onboarding** - Clear patterns and conventions  
✅ **Reduced errors** - Less manual work = fewer mistakes  
✅ **Better consistency** - Framework enforces best practices  

### For Production

✅ **Authorization** - Declarative auth policies  
✅ **Configurable** - YAML-based configuration  
✅ **Extensible** - Plugin architecture via modules  
✅ **Performance** - Efficient link enrichment with no N+1 queries  
✅ **Soft Deletes** - Built-in soft delete support  

---

## 🤝 Contributing

Contributions are welcome! Please feel free to submit a Pull Request.

---

## 📄 License

This project is licensed under the MIT License - see the [LICENSE-MIT](LICENSE-MIT) file for details.

---

## 🌟 Why This-RS?

> "The best code is the code you don't have to write."

This-RS eliminates boilerplate while maintaining type safety and flexibility. Perfect for:
- 🏢 Microservices architectures
- 🔌 REST APIs with complex relationships
- 🚀 Rapid prototyping
- 📊 Data-rich applications with interconnected entities

**Built with Rust. Designed for productivity. Ready for production.** 🦀✨

---

<p align="center">
  Made with ❤️ and 🦀 by the This-RS community
</p>
