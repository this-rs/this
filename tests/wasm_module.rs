//! A REAL module compiled to wasm, loaded and served as a normal module.
//!
//! The guest is written in WAT (assembled with `wat`, no forge needed): it has its own
//! logic — it refuses an empty title with its own message, loops on a "boom" title, and
//! answers the named operation `add_task` with a `put` effect. Everything goes through the
//! ordinary surface: `DefaultModuleLoader` → `ModuleInterface` (creators, fetchers, routes).
#![cfg(feature = "wasm-modules")]

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use ed25519_dalek::SigningKey;
use serde_json::{Value, json};
use tower::ServiceExt;

use this::descriptor::{
    ContractError, EntityDesc, FieldDesc, FieldType, ModuleDescriptor, Operation, PolicyExpr,
};
use this::server::module_runtime::{
    DefaultModuleLoader, ModuleInterface, ModuleLoader, ModuleSource, WasmModuleLoader,
};
use this::storage::vfs::module_store::TrustAnchor;
use this::storage::vfs::{InMemoryVfs, ModuleStore, VfsModuleStore};
use this_module_harness::call::Limits;

// ── fixtures ────────────────────────────────────────────────────────────────

fn key() -> SigningKey {
    SigningKey::from_bytes(&[7u8; 32])
}

/// A store that admits code: signs what it commits, trusts its own key.
fn store() -> Arc<VfsModuleStore> {
    let mut anchor = TrustAnchor::new();
    anchor.insert("test", key().verifying_key());
    Arc::new(
        VfsModuleStore::new(Arc::new(InMemoryVfs::new()))
            .with_signing_key("test", key())
            .with_trust_anchor(anchor),
    )
}

fn taches(version: &str) -> ModuleDescriptor {
    let mut m = ModuleDescriptor::new("taches", version);
    let mut e = EntityDesc::new("task");
    e.fields
        .push(FieldDesc::new("title", FieldType::String).tag(16));
    for op in Operation::CRUD {
        e.permissions.ops.insert(op, PolicyExpr::Authenticated);
    }
    m.entities.push(e);
    m
}

/// Bytes as WAT escapes: a JSON enters a data segment without a quote to escape.
fn esc(s: &str) -> String {
    s.bytes().map(|b| format!("\\{b:02x}")).collect()
}

