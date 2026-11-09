//! Le stockage de la forge : mémoire, LMDB (persistant d'un démarrage à l'autre), et le choix
//! par la configuration.

use this_forge::entities::{Account, Build, Invitation, Session};
use this_forge::store::Store;
use this_forge::{ForgeConfig, StoreKind};

fn account(email: &str) -> Account {
    Account::new(email.into(), "active".into(), "phc".into())
}

async fn fill(store: &Store) -> uuid::Uuid {
    let a = store.accounts.create(account("a@b.co")).await.unwrap();
    store
        .invitations
        .create(Invitation::new(
            "h".into(),
            "active".into(),
            3,
            Some("note".into()),
        ))
        .await
        .unwrap();
    store
        .sessions
        .create(Session::new(
            "r".into(),
            "active".into(),
            a.id.to_string(),
            42,
        ))
        .await
        .unwrap();
    store
        .builds
        .create(Build::new(
            "demo".into(),
            "queued".into(),
            a.id.to_string(),
            "1.0.0".into(),
            String::new(),
            "sha".into(),
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
    a.id
}

async fn check(store: &Store, account_id: uuid::Uuid) {
    let a = store.accounts.get(&account_id).await.unwrap().unwrap();
    assert_eq!(
        (a.name.as_str(), a.password_hash.as_str()),
        ("a@b.co", "phc")
    );
    let inv = store.invitations.list().await.unwrap();
    assert_eq!(inv.len(), 1);
    assert_eq!(
        (inv[0].uses_left, inv[0].note.as_deref()),
        (3, Some("note"))
    );
    let s = store.sessions.list().await.unwrap();
    assert_eq!((s.len(), s[0].expires_at), (1, 42));
    let b = store.builds.list().await.unwrap();
    assert_eq!(
        (b.len(), b[0].name.as_str(), b[0].status.as_str()),
        (1, "demo", "queued")
    );
}

#[tokio::test]
async fn the_memory_store_keeps_each_entity_in_its_own_service() {
    let store = Store::memory();
    let id = fill(&store).await;
    check(&store, id).await;
    // Un clone partage les mêmes données.
    check(&store.clone(), id).await;
}

#[tokio::test]
async fn the_lmdb_store_creates_one_environment_per_entity_and_survives_a_restart() {
    let dir = tempfile::tempdir().unwrap();
    let id = {
        let store = Store::lmdb(dir.path()).unwrap();
        fill(&store).await
    };
    for n in ["account", "invitation", "session", "build"] {
        assert!(dir.path().join("db").join(n).is_dir(), "{n}");
    }
    let reopened = Store::lmdb(dir.path()).unwrap();
    check(&reopened, id).await;
}

#[tokio::test]
async fn lmdb_refuses_a_data_dir_it_cannot_create() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("occupé");
    std::fs::write(&file, "pas un dossier").unwrap();
    assert!(Store::lmdb(&file).is_err());
}

#[tokio::test]
async fn from_config_picks_the_store_kind() {
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = ForgeConfig::new(dir.path());

    cfg.store = StoreKind::Memory;
    let mem = Store::from_config(&cfg).unwrap();
    mem.accounts.create(account("m@b.co")).await.unwrap();
    assert!(!dir.path().join("db").exists(), "la mémoire n'écrit rien");

    cfg.store = StoreKind::Lmdb;
    let disk = Store::from_config(&cfg).unwrap();
    assert!(dir.path().join("db").join("account").is_dir());
    assert!(disk.accounts.list().await.unwrap().is_empty());
}
