# Enriched Links Guide

## 🎯 Overview

this-rs automatically **enriches link responses** with full entity data, eliminating the need for separate queries and preventing N+1 query problems.

## ✨ What Are Enriched Links?

When you query links, instead of just getting IDs, you get **complete entity objects** embedded in the response.

### Without Enrichment (Traditional APIs)

```json
{
  "links": [
    {
      "id": "link-123",
      "source_id": "order-abc",
      "target_id": "invoice-xyz"
    }
  ]
}
// Now you need 2 more queries:
// GET /orders/order-abc
// GET /invoices/invoice-xyz
```

### With Enrichment (this-rs)

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
        "due_date": "2024-12-31",
        "status": "pending"
      },
      "metadata": {
        "created_at": "2024-01-15",
        "priority": "high"
      }
    }
  ]
}
// ✅ All data in one response!
```

---

## 🚀 How It Works

### 1. EntityFetcher Trait

Each entity store implements `EntityFetcher`:

```rust
#[async_trait]
pub trait EntityFetcher: Send + Sync {
    async fn fetch_as_json(&self, entity_id: &Uuid) -> Result<serde_json::Value>;
}

// Implementation example
#[async_trait]
impl EntityFetcher for OrderStore {
    async fn fetch_as_json(&self, entity_id: &Uuid) -> Result<serde_json::Value> {
        let order = self.get(entity_id)
            .ok_or_else(|| anyhow::anyhow!("Order not found"))?;
        Ok(serde_json::to_value(order)?)
    }
}
```

### 2. Module Registration

Modules provide entity fetchers:

```rust
impl Module for BillingModule {
    fn get_entity_fetcher(&self, entity_type: &str) -> Option<Arc<dyn EntityFetcher>> {
        match entity_type {
            "order" => Some(Arc::new(self.store.orders.clone())),
            "invoice" => Some(Arc::new(self.store.invoices.clone())),
            "payment" => Some(Arc::new(self.store.payments.clone())),
            _ => None,
        }
    }
}
```

### 3. Automatic Enrichment

ServerBuilder collects all fetchers:

```rust
// ServerBuilder.build()
let mut fetchers_map: HashMap<String, Arc<dyn EntityFetcher>> = HashMap::new();
for module in &self.modules {
    for entity_type in module.entity_types() {
        if let Some(fetcher) = module.get_entity_fetcher(entity_type) {
            fetchers_map.insert(entity_type.to_string(), fetcher);
        }
    }
}

// AppState has access to all fetchers
let link_state = AppState {
    entity_fetchers: Arc::new(fetchers_map),
    // ...
};
```

### 4. Smart Context-Aware Enrichment

```rust
pub enum EnrichmentContext {
    FromSource,   // Query from source -> only enrich target
    FromTarget,   // Query from target -> only enrich source
    DirectLink,   // Direct link access -> enrich both
}

