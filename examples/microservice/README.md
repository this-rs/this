# Billing Microservice Example

## Description

Complete example of a **billing** microservice managing the Order → Invoice → Payment workflow, demonstrating:
- Clean modular architecture with **auto-generated routes**
- **ServerBuilder**: Zero boilerplate for routing
- Bidirectional link navigation
- Module system with `Module` trait
- In-memory store (replaceable with ScyllaDB/DynamoDB)
- Authorization policies in configuration
- **EntityCreator**: Create new entities with automatic linking
- **Macro-driven entities**: Zero boilerplate entity definitions

## 🚀 The Magic of Auto-Generation

This microservice uses the framework's `ServerBuilder` to **auto-generate all routes**:

```rust
#[tokio::main]
async fn main() -> Result<()> {
    let entity_store = EntityStore::new();
    let module = BillingModule::new(entity_store);

    // ✨ All routes are auto-generated here!
    let app = ServerBuilder::new()
        .with_link_service(InMemoryLinkService::new())
        .register_module(module)?
        .build()?;

    let listener = tokio::net::TcpListener::bind("127.0.0.1:3000").await?;
    axum::serve(listener, app).await?;
    Ok(())
}
```

**Zero manual routing lines needed!** All CRUD and link routes are created automatically.

## Structure

```
microservice/
├── config/              # Externalized configuration
│   └── links.yaml       # Entity, link, and auth configuration
├── store.rs             # Aggregated store (access to individual stores)
├── main.rs              # Entry point (~150 lines including 100 lines of test data)
├── module.rs            # BillingModule (implements Module trait)
└── entities/            # One folder per entity (best practice)
    ├── mod.rs           # Entity re-exports
    ├── order/
    │   ├── mod.rs       # Order module
    │   ├── model.rs     # Order struct (uses macro!)
    │   ├── store.rs     # OrderStore (in-memory)
    │   ├── handlers.rs  # HTTP handlers (create, list, get, etc.)
    │   └── descriptor.rs # OrderDescriptor (registers routes)
    ├── invoice/
    │   └── ... (same structure)
    └── payment/
        └── ... (same structure)
```

## Workflow

```
Order ──(has_invoice)──► Invoice ──(has_payment)──► Payment
  │                         │                           │
  └─────────────────────────┴───────────────────────────┘
         Complete billing workflow with links
```

## Running the Example

```bash
cd this-rs
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
    GET    /invoices                      - List all invoices
    POST   /invoices                      - Create a new invoice
    ... (same for payments)

  🔗 Link Routes (Generic for all entities):
    GET    /links/{link_id}                  - Get a specific link by ID
    GET    /{entity}/{id}/{route_name}        - List links (e.g. /orders/123/invoices)
    POST   /{entity}/{id}/{route_name}        - Create new entity + link automatically ✅
    GET    /{source}/{id}/{route_name}/{target_id}  - Get a specific link
    POST   /{source}/{id}/{route_name}/{target_id}  - Create link between existing entities
    PUT    /{source}/{id}/{route_name}/{target_id}  - Update link metadata
    DELETE /{source}/{id}/{route_name}/{target_id}  - Delete a link
    GET    /{entity}/{id}/links               - Introspection (list available link types)

  📋 Specific Link Routes (from config):
    GET    /orders/{id}/invoices             - List invoices for an order
    POST   /orders/{id}/invoices             - Create new invoice + link ✅
    GET    /orders/{id}/invoices/{invoice_id} - Get specific order→invoice link
    POST   /orders/{id}/invoices/{invoice_id} - Link existing order & invoice
    PUT    /orders/{id}/invoices/{invoice_id} - Update order→invoice link
    DELETE /orders/{id}/invoices/{invoice_id} - Delete order→invoice link
    GET    /invoices/{id}/order              - Get order for an invoice
    GET    /invoices/{id}/payments           - List payments for an invoice
    POST   /invoices/{id}/payments           - Create new payment + link ✅
    GET    /invoices/{id}/payments/{payment_id} - Get specific invoice→payment link
    POST   /invoices/{id}/payments/{payment_id} - Link existing invoice & payment
    GET    /payments/{id}/invoice            - Get invoice for a payment
```

## API Usage Examples

### Create Entities

