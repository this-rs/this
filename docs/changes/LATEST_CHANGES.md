# 🎯 Latest Changes

## v0.0.7 - GraphQL API Exposure (Latest)

### Summary

Major new feature introducing **dynamic GraphQL API exposure** with automatic schema generation from registered entities.

**Date**: January 2025  
**Version**: 0.0.7  
**Impact**: New feature - No breaking changes

### New Features

- ✅ **Dynamic GraphQL Schema** - Automatically generated from entity definitions
- ✅ **Specific Entity Types** - Each entity gets its own GraphQL type (`Order`, `Invoice`, etc.)
- ✅ **Automatic Relations** - Relations discovered from `links.yaml` configuration
- ✅ **Full CRUD Operations** - Create, Read, Update, Delete via GraphQL mutations
- ✅ **List Queries with Pagination** - `orders`, `invoices`, `payments` queries with limit/offset
- ✅ **Single Entity Queries** - `order(id)`, `invoice(id)`, `payment(id)` queries
- ✅ **Nested Relations** - Query entities with their relations recursively
- ✅ **Specialized Link Mutations** - `createInvoiceForOrder`, `linkPaymentToInvoice`, `unlinkPaymentFromInvoice`
- ✅ **Generic Link Mutations** - `createLink`, `deleteLink` for flexible link management
- ✅ **GraphQL Playground** - Interactive query interface at `/graphql/playground`
- ✅ **SDL Schema Export** - Download complete schema at `/graphql/schema`
- ✅ **Custom Executor** - Runtime query execution with dynamic field resolution
- ✅ **Modular Architecture** - Clean separation with executor sub-modules

### Implementation

The GraphQL exposure is built with:

- **Custom Executor**: Parses and executes queries at runtime (not compile-time)
- **Schema Generator**: Dynamically creates SDL from entities and links configuration
- **Field Resolver**: Recursively resolves fields and relations
- **Modular Design**: Separated into 6 executor modules for maintainability

### Usage

Enable GraphQL feature:

```toml
[dependencies]
this-rs = { version = "0.0.7", features = ["graphql"] }
```

Build server with GraphQL:

```rust
let host = Arc::new(
    ServerBuilder::new()
        .with_link_service(InMemoryLinkService::new())
        .register_module(module)?
        .build_host()?
);

let graphql_router = GraphQLExposure::build_router(host)?;
```

### Documentation

- ✅ [GraphQL Guide](../guides/GRAPHQL.md) - Complete user guide
- ✅ [GraphQL Implementation](../architecture/GRAPHQL_IMPLEMENTATION.md) - Technical details
- ✅ [GraphQL Example](../../examples/microservice/README_GRAPHQL.md) - Complete example

### Example Queries

**List entities:**
```graphql
query {
  orders(limit: 10) {
    id
    number
    customerName
    amount
  }
}
```

**Get with relations:**
```graphql
query {
  order(id: "UUID") {
    id
    number
    invoices {
      id
      amount
      payments {
        id
        method
      }
    }
  }
}
```

**Create entity:**
```graphql
mutation {
  createOrder(data: {
    number: "ORD-001"
    customerName: "John Doe"
    amount: 1000.0
    status: "active"
  }) {
    id
    number
  }
}
```

### Next Steps

- [ ] GraphQL Subscriptions (WebSocket support)
- [ ] Advanced filtering in queries
- [ ] Schema caching for performance
- [ ] Field-level authorization
- [ ] GraphQL directive support

---

## v0.0.6 - Generic Pagination and Filtering

### Summary

New feature introducing **generic pagination and filtering** for all list endpoints (entities and links), with automatic pagination by default to prevent returning all data at once.

**Date**: December 2024  
**Version**: 0.0.6  
**Impact**: Enhancement - Default pagination applied

### New Features

