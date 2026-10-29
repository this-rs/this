//! Tests de **sécurité** — bail d'écriture et journal.
//!
//! Le bail et le journal sont les deux pièces qui protègent la **donnée**. Ce
//! fichier vérifie ce qu'elles garantissent réellement, et — tout aussi
//! important — ce qu'elles **ne** garantissent pas : les tests nommés
//! `documented_limit_*` affirment une possibilité d'attaque, pour que la
//! frontière de confiance soit explicite plutôt que supposée.
//!
//! La distinction est celle-ci : le bail protège contre un **écrivain périmé
//! coopératif** (un ancien processus resté vivant), pas contre un **attaquant
//! qui écrit dans le store**. Le journal donne un **ordre** et une
//! **idempotence**, pas une **preuve d'intégrité**.

#![cfg(feature = "obrain")]

use std::path::PathBuf;

use this::events::log_obrain::{JournalEntry, ObrainJournalLog};
use this::storage::ObrainStore;
use this::storage::writer_lease::{
    FileAuthority, LeaseError, OwnerId, ShardId, StoreId, WriterLease,
};

fn dir_shard(dir: &std::path::Path) -> ShardId {
    ShardId {
        store: StoreId::Dir(dir.to_path_buf()),
        space: None,
    }
}

fn entry(payload: &str) -> JournalEntry {
    JournalEntry {
        seq: 0,
        kind: "event".to_string(),
        at_ms: 0,
        payload: serde_json::json!({ "v": payload }),
    }
}

// ===========================================================================
// 1. LE BAIL : ce qu'il empêche vraiment
// ===========================================================================

/// Le bail refusé au **second écrivain**, y compris dans le même processus :
/// c'est le trou d'obrain que this-rs ferme.
#[test]
fn a_second_writer_is_refused_in_the_same_process() {
    let dir = tempfile::tempdir().expect("répertoire");
    let authority = FileAuthority::new(dir.path());
    let shard = dir_shard(dir.path());

    let first = authority
        .acquire(&shard, &OwnerId::this_process())
        .expect("premier écrivain");

    match authority.acquire(&shard, &OwnerId::this_process()) {
        Err(LeaseError::LockRefused { .. }) => {}
        other => panic!(
            "le second écrivain doit être REFUSÉ, obtenu {:?}",
            other.map(|h| h.epoch())
        ),
    }
    first.assert_live().expect("le détenteur garde son bail");
}

/// Un écrivain **périmé** ne peut plus écrire, mais peut encore lire : le refus
/// est ciblé, pas total.
#[tokio::test]
async fn a_stale_writer_cannot_write_but_can_still_read() {
    use this::core::entity::{Data, Entity};
    use this::core::service::DataService;

    // Un type minimal, comme l'écrirait une application.
    #[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
    struct Doc {
        id: uuid::Uuid,
        name: String,
        status: String,
        created_at: chrono::DateTime<chrono::Utc>,
        updated_at: chrono::DateTime<chrono::Utc>,
    }

    impl Entity for Doc {
        type Service = ();
        fn resource_name() -> &'static str {
            "docs"
        }
        fn resource_name_singular() -> &'static str {
            "doc"
        }
        fn service_from_host(
            _: &std::sync::Arc<dyn std::any::Any + Send + Sync>,
        ) -> anyhow::Result<std::sync::Arc<()>> {
            Ok(std::sync::Arc::new(()))
        }
        fn id(&self) -> uuid::Uuid {
            self.id
        }
        fn entity_type(&self) -> &str {
            "doc"
        }
        fn created_at(&self) -> chrono::DateTime<chrono::Utc> {
            self.created_at
        }
        fn updated_at(&self) -> chrono::DateTime<chrono::Utc> {
            self.updated_at
        }
        fn deleted_at(&self) -> Option<chrono::DateTime<chrono::Utc>> {
            None
        }
        fn status(&self) -> &str {
            &self.status
        }
    }
    impl Data for Doc {
        fn name(&self) -> &str {
            &self.name
        }
        fn indexed_fields() -> &'static [&'static str] {
            &["name"]
        }
        fn field_value(&self, field: &str) -> Option<this::core::field::FieldValue> {
            match field {
                "name" => Some(this::core::field::FieldValue::String(self.name.clone())),
                _ => None,
            }
        }
    }

    let dir = tempfile::tempdir().expect("répertoire");
    let store = ObrainStore::open(dir.path()).expect("store");
    let authority = FileAuthority::new(dir.path());
    store
        .enable_write_fencing_with(&authority)
        .expect("bail armé");

    let service = this::storage::ObrainDataService::<Doc>::new(std::sync::Arc::clone(&store));
    let now = chrono::Utc::now();
    let doc = Doc {
        id: uuid::Uuid::new_v4(),
        name: "avant".into(),
        status: "active".into(),
        created_at: now,
        updated_at: now,
    };
    let created = service.create(doc).await.expect("écriture autorisée");

    // Le writer perd son bail (un autre a repris le shard).
    store.fence_write_lease_for_test(2);

    let again = Doc {
        id: uuid::Uuid::new_v4(),
        name: "après".into(),
        status: "active".into(),
        created_at: now,
        updated_at: now,
    };
    assert!(
        service.create(again).await.is_err(),
        "un écrivain périmé ne doit PLUS écrire"
    );
    assert!(
        service.get(&created.id).await.expect("lecture").is_some(),
        "…mais la lecture reste servie : le refus est ciblé"
    );
}

