# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added
- **Engine-agnostic entity identity** (`obrain` feature, `this::core::identity` + `this::storage::obrain_identity`) — the *same entity whatever the engine* wrote it: the `_uuid`/`_data` convention first, then derived identity (uuid v8) for stores with flat properties. Measured on real bases: **0 resolved by convention / 39 by derived identity** on aletheia's `entity.obrain`, **1,553,071** on the Linux kernel graph. IDs are recomputable by a third party (`canonical_bytes_hex`), and `tests/identity_conformance.rs` locks the non-regression: a store written by this-rs yields **exactly** the same identifiers as before.
- **Write fencing and epochs** (`this::storage::writer_lease`) — a second writer is **refused**, including *in the same process* (obrain returns an existing lock part, so two `ObrainDB::open()` of one directory both succeed). Checked at operation **entry**, not at commit: obrain writes the WAL before the mmap mutation. A stale writer can no longer write but can still read; the epoch only ever grows. Frame format frozen by test (128 bytes, `THISLSE1`, CRC32).
- **Durable ordered journal** (`this::events::log_obrain`) — `EventLog` over obrain's journal zone: re-sorted and de-duplicated on read (the engine returns *physical* order), idempotent by `seq`, durable across reopen, per-consumer positions, replicas carrying their source positions.
- **Policy enforcement, armed by default** (`this::server::middleware::authorization`) — declared `auth:` policies are now **applied**. Route → entity → operation → policy resolution, fail-closed (an unreadable policy denies), abstention on unknown routes (never an invented policy), `403` on refusal with `{scope, policy, origin, reason}`. Opt out explicitly with `ServerBuilder::without_authorization()` (logged as a warning). **Breaking for deployments that relied on open access** — which is the point: the default must be safe.
- **Materialised IAM decisions** (`this::storage::iam`) — the decision is an indexed property node per (principal, scope): **7.96 µs** against 115.25 µs by traversal (**×14**), batched writes **×20** cheaper than one commit per principal. Policies spelled `perm:<permission>` are answered by that decision, composed with the existing vocabulary.
- **Turn-key obrain attachment** — `ServerBuilder::with_obrain_store(store)`: one call wires the relief (`GET /graph`), identity, the IAM plan, policy enforcement and — on a writable store — the write lease. Fencing is **fail-closed at construction**: if the lease cannot be taken, `build_host()` fails with the shard named and the escape hatch (`without_write_fencing()`). A read-only store never fences.
- **`GraphBackend` — the interface switch** (`this::storage::GraphBackend`) — one object-safe trait implemented by **both** `obrain_engine::ObrainDB` and `ObrainStore`, so a consumer's graph code runs unchanged on either (`pages(store.db())` / `pages(&obrain_store)`) and gains identity, IAM, fencing and journal by switching implementation.
- **Self-describing relief** — `GET /graph` publishes identity, lease, journal and IAM state, and **says what is missing** ("fencing NOT armed", "no transport") instead of looking complete; `GET /graph/identity` publishes the identity chain and a bounded `?counts=true`.
- **Measured latency contract** (`examples/latency_contract`) — in-process **230 ns** (hot lease check) to **5.77 µs** (IAM decision); HTTP **63.75 µs** to **184.94 µs**; journal append **2.90 ms** (durability, not a hot path).
- **vite-rs frontend hosting** (`frontend` feature, `this::server::frontend`) — the API process can serve the project SPA itself, reusing vite-rs serving rules (statics/ETag/304/MIME/SPA fallback) for both dev and embedded production assets; dev mode adds the vite-rs hot-reload loop, browser capture and the `/__dev` AI command bridge (`DevState`), activated via `SERVE_FRONTEND`/`THIS_*` env vars (`attach_from_env`).
- **Obrain engine re-coupled to this-rs/obrain (banc-obrain) checkout** — the durable cognitive graph engine (substrate + `EveryCommit` fsync WAL); `delete_by_entity` now ends in a WAL commit, and reopen-persistence tests cover data, links, search, post-reopen mutations and batch deletes.

### Planned
- ScyllaDB storage backend
- PostgreSQL storage backend
- GraphQL subscriptions support
- gRPC API exposure
- Webhook system for entity events
- Performance optimizations for large datasets

---

## [0.0.6] - 2025-01-29

### Added - GraphQL API Support 🎉

#### Core GraphQL Features
- **Dynamic GraphQL Schema Generation** - Automatic schema from entity definitions
- **Specific Entity Types** - Each entity gets its own GraphQL type (`Order`, `Invoice`, `Payment`)
- **Full CRUD Operations** - Complete Create, Read, Update, Delete via GraphQL
- **Automatic Relations** - Relations discovered from `links.yaml` configuration
- **Nested Query Support** - Query entities with their relations recursively

#### GraphQL Endpoints
- `POST /graphql` - Main GraphQL endpoint
- `GET /graphql/playground` - Interactive GraphQL playground
- `GET /graphql/schema` - SDL schema introspection endpoint

