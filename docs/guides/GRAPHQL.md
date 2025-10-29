# GraphQL API Guide

This guide explains how to use the GraphQL API exposure in `this-rs`, which provides a fully dynamic GraphQL schema generated automatically from your entity definitions.

## 🎯 Overview

The GraphQL exposure provides:

- ✅ **100% Dynamic Schema** - Automatically generated from registered entities
- ✅ **Specific Types** - Each entity gets its own GraphQL type (`Order`, `Invoice`, etc.)
- ✅ **Automatic Relations** - Relations are discovered from `links.yaml`
- ✅ **Full CRUD** - Create, Read, Update, Delete operations via GraphQL
- ✅ **Link Management** - Specialized mutations for linking/unlinking entities
- ✅ **Type-Safe Queries** - Use actual entity names (`orders`, `order`, etc.)

## 🚀 Quick Start

### Enable GraphQL Feature

Add the GraphQL feature to your `Cargo.toml`:

```toml
[dependencies]
this-rs = { version = "0.0.6", features = ["graphql"] }
```

### Build Server with GraphQL

```rust
use this::prelude::*;
use this::server::{ServerBuilder, RestExposure, GraphQLExposure};

#[tokio::main]
async fn main() -> Result<()> {
    let entity_store = EntityStore::new();
    let module = BillingModule::new(entity_store);
    
    let host = Arc::new(
        ServerBuilder::new()
            .with_link_service(InMemoryLinkService::new())
            .register_module(module)?
            .build_host()?
    );
    
    // Create REST router
    let rest_router = RestExposure::build_router(host.clone())?;
    
    // Create GraphQL router
    let graphql_router = GraphQLExposure::build_router(host)?;
    
    // Combine both
    let app = Router::new()
        .merge(rest_router)
        .merge(graphql_router);
    
    let listener = tokio::net::TcpListener::bind("127.0.0.1:3000").await?;
    axum::serve(listener, app).await?;
    Ok(())
}
```

### Access GraphQL

The server now exposes:

- **GraphQL Endpoint**: `POST /graphql`
- **GraphQL Playground**: `GET /graphql/playground`
- **Schema SDL**: `GET /graphql/schema`

## 📊 Schema Generation

The GraphQL schema is **automatically generated** at runtime from:

1. **Entity Definitions**: All entities registered via modules
2. **Entity Introspection**: Fields are discovered via `EntityFetcher::get_sample_entity()` or `list_as_json()`
3. **Link Configuration**: Relations are added from `links.yaml`

### Example Generated Schema

For entities `Order`, `Invoice`, and `Payment` with links configured:

```graphql
type Order {
  id: ID!
  number: String!
  customerName: String!
  amount: Float!
  status: String!
  createdAt: String!
  updatedAt: String!
  invoices: [Invoice!]!
}

type Invoice {
  id: ID!
  number: String!
  amount: Float!
  dueDate: String!
  status: String!
  order: Order
  payments: [Payment!]!
}

type Payment {
  id: ID!
  number: String!
  amount: Float!
  method: String!
  status: String!
  invoice: Invoice
}

type Query {
  order(id: ID!): Order
  orders(limit: Int, offset: Int): [Order!]!
  invoice(id: ID!): Invoice
  invoices(limit: Int, offset: Int): [Invoice!]!
  payment(id: ID!): Payment
  payments(limit: Int, offset: Int): [Payment!]!
}

type Mutation {
  createOrder(data: JSON!): Order!
  updateOrder(id: ID!, data: JSON!): Order!
  deleteOrder(id: ID!): Boolean!
  createInvoice(data: JSON!): Invoice!
  updateInvoice(id: ID!, data: JSON!): Invoice!
  deleteInvoice(id: ID!): Boolean!
  createPayment(data: JSON!): Payment!
  updatePayment(id: ID!, data: JSON!): Payment!
  deletePayment(id: ID!): Boolean!
  
  # Link mutations
  createLink(sourceId: ID!, targetId: ID!, linkType: String!, metadata: JSON): Link!
  deleteLink(id: ID!): Boolean!
  
  # Typed link mutations
  createInvoiceForOrder(parentId: ID!, data: JSON!, linkType: String): Invoice!
  linkPaymentToInvoice(sourceId: ID!, targetId: ID!, linkType: String, metadata: JSON): Link!
  unlinkPaymentFromInvoice(sourceId: ID!, targetId: ID!, linkType: String): Boolean!
}
```