/// **Limite documentée** : le bail est un verrou, **pas une signature**. Qui peut
/// écrire dans le fichier de bail peut **baisser l'époque** — et l'époque est
/// censée être le jeton de fencing. Le test l'affirme pour que la frontière soit
/// connue : la protection réelle est la maîtrise du répertoire du store.
#[test]
fn documented_limit_a_lowered_epoch_in_the_lease_file_is_accepted() {
    use this::storage::writer_lease::LeaseRecord;

    let dir = tempfile::tempdir().expect("répertoire");
    let authority = FileAuthority::new(dir.path());
    let shard = dir_shard(dir.path());

    let first = authority
        .acquire(&shard, &OwnerId::this_process())
        .expect("premier bail");
    assert_eq!(first.epoch(), 1);
    let path = first.path().expect("fichier de bail").to_path_buf();
    first.release();

    // On fabrique une trame d'époque 0 : aucune signature ne s'y oppose.
    let forged = WriterLease {
        shard: shard.clone(),
        owner: OwnerId::this_process(),
        epoch: 0,
        issued_at_ms: this::storage::writer_lease::now_ms(),
        expires_at_ms: this::storage::writer_lease::now_ms() + 60_000,
        heartbeat: 1,
    };
    std::fs::write(&path, LeaseRecord::encode(&forged)).expect("trame forgée");

    let after = authority
        .acquire(&shard, &OwnerId::this_process())
        .expect("acquisition après forçage");
    assert_eq!(
        after.epoch(),
        1,
        "l'époque est dérivée du fichier : une trame forgée à 0 redonne 1 — le \
         fencing n'authentifie pas son jeton, il faut donc maîtriser le répertoire"
    );
}

/// **Limite documentée** : le CRC détecte la **corruption**, pas la
/// **falsification**. Un attaquant qui recalcule le CRC produit une trame valide
/// pour un autre propriétaire.
#[test]
fn documented_limit_a_recomputed_crc_makes_a_forged_frame_valid() {
    use this::storage::writer_lease::LeaseRecord;

    let dir = tempfile::tempdir().expect("répertoire");
    let authority = FileAuthority::new(dir.path());
    let shard = dir_shard(dir.path());
    let handle = authority.acquire(&shard, &OwnerId::this_process()).unwrap();
    let path = handle.path().unwrap().to_path_buf();
    handle.release();

    // On change le propriétaire ET l'époque… puis on laisse `encode` recalculer
    // le CRC : la trame est parfaitement valide.
    let mut forged = WriterLease {
        shard: shard.clone(),
        owner: OwnerId::this_process(),
        epoch: 42,
        issued_at_ms: 0,
        expires_at_ms: i64::MAX,
        heartbeat: 0,
    };
    forged.owner.instance = uuid::Uuid::from_u128(0xdead_beef);
    forged.owner.host = "ailleurs".to_string();
    std::fs::write(&path, LeaseRecord::encode(&forged)).unwrap();

    let read = LeaseRecord::decode(
        &std::fs::read(&path)
            .unwrap()
            .try_into()
            .expect("128 octets"),
        shard,
    )
    .expect("une trame forgée mais cohérente est acceptée");
    assert_eq!(read.epoch, 42);
    assert_eq!(read.owner.host, "ailleurs");
}