/// The guest. `manifest` is what `manifest()` answers; `imports` are extra WAT lines.
fn guest(manifest: &str, imports: &str, start: &str) -> Vec<u8> {
    let strings: [&str; 8] = [
        manifest,
        r#""title":"""#,
        r#""title":"boom""#,
        r#""name":"evil""#,
        r#""name":"add_task""#,
        r#"{"ok":false,"error":{"code":"taches.invalid_title","message":"le titre est vide","field":"title"}}"#,
        r#"{"ok":true,"effects":[{"kind":"put","entity":"task","body":{"title":"ajoutee"}},{"kind":"teleport"}]}"#,
        r#"{"ok":true,"output":{"added":true},"effects":[{"kind":"put","entity":"task","body":{"title":"ajoutee"}},{"kind":"log","level":"info","message":"add_task"}]}"#,
    ];
    let mut at = 16usize;
    let mut slots = Vec::new();
    let mut data = String::new();
    for s in strings {
        slots.push((at, s.len()));
        data.push_str(&format!("(data (i32.const {at}) \"{}\")\n", esc(s)));
        at += s.len() + 8;
    }
    assert!(at < 8192, "data overlaps the heap");
    let pack = |(p, l): (usize, usize)| ((p as u64) << 32) | l as u64;
    let ok = r#"{"ok":true}"#;
    let ok_at = at;
    data.push_str(&format!("(data (i32.const {ok_at}) \"{}\")\n", esc(ok)));
    let wat = format!(
        r#"(module
  {imports}
  (memory (export "memory") 2)
  (global $heap (mut i32) (i32.const 8192))
  {data}
  {start}
  (func (export "alloc") (param $n i32) (result i32)
    (local $p i32)
    (local.set $p (global.get $heap))
    (global.set $heap (i32.add (global.get $heap) (local.get $n)))
    (local.get $p))
  (func $contains (param $h i32) (param $hl i32) (param $p i32) (param $pl i32) (result i32)
    (local $i i32) (local $j i32)
    (if (i32.lt_u (local.get $hl) (local.get $pl)) (then (return (i32.const 0))))
    (block $done
      (loop $outer
        (br_if $done (i32.gt_u (local.get $i) (i32.sub (local.get $hl) (local.get $pl))))
        (local.set $j (i32.const 0))
        (block $mismatch
          (loop $inner
            (if (i32.ge_u (local.get $j) (local.get $pl)) (then (return (i32.const 1))))
            (br_if $mismatch (i32.ne
              (i32.load8_u (i32.add (local.get $h) (i32.add (local.get $i) (local.get $j))))
              (i32.load8_u (i32.add (local.get $p) (local.get $j)))))
            (local.set $j (i32.add (local.get $j) (i32.const 1)))
            (br $inner)))
        (local.set $i (i32.add (local.get $i) (i32.const 1)))
        (br $outer)))
    (i32.const 0))
  (func (export "manifest") (result i64) (i64.const {manifest_packed}))
  (func (export "handle") (param $op i32) (param $opl i32) (param $req i32) (param $rl i32) (result i64)
    (if (call $contains (local.get $req) (local.get $rl) (i32.const {boom}) (i32.const {boom_len}))
      (then (loop $l (br $l))))
    (if (call $contains (local.get $req) (local.get $rl) (i32.const {empty}) (i32.const {empty_len}))
      (then (return (i64.const {r_empty}))))
    (if (call $contains (local.get $req) (local.get $rl) (i32.const {evil}) (i32.const {evil_len}))
      (then (return (i64.const {r_evil}))))
    (if (call $contains (local.get $req) (local.get $rl) (i32.const {add}) (i32.const {add_len}))
      (then (return (i64.const {r_add}))))
    (i64.const {r_ok}))
)"#,
        manifest_packed = pack(slots[0]),
        empty = slots[1].0,
        empty_len = slots[1].1,
        boom = slots[2].0,
        boom_len = slots[2].1,
        evil = slots[3].0,
        evil_len = slots[3].1,
        add = slots[4].0,
        add_len = slots[4].1,
        r_empty = pack(slots[5]),
        r_evil = pack(slots[6]),
        r_add = pack(slots[7]),
        r_ok = pack((ok_at, ok.len())),
    );
    wat::parse_str(&wat).expect("WAT assembles")
}

fn manifest_of(d: &ModuleDescriptor) -> String {
    serde_json::to_string(d).unwrap()
}

async fn publish(store: &VfsModuleStore, descriptor: &ModuleDescriptor, wasm: &[u8]) {
    store
        .put_with_assets(descriptor, &[], &[("program.wasm", wasm)])
        .await
        .expect("a signed module with code is admitted");
}

fn loader(store: Arc<VfsModuleStore>) -> DefaultModuleLoader {
    let s: Arc<dyn ModuleStore> = store;
    DefaultModuleLoader::new().with_plugin("wasm", Arc::new(WasmModuleLoader::new(s)))
}

async fn load(l: &DefaultModuleLoader, location: &str) -> Result<ModuleInterface, ContractError> {
    l.load(&ModuleSource::Wasm {
        location: location.to_string(),
    })
    .await
}

fn contract(e: anyhow::Error) -> ContractError {
    e.downcast::<ContractError>()
        .expect("a ContractError, not a panic or an opaque error")
}

async fn titles(i: &ModuleInterface) -> Vec<String> {
    i.fetchers["task"]
        .list_as_json(None, None)
        .await
        .unwrap()
        .iter()
        .map(|r| r["title"].as_str().unwrap().to_string())
        .collect()
}