## 🔍 Queries

### List Entities with Pagination

```graphql
query {
  orders(limit: 10, offset: 0) {
    id
    number
    customerName
    amount
    status
  }
}
```

**Response**:
```json
{
  "data": {
    "orders": [
      {
        "id": "123e4567-e89b-12d3-a456-426614174000",
        "number": "ORD-001",
        "customerName": "John Doe",
        "amount": 1000.0,
        "status": "active"
      }
    ]
  }
}
```

### Get Single Entity

```graphql
query {
  order(id: "123e4567-e89b-12d3-a456-426614174000") {
    id
    number
    customerName
    amount
    status
    createdAt
  }
}
```

### Query with Relations

GraphQL automatically resolves relations configured in `links.yaml`:

```graphql
query {
  order(id: "123e4567-e89b-12d3-a456-426614174000") {
    id
    number
    customerName
    invoices {
      id
      number
      amount
      dueDate
      payments {
        id
        amount
        method
        transactionId
      }
    }
  }
}
```

**Nested Relations**: Relations can be nested to any depth. The executor automatically resolves them recursively.

## ✏️ Mutations

### Create Entity

```graphql
mutation {
  createOrder(data: {
    number: "ORD-999"
    customerName: "Jane Doe"
    amount: 2000.0
    status: "active"
    notes: "New customer order"
  }) {
    id
    number
    customerName
    amount
    status
  }
}
```

**Note**: The `data` argument is a `JSON!` scalar type that accepts any JSON object matching your entity structure.

### Update Entity

```graphql
mutation {
  updateOrder(
    id: "123e4567-e89b-12d3-a456-426614174000"
    data: {
      amount: 2500.0
      status: "completed"
    }
  ) {
    id
    amount
    status
  }
}
```

### Delete Entity

```graphql
mutation {
  deleteOrder(id: "123e4567-e89b-12d3-a456-426614174000")
}
```

Returns `true` if successfully deleted, `false` otherwise.

### Create and Link Entity

Create a new entity and automatically link it to a parent:

```graphql
mutation {
  createInvoiceForOrder(
    parentId: "123e4567-e89b-12d3-a456-426614174000"
    data: {
      number: "INV-999"
      amount: 1500.0
      status: "pending"
      dueDate: "2024-12-31"
    }
  ) {
    id
    number
    amount
    order {
      id
      number
    }
  }
}
```

This mutation:
1. Creates the new `Invoice` entity
2. Automatically creates a link between the `Order` (parent) and `Invoice`
3. Returns the created entity with its relation resolved

### Link Existing Entities

Link two existing entities together:

```graphql
mutation {
  linkPaymentToInvoice(
    sourceId: "payment-uuid"
    targetId: "invoice-uuid"
    linkType: "payment"
    metadata: {
      processed: true
      timestamp: "2024-01-15T10:30:00Z"
    }
  ) {
    id
    linkType
    sourceId
    targetId
    metadata
  }
}
```

### Unlink Entities

Remove a link between two entities:

```graphql
mutation {
  unlinkPaymentFromInvoice(
    sourceId: "payment-uuid"
    targetId: "invoice-uuid"
  )
}
```

Returns `true` if unlinked successfully.

### Generic Link Mutations

#### Create Link

```graphql
mutation {
  createLink(
    sourceId: "order-uuid"
    targetId: "invoice-uuid"
    linkType: "has_invoice"
    metadata: {
      note: "Test link"
      priority: "high"
    }
  ) {
    id
    linkType
    sourceId
    targetId
    metadata
    createdAt
  }
}
```