- ✅ **Generic Pagination** - Automatic pagination for all list endpoints (entities and links)
- ✅ **Default Pagination** - Always active (page=1, limit=20) to prevent overwhelming responses
- ✅ **Generic Filtering** - Query parameter filtering on entity and link fields
- ✅ **Nested Field Filtering** - Filter on nested entity fields (`target.status`, `source.name`, etc.)
- ✅ **Filter on Links** - Filter link lists based on properties of linked entities
- ✅ **Unified Response Format** - Consistent `PaginatedResponse<T>` structure
- ✅ **QueryParams** - Standardized query parameter handling
- ✅ **Works on All Link Levels** - Pagination and filtering for 2+ level routes

### Migration Guide

No breaking changes! Pagination is now automatic:

```rust
// Before - would return ALL entities
GET /orders

// After - automatically paginated
GET /orders
Response: { "data": [...], "pagination": { "page": 1, "limit": 20, ... } }

// You can still get all data with explicit pagination
GET /orders?limit=1000
```

### Documentation

- ✅ [Pagination and Filtering Guide](../guides/PAGINATION_AND_FILTERING.md) - Complete guide
- ✅ Updated examples to show pagination usage

### Example Usage

```bash
# List entities with default pagination (20 per page)
curl 'http://127.0.0.1:3000/orders'

# Custom pagination
curl 'http://127.0.0.1:3000/orders?page=2&limit=10'

# With filters
curl 'http://127.0.0.1:3000/orders?filter={"status":"pending"}'

# Links with pagination
curl 'http://127.0.0.1:3000/orders/{id}/invoices?page=1&limit=5'

# Links with filters on nested entities
curl 'http://127.0.0.1:3000/orders/{id}/invoices?filter={"target.status":"paid"}'

# Nested links (3+ levels) with pagination
curl 'http://127.0.0.1:3000/orders/{id}/invoices/{id}/payments?page=1&limit=10'
```

---

## v0.0.5 - Automatic Validation and Filtering

### Summary

Major new feature introducing **automatic validation and filtering** with declarative rules defined directly in entity models.

**Date**: December 2024  
**Version**: 0.0.5  
**Impact**: New feature - No breaking changes

### New Features

- ✅ **Automatic Validation** - Declarative validation rules in entity definitions
- ✅ **Data Filtering** - Automatic data transformation (trim, lowercase, uppercase, round)
- ✅ **`impl_data_entity_validated!` Macro** - Extended entity macro with validation support
- ✅ **`Validated<T>` Extractor** - Axum extractor for automatic validation
- ✅ **Reusable Validators** - `required`, `optional`, `positive`, `string_length`, `max_value`, `in_list`, `date_format`
- ✅ **Reusable Filters** - `trim`, `uppercase`, `lowercase`, `round_decimals`
- ✅ **Operation-Specific Rules** - Different validation/filter rules for create vs update

### Migration Guide

If you're using `impl_data_entity!`, you can continue using it. However, we recommend upgrading to `impl_data_entity_validated!`:

```rust
// Old way (still works)
impl_data_entity!(Invoice, "invoice", ["number"], { amount: f64 });

// New way with validation
impl_data_entity_validated!(
    Invoice, "invoice", ["number"], { amount: f64 },
    validate: {
        create: {
            amount: [required positive max_value(1_000_000.0)],
        },
    },
    filters: {
        create: {
            amount: [round_decimals(2)],
        },
    }
);
```

### Documentation

- ✅ [Validation and Filtering Guide](../guides/VALIDATION_AND_FILTERING.md) - Complete guide
- ✅ Updated [Getting Started](../guides/GETTING_STARTED.md) with validation examples
- ✅ Updated [Quick Start](../guides/QUICK_START.md) with validation examples
- ✅ Updated main [README](../../README.md) with validation feature

---

## v0.0.2 - Entity System Refactoring

### Summary

Major architectural refactoring introducing a **macro-driven entity system**, **automatic entity creation with linking**, and removal of multi-tenancy in favor of a cleaner, simpler architecture.

**Date**: October 2024  
**Version**: 0.0.2  
**Impact**: **BREAKING CHANGES** - Migration required

---

## 🔥 Major Changes

### 1. ❌ Removed Multi-Tenancy

