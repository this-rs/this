# GraphQL Implementation Architecture

This document explains the technical implementation of GraphQL exposure in `this-rs`, including the dynamic schema generation and custom executor.

## 📐 Overview

The GraphQL implementation is **completely modular** and separate from the core framework:

```
src/server/exposure/
├── rest/
│   └── mod.rs         # REST API exposure
└── graphql/
    ├── mod.rs         # GraphQL exposure entry point
    ├── schema.rs      # Legacy async-graphql schema (unused)
    ├── schema_generator.rs  # Dynamic SDL schema generator
    ├── dynamic_schema.rs    # Legacy dynamic schema (unused)
    └── executor/
        ├── mod.rs     # Executor module entry
        ├── core.rs    # Main executor orchestration
        ├── query_executor.rs    # Query resolution
        ├── mutation_executor.rs # CRUD mutations
        ├── link_mutations.rs    # Link-specific mutations
        ├── field_resolver.rs    # Field and relation resolution
        └── utils.rs   # Utility functions
```

## 🏗️ Architecture Components

### 1. GraphQLExposure

**Location**: `src/server/exposure/graphql/mod.rs`

The entry point that builds the GraphQL router. It's completely separate from REST exposure.

```rust
pub struct GraphQLExposure;

impl GraphQLExposure {
    pub fn build_router(host: Arc<ServerHost>) -> Result<Router> {
        Router::new()
            .route("/graphql", post(graphql_handler_custom))
            .route("/graphql/playground", get(graphql_playground))
            .route("/graphql/schema", get(graphql_dynamic_schema))
            .layer(Extension(host))
    }
}
```

**Endpoints**:
- `POST /graphql` - GraphQL query/mutation endpoint
- `GET /graphql/playground` - Interactive GraphQL playground
- `GET /graphql/schema` - SDL schema export

### 2. SchemaGenerator

**Location**: `src/server/exposure/graphql/schema_generator.rs`

Dynamically generates GraphQL SDL (Schema Definition Language) from:
- Registered entities in `ServerHost`
- Field discovery via `EntityFetcher::get_sample_entity()` or `list_as_json()`
- Relations from `links.yaml` configuration

**Key Methods**:
- `generate_sdl()` - Orchestrates full schema generation
- `generate_entity_type()` - Generates type definition for an entity
- `generate_query_root()` - Generates Query type with all entity queries
- `generate_mutation_root()` - Generates Mutation type with CRUD and link operations
- `get_relations_for()` - Extracts relations for an entity from config

**Example Output**:
```graphql
type Order {
  id: ID!
  number: String!
  customerName: String!
  amount: Float!
  invoices: [Invoice!]!  # From links.yaml
}

type Query {
  order(id: ID!): Order
  orders(limit: Int, offset: Int): [Order!]!
}

type Mutation {
  createOrder(data: JSON!): Order!
  updateOrder(id: ID!, data: JSON!): Order!
  deleteOrder(id: ID!): Boolean!
  createInvoiceForOrder(parentId: ID!, data: JSON!): Invoice!
}
```

### 3. GraphQLExecutor

**Location**: `src/server/exposure/graphql/executor/core.rs`

A **custom GraphQL executor** that:
- Parses incoming GraphQL queries using `graphql-parser`
- Executes queries against the dynamic schema
- Resolves fields dynamically using `EntityFetcher` and `EntityCreator`
- Handles relations via `LinkService`

**Why Custom?**: `async-graphql` requires compile-time type definitions. Our dynamic schema requires runtime execution, so we implemented a custom executor.

**Structure**:
```rust
pub struct GraphQLExecutor {
    host: Arc<ServerHost>,
    schema_sdl: String,  // Generated SDL (currently unused but stored)
}

impl GraphQLExecutor {
    pub async fn execute(&self, query: &str, variables: Option<HashMap<String, Value>>) -> Result<Value>;
    async fn execute_document(&self, doc: &Document, variables: HashMap<String, Value>) -> Result<Value>;
    async fn execute_query(&self, selections: &[Selection], variables: &HashMap<String, Value>) -> Result<Value>;
    async fn execute_mutation(&self, selections: &[Selection], variables: &HashMap<String, Value>) -> Result<Value>;
}
```

### 4. Query Executor

**Location**: `src/server/exposure/graphql/executor/query_executor.rs`

Resolves GraphQL queries:

- **Plural queries** (`orders`, `invoices`): Calls `EntityFetcher::list_as_json()` with pagination
- **Singular queries** (`order`, `invoice`): Calls `EntityFetcher::fetch_as_json()` with UUID