async fn post(router: axum::Router, uri: &str, body: Value) -> (StatusCode, Value) {
    let res = router
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri(uri)
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = res.status();
    let bytes = axum::body::to_bytes(res.into_body(), 1 << 20)
        .await
        .unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

// ── tests ───────────────────────────────────────────────────────────────────

#[tokio::test]
async fn a_compiled_module_serves_create_get_list_with_its_own_logic() {
    let store = store();
    let d = taches("0.1.0");
    publish(&store, &d, &guest(&manifest_of(&d), "", "")).await;
    let l = loader(store);
    let i = load(&l, "taches@0.1.0").await.expect("loads");

    assert_eq!(i.descriptor.name, "taches");
    let created = i.creators["task"]
        .create_from_json(json!({"title": "écrire le loader"}))
        .await
        .expect("a valid title is accepted");
    assert_eq!(created["title"], "écrire le loader");
    let id = created["id"].as_str().unwrap().parse().unwrap();

    let err = contract(
        i.creators["task"]
            .create_from_json(json!({"title": ""}))
            .await
            .expect_err("an empty title is refused by the MODULE"),
    );
    assert_eq!(err.message, "le titre est vide", "the module's own message");
    assert_eq!(err.code.as_str(), "taches.invalid_title");
    assert_eq!(err.fields[0].path, "title");

    let got = i.fetchers["task"].fetch_as_json(&id).await.unwrap();
    assert_eq!(got["title"], "écrire le loader");
    assert_eq!(titles(&i).await, vec!["écrire le loader"]);

    // The ordinary REST route carries the module's refusal as a 4xx with its message.
    let router = i.routes.clone().expect("routes");
    let (status, body) = post(router, "/tasks", json!({"title": ""})).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(body["error"], "le titre est vide");
}

#[tokio::test]
async fn a_bare_name_loads_the_latest_version() {
    let store = store();
    for v in ["0.1.0", "0.2.0"] {
        let d = taches(v);
        publish(&store, &d, &guest(&manifest_of(&d), "", "")).await;
    }
    let l = loader(store);
    let i = load(&l, "taches").await.expect("loads");
    assert_eq!(i.descriptor.version, "0.2.0");
}

#[tokio::test]
async fn a_named_operation_applies_its_effects_through_the_host() {
    let store = store();
    let d = taches("0.1.0");
    publish(&store, &d, &guest(&manifest_of(&d), "", "")).await;
    let l = loader(store);
    let i = load(&l, "taches@0.1.0").await.unwrap();

    let router = i.routes.clone().unwrap();
    let (status, body) = post(router, "/tasks/ops/add_task", json!({})).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body, json!({"added": true}));
    assert_eq!(titles(&i).await, vec!["ajoutee"], "the `put` was applied");
}

#[tokio::test]
async fn a_looping_guest_is_stopped_by_its_fuel_and_the_runtime_survives() {
    let store = store();
    let d = taches("0.1.0");
    publish(&store, &d, &guest(&manifest_of(&d), "", "")).await;
    let s: Arc<dyn ModuleStore> = store;
    let limits = Limits {
        fuel: 300_000,
        ..Limits::module_abi()
    };
    let l = DefaultModuleLoader::new().with_plugin(
        "wasm",
        Arc::new(WasmModuleLoader::new(s).with_limits(limits)),
    );
    let i = load(&l, "taches@0.1.0").await.unwrap();

    let err = contract(
        i.creators["task"]
            .create_from_json(json!({"title": "boom"}))
            .await
            .expect_err("the loop must be stopped"),
    );
    assert!(
        err.params["reason"].as_str().unwrap().contains("fuel"),
        "the refusal names the fuel: {err:?}"
    );
    assert!(
        titles(&i).await.is_empty(),
        "nothing stored by a stopped call"
    );

    // Same interface, next call: a fresh instance, normal answer.
    i.creators["task"]
        .create_from_json(json!({"title": "après"}))
        .await
        .expect("the runtime stays usable");
    assert_eq!(titles(&i).await, vec!["après"]);
}

#[tokio::test]
async fn a_module_that_declares_an_import_is_refused_before_instantiation() {
    let store = store();
    let d = taches("0.1.0");
    // If it were instantiated, the start function would trap with another message.
    let wasm = guest(
        &manifest_of(&d),
        r#"(import "host" "now" (func $now (result i64)))"#,
        "(func $s unreachable) (start $s)",
    );
    publish(&store, &d, &wasm).await;
    let l = loader(store);
    let err = load(&l, "taches@0.1.0").await.err().expect("refused");
    let reason = err.params["reason"].as_str().unwrap();
    assert!(reason.contains("host.now"), "names the import: {reason}");
    assert!(reason.contains("import"), "{reason}");
}