**Before (v0.0.1)**:
```rust
struct Order {
    id: Uuid,
    tenant_id: Uuid,  // ❌ Required everywhere
    name: String,
    amount: f64,
}

// All service methods required tenant_id
service.create(&tenant_id, order).await?;
service.get(&tenant_id, &id).await?;
```

**After (v0.0.2)**:
```rust
// ✅ No tenant_id!
impl_data_entity!(Order, "order", ["name"], {
    amount: f64,
});

// Clean API without tenant_id
service.create(order).await?;
service.get(&id).await?;
```

**Rationale**: 
- Simpler API surface
- Easier to get started
- Tenant isolation can be handled at infrastructure level (separate databases, Kubernetes namespaces, etc.)
- 90% of use cases don't need application-level multi-tenancy

### 2. ✅ New Entity Hierarchy

**3-Level Architecture**:

```
Entity (Base)
├── id: Uuid
├── type: String
├── created_at: DateTime<Utc>
├── updated_at: DateTime<Utc>
├── deleted_at: Option<DateTime<Utc>>  ✅ Built-in soft delete
└── status: String

    ├─► Data (Business entities)
    │   └── name: String
    │       + custom fields...
    │
    └─► Link (Relationships)
        ├── source_id: Uuid
        ├── target_id: Uuid
        └── link_type: String
```

**Benefits**:
- Clear separation of concerns
- Built-in soft delete support at Entity level
- Automatic timestamp management
- Type-safe entity types

### 3. ✅ Macro-Driven Entity Definitions

**Before (v0.0.1)**: Manual boilerplate (50+ lines per entity)
```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Order {
    id: Uuid,
    tenant_id: Uuid,
    name: String,
    amount: f64,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
    // ... 10+ more lines of fields
}

impl Entity for Order {
    // ... 30+ lines of trait implementation
}

impl Data for Order {
    // ... 20+ lines of trait implementation
}
```

**After (v0.0.2)**: Macro magic! (4 lines)
```rust
impl_data_entity!(Order, "order", ["name", "amount"], {
    amount: f64,
});

// ✨ Auto-generates:
// - All base Entity fields (id, type, created_at, etc.)
// - name field (from Data trait)
// - Your custom fields
// - Full trait implementations (Entity, Data)
// - Constructor: Order::new(...)
// - Utility methods: soft_delete(), touch(), set_status(), restore()
// - Serde implementations
```

**Macros Provided**:
- `impl_data_entity!` - Complete Data entity
- `impl_link_entity!` - Custom Link entity
- `entity_fields!` - Inject base Entity fields
- `data_fields!` - Inject Entity + name
- `link_fields!` - Inject Entity + link fields

### 4. ✅ EntityCreator Trait for Auto-Creation

**New Feature**: Create entities automatically when creating links!

```rust
// Implement EntityCreator on your store
#[async_trait]
impl EntityCreator for OrderStore {
    async fn create_from_json(&self, entity_data: serde_json::Value) -> Result<serde_json::Value> {
        let order = Order::new(
            entity_data["name"].as_str().unwrap_or("").to_string(),
            entity_data["status"].as_str().unwrap_or("active").to_string(),
            entity_data["amount"].as_f64().unwrap_or(0.0),
        );
        self.add(order.clone());
        Ok(serde_json::to_value(order)?)
    }
}
```

**Usage**: Two ways to create links now!

#### Method 1: Link Existing Entities
```bash
POST /orders/{order_id}/invoices/{invoice_id}
Body: { "metadata": { "note": "Standard invoice" } }
```

#### Method 2: Create New Entity + Link Automatically 🎉
```bash
POST /orders/{order_id}/invoices
Body: {
  "entity": {
    "name": "INV-999",
    "amount": 999.99,
    "status": "active"
  },
  "metadata": { "note": "Auto-created" }
}

# Returns BOTH entity and link!
{
  "entity": { "id": "...", "name": "INV-999", ... },
  "link": { "id": "...", "source_id": "...", "target_id": "...", ... }
}
```

### 5. ✅ Enhanced Link Enrichment

**Feature**: Links now include full entity data automatically!

