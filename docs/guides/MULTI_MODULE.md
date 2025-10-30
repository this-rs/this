# Multi-Module Configuration

This guide explains how to use multiple modules in a single this-rs application and how configuration merging works.

## Overview

this-rs supports registering multiple modules in a single application. Each module can define its own entities, links, and validation rules. The framework automatically merges all configurations intelligently.

## Basic Multi-Module Setup

```rust
use this::prelude::*;

#[tokio::main]
async fn main() -> Result<()> {
    let app = ServerBuilder::new()
        .with_link_service(InMemoryLinkService::new())
        .register_module(CatalogModule::new())?   // Module 1
        .register_module(OrderModule::new())?     // Module 2
        .register_module(BillingModule::new())?   // Module 3
        .build()?;
    
    let listener = tokio::net::TcpListener::bind("0.0.0.0:3000").await?;
    axum::serve(listener, app).await?;
    Ok(())
}
```

## Configuration Merging Rules

When multiple modules are registered, their configurations are merged according to these rules:

### 1. Entities

**Rule**: Entities are combined from all modules. If multiple modules define the same entity (by `singular` name), **the last definition wins**.

**Example**:

```yaml
# Module 1 (catalog-service)
entities:
  - singular: product
    plural: products
    auth:
      list: public

# Module 2 (inventory-service)
entities:
  - singular: product
    plural: products
    auth:
      list: authenticated  # ← This wins!
```

**Result**: Only one `product` entity with `list: authenticated`.

**Use case**: This allows modules to override entity configurations from other modules when needed.

### 2. Links

**Rule**: Links are combined from all modules. If multiple modules define the same link (by `link_type` + `source_type` + `target_type`), **the last definition wins**.

**Example**:

```yaml
# Module 1
links:
  - link_type: contains
    source_type: order
    target_type: product
    forward_route_name: products
    description: "Order contains products (v1)"

# Module 2
links:
  - link_type: contains
    source_type: order
    target_type: product
    forward_route_name: items  # ← Different route name
    description: "Order contains products (v2)"
```

**Result**: Only one `contains` link with route name `items` and description "v2".

**Use case**: This allows fine-tuning link behavior in specialized modules.

### 3. Validation Rules

**Rule**: Validation rules are **combined** for the same `link_type`. Rules from all modules are merged together.

**Example**:

```yaml
# Module 1
validation_rules:
  works_at:
    - source: user
      targets: [company]

# Module 2
validation_rules:
  works_at:
    - source: user
      targets: [project]
```

**Result**: The `works_at` link type accepts both `user -> company` and `user -> project`.

**Use case**: This allows modules to extend validation rules rather than replacing them.

## Practical Example: E-commerce System

Let's build an e-commerce system with 3 modules:

### Module 1: Catalog Service

```yaml
# config/catalog-links.yaml
entities:
  - singular: product
    plural: products
  - singular: category
    plural: categories

links:
  - link_type: belongs_to
    source_type: product
    target_type: category
    forward_route_name: category
    reverse_route_name: products
```

```rust
// catalog/module.rs
pub struct CatalogModule {
    products: Arc<ProductStore>,
    categories: Arc<CategoryStore>,
}

impl Module for CatalogModule {
    fn name(&self) -> &str { "catalog-service" }
    
    fn entity_types(&self) -> Vec<&str> {
        vec!["product", "category"]
    }
    
    fn links_config(&self) -> Result<LinksConfig> {
        LinksConfig::from_yaml_file("config/catalog-links.yaml")
    }
    
    // ... register_entities, get_entity_fetcher, etc.
}
```

### Module 2: Order Service

```yaml
# config/order-links.yaml
entities:
  - singular: order
    plural: orders
  - singular: customer
    plural: customers

links:
  - link_type: contains
    source_type: order
    target_type: product
    forward_route_name: products
    reverse_route_name: orders
  
  - link_type: placed_by
    source_type: order
    target_type: customer
    forward_route_name: customer
    reverse_route_name: orders
```

```rust
// order/module.rs
impl Module for OrderModule {
    fn name(&self) -> &str { "order-service" }
    
    fn entity_types(&self) -> Vec<&str> {
        vec!["order", "customer"]
    }
    
    fn links_config(&self) -> Result<LinksConfig> {
        LinksConfig::from_yaml_file("config/order-links.yaml")
    }
    
    // ...
}
```

### Module 3: Billing Service

```yaml
# config/billing-links.yaml
entities:
  - singular: invoice
    plural: invoices
  - singular: payment
    plural: payments

links:
  - link_type: generates
    source_type: order
    target_type: invoice
    forward_route_name: invoices
    reverse_route_name: order
  
  - link_type: settles
    source_type: payment
    target_type: invoice
    forward_route_name: invoice
    reverse_route_name: payments
```

