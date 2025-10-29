# GraphQL Microservice Example

This example demonstrates the use of GraphQL exposure in the `this-rs` framework alongside the REST API.

## 🎯 Features

- ✅ **Named queries by entity type**: `order(id)`, `invoice(id)`, `payment(id)`
- ✅ **List queries**: `orders`, `invoices`, `payments` with pagination support
- ✅ **Automatic relations**: Access related entities via fields (`order.invoices`, `invoice.payments`)
- ✅ **Nested relations**: Deep navigation (`order.invoices.payments`)
- ✅ **Automatically generated schema**: Based on entities registered by modules
- ✅ **GraphQL Playground**: Interactive interface for testing queries
- ✅ **Full CRUD mutations**: Create, update, and delete entities via GraphQL
- ✅ **Link mutations**: Create, link, and unlink entities with specialized mutations

## Getting Started

```bash
cargo run --example microservice_graphql --features graphql
```

The server starts on `http://127.0.0.1:3000` with both REST and GraphQL endpoints.

## Available Endpoints

- **REST API**: All standard CRUD endpoints
- **GraphQL API**: 
  - Endpoint `/graphql` (POST)
  - Playground `/graphql/playground` (GET)
  - Schema SDL `/graphql/schema` (GET)

## GraphQL Playground

Access the interactive playground: http://127.0.0.1:3000/graphql/playground

## GraphQL Schema

**Download the SDL schema**: http://127.0.0.1:3000/graphql/schema

The schema is **automatically generated** from registered entities:
- ✅ Specific GraphQL types for each entity (`Order`, `Invoice`, `Payment`)
- ✅ All fields automatically discovered from data
- ✅ Automatic relations from `links.yaml`
- ✅ Complete CRUD queries and mutations
- ✅ **100% generic** - no hardcoded code in the framework

The SDL (Schema Definition Language) is useful for:
- Generating typed GraphQL clients
- Automatic documentation
- Query validation
- Integration with tools like GraphQL Code Generator

## Available GraphQL Queries

### List entities with pagination

```graphql
query {
  orders(limit: 10, offset: 0) {
    id
    number
    customerName
    amount
    status
    invoices {
      id
      number
      amount
    }
  }
}
```

### Get entity by ID

Instead of using `entity(id, entityType)`, you can query directly by type:

```graphql
query {
  order(id: "UUID") {
    id
    number
    customerName
    amount
    status
    createdAt
    updatedAt
  }
}
```

### Automatic relations

Entities automatically expose their relations via fields:

```graphql
query {
  order(id: "UUID") {
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

**Result**:
```json
{
  "data": {
    "order": {
      "id": "d16e72cf-d7f7-41f4-aa86-ca428967fa0a",
      "number": "ORD-001",
      "invoices": [
        {
          "id": "b5ef6156-0dcb-49fd-b425-5805044ddbc4",
          "number": "INV-002",
          "payments": [
            {
              "id": "90164a77-d517-4c27-8677-ac56a665cb9c",
              "amount": 500.0,
              "method": "credit_card"
            }
          ]
        }
      ]
    }
  }
}
```

## Available GraphQL Mutations

### Create entity

```graphql
mutation {
  createOrder(data: {
    number: "ORD-001"
    customerName: "John Doe"
    amount: 1000.0
    status: "active"
    notes: "First order"
  }) {
    id
    number
    customerName
    amount
    status
  }
}
```

### Update entity

```graphql
mutation {
  updateOrder(
    id: "UUID"
    data: {
      amount: 1500.0
      status: "completed"
    }
  ) {
    id
    amount
    status
  }
}
```

### Delete entity

```graphql
mutation {
  deleteOrder(id: "UUID")
}
```

Returns `true` if deleted successfully, `false` otherwise.

### Create and link entity

Create a new entity and automatically link it to a parent:

```graphql
mutation {
  createInvoiceForOrder(
    parentId: "ORDER_UUID"
    data: {
      number: "INV-001"
      amount: 500.0
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

### Link existing entities

Link two existing entities together:

```graphql
mutation {
  linkPaymentToInvoice(
    sourceId: "PAYMENT_UUID"
    targetId: "INVOICE_UUID"
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

### Unlink entities

Remove a link between two entities:

```graphql
mutation {
  unlinkPaymentFromInvoice(
    sourceId: "PAYMENT_UUID"
    targetId: "INVOICE_UUID"
  )
}
```

### Generic link mutations

#### Create a link

```graphql
mutation {
  createLink(
    sourceId: "UUID"
    targetId: "UUID"
    linkType: "has_invoice"
    metadata: {note: "Test link", priority: "high"}
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

**Without metadata**:
```graphql
mutation {
  createLink(
    sourceId: "UUID"
    targetId: "UUID"
    linkType: "has_invoice"
  ) {
    id
    linkType
  }
}
```

#### Delete a link

```graphql
mutation {
  deleteLink(id: "UUID")
}
```

Returns `true` if deleted, `false` otherwise.

## Examples with curl

### Get the SDL schema

```bash
curl http://127.0.0.1:3000/graphql/schema
```

Returns the complete schema in SDL format, including all available types, queries, and mutations.

### List orders

```bash
curl -X POST http://127.0.0.1:3000/graphql \
  -H "Content-Type: application/json" \
  -d '{"query": "query { orders(limit: 5) { id number customerName amount } }"}'
```

### Get an order with relations

```bash
# Get an ID from REST
ORDER_ID=$(curl -s http://127.0.0.1:3000/orders | jq -r '.data[0].id')

# GraphQL query with relations
curl -X POST http://127.0.0.1:3000/graphql \
  -H "Content-Type: application/json" \
  -d "{\"query\": \"query { order(id: \\\"$ORDER_ID\\\") { id number customerName invoices { id number amount payments { id amount method } } } }\"}"
```

### Get an invoice with payments

```bash
INVOICE_ID=$(curl -s http://127.0.0.1:3000/invoices | jq -r '.data[0].id')

curl -X POST http://127.0.0.1:3000/graphql \
  -H "Content-Type: application/json" \
  -d "{\"query\": \"query { invoice(id: \\\"$INVOICE_ID\\\") { id number amount payments { id amount method } } }\"}"
```

### Create an order

```bash
curl -X POST http://127.0.0.1:3000/graphql \
  -H "Content-Type: application/json" \
  -d '{
    "query": "mutation { createOrder(data: { number: \"ORD-999\", customerName: \"Jane Doe\", amount: 2000.0, status: \"active\", notes: \"Test order\" }) { id number customerName amount } }"
  }'
```

### Create and link invoice

```bash
ORDER_ID=$(curl -s http://127.0.0.1:3000/orders | jq -r '.data[0].id')

curl -X POST http://127.0.0.1:3000/graphql \
  -H "Content-Type: application/json" \
  -d "{\"query\": \"mutation { createInvoiceForOrder(parentId: \\\"$ORDER_ID\\\", data: { number: \\\"INV-999\\\", amount: 1500.0, status: \\\"pending\\\", dueDate: \\\"2024-12-31\\\" }) { id number amount } }\"}"
```

### Link entities

```bash
ORDER_ID=$(curl -s http://127.0.0.1:3000/orders | jq -r '.data[0].id')
INVOICE_ID=$(curl -s http://127.0.0.1:3000/invoices | jq -r '.data[0].id')

curl -X POST http://127.0.0.1:3000/graphql \
  -H "Content-Type: application/json" \
  -d "{\"query\": \"mutation { createLink(sourceId: \\\"$ORDER_ID\\\", targetId: \\\"$INVOICE_ID\\\", linkType: \\\"has_invoice\\\") { id linkType } }\"}"
```

## Architecture

This example combines two framework exposures:

1. **REST Exposure** (`RestExposure`): Provides the classic REST API
2. **GraphQL Exposure** (`GraphQLExposure`): Provides the GraphQL API

Both exposures share the same `ServerHost`, which contains:
- Link configuration
- Link service
- Entity registry
- Entity fetchers and creators

## Technical Notes

- The GraphQL schema is automatically generated from registered entities
- GraphQL types are specific to each entity (`Order`, `Invoice`, `Payment`) - not generic
- Fields are automatically discovered from entity data
- Relations are automatically added from `links.yaml` configuration
- Entity metadata is in JSON format for maximum flexibility
- Link metadata is also in JSON format
- GraphQL Playground is available for interactive testing

## ✅ Implemented Features

- ✅ Named queries by entity type (`order`, `invoice`, `payment`)
- ✅ List queries with pagination (`orders`, `invoices`, `payments`)
- ✅ Automatic relations between entities
- ✅ Nested navigation (relations of relations)
- ✅ Full CRUD mutations for entities (`createOrder`, `updateOrder`, `deleteOrder`)
- ✅ Link mutations (create, link, unlink)
- ✅ Specialized mutations (`createInvoiceForOrder`, `linkPaymentToInvoice`)
- ✅ Generic `createLink` and `deleteLink` mutations
- ✅ GraphQL Playground for interactive testing
- ✅ SDL schema export via `/graphql/schema`

## Current Limitations

- ⚠️ GraphQL Subscriptions not implemented
- ⚠️ Advanced filtering on list queries (to be added)
- ⚠️ Sorting on list queries (to be added)

## Next Steps

- [ ] Add advanced filtering support in GraphQL queries
- [ ] Add sorting capabilities
- [ ] Implement GraphQL Subscriptions for real-time updates
- [ ] Add field-level authorization policies
- [ ] Support for GraphQL directives (`@deprecated`, `@skip`, etc.)