**Before (v0.0.1)**:
```json
{
  "links": [
    {
      "id": "link-123",
      "source_id": "order-abc",
      "target_id": "invoice-xyz"
      // ❌ Need separate query to get invoice data
    }
  ]
}
```

**After (v0.0.2)**:
```json
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
        // ✅ Full entity data included!
        ...
      }
    }
  ]
}
```

**Smart Enrichment**:
- From source (`/orders/{id}/invoices`) → Only target entities included
- From target (`/invoices/{id}/order`) → Only source entity included
- Direct link (`/orders/{id}/invoices/{inv_id}`) → Both entities included
- **Linked entities in the response** - fetched per link, not batched (see `docs/guides/ENRICHED_LINKS.md`)

### 6. ✅ Updated Module Trait

**New Methods**:

```rust
pub trait Module: Send + Sync {
    // ... existing methods ...
    
    // NEW: Provide entity fetchers for link enrichment
    fn get_entity_fetcher(&self, entity_type: &str) -> Option<Arc<dyn EntityFetcher>>;
    
    // NEW: Provide entity creators for auto-creation
    fn get_entity_creator(&self, entity_type: &str) -> Option<Arc<dyn EntityCreator>>;
}
```

---

## 📊 Impact Comparison

| Metric | Before (v0.0.1) | After (v0.0.2) | Improvement |
|--------|-----------------|----------------|-------------|
| **Lines per entity** | ~150 | ~4 | **-97%** |
| **Manual routing** | Required | Auto-generated | **100%** |
| **tenant_id everywhere** | Yes ❌ | No ✅ | **Cleaner API** |
| **Link creation methods** | 1 | 2 | **More flexible** |
| **Link enrichment** | Manual | Automatic | **Better DX** |
| **Soft delete** | Manual | Built-in | **Standardized** |

---

## 🔄 Migration Guide

### Step 1: Remove tenant_id

**Before**:
```rust
struct Order {
    id: Uuid,
    tenant_id: Uuid,  // ❌ Remove
    name: String,
}

service.create(&tenant_id, order).await?;
```

**After**:
```rust
impl_data_entity!(Order, "order", ["name"], {
    amount: f64,
});

service.create(order).await?;  // ✅ No tenant_id
```

### Step 2: Use Macros for Entity Definitions

**Before**: 150+ lines of boilerplate

**After**: 4 lines with macro
```rust
impl_data_entity!(Order, "order", ["name", "number"], {
    number: String,
    amount: f64,
    customer_name: Option<String>,
});
```

### Step 3: Implement EntityCreator (Optional but Recommended)

```rust
#[async_trait]
impl EntityCreator for OrderStore {
    async fn create_from_json(&self, entity_data: serde_json::Value) -> Result<serde_json::Value> {
        let order = Order::new(
            entity_data["name"].as_str().unwrap_or("").to_string(),
            entity_data["status"].as_str().unwrap_or("active").to_string(),
            entity_data["number"].as_str().unwrap_or("").to_string(),
            entity_data["amount"].as_f64().unwrap_or(0.0),
            entity_data["customer_name"].as_str().map(String::from),
        );
        self.add(order.clone());
        Ok(serde_json::to_value(order)?)
    }
}
```

### Step 4: Update Module Implementation

```rust
impl Module for BillingModule {
    // ... existing methods ...
    
    fn get_entity_fetcher(&self, entity_type: &str) -> Option<Arc<dyn EntityFetcher>> {
        match entity_type {
            "order" => Some(Arc::new(self.store.orders.clone())),
            _ => None,
        }
    }
    
    fn get_entity_creator(&self, entity_type: &str) -> Option<Arc<dyn EntityCreator>> {
        match entity_type {
            "order" => Some(Arc::new(self.store.orders.clone())),
            _ => None,
        }
    }
}
```

### Step 5: Update API Calls

**Link Creation - Before**:
```bash
# Only one way
POST /orders/{id}/invoices
Body: { "target_id": "{invoice_id}", "metadata": {...} }
```

