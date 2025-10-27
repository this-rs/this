# Pagination and Filtering

## Overview

This framework now supports automatic pagination and filtering for all list endpoints. This ensures that:

1. **No results flooding** - Pagination is always applied by default
2. **Generic filters** - Filter any field with comparison operators
3. **Flexible sorting** - Sort by any field, ascending or descending
4. **Works for entities and links** - Same pattern everywhere

## 🚀 Quick Start

### Basic Pagination

```bash
# Page 1 with default limit (20)
GET /orders

# Page 2 with 10 items per page
GET /orders?page=2&limit=10
```

### Filtering

```bash
# Exact match
GET /orders?filter={"status": "active"}

# Comparisons
GET /orders?filter={"amount>": 1000, "amount<": 5000}

# Multiple conditions (AND)
GET /orders?filter={"status": "active", "customer_name": "Acme Corp"}
```

### Sorting

```bash
# Ascending (default)
GET /orders?sort=amount:asc

# Descending
GET /orders?sort=created_at:desc

# Without explicit direction (ascending by default)
GET /orders?sort=number
```

### Combined

```bash
GET /orders?page=1&limit=20&filter={"status": "active", "amount>": 100}&sort=amount:desc
```

## 📋 Query Parameters

### `page` (optional, default: 1)

Page number starting from 1.

```bash
?page=2
```

### `limit` (optional, default: 20, max: 100)

Number of items per page. Defaults to 20, maximum 100.

```bash
?limit=10
```

### `filter` (optional)

JSON object with filter criteria.

**Supported operators:**
- `{"field": "value"}` - Exact match
- `{"field>": value}` - Greater than
- `{"field<": value}` - Less than
- `{"field>=": value}` - Greater or equal
- `{"field<=": value}` - Less or equal

```bash
# URL encoded JSON
?filter=%7B%22status%22%3A%20%22active%22%7D

# Pretty-printed for readability
?filter={"status": "active", "amount>": 100}
```

### `sort` (optional)

Field name with optional direction.

**Format:** `field[:asc|:desc]`

```bash
?sort=amount:desc
?sort=created_at:asc
?sort=number  # ascending by default
```

## 📊 Response Format

All list endpoints return a paginated response:

```json
{
  "data": [
    {
      "id": "...",
      "type": "order",
      "number": "ORD-001",
      "amount": 1500.00,
      "status": "active",
      ...
    },
    ...
  ],
  "pagination": {
    "page": 1,
    "limit": 20,
    "total": 145,
    "total_pages": 8,
    "has_next": true,
    "has_prev": false
  }
}
```

## 🎯 Examples by Entity

### Orders

```bash
# List all orders (paginated)
GET /orders

# Filter by status
GET /orders?filter={"status": "active"}

# Filter by amount range
GET /orders?filter={"amount>": 1000, "amount<": 5000}

# Filter by customer
GET /orders?filter={"customer_name": "Acme Corp"}

# Sort by amount descending
GET /orders?sort=amount:desc

# Combine everything
GET /orders?page=1&limit=10&filter={"status": "active", "amount>": 1000}&sort=created_at:desc
```

### Invoices

```bash
# List all invoices
GET /invoices

# Filter by status
GET /invoices?filter={"status": "paid"}

# Filter unpaid invoices above threshold
GET /invoices?filter={"status": "sent", "amount>": 500}
```

### Payments

```bash
# List all payments
GET /payments

# Filter by method
GET /payments?filter={"method": "card"}

# Filter by amount
GET /payments?filter={"amount>": 100}
```

## 🔗 Link Endpoints

Pagination and filtering work for link endpoints too!

```bash
# List links with pagination
GET /orders/{order_id}/invoices?page=1&limit=5

# Filter linked entities
GET /orders/{order_id}/invoices?filter={"status": "paid"}

# Sort linked entities
GET /orders/{order_id}/invoices?sort=amount:desc
```

## ⚙️ Implementation

### Adding to Your Stores

Implement the `QueryableStore` trait:

```rust
use this::prelude::QueryableStore;
use serde_json::Value;

impl QueryableStore<Order> for OrderStore {
    fn apply_filters(&self, data: Vec<Order>, filter: &Value) -> Vec<Order> {
        // Apply your filter logic
        data
    }
    
    fn apply_sort(&self, mut data: Vec<Order>, sort: &str) -> Vec<Order> {
        // Apply your sort logic
        data
    }
    
    fn list_all(&self) -> Vec<Order> {
        self.list()
    }
}
```

### Updating Handlers

```rust
use axum::extract::Query;
use this::prelude::*;

pub async fn list_orders(
    State(state): State<OrderAppState>,
    Query(params): Query<QueryParams>,
) -> Json<PaginatedResponse<Value>> {
    let page = params.page();
    let limit = params.limit();
    
    // Get all orders
    let mut orders = state.store.list();
    
    // Apply filters if provided
    if let Some(filter) = params.filter_value() {
        orders = state.store.apply_filters(orders, &filter);
    }
    
    // Apply sort if provided
    if let Some(sort) = &params.sort {
        orders = state.store.apply_sort(orders, sort);
    }
    
    let total = orders.len();
    
    // ALWAYS paginate
    let start = (page - 1) * limit;
    let paginated: Vec<Value> = orders
        .into_iter()
        .skip(start)
        .take(limit)
        .map(|order| serde_json::to_value(order).unwrap())
        .collect();
    
    Json(PaginatedResponse {
        data: paginated,
        pagination: PaginationMeta::new(page, limit, total),
    })
}
```

## ⚠️ Important Notes

1. **Pagination is ALWAYS applied** - Even without `page` or `limit` parameters, pagination defaults are used
2. **Maximum limit** - Can't exceed 100 items per page (prevents accidental memory exhaustion)
3. **Filter format** - Use URL-encoded JSON for complex filters
4. **Sort format** - Use `field:asc` or `field:desc`, or just `field` (defaults to ascending)

## ✅ Best Practices

1. **Always use pagination** - Don't return all results without pagination
2. **Index filtered fields** - In production with DynamoDB, create GSIs for frequently filtered fields
3. **Combine filters logically** - Multiple filters use AND logic
4. **Use appropriate limits** - Don't make limits too large (memory concerns)

## 🎉 Benefits

- ✅ **Prevents memory issues** - No accidental loading of massive datasets
- ✅ **Generic** - Works for all entities and links
- ✅ **Flexible** - Filter and sort on any field
- ✅ **Efficient** - Filters applied before pagination
- ✅ **RESTful** - Follows standard REST patterns