#[tokio::test]
async fn an_invalid_manifest_is_refused_by_check_with_the_descriptor_error_code() {
    let store = store();
    let d = taches("0.1.0");
    // The store holds a valid descriptor; the guest's `manifest()` answers another one,
    // with a field declared twice.
    let mut bad = taches("0.1.0");
    let dup = bad.entities[0].fields[0].clone();
    bad.entities[0].fields.push(dup);
    publish(&store, &d, &guest(&manifest_of(&bad), "", "")).await;
    let l = loader(store);
    let err = load(&l, "taches@0.1.0").await.err().expect("refused");
    assert!(
        err.code.as_str().starts_with("this.descriptor."),
        "descriptor error code, got {}",
        err.code.as_str()
    );
}

#[tokio::test]
async fn a_manifest_that_contradicts_the_stored_identity_is_refused() {
    let store = store();
    let d = taches("0.1.0");
    let mut other = taches("0.1.0");
    other.name = "autres".into();
    publish(&store, &d, &guest(&manifest_of(&other), "", "")).await;
    let l = loader(store);
    let err = load(&l, "taches@0.1.0").await.err().expect("refused");
    assert_eq!(err.code.as_str(), "this.module.identity_mismatch");
}

#[tokio::test]
async fn an_effect_of_unknown_kind_is_refused_and_nothing_is_written() {
    let store = store();
    let d = taches("0.1.0");
    publish(&store, &d, &guest(&manifest_of(&d), "", "")).await;
    let l = loader(store);
    let i = load(&l, "taches@0.1.0").await.unwrap();

    // The guest answers `[put(valid), teleport]`: the valid one must NOT be applied.
    let router = i.routes.clone().unwrap();
    let (status, body) = post(router, "/tasks/ops/evil", json!({})).await;
    assert!(!status.is_success(), "refused, got {status} {body}");
    assert!(
        titles(&i).await.is_empty(),
        "no partial effect: {:?}",
        titles(&i).await
    );
}

#[tokio::test]
async fn code_that_was_modified_in_the_store_is_never_served() {
    let vfs = Arc::new(InMemoryVfs::new());
    let mut anchor = TrustAnchor::new();
    anchor.insert("test", key().verifying_key());
    let store = VfsModuleStore::new(vfs.clone())
        .with_signing_key("test", key())
        .with_trust_anchor(anchor);
    let d = taches("0.1.0");
    publish(&store, &d, &guest(&manifest_of(&d), "", "")).await;
    assert!(store.code("taches", "0.1.0").await.unwrap().is_some());

    use this::storage::vfs::{Precondition, Vfs, VfsPath};
    let path = VfsPath::parse("/modules/taches/0.1.0/assets/program.wasm").unwrap();
    vfs.write(&path, b"\0asm-tampered", Precondition::None)
        .await
        .unwrap();
    assert!(
        store.code("taches", "0.1.0").await.is_err(),
        "a tampered artifact is refused, never served"
    );
}

#[tokio::test]
async fn with_wasm_modules_registers_the_loader_directly() {
    let store = store();
    let d = taches("0.1.0");
    publish(&store, &d, &guest(&manifest_of(&d), "", "")).await;
    let s: Arc<dyn ModuleStore> = store;
    let l = DefaultModuleLoader::new().with_wasm_modules(s);
    let i = load(&l, "taches@0.1.0").await.expect("loads");
    i.creators["task"]
        .create_from_json(json!({"title": "direct"}))
        .await
        .unwrap();
}

#[tokio::test]
async fn without_a_registered_loader_a_wasm_source_is_refused() {
    let l = DefaultModuleLoader::new();
    let err = load(&l, "taches@0.1.0").await.err().expect("fail-closed");
    assert_eq!(err.code.as_str(), "this.module.format_unsupported");
}
