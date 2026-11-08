//! Tests de bout en bout qui lancent le VRAI conteneur de build (docker, sans réseau).
//!
//! Ignorés par défaut : `cargo test` reste vert sans docker. Pour les lancer :
//!
//! ```text
//! docker build -t this-forge-builder:1.93 -f crates/this-forge/docker/Dockerfile crates/this-forge/docker
//! cargo test -p this-forge --test docker -- --ignored --nocapture --test-threads=1
//! ```
mod common;

use std::time::Instant;

use axum::http::StatusCode;
use common::*;
use serde_json::json;
use this_forge::runner::{BuildOutcome, BuildRunner, SourceFile};
use this_forge::{ForgeConfig, sign};

fn lib(content: &str) -> Vec<SourceFile> {
    vec![SourceFile {
        path: "src/lib.rs".into(),
        content: content.into(),
    }]
}

#[tokio::test]
#[ignore = "docker"]
async fn end_to_end_valid_build_is_signed_and_verifiable() {
    let h = harness().await;
    let (t, _, id) = h.signup("a@b.co").await;
    let t0 = Instant::now();
    let r = h.submit(&t, "demo", good_files()).await;
    assert_eq!(
        r.status,
        StatusCode::ACCEPTED,
        "{}",
        String::from_utf8_lossy(&r.bytes)
    );
    assert_eq!(r.json()["status"], "queued");
    let bid = r.json()["id"].as_str().unwrap().to_string();

    // SSE : status(building) … diagnostic* … artifact, done
    let events = h
        .call("GET", &format!("/v1/builds/{bid}/events"), Some(&t), None)
        .await;
    let text = String::from_utf8_lossy(&events.bytes).to_string();
    eprintln!("SSE:\n{text}");
    assert!(
        text.contains("event: status")
            && text.contains("event: artifact")
            && text.contains("event: done"),
        "{text}"
    );
    assert!(text.contains("building") || text.contains("queued"));

    let j = h.wait_build(&t, &bid).await;
    eprintln!(
        "build total (file + build + signature) : {:?}",
        t0.elapsed()
    );
    assert_eq!(j["status"], "signed", "{j}");
    let size = j["artifact"]["size"].as_i64().unwrap();
    eprintln!("taille de l'artefact : {size} octets");

    let art = h
        .call("GET", &format!("/v1/builds/{bid}/artifact"), Some(&t), None)
        .await;
    assert_eq!(art.status, StatusCode::OK);
    assert_eq!(art.headers["content-type"], "application/wasm");
    assert_eq!(art.bytes.len() as i64, size);
    assert_eq!(&art.bytes[..4], b"\0asm");

    // ce que fait l'appareil : clé publiée -> signature -> charge -> hash du wasm
    let key = h.call("GET", "/v1/forge/key", None, None).await.json();
    let manifest_b64 = art.headers["x-forge-manifest"].to_str().unwrap();
    let signature = art.headers["x-forge-signature"].to_str().unwrap();
    let manifest = sign::unb64url(manifest_b64).unwrap();
    sign::verify(key["public_key"].as_str().unwrap(), &manifest, signature)
        .expect("la signature se vérifie");
    let payload: serde_json::Value = serde_json::from_slice(&manifest).unwrap();
    assert_eq!(payload["wasm_sha256"], sign::sha256_hex(&art.bytes));
    assert_eq!(payload["account"], id);
    assert_eq!(
        payload["module"],
        json!({"name": "demo", "version": "1.0.0"})
    );
    assert_eq!(payload["forge_version"], "0.1.0");
    assert!(
        payload["toolchain"]
            .as_str()
            .unwrap()
            .starts_with("rustc 1.93")
    );
    assert_eq!(j["artifact"]["signature"], signature);
    // la charge est bien canonique (clés triées, compacte)
    assert_eq!(sign::canonical_json(&payload).as_bytes(), &manifest[..]);
    // une signature falsifiée, ou une autre charge, ne passe pas
    let mut tampered = manifest.clone();
    tampered[10] ^= 1;
    assert!(sign::verify(key["public_key"].as_str().unwrap(), &tampered, signature).is_err());
}

#[tokio::test]
#[ignore = "docker"]
async fn invalid_build_returns_structured_diagnostics() {
    let h = harness().await;
    let (t, _, _) = h.signup("a@b.co").await;
    let src = "pub fn f() -> u32 {\n    let x: u32 = \"pas un nombre\";\n    x\n}\n";
    let r = h
        .submit(
            &t,
            "broken",
            json!([{"path": "src/lib.rs", "content": src}]),
        )
        .await;
    let bid = r.json()["id"].as_str().unwrap().to_string();
    let j = h.wait_build(&t, &bid).await;
    eprintln!(
        "diagnostics : {}",
        serde_json::to_string_pretty(&j["diagnostics"]).unwrap()
    );
    assert_eq!(j["status"], "failed");
    assert!(j["artifact"].is_null());
    let d = &j["diagnostics"][0];
    assert_eq!(d["file"], "src/lib.rs");
    assert_eq!(d["line"], 2);
    assert_eq!(d["level"], "error");
    assert!(d["message"].as_str().unwrap().contains("mismatched types"));
    assert!(d["column"].as_u64().unwrap() > 0);
    let art = h
        .call("GET", &format!("/v1/builds/{bid}/artifact"), Some(&t), None)
        .await;
    assert_eq!(
        (art.status, art.code().as_str()),
        (StatusCode::CONFLICT, "build_failed")
    );
}

