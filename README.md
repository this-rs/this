# this-rs 🦀

> A framework for building **complex multi-entity APIs** with **many relationships** in Rust.
> Four protocols, nine storage backends, cognitive intelligence — from one codebase.
>
> **Designed for APIs with 5+ entities and complex relationships.**  
> For simple CRUD APIs, consider using Axum directly.

[![CI](https://github.com/this-rs/this/actions/workflows/ci.yml/badge.svg)](https://github.com/this-rs/this/actions/workflows/ci.yml)
[![Documentation](https://github.com/this-rs/this/actions/workflows/docs.yml/badge.svg)](https://github.com/this-rs/this/actions/workflows/docs.yml)
[![Crates.io](https://img.shields.io/crates/v/this-rs.svg)](https://crates.io/crates/this-rs)
[![docs.rs](https://docs.rs/this-rs/badge.svg)](https://docs.rs/this-rs)
[![License](https://img.shields.io/crates/l/this-rs.svg)](LICENSE-MIT)
[![codecov](https://codecov.io/gh/this-rs/this/graph/badge.svg)](https://codecov.io/gh/this-rs/this)

**Website**: [this.rs](https://this.rs)

---

## ✨ Highlights

### 🚀 Core Features
- 🔌 **Generic Entity System** — Add entities without modifying framework code
- 🤖 **Auto-Generated Routes** — Declare a module, routes are created automatically
- 🏗️ **Macro-Driven Entities** — Define entities with zero boilerplate using macros
- 🔒 **Type-Safe** — Full Rust compile-time guarantees

### 🔗 Relationship Management
- 🔗 **Flexible Relationships** — Multiple link types between same entities
- ↔️ **Bidirectional Navigation** — Query relationships from both directions
- ✨ **Auto-Enriched Links** — Full entities in responses, no N+1 queries
- 🎯 **Smart Entity Creation** — Create new entities + links in one API call

### 🌐 Multi-Protocol Support
- 🌍 **REST API** — Traditional RESTful endpoints with auto-routing
- 📊 **GraphQL API** — Dynamic schema generation with full CRUD, relations, and subscriptions
- ⚡ **gRPC** — Auto-generated Protocol Buffers with EntityService, LinkService, EventService, and NotificationService
- 🔌 **WebSocket** — Real-time bidirectional communication with subscribe/unsubscribe filters
- 📡 **SSE** — Server-Sent Events for lightweight real-time streaming

### 💾 Storage Backends (9)
- **In-Memory** — Zero-config for development and testing
- **PostgreSQL** — Production-ready relational backend
- **MongoDB** — Document store backend
- **Neo4j** — Graph database for deep relationship traversals
- **ScyllaDB** — Distributed wide-column store (CQL compatible)
- **MySQL** — Traditional relational backend
- **DynamoDB** — AWS serverless storage
- **LMDB** — Embedded key-value store (zero-copy reads)
- **Obrain** — Cognitive graph database with hybrid intelligence; the backend now couples with the this-rs/obrain engine checkout and records durable per-mutation WAL commits

### 📣 Event System
- 🔔 **EventBus** — Built-in non-blocking broadcast on all CRUD operations
- 🔄 **Event Flows** — Declarative YAML pipelines with 8 operators (filter, map, batch, deduplicate, rate_limit, fan_out, resolve, deliver)
- 📬 **Notifications** — In-app NotificationStore with read/unread tracking across REST, GraphQL, and gRPC
- 🪝 **Webhook Sinks** — Deliver events to external webhooks with retry logic
- 📱 **Push Notifications** — Expo push notification delivery via Event Flows

### 🧠 Cognitive Signals
- 🚨 **AnomalyDetected** — Unusual patterns in data mutations (threshold-based)
- 🔄 **CoChangeDetected** — Entities that always change together (correlation tracking)
- 🕸️ **StigmergyLockIn** — Emergent behavioral lock-ins
- 🩹 **ScarCreated** — Failure patterns to avoid repeating
- 📖 **EpisodeLearned** — Learned behavioral sequences
- 🌉 **CognitiveNotificationBridge** — Subscribes to EventBus, evaluates thresholds, routes to SinkRegistry

### 🔐 Authentication & Authorization
- 🔑 **WAMI Auth STS** — Built-in JWT authentication with Ed25519 key pairs
- 🚀 **Bootstrap Mode** — Auto-generates keys at startup for zero-config development
- 🛡️ **RBAC Policies** — Per-entity authorization policies in YAML (authenticated, owner, admin_only, service_only)
- 🧩 **Custom Resolvers** — Reusable authorization logic referenced with `resolver:name` in YAML
- 🏢 **Multi-Tenant** — Opt-in tenant isolation via `impl_entity_multi_tenant!` macro
- 🗑️ **GDPR Erasure** — One-call tenant data deletion: `DELETE /tenants/:tenant_id/data`

### ⚡ Developer Experience
- ✅ **Automatic Validation & Filtering** — Zero-boilerplate data validation with declarative rules
- 📄 **Generic Pagination & Query Filtering** — Automatic pagination for all list endpoints
- 📝 **Auto-Pluralization** — Smart plural forms (company → companies)
- ⚙️ **YAML Configuration** — Declarative entity, link, event flow, auth, and cognitive definitions
- 🛠️ **CLI Scaffolding** — `this-cli` generates entire projects in seconds
- 🖥️ **Frontend serving (`frontend` feature)** — optional feature that serves the project's SPA from the API process using the detached vite-rs crate (dev hot reload + `/__dev` AI command bridge; embedded production serving with vite-rs rules), via `this::server::frontend`
- 🛰️ **Deployment host (`this-host`)** — a config-driven binary (`deploy.yaml`) that attaches existing projects — their base, their identities, their policies, their exposure — without rewriting them and **without writing Rust**: one `mount` per attached project, a published plan (`GET /`, `--dry-run`) that says what is attached **and what is not**, and `read_only: true` by default so attaching a live base does not open it for writing. See [Deploying a structure](docs/hote-de-deploiement.md)

---

## 🎯 Is this-rs Right for You?

### ✅ **Perfect Fit** - You Should Use this-rs

- **Many entities** (5+ entities with CRUD operations)
- **Complex relationships** (multiple link types between entities)
- **Bidirectional navigation** (need to query relationships from both directions)
- **Multi-protocol APIs** (want both REST and GraphQL from same codebase)
- **Microservices architecture** (building multiple interconnected services)
- **Rapid iteration** (adding entities frequently, need consistency)

**Example use cases**: CMS, ERP, e-commerce platforms, social networks, project management tools

### ⚠️ **Probably Overkill** - Consider Alternatives

- **Simple CRUD** (< 5 entities with basic operations)
- **No relationships** (entities are independent)
- **Single small API** (not planning to scale)
- **Learning Rust/Axum** (start with Axum directly, add this-rs later if needed)
- **Maximum performance critical** (framework adds small overhead)

**For simple projects, use [Axum](https://github.com/tokio-rs/axum) + [utoipa](https://github.com/juhaku/utoipa) directly.**

**📖 See [Alternatives Comparison](docs/ALTERNATIVES.md) for detailed analysis of when to use what.**

### 📊 ROI by Project Size

| Entities | Relationships | Recommended | Time Saved |
|----------|---------------|-------------|------------|
| 1-3 | Few | ❌ Axum directly | - |
| 3-5 | Some | ⚠️ Consider this-rs | ~20% |
| 5-10 | Many | ✅ this-rs recommended | ~40% |
| 10+ | Complex | ✅✅ this-rs highly recommended | ~60% |

---

## 💡 What this-rs Actually Saves

### Without this-rs (Pure Axum)
```rust
// For each entity, you write:
// 1. Entity definition (✓ same in both)
// 2. CRUD handlers (✓ same in both - you still write business logic)
// 3. Routes registration (❌ REPETITIVE - 30+ lines per entity)
// 4. Link routes (❌ REPETITIVE - 50+ lines per relationship)
// 5. Link enrichment (❌ MANUAL - N+1 queries if not careful)
// 6. GraphQL schema (❌ MANUAL - duplicate type definitions)

// Example: 10 entities with 15 relationships
// = ~500 lines of repetitive routing code
```

### With this-rs (✅)
```rust
// 1. Entity definition (✓ with macro helpers)
impl_data_entity!(Product, "product", ["name", "sku"], {
    sku: String,
    price: f64,
});

// 2. CRUD handlers (✓ you still write these - it's your business logic)
// 3. Routes registration (✅ AUTO-GENERATED)
// 4. Link routes (✅ AUTO-GENERATED from YAML)
// 5. Link enrichment (✅ AUTOMATIC - no N+1 queries)
// 6. GraphQL schema (✅ AUTO-GENERATED from entities)

// Main.rs for 10 entities with 15 relationships
let app = ServerBuilder::new()
    .register_module(module)?  // ← ~40 lines total
    .build()?;
```

**What you save**: Routing boilerplate, link management, GraphQL schema duplication.  
**What you still write**: Business logic handlers (as you should!).

---

## 🚀 Quick Example

### 1. Define Your Entity with Macros

```rust
use this::prelude::*;

// Macro generates full entity with all base fields
impl_data_entity!(Product, "product", ["name", "sku"], {
    sku: String,
    price: f64,
    description: Option<String>,
    stock: i32,
});

// Automatically includes:
// - id: Uuid (auto-generated)
// - type: String (auto-set to "product")
// - name: String (required)
// - created_at: DateTime<Utc> (auto-generated)
// - updated_at: DateTime<Utc> (auto-managed)
// - deleted_at: Option<DateTime<Utc>> (soft delete)
// - status: String (required)
```

### 2. Create Entity Store with EntityCreator

```rust
use this::prelude::*;

#[derive(Clone)]
pub struct ProductStore {
    data: Arc<RwLock<HashMap<Uuid, Product>>>,
}

// Implement EntityFetcher for link enrichment
#[async_trait]
impl EntityFetcher for ProductStore {
    async fn fetch_as_json(&self, entity_id: &Uuid) -> Result<serde_json::Value> {
        let product = self.get(entity_id)
            .ok_or_else(|| anyhow::anyhow!("Product not found"))?;
        Ok(serde_json::to_value(product)?)
    }
}

// Implement EntityCreator for automatic entity creation
#[async_trait]
impl EntityCreator for ProductStore {
    async fn create_from_json(&self, entity_data: serde_json::Value) -> Result<serde_json::Value> {
        let product = Product::new(
            entity_data["name"].as_str().unwrap_or("").to_string(),
            entity_data["status"].as_str().unwrap_or("active").to_string(),
            entity_data["sku"].as_str().unwrap_or("").to_string(),
            entity_data["price"].as_f64().unwrap_or(0.0),
            entity_data["description"].as_str().map(String::from),
            entity_data["stock"].as_i64().unwrap_or(0) as i32,
        );
        self.add(product.clone());
        Ok(serde_json::to_value(product)?)
    }
}
```

### Describe it once (ModuleDescriptor)

A module can also be declared as data. One `ModuleDescriptor` — entities, fields, validators, typed
filters, links, policies, messages in French and English — drives REST, GraphQL and gRPC: the same invalid
payload gets the same contract code (`this.validation.positive`) and the matching status on each protocol, a
filter on a field the descriptor does not expose is a typed error, and the message follows the caller's
`Accept-Language`.

```rust
let mut invoice = EntityDesc::new("invoice");
invoice.fields = vec![
    FieldDesc::new("amount", FieldType::F64)
        .validator(ValidatorSpec::Positive, Applies::Always)
        .filterable(&[FilterOp::Gt, FilterOp::Eq])
        .sortable(),
];
invoice.permissions.ops.insert(Operation::Create, PolicyExpr::HasRole(vec!["admin".into()]));
let mut module = ModuleDescriptor::new("billing", "1.0.0");
module.entities.push(invoice);
```

Runnable, with tests: [`examples/descriptor_api`](examples/descriptor_api/main.rs)
(`cargo test --example descriptor_api --features graphql,grpc`). Migrating an existing module (additive;
a few non-additive changes are listed): [`docs/migration/module-descriptor.md`](docs/migration/module-descriptor.md).
Design: [`docs/rfc/module-descriptor.md`](docs/rfc/module-descriptor.md). The default `EntityFetcher::list_query`
is O(n) and bounded (10 000 rows, error `this.query.too_large`); override it to push queries to your backend.

### 3. Create Module

```rust
impl Module for CatalogModule {
    fn name(&self) -> &str { "catalog-service" }
    fn entity_types(&self) -> Vec<&str> { vec!["product"] }
    
    fn links_config(&self) -> Result<LinksConfig> {
        LinksConfig::from_file("config/links.yaml")
    }
    
    fn register_entities(&self, registry: &mut EntityRegistry) {
        registry.register(Box::new(ProductDescriptor::new(self.store.clone())));
    }
    
    fn get_entity_fetcher(&self, entity_type: &str) -> Option<Arc<dyn EntityFetcher>> {
        match entity_type {
            "product" => Some(Arc::new(self.store.clone()) as Arc<dyn EntityFetcher>),
            _ => None,
        }
    }
    
    fn get_entity_creator(&self, entity_type: &str) -> Option<Arc<dyn EntityCreator>> {
        match entity_type {
            "product" => Some(Arc::new(self.store.clone()) as Arc<dyn EntityCreator>),
            _ => None,
        }
    }
}
```

### 4. Launch Server (Auto-Generated Routes!)

#### REST-only (simplest)
```rust
#[tokio::main]
async fn main() -> Result<()> {
    let app = ServerBuilder::new()
        .with_link_service(InMemoryLinkService::new())
        .register_module(CatalogModule::new(store))?
        .build()?;  // ← All REST routes created automatically!
    
    let listener = tokio::net::TcpListener::bind("0.0.0.0:3000").await?;
    axum::serve(listener, app).await?;
    Ok(())
}
```

#### Multi-Protocol with Events, Auth & Cognitive Signals
```rust
use this::prelude::*;

#[tokio::main]
async fn main() -> Result<()> {
    let host = ServerBuilder::new()
        .with_link_service(InMemoryLinkService::new())
        .register_module(CatalogModule::new(store))?
        .build_host()?                                          // transport-agnostic host
        .with_event_bus(EventBus::new(256))                     // event broadcasting
        .with_notification_store(InMemoryNotificationStore::new())
        .with_auth_config_file("config/auth.yaml")?;            // WAMI Auth STS

    let host = Arc::new(host);
    let app = Router::new()
        .merge(RestExposure::build_router(host.clone(), vec![])?)
        .merge(GraphQLExposure::build_router(host.clone())?)
        .merge(GrpcExposure::build_router(host.clone())?)
        .merge(WebSocketExposure::build_router(host)?);

    let listener = tokio::net::TcpListener::bind("0.0.0.0:3000").await?;
    axum::serve(listener, app).await?;
    Ok(())
}
```

**That's it!** Routes are auto-generated:

**REST API:**
- ✅ `GET /products` — List all (paginated)
- ✅ `POST /products` — Create new product
- ✅ `GET /products/:id` — Get by ID
- ✅ `PUT /products/:id` — Update product
- ✅ `DELETE /products/:id` — Delete product
- ✅ `GET /products/:id/links` — Introspection
- ✅ Link routes (auto-generated from YAML)

**GraphQL API:**
- ✅ `POST /graphql` — Full CRUD + subscriptions
- ✅ `GET /graphql/playground` — Interactive playground
- ✅ Auto-generated types, queries, mutations, and relations

**gRPC:**
- ✅ `EntityService` — List, Get, Create, Update, Delete
- ✅ `LinkService` — CRUD for relationships
- ✅ `EventService` — `StreamEvents` (server-streaming)
- ✅ `NotificationService` — GetNotifications, MarkAsRead
- ✅ `GET /grpc/proto` — Proto export for client generation

**WebSocket & SSE:**
- ✅ `/ws` — Subscribe/unsubscribe with entity_type/event_type filters
- ✅ `/events/stream` — SSE endpoint with query filters

**Auth (WAMI STS):**
- ✅ `POST /auth/token` — JWT token issuance
- ✅ `GET /auth/keys` — Public key endpoint (Ed25519)
- ✅ `POST /auth/refresh` — Token refresh
- ✅ `POST /auth/revoke` — Token revocation
- ✅ `DELETE /tenants/:tenant_id/data` — GDPR erasure

---

## 🔗 Advanced Link Features

### Two Ways to Create Links

#### 1. Link Existing Entities
```bash
# POST /orders/{order_id}/invoices/{invoice_id}
curl -X POST http://localhost:3000/orders/abc-123/invoices/inv-456 \
  -H 'Content-Type: application/json' \
  -d '{"metadata": {"priority": "high"}}'
```

#### 2. Create New Entity + Link Automatically
```bash
# POST /orders/{order_id}/invoices
curl -X POST http://localhost:3000/orders/abc-123/invoices \
  -H 'Content-Type: application/json' \
  -d '{
    "entity": {
      "number": "INV-999",
      "amount": 1500.00,
      "status": "active"
    },
    "metadata": {"priority": "high"}
  }'

# Response includes both created entity AND link!
{
  "entity": {
    "id": "inv-999-uuid",
    "type": "invoice",
    "name": "INV-999",
    "amount": 1500.00,
    ...
  },
  "link": {
    "id": "link-uuid",
    "source_id": "abc-123",
    "target_id": "inv-999-uuid",
    ...
  }
}
```

### Auto-Enriched Link Responses

When you query links, you automatically get full entity data:

```bash
# GET /orders/{id}/invoices
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
        ...
      }
    }
  ]
}
```

**No N+1 queries!** Entities are fetched efficiently in the background.

---

## 📐 ModuleDescriptor — Declare It Once

A module can be **described as data** instead of restated in code. A single `ModuleDescriptor` holds the
entities and their fields, the field rules (validators, transforms, filterable/sortable, read-only), the
links, the policy of every operation, the stable error codes and their French/English messages. REST,
GraphQL, gRPC, WebSocket, the generated contracts and the CLI all derive from it — no second declaration,
nothing to keep in sync.

```rust
let mut invoice = EntityDesc::new("invoice");
invoice.fields = vec![
    FieldDesc::new("amount", FieldType::F64)
        .validator(ValidatorSpec::Positive, Applies::Always)
        .filterable(&[FilterOp::Gt, FilterOp::Eq])
        .sortable(),
];
invoice.permissions.ops.insert(Operation::Create, PolicyExpr::HasRole(vec!["admin".into()]));

let mut billing = ModuleDescriptor::new("billing", "1.0.0");
billing.entities.push(invoice);

impl Module for BillingModule {
    fn name(&self) -> &str { "billing" }
    fn version(&self) -> &str { "1.0.0" }

    // Declared once — the host derives validation, filters, policies and contracts from it.
    fn descriptor(&self) -> Option<ModuleDescriptor> { Some(billing.clone()) }

    // Registration of stores and fetchers is unchanged.
}
```

### What the host derives (nothing to redeclare)

| Surface | Derived from the descriptor |
|---|---|
| **REST** | create/update validation — including a **missing** required field — typed `?filter=` / `?sort=`, the policy of each operation, localized error bodies |
| **GraphQL** | the same validators, `filter` / `sort` arguments on list fields, the same policies |
| **gRPC** | the same validators, `this-filter` / `this-sort` metadata, identity read from call metadata, the same policies |
| **WebSocket** | the same policies; the `graphql-ws` handshake is CSWSH-guarded when identity comes from the `this_token` cookie |
| **Contracts** | `GET /_this/descriptors`, `/_this/descriptors/{module}`, `/_this/schema/{entity}/{create\|update}` (JSON Schema) and `/_this/openapi.json` — all read-only |
| **CLI** | `this descriptor check`, `this descriptor lock`, `this descriptor export` (JSON, YAML, SDL, `.proto`, JSON Schema) |
| **Frontend / other tools** | `this descriptor export` renders the same declaration as JSON, YAML, SDL, `.proto` or JSON Schema — one source for generated clients, mocks and docs |

The parts that cannot live in the descriptor keep **one** owner: `impl_data_entity_validated!` for the struct field types,
`descriptor/module.yaml` for identity (`name`, `version`) and `requires`, `descriptor/descriptor.lock` for the proto tags
(>= 16, never moved, never reused). `this descriptor check` fails when a declaration drifts — `this.descriptor.declared_twice`,
`this.descriptor.tag_reused`, `this.module.identity_mismatch`, `this.module.plural_mismatch` — instead of letting two copies diverge silently.

### Stable error codes

Every failure the described surface answers carries a machine-readable code with the same meaning on every
protocol, plus one status per protocol — a client branches on the code, not on the wording.

| Family | Codes | REST / gRPC / GraphQL |
|---|---|---|
| `this.validation.*` | `required`, `type_mismatch`, `positive`, `string_length`, `unknown_field`, `read_only`, `custom` | 422 · `INVALID_ARGUMENT` · `BAD_USER_INPUT` |
| `this.query.*` | `malformed_filter`, `field_not_filterable`, `field_not_sortable`, `too_large` | 400 · `INVALID_ARGUMENT` · `BAD_USER_INPUT` |
| `this.auth.*` | `unauthenticated`, `forbidden` | 401 / 403 · `UNAUTHENTICATED` / `PERMISSION_DENIED` · `UNAUTHENTICATED` / `FORBIDDEN` |
| `this.module.*` | `not_found`, `duplicate_entity`, `duplicate_plural`, `identity_mismatch`, `plural_mismatch`, `dependency_missing`, `dependency_version`, `hot_load_disabled`, `has_dependents`, `unavailable`, `version_not_newer` | 404 / 409 / 400 by code |
| `this.descriptor.*` | `invalid`, `declared_twice`, `tag_reused`, `syntax_unsupported` | build / check time |
| `this.transport.*` | `bad_request`, `not_found`, `method_not_allowed`, `missing_arg` | 400 / 404 / 405 |

The same invalid payload therefore gets the **same code** on the three protocols (REST `422
this.validation.positive`, gRPC `INVALID_ARGUMENT`, GraphQL `BAD_USER_INPUT`) — proven by the test
`same_invalid_payload_same_code_on_rest_graphql_grpc`.

Two behaviours worth knowing:

- an unknown query field is a **typed error**, never silently ignored (`this.query.field_not_filterable`);
- the default `EntityFetcher::list_query` is bounded to 10 000 rows and answers `this.query.too_large`
  (400) beyond that — override it to push filtering and pagination down to your backend.

### i18n — one message, the caller's language

Messages are declared once, in French and English, next to the entity; a module can ship its own bundles.
The locale is resolved **per request** with one priority order, on every protocol:

1. explicit choice in the URL: `?lang=en` (or `?locale=en`);
2. persisted choice: cookie `this_locale=en`;
3. `x-this-locale` header (gRPC metadata for gRPC);
4. `Accept-Language` (gRPC metadata `accept-language`);
5. the catalog's default locale.

An explicit choice the catalog does not have falls through to the next hint — it never produces an empty
message. The error body carries `code`, `kind`, `message_key`, `fields` and the localized `message`
(RFC 9457 keys included), so a client can branch on the code and display the text as-is.

### Auth — one evaluator, every protocol

Policies are declared per operation in the descriptor (`PolicyExpr::HasRole`, `AdminOnly`, `any(...)`,
`all(...)`, `not(...)`) and evaluated by a **single** `PolicyIndex` on REST, GraphQL, gRPC and WebSocket.
Resolution order is fixed: **descriptor (explicit) → `links.yaml` link table → `links.yaml` entity table →
default**. An authenticated identity that matches no rule gets `this.auth.forbidden`; an anonymous caller
`this.auth.unauthenticated`.

`AuthContext` comes from each protocol's own channel — the `Authorization` header, a session cookie, a
GraphQL/WebSocket handshake, gRPC metadata — so a policy is written once and enforced everywhere.

### IAM — WAMI policies behind `perm:`

A descriptor (or `links.yaml`) can write `perm:orders:write` / `perm:orders.write` instead of a
hand-rolled rule. With the `wami` feature and a policy definition, that permission is answered by
[wami](https://github.com/Lsh0x/wami)'s evaluator — deny-overrides-allow, permission boundaries,
conditions and ARN globbing, the AWS-shaped model:

```yaml
auth:
  provider: wami
  wami:
    mode: sts                                   # bootstrap (dev) | sts | embedded
    public_key: ${WAMI_PUBLIC_KEY}
    audience: https://api.example.com           # required outside bootstrap
    policies_path: ./config/wami-policies.json  # arms the IAM on every surface
    identity_path: ./data/wami-identity.json    # tenant-segment journal
```

- **Permission → action** — `perm:orders:write` and `perm:orders.write` both map to `this:orders:write`.
- **Target → resource** — the checked row travels with the question: `orders/<uuid>` for one row,
  `orders/*` when the call names only a type, `link/<type>/<source>/<target>` for a link
  (`link/owns/users/orders`, or `link/owns/*/*` when the surface knows only the link type), and
  `resource/*` when it knows no target at all — only `Resource: "*"` matches that.
- **Roles are groups** — every role in the token reads the documents registered for it, beside the
  principal's own; a `Deny` in any of them still wins.
- **Fail-closed** — an unreadable document, an unknown condition operator, a permission that cannot be
  mapped to an action, an anonymous or service caller, or a condition key that is unknown (source IP,
  transport, MFA) all make the decision *unevaluable*, which is a refusal. `owner`
  (`PolicyExpr::Owner`) stays with the built-in evaluator: there is no row for wami to read a
  proprietor from.
- **Tokens** — Ed25519 through wami's `KeyManager`, `kid` in the JOSE header (RFC 7638), `aud` and
  `token_use` (`access` / `refresh`); `GET /auth/keys` serves the JWKS. A refresh token never
  authenticates a request.
- **Migration window** — tokens minted before `aud` / `token_use` existed are accepted in `bootstrap`
  mode only, or with `accept_legacy_tokens: true`; remove it once every client has reissued.
- **A provider that cannot be built refuses to start.** `provider: wami` in a build without the
  feature, an unusable key, an STS that cannot be created, or `provider: oidc` (no OIDC provider yet)
  all stop `build_host` with a named error. They used to log a warning and carry on — which left the
  host with `NoAuthProvider`, that is anonymous callers for whom `is_owner` answers `true`.
- **Tenant identity** — `Uuid` tenants get an allocated `u64` segment, journalled through
  `identity_path` (write-through, tmp + rename). Nothing is derived by hashing: a collision would be a
  tenant crossing. `identity_path` is required with `policies_path` outside `bootstrap`.
- **An external IdP is a first-class provider.** `provider: oidc` verifies tokens minted by an
  OpenID Provider — `this` is a resource server, not an authorization server (ADR D11):

  ```yaml
  auth:
    provider: oidc
    oidc:
      discovery_url: https://idp.example.com/.well-known/openid-configuration
      audience: https://api.example.com   # required
  ```

  The issuer and key set come from the discovery document, keys are selected by `kid`, and an
  unknown `kid` costs exactly one refetch — what a rotation looks like from here. Symmetric
  algorithms and `none` are refused, `aud` and `iss` must match, and a subject that is not a UUID
  is mapped to a UUIDv5 derived from the subject, so it stays stable across restarts.
- **`scope` grants where nothing else decides.** An OIDC access token carries `scope`
  (RFC 6749 §3.3). It is read into `AuthContext::User::scopes`, and `Authorization` consults it only
  where the IAM answers nothing — no policy engine wired, or no principal to look up. A wired IAM
  keeps the last word, so `admin_only` still refuses a token carrying `*`. `perm:orders:read` is
  granted by `orders:read`, `orders.read`, `orders:*`, `orders.*` or `*`.
- Design, decisions and known limits: [docs/wami-core-migration.md](docs/wami-core-migration.md) ·
  [docs/oidc-resource-server.md](docs/oidc-resource-server.md).

### Hot loading from a VFS

A declarative module can be loaded into a **live** host, replaced and unloaded, from a `ModuleStore`
behind the VFS (`InMemoryVfs`, `LocalFsVfs`, `ObrainVfs` — read through `VfsModuleStore`):

```rust
let app = ServerBuilder::new()
    .register_module(BillingModule::new(store))?
    .with_module_runtime()       // runtime routes + admin interface
    .with_hot_load_enabled(true) // load / replace / unload — closed by default
    .build()?;
```

- `ModuleSource::Stored` reads the module from the VFS and checks it with `CheckProfile::Stored`;
  `requires` (semver) are ordered topologically and **fail closed** — a missing dependency is
  `this.module.dependency_missing`, a version mismatch `this.module.dependency_version`.
- The admin interface is mounted with its own policy (`RuntimeOptions::admin_policy`, `admin_only` by
  default) — loading code into a running host is never open by default, whatever `authorize` says:

  | Method | Path | Effect |
  |---|---|---|
  | `GET` | `/admin/modules` | status of every module (active, draining, unloaded, failed, offered) |
  | `GET` | `/admin/modules/{name}` | status of one module |
  | `POST` | `/admin/modules` | `{"manifest": {…}}`, `{"store": {"name": "…", "version": "…"}}` or `{"module": "<offered>"}` |
  | `DELETE` | `/admin/modules/{name}` | unload — refused with `this.module.has_dependents` while a dependent is active |

- An unload **drains** in-flight requests for up to 30 s (`drain_grace`), then cancels them; afterwards the
  module's plural answers 404 `this.module.not_found` — a tombstone, not a silent 400.
- A **cancelled unload** (client disconnect mid-drain) reverts the module to `active` instead of leaving it
  stuck `draining` (503) with no legal transition out.
- A **`replace` is forward-only**: a downgrade or identical version is refused (`this.module.version_not_newer`),
  and proto-tag compatibility (`check_compat`, the same rule the store enforces) is checked before publishing —
  a hot swap cannot silently rewrite the wire contract.
- A **compiled** module (`ModuleSource::Compiled`) runs its `init` then `start` lifecycle hooks before it is
  served; a failure refuses the load and records the module `failed`. `unload` / `replace` call `stop`.
  `ModuleRuntime::health(name)` probes the module's `health()` without touching the served routes.
- `ModuleRuntime::dry_run(source)` validates a load or replace (resolution, version ordering, proto-tag
  compatibility) **without publishing anything** — a sandbox for a module change, returning the status it
  would get (`resolved`).
- With `with_hot_load_enabled(true)` absent, every load/unload and the admin interface answer
  `this.module.hot_load_disabled`.
- Runtime modules are served over REST by the runtime dispatch layer; GraphQL, gRPC and WebSocket do not
  serve hot-loaded modules yet, and say so rather than pretending.

### Run it, read it

- Runnable, with tests: [`examples/descriptor_api`](examples/descriptor_api/main.rs) —
  `cargo test --example descriptor_api --features graphql,grpc`.
- Design: [docs/rfc/module-descriptor.md](docs/rfc/module-descriptor.md) ·
  Migration of an existing module (additive, non-additive changes listed):
  [docs/migration/module-descriptor.md](docs/migration/module-descriptor.md).
- CLI side: `this descriptor init` · `check` · `lock` · `export`, and `this add module` — see the
  [this-cli README](https://github.com/this-rs/this-cli).

---

## 📚 Examples

### [Microservice Example](examples/microservice/)

Complete billing microservice with **auto-generated routes** for both REST and GraphQL:

#### REST API
```bash
cargo run --example microservice
```

Output:
```
🚀 Starting billing-service v1.0.0
📦 Entities: ["order", "invoice", "payment"]
🌐 Server running on http://127.0.0.1:3000

📚 Entity Routes (CRUD - Auto-generated):
  GET    /orders                        - List all orders
  POST   /orders                        - Create a new order
  GET    /orders/{id}                   - Get a specific order
  PUT    /orders/{id}                   - Update an order
  DELETE /orders/{id}                   - Delete an order

🔗 Link Routes (Auto-generated):
  GET    /orders/{id}/invoices          - List invoices for an order
  POST   /orders/{id}/invoices          - Create new invoice + link automatically
  POST   /orders/{id}/invoices/{inv_id} - Link existing order & invoice
  PUT    /orders/{id}/invoices/{inv_id} - Update link metadata
  DELETE /orders/{id}/invoices/{inv_id} - Delete link
```

See [examples/microservice/README.md](examples/microservice/README.md) for full REST API details.

#### GraphQL API
```bash
cargo run --example microservice_graphql --features graphql
```

The same entities are exposed via GraphQL with:
- **Dynamic Schema Generation** - Types (`Order`, `Invoice`, `Payment`) auto-generated
- **Full CRUD** - Queries and mutations for all entities
- **Automatic Relations** - Navigate `order.invoices`, `invoice.payments` automatically
- **Interactive Playground** - Test queries at `http://127.0.0.1:3000/graphql/playground`

Example query:
```graphql
query {
  orders {
    id
    number
    customerName
    amount
    invoices {
      id
      number
      amount
      payments {
        id
        amount
        method
      }
    }
  }
}
```

See [examples/microservice/README_GRAPHQL.md](examples/microservice/README_GRAPHQL.md) for full GraphQL details.

---

## 🏗️ Architecture

### Entity Hierarchy

```
Entity (Base Trait)
├── id: Uuid
├── type: String
├── created_at: DateTime<Utc>
├── updated_at: DateTime<Utc>
├── deleted_at: Option<DateTime<Utc>>
└── status: String

    ├─► Data (Inherits Entity)
    │   └── name: String
    │       + indexed_fields()
    │       + field_value()
    │
    └─► Link (Inherits Entity)
        ├── source_id: Uuid
        ├── target_id: Uuid
        └── link_type: String
```

### Core Concepts

1. **ServerBuilder** — Fluent API for building HTTP servers
2. **ServerHost** — Transport-agnostic host holding all registries and services
3. **EntityDescriptor** — Describes how to generate routes for an entity
4. **EntityRegistry** — Collects and builds all entity routes
5. **Module** — Groups related entities with configuration
6. **LinkService** — Generic relationship management (9 backend implementations)
7. **EntityFetcher** — Dynamically fetch entities for link enrichment
8. **EntityCreator** — Dynamically create entities with automatic linking
9. **EventBus** — Non-blocking broadcast channel for entity lifecycle events
10. **Event Flows** — Declarative YAML pipelines (filter → map → batch → deduplicate → deliver)
11. **CognitiveNotificationBridge** — Monitors EventBus for anomalies, co-changes, scars, stigmergy, episodes
12. **SinkRegistry** — Routes signals to webhooks, in-app notifications, push, SSE
13. **Exposures** — Protocol adapters: RestExposure, GraphQLExposure, GrpcExposure, WebSocketExposure

### Macro System

- `impl_data_entity!` - Generate a complete Data entity
- `impl_link_entity!` - Generate a custom Link entity
- `entity_fields!` - Inject base Entity fields
- `data_fields!` - Inject Entity + name fields
- `link_fields!` - Inject Entity + link fields

---

## 📖 Documentation

### 🚀 Getting Started
- **[Quick Start](docs/guides/QUICK_START.md)** - Fast introduction
- **[Getting Started](docs/guides/GETTING_STARTED.md)** - Step-by-step tutorial

### 🌐 API Exposure
- **[GraphQL Guide](docs/guides/GRAPHQL.md)** - 🆕 Dynamic GraphQL API with auto-generated schema
- **[Custom Routes](docs/guides/CUSTOM_ROUTES.md)** - Adding custom endpoints alongside auto-routes

### 🔗 Features
- **[Validation & Filtering](docs/guides/VALIDATION_AND_FILTERING.md)** - Automatic data validation
- **[Pagination & Filtering](docs/guides/PAGINATION_AND_FILTERING.md)** - Generic pagination and query filtering
- **[Enriched Links](docs/guides/ENRICHED_LINKS.md)** - Auto-enrichment & performance
- **[Link Authorization](docs/guides/LINK_AUTHORIZATION.md)** - Securing relationships

### 🏗️ Architecture
- **[Architecture](docs/architecture/ARCHITECTURE.md)** - Technical deep dive
- **[ServerBuilder](docs/architecture/SERVER_BUILDER_IMPLEMENTATION.md)** - Auto-routing details
- **[GraphQL Implementation](docs/architecture/GRAPHQL_IMPLEMENTATION.md)** - 🆕 Custom executor design
- **[Full Documentation](docs/)** - Complete documentation index

---

## 🎁 Key Benefits

### For Developers

✅ **-88% less routing boilerplate** (340 → 40 lines in microservice example)  
✅ **Add entity quickly** - Macro helpers + module registration  
✅ **Consistent patterns** - Same structure for all entities  
✅ **Type-safe** - Full Rust compile-time checks  
✅ **Scales well** - Adding the 10th entity is as easy as the 1st  
✅ **Multi-protocol** - 🆕 Same entities exposed via REST and GraphQL  
⚠️ **Learning curve** - Framework abstractions to understand (traits, registry)  

### For Teams

✅ **Faster development** - Less code to write and maintain  
✅ **Easier onboarding** - Clear patterns and conventions  
✅ **Reduced errors** - Less manual work = fewer mistakes  
✅ **Better consistency** - Framework enforces best practices  
✅ **Flexible APIs** - 🆕 Choose REST, GraphQL, or both  

### For Production

✅ **WAMI Auth STS** — JWT authentication with Ed25519, bootstrap mode, RBAC policies  
✅ **Custom Auth Resolvers** — Reusable authorization closures (`resolver:name` in YAML)  
✅ **Multi-Tenant** — Opt-in tenant isolation per entity  
✅ **GDPR Erasure** — One-call cascade delete across entities and links  
✅ **9 Storage Backends** — In-Memory, PostgreSQL, MongoDB, Neo4j, ScyllaDB, MySQL, DynamoDB, LMDB, Obrain  
✅ **Cognitive Signals** — Anomaly detection, co-change tracking, threshold-based routing  
✅ **Event Flows** — Declarative pipelines with webhooks, push notifications, and SSE  
✅ **Notifications** — In-app NotificationStore with read/unread tracking  
✅ **Soft Deletes** — Opt-in reversible delete: `ServerBuilder::with_soft_delete()` marks `deleted_at` instead of removing the row, hides marked rows on GraphQL / gRPC / link routes, and exposes `restore` / `purge` / `*_including_deleted` as the escape hatches. One contract for all 9 backends; for application-owned REST routes, wrap your store in `SoftDeleting::new(service)`. Off by default — turning it on changes what existing `deleted_at` values mean.
✅ **Dynamic Schema** — GraphQL schema auto-generated from entities  

---

## 🤝 Contributing

Contributions are welcome! Please feel free to submit a Pull Request.

---

## 📄 License

This project is licensed under the MIT License - see the [LICENSE-MIT](LICENSE-MIT) file for details.

---

## 🌟 Why this-rs?

> "The best code is the code you don't have to write... *if you're writing it 50 times.*"

this-rs eliminates **repetitive routing and relationship boilerplate** while maintaining type safety. 

**Perfect for**:
- 🏢 **Microservices architectures** with many entities
- 🔗 **Complex relationship graphs** (many-to-many, bidirectional)
- 🔮 **Four protocols from one codebase** (REST + GraphQL + gRPC + WebSocket)
- 🧠 **Cognitive APIs** with anomaly detection, co-change tracking, and intelligent signals
- 🔐 **Secure multi-tenant platforms** with WAMI Auth, RBAC, and GDPR compliance
- 🚀 **Rapidly evolving domains** (adding entities frequently)
- 📊 **Hybrid backends** (PostgreSQL for CRUD + Neo4j/Obrain for graph traversals)

**NOT ideal for**:
- ❌ Simple CRUD APIs (< 5 entities)
- ❌ Maximum performance critical paths (framework adds overhead)
- ❌ Learning projects (start with Axum first)

### 🆕 What's New in v0.0.9

- 🔔 **EventBus** — Built-in event system with non-blocking broadcast on all CRUD operations
- 🔄 **Event Flows** — Declarative YAML pipelines: filter → map → batch → deduplicate → deliver
- 📬 **Notifications** — In-app NotificationStore with read/unread tracking (REST + GraphQL + gRPC)
- 🪝 **Webhook & Push Sinks** — Deliver events externally via webhooks or Expo push notifications
- 📡 **SSE Streaming** — `GET /events/stream` for lightweight real-time event streaming
- 🧠 **Cognitive Signals** — CognitiveNotificationBridge detects anomalies, co-changes, scars, stigmergy, episodes
- 🔐 **WAMI Auth STS** — JWT authentication with Ed25519 keys, bootstrap mode, per-entity RBAC policies
- 🧩 **Custom Auth Resolvers** — Reusable authorization closures referenced with `resolver:name` in YAML
- 🗑️ **GDPR Erasure** — `DELETE /tenants/:tenant_id/data` cascades across all entities and links
- 💾 **3 New Backends** — MySQL, LMDB, and Obrain storage backends (total: 9)
- ⚡ **gRPC** — Full gRPC support with EntityService, LinkService, EventService, NotificationService
- 🔌 **WebSocket** — Real-time bidirectional events with subscribe/unsubscribe filters

---

## 🤔 Honest Trade-offs

### What this-rs Adds ✅
- Auto-generated routing for entities and links (REST + GraphQL + gRPC + WebSocket)
- Bidirectional relationship navigation with auto-enrichment
- 9 storage backends — same API, any database
- EventBus + Event Flows with declarative YAML pipelines
- Cognitive signals (anomaly detection, co-changes, stigmergy, scars, episodes)
- WAMI Auth STS with JWT, RBAC, custom resolvers, multi-tenant, GDPR erasure
- Notifications (in-app + webhook + push + SSE)
- YAML-driven configuration for everything

### What You Still Write ✍️
- Entity definitions (with macro helpers)
- Business logic handlers (create, update, delete, custom queries)
- Validation rules (with `Validated<T>` extractor)
- Custom auth resolvers (closures or trait impls)

### The Cost ⚠️
- Learning curve (framework patterns and traits)
- Some abstraction overhead (dynamic dispatch, registry lookups)
- YAML configuration to maintain
- Smaller ecosystem than pure Axum

**Built with Rust. Four protocols. Nine backends. Cognitive intelligence. One codebase.** 🦀✨

---

<p align="center">
  Made with ❤️ and 🦀 by the this-rs community
</p>
