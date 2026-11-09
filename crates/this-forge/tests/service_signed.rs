//! Un build qui RÉUSSIT, de la file à l'artefact signé, sans docker : `docker_bin` est un
//! faux exécutable (script `sh`) qui répond à `rustc -V` et dépose un wasm conforme à
//! l'ABI dans `/out`, comme le ferait le conteneur. Le runner et le worker sont les vrais.
//!
//! Seul dans son binaire de test : écrire un script puis l'exécuter pendant qu'un autre
//! test du même processus fait un `fork` expose à `ETXTBSY` (cf. `tests/runner.rs`).

use std::os::unix::fs::PermissionsExt;
use std::time::{Duration, Instant};

use this_forge::entities::Build;
use this_forge::runner::SourceFile;
use this_forge::sign::{sha256_hex, unb64url, verify};
use this_forge::{Forge, ForgeConfig, StoreKind};

const SCRIPT: &str = r#"#!/bin/sh
case " $* " in
  *" rustc -V "*) echo "rustc 1.93.1 (01f6ddf75 2026-02-11)"; exit 0;;
esac
out=""
for a in "$@"; do
  case "$a" in
    *:/out:rw) out="${a%:/out:rw}";;
  esac
done
cp "@WASM@" "$out/module.wasm"
echo '{"reason":"build-finished","success":true}'
exit 0
"#;

#[tokio::test]
async fn a_successful_build_is_stored_signed_with_a_verifiable_manifest() {
    let tools = tempfile::tempdir().unwrap();
    let wasm = wat::parse_str(
        r#"(module
            (memory (export "memory") 1)
            (func (export "alloc") (param i32) (result i32) local.get 0)
            (func (export "manifest") (result i64) i64.const 0)
            (func (export "handle") (param i32 i32 i32 i32) (result i64) i64.const 0))"#,
    )
    .unwrap();
    let wasm_path = tools.path().join("module.wasm");
    std::fs::write(&wasm_path, &wasm).unwrap();
    let bin = tools.path().join("docker");
    std::fs::write(&bin, SCRIPT.replace("@WASM@", wasm_path.to_str().unwrap())).unwrap();
    std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();

    let data = tempfile::tempdir().unwrap();
    let mut cfg = ForgeConfig::new(data.path());
    cfg.store = StoreKind::Memory;
    cfg.docker_bin = bin.to_str().unwrap().into();
    let forge = Forge::start(cfg).await.unwrap();

    let b = forge
        .store
        .builds
        .create(Build::new(
            "demo".into(),
            "queued".into(),
            "acct-1".into(),
            "1.2.3".into(),
            String::new(),
            "src-sha".into(),
            None,
            None,
            0,
            "[]".into(),
            None,
            None,
            None,
            None,
        ))
        .await
        .unwrap();
    let files = vec![SourceFile {
        path: "src/lib.rs".into(),
        content: "pub fn f() {}".into(),
    }];
    forge.enqueue(b.id, &files).await.unwrap();

    let deadline = Instant::now() + Duration::from_secs(5);
    let done = loop {
        let cur = forge.store.builds.get(&b.id).await.unwrap().unwrap();
        if cur.status != "queued" && cur.status != "building" {
            break cur;
        }
        assert!(
            Instant::now() < deadline,
            "resté `{}` plus de 5 s",
            cur.status
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    };

    assert_eq!(
        done.status, "signed",
        "diagnostics : {}",
        done.diagnostics_json
    );
    assert_eq!(done.toolchain, "rustc 1.93.1");
    assert_eq!(done.wasm_size, Some(wasm.len() as i64));
    assert_eq!(
        done.wasm_sha256.as_deref(),
        Some(sha256_hex(&wasm).as_str())
    );
    assert!(done.started_at.is_some() && done.finished_at.is_some());
    assert_eq!(done.diagnostics_json, "[]");
    assert_eq!(
        std::fs::read(forge.artifact_path(&b.id)).unwrap(),
        wasm,
        "l'artefact est écrit tel que produit"
    );
    assert!(
        !data
            .path()
            .join("sources")
            .join(format!("{}.json", b.id))
            .exists(),
        "les sources sont effacées"
    );

    // Le manifeste est l'attestation canonique, et la signature la couvre exactement.
    let manifest = unb64url(done.manifest_b64.as_deref().unwrap()).unwrap();
    let att: serde_json::Value = serde_json::from_slice(&manifest).unwrap();
    assert_eq!(att["account"], "acct-1");
    assert_eq!(att["module"]["name"], "demo");
    assert_eq!(att["module"]["version"], "1.2.3");
    assert_eq!(att["source_sha256"], "src-sha");
    assert_eq!(att["toolchain"], "rustc 1.93.1");
    assert_eq!(att["wasm_sha256"], sha256_hex(&wasm));
    assert_eq!(att["forge_version"], this_forge::service::FORGE_VERSION);
    assert_eq!(
        this_forge::service::canonical(&att).as_bytes(),
        manifest.as_slice()
    );
    let sig = done.signature.as_deref().unwrap();
    verify(&forge.keys.public_hex(), &manifest, sig).expect("signature valide");
    let mut tampered = manifest.clone();
    tampered[1] ^= 1;
    assert!(verify(&forge.keys.public_hex(), &tampered, sig).is_err());
}
