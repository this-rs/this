# Full API Example

## Description

Complete HTTP API example with Axum server, demonstrating:
- Entity CRUD operations via HTTP
- Link management via RESTful routes
- Auto-generated routes with `ServerBuilder`
- Module system with multi-entity support
- In-memory storage

## Structure

```
full_api/
└── main.rs    # Complete API in a single file (~300 lines)
```

## Running

```bash
cargo run --example full_api
```

The server will start on `http://localhost:3000`.

## What You'll Learn

- ✅ Build complete REST API with `ServerBuilder`
- ✅ Define multiple entities (User, Car, Company)
- ✅ Auto-generate CRUD routes
- ✅ Auto-generate link routes
- ✅ Module system with `EntityFetcher` and `EntityCreator`
- ✅ HTTP handlers with Axum
- ✅ Bidirectional link navigation

## Entities

### User
```rust
impl_data_entity!(User, "user", ["name", "email"], {
    email: String,
});
```

### Car
```rust
impl_data_entity!(Car, "car", ["name", "brand", "model"], {
    brand: String,
    model: String,
    year: i32,
});
```

### Company
```rust
impl_data_entity!(Company, "company", ["name", "registration_number"], {
    registration_number: String,
});
```

## API Routes

When you run the server, you'll see:

```
🚀 Full API Example - This-RS
🌐 Server running on http://127.0.0.1:3000

📚 Auto-generated Entity Routes:
  GET    /users                        - List all users
  POST   /users                        - Create a user
  GET    /users/{id}                   - Get a user
  PUT    /users/{id}                   - Update a user
  DELETE /users/{id}                   - Delete a user

  GET    /cars                         - List all cars
  POST   /cars                         - Create a car
  ... (same for companies)

🔗 Auto-generated Link Routes:
  GET    /users/{id}/cars-owned        - List cars owned by user
  POST   /users/{id}/cars-owned        - Create new car + link
  POST   /users/{id}/cars-owned/{car_id} - Link existing car
  GET    /cars/{id}/owner              - Get owner of car

  GET    /users/{id}/companies-owned   - List companies owned by user
  GET    /companies/{id}/owner         - Get owner of company
```

## Usage Examples

### Create Entities

```bash
# Create a user
curl -X POST http://localhost:3000/users \
  -H 'Content-Type: application/json' \
  -d '{
    "name": "Alice",
    "email": "alice@example.com",
    "status": "active"
  }'

# Create a car
curl -X POST http://localhost:3000/cars \
  -H 'Content-Type: application/json' \
  -d '{
    "name": "Tesla Model 3",
    "brand": "Tesla",
    "model": "Model 3",
    "year": 2023,
    "status": "active"
  }'
```

### Create Links

#### Method 1: Link Existing Entities
```bash
curl -X POST http://localhost:3000/users/{user_id}/cars-owned/{car_id} \
  -H 'Content-Type: application/json' \
  -d '{
    "metadata": {
      "purchase_date": "2024-01-15",
      "price": 45000
    }
  }'
```

#### Method 2: Create New Entity + Link
```bash
curl -X POST http://localhost:3000/users/{user_id}/cars-owned \
  -H 'Content-Type: application/json' \
  -d '{
    "entity": {
      "name": "BMW X5",
      "brand": "BMW",
      "model": "X5",
      "year": 2024,
      "status": "active"
    },
    "metadata": {
      "purchase_date": "2024-03-20",
      "price": 65000
    }
  }'
```

### Query Links

```bash
# Get all cars owned by a user (with full car data!)
curl http://localhost:3000/users/{user_id}/cars-owned | jq .

# Get owner of a car (with full user data!)
curl http://localhost:3000/cars/{car_id}/owner | jq .

# Get specific link with both entities
curl http://localhost:3000/users/{user_id}/cars-owned/{car_id} | jq .
```

### Update and Delete

```bash
# Update a user
curl -X PUT http://localhost:3000/users/{user_id} \
  -H 'Content-Type: application/json' \
  -d '{
    "name": "Alice Smith",
    "email": "alice.smith@example.com"
  }'

# Delete a link
curl -X DELETE http://localhost:3000/users/{user_id}/cars-owned/{car_id}

# Delete an entity
curl -X DELETE http://localhost:3000/cars/{car_id}
```

## Code Highlights

### ServerBuilder Magic

```rust
let app = ServerBuilder::new()
    .with_link_service(InMemoryLinkService::new())
    .register_module(app_module)?
    .build()?;
```

