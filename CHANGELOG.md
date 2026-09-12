# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Integration
- **Consumer contract** — three ways to integrate the stack into another program (the obrain Dashboard, a harness, an application), each independent of the others: as a **library** (`this::storage::{Iam, AclStamp, GraphBackend, ObrainStore}`, re-exported so no deep paths are needed), over **HTTP** (`GET /graph`), or through **obrain alone**. See `docs/integration-consommateurs.md`.
- **The relief now publishes the IAM data contract**: schema version, decision count (active space), the property the enforcement actually reads (`acl_perms_blob`), and the labels, properties and edges with their meaning and a `written` flag — `true` when the framework writes it, `false` when it only reads it if present. A consumer no longer has to read framework source to know what it may rely on.
- **`ObrainStore::check_write` is public**: a program that writes through its own path (a dashboard in write mode, a migration tool, a job) must be able to honour the write lease. It was `pub(crate)`, so the fence could not be respected by anyone outside the crate.
- **Documentation is warning-free**: `RUSTDOCFLAGS="-D warnings" cargo doc -p this-rs --no-deps` passes with and without `--all-features` — 14 defects fixed (links broken by module-declaration site resolution, links from public documentation to private items, a bare URL). The module docs no longer depend on where they are declared.
- `tests/consumer_contract.rs` (8 tests) plays the role of an external program: it reads a decision through the public API, asks the hot-path question, opens a store **read-only** without taking the write lease, **reads the decisions with obrain alone** (no `Iam` type — the proof the data is not captive), discovers decisions by label, checks the published contract over HTTP, and enforces its own policies on its own router.

### Performance
- **IAM hot path: 6.15 µs → 2.37 µs per authorised request (×2.6)**, measured. The hot path no longer builds a full `AclStamp` — roles, scope, principal and a **SHA-256 digest** — to answer a membership question: `Iam::allows(principal, scope, permission)` does one index lookup, **one** property read and a string scan. Permissions are additionally stored in a hot-path form (`acl_perms_blob`), and permissions containing a control character are refused at materialisation, so one permission cannot impersonate another.
- `stamp_of` reads its seven fields through the engine's `get_node_property` ("without loading all properties") and `node_has_label`, instead of materialising the node and rebuilding a B-tree with allocated `String` keys: **4.52 µs → 2.88 µs**. The full decision follows: 4.12 → 3.23 µs.
- **Fixed (security):** `stamp_of` read through the **root** session, so in a named space a node could be missing (fail-closed but wrong) or the id could designate a *different node of the same number* in the root. It now reads through the active space — the same scoping as `acl_node` — and a security test locks it.
- `GraphBackend::node_property` no longer reads every property to return one (it called `node_properties().get(key)`); it uses the engine's direct `Node::get_property`. A generic win for every facade consumer.
- **Not optimised, on purpose**: route resolution plus a vocabulary policy costs ~240 ns, i.e. 0.5% of a ~45 µs HTTP request. A pre-parsed route table would add a second referential to maintain for half a percent.
- **Write path**: the IAM is never consulted on writes (verified: no `iam()` reference in `src/storage/obrain.rs` or `src/core/service.rs`); the only added cost is the write lease — **0.01%** of a journal append (210 ns vs 1.91 ms), now printed by the latency contract.

