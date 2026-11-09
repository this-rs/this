//! Le service de la forge sans docker : démarrage, reprise après redémarrage, worker,
//! invitations. Le runner pointe vers un binaire docker inexistant : chaque build échoue
//! vite et toujours de la même façon, sans dépendre de la machine.

use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use this_forge::entities::Build;
use this_forge::runner::{Diagnostic, SourceFile};
use this_forge::sign::sha256_hex;
use this_forge::store::Store;
use this_forge::{Forge, ForgeConfig, StoreKind};
use uuid::Uuid;

const NO_DOCKER: &str = "/nonexistent/this-forge-test/docker";

fn cfg(dir: &Path) -> ForgeConfig {
    let mut c = ForgeConfig::new(dir);
    c.store = StoreKind::Lmdb;
    c.docker_bin = NO_DOCKER.into();
    c
}

fn build(status: &str) -> Build {
    Build::new(
        "demo".into(),
        status.into(),
        "acct".into(),
        "1.0.0".into(),
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
    )
}

fn files() -> Vec<SourceFile> {
    vec![SourceFile {
        path: "src/lib.rs".into(),
        content: "pub fn f() {}".into(),
    }]
}

fn source_file(dir: &Path, id: &Uuid) -> std::path::PathBuf {
    dir.join("sources").join(format!("{id}.json"))
}

fn diagnostics(b: &Build) -> Vec<Diagnostic> {
    serde_json::from_str(&b.diagnostics_json).unwrap()
}