#### Delete Link

```graphql
mutation {
  deleteLink(id: "link-uuid")
}
```

## 🔗 Automatic Relations

Relations are automatically added to GraphQL types based on `links.yaml` configuration.

### Forward Relations (One-to-Many)

```yaml
links:
  - link_type: has_invoice
    source_type: order
    target_type: invoice
    forward_route_name: invoices
    reverse_route_name: order
```

This creates:
- `Order.invoices: [Invoice!]!` (forward)
- `Invoice.order: Order` (reverse)

### Usage in Queries

```graphql
query {
  orders {
    id
    number
    invoices {  # Forward relation
      id
      amount
    }
  }
}
```

```graphql
query {
  invoice(id: "...") {
    id
    amount
    order {  # Reverse relation
      id
      number
    }
  }
}
```

## 📝 JSON Scalar Type

The `JSON` scalar type is used for:
- Mutation `data` arguments (flexible entity creation/updates)
- Link `metadata` (custom link metadata)

### Example with JSON Input

```graphql
mutation {
  createOrder(data: {
    number: "ORD-001"
    customerName: "John Doe"
    amount: 1000.0
    status: "active"
    customField: "value"
    nested: {
      key: "value"
    }
  }) {
    id
  }
}
```

## 🎨 Schema Discovery

Fields are discovered automatically from entity data:

1. **Sample Entity**: `EntityFetcher::get_sample_entity()` provides a sample
2. **List Fallback**: If no sample, `EntityFetcher::list_as_json()` is used
3. **Field Inference**: JSON structure is analyzed to determine GraphQL types

### Field Type Mapping

| JSON Type | GraphQL Type |
|-----------|--------------|
| `string` | `String!` |
| `number` (integer) | `Int!` |
| `number` (float) | `Float!` |
| `boolean` | `Boolean!` |
| `null` or `missing` | `String` (nullable) |
| `object` | `JSON!` |
| `array` | `[JSON!]!` |

### Entity Fields

All entities automatically include:
- `id: ID!` - UUID as GraphQL ID
- `createdAt: String!` - ISO 8601 timestamp
- `updatedAt: String!` - ISO 8601 timestamp
- `deletedAt: String` - ISO 8601 timestamp (nullable)
- `status: String!` - Entity status
- Custom fields from entity definition

## 🛠️ Implementation Requirements

For GraphQL to work, your modules must provide:

### EntityFetcher Implementation

```rust
#[async_trait]
impl EntityFetcher for OrderStore {
    async fn fetch_as_json(&self, entity_id: &Uuid) -> Result<serde_json::Value> {
        let order = self.get(entity_id)
            .ok_or_else(|| anyhow::anyhow!("Order not found"))?;
        Ok(serde_json::to_value(order)?)
    }
    
    async fn list_as_json(
        &self,
        limit: Option<i32>,
        offset: Option<i32>,
    ) -> Result<Vec<serde_json::Value>> {
        let orders = self.list();
        let offset = offset.unwrap_or(0) as usize;
        let limit = limit.unwrap_or(20) as usize;
        
        let paginated: Vec<Order> = orders
            .into_iter()
            .skip(offset)
            .take(limit)
            .collect();
        
        paginated
            .into_iter()
            .map(|order| serde_json::to_value(order).map_err(Into::into))
            .collect()
    }
    
    async fn get_sample_entity(&self) -> Result<serde_json::Value> {
        // Return a sample entity for schema discovery
        let sample = Order::new(
            "ORD-SAMPLE".to_string(),
            "active".to_string(),
            "ORD-001".to_string(),
            1000.0,
            Some("Sample Customer".to_string()),
            Some("Sample notes".to_string()),
        );
        Ok(serde_json::to_value(sample)?)
    }
}
```

### EntityCreator Implementation