```rust
pub async fn resolve_query_field(
    host: &Arc<ServerHost>,
    field: &Field<'_, String>,
) -> Result<Value> {
    // Check if plural query
    if let Some(entity_type) = get_entity_type_from_plural(host, field_name) {
        let entities = fetcher.list_as_json(limit, offset).await?;
        // Resolve sub-fields...
    }
    
    // Check if singular query
    if let Some(entity_type) = get_entity_type_from_singular(host, field_name) {
        let entity = fetcher.fetch_as_json(&uuid).await?;
        // Resolve sub-fields...
    }
}
```

### 5. Mutation Executor

**Location**: `src/server/exposure/graphql/executor/mutation_executor.rs`

Handles all CRUD mutations:

- `create{Entity}` - Calls `EntityCreator::create_from_json()`
- `update{Entity}` - Calls `EntityCreator::update_from_json()`
- `delete{Entity}` - Calls `EntityCreator::delete()`

Dispatches to specialized modules for link mutations.

### 6. Link Mutations

**Location**: `src/server/exposure/graphql/executor/link_mutations.rs`

Specialized mutations for link management:

- `createLink` - Generic link creation
- `deleteLink` - Generic link deletion
- `create{Target}For{Source}` - Create entity + link (e.g., `createInvoiceForOrder`)
- `link{Target}To{Source}` - Link existing entities (e.g., `linkPaymentToInvoice`)
- `unlink{Target}From{Source}` - Remove link (e.g., `unlinkPaymentFromInvoice`)

**Convention**: Mutation names follow patterns:
- `create{Target}For{Source}` → Creates target, links to source
- `link{Source}To{Target}` → Links source to target
- `unlink{Source}From{Target}` → Removes link from source to target

### 7. Field Resolver

**Location**: `src/server/exposure/graphql/executor/field_resolver.rs`

Resolves entity fields and relations:

- **Direct fields**: Extracts from JSON entity data
- **Relations**: Uses `LinkService` to find links, then `EntityFetcher` to resolve entities
- **Recursion**: Uses `BoxFuture` to handle nested selections recursively

**Key Functions**:
```rust
pub async fn resolve_entity_fields(
    host: &Arc<ServerHost>,
    entity: Value,
    selections: &[Selection],
    entity_type: &str,
) -> Result<Value>

async fn resolve_relation_field_inner(
    host: &Arc<ServerHost>,
    entity: &serde_json::Map<String, Value>,
    field: &Field,
    entity_type: &str,
) -> Result<Option<Value>>
```

**Relation Resolution Logic**:
1. Check if field name matches `forward_route_name` in links config → Forward relation
2. Check if field name matches `reverse_route_name` in links config → Reverse relation
3. Use `LinkService::find_by_source()` or `find_by_target()` to get links
4. Fetch linked entities via `EntityFetcher::fetch_as_json()`
5. Recursively resolve nested selections

## 🔄 Execution Flow

### Query Execution

```
1. HTTP Request → POST /graphql
   ↓
2. graphql_handler_custom() → Creates GraphQLExecutor
   ↓
3. GraphQLExecutor::execute()
   ↓ Parse query with graphql-parser
   ↓
4. execute_document() → Detect operation type
   ↓
5. execute_query() → For each field
   ↓
6. query_executor::resolve_query_field()
   ↓ Identify entity type (plural/singular)
   ↓
7. EntityFetcher::list_as_json() or fetch_as_json()
   ↓
8. field_resolver::resolve_entity_fields()
   ↓ For each selection
   ↓
9a. Direct field → Extract from JSON
9b. Relation field → resolve_relation_field_inner()
   ↓
10. LinkService::find_by_source() / find_by_target()
    ↓
11. EntityFetcher::fetch_as_json() for each linked entity
    ↓
12. Recursive resolve_entity_fields() for nested selections
    ↓
13. Return resolved JSON
```

### Mutation Execution

```
1. HTTP Request → POST /graphql (mutation)
   ↓
2. GraphQLExecutor::execute()
   ↓
3. execute_mutation() → For each field
   ↓
4. mutation_executor::resolve_mutation_field()
   ↓ Dispatch by mutation name pattern
   ↓
5a. CRUD mutation → mutation_executor::create/update/delete_entity_mutation()
    ↓ EntityCreator::create_from_json() / update_from_json() / delete()
    
5b. Link mutation → link_mutations::*_mutation()
    ↓ LinkService::create() / delete()
    ↓ LinkService::find_by_source() (for unlink)
    
6. field_resolver::resolve_entity_fields() → Resolve sub-selections
   ↓
7. Return resolved entity/link
```

## 🎯 Design Decisions

### Why Custom Executor Instead of async-graphql?

**Problem**: `async-graphql` requires compile-time type definitions. Our entities are defined at runtime.