```bash
# Create an order
curl -X POST http://localhost:3000/orders \
  -H 'Content-Type: application/json' \
  -d '{
    "number": "ORD-123",
    "amount": 1500.00,
    "customer_name": "Acme Corp",
    "status": "active"
  }'

# Create an invoice
curl -X POST http://localhost:3000/invoices \
  -H 'Content-Type: application/json' \
  -d '{
    "number": "INV-456",
    "amount": 1500.00,
    "due_date": "2024-12-31",
    "status": "pending"
  }'
```

### Create Links - Two Methods

#### Method 1: Link Existing Entities
```bash
# Link existing order and invoice
curl -X POST http://localhost:3000/orders/{order_id}/invoices/{invoice_id} \
  -H 'Content-Type: application/json' \
  -d '{
    "metadata": {
      "note": "Standard invoice",
      "priority": "high",
      "created_by": "system"
    }
  }'
```

#### Method 2: Create New Entity + Link Automatically ✨
```bash
# Create a NEW invoice and link it to the order in one call!
curl -X POST http://localhost:3000/orders/{order_id}/invoices \
  -H 'Content-Type: application/json' \
  -d '{
    "entity": {
      "number": "INV-999",
      "amount": 999.99,
      "due_date": "2024-12-31",
      "status": "active"
    },
    "metadata": {
      "note": "Auto-created invoice",
      "priority": "high"
    }
  }'

# Response includes BOTH the created invoice AND the link!
{
  "entity": {
    "id": "invoice-uuid",
    "type": "invoice",
    "name": "INV-999",
    "number": "INV-999",
    "amount": 999.99,
    "created_at": "2024-10-23T...",
    ...
  },
  "link": {
    "id": "link-uuid",
    "type": "link",
    "link_type": "has_invoice",
    "source_id": "order-uuid",
    "target_id": "invoice-uuid",
    "created_at": "2024-10-23T...",
    ...
  }
}
```

### Query Links (Auto-Enriched!)

```bash
# List invoices for an order (includes full invoice data!)
curl http://localhost:3000/orders/{order_id}/invoices | jq .

# Response with enriched entities:
{
  "links": [
    {
      "id": "link-123",
      "type": "link",
      "link_type": "has_invoice",
      "source_id": "order-uuid",
      "target_id": "invoice-uuid",
      "target": {
        "id": "invoice-uuid",
        "type": "invoice",
        "name": "INV-001",
        "number": "INV-001",
        "amount": 1500.00,
        "due_date": "2024-12-31",
        ...
      },
      "metadata": {
        "note": "Standard invoice",
        "priority": "high"
      }
    }
  ],
  "count": 1,
  "link_type": "has_invoice",
  "direction": "Forward"
}

# Get a specific link (includes both order and invoice!)
curl http://localhost:3000/orders/{order_id}/invoices/{invoice_id} | jq .

# Get order from an invoice (reverse navigation)
curl http://localhost:3000/invoices/{invoice_id}/order | jq .
```

### Update and Delete

```bash
# Update link metadata
curl -X PUT http://localhost:3000/orders/{order_id}/invoices/{invoice_id} \
  -H 'Content-Type: application/json' \
  -d '{
    "metadata": {
      "status": "verified",
      "verified_by": "admin",
      "verified_at": "2024-10-23T12:00:00Z"
    }
  }'

# Delete a link
curl -X DELETE http://localhost:3000/orders/{order_id}/invoices/{invoice_id}

# Update an entity
curl -X PUT http://localhost:3000/orders/{order_id} \
  -H 'Content-Type: application/json' \
  -d '{
    "amount": 2000.00,
    "notes": "Updated amount"
  }'
```

## Key Features Demonstrated

### 1. Macro-Driven Entities (Zero Boilerplate!)

```rust
// entities/order/model.rs
use this::prelude::*;

impl_data_entity!(Order, "order", ["name", "number"], {
    number: String,
    amount: f64,
    customer_name: Option<String>,
    notes: Option<String>,
});

// That's it! You get:
// - All base Entity fields (id, type, created_at, updated_at, deleted_at, status)
// - name field (from Data trait)
// - Your custom fields (number, amount, customer_name, notes)
// - Full trait implementations (Entity, Data)
// - Constructor: Order::new(...)
// - Utility methods: soft_delete(), touch(), set_status(), restore()
```

### 2. EntityCreator for Automatic Entity Creation