#### GraphQL Queries
- List queries with pagination (`orders(limit: Int, offset: Int)`)
- Single entity queries (`order(id: ID!)`)
- Automatic relation resolution (`order.invoices`, `invoice.payments`)

#### GraphQL Mutations
- CRUD mutations (`createOrder`, `updateOrder`, `deleteOrder`)
- Specialized link mutations (`createInvoiceForOrder`, `linkPaymentToInvoice`)
- Generic link mutations (`createLink`, `deleteLink`)

#### Implementation Details
- **Custom GraphQL Executor** - Runtime query execution with dynamic field resolution
- **Schema Generator** - Dynamically creates SDL from entities and links
- **Modular Architecture** - Executor split into 6 sub-modules for maintainability:
  - `core.rs` - Main executor orchestration
  - `query_executor.rs` - Query resolution logic
  - `mutation_executor.rs` - Mutation resolution logic
  - `link_mutations.rs` - Link-specific mutations
  - `field_resolver.rs` - Field and relation resolution
  - `utils.rs` - Utility functions

#### Multi-Protocol Architecture
- **ServerHost** - Transport-agnostic core for multi-protocol support
- **Exposure Modules** - Separate modules for REST and GraphQL
- **RestExposure** - REST API exposure implementation
- **GraphQLExposure** - GraphQL API exposure implementation
- Feature flag `graphql` for optional GraphQL support

### Changed
- **ServerBuilder** - Added `build_host()` method for transport-agnostic host creation
- **EntityFetcher trait** - Added `get_sample_entity()` and `list_as_json()` for schema introspection
- **EntityCreator trait** - Added `update_from_json()` and `delete()` for full CRUD support

### Documentation
- New comprehensive [GraphQL Guide](docs/guides/GRAPHQL.md)
- New [GraphQL Implementation](docs/architecture/GRAPHQL_IMPLEMENTATION.md) architecture doc
- Updated [README](README.md) with GraphQL examples
- Updated [microservice example](examples/microservice/) with GraphQL support
- New [GraphQL microservice README](examples/microservice/README_GRAPHQL.md)

### Dependencies
- Added `graphql-parser = "0.4"` for custom executor
- Added `futures = "0.3"` for async recursion
- Updated `async-graphql = "7"` and `async-graphql-axum = "7"`

---

## [0.0.5] - 2024-12-15

### Added - Validation and Filtering

#### Validation System
- **`impl_data_entity_validated!` Macro** - Extended entity macro with validation support
- **`Validated<T>` Extractor** - Axum extractor for automatic validation
- **Reusable Validators**:
  - `required` - Field must be present and non-empty
  - `optional` - Field is optional
  - `positive` - Numeric field must be > 0
  - `string_length(min, max)` - String length constraints
  - `max_value(n)` - Maximum numeric value
  - `in_list([...])` - Value must be in list
  - `date_format(format)` - Date string format validation

#### Filtering System
- **Reusable Filters**:
  - `trim` - Remove leading/trailing whitespace
  - `uppercase` - Convert to uppercase
  - `lowercase` - Convert to lowercase
  - `round_decimals(n)` - Round to n decimal places

#### Operation-Specific Rules
- Different validation/filter rules for `create` vs `update` operations
- Declarative syntax in entity macros

### Documentation
- New [Validation and Filtering Guide](docs/guides/VALIDATION_AND_FILTERING.md)
- Updated [Getting Started](docs/guides/GETTING_STARTED.md) with validation examples
- Updated [Quick Start](docs/guides/QUICK_START.md) with validation examples

---

## [0.0.4] - 2024-11-20

### Added - Pagination and Query Filtering

#### Generic Pagination
- **Automatic pagination** for all list endpoints
- Query parameters: `?limit=20&offset=0`
- `PaginatedResponse<T>` with metadata:
  - `total` - Total number of items
  - `limit` - Items per page
  - `offset` - Current offset
  - `has_more` - Boolean flag for more pages

#### Query Filtering
- **QueryableStore trait** for filterable storage
- Generic query filtering system
- Support for common filter operations

### Documentation
- New [Pagination and Filtering Guide](docs/guides/PAGINATION_AND_FILTERING.md)
- Updated API documentation with pagination examples

---

## [0.0.3] - 2024-11-01

### Added - DynamoDB Support

#### Storage Backend
- **DynamoDBLinkService** - AWS DynamoDB backend for links
- **DynamoDB feature flag** - Optional dependency
- Configuration via AWS SDK

#### Features
- Efficient link queries using DynamoDB secondary indices
- Support for link metadata
- Batch operations for performance

### Changed
- Made storage backends optional via feature flags
- Default to `in-memory` storage

### Documentation
- Added DynamoDB setup guide
- Updated architecture documentation with storage layer

---

## [0.0.2] - 2024-10-25

### Added