### Added
- **Exposure configuration: CORS, served domains, and Caddyfile delegation** (`config/server.yaml`, `ServerBuilder::with_server_config_file` / `with_cors` / `with_domains` / `delegate_to_caddy`). Nothing is enabled by default: no CORS layer, no domain guard, no proxy description — the framework does not invent a deployment's domains. See `docs/domaines-cors-caddy.md`.
  - **Domains**: a closed `allowed` list (any other `Host` is refused **421**), one-level `*.` wildcards, `trust_forwarded_host` for a trusted proxy, and a domain can designate the **tenant** (`tenants`, `subdomain_base`) — the domain now takes precedence over the token's claims, so a token from another tenant cannot change tenant by assertion.
  - **CORS**: closed origin list, methods/headers/expose/credentials/max-age; a preflight passes **without a token** because the CORS layer is the outermost one (verified: `a_preflight_passes_without_a_token_even_when_authentication_is_installed`). Impossible configurations (no origin while enabled, `*` with credentials) fail at **build time**.
  - **Caddyfile delegation**: `host.caddyfile()` and `cargo run --example caddyfile -- config/server.yaml` render a `Caddyfile` **from the same configuration** (one site block per served host, upstream, ACME email, `tls internal|off`, security headers, `extra_directives` escape hatch). This is a description the operator applies, not automation: this-rs does not drive Caddy, and the file says who sets CORS (the application, so two layers never contradict each other).
  - `ServerHost::exposure()` publishes **counts and facts, never the lists**: an operator who wants the values reads their configuration.
- **Fixed (behaviour, high impact): the policies were evaluated BEFORE authentication.** In axum the last layer applied is the outermost, and the authorization layer was applied last — so it ran first, saw `Anonymous`, and refused every non-public route to a caller holding a valid token. Fail-closed, so not a leak, but an application that worked accused its own policies. Verified by `tests/chain_order.rs` (an authenticated caller was 403) and fixed: application order is now policies → tenant → auth → domain → CORS, i.e. execution CORS → domain → auth → tenant → policies.
- **Fixed: `CorsConfig`'s programmatic `Default` differed from its YAML default** (empty method/header lists in code, usual lists in YAML) — two defaults for one type. A test now pins them equal.

### Security
- **Audit of the authorization chain — every path of every protocol.** The resolver only judged the shapes it "expected"; the router serves more, and everything else **abstained**, i.e. was not judged at all. Measured and fixed:
  - **`/notifications/{user_id}/…` and `/device-tokens/{user_id}` were an IDOR.** They are not entity routes (so no `auth:` policy attached) and took the user id **from the path**, with no comparison to the caller: any caller could read, delete and mute another user's notifications, read and rewrite their preferences, and register or delete their **device tokens** — i.e. hijack their push notifications. None of these routes had a test. A single guard layer now runs on the whole router: no context → **401**, another user → **403**, self / admin / service → allowed (documented). Twelve routes, one guard, impossible to forget on a route added later.
  - **Four link shapes were served but not judged**: `POST /{entity}/{id}/{target}` (which **creates a linked entity**), and `GET|POST|PUT|DELETE /{src}/{id}/{route}/{target}` (4 segments, where the resolver expected 5), plus every deep path served by the link router's **fallback**. A client could therefore create an entity, or create/update/delete links, without any policy being consulted. Now: any read under an entity (`>= 3 segments`) is `list_links`, `POST` at 3 segments is judged by the **target** entity's `create` (creating an invoice is an invoice creation, not an order operation), writes at `>= 4` are `create_link`/`update_link`/`delete_link`. New operation `update_link` in `EntityAuthConfig` (`#[serde(default)]`: existing configs keep working).
  - **GraphQL, gRPC and WebSocket bypassed every policy.** `GraphQLExposure::build_router` mounted a query+**mutation** executor with no authorization at all, and `ServerBuilder::build_with_grpc()` — a framework shortcut — built REST (enforced) and gRPC (not enforced) into one router, silently. These surfaces are not resolved by route, so no policy can attach: the framework now **refuses to mount them while policies are armed**, and an application that accepts the risk says so in its code (`build_router_unenforced`, `build_router_no_fallback_unenforced`, `build_with_grpc_unenforced`), logged as a warning at every mount.
  - **The frontend is a surface too, and the last one to escape the layers.** It is served by a **fallback** (`Frontend::attach` → `fallback_service`), so it appears in no `.route(…)` inventory, and whether the auth/authz layers see it depends on the **order** they are applied. Measured: `api.layer(mw)` then `attach` → the fallback answers **200 without the layer seeing the request**; `attach` then `layer(mw)` → the layer sees it. Attaching last — the natural order, "build my API, then plug the frontend" — therefore serves the SPA **and `/__dev`** outside authentication. Static assets are harmless; `/__dev` runs the rebuild command. Documented at the call site with the measurement, and locked by `tests/frontend_surface.rs`.
  - **The command bridge is now CLOSED by default.** `THIS_DEV=true` used to be enough to open `/__dev/command*`, which executes the rebuild command on HTTP request — a deployment inheriting that variable exposed it. Opening it now requires `THIS_DEV_COMMANDS=true` (or `1`/`yes`/`on`), and the opening is logged as a warning.
  - The framework adds **no CORS layer of its own** (asserted by test): that policy belongs to the application, and the framework must not let a deployment believe it is protected — or exposed — by something it did not write.
  - **The relief publishes the surface map** (`surfaces`), now including `frontend` and `dev_bridge`:: each surface, the paths it serves, whether it enforces policies, by what mechanism, and what protects it otherwise — plus the explicit opt-in required. `enforces_policies: false` is published, not hidden.
