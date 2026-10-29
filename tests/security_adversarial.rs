//! Adversarial security review of the module descriptor stack.
//!
//! Each test ATTACKS one surface of a host with policies armed and records every
//! attempt that gets through (one run lists all the bypasses). A test is red only on a
//! real bypass.
//!
//! Run: `cargo test -p this-rs --all-features --test security_adversarial`.

#![cfg(all(feature = "graphql", feature = "grpc"))]

use std::sync::Arc;

use async_trait::async_trait;
use axum::Router;
use axum::body::Body;
use axum::http::{HeaderMap, Method, Request, StatusCode};
use serde_json::{Value, json};
use tower::ServiceExt;
use uuid::Uuid;

use this::core::auth::{AuthContext, AuthProvider};
use this::descriptor::{EntityDesc, FieldDesc, FieldType, ModuleDescriptor, Operation, PolicyExpr};
use this::server::builder::ServerBuilder;
use this::server::exposure::GraphQLExposure;
use this::server::exposure::grpc::entity_service::EntityServiceImpl;
use this::server::exposure::grpc::link_service::LinkServiceImpl;
use this::server::exposure::grpc::proto::entity_service_server::EntityService;
use this::server::exposure::grpc::proto::link_service_server::LinkService;
use this::server::exposure::grpc::proto::{
    CreateEntityRequest, CreateLinkRequest, DeleteEntityRequest, DeleteLinkRequest,
    FindLinksRequest, GetEntityRequest, GetLinkRequest, ListEntitiesRequest, UpdateEntityRequest,
};
use this::server::exposure::rest::RestExposure;
use this::server::host::ServerHost;
use this::server::module_runtime::ModuleSource;
use this::storage::InMemoryLinkService;

/// `Bearer root` -> `Admin`; `Bearer admin` -> role `admin`; `Bearer user` -> no role.
struct Tokens;

#[async_trait]
impl AuthProvider for Tokens {
    async fn extract_context(&self, h: &HeaderMap) -> anyhow::Result<AuthContext> {
        let roles = match h.get("authorization").and_then(|v| v.to_str().ok()) {
            Some("Bearer root") => {
                return Ok(AuthContext::Admin {
                    admin_id: Uuid::from_u128(9),
                });
            }
            Some("Bearer admin") => vec!["admin".to_string()],
            Some("Bearer user") => vec![],
            _ => anyhow::bail!("no or bad token"),
        };
        Ok(AuthContext::User {
            user_id: Uuid::from_u128(1),
            tenant_id: Uuid::from_u128(2),
            roles,
        })
    }
    async fn is_owner(&self, _: &Uuid, _: &Uuid, _: &str) -> anyhow::Result<bool> {
        Ok(false)
    }
    async fn has_role(&self, _: &Uuid, _: &str) -> anyhow::Result<bool> {
        Ok(false)
    }
}

/// `ledger { entry { amount: f64 } }`: every CRUD operation requires the `admin` role.
fn ledger() -> ModuleDescriptor {
    let mut e = EntityDesc::new("entry");
    e.fields.push(FieldDesc::new("amount", FieldType::F64));
    for op in Operation::CRUD {
        e.permissions
            .ops
            .insert(op, PolicyExpr::HasRole(vec!["admin".into()]));
    }
    let mut m = ModuleDescriptor::new("ledger", "1.0.0");
    m.entities.push(e);
    m.normalize();
    m
}

/// `shop { item { qty: i64, code: string immutable, audit: string read_only, note?: string } }`,
/// open to any authenticated caller: used to attack the VALIDATION of declarative writes.
fn shop() -> ModuleDescriptor {
    let mut e = EntityDesc::new("item");
    e.fields.push(FieldDesc::new("qty", FieldType::I64));
    e.fields
        .push(FieldDesc::new("code", FieldType::String).immutable());
    e.fields.push(
        FieldDesc::new("audit", FieldType::String)
            .read_only()
            .optional(),
    );
    e.fields
        .push(FieldDesc::new("note", FieldType::String).optional());
    for op in Operation::CRUD {
        e.permissions.ops.insert(op, PolicyExpr::Authenticated);
    }
    let mut m = ModuleDescriptor::new("shop", "1.0.0");
    m.entities.push(e);
    m.normalize();
    m
}

