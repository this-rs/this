# Multi-Tenancy Support

## Overview

`this-rs` provides **optional, opt-in multi-tenancy support** at the framework level. By default, all entities work in single-tenant mode, but you can enable multi-tenant isolation when needed.

## Key Features

✅ **Backward Compatible** - Existing code works without modification  
✅ **Opt-in Design** - Enable multi-tenancy only where needed  
✅ **Type-Safe** - Enforced at compile time via trait methods  
✅ **Zero-Cost** - No overhead when not used  

## Architecture

### Entity Trait

The base `Entity` trait includes an optional `tenant_id()` method with a default implementation:

```rust
pub trait Entity: Clone + Send + Sync + 'static {
    // ... other methods ...
    
    /// Get the tenant ID for multi-tenant isolation.
    ///
    /// Returns None by default for single-tenant applications or system-wide entities.
    fn tenant_id(&self) -> Option<Uuid> {
        None  // Default implementation
    }
}
```

### Default Behavior (Single-Tenant)

By default, all entities return `None` for `tenant_id()`:

```rust
impl_data_entity!(
    Order,
    "order",
    ["name", "number"],
    {
        number: String,
        amount: f64,
    }
);

let order = Order::new(/* ... */);
assert!(order.tenant_id().is_none());  // ✅ No tenant by default
```

### Opt-In Multi-Tenancy

To enable multi-tenancy for a specific entity, override the `tenant_id()` method:

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MultiTenantOrder {
    // Standard entity fields
    pub id: Uuid,
    pub entity_type: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub deleted_at: Option<DateTime<Utc>>,
    pub status: String,
    pub name: String,
    
    // Add tenant_id field
    pub tenant_id: Uuid,  // Required for this entity
    
    // Business-specific fields
    pub number: String,
    pub amount: f64,
}

impl Entity for MultiTenantOrder {
    // ... implement required methods ...
    
    fn tenant_id(&self) -> Option<Uuid> {
        Some(self.tenant_id)  // Override to return actual tenant
    }
}
```

## LinkEntity Multi-Tenancy

`LinkEntity` has built-in support for multi-tenancy:

### Creating Links Without Tenant

```rust
use this::core::link::LinkEntity;

let link = LinkEntity::new(
    "has_invoice",
    order_id,
    invoice_id,
    None,
);

assert!(link.tenant_id.is_none());  // ✅ No tenant
```

### Creating Links With Tenant

```rust
let tenant_id = Uuid::new_v4();

let link = LinkEntity::new_with_tenant(
    tenant_id,
    "has_invoice",
    order_id,
    invoice_id,
    None,
);

assert_eq!(link.tenant_id, Some(tenant_id));  // ✅ Tenant set
```

## DynamoDB Integration

When using DynamoDB with multi-tenant tables, the `tenant_id` field is automatically serialized:

```rust
pub struct DynamoDBLinkService {
    client: DynamoDBClient,
    table_name: String,
}

// The tenant_id field is automatically included in DynamoDB items
impl LinkService for DynamoDBLinkService {
    async fn create(&self, link: LinkEntity) -> Result<LinkEntity> {
        let item = self.link_to_item(&link).await?;
        // item will include tenant_id if set
        // ...
    }
}
```

### Partition Key Strategy

For multi-tenant DynamoDB tables, use `tenant_id` as the partition key:

```
Table: links
Partition Key: tenant_id (String)
Sort Key: id (String)

Table: users
Partition Key: tenant_id (String)
Sort Key: id (String)
```

This ensures tenant isolation at the storage level.

### Efficient Queries with `list_by_tenant()`

`DynamoDBDataService` and `DynamoDBLinkService` provide efficient Query-based methods for multi-tenant tables:

#### For Data Entities

```rust
use this::storage::DynamoDBDataService;

let service = DynamoDBDataService::<User>::new(client, "users".to_string());

// Efficient Query (uses tenant_id as partition key)
let users = service.list_by_tenant(&tenant_id).await?;

// Or with a GSI
let users = service.list_by_tenant_gsi(&tenant_id, "tenant_id-index").await?;
```

#### For Links

```rust
use this::storage::DynamoDBLinkService;

let link_service = DynamoDBLinkService::new(client, "links".to_string());

// List all links for a tenant (Query, not Scan!)
let links = link_service.list_links_by_tenant(&tenant_id).await?;