```rust
#[async_trait]
impl EntityCreator for OrderStore {
    async fn create_from_json(&self, entity_data: serde_json::Value) -> Result<serde_json::Value> {
        let order = Order::new(
            entity_data["number"].as_str().unwrap_or("").to_string(),
            entity_data["status"].as_str().unwrap_or("active").to_string(),
            entity_data["customerName"].as_str().unwrap_or("").to_string(),
            entity_data["amount"].as_f64().unwrap_or(0.0),
            entity_data["notes"].as_str().map(String::from),
            None,
        );
        self.add(order.clone());
        Ok(serde_json::to_value(order)?)
    }
    
    async fn update_from_json(
        &self,
        entity_id: &Uuid,
        entity_data: serde_json::Value,
    ) -> Result<serde_json::Value> {
        let mut order = self.get(entity_id)
            .ok_or_else(|| anyhow::anyhow!("Order not found"))?;
        
        if let Some(amount) = entity_data.get("amount").and_then(|v| v.as_f64()) {
            // Update fields as needed
        }
        
        self.update(entity_id, order.clone());
        Ok(serde_json::to_value(order)?)
    }
    
    async fn delete(&self, entity_id: &Uuid) -> Result<()> {
        self.remove(entity_id);
        Ok(())
    }
}
```

### Module Registration

```rust
impl Module for BillingModule {
    // ... other methods ...
    
    fn get_entity_fetcher(&self, entity_type: &str) -> Option<Arc<dyn EntityFetcher>> {
        match entity_type {
            "order" => Some(Arc::new(self.store.orders.clone())),
            "invoice" => Some(Arc::new(self.store.invoices.clone())),
            "payment" => Some(Arc::new(self.store.payments.clone())),
            _ => None,
        }
    }
    
    fn get_entity_creator(&self, entity_type: &str) -> Option<Arc<dyn EntityCreator>> {
        match entity_type {
            "order" => Some(Arc::new(self.store.orders.clone())),
            "invoice" => Some(Arc::new(self.store.invoices.clone())),
            "payment" => Some(Arc::new(self.store.payments.clone())),
            _ => None,
        }
    }
}
```

## 🔧 Advanced Usage

### Using GraphQL Playground

Access the interactive playground at `http://localhost:3000/graphql/playground` to:

- Explore the schema
- Test queries interactively
- View autocomplete suggestions
- See query execution time

### Downloading Schema SDL

Get the complete schema in SDL format:

```bash
curl http://localhost:3000/graphql/schema
```

Useful for:
- Generating TypeScript types
- Creating GraphQL clients
- Documentation generation
- Schema validation tools

### Error Handling

GraphQL returns structured errors:

```json
{
  "data": null,
  "errors": [
    {
      "message": "Missing required argument 'data'",
      "locations": [{"line": 2, "column": 3}],
      "path": ["createOrder"]
    }
  ]
}
```

## ⚠️ Limitations

- **Subscriptions**: Not yet implemented (planned)
- **Advanced Filtering**: Limited to pagination (advanced filters coming)
- **Schema Caching**: Schema is regenerated on each request (caching planned)
- **Field-Level Authorization**: Not yet implemented

## 🎯 Best Practices

1. **Always implement `get_sample_entity()`** - Enables accurate schema generation
2. **Use typed mutations** - Prefer `createInvoiceForOrder` over generic `createLink` when possible
3. **Include IDs in selections** - Always select `id` field for mutations that return entities
4. **Leverage nested queries** - Use relation queries to fetch related data in one request
5. **Monitor query complexity** - Deeply nested queries can be expensive

## 📚 Related Documentation

- [Architecture Overview](../architecture/ARCHITECTURE.md) - Framework architecture
- [GraphQL Implementation](../architecture/GRAPHQL_IMPLEMENTATION.md) - Technical details
- [Microservice Example](../../examples/microservice/README_GRAPHQL.md) - Complete example

## 🚀 Next Steps

- Try the [GraphQL example](../../examples/microservice/main_graphql.rs)
- Explore the generated schema at `/graphql/schema`
- Test queries in the GraphQL Playground
- Read the [technical implementation guide](../architecture/GRAPHQL_IMPLEMENTATION.md)