/// En revanche, une trame **corrompue** (CRC faux) est **refusée**, et
/// l'acquisition ne repart pas de l'époque 1 en silence : repartir de zéro
/// lèverait un fencing en cours.
#[test]
fn a_corrupt_frame_is_refused_and_never_restarts_the_epoch() {
    let dir = tempfile::tempdir().expect("répertoire");
    let authority = FileAuthority::new(dir.path());
    let shard = dir_shard(dir.path());
    let handle = authority.acquire(&shard, &OwnerId::this_process()).unwrap();
    let path = handle.path().unwrap().to_path_buf();
    handle.release();

    let mut bytes = std::fs::read(&path).unwrap();
    bytes[40] ^= 0xff; // un octet abîmé
    std::fs::write(&path, &bytes).unwrap();

    match authority.acquire(&shard, &OwnerId::this_process()) {
        Err(LeaseError::Unavailable(detail)) => assert!(
            detail.contains("corromp") || detail.contains("CRC"),
            "le refus doit nommer la corruption : {detail}"
        ),
        other => panic!(
            "une trame corrompue doit être REFUSÉE, obtenu {:?}",
            other.map(|h| h.epoch())
        ),
    }
}

// ===========================================================================
// 2. LE JOURNAL : ce qu'il garantit, et ce qu'il ne garantit pas
// ===========================================================================

/// L'ordre **logique** est garanti même si le moteur rend l'ordre physique : un
/// journal répliqué ne se rejoue pas dans le désordre.
#[test]
fn the_journal_returns_logical_order_even_from_a_scrambled_zone() {
    let store = ObrainStore::new_in_memory();
    let journal = ObrainJournalLog::open(std::sync::Arc::clone(&store), "sec-chan").unwrap();

    let mut batch: Vec<JournalEntry> = (0..8).map(|i| entry(&i.to_string())).collect();
    journal.append_entries(&mut batch).unwrap();

    // On réécrit la zone à l'envers (append concurrent, resynchronisation…).
    let substrate = store.db().substrate_handle().expect("substrat");
    let mut lines: Vec<String> = journal
        .read_all()
        .unwrap()
        .iter()
        .map(|e| e.to_line())
        .collect();
    lines.reverse();
    obrain_engine::journal_zone::write_session_journal(&substrate, "sec-chan", &lines).unwrap();

    let seqs: Vec<u64> = journal
        .read_from(0, 1024)
        .unwrap()
        .entries
        .iter()
        .map(|e| e.seq)
        .collect();
    assert_eq!(seqs, (1..=8).collect::<Vec<u64>>(), "ordre logique rétabli");
}

/// **Limite documentée** : le journal n'est pas une chaîne de hachage. Retirer
/// une entrée laisse un **trou visible** (`seq` manquant) — pas une preuve, et
/// pas une reconstruction. Un consommateur qui exige la contiguïté doit la
/// vérifier lui-même ; l'adaptateur, lui, ne **comble jamais** un trou.
#[test]
fn documented_limit_the_journal_is_not_tamper_evident_but_never_hides_a_gap() {
    let store = ObrainStore::new_in_memory();
    let journal = ObrainJournalLog::open(std::sync::Arc::clone(&store), "gap-chan").unwrap();

    let mut batch: Vec<JournalEntry> = (0..6).map(|i| entry(&i.to_string())).collect();
    journal.append_entries(&mut batch).unwrap();

    // On supprime l'entrée 3 de la zone, comme le ferait une falsification.
    let substrate = store.db().substrate_handle().expect("substrat");
    let kept: Vec<String> = journal
        .read_all()
        .unwrap()
        .into_iter()
        .filter(|e| e.seq != 3)
        .map(|e| e.to_line())
        .collect();
    obrain_engine::journal_zone::write_session_journal(&substrate, "gap-chan", &kept).unwrap();

    let seqs: Vec<u64> = journal
        .read_from(0, 1024)
        .unwrap()
        .entries
        .iter()
        .map(|e| e.seq)
        .collect();
    assert_eq!(seqs, vec![1, 2, 4, 5, 6], "le trou reste VISIBLE");
    assert!(
        !seqs.contains(&3),
        "l'adaptateur ne réinvente jamais une entrée manquante — mais il ne peut \
         pas prouver qu'elle a existé : il n'y a pas de chaîne de hachage"
    );
    assert_eq!(
        journal.last_seq().unwrap(),
        Some(6),
        "la tête reste celle de la dernière entrée présente"
    );
}

