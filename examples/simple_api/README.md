# Simple API Example

## Description

Basic example demonstrating the fundamental concepts of This-RS:
- Entity definition using macros
- Link creation between entities
- In-memory link service usage

## Structure

```
simple_api/
└── main.rs    # Complete example in a single file
```

## Running

```bash
cargo run --example simple_api
```

## What You'll Learn

- ✅ Define entities using `impl_data_entity!` macro
- ✅ Create entities with auto-generated IDs and timestamps
- ✅ Create links between entities
- ✅ Use `InMemoryLinkService` for storage
- ✅ Query links bidirectionally

## Code Overview

### Entity Definitions

```rust
use this::prelude::*;

// User entity with email field
impl_data_entity!(User, "user", ["name", "email"], {
    email: String,
});

// Car entity with brand, model, and year
impl_data_entity!(Car, "car", ["name", "brand", "model"], {
    brand: String,
    model: String,
    year: i32,
});
```

**What the macro generates**:
- All base Entity fields (id, type, created_at, updated_at, deleted_at, status)
- name field (from Data trait)
- Your custom fields
- Constructor: `User::new(name, status, email)`
- Utility methods: `soft_delete()`, `touch()`, `restore()`

### Creating Entities

```rust
// Create users
let alice = User::new(
    "Alice".to_string(),
    "active".to_string(),
    "alice@example.com".to_string(),
);

let bob = User::new(
    "Bob".to_string(),
    "active".to_string(),
    "bob@example.com".to_string(),
);

// Create cars
let tesla = Car::new(
    "Tesla Model 3".to_string(),
    "active".to_string(),
    "Tesla".to_string(),
    "Model 3".to_string(),
    2023,
);

let bmw = Car::new(
    "BMW X5".to_string(),
    "active".to_string(),
    "BMW".to_string(),
    "X5".to_string(),
    2024,
);
```

### Creating Links

```rust
// Initialize link service
let link_service = InMemoryLinkService::new();

// Create links: User owns Car
let link1 = LinkEntity::new(
    "owner",                    // link_type
    alice.id,                   // source_id (user)
    tesla.id,                   // target_id (car)
    None,                       // metadata
);
link_service.create(link1).await?;

let link2 = LinkEntity::new("owner", alice.id, bmw.id, None);
link_service.create(link2).await?;

let link3 = LinkEntity::new("owner", bob.id, bmw.id, None);
link_service.create(link3).await?;
```

### Querying Links

#### Find by Source (Forward)

```rust
// Find all cars owned by Alice
let alice_owned = link_service
    .find_by_source(&alice.id, Some("owner"), None)
    .await?;

println!("🚗 Alice owns {} car(s):", alice_owned.len());
for link in alice_owned {
    println!("  - Car ID: {}", link.target_id);
}
```

#### Find by Target (Reverse)

```rust
// Find all owners of BMW
let bmw_owners = link_service
    .find_by_target(&bmw.id, Some("owner"), None)
    .await?;

println!("👥 BMW is owned by {} person(s):", bmw_owners.len());
for link in bmw_owners {
    println!("  - Owner ID: {}", link.source_id);
}
```

## Output Example

```
🚀 Simple API Example - This-RS

👤 Created users:
  - Alice (alice@example.com)
  - Bob (bob@example.com)

🚗 Created cars:
  - Tesla Model 3 (2023)
  - BMW X5 (2024)

🔗 Created links:
  - Alice → Tesla Model 3 (owner)
  - Alice → BMW X5 (owner)
  - Bob → BMW X5 (owner)

📊 Query Results:

🚗 Alice owns 2 car(s):
  - Tesla Model 3
  - BMW X5

🚗 Bob owns 1 car(s):
  - BMW X5

👥 BMW X5 has 2 owner(s):
  - Alice
  - Bob

✅ Example completed successfully!
```

## Key Concepts

### 1. Macro-Driven Entities

```rust
impl_data_entity!(User, "user", ["name", "email"], {
    email: String,
});
```

This single line generates:
- Complete struct with all fields
- Entity and Data trait implementations
- Constructor and utility methods
- Serde serialization support

### 2. Automatic ID and Timestamp Generation

```rust
let user = User::new("Alice".to_string(), "active".to_string(), "alice@example.com".to_string());
// user.id is auto-generated (UUID)
// user.created_at is auto-set (current time)
// user.updated_at is auto-set (current time)
// user.entity_type is "user" (auto-set)
```

### 3. Polymorphic Links

```rust
let link = LinkEntity::new("owner", source_id, target_id, None);
```

Links are generic:
- Work with any entity types
- Support metadata (optional JSON)
- Bidirectional querying

### 4. In-Memory Storage

```rust
let link_service = InMemoryLinkService::new();
```

Perfect for:
- Development and testing
- Prototyping
- Learning the framework

For production, use:
- DynamoDB storage backend
- PostgreSQL (community contribution)
- Your custom storage implementation

## Next Steps

- Try the [Microservice Example](../microservice/README.md) for a complete API
- Read the [Getting Started Guide](../../docs/guides/GETTING_STARTED.md)
- Explore [Enriched Links](../../docs/guides/ENRICHED_LINKS.md)

---

**This example shows the core concepts with zero HTTP boilerplate!** 🚀🦀✨
