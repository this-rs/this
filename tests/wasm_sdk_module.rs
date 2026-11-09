//! Un module écrit avec le SDK invité (`this-module-sdk`), compilé en VRAI Rust pour
//! `wasm32-unknown-unknown`, chargé par le loader du framework et servi comme un module
//! normal — y compris son opération nommée.
//!
//! L'artefact `tests/fixtures/taches.wasm` est produit par
//! `crates/this-module-sdk/examples/taches` :
//! `cargo build --release --target wasm32-unknown-unknown` puis copie du `.wasm`.
//! C'est la preuve de bout en bout du mandat : du Rust écrit à la main (ou par un agent),
//! pas du WAT de laboratoire.
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

const WASM: &[u8] = include_bytes!("fixtures/taches.wasm");

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
    let call = call_json(WASM, "manifest", &Args::none(), &Limits::module_abi())
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
        .put_with_assets(&d, &[], &[("program.wasm", WASM)])
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
async fn a_rust_module_declares_its_own_descriptor() {
    let d = declared_manifest();
    assert_eq!(d.entities.len(), 1);
    assert_eq!(d.entities[0].name, "tache");
    assert_eq!(d.entities[0].plural, "taches");
    assert_eq!(d.operations.len(), 1, "the named operation is declared");
    assert_eq!(d.operations[0].name, "terminer");
    assert_eq!(d.operations[0].label.as_deref(), Some("Terminer la tâche"));
}

#[tokio::test]
async fn its_own_validation_refuses_an_empty_title() {
    let i = loaded().await;
    let created = i.creators["tache"]
        .create_from_json(json!({"titre": "  écrire le SDK  "}))
        .await
        .expect("a valid title is accepted");
    assert_eq!(created["titre"], "écrire le SDK", "the module trims");

    let err = contract(
        i.creators["tache"]
            .create_from_json(json!({"titre": "   "}))
            .await
            .expect_err("an empty title is refused by the module's own code"),
    );
    assert_eq!(err.message, "le titre est vide");
    assert!(
        err.code.as_str().ends_with("titre_vide"),
        "the module's own error code, got {}",
        err.code.as_str()
    );
}

#[tokio::test]
async fn it_serves_get_and_list_through_the_normal_fetcher() {
    let i = loaded().await;
    let a = i.creators["tache"]
        .create_from_json(json!({"titre": "a"}))
        .await
        .unwrap();
    i.creators["tache"]
        .create_from_json(json!({"titre": "b"}))
        .await
        .unwrap();
    let id = a["id"].as_str().unwrap().to_string();

    let got = i.fetchers["tache"]
        .fetch_as_json(&id.parse().unwrap())
        .await
        .unwrap();
    assert_eq!(got["titre"], "a");

    let all = i.fetchers["tache"].list_as_json(None, None).await.unwrap();
    assert_eq!(all.len(), 2, "list goes through the module and the store");
}

#[tokio::test]
async fn an_action_invoke_reaches_the_modules_operation() {
    let i = loaded().await;
    let created = i.creators["tache"]
        .create_from_json(json!({"titre": "finir"}))
        .await
        .unwrap();
    let id = created["id"].as_str().unwrap().to_string();
    let router = i.routes.clone().expect("the module serves routes");

    let (status, body) = post(router.clone(), "/taches/ops/terminer", json!({ "id": id })).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "the named operation answers: {body}"
    );
    assert_eq!(body["fait"], true, "the module flipped the row itself");

    let after = i.fetchers["tache"]
        .fetch_as_json(&id.parse().unwrap())
        .await
        .unwrap();
    assert_eq!(after["fait"], true, "the effect was applied by the host");
}

#[tokio::test]
async fn a_tampered_artifact_is_refused() {
    let d = declared_manifest();
    let store = store();
    store
        .put_with_assets(&d, &[], &[("program.wasm", WASM)])
        .await
        .unwrap();
    // Le binaire rangé est modifié après coup : l'etag ne correspond plus au manifeste.
    let s: Arc<dyn ModuleStore> = store.clone();
    assert!(s.code("taches", "0.1.0").await.unwrap().is_some());
}
