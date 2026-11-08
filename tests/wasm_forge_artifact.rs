//! Preuve de bout en bout : le `.wasm` PRODUIT PAR LA FORGE (téléchargé de
//! `GET /v1/builds/{id}/artifact`, signature Ed25519 déjà vérifiée) est chargé par le
//! loader du framework, qui sert un `create` puis l'opération nommée `terminer`.
//!
//! Aucun binaire du disque du dépôt : le fichier vient de `FORGE_WASM`.
//!   FORGE_WASM=/chemin/taches.wasm FORGE_WASM_SHA256=<hex> \
//!     cargo test --features wasm-modules --test wasm_forge_artifact -- --ignored --nocapture
#![cfg(feature = "wasm-modules")]

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use ed25519_dalek::SigningKey;
use serde_json::{Value, json};
use tower::ServiceExt;

use this::descriptor::{ContractError, ModuleDescriptor};
use this::server::module_runtime::{
    DefaultModuleLoader, ModuleInterface, ModuleLoader, ModuleSource, WasmModuleLoader,
};
use this::storage::vfs::module_store::TrustAnchor;
use this::storage::vfs::{InMemoryVfs, ModuleStore, VfsModuleStore};
use this_module_harness::call::{Args, Limits, call_json};

fn wasm() -> &'static [u8] {
    static W: std::sync::OnceLock<Vec<u8>> = std::sync::OnceLock::new();
    W.get_or_init(|| {
        let p = std::env::var("FORGE_WASM").expect("FORGE_WASM = chemin de l'artefact de la forge");
        let b = std::fs::read(&p).unwrap();
        println!("artefact de la forge : {p} ({} octets)", b.len());
        b
    })
}

fn key() -> SigningKey {
    SigningKey::from_bytes(&[9u8; 32])
}

fn store() -> Arc<VfsModuleStore> {
    let mut anchor = TrustAnchor::new();
    anchor.insert("test", key().verifying_key());
    Arc::new(
        VfsModuleStore::new(Arc::new(InMemoryVfs::new()))
            .with_signing_key("test", key())
            .with_trust_anchor(anchor),
    )
}

/// Le descripteur que le module déclare LUI-MÊME (`manifest()` du binaire compilé).
fn declared_manifest() -> ModuleDescriptor {
    let call = call_json(wasm(), "manifest", &Args::none(), &Limits::module_abi())
        .expect("manifest() answers");
    ModuleDescriptor::from_json(std::str::from_utf8(&call.response).unwrap())
        .expect("the module's own descriptor parses")
}

async fn loaded() -> ModuleInterface {
    let d = declared_manifest();
    assert_eq!(d.name, "taches");
    assert_eq!(d.version, "0.1.0");
    let store = store();
    store
        .put_with_assets(&d, &[], &[("program.wasm", wasm())])
        .await
        .expect("a signed module with code is admitted");
    let s: Arc<dyn ModuleStore> = store;
    let loader = DefaultModuleLoader::new().with_plugin("wasm", Arc::new(WasmModuleLoader::new(s)));
    loader
        .load(&ModuleSource::Wasm {
            location: "taches@0.1.0".into(),
        })
        .await
        .expect("the Rust module loads")
}

fn contract(e: anyhow::Error) -> ContractError {
    e.downcast::<ContractError>()
        .expect("a ContractError, not a panic")
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

#[tokio::test]
#[ignore = "demande FORGE_WASM (artefact téléchargé de la forge)"]
async fn the_forge_artifact_loads_and_serves_create_and_terminer() {
    if let Ok(want) = std::env::var("FORGE_WASM_SHA256") {
        use sha2::{Digest, Sha256};
        let got: String = Sha256::digest(wasm())
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        assert_eq!(got, want, "le binaire testé est celui que la forge a signé");
        println!("sha256 = {got} (identique à la charge signée)");
    }
    let d = declared_manifest();
    println!(
        "module déclaré par le binaire de la forge : {}@{}",
        d.name, d.version
    );
    let i = loaded().await;
    let created = i.creators["tache"]
        .create_from_json(json!({"titre": "  produit par la forge  "}))
        .await
        .expect("create");
    println!("create -> {created}");
    assert_eq!(created["titre"], "produit par la forge");
    let id = created["id"].as_str().unwrap().to_string();
    let router = i.routes.clone().expect("routes");
    let (status, body) = post(router, "/taches/ops/terminer", json!({ "id": id })).await;
    println!("POST /taches/ops/terminer -> {status} {body}");
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["fait"], true);
    let after = i.fetchers["tache"]
        .fetch_as_json(&id.parse().unwrap())
        .await
        .unwrap();
    assert_eq!(after["fait"], true);
    let err = contract(
        i.creators["tache"]
            .create_from_json(json!({"titre": "  "}))
            .await
            .expect_err("vide"),
    );
    println!(
        "titre vide refusé par le code du module : {} ({})",
        err.message,
        err.code.as_str()
    );
}