```rust
// billing/module.rs
impl Module for BillingModule {
    fn name(&self) -> &str { "billing-service" }
    
    fn entity_types(&self) -> Vec<&str> {
        vec!["invoice", "payment"]
    }
    
    fn links_config(&self) -> Result<LinksConfig> {
        LinksConfig::from_yaml_file("config/billing-links.yaml")
    }
    
    // ...
}
```

### Main Application

```rust
#[tokio::main]
async fn main() -> Result<()> {
    let app = ServerBuilder::new()
        .with_link_service(InMemoryLinkService::new())
        .register_module(CatalogModule::new())?
        .register_module(OrderModule::new())?
        .register_module(BillingModule::new())?
        .build()?;
    
    let listener = tokio::net::TcpListener::bind("0.0.0.0:3000").await?;
    axum::serve(listener, app).await?;
    Ok(())
}
```

### Resulting Configuration

After merging, the application will have:

**Entities**: `product`, `category`, `order`, `customer`, `invoice`, `payment` (6 total)

**Links**:
- `product -> category` (belongs_to)
- `order -> product` (contains)
- `order -> customer` (placed_by)
- `order -> invoice` (generates)
- `payment -> invoice` (settles)

**Routes** (auto-generated):
```
GET    /products
POST   /products
GET    /products/{id}
GET    /products/{id}/category           # From catalog module
GET    /products/{id}/orders              # From order module

GET    /orders
POST   /orders
GET    /orders/{id}
GET    /orders/{id}/products              # From order module
GET    /orders/{id}/customer              # From order module
GET    /orders/{id}/invoices              # From billing module

GET    /invoices
GET    /invoices/{id}
GET    /invoices/{id}/order               # Reverse link
GET    /invoices/{id}/payments            # Reverse link

... and so on for all entities
```

## Best Practices

### 1. Module Independence

Each module should be as independent as possible:

```rust
// ✅ Good: Module owns its entities
impl Module for CatalogModule {
    fn entity_types(&self) -> Vec<&str> {
        vec!["product", "category"]  // Only entities owned by this module
    }
}

// ❌ Bad: Module references entities from other modules
impl Module for CatalogModule {
    fn entity_types(&self) -> Vec<&str> {
        vec!["product", "category", "order"]  // Order is not in catalog!
    }
}
```

### 2. Cross-Module Links

It's okay for links to cross module boundaries:

```yaml
# billing/config.yaml
links:
  - link_type: generates
    source_type: order        # From order-service
    target_type: invoice      # From billing-service
    forward_route_name: invoices
    reverse_route_name: order
```

The framework will automatically resolve these cross-module relationships as long as both modules are registered.

### 3. Entity Overrides

If you need to override an entity from another module, register your module **last**:

```rust
let app = ServerBuilder::new()
    .with_link_service(service)
    .register_module(BaseModule)?      // Defines product
    .register_module(CustomModule)?    // Overrides product ← wins!
    .build()?;
```

### 4. Validation Rule Extension

Use validation rules to restrict cross-module links:

```yaml
# Base module
validation_rules:
  works_at:
    - source: user
      targets: [company]

# Extension module
validation_rules:
  works_at:
    - source: user
      targets: [project, organization]  # Adds more targets
```

Result: `user` can link to `company`, `project`, or `organization`.

## Testing Multi-Module Configurations

Test your merged configuration:

```rust
#[test]
fn test_multi_module_config() {
    let catalog_config = CatalogModule::new().links_config().unwrap();
    let order_config = OrderModule::new().links_config().unwrap();
    
    let merged = LinksConfig::merge(vec![catalog_config, order_config]);
    
    // Verify expected entities
    assert_eq!(merged.entities.len(), 4);
    
    // Verify cross-module links work
    let link = merged.find_link_definition("contains", "order", "product");
    assert!(link.is_some());
}
```

## Troubleshooting

### Problem: Entity not found

**Symptom**: `Entity type 'product' not found`

**Solution**: Make sure the module defining `product` is registered:

```rust
.register_module(CatalogModule::new())?  // Defines product
```

### Problem: Link not working

**Symptom**: `Link type 'contains' not allowed for order -> product`

**Solution**: Check validation rules. Either remove them or add the combination:

```yaml
validation_rules:
  contains:
    - source: order
      targets: [product]
```

### Problem: Unexpected entity behavior

**Symptom**: Entity has wrong auth config

**Solution**: Check module registration order. Last module wins:

```rust
.register_module(Module1)?  // Defines product with auth: public
.register_module(Module2)?  // Overrides product with auth: authenticated ← used
```

## See Also

- [Getting Started](GETTING_STARTED.md)
- [Link Configuration](ENRICHED_LINKS.md)
- [Architecture Overview](../architecture/ARCHITECTURE.md)