async fn enrich_links_with_entities(
    state: &AppState,
    links: Vec<LinkEntity>,
    context: EnrichmentContext,
    link_definition: &LinkDefinition,
) -> Result<Vec<EnrichedLink>> {
    for link in links {
        let source_entity = match context {
            EnrichmentContext::FromSource => None,  // Skip, we came from source
            _ => fetch_entity(state, &link_definition.source_type, &link.source_id).await,
        };
        
        let target_entity = match context {
            EnrichmentContext::FromTarget => None,  // Skip, we came from target
            _ => fetch_entity(state, &link_definition.target_type, &link.target_id).await,
        };
        
        enriched.push(EnrichedLink {
            source: source_entity,
            target: target_entity,
            // ...
        });
    }
}
```

---

## 🎨 Enrichment Patterns

### Pattern 1: Forward Navigation (From Source)

```bash
GET /orders/123/invoices
```

**Enrichment**: Only `target` (invoices) included

```json
{
  "links": [
    {
      "source_id": "order-123",
      "target_id": "invoice-456",
      "target": { /* Full invoice data */ }
      // No "source" field (we came from the order)
    }
  ]
}
```

**Rationale**: You already have the order (you queried from it), only need invoice data.

### Pattern 2: Reverse Navigation (From Target)

```bash
GET /invoices/456/order
```

**Enrichment**: Only `source` (order) included

```json
{
  "links": [
    {
      "source_id": "order-123",
      "target_id": "invoice-456",
      "source": { /* Full order data */ }
      // No "target" field (we came from the invoice)
    }
  ]
}
```

**Rationale**: You already have the invoice, only need order data.

### Pattern 3: Direct Link Access

```bash
GET /orders/123/invoices/456
# or
GET /links/link-uuid
```

**Enrichment**: Both `source` and `target` included

```json
{
  "id": "link-uuid",
  "source_id": "order-123",
  "target_id": "invoice-456",
  "source": { /* Full order data */ },
  "target": { /* Full invoice data */ }
}
```

**Rationale**: Direct link access, provide complete context.

---

## 🔥 Performance Optimization

### No N+1 Queries

**Traditional Approach (N+1 Problem)**:
```
1 query: Get links (N results)
N queries: Get each target entity
Total: N+1 queries ❌
```

**this-rs Approach**:
```
1 query: Get links
1 batch operation: Fetch all entities efficiently
Total: Effectively 2 operations ✅
```

### Efficient Fetching

```rust
// Entities are fetched in parallel when possible
async fn enrich_links_with_entities(...) {
    let mut tasks = vec![];
    
    for link in links {
        if need_source {
            tasks.push(fetch_entity(..., link.source_id));
        }
        if need_target {
            tasks.push(fetch_entity(..., link.target_id));
        }
    }
    
    // Execute all fetches concurrently
    let results = join_all(tasks).await;
}
```

### Caching (Optional)

You can add caching in your `EntityFetcher` implementation:

```rust
#[async_trait]
impl EntityFetcher for OrderStore {
    async fn fetch_as_json(&self, entity_id: &Uuid) -> Result<serde_json::Value> {
        // Check cache first
        if let Some(cached) = self.cache.get(entity_id) {
            return Ok(cached);
        }
        
        // Fetch from storage
        let order = self.get(entity_id)?;
        let json = serde_json::to_value(order)?;
        
        // Cache for next time
        self.cache.put(entity_id, json.clone());
        
        Ok(json)
    }
}
```

---

## 💡 Usage Examples

### Example 1: Get All Invoices for an Order

```bash
curl http://localhost:3000/orders/abc-123/invoices | jq .
```

Response:
```json
{
  "links": [
    {
      "id": "link-1",
      "source_id": "abc-123",
      "target_id": "inv-001",
      "target": {
        "id": "inv-001",
        "type": "invoice",
        "name": "INV-001",
        "amount": 1500.00,
        "status": "pending"
      }
    },
    {
      "id": "link-2",
      "source_id": "abc-123",
      "target_id": "inv-002",
      "target": {
        "id": "inv-002",
        "type": "invoice",
        "name": "INV-002",
        "amount": 2500.00,
        "status": "paid"
      }
    }
  ],
  "count": 2
}
```

### Example 2: Get Order for an Invoice (Reverse)

```bash
curl http://localhost:3000/invoices/inv-001/order | jq .
```

Response:
```json
{
  "links": [
    {
      "id": "link-1",
      "source_id": "abc-123",
      "target_id": "inv-001",
      "source": {
        "id": "abc-123",
        "type": "order",
        "name": "ORD-123",
        "amount": 5000.00,
        "customer_name": "Acme Corp"
      }
    }
  ]
}
```

### Example 3: Get Specific Link with Both Entities

```bash
curl http://localhost:3000/orders/abc-123/invoices/inv-001 | jq .
```

Response:
```json
{
  "id": "link-1",
  "source_id": "abc-123",
  "target_id": "inv-001",
  "source": {
    "id": "abc-123",
    "type": "order",
    "name": "ORD-123",
    "amount": 5000.00
  },
  "target": {
    "id": "inv-001",
    "type": "invoice",
    "name": "INV-001",
    "amount": 1500.00
  },
  "metadata": {
    "created_at": "2024-01-15"
  }
}
```

---

## 🎯 Best Practices

### 1. Implement EntityFetcher for All Entities

```rust
// ✅ Do this
#[async_trait]
impl EntityFetcher for YourStore {
    async fn fetch_as_json(&self, entity_id: &Uuid) -> Result<serde_json::Value> {
        // Implementation
    }
}
```

### 2. Register Fetchers in Module

```rust
// ✅ Do this
impl Module for YourModule {
    fn get_entity_fetcher(&self, entity_type: &str) -> Option<Arc<dyn EntityFetcher>> {
        match entity_type {
            "your_entity" => Some(Arc::new(self.store.clone())),
            _ => None,
        }
    }
}
```

### 3. Handle Missing Entities Gracefully

```rust
// ✅ Do this - return None instead of error
async fn fetch_entity(...) -> Option<serde_json::Value> {
    match fetcher.fetch_as_json(entity_id).await {
        Ok(entity) => Some(entity),
        Err(_) => None,  // Entity not found or deleted
    }
}

// Enriched link with missing entity
{
  "source_id": "abc-123",
  "target_id": "deleted-entity",
  "target": null  // Entity was deleted or not found
}
```

### 4. Use Appropriate Enrichment Context

The framework automatically chooses the right context:
- `/orders/123/invoices` → `FromSource`
- `/invoices/456/order` → `FromTarget`
- `/orders/123/invoices/456` → `DirectLink`

---

## 🎁 Benefits

✅ **No N+1 Queries** - All data fetched efficiently  
✅ **Better UX** - Clients get complete data in one request  
✅ **Reduced Network** - Fewer round trips  
✅ **Type-Safe** - EntityFetcher trait ensures correctness  
✅ **Context-Aware** - Smart enrichment based on query direction  
✅ **Flexible** - Easy to customize per entity  

---

## 📚 Related Documentation

- [Architecture](../architecture/ARCHITECTURE.md)
- [Getting Started](GETTING_STARTED.md)
- [Multi-Level Navigation](MULTI_LEVEL_NAVIGATION.md)

---

**Enriched links make your API fast, efficient, and delightful to use!** 🚀✨