// Find links by source within a tenant
let order_links = link_service
    .find_by_source_and_tenant(&tenant_id, &order_id, Some("has_invoice"))
    .await?;
```

**Performance Comparison:**

| Operation | Old Method | New Method | Performance |
|-----------|-----------|------------|-------------|
| List all links | `list()` + filter (Scan) | `list_by_tenant()` (Query) | **100-1000x faster** |
| Find by source | `find_by_source()` (Scan) | `find_by_source_and_tenant()` (Query) | **10-100x faster** |

**Why Query is Better:**

- ✅ **Faster**: Only reads items for the specific tenant
- ✅ **Cheaper**: Lower RCU consumption
- ✅ **Scalable**: Performance doesn't degrade with table size
- ✅ **Predictable**: Consistent latency regardless of data volume

## Authentication Context Integration

Combine with `AuthContext` for automatic tenant extraction:

```rust
use this::prelude::*;

async fn create_link_handler(
    auth: AuthContext,
    payload: CreateLinkPayload,
) -> Result<LinkEntity> {
    let tenant_id = auth.tenant_id()
        .ok_or_else(|| anyhow!("Missing tenant context"))?;
    
    let link = LinkEntity::new_with_tenant(
        tenant_id,
        payload.link_type,
        payload.source_id,
        payload.target_id,
        payload.metadata,
    );
    
    link_service.create(link).await
}
```

## JSON Serialization

The `tenant_id` field uses `skip_serializing_if` to keep JSON clean:

### Without Tenant
```json
{
  "id": "123e4567-e89b-12d3-a456-426614174000",
  "type": "link",
  "link_type": "has_invoice",
  "source_id": "...",
  "target_id": "...",
  "status": "active"
}
```

### With Tenant
```json
{
  "id": "123e4567-e89b-12d3-a456-426614174000",
  "type": "link",
  "tenant_id": "789e4567-e89b-12d3-a456-426614174999",
  "link_type": "has_invoice",
  "source_id": "...",
  "target_id": "...",
  "status": "active"
}
```

## Migration Guide

### Existing Single-Tenant Applications

**No changes required!** Your code continues to work as-is.

### Enabling Multi-Tenancy

1. **Add tenant_id field** to your entity structs
2. **Override tenant_id()** method in Entity implementation
3. **Update constructors** to accept tenant_id parameter
4. **Use new_with_tenant()** for LinkEntity creation

### Example Migration

Before:
```rust
let link = LinkEntity::new("owner", user_id, car_id, None);
```

After (multi-tenant):
```rust
let tenant_id = auth.tenant_id().unwrap();
let link = LinkEntity::new_with_tenant(tenant_id, "owner", user_id, car_id, None);
```

## Best Practices

### ✅ Do

- Use `new_with_tenant()` in multi-tenant applications
- Extract `tenant_id` from authentication context
- Use `tenant_id` as DynamoDB partition key
- Keep single-tenant applications simple (don't add tenant_id)

### ❌ Don't

- Don't add tenant_id if you don't need multi-tenancy
- Don't trust client-provided tenant_id (always use auth context)
- Don't mix tenants in queries (use proper filtering)
- Don't forget to set tenant_id in multi-tenant mode

## Testing

### Single-Tenant Tests
```rust
#[test]
fn test_link_without_tenant() {
    let link = LinkEntity::new("owner", uuid1, uuid2, None);
    assert!(link.tenant_id.is_none());
}
```

### Multi-Tenant Tests
```rust
#[test]
fn test_link_with_tenant() {
    let tenant_id = Uuid::new_v4();
    let link = LinkEntity::new_with_tenant(
        tenant_id,
        "owner",
        uuid1,
        uuid2,
        None,
    );
    assert_eq!(link.tenant_id, Some(tenant_id));
}
```

## Performance Considerations

- **Single-Tenant**: Zero overhead, no extra field checks
- **Multi-Tenant**: Minimal overhead, optional UUID field
- **DynamoDB**: Efficient partitioning by tenant_id
- **Serialization**: `skip_serializing_if` reduces JSON size

## Summary

Multi-tenancy in `this-rs` is:

1. **Optional** - Only use it if you need it
2. **Simple** - Just add a field and override a method
3. **Safe** - Type-checked at compile time
4. **Efficient** - Zero-cost when not used
5. **Compatible** - Existing code keeps working

This design follows Rust's principle: **pay for what you use**.