#[tokio::test]
#[ignore = "docker"]
async fn module_breaking_the_abi_is_not_signed() {
    let h = harness().await;
    let (t, _, _) = h.signup("a@b.co").await;
    // compile, mais n'exporte ni alloc, ni manifest, ni handle
    let r = h
        .submit(
            &t,
            "noabi",
            json!([{"path": "src/lib.rs", "content": "pub fn x() {}\n"}]),
        )
        .await;
    let j = h.wait_build(&t, r.json()["id"].as_str().unwrap()).await;
    assert_eq!(j["status"], "failed");
    let all = j["diagnostics"].to_string();
    eprintln!("ABI diagnostics : {all}");
    assert!(
        all.contains("`alloc` manquant") && all.contains("`handle` manquant"),
        "{all}"
    );
}

/// Le conteneur est SANS RÉSEAU : un `build.rs` (que la validation refuse, donc posé ici
/// hors validation) qui ouvre une connexion échoue.
#[tokio::test]
#[ignore = "docker"]
async fn module_that_reaches_for_the_network_fails() {
    let dir = tempfile::tempdir().unwrap();
    let runner = BuildRunner::new(ForgeConfig::new(dir.path()));
    let build_rs = r#"fn main() {
        match std::net::TcpStream::connect_timeout(&"1.1.1.1:80".parse().unwrap(), std::time::Duration::from_secs(5)) {
            Ok(_) => println!("cargo:warning=RESEAU ACCESSIBLE"),
            Err(e) => panic!("pas de réseau : {e}"),
        }
    }"#;
    match runner
        .run_with_extra(&lib("pub fn f() {}\n"), &[("build.rs", build_rs)])
        .await
    {
        BuildOutcome::Failed { diagnostics, .. } => {
            let all = format!("{diagnostics:?}");
            eprintln!("réseau : {all}");
            assert!(all.contains("pas de réseau"), "{all}");
            assert!(!all.contains("RESEAU ACCESSIBLE"));
        }
        BuildOutcome::Wasm { .. } => panic!("le build a pu atteindre le réseau !"),
    }
}

/// Une dépendance hors liste blanche est refusée hors ligne (le client ne peut de toute
/// façon pas fournir de Cargo.toml : c'est l'`extra` du test qui la glisse).
#[tokio::test]
#[ignore = "docker"]
async fn dependency_outside_the_vendored_set_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let runner = BuildRunner::new(ForgeConfig::new(dir.path()));
    match runner.run(&lib("use rand::Rng;\npub fn f() {}\n")).await {
        BuildOutcome::Failed { diagnostics, .. } => {
            let all = format!("{diagnostics:?}");
            eprintln!("dépendance : {all}");
            assert!(all.contains("rand"), "{all}");
        }
        BuildOutcome::Wasm { .. } => panic!("une dépendance non vendorée a compilé"),
    }
}

#[tokio::test]
#[ignore = "docker"]
async fn build_over_the_time_limit_is_killed() {
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = ForgeConfig::new(dir.path());
    cfg.build_timeout = std::time::Duration::from_millis(300);
    let runner = BuildRunner::new(cfg);
    match runner.run(&lib("pub fn f() {}\n")).await {
        BuildOutcome::Failed {
            diagnostics,
            elapsed,
        } => {
            assert!(
                diagnostics[0].message.contains("durée maximale"),
                "{diagnostics:?}"
            );
            assert!(elapsed.as_secs() < 30);
        }
        BuildOutcome::Wasm { .. } => panic!("aurait dû expirer"),
    }
}

/// L'isolement bout en bout : le build RÉEL de A est invisible de B.
#[tokio::test]
#[ignore = "docker"]
async fn isolation_end_to_end() {
    let h = harness().await;
    let (ta, _, _) = h.signup("a@b.co").await;
    let (tb, _, _) = h.signup("b@b.co").await;
    let r = h.submit(&ta, "mine", good_files()).await;
    let id = r.json()["id"].as_str().unwrap().to_string();
    assert_eq!(h.wait_build(&ta, &id).await["status"], "signed");
    for p in [
        format!("/v1/builds/{id}"),
        format!("/v1/builds/{id}/artifact"),
    ] {
        let rb = h.call("GET", &p, Some(&tb), None).await;
        eprintln!("B GET {p} -> {} {}", rb.status, rb.code());
        assert_eq!(rb.status, StatusCode::NOT_FOUND);
    }
    assert_eq!(
        h.call("GET", &format!("/v1/builds/{id}/artifact"), Some(&ta), None)
            .await
            .status,
        StatusCode::OK
    );
}