- **Tests that encoded the gaps were updated, deliberately** (`shapes_outside_the_contract_abstain`, `every_abstention_carries_an_actionable_reason`, the notification handler tests which called the routes with no identity at all): they asserted the old, unsafe contract. `tests/policy_surfaces.rs` (9 tests) replaces them with the corrected contract, including the invariant that **no read shape of a registered entity abstains any more**, at any depth.
- **Security test suites** (`tests/security_iam.rs`, 24 tests · `tests/security_durability.rs`, 11) — each test names the attack it models: privilege escalation through policy composition (`any:not:admin_only` authorises everyone *except* admins), route bypass (path traversal, method-override header, HEAD as a side door), decision capture (key collision, neighbouring tenant, neighbouring **space**, replay after invalidation), identity collisions (length-ambiguous names, Unicode normalisation) and the trust boundary itself (`documented_limit_*` tests assert what an attacker with store write access CAN do, so it is known rather than assumed).
- **Fixed — IAM lookups were not scoped to the active space.** `principal_node` and `acl_node` queried the **root** index: in a named space, a decision from another space could answer. (An earlier claim of mine that this was fixed was wrong — the replacement never matched the multi-line calls. The security suite is what caught it.)
- **Fixed — the ACL key separator was not injective.** Names containing the US separator (0x1F) collided (`("a\u{1f}b","c")` and `("a","b\u{1f}c")` produced the same key, so one decision overwrote the other). Identity parts containing control characters, or empty, are now **refused** — fail-closed, and reachable through a custom principal mapping.
- **Hardened — a duplicated ACL key makes the decision ambiguous, and an ambiguous decision is refused** rather than resolved by index order (an attacker adding a node could otherwise flip it).

### Added
- **Adoption by an existing project** — the framework now has a documented, tested migration path (see `docs/adoption-projet-existant.md`): branch the store, **observe** the traffic, make the IAM native to your own identities, then arm.
  - `ServerBuilder::authorization_report_only()` — denials are computed, logged and **published in the response** (`x-this-rs-authz: observed-deny` plus `x-this-rs-authz-policy` / `x-this-rs-authz-scope`) but nothing is blocked. This is how a running service adopts policy enforcement without being broken by it.
  - `ServerBuilder::with_principal_mapping(...)` — the application supplies `AuthContext -> (principal, scope)`, so materialised IAM decisions apply to **its** identity model (`account:…`/`org:…`, API key/project, subject/workspace) instead of this-rs's. Without a mapping a foreign identity is **refused, not guessed** (fail-closed, measured).
  - `with_obrain_store()` is now genuinely one call: if no `LinkService` is provided, the obrain link service is attached automatically — plugging a store no longer fails on `LinkService is required`, a requirement unrelated to what the caller asked for.
  - `Module::get_entity_fetcher` / `get_entity_creator` default to `None` (they exist only for the GraphQL and link-by-id surfaces). They were mandatory, which forced every module — and every project adopting this-rs — to write two empty methods just to compile.
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
