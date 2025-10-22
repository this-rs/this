# This-RS 🦀

> A generic entity and relationship management framework for building RESTful APIs in Rust with **zero boilerplate**.

[![Crates.io](https://img.shields.io/crates/v/this-rs.svg)](https://crates.io/crates/this-rs)
[![Documentation](https://docs.rs/this-rs/badge.svg)](https://docs.rs/this-rs)
[![License](https://img.shields.io/crates/l/this-rs.svg)](LICENSE-MIT)

---

## ✨ Highlights

- 🔌 **Generic Entity System** - Add entities without modifying framework code
- 🤖 **Auto-Generated Routes** - Declare a module, routes are created automatically
- 🔗 **Flexible Relationships** - Multiple link types between same entities
- ↔️ **Bidirectional Navigation** - Query relationships from both directions
- ✨ **Auto-Enriched Links** - Full entities in responses, no N+1 queries
- 📝 **Auto-Pluralization** - Smart plural forms (company → companies)
- ⚙️ **YAML Configuration** - Declarative entity and link definitions
- 🏢 **Multi-Tenant** - Built-in tenant isolation
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
// Add new entity = 4 files, routes auto-generated
// 1. model.rs    - Data structure
// 2. store.rs    - Persistence
// 3. handlers.rs - Business logic
// 4. descriptor.rs - Route registration

// Main.rs stays unchanged!
let app = ServerBuilder::new()
    .register_module(module)?  // ← Everything auto-generated
    .build()?;
```

**Result**: Zero boilerplate, maximum productivity.

---

## 🚀 Quick Example

### 1. Define Your Entity

```rust
use this::prelude::*;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Product {
    id: Uuid,
    tenant_id: Uuid,
    name: String,
    price: f64,
}
```

### 2. Create Entity Descriptor

```rust
impl EntityDescriptor for ProductDescriptor {
    fn entity_type(&self) -> &str { "product" }
    fn plural(&self) -> &str { "products" }
    
    fn build_routes(&self) -> Router {
        Router::new()
            .route("/products", get(list).post(create))
            .route("/products/:id", get(get_by_id))
            .with_state(state)
    }
}
```

### 3. Register in Module

```rust
impl Module for MyModule {
    fn register_entities(&self, registry: &mut EntityRegistry) {
        registry.register(Box::new(ProductDescriptor::new(store)));
    }
}
```

### 4. Launch Server (Auto-Generated Routes!)

```rust
#[tokio::main]
async fn main() -> Result<()> {
    let app = ServerBuilder::new()
        .with_link_service(InMemoryLinkService::new())
        .register_module(MyModule::new(store))?
        .build()?;  // ← All routes created automatically!
    
    let listener = tokio::net::TcpListener::bind("0.0.0.0:3000").await?;
    axum::serve(listener, app).await?;
    Ok(())
}
```

**That's it!** Routes are auto-generated:
- ✅ `GET /products` - List all
- ✅ `POST /products` - Create
- ✅ `GET /products/:id` - Get by ID
- ✅ `GET /products/:id/links` - Introspection
- ✅ Link routes (if configured in YAML)

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

📚 All routes auto-generated:
  - GET    /orders, /invoices, /payments
  - POST   /orders, /invoices, /payments
  - GET    /orders/:id, /invoices/:id, /payments/:id
  - Link routes for relationships
```

See [examples/microservice/README.md](examples/microservice/README.md) for full details.

---

## 🏗️ Architecture

### Core Concepts

1. **ServerBuilder** - Fluent API for building HTTP servers
2. **EntityDescriptor** - Describes how to generate routes for an entity
3. **EntityRegistry** - Collects and builds all entity routes
4. **Module** - Groups related entities with configuration
5. **LinkService** - Generic relationship management

### Key Features

#### Auto-Generated CRUD Routes
```rust
// Just register your entities
module.register_entities(registry);

// Framework generates:
// GET    /{plural}
// POST   /{plural}
// GET    /{plural}/:id
```

#### Auto-Generated Link Routes
```yaml
# config/links.yaml
links:
  - link_type: has_invoice
    source_type: order
    target_type: invoice
    forward_route_name: invoices
    reverse_route_name: order
```

Framework generates:
- `GET /orders/:id/invoices` - Forward navigation
- `GET /invoices/:id/order` - Reverse navigation
- `POST /orders/:id/has_invoice/invoices/:invoice_id` - Create link
- `DELETE /orders/:id/has_invoice/invoices/:invoice_id` - Delete link

---

## 📖 Documentation

- **[Getting Started](docs/guides/GETTING_STARTED.md)** - Step-by-step tutorial
- **[Quick Start](docs/guides/QUICK_START.md)** - Fast introduction
- **[Enriched Links](docs/guides/ENRICHED_LINKS.md)** - Auto-enrichment & performance
- **[Architecture](docs/architecture/ARCHITECTURE.md)** - Technical deep dive
- **[ServerBuilder](docs/architecture/SERVER_BUILDER_IMPLEMENTATION.md)** - Auto-routing details
- **[Full Documentation](docs/)** - Complete documentation index

---

## 🎁 Key Benefits

### For Developers

✅ **-88% less boilerplate** (340 → 40 lines in main.rs)  
✅ **Add entity in minutes** - No routing changes needed  
✅ **Consistent patterns** - Same structure for all entities  
✅ **Type-safe** - Full Rust compile-time checks  
✅ **Scalable** - 3 or 300 entities = same simplicity  

### For Teams

✅ **Faster development** - Less code to write and maintain  
✅ **Easier onboarding** - Clear patterns and conventions  
✅ **Reduced errors** - Less manual work = fewer mistakes  
✅ **Better consistency** - Framework enforces best practices  

### For Production

✅ **Multi-tenant** - Built-in tenant isolation  
✅ **Authorization** - Declarative auth policies  
✅ **Configurable** - YAML-based configuration  
✅ **Extensible** - Plugin architecture via modules  

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
- 📊 Multi-tenant SaaS applications

**Built with Rust. Designed for productivity. Ready for production.** 🦀✨

---

<p align="center">
  Made with ❤️ and 🦀 by the This-RS community
</p>