**Solution**: Custom executor that:
- Parses queries at runtime using `graphql-parser`
- Resolves fields dynamically using runtime services
- Works with any entity type without code generation

**Trade-offs**:
- ✅ 100% dynamic, no code generation needed
- ✅ Works with any entity automatically
- ❌ More complex than using async-graphql
- ❌ Manual query parsing and execution

### Why JSON for Mutation Data?

**Decision**: Use `JSON!` scalar for mutation `data` argument instead of strongly-typed input types.

**Rationale**:
- Entities are defined at runtime
- Cannot generate input types at compile time
- JSON provides flexibility for any entity structure
- Matches REST API patterns

**Trade-offs**:
- ✅ Maximum flexibility
- ✅ No code generation needed
- ❌ Less type safety in GraphQL schema
- ❌ No autocomplete for data structure

### Why Separate Executor Modules?

**Decision**: Split executor into 6 modules (core, query, mutation, links, fields, utils).

**Rationale**:
- Original `executor.rs` was 751 lines
- Better maintainability and testability
- Clear separation of concerns
- Easier to add features

**Structure**:
- `core.rs` (~122 lines) - Orchestration
- `query_executor.rs` (~96 lines) - Query resolution
- `mutation_executor.rs` (~149 lines) - CRUD mutations
- `link_mutations.rs` (~241 lines) - Link operations
- `field_resolver.rs` (~177 lines) - Field/relation resolution
- `utils.rs` (~132 lines) - Utilities

## 🔧 Extension Points

### Adding New Query Types

1. Add query to `SchemaGenerator::generate_query_root()`
2. Add resolver in `query_executor.rs`

### Adding New Mutation Types

1. Add mutation to `SchemaGenerator::generate_mutation_root()`
2. Add handler in `mutation_executor.rs` or `link_mutations.rs`

### Adding New Field Resolvers

1. Extend `field_resolver.rs` with new resolution logic
2. Update `resolve_entity_fields_impl()` to handle new field types

## 📊 Performance Considerations

### Schema Generation

**Current**: Schema is generated on each request to `/graphql/schema`.

**Future Optimization**: Cache generated SDL and invalidate when entities change.

### Query Execution

**Current**: Executor created per request.

**Future Optimization**: Cache executor instance (schema doesn't change at runtime).

### Field Resolution

**Current**: Sequential fetching of related entities.

**Future Optimization**: Batch fetching using DataLoader pattern.

### Nested Queries

**Current**: Recursive resolution may fetch same entity multiple times.

**Future Optimization**: Add query depth limit and entity fetching cache.

## 🧪 Testing

Each executor module can be tested independently:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_utils_pluralize() {
        assert_eq!(utils::pluralize("order"), "orders");
        assert_eq!(utils::pluralize("company"), "companies");
    }
    
    #[tokio::test]
    async fn test_query_resolution() {
        let host = create_test_host();
        let field = parse_query_field("orders");
        let result = query_executor::resolve_query_field(&host, &field).await?;
        // Assert result...
    }
}
```

## 🔮 Future Enhancements

### Planned Features

1. **Schema Caching**: Cache generated SDL
2. **Executor Caching**: Reuse executor instance
3. **DataLoader**: Batch entity fetching
4. **Query Complexity Analysis**: Prevent expensive queries
5. **Field-Level Authorization**: Integrate with auth system
6. **Subscriptions**: WebSocket support for real-time updates
7. **Directives Support**: `@deprecated`, `@skip`, `@include`
8. **Input Type Generation**: Strongly-typed input types (if possible)

### Technical Debt

1. **Legacy Files**: `schema.rs` and `dynamic_schema.rs` are unused (can be removed)
2. **Error Handling**: More structured GraphQL errors
3. **Performance**: Add benchmarks and optimize hot paths
4. **Documentation**: Add inline documentation for complex logic

## 📚 Related Files

- [GraphQL Guide](../guides/GRAPHQL.md) - User guide
- [Architecture Overview](./ARCHITECTURE.md) - Overall framework architecture
- [Server Builder](./SERVER_BUILDER_IMPLEMENTATION.md) - Server construction
- [GraphQL Example](../../examples/microservice/README_GRAPHQL.md) - Complete example

## 🎯 Summary

The GraphQL implementation is:

- ✅ **100% Dynamic** - No compile-time code generation
- ✅ **Modular** - Clean separation of concerns
- ✅ **Extensible** - Easy to add new features
- ✅ **Type-Safe** - Uses Rust types internally
- ✅ **Performant** - Efficient execution with room for optimization

**Key Innovation**: Custom executor that enables truly dynamic GraphQL without sacrificing type safety or developer experience.