/// Attend qu'un build quitte `queued`/`building` : polling de 20 ms, borné à 5 s.
async fn settled(forge: &Forge, id: Uuid) -> Build {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let b = forge.store.builds.get(&id).await.unwrap().unwrap();
        if b.status != "queued" && b.status != "building" {
            return b;
        }
        assert!(
            Instant::now() < deadline,
            "le build {id} est resté `{}` plus de 5 s",
            b.status
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// Le worker traite UN build à la fois, dans l'ordre : quand ce témoin est réglé, tout
/// ce qui a été mis en file avant lui l'est aussi.
async fn drain(forge: &Arc<Forge>) {
    let witness = forge.store.builds.create(build("queued")).await.unwrap();
    forge.enqueue(witness.id, &files()).await.unwrap();
    assert_eq!(settled(forge, witness.id).await.status, "failed");
}

#[tokio::test]
async fn start_creates_its_directories_and_a_signing_key() {
    let dir = tempfile::tempdir().unwrap();
    let forge = Forge::start(cfg(dir.path())).await.unwrap();
    assert!(dir.path().join("artifacts").is_dir());
    assert!(dir.path().join("sources").is_dir());
    assert!(dir.path().join("db").join("build").is_dir(), "LMDB ouvert");
    assert!(dir.path().join("forge-signing.key").is_file());
    // Ed25519 : 32 octets de clé publique, en hexadécimal.
    let public = forge.keys.public_hex();
    assert_eq!(public.len(), 64);
    assert!(public.chars().all(|c| c.is_ascii_hexdigit()));
    assert!(
        forge.dummy_hash.starts_with("$argon2"),
        "hachage factice prêt"
    );
    assert_eq!(
        forge.artifact_path(&Uuid::nil()),
        dir.path()
            .join("artifacts")
            .join(format!("{}.wasm", Uuid::nil()))
    );
}

#[tokio::test]
async fn start_refuses_a_data_dir_that_is_a_file() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("pas-un-dossier");
    std::fs::write(&file, "x").unwrap();
    assert!(Forge::start(cfg(&file)).await.is_err());
}

#[tokio::test]
async fn an_enqueued_build_is_written_then_failed_with_the_runner_message() {
    let dir = tempfile::tempdir().unwrap();
    let forge = Forge::start(cfg(dir.path())).await.unwrap();
    let b = forge.store.builds.create(build("queued")).await.unwrap();

    forge.enqueue(b.id, &files()).await.unwrap();
    // Le worker peut déjà avoir consommé le fichier ; s'il est encore là, c'est le nôtre.
    if let Ok(bytes) = std::fs::read(source_file(dir.path(), &b.id)) {
        let written: Vec<SourceFile> = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(written[0].path, "src/lib.rs");
    }

    let done = settled(&forge, b.id).await;
    assert_eq!(done.status, "failed");
    let d = diagnostics(&done);
    assert_eq!(d.len(), 1);
    assert!(
        d[0].message
            .starts_with("la forge n'a pas pu lancer le build : "),
        "{}",
        d[0].message
    );
    assert!(done.started_at.is_some(), "est passé par `building`");
    assert!(done.finished_at.is_some());
    assert_eq!(
        done.toolchain, "rustc unknown",
        "pas de docker, pas de version"
    );
    assert!(done.signature.is_none() && done.wasm_sha256.is_none());
    assert!(
        !source_file(dir.path(), &b.id).exists(),
        "les sources sont effacées après le build"
    );
    assert!(!forge.artifact_path(&b.id).exists());
}

#[tokio::test]
async fn enqueue_writes_the_sources_before_queueing() {
    // Le fichier doit exister même pour un id que le worker ignore (il ne l'efface pas).
    let dir = tempfile::tempdir().unwrap();
    let forge = Forge::start(cfg(dir.path())).await.unwrap();
    let unknown = Uuid::new_v4();
    forge.enqueue(unknown, &files()).await.unwrap();
    drain(&forge).await;
    let written: Vec<SourceFile> =
        serde_json::from_slice(&std::fs::read(source_file(dir.path(), &unknown)).unwrap()).unwrap();
    assert_eq!(
        (written[0].path.as_str(), written[0].content.as_str()),
        ("src/lib.rs", "pub fn f() {}")
    );
    assert!(forge.store.builds.get(&unknown).await.unwrap().is_none());
}

#[tokio::test]
async fn run_one_leaves_unknown_and_already_finished_builds_alone() {
    let dir = tempfile::tempdir().unwrap();
    let forge = Forge::start(cfg(dir.path())).await.unwrap();
    let mut signed = build("signed");
    signed.signature = Some("sig".into());
    signed.wasm_sha256 = Some("w".into());
    let signed = forge.store.builds.create(signed).await.unwrap();

    forge.enqueue(Uuid::new_v4(), &files()).await.unwrap();
    forge.enqueue(signed.id, &files()).await.unwrap();
    drain(&forge).await;

    let after = forge.store.builds.get(&signed.id).await.unwrap().unwrap();
    assert_eq!(after.status, "signed");
    assert_eq!(after.signature.as_deref(), Some("sig"));
    assert_eq!(after.wasm_sha256.as_deref(), Some("w"));
    assert!(after.started_at.is_none(), "jamais repassé en `building`");
    assert_eq!(after.diagnostics_json, "[]");
    assert_eq!(after.updated_at, signed.updated_at);
    assert_eq!(
        forge.store.builds.list().await.unwrap().len(),
        2,
        "rien créé"
    );
}

#[tokio::test]
async fn unreadable_sources_fail_the_build_readably() {
    // `sources/<id>.json` est un DOSSIER : il existe (la reprise le remet en file) mais
    // ne se lit pas — le worker doit dire « sources introuvables ».
    let dir = tempfile::tempdir().unwrap();
    let id = {
        let store = Store::lmdb(dir.path()).unwrap();
        store.builds.create(build("queued")).await.unwrap().id
    };
    std::fs::create_dir_all(source_file(dir.path(), &id)).unwrap();

    let forge = Forge::start(cfg(dir.path())).await.unwrap();
    let done = settled(&forge, id).await;
    assert_eq!(done.status, "failed");
    assert_eq!(diagnostics(&done)[0].message, "sources introuvables");
    assert!(done.started_at.is_none(), "n'a jamais commencé");
}

#[tokio::test]
async fn corrupt_sources_become_an_internal_error_not_a_stuck_build() {
    // Un fichier source qui n'est pas du JSON : `run_one` rend une erreur, le worker la
    // transforme en échec lisible au lieu de laisser le build `queued` pour toujours.
    let dir = tempfile::tempdir().unwrap();
    let id = {
        let store = Store::lmdb(dir.path()).unwrap();
        store.builds.create(build("queued")).await.unwrap().id
    };
    std::fs::create_dir_all(dir.path().join("sources")).unwrap();
    std::fs::write(source_file(dir.path(), &id), b"pas du json").unwrap();

    let forge = Forge::start(cfg(dir.path())).await.unwrap();
    let done = settled(&forge, id).await;
    assert_eq!(done.status, "failed");
    let msg = &diagnostics(&done)[0].message;
    assert!(msg.starts_with("erreur interne de la forge : "), "{msg}");
}

#[tokio::test]
async fn recover_fails_interrupted_builds_and_requeues_those_with_sources() {
    let dir = tempfile::tempdir().unwrap();
    let (building, with_src, without_src, signed) = {
        let store = Store::lmdb(dir.path()).unwrap();
        let mut s = build("signed");
        s.signature = Some("sig".into());
        (
            store.builds.create(build("building")).await.unwrap(),
            store.builds.create(build("queued")).await.unwrap(),
            store.builds.create(build("queued")).await.unwrap(),
            store.builds.create(s).await.unwrap(),
        )
    };
    std::fs::create_dir_all(dir.path().join("sources")).unwrap();
    std::fs::write(
        source_file(dir.path(), &with_src.id),
        serde_json::to_vec(&files()).unwrap(),
    )
    .unwrap();

    let forge = Forge::start(cfg(dir.path())).await.unwrap();

    // Décidés par `recover` AVANT que `start` rende la main : aucune attente.
    let b = forge.store.builds.get(&building.id).await.unwrap().unwrap();
    assert_eq!(b.status, "failed");
    assert!(b.finished_at.is_some());
    assert_eq!(
        diagnostics(&b)[0].message,
        "build interrompu par un redémarrage de la forge"
    );
    let b = forge
        .store
        .builds
        .get(&without_src.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(b.status, "failed");
    assert!(b.finished_at.is_some());
    assert_eq!(
        diagnostics(&b)[0].message,
        "sources perdues pendant un redémarrage"
    );
    let b = forge.store.builds.get(&signed.id).await.unwrap().unwrap();
    assert_eq!(
        (b.status.as_str(), b.signature.as_deref()),
        ("signed", Some("sig"))
    );

    // Celui qui avait ses sources est remis en file : le worker le traite (et échoue
    // avec le message du runner, pas avec celui de la reprise).
    let b = settled(&forge, with_src.id).await;
    assert_eq!(b.status, "failed");
    assert!(b.started_at.is_some(), "réellement relancé");
    assert!(
        diagnostics(&b)[0]
            .message
            .starts_with("la forge n'a pas pu lancer le build : ")
    );
}

#[tokio::test]
async fn invitations_are_returned_in_clear_and_stored_hashed() {
    let dir = tempfile::tempdir().unwrap();
    let forge = Forge::start(cfg(dir.path())).await.unwrap();
    let a = forge
        .create_invitation(3, Some("bêta".into()))
        .await
        .unwrap();
    let b = forge.create_invitation(1, None).await.unwrap();

    for code in [&a, &b] {
        let tail = code.strip_prefix("forge-").expect("préfixe forge-");
        // 12 octets aléatoires en base64url sans remplissage : 16 caractères.
        assert_eq!(tail.len(), 16, "{code}");
        assert!(
            tail.chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        );
    }
    assert_ne!(a, b);

    let stored = forge.store.invitations.list().await.unwrap();
    assert_eq!(stored.len(), 2);
    let ia = stored
        .iter()
        .find(|i| i.name == sha256_hex(a.as_bytes()))
        .expect("le sha256 du code est stocké");
    assert_eq!(
        (ia.uses_left, ia.note.as_deref(), ia.status.as_str()),
        (3, Some("bêta"), "active")
    );
    let ib = stored
        .iter()
        .find(|i| i.name == sha256_hex(b.as_bytes()))
        .unwrap();
    assert_eq!((ib.uses_left, ib.note.as_deref()), (1, None));
    for i in &stored {
        assert!(
            i.name != a && i.name != b,
            "le code en clair n'est jamais stocké"
        );
    }
}

#[test]
fn canonical_is_the_signed_json_form() {
    let v = serde_json::json!({"b": 1, "a": [true, null]});
    assert_eq!(
        this_forge::service::canonical(&v),
        r#"{"a":[true,null],"b":1}"#
    );
}