**Link Creation - After**:
```bash
# Method 1: Link existing
POST /orders/{order_id}/invoices/{invoice_id}
Body: { "metadata": {...} }

# Method 2: Create new + link
POST /orders/{order_id}/invoices
Body: { "entity": {...}, "metadata": {...} }
```

---

## ✨ New Features

### 1. Entity-level Soft Delete (`soft_delete()` / `restore()`)

> Note: these methods only mark the entity. They do not change DELETE routes, which are hard deletes by default. See the README (Soft Deletes) for `ServerBuilder::with_soft_delete()` and `SoftDeleting`.


```rust
let mut order = Order::new(...);
order.soft_delete();  // Sets deleted_at timestamp
// Order still in database, just marked as deleted

order.restore();  // Clears deleted_at
// Order is active again
```

### 2. Automatic Timestamp Management

```rust
let mut order = Order::new(...);
// created_at and updated_at are auto-set

order.touch();  // Updates updated_at to now
```

### 3. Status Management

```rust
let mut order = Order::new(...);
order.set_status("completed".to_string());
// Status changed + updated_at refreshed
```

### 4. Type-Safe Entity Types

```rust
impl_data_entity!(Order, "order", ["name"], { amount: f64 });
// entity.entity_type() always returns "order"
// Impossible to have type mismatches!
```

---

## 🎯 Benefits

### For Developers

✅ **97% less boilerplate** - Macros generate everything  
✅ **Cleaner API** - No tenant_id clutter  
✅ **More flexibility** - Two ways to create links  
✅ **Better DX** - Auto-enriched link responses  
✅ **Built-in features** - Soft delete, timestamps, status  

### For Teams

✅ **Faster development** - Less code to write  
✅ **Easier maintenance** - Less code to maintain  
✅ **Consistent patterns** - Macros enforce consistency  
✅ **Better onboarding** - Simpler concepts  

### For Production

✅ **Performance** - Efficient link enrichment  
✅ **Flexibility** - Choose tenancy model  
✅ **Reliability** - Type-safe, compile-time checks  
✅ **Scalability** - Clean architecture scales well  

---

## 📚 Updated Documentation

All documentation has been updated to reflect these changes:

- ✅ [README.md](../../README.md) - Main overview
- ✅ [Quick Start](../guides/QUICK_START.md) - Fast intro
- ✅ [Getting Started](../guides/GETTING_STARTED.md) - Step-by-step tutorial
- ✅ [Architecture](../architecture/ARCHITECTURE.md) - Technical details
- ✅ [Microservice Example](../../examples/microservice/README.md) - Production patterns

---

## 🔮 Future Enhancements

Planned for v0.0.3+:

- [ ] ScyllaDB storage backend
- [ ] PostgreSQL storage backend
- [ ] Additional validators and filters
- [ ] Webhook system for entity events
- [x] GraphQL support ✅ (v0.0.7)
- [ ] GraphQL Subscriptions
- [ ] Performance optimizations for large datasets

---

## 🤝 Breaking Changes Summary

| Change | Migration Effort | Impact |
|--------|-----------------|---------|
| Removed tenant_id | Medium | All entities and service calls |
| New entity hierarchy | Low | Only trait implementations |
| Macro system | Low | Simplifies entity definition |
| EntityCreator trait | Low | Optional, add when needed |
| Module trait methods | Low | Just add two new methods |

---

## 🎉 Conclusion

This refactoring makes this-rs:
- ✅ **Simpler** - Removed complexity (tenant_id)
- ✅ **More powerful** - Added features (EntityCreator, enrichment)
- ✅ **Easier to use** - Macros eliminate boilerplate
- ✅ **More flexible** - Multiple ways to achieve goals

**The framework is now production-ready with a clean, modern architecture!** 🚀🦀✨

---

## 📞 Support

Questions or issues with migration?
- 📖 Check the [Getting Started Guide](../guides/GETTING_STARTED.md)
- 💬 Open a GitHub Discussion
- 🐛 Report bugs in GitHub Issues
- 📧 Contact the maintainers

**Welcome to this-rs v0.0.2!** 🎊