#### Macro System
- **`impl_data_entity!` Macro** - Generate complete Data entities with zero boilerplate
- **`impl_link_entity!` Macro** - Generate custom Link entities
- **Helper macros**: `entity_fields!`, `data_fields!`, `link_fields!`
- 97% reduction in entity definition code

#### Entity Creation System
- **EntityCreator trait** - Dynamically create entities
- **Two ways to create links**:
  1. Link existing entities: `POST /{source}/{id}/{route}/{target_id}`
  2. Create entity + link: `POST /{source}/{id}/{route}` with entity data
- Auto-enriched responses with full entity data

#### Entity Features
- **Built-in soft delete** - `soft_delete()` and `restore()` methods
- **Automatic timestamps** - `created_at`, `updated_at` auto-managed
- **Status management** - `set_status()` with timestamp update
- **Type safety** - Entity type enforcement via macros

#### Auto-Routing
- **ServerBuilder** - Fluent API for building servers
- **Auto-route generation** - 88% code reduction (340 → 40 lines)
- **EntityRegistry** - Automatic CRUD route creation
- **Module system** - Group related entities with configuration

### Changed

#### Breaking Changes
- **Removed multi-tenancy** - Simplified architecture without `tenant_id`
- **New entity hierarchy** - `Entity` → `Data` / `Link` trait hierarchy
- **Updated Module trait** - Added `register_entities()` and entity fetcher/creator methods
- **Simplified service APIs** - Removed tenant_id from all operations

### Removed
- Multi-tenancy support (can be implemented at application level if needed)
- Tenant-scoped queries and operations
- `tenant_id` field from all entities

### Documentation
- Complete rewrite of all documentation
- New [Quick Start Guide](docs/guides/QUICK_START.md)
- New [Getting Started Guide](docs/guides/GETTING_STARTED.md)
- Updated [Architecture Documentation](docs/architecture/ARCHITECTURE.md)
- New [ServerBuilder Implementation Guide](docs/architecture/SERVER_BUILDER_IMPLEMENTATION.md)

### Migration Guide

#### Entities
```rust
// Old (v0.0.1)
struct Order {
    id: Uuid,
    tenant_id: Uuid,  // ❌ Removed
    name: String,
    amount: f64,
}

// New (v0.0.2)
impl_data_entity!(Order, "order", ["name"], {
    amount: f64,
});
// ✅ tenant_id removed, macro generates all boilerplate
```

#### Services
```rust
// Old (v0.0.1)
service.create(&tenant_id, order).await?;
service.get(&tenant_id, &id).await?;

// New (v0.0.2)
service.create(order).await?;
service.get(&id).await?;
// ✅ No more tenant_id parameter
```

---

## [0.0.1] - 2024-10-22

### Added - Initial Release

#### Core Framework
- Generic entity and relationship management framework
- Bidirectional link navigation
- YAML-based configuration for links
- Auto-pluralization system
- Flexible relationships between entities
- In-memory storage implementation

#### Multi-Tenancy (Removed in v0.0.2)
- Multi-tenant support with tenant isolation
- Tenant-scoped queries and operations

#### API Features
- Auto-generated CRUD routes for entities
- Link enrichment with full entity data
- Contextual link fetching (forward/reverse/direct)

#### Core Components
- `EntityDescriptor` trait for route generation
- `Module` trait for grouping entities
- `LinkService` for relationship management
- `EntityFetcher` for dynamic entity loading

#### Examples
- `simple_api` - Basic CRUD example
- `full_api` - Complete API with all features
- `microservice` - Billing microservice example

#### Documentation
- Comprehensive README with examples
- Architecture documentation
- Getting started guide
- API documentation
- Link enrichment guide
- Multi-level navigation guide

---

## Version Comparison Summary

| Version | Key Feature | Impact |
|---------|-------------|--------|
| **0.0.6** | 🆕 **GraphQL Support** | Multi-protocol APIs (REST + GraphQL) |
| 0.0.5 | Validation & Filtering | Automatic data validation |
| 0.0.4 | Pagination | Generic pagination for lists |
| 0.0.3 | DynamoDB Support | Production storage backend |
| 0.0.2 | Macro System + Auto-Routing | 97% less boilerplate |
| 0.0.1 | Initial Release | Core framework |

---

[Unreleased]: https://github.com/triviere/this-rs/compare/v0.0.6...HEAD
[0.0.6]: https://github.com/triviere/this-rs/compare/v0.0.5...v0.0.6
[0.0.5]: https://github.com/triviere/this-rs/compare/v0.0.4...v0.0.5
[0.0.4]: https://github.com/triviere/this-rs/compare/v0.0.3...v0.0.4
[0.0.3]: https://github.com/triviere/this-rs/compare/v0.0.2...v0.0.3
[0.0.2]: https://github.com/triviere/this-rs/compare/v0.0.1...v0.0.2
[0.0.1]: https://github.com/triviere/this-rs/releases/tag/v0.0.1