struct Armed {
    host: Arc<ServerHost>,
    rest: Router,
    graphql: Router,
}

fn host(hot_load: bool) -> Armed {
    let b = ServerBuilder::new()
        .with_link_service(InMemoryLinkService::new())
        .with_module_runtime();
    let b = if hot_load {
        b.with_hot_load_enabled(true)
    } else {
        b
    };
    let host = b
        .build_host()
        .expect("host")
        .with_auth_provider(Arc::new(Tokens));
    assert!(host.authorize);
    let host = Arc::new(host);
    Armed {
        rest: RestExposure::build_router(host.clone(), vec![]).expect("REST"),
        graphql: GraphQLExposure::build_router(host.clone()).expect("GraphQL"),
        host,
    }
}

impl Armed {
    async fn rest(
        &self,
        method: Method,
        uri: &str,
        token: Option<&str>,
        body: Option<Value>,
    ) -> (StatusCode, Value) {
        let mut req = Request::builder().method(method).uri(uri);
        if let Some(t) = token {
            req = req.header("authorization", format!("Bearer {t}"));
        }
        let req = match body {
            Some(b) => req
                .header("content-type", "application/json")
                .body(Body::from(b.to_string())),
            None => req.body(Body::empty()),
        }
        .unwrap();
        let res = self.rest.clone().oneshot(req).await.unwrap();
        let status = res.status();
        let bytes = axum::body::to_bytes(res.into_body(), usize::MAX)
            .await
            .unwrap();
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        )
    }

    async fn gql(&self, token: &str, query: &str, variables: Value) -> Value {
        let res = self
            .graphql
            .clone()
            .oneshot(
                Request::post("/graphql")
                    .header("content-type", "application/json")
                    .header("authorization", format!("Bearer {token}"))
                    .body(Body::from(
                        json!({ "query": query, "variables": variables }).to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        let bytes = axum::body::to_bytes(res.into_body(), usize::MAX)
            .await
            .unwrap();
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    }

    fn grpc<T>(&self, token: &str, msg: T) -> tonic::Request<T> {
        let mut req = tonic::Request::new(msg);
        req.metadata_mut()
            .insert("authorization", format!("Bearer {token}").parse().unwrap());
        req
    }
}

fn denied(s: StatusCode) -> bool {
    s == StatusCode::UNAUTHORIZED
        || s == StatusCode::FORBIDDEN
        || s == StatusCode::NOT_FOUND
        || s == StatusCode::METHOD_NOT_ALLOWED
        || s == StatusCode::BAD_REQUEST
}

/// (a) REST: a caller without the role reaches NOTHING of a hot-loaded admin-only
/// module, whatever the spelling of the path or the verb.
#[tokio::test]
async fn rest_probes_on_hot_loaded_admin_only_module_reach_nothing() {
    let a = host(true);
    a.host
        .module_runtime
        .clone()
        .unwrap()
        .load(ModuleSource::Manifest(ledger().to_json_pretty()))
        .await
        .expect("load");
    let (s, created) = a
        .rest(
            Method::POST,
            "/entries",
            Some("admin"),
            Some(json!({"name": "secret-row", "amount": 1.0})),
        )
        .await;
    assert_eq!(s, StatusCode::CREATED, "{created}");
    let id = created["id"].as_str().unwrap().to_string();
    let other = Uuid::new_v4();

    let mut leaks: Vec<String> = vec![];
    let mut probe = |label: String, s: StatusCode, v: &Value| {
        if !denied(s) || v.to_string().contains("secret-row") {
            leaks.push(format!("{label}: {s} {v}"));
        }
    };
    let paths = [
        "/entries".to_string(),
        "//entries".to_string(),
        "/entries/".to_string(),
        "/%65ntries".to_string(),
        "/%65ntries/".to_string(),
        "/entries?limit=1".to_string(),
        "/entries/.".to_string(),
        "/./entries".to_string(),
        "/x/../entries".to_string(),
        "/Entries".to_string(),
        format!("/entries/{id}"),
        format!("/%65ntries/{id}"),
        format!("//entries/{id}"),
        format!("/entries//{id}"),
        format!("/entries/{id}/"),
        format!("/entries/{id}/entries"),
        format!("/entries/{id}/entries/{other}"),
        format!("/entries/{id}/anything/{other}"),
        format!("/entries/{id}/%2e%2e/{id}"),
    ];
    for p in &paths {
        for m in [
            Method::GET,
            Method::HEAD,
            Method::POST,
            Method::PUT,
            Method::PATCH,
            Method::DELETE,
            Method::OPTIONS,
        ] {
            for token in [Some("user"), None] {
                let body = (m != Method::GET && m != Method::HEAD && m != Method::DELETE)
                    .then(|| json!({"name": "x", "amount": 2.0}));
                let (s, v) = a.rest(m.clone(), p, token, body).await;
                // OPTIONS is CORS preflight territory: allowed to answer 2xx with no body.
                if m == Method::OPTIONS && s.is_success() && v.is_null() {
                    continue;
                }
                probe(format!("{m} {p} as {token:?}"), s, &v);
            }
        }
    }
    // The admin still sees exactly the one row (nothing was created/changed by the probes).
    let (_, list) = a.rest(Method::GET, "/entries", Some("admin"), None).await;
    assert_eq!(list["data"].as_array().map(Vec::len), Some(1), "{list}");
    assert!(
        leaks.is_empty(),
        "{} bypass(es):\n- {}",
        leaks.len(),
        leaks.join("\n- ")
    );
}

/// (a) GraphQL: aliases, fragments, variables, nested selections, batches and
/// introspection-driven shortcuts do not read or write what the role forbids.
#[tokio::test]
async fn graphql_probes_on_hot_loaded_admin_only_module_reach_nothing() {
    let a = host(true);
    a.host
        .module_runtime
        .clone()
        .unwrap()
        .load(ModuleSource::Manifest(ledger().to_json_pretty()))
        .await
        .expect("load");
    let (_, created) = a
        .rest(
            Method::POST,
            "/entries",
            Some("admin"),
            Some(json!({"name": "secret-row", "amount": 1.0})),
        )
        .await;
    let id = created["id"].as_str().unwrap().to_string();

    let queries: Vec<(String, Value)> = vec![
        ("{ entries { id name } }".to_string(), json!({})),
        ("{ a: entries { id } b: entries { id } }".to_string(), json!({})),
        (
            "query { ...F } fragment F on Query { entries { id name } }".to_string(),
            json!({}),
        ),
        (
            "query($i: String!) { entry(id: $i) { id name } }".to_string(),
            json!({"i": id}),
        ),
        (format!("{{ entry(id: \"{id}\") {{ id name amount }} }}"), json!({})),
        (
            "query($d: Boolean!) { entries @include(if: $d) { id name } }".to_string(),
            json!({"d": true}),
        ),
        (
            "mutation { createEntry(data: {name: \"x\", amount: 2.0}) { id } }".to_string(),
            json!({}),
        ),
        (
            "mutation($d: JSON!) { createEntry(data: $d) { id } }".to_string(),
            json!({"d": {"name": "x", "amount": 2.0}}),
        ),
        (format!("mutation {{ updateEntry(id: \"{id}\", data: {{name: \"hijack\"}}) {{ id }} }}"),
            json!({}),
        ),
        (format!("mutation {{ deleteEntry(id: \"{id}\") }}"),
            json!({}),
        ),
        (
            "mutation { a: createEntry(data: {name: \"x\", amount: 1.0}) { id } b: createEntry(data: {name: \"y\", amount: 1.0}) { id } }".to_string(),
            json!({}),
        ),
    ];
    let mut leaks = vec![];
    for (q, vars) in &queries {
        let out = a.gql("user", q, vars.clone()).await;
        let data_has_row = out["data"].to_string().contains("secret-row")
            || out["data"]
                .as_object()
                .is_some_and(|d| d.values().any(|v| !v.is_null()));
        if data_has_row {
            leaks.push(format!("{q}: {out}"));
        }
    }
    // Batched operations in one HTTP request (array body).
    let res = a
        .graphql
        .clone()
        .oneshot(
            Request::post("/graphql")
                .header("content-type", "application/json")
                .header("authorization", "Bearer user")
                .body(Body::from(
                    json!([{"query": "{ entries { id name } }"}, {"query": "{ entries { id } }"}])
                        .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    let bytes = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    let text = String::from_utf8_lossy(&bytes).to_string();
    if text.contains("secret-row") {
        leaks.push(format!("batched array: {text}"));
    }
    // The row is intact.
    let (_, got) = a
        .rest(Method::GET, &format!("/entries/{id}"), Some("admin"), None)
        .await;
    assert_eq!(got["name"], "secret-row", "row modified by a probe: {got}");
    let (_, list) = a.rest(Method::GET, "/entries", Some("admin"), None).await;
    assert_eq!(list["data"].as_array().map(Vec::len), Some(1), "{list}");
    assert!(
        leaks.is_empty(),
        "{} bypass(es):\n- {}",
        leaks.len(),
        leaks.join("\n- ")
    );
}

/// (a) gRPC: every entity RPC and the link service refuse a caller without the role,
/// including the "joker" link calls (empty link type / entity type).
#[tokio::test]
async fn grpc_probes_on_hot_loaded_admin_only_module_reach_nothing() {
    let a = host(true);
    a.host
        .module_runtime
        .clone()
        .unwrap()
        .load(ModuleSource::Manifest(ledger().to_json_pretty()))
        .await
        .expect("load");
    let (_, created) = a
        .rest(
            Method::POST,
            "/entries",
            Some("admin"),
            Some(json!({"name": "secret-row", "amount": 1.0})),
        )
        .await;
    let id = created["id"].as_str().unwrap().to_string();
    // A link from the protected entry, created behind the framework's back.
    let link = this::core::link::LinkEntity::new(
        "owns",
        Uuid::parse_str(&id).unwrap(),
        Uuid::new_v4(),
        None,
    );
    let link_id = link.id;
    a.host.link_service.create(link).await.unwrap();

    let svc = EntityServiceImpl::new(a.host.clone());
    let links = LinkServiceImpl::new(a.host.clone());
    let mut leaks: Vec<String> = vec![];
    let mut note = |what: String, r: Result<String, tonic::Status>| {
        let what = what.as_str();
        if let Ok(v) = r {
            leaks.push(format!("{what}: allowed ({v})"));
        }
    };
    note(
        "get".to_string(),
        svc.get_entity(a.grpc(
            "user",
            GetEntityRequest {
                entity_type: "entry".into(),
                entity_id: id.clone(),
            },
        ))
        .await
        .map(|r| format!("{:?}", r.into_inner())),
    );
    note(
        "list".to_string(),
        svc.list_entities(a.grpc(
            "user",
            ListEntitiesRequest {
                entity_type: "entry".into(),
                limit: 10,
                offset: 0,
            },
        ))
        .await
        .map(|r| format!("{:?}", r.into_inner())),
    );
    // Entity type spelled differently.
    for t in ["Entry", "entries", "ENTRY", "entry ", "ledger.entry"] {
        note(
            format!("list as `{t}`"),
            svc.list_entities(a.grpc(
                "user",
                ListEntitiesRequest {
                    entity_type: t.into(),
                    limit: 10,
                    offset: 0,
                },
            ))
            .await
            .map(|r| format!("{:?}", r.into_inner())),
        );
    }
    note(
        "update".to_string(),
        svc.update_entity(a.grpc(
            "user",
            UpdateEntityRequest {
                entity_type: "entry".into(),
                entity_id: id.clone(),
                data: None,
            },
        ))
        .await
        .map(|r| format!("{:?}", r.into_inner())),
    );
    note(
        "delete".to_string(),
        svc.delete_entity(a.grpc(
            "user",
            DeleteEntityRequest {
                entity_type: "entry".into(),
                entity_id: id.clone(),
            },
        ))
        .await
        .map(|r| format!("{:?}", r.into_inner())),
    );
    note(
        "create".to_string(),
        svc.create_entity(a.grpc(
            "user",
            CreateEntityRequest {
                entity_type: "entry".into(),
                data: None,
            },
        ))
        .await
        .map(|r| format!("{:?}", r.into_inner())),
    );
    // Link service: jokers.
    for (lt, et) in [
        ("", ""),
        ("owns", ""),
        ("", "entry"),
        ("owns", "entry"),
        ("*", ""),
    ] {
        for by_target in [false, true] {
            let req = a.grpc(
                "user",
                FindLinksRequest {
                    entity_id: id.clone(),
                    link_type: lt.into(),
                    entity_type: et.into(),
                },
            );
            let r = if by_target {
                links.find_links_by_target(req).await
            } else {
                links.find_links_by_source(req).await
            };
            note(
                format!("find_links(by_target={by_target}, type={lt:?}, entity={et:?})"),
                r.map(|r| format!("{:?}", r.into_inner())),
            );
        }
    }
    note(
        "get_link".to_string(),
        links
            .get_link(a.grpc(
                "user",
                GetLinkRequest {
                    link_id: link_id.to_string(),
                },
            ))
            .await
            .map(|r| format!("{:?}", r.into_inner())),
    );
    note(
        "create_link".to_string(),
        links
            .create_link(a.grpc(
                "user",
                CreateLinkRequest {
                    link_type: "owns".into(),
                    source_id: id.clone(),
                    target_id: Uuid::new_v4().to_string(),
                    metadata: None,
                },
            ))
            .await
            .map(|r| format!("{:?}", r.into_inner())),
    );
    note(
        "delete_link".to_string(),
        links
            .delete_link(a.grpc(
                "user",
                DeleteLinkRequest {
                    link_id: link_id.to_string(),
                },
            ))
            .await
            .map(|r| format!("{:?}", r.into_inner())),
    );
    assert!(
        a.host.link_service.get(&link_id).await.unwrap().is_some(),
        "the link was deleted by a caller without the role"
    );
    assert!(
        leaks.is_empty(),
        "{} bypass(es):\n- {}",
        leaks.len(),
        leaks.join("\n- ")
    );
}

/// (c) Hot loading is CLOSED by default on EVERY entry: Rust API (load, replace,
/// unload, load_offered), the admin REST interface (even for an admin) and nothing is
/// served afterwards.
#[tokio::test]
async fn hot_load_closed_by_default_on_every_entry() {
    let a = host(false);
    let rt = a.host.module_runtime.clone().unwrap();
    let manifest = ledger().to_json_pretty();
    rt.offer("ledger", ModuleSource::Manifest(manifest.clone()))
        .await;
    let mut open: Vec<String> = vec![];
    if rt
        .load(ModuleSource::Manifest(manifest.clone()))
        .await
        .is_ok()
    {
        open.push("load(Manifest)".into());
    }
    if rt.load_offered("ledger").await.is_ok() {
        open.push("load_offered".into());
    }
    if rt
        .replace(ModuleSource::Manifest(manifest.clone()))
        .await
        .is_ok()
    {
        open.push("replace".into());
    }
    if rt.unload("ledger").await.is_ok() {
        open.push("unload".into());
    }
    for (m, uri, body) in [
        (
            Method::POST,
            "/admin/modules",
            Some(json!({"manifest": serde_json::from_str::<Value>(&manifest).unwrap()})),
        ),
        (
            Method::POST,
            "/admin/modules",
            Some(json!({"module": "ledger"})),
        ),
        (Method::DELETE, "/admin/modules/ledger", None),
        (Method::DELETE, "/admin/modules/ledger/", None),
        (
            Method::POST,
            "/admin/modules/",
            Some(json!({"module": "ledger"})),
        ),
    ] {
        let (s, v) = a.rest(m.clone(), uri, Some("root"), body).await;
        if s.is_success() {
            open.push(format!("admin REST {m} {uri}: {s} {v}"));
        }
    }
    let (s, v) = a.rest(Method::GET, "/entries", Some("admin"), None).await;
    assert!(
        s == StatusCode::NOT_FOUND || s == StatusCode::FORBIDDEN,
        "module served though hot loading is closed: {s} {v}"
    );
    assert!(open.is_empty(), "hot load open on: {open:?}");
}

/// (c) Admin interface with hot loading open: an authenticated non-admin (even with the
/// `admin` ROLE) is refused; the host admin is accepted; path spellings do not slip past
/// the prefix check.
#[tokio::test]
async fn admin_interface_requires_host_admin_even_when_open() {
    let a = host(true);
    let manifest: Value = serde_json::from_str(&ledger().to_json_pretty()).unwrap();
    let mut open: Vec<String> = vec![];
    for uri in [
        "/admin/modules",
        "/admin/modules/",
        "//admin/modules",
        "/admin//modules",
        "/%61dmin/modules",
        "/admin/%6dodules",
        "/admin/modules/%2e",
        "/ADMIN/modules",
    ] {
        for token in [Some("user"), Some("admin"), None] {
            let (s, v) = a
                .rest(
                    Method::POST,
                    uri,
                    token,
                    Some(json!({"manifest": manifest.clone()})),
                )
                .await;
            if s.is_success() {
                open.push(format!("{uri} as {token:?}: {s} {v}"));
            }
        }
    }
    let (s, _) = a.rest(Method::GET, "/entries", Some("admin"), None).await;
    assert!(
        s == StatusCode::NOT_FOUND || s == StatusCode::FORBIDDEN,
        "module got loaded by a non-admin: {s}; {open:?}"
    );
    assert!(open.is_empty(), "admin interface reachable: {open:?}");
    let (s, v) = a
        .rest(
            Method::POST,
            "/admin/modules",
            Some("root"),
            Some(json!({"manifest": manifest})),
        )
        .await;
    assert_eq!(s, StatusCode::CREATED, "host admin refused: {v}");
}

/// (c) A failed load / failed replace leaves NOTHING half-served, and a replace that fails
/// keeps the previous version serving.
#[tokio::test]
async fn failed_load_and_failed_replace_leave_a_consistent_state() {
    let a = host(true);
    let rt = a.host.module_runtime.clone().unwrap();
    // A descriptor failing the check (public in a manifest, host option off).
    let mut bad = ledger();
    bad.entities[0]
        .permissions
        .ops
        .insert(Operation::List, PolicyExpr::Public);
    assert!(
        rt.load(ModuleSource::Manifest(bad.to_json_pretty()))
            .await
            .is_err()
    );
    let (s, _) = a.rest(Method::GET, "/entries", Some("admin"), None).await;
    assert!(
        s == StatusCode::NOT_FOUND || s == StatusCode::FORBIDDEN,
        "half-served after failed load: {s}"
    );
    let (s, _) = a.rest(Method::GET, "/entries", Some("root"), None).await;
    assert!(
        s.is_client_error(),
        "half-served after failed load (root): {s}"
    );

    rt.load(ModuleSource::Manifest(ledger().to_json_pretty()))
        .await
        .expect("load");
    let (s, created) = a
        .rest(
            Method::POST,
            "/entries",
            Some("admin"),
            Some(json!({"name": "keep", "amount": 1.0})),
        )
        .await;
    assert_eq!(s, StatusCode::CREATED, "{created}");
    // Replace with a descriptor that makes list public: refused, the old one still serves.
    let mut wider = ledger();
    wider.version = "1.1.0".into();
    wider.entities[0]
        .permissions
        .ops
        .insert(Operation::List, PolicyExpr::Public);
    assert!(
        rt.replace(ModuleSource::Manifest(wider.to_json_pretty()))
            .await
            .is_err(),
        "replace admitted a public policy in a manifest"
    );
    let (s, v) = a.rest(Method::GET, "/entries", Some("user"), None).await;
    assert_eq!(
        s,
        StatusCode::FORBIDDEN,
        "policy widened by a failed replace: {v}"
    );
    let (s, v) = a.rest(Method::GET, "/entries", Some("admin"), None).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(
        v["data"].as_array().map(Vec::len),
        Some(1),
        "rows lost: {v}"
    );
}

/// (e) Validation of declarative writes: read-only, immutable, null, wrong types,
/// unknown and server-owned keys are refused on create AND update, and a refused write
/// leaves the row untouched.
#[tokio::test]
async fn declarative_writes_refuse_server_owned_immutable_null_and_mistyped() {
    let a = host(true);
    a.host
        .module_runtime
        .clone()
        .unwrap()
        .load(ModuleSource::Manifest(shop().to_json_pretty()))
        .await
        .expect("load");
    let (s, row) = a
        .rest(
            Method::POST,
            "/items",
            Some("user"),
            Some(json!({"name": "n", "qty": 1, "code": "c1"})),
        )
        .await;
    assert_eq!(s, StatusCode::CREATED, "{row}");
    let id = row["id"].as_str().unwrap().to_string();
    let uri = format!("/items/{id}");

    let mut bypass: Vec<String> = vec![];
    // Create attempts that must be refused.
    for (label, body) in [
        (
            "server id",
            json!({"name": "n", "qty": 1, "code": "c", "id": Uuid::new_v4()}),
        ),
        (
            "created_at",
            json!({"name": "n", "qty": 1, "code": "c", "created_at": "1999-01-01T00:00:00Z"}),
        ),
        (
            "updated_at",
            json!({"name": "n", "qty": 1, "code": "c", "updated_at": "1999-01-01T00:00:00Z"}),
        ),
        (
            "deleted_at",
            json!({"name": "n", "qty": 1, "code": "c", "deleted_at": null}),
        ),
        (
            "type",
            json!({"name": "n", "qty": 1, "code": "c", "type": "user"}),
        ),
        (
            "tenant_id",
            json!({"name": "n", "qty": 1, "code": "c", "tenant_id": Uuid::new_v4()}),
        ),
        (
            "read_only audit",
            json!({"name": "n", "qty": 1, "code": "c", "audit": "forged"}),
        ),
        (
            "unknown key",
            json!({"name": "n", "qty": 1, "code": "c", "zzz": 1}),
        ),
        (
            "null required",
            json!({"name": "n", "qty": null, "code": "c"}),
        ),
        ("null name", json!({"name": null, "qty": 1, "code": "c"})),
        ("missing required", json!({"name": "n", "code": "c"})),
        (
            "string as i64",
            json!({"name": "n", "qty": "1", "code": "c"}),
        ),
        (
            "float as i64",
            json!({"name": "n", "qty": 1.5, "code": "c"}),
        ),
        (
            "huge as i64",
            json!({"name": "n", "qty": 1e30, "code": "c"}),
        ),
        (
            "array as string",
            json!({"name": "n", "qty": 1, "code": ["c"]}),
        ),
        ("object body array", json!([{"name": "n"}])),
        ("scalar body", json!("x")),
        ("null body", Value::Null),
    ] {
        let (s, v) = a
            .rest(Method::POST, "/items", Some("user"), Some(body))
            .await;
        if s.is_success() {
            bypass.push(format!("create {label}: {s} {v}"));
        }
    }
    // Update attempts that must be refused.
    for (label, body) in [
        ("immutable code", json!({"code": "c2"})),
        ("server id", json!({"id": Uuid::new_v4()})),
        ("created_at", json!({"created_at": "1999-01-01T00:00:00Z"})),
        ("updated_at", json!({"updated_at": "1999-01-01T00:00:00Z"})),
        ("type", json!({"type": "user"})),
        ("tenant_id", json!({"tenant_id": Uuid::new_v4()})),
        ("read_only audit", json!({"audit": "forged"})),
        ("unknown key", json!({"zzz": 1})),
        ("null required", json!({"qty": null})),
        ("null name", json!({"name": null})),
        ("string as i64", json!({"qty": "7"})),
        ("object as string", json!({"note": {"a": 1}})),
        ("status null", json!({"status": null})),
        ("deleted_at", json!({"deleted_at": "1999-01-01T00:00:00Z"})),
    ] {
        for m in [Method::PATCH, Method::PUT] {
            let (s, v) = a
                .rest(m.clone(), &uri, Some("user"), Some(body.clone()))
                .await;
            if s.is_success() {
                bypass.push(format!("{m} {label}: {s} {v}"));
            }
        }
    }
    let (_, now) = a.rest(Method::GET, &uri, Some("user"), None).await;
    for k in ["id", "type", "created_at", "code", "qty", "name"] {
        if now[k] != row[k] {
            bypass.push(format!("row field `{k}` changed: {} -> {}", row[k], now[k]));
        }
    }
    assert!(
        bypass.is_empty(),
        "{} bypass(es):\n- {}",
        bypass.len(),
        bypass.join("\n- ")
    );
}

/// (f) A client cannot force an unbounded read: `limit`, `page`, deep filters.
#[tokio::test]
async fn list_queries_are_bounded() {
    let a = host(true);
    a.host
        .module_runtime
        .clone()
        .unwrap()
        .load(ModuleSource::Manifest(shop().to_json_pretty()))
        .await
        .expect("load");
    for i in 0..130 {
        let (s, v) = a
            .rest(
                Method::POST,
                "/items",
                Some("user"),
                Some(json!({"name": format!("n{i}"), "qty": i, "code": "c"})),
            )
            .await;
        assert_eq!(s, StatusCode::CREATED, "{v}");
    }
    let mut bad = vec![];
    for q in [
        "limit=100000000",
        "limit=18446744073709551615",
        "page=18446744073709551615&limit=100",
        "page=4294967296&limit=100",
        "limit=0",
    ] {
        let (s, v) = a
            .rest(Method::GET, &format!("/items?{q}"), Some("user"), None)
            .await;
        let n = v["data"].as_array().map(Vec::len).unwrap_or(0);
        if n > 100 || s.is_server_error() {
            bad.push(format!("{q}: {s} rows={n}"));
        }
    }
    // A very deep filter must be refused, not crash the worker (stack) or loop.
    let deep = format!(
        "{}{}{}",
        r#"{"not":"#.repeat(2000),
        r#"{"cmp":{"field":"qty","op":"eq","value":1}}"#,
        "}".repeat(2000)
    );
    let enc = deep
        .replace('{', "%7B")
        .replace('}', "%7D")
        .replace('"', "%22");
    let (s, _) = a
        .rest(
            Method::GET,
            &format!("/items?filter={enc}"),
            Some("user"),
            None,
        )
        .await;
    if s.is_server_error() || s.is_success() {
        bad.push(format!("deep filter: {s}"));
    }
    assert!(bad.is_empty(), "{bad:?}");
}

/// (b) A manifest cannot widen access to ANONYMOUS callers through a negation when the host
/// did not allow `public`: `not:deny` is `public` by another name. Refused at load; nothing
/// served; the anonymous caller gets nothing either.
#[tokio::test]
async fn manifest_cannot_admit_anonymous_through_negation() {
    let a = host(true);
    let rt = a.host.module_runtime.clone().unwrap();
    for (i, pol) in [
        "not:deny",
        "not:role:banned",
        "any:authenticated,not:authenticated",
    ]
    .iter()
    .enumerate()
    {
        let mut m = ledger();
        m.name = format!("neg{i}");
        m.entities[0].name = format!("thing{i}");
        m.entities[0].plural = format!("things{i}");
        for op in Operation::CRUD {
            m.entities[0]
                .permissions
                .ops
                .insert(op, PolicyExpr::parse(pol).unwrap());
        }
        let r = rt.load(ModuleSource::Manifest(m.to_json_pretty())).await;
        assert!(
            r.is_err(),
            "`{pol}` loaded from a manifest: anonymous access"
        );
        let (s, _) = a
            .rest(Method::GET, &format!("/things{i}"), None, None)
            .await;
        assert!(s.is_client_error(), "`{pol}` served: {s}");
    }
}