/// Une ligne **inordonnançable** est refusée : on ne réplique pas du désordre.
#[test]
fn a_line_without_a_sequence_is_refused_by_the_reader() {
    assert!(JournalEntry::from_line(r#"{"kind":"event"}"#).is_none());
    assert!(JournalEntry::from_line("pas du json").is_none());
    assert!(JournalEntry::from_line(r#"{"seq":"douze"}"#).is_none());
    // Et une charge utile hostile ne casse pas la lecture.
    let hostile = JournalEntry::from_line(
        r#"{"seq":1,"kind":"event","at_ms":0,"payload":{"x":"\u0000\u001f</script>"}}"#,
    )
    .expect("une charge utile hostile reste une entrée");
    assert_eq!(hostile.seq, 1);
}

/// Le rejeu **idempotent** : répliquer deux fois le même lot ne duplique rien —
/// c'est ce qui rend une reprise après incident sûre.
#[test]
fn replaying_a_replication_batch_never_duplicates_entries() {
    let source = ObrainStore::new_in_memory();
    let src = ObrainJournalLog::open(std::sync::Arc::clone(&source), "src").unwrap();
    let mut batch: Vec<JournalEntry> = (0..5).map(|i| entry(&i.to_string())).collect();
    src.append_entries(&mut batch).unwrap();

    let replica = ObrainStore::new_in_memory();
    let dst = ObrainJournalLog::open(std::sync::Arc::clone(&replica), "dst").unwrap();

    // Trois fois le même lot, comme un réplicateur qui redémarre.
    for _ in 0..3 {
        let mut carried = src.read_all().unwrap();
        dst.append_entries(&mut carried).unwrap();
    }
    assert_eq!(
        dst.len().unwrap(),
        5,
        "le réplica porte les positions de la source, et rien de plus"
    );
    assert_eq!(dst.read_all().unwrap(), src.read_all().unwrap());
}

/// Un **canal** hostile (nom de zone forgé) est refusé : le nom devient un nom
/// de zone du substrat, il ne doit pas pouvoir sortir du répertoire.
#[test]
fn a_channel_name_cannot_escape_its_zone() {
    let store = ObrainStore::new_in_memory();
    for channel in ["../evasion", "a/b", "a\u{0}b", ""] {
        assert!(
            ObrainJournalLog::open(std::sync::Arc::clone(&store), channel).is_err(),
            "le canal {channel:?} doit être refusé"
        );
    }
    // Et les noms légitimes passent.
    let _ = ObrainJournalLog::open(std::sync::Arc::clone(&store), "chan-ok.1").unwrap();
}

/// Vérification finale : le chemin du fichier de bail ne permet pas de sortir du
/// répertoire du store, même avec une clé de shard hostile.
#[test]
fn the_lease_path_never_escapes_the_store_directory() {
    let dir = tempfile::tempdir().expect("répertoire");
    let authority = FileAuthority::new(dir.path());
    let hostile = ShardId {
        store: StoreId::Dir(PathBuf::from("/tmp/../etc")),
        space: Some("../../etc/passwd".to_string()),
    };
    let path = authority.lease_path(&hostile);
    assert_eq!(
        path.parent(),
        Some(dir.path()),
        "le fichier de bail vit TOUJOURS dans le répertoire donné : la clé de shard \
         est hachée, elle ne peut pas composer un chemin"
    );
    assert!(
        path.file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.len() == 22 && n.ends_with(".lease")),
        "le nom est une clé hachée de 16 hex + `.lease` : {:?}",
        path.file_name()
    );
}