```rust
// entities/order/store.rs
#[async_trait]
impl EntityCreator for OrderStore {
    async fn create_from_json(&self, entity_data: serde_json::Value) -> Result<serde_json::Value> {
        let order = Order::new(
            entity_data["number"].as_str().unwrap_or("ORD-000").to_string(),
            entity_data["status"].as_str().unwrap_or("active").to_string(),
            entity_data["number"].as_str().unwrap_or("ORD-000").to_string(),
            entity_data["amount"].as_f64().unwrap_or(0.0),
            entity_data["customer_name"].as_str().map(String::from),
            entity_data["notes"].as_str().map(String::from),
        );
        
        self.add(order.clone());
        Ok(serde_json::to_value(order)?)
    }
}
```

### 3. EntityFetcher for Link Enrichment

```rust
// entities/order/store.rs
#[async_trait]
impl EntityFetcher for OrderStore {
    async fn fetch_as_json(&self, entity_id: &Uuid) -> Result<serde_json::Value> {
        let order = self.get(entity_id)
            .ok_or_else(|| anyhow::anyhow!("Order not found: {}", entity_id))?;
        Ok(serde_json::to_value(order)?)
    }
}
```

### 4. Module System with Auto-Registration

```rust
// module.rs
impl Module for BillingModule {
    fn name(&self) -> &str { "billing-service" }
    fn entity_types(&self) -> Vec<&str> { vec!["order", "invoice", "payment"] }
    
    fn links_config(&self) -> Result<LinksConfig> {
        LinksConfig::from_file("examples/microservice/config/links.yaml")
    }
    
    fn register_entities(&self, registry: &mut EntityRegistry) {
        registry.register(Box::new(OrderDescriptor::new(self.store.orders.clone())));
        registry.register(Box::new(InvoiceDescriptor::new(self.store.invoices.clone())));
        registry.register(Box::new(PaymentDescriptor::new(self.store.payments.clone())));
    }
    
    fn get_entity_fetcher(&self, entity_type: &str) -> Option<Arc<dyn EntityFetcher>> {
        match entity_type {
            "order" => Some(Arc::new(self.store.orders.clone()) as Arc<dyn EntityFetcher>),
            "invoice" => Some(Arc::new(self.store.invoices.clone()) as Arc<dyn EntityFetcher>),
            "payment" => Some(Arc::new(self.store.payments.clone()) as Arc<dyn EntityFetcher>),
            _ => None,
        }
    }
    
    fn get_entity_creator(&self, entity_type: &str) -> Option<Arc<dyn EntityCreator>> {
        match entity_type {
            "order" => Some(Arc::new(self.store.orders.clone()) as Arc<dyn EntityCreator>),
            "invoice" => Some(Arc::new(self.store.invoices.clone()) as Arc<dyn EntityCreator>),
            "payment" => Some(Arc::new(self.store.payments.clone()) as Arc<dyn EntityCreator>),
            _ => None,
        }
    }
}
```

### 5. YAML Configuration

```yaml
# config/links.yaml
entities:
  - singular: order
    plural: orders
  - singular: invoice
    plural: invoices
  - singular: payment
    plural: payments

links:
  - link_type: has_invoice
    source_type: order
    target_type: invoice
    forward_route_name: invoices
    reverse_route_name: order
    description: "Order has invoices"
    
  - link_type: has_payment
    source_type: invoice
    target_type: payment
    forward_route_name: payments
    reverse_route_name: invoice
    description: "Invoice has payments"
```

## Architecture Benefits

### Before this-rs (❌)
```
- 300+ lines of manual routing
- Duplicate CRUD logic per entity
- Manual link handling
- N+1 query problems
- Inconsistent patterns
```

### With this-rs (✅)
```
- 40 lines in main.rs
- Zero routing code
- Automatic link enrichment
- No N+1 queries
- Consistent patterns everywhere
```

## Next Steps

1. **Add More Entities**: Just create a new folder in `entities/` with the 5 files
2. **Add More Links**: Update `config/links.yaml`
3. **Replace Storage**: Implement `DataService` and `LinkService` for your DB
4. **Add Authorization**: Configure auth policies in `links.yaml`
5. **Deploy**: It's a standard Axum server, deploy anywhere!

## Documentation

- [Main README](../../README.md) - Framework overview
- [Quick Start](../../docs/guides/QUICK_START.md) - Fast introduction
- [Architecture](../../docs/architecture/ARCHITECTURE.md) - Technical details
- [Enriched Links](../../docs/guides/ENRICHED_LINKS.md) - Link enrichment guide

---

**This microservice demonstrates production-ready patterns with zero boilerplate!** 🚀🦀✨