**This single call**:
- Registers all entities (User, Car, Company)
- Generates all CRUD routes
- Generates all link routes
- Collects EntityFetchers for link enrichment
- Collects EntityCreators for auto-creation
- Returns ready-to-use Axum Router

**Zero manual routing code!**

### Module Implementation

```rust
impl Module for AppModule {
    fn name(&self) -> &str { "full-api" }
    
    fn entity_types(&self) -> Vec<&str> {
        vec!["user", "car", "company"]
    }
    
    fn links_config(&self) -> Result<LinksConfig> {
        Ok(LinksConfig {
            entities: vec![
                EntityConfig { singular: "user".into(), plural: "users".into() },
                EntityConfig { singular: "car".into(), plural: "cars".into() },
                EntityConfig { singular: "company".into(), plural: "companies".into() },
            ],
            links: vec![
                LinkDefinition {
                    link_type: "owner".to_string(),
                    source_type: "user".to_string(),
                    target_type: "car".to_string(),
                    forward_route_name: "cars-owned".to_string(),
                    reverse_route_name: "owner".to_string(),
                    // ...
                },
                // More link definitions...
            ],
            // ...
        })
    }
    
    fn register_entities(&self, registry: &mut EntityRegistry) {
        registry.register(Box::new(UserDescriptor::new(self.user_store.clone())));
        registry.register(Box::new(CarDescriptor::new(self.car_store.clone())));
        registry.register(Box::new(CompanyDescriptor::new(self.company_store.clone())));
    }
    
    fn get_entity_fetcher(&self, entity_type: &str) -> Option<Arc<dyn EntityFetcher>> {
        match entity_type {
            "user" => Some(Arc::new(self.user_store.clone())),
            "car" => Some(Arc::new(self.car_store.clone())),
            "company" => Some(Arc::new(self.company_store.clone())),
            _ => None,
        }
    }
    
    fn get_entity_creator(&self, entity_type: &str) -> Option<Arc<dyn EntityCreator>> {
        match entity_type {
            "user" => Some(Arc::new(self.user_store.clone())),
            "car" => Some(Arc::new(self.car_store.clone())),
            "company" => Some(Arc::new(self.company_store.clone())),
            _ => None,
        }
    }
}
```

## Response Examples

### List Links with Enrichment

Request:
```bash
GET /users/abc-123/cars-owned
```

Response:
```json
{
  "links": [
    {
      "id": "link-uuid",
      "source_id": "abc-123",
      "target_id": "car-uuid",
      "target": {
        "id": "car-uuid",
        "type": "car",
        "name": "Tesla Model 3",
        "brand": "Tesla",
        "model": "Model 3",
        "year": 2023,
        "status": "active"
      },
      "metadata": {
        "purchase_date": "2024-01-15",
        "price": 45000
      }
    }
  ],
  "count": 1
}
```

### Create Entity + Link

Request:
```bash
POST /users/abc-123/cars-owned
Body: {
  "entity": { "name": "BMW X5", "brand": "BMW", "model": "X5", "year": 2024, "status": "active" },
  "metadata": { "purchase_date": "2024-03-20" }
}
```

Response:
```json
{
  "entity": {
    "id": "new-car-uuid",
    "type": "car",
    "name": "BMW X5",
    "brand": "BMW",
    "model": "X5",
    "year": 2024,
    "created_at": "2024-10-23T12:00:00Z",
    "updated_at": "2024-10-23T12:00:00Z",
    "status": "active"
  },
  "link": {
    "id": "new-link-uuid",
    "type": "link",
    "link_type": "owner",
    "source_id": "abc-123",
    "target_id": "new-car-uuid",
    "metadata": {
      "purchase_date": "2024-03-20"
    },
    "created_at": "2024-10-23T12:00:00Z",
    "updated_at": "2024-10-23T12:00:00Z"
  }
}
```

## Key Features Demonstrated

✅ **Auto-Generated Routes** - Zero manual routing  
✅ **Entity Macros** - Define entities in 4 lines  
✅ **Link Enrichment** - Full entity data in responses  
✅ **Bidirectional Navigation** - Query from either side  
✅ **EntityCreator** - Create entities with automatic linking  
✅ **Module System** - Clean organization  
✅ **Type Safety** - Full Rust compile-time checks  

## Next Steps

- Review the [Microservice Example](../microservice/README.md) for production patterns
- Read the [Architecture Guide](../../docs/architecture/ARCHITECTURE.md)
- Explore [Link Authorization](../../docs/guides/LINK_AUTHORIZATION.md)

---

**A complete, production-ready API in ~300 lines of code!** 🚀🦀✨
