# This-RS 🚀

A generic entity and relationship management framework for building RESTful APIs in Rust.

[![Crates.io](https://img.shields.io/crates/v/this-rs.svg)](https://crates.io/crates/this-rs)
[![Documentation](https://docs.rs/this-rs/badge.svg)](https://docs.rs/this-rs)
[![License](https://img.shields.io/crates/l/this-rs.svg)](LICENSE)

## ✨ Features

- **🔌 Generic Entity System**: Define new entities without modifying framework code
- **🔗 Flexible Relationships**: Support multiple link types between same entities
- **↔️ Bidirectional Navigation**: Query relationships from both directions
- **📝 Auto-Pluralization**: Intelligent plural forms (company → companies)
- **⚙️ Configuration-Based**: Define relationships via YAML
- **🏢 Multi-tenant Support**: Built-in tenant isolation
- **🔒 Type-Safe**: Leverage Rust's type system for compile-time guarantees
- **🚀 Zero Boilerplate**: Macros generate repetitive code automatically

## 🎯 Philosophy

**The Problem**: In traditional frameworks, adding a new entity requires:
- Modifying link/relationship modules
- Updating route definitions
- Writing repetitive CRUD code
- Maintaining consistency across modules

**The Solution**: This-RS uses:
- **String-based polymorphism** for entity and link types
- **YAML configuration** for relationship definitions
- **Generic traits** that work with any entity type
- **Macros** to eliminate boilerplate

**Result**: Add a new entity in ~15 lines of code, without touching existing modules.

## 🚀 Quick Start

### Installation

Add to your `Cargo.toml`:

```toml
[dependencies]
this-rs = "0.1"
tokio = { version = "1", features = ["full"] }
serde = { version = "1", features = ["derive"] }
uuid = { version = "1", features = ["v4", "serde"] }
```

### Define Your Entities

```rust
use this::prelude::*;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct User {
    id: Uuid,
    tenant_id: Uuid,
    name: String,
    email: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Car {
    id: Uuid,
    tenant_id: Uuid,
    brand: String,
    model: String,
}

// Implement Data trait with a single macro call
impl_data_entity!(User, "user", ["name", "email"]);
impl_data_entity!(Car, "car", ["brand", "model"]);
```

### Configure Relationships

Create `links.yaml`:

```yaml
entities:
  - singular: user
    plural: users
  - singular: car
    plural: cars

links:
  # Users can own cars
  - link_type: owner
    source_type: user
    target_type: car
    forward_route_name: cars-owned      # GET /users/{id}/cars-owned
    reverse_route_name: users-owners    # GET /cars/{id}/users-owners
    description: "User owns a car"
  
  # Users can drive cars (different relationship!)
  - link_type: driver
    source_type: user
    target_type: car
    forward_route_name: cars-driven     # GET /users/{id}/cars-driven
    reverse_route_name: users-drivers   # GET /cars/{id}/users-drivers
    description: "User drives a car"
```

### Use the Framework

```rust
use this::prelude::*;

#[tokio::main]
async fn main() -> Result<()> {
    // Load configuration
    let config = LinksConfig::from_yaml_file("links.yaml")?;
    let registry = Arc::new(LinkRouteRegistry::new(Arc::new(config)));
    
    // Create services
    let link_service = Arc::new(InMemoryLinkService::new());
    
    let tenant_id = Uuid::new_v4();
    let user_id = Uuid::new_v4();
    let car_id = Uuid::new_v4();
    
    // User owns a car
    link_service.create(
        &tenant_id,
        "owner",
        EntityReference::new(user_id, "user"),
        EntityReference::new(car_id, "car"),
        None,
    ).await?;
    
    // User also drives the same car
    link_service.create(
        &tenant_id,
        "driver",
        EntityReference::new(user_id, "user"),
        EntityReference::new(car_id, "car"),
        None,
    ).await?;
    
    // Query all cars owned by user
    let owned_cars = link_service.find_by_source(
        &tenant_id,
        &user_id,
        "user",
        Some("owner"),
        Some("car"),
    ).await?;
    
    println!("User owns {} cars", owned_cars.len());
    
    Ok(())
}
```

## 🏗️ Architecture

```
this-rs/
├── core/           # Generic framework code
│   ├── entity.rs   # Entity and Data traits
│   ├── link.rs     # Link structures
│   ├── field.rs    # Field validation
│   └── service.rs  # Service traits
├── links/          # Link management (agnostic)
│   ├── service.rs  # LinkService implementation
│   └── registry.rs # Route resolution
├── entities/       # Entity-specific code
│   └── macros.rs   # Boilerplate generation
└── config/         # Configuration loading
    └── mod.rs
```

## 📖 Key Concepts

### Entities

Two types of entities:

1. **Data Entities**: Concrete domain objects (User, Car, Company)
   - Have unique IDs
   - Belong to a tenant
   - Have searchable fields

2. **Link Entities**: Relationships between Data entities
   - Completely polymorphic (work with any entity types)
   - Support metadata
   - Bidirectional navigation

### Multiple Relationships

The same entity types can have multiple relationship types:

```yaml
# User ↔ Car can be both owner AND driver
links:
  - link_type: owner
    source_type: user
    target_type: car
    # ...
  
  - link_type: driver
    source_type: user
    target_type: car
    # ...
```

This generates distinct routes:
- `/users/{id}/cars-owned` - cars owned by user
- `/users/{id}/cars-driven` - cars driven by user
- `/cars/{id}/users-owners` - owners of car
- `/cars/{id}/users-drivers` - drivers of car

### Tenant Isolation

All operations are tenant-scoped:

```rust
// Each request includes tenant_id
link_service.create(&tenant_id, ...);
link_service.find_by_source(&tenant_id, ...);
```

Tenants cannot access each other's data.

## 🎯 Design Goals

### ✅ DO

- Be completely entity-agnostic in the link module
- Support any combination of source/target types
- Support multiple link types between same entities
- Handle irregular plurals (company → companies)
- Provide bidirectional navigation
- Validate via configuration (not hardcoding)

### ❌ DON'T

- Use enums for EntityType or LinkType
- Hardcode entity type validations
- Duplicate CRUD handler code
- Require modifying `links/` when adding entities
- Manage plurals naively (just adding 's')

## 📚 Examples

See the `examples/` directory:

- `simple_api.rs` - Basic entity and link usage
- `multi_entity.rs` - Multiple entities with complex relationships

Run examples:

```bash
cargo run --example simple_api
```

## 🧪 Testing

```bash
# Run all tests
cargo test

# Run with output
cargo test -- --nocapture

# Run specific test
cargo test test_pluralize
```

## 🤝 Contributing

Contributions are welcome! Please:

1. Fork the repository
2. Create a feature branch
3. Add tests for new functionality
4. Ensure all tests pass
5. Submit a pull request

## 📄 License

Licensed under either of:

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT license ([LICENSE-MIT](LICENSE-MIT))

at your option.

## 🌟 Roadmap

- [ ] PostgreSQL LinkService implementation
- [ ] Axum HTTP handlers generation
- [ ] GraphQL support
- [ ] Link validation rules
- [ ] Cascade delete options
- [ ] Link versioning/history
- [ ] Performance benchmarks

## 💡 Inspiration

This framework was inspired by the need for:
- Domain-driven design in Rust
- Flexible relationship modeling
- Rapid prototyping without boilerplate
- Type-safe yet generic systems

---

Built with ❤️ in Rust
