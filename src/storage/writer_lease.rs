//! Bail d'écriture et **jeton de fencing**.
//!
//! # Pourquoi ce module existe
//!
//! Obrain garantit un seul écrivain par store **par convention d'appelant**
//! (`OBRAIN_WRITE=1`), pas par le verrou : son RFC (`docs/rfc/substrate/multi-writer.md`)
//! l'écrit noir sur blanc — deux `open_shared_write` sur le même store *« ne se
//! refusent pas — ils corrompraient le WAL »*. Et le trou n'est pas seulement
//! inter-processus : dans **un même processus**, deux `ObrainDB::open()` du même
//! répertoire réussissent tous les deux (`obrain-substrate/src/file.rs:179-207` :
//! si une part de verrou existe déjà, elle est *retournée*).
//!
//! this-rs doit donc **imposer** l'unicité. Deux pièces, et deux seulement :
//!
//! 1. **l'exclusion** — un verrou OS sur un fichier possédé par this-rs ;
//! 2. **le fencing** — une époque persistée **hors de l'état rejoué**, comparée
//!    avant toute écriture, de sorte qu'un écrivain périmé soit *refusé* et non
//!    *silencieusement accepté*.
//!
//! # Le constat qui commande la conception
//!
//! **`commit_wal` ne peut pas servir de point de contrôle.** Côté obrain, le WAL
//! est écrit **avant** la mutation mmap (`obrain-substrate/src/writer.rs:287-309` :
//! « (1) WAL first » puis « (2) mmap mutation »), et il n'existe **aucun marqueur
//! de commit annulable** (les records de mutation partent avec `flags: 0` ;
//! `FLAG_COMMIT` ne sert qu'à déclencher le fsync, posé par le record de
//! checkpoint). Vérifier au commit laisserait donc le record du writer périmé
//! **déjà dans le WAL**.
//! ⇒ **Le verrou doit être pris en ENTRÉE d'opération** ; la vérification au
//! commit n'est qu'un filet.
//!
//! # Pourquoi l'époque n'est pas dans le substrat
//!
//! | Emplacement | Verdict |
//! |---|---|
//! | en-tête `substrate.meta` | gelé (96 o, `SUBSTRATE_FORMAT_VERSION = 1`), et un second `SubstrateFile` se heurte au `flock` |
//! | colonne blob | non exposée : `SubstrateStore` n'a aucune méthode publique `*blob*` |
//! | propriété de nœud | **elle vit dans l'état rejoué** — un rejeu, un checkpoint ou une restauration ferait **reculer l'époque**, ce qui casse le jeton |
//! | **fichier possédé par this-rs** | **retenu** — le répertoire du store tolère des entrées étrangères (tous les `read_dir` d'obrain filtrent par préfixe, et `<store>/graphs/` est déjà un sous-répertoire) |
//!
//! # Le format est gelé
//!
//! La trame fait **128 octets**, commence par `THISLSE1`, porte une version et un
//! **CRC32**, et l'époque est **dupliquée en clair** à offset fixe pour que la
//! réparation d'une trame corrompue reste possible — mais **explicitement**
//! ([`LeaseRecord::decode_lossy_epoch`]), jamais en silence.

use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Identités
// ---------------------------------------------------------------------------

/// De quel store il s'agit.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum StoreId {
    /// Store persistant : chemin **canonicalisé** (deux chemins équivalents ne
    /// doivent pas produire deux shards).
    Dir(PathBuf),
    /// Store volatil (substrat temporaire d'un `new_in_memory()`) : pas de
    /// fichier de bail possible, on identifie par instance.
    Volatile(u64),
}

/// L'unité d'écriture : un store, **et un espace** (graphe nommé).
///
/// Les deux comptent : deux espaces d'un même store sont deux substrats qui
/// allouent chacun leurs identifiants à partir de 1.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ShardId {
    /// Le store.
    pub store: StoreId,
    /// L'espace (graphe nommé) — `None` = graphe par défaut.
    pub space: Option<String>,
}

impl ShardId {
    /// Identifiant textuel stable du shard (16 caractères hexadécimaux).
    ///
    /// FNV-1a 64 bits, **même famille que le `graph_scope()` d'obrain** : deux
    /// shards distincts ne doivent pas partager une clé, sans pour autant
    /// dépendre d'une horloge ou d'un ordre d'allocation.
    pub fn key(&self) -> String {
        let mut h = fnv1a64(b"this-rs/shard/v1\0");
        match &self.store {
            StoreId::Dir(path) => {
                h = fnv1a64_step(h, b"dir\0");
                h = fnv1a64_step(h, path.to_string_lossy().as_bytes());
            }
            StoreId::Volatile(id) => {
                h = fnv1a64_step(h, b"vol\0");
                h = fnv1a64_step(h, &id.to_le_bytes());
            }
        }
        h = fnv1a64_step(h, b"\0space\0");
        if let Some(space) = &self.space {
            h = fnv1a64_step(h, space.as_bytes());
        }
        format!("{h:016x}")
    }
}

/// Qui écrit.
///
/// L'identité est **stable pour la vie du processus** : le pid seul ne suffit
/// pas (il est réutilisable), d'où l'`Uuid` tiré au démarrage.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OwnerId {
    /// Identifiant d'instance, tiré une fois par processus.
    pub instance: Uuid,
    /// Le pid, pour les diagnostics humains.
    pub process: u32,
    /// Le nom d'hôte — tronqué à 32 octets dans la trame.
    pub host: String,
    /// Quand ce writer a démarré (ms depuis l'époque Unix).
    pub started_at_ms: i64,
}

impl OwnerId {
    /// Décrit ce processus.
    pub fn this_process() -> Self {
        Self {
            instance: Uuid::new_v4(),
            process: std::process::id(),
            host: hostname(),
            started_at_ms: now_ms(),
        }
    }
}

/// Le bail lui-même.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WriterLease {
    /// Le shard couvert.
    pub shard: ShardId,
    /// Le détenteur.
    pub owner: OwnerId,
    /// **L'époque** : strictement croissante à chaque acquisition. C'est le
    /// jeton de fencing — un écrivain qui présente une époque inférieure à
    /// celle du store est refusé.
    pub epoch: u64,
    /// Émission.
    pub issued_at_ms: i64,
    /// Expiration.
    pub expires_at_ms: i64,
    /// Compteur de battements (diagnostic de vivacité).
    pub heartbeat: u64,
}

impl WriterLease {
    /// Le bail est-il expiré, à une tolérance près ?
    ///
    /// La tolérance (`skew`) protège d'une horloge qui avance ou recule entre
    /// deux nœuds : **sans elle, un bail expiré d'une seconde serait repris
    /// alors que son détenteur écrit encore**.
    pub fn is_expired_at(&self, now_ms: i64, skew: Duration) -> bool {
        self.expires_at_ms + skew.as_millis() as i64 <= now_ms
    }

    /// Durée restante, si le bail court encore.
    pub fn remaining(&self, now_ms: i64) -> Option<Duration> {
        let left = self.expires_at_ms - now_ms;
        (left > 0).then(|| Duration::from_millis(left as u64))
    }
}

// ---------------------------------------------------------------------------
// Trame persistée — FORMAT GELÉ
// ---------------------------------------------------------------------------

/// Erreurs de lecture d'une trame de bail.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum LeaseError {
    /// La trame ne commence pas par la signature attendue.
    #[error("signature de bail invalide")]
    BadMagic,
    /// Version de trame inconnue — refusée plutôt que devinée.
    #[error("version de bail inconnue : {0}")]
    BadVersion(u8),
    /// Le CRC ne correspond pas : la trame est corrompue.
    #[error("CRC de bail invalide (trame corrompue)")]
    BadCrc,
    /// Le device ou le fichier n'est pas exploitable.
    #[error("bail indisponible : {0}")]
    Unavailable(String),
    /// Le système a refusé le verrou exclusif : un autre écrivain le détient
    /// (dans un autre processus **ou dans celui-ci**), ou l'E/S a échoué. Le
    /// détail est celui du système — on ne devine pas lequel des deux.
    #[error("verrou de bail refusé : {detail}")]
    LockRefused {
        /// Le message du système.
        detail: String,
    },
    /// Le détenteur n'est plus celui qui a le droit d'écrire.
    #[error("écriture refusée : ce writer est périmé (époque {mine} < {current})")]
    Fenced {
        /// Époque du writer qui a tenté d'écrire.
        mine: u64,
        /// Époque en vigueur sur le store.
        current: u64,
    },
}

/// Codec de la trame de bail.
///
/// # Disposition (128 octets, gelée)
///
/// ```text
///   0..8    MAGIC   b"THISLSE1"
///   8       version 1
///   9       flags = 0
///  10..12   réservé
///  12..20   epoch            (u64 LE)   ← dupliqué « en clair » pour la réparation
///  20..28   issued_at_ms     (i64 LE)
///  28..36   expires_at_ms    (i64 LE)
///  36..44   heartbeat        (u64 LE)
///  44..60   owner.instance   (16 octets)
///  60..64   owner.process    (u32 LE)
///  64..72   shard.key()      (u64 LE — la clé, pas le chemin)
///  72..104  owner.host       (32 octets UTF-8, complétés par des zéros)
/// 104..120  réservé (zéro)
/// 120..124  CRC32 sur 0..120 (u32 LE)
/// 124..128  réservé (zéro)
/// ```
///
/// Le chemin du store n'est **pas** dans la trame : il est déjà dans le nom du
/// fichier (`<store>/<shard_key>.lock`), et y mettre un chemin rendrait la trame
/// dépendante d'un système de fichiers.
pub struct LeaseRecord;

impl LeaseRecord {
    /// Signature de la trame.
    pub const MAGIC: [u8; 8] = *b"THISLSE1";
    /// Version du format.
    pub const VERSION: u8 = 1;
    /// Taille de la trame.
    pub const SIZE: usize = 128;
    /// Offset de l'époque, dupliquée pour la réparation explicite.
    pub const EPOCH_OFFSET: usize = 12;
    /// Nombre d'octets couverts par le CRC.
    pub const CRC_COVERAGE: usize = 120;
    /// Offset du CRC.
    pub const CRC_OFFSET: usize = 120;
    /// Longueur maximale du nom d'hôte conservé.
    pub const HOST_MAX: usize = 32;

    /// Sérialise un bail en trame de 128 octets.
    pub fn encode(lease: &WriterLease) -> [u8; Self::SIZE] {
        let mut b = [0u8; Self::SIZE];
        b[0..8].copy_from_slice(&Self::MAGIC);
        b[8] = Self::VERSION;
        b[9] = 0; // flags
        b[Self::EPOCH_OFFSET..Self::EPOCH_OFFSET + 8].copy_from_slice(&lease.epoch.to_le_bytes());
        b[20..28].copy_from_slice(&lease.issued_at_ms.to_le_bytes());
        b[28..36].copy_from_slice(&lease.expires_at_ms.to_le_bytes());
        b[36..44].copy_from_slice(&lease.heartbeat.to_le_bytes());
        b[44..60].copy_from_slice(lease.owner.instance.as_bytes());
        b[60..64].copy_from_slice(&lease.owner.process.to_le_bytes());
        // La clé du shard : on hache la clé textuelle pour rester stable même si
        // la représentation du store change de forme.
        let key = fnv1a64(lease.shard.key().as_bytes());
        b[64..72].copy_from_slice(&key.to_le_bytes());
        let host = lease.owner.host.as_bytes();
        let n = host.len().min(Self::HOST_MAX);
        b[72..72 + n].copy_from_slice(&host[..n]);
        let crc = crc32(&b[..Self::CRC_COVERAGE]);
        b[Self::CRC_OFFSET..Self::CRC_OFFSET + 4].copy_from_slice(&crc.to_le_bytes());
        b
    }

    /// Relit une trame, en **refusant** tout ce qui n'est pas conforme.
    ///
    /// Le store et l'espace ne sont pas dans la trame : l'appelant les fournit
    /// (il les connaît — c'est son store).
    pub fn decode(bytes: &[u8; Self::SIZE], shard: ShardId) -> Result<WriterLease, LeaseError> {
        if bytes[0..8] != Self::MAGIC {
            return Err(LeaseError::BadMagic);
        }
        // L'INTÉGRITÉ avant l'interprétation : une trame corrompue doit se
        // signaler comme corrompue, pas comme « version inconnue ». Sinon un
        // octet abîmé ferait croire à un format plus récent.
        let expected = u32::from_le_bytes(
            bytes[Self::CRC_OFFSET..Self::CRC_OFFSET + 4]
                .try_into()
                .map_err(|_| LeaseError::BadCrc)?,
        );
        if crc32(&bytes[..Self::CRC_COVERAGE]) != expected {
            return Err(LeaseError::BadCrc);
        }
        if bytes[8] != Self::VERSION {
            return Err(LeaseError::BadVersion(bytes[8]));
        }

        let mut host_end = 72 + Self::HOST_MAX;
        let host_bytes = &bytes[72..host_end];
        let zero = host_bytes
            .iter()
            .position(|b| *b == 0)
            .unwrap_or(host_bytes.len());
        host_end = 72 + zero;
        let host = String::from_utf8_lossy(&bytes[72..host_end]).to_string();

        Ok(WriterLease {
            shard,
            owner: OwnerId {
                instance: Uuid::from_slice(&bytes[44..60]).map_err(|_| LeaseError::BadMagic)?,
                process: u32::from_le_bytes(
                    bytes[60..64].try_into().map_err(|_| LeaseError::BadCrc)?,
                ),
                host,
                started_at_ms: 0, // non persisté : ce n'est pas un fait de fencing
            },
            epoch: u64::from_le_bytes(
                bytes[Self::EPOCH_OFFSET..Self::EPOCH_OFFSET + 8]
                    .try_into()
                    .map_err(|_| LeaseError::BadCrc)?,
            ),
            issued_at_ms: i64::from_le_bytes(
                bytes[20..28].try_into().map_err(|_| LeaseError::BadCrc)?,
            ),
            expires_at_ms: i64::from_le_bytes(
                bytes[28..36].try_into().map_err(|_| LeaseError::BadCrc)?,
            ),
            heartbeat: u64::from_le_bytes(
                bytes[36..44].try_into().map_err(|_| LeaseError::BadCrc)?,
            ),
        })
    }

    /// L'époque **sans valider le CRC** — réservé à la réparation explicite.
    ///
    /// C'est le seul chemin qui lit une trame potentiellement corrompue : il
    /// existe pour qu'un opérateur puisse récupérer une époque quand le CRC est
    /// perdu, **en le sachant**. Aucun chemin automatique ne l'appelle.
    pub fn decode_lossy_epoch(bytes: &[u8; Self::SIZE]) -> Option<u64> {
        if bytes[0..8] != Self::MAGIC {
            return None;
        }
        Some(u64::from_le_bytes(
            bytes[Self::EPOCH_OFFSET..Self::EPOCH_OFFSET + 8]
                .try_into()
                .ok()?,
        ))
    }
}

// ---------------------------------------------------------------------------
// Autorité de bail
// ---------------------------------------------------------------------------

/// Qui délivre les baux.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthorityKind {
    /// Aucun fencing : un seul écrivain par processus, aucun arbitrage.
    /// C'est le défaut mono-nœud — le comportement d'aujourd'hui.
    None,
    /// Verrou de fichier + époque dans un fichier possédé par this-rs.
    File,
    /// Bail délivré par un consensus (carte de shards + époques).
    Consensus,
}

/// L'état d'un bail en vigueur, tel qu'il se **publie**.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LeaseInfo {
    /// L'époque détenue — le jeton de fencing.
    pub epoch: u64,
    /// La clé du shard couvert (16 hex).
    pub shard: String,
    /// Qui a délivré le bail (`None` = autorité non renseignée).
    pub authority: Option<AuthorityKind>,
}

/// Le contrat d'une autorité de bail.
///
/// Volontairement **synchrone** : acquérir un bail est une opération locale
/// (verrou de fichier) ou déjà arbitrée en amont (consensus). Aucun bail ne doit
/// être demandé sur le chemin d'une requête de lecture.
pub trait LeaseAuthority: Send + Sync {
    /// Le type d'autorité, publié.
    fn kind(&self) -> AuthorityKind;
    /// Le bail en vigueur pour ce shard, s'il y en a un.
    fn current(&self, shard: &ShardId) -> Result<Option<WriterLease>, LeaseError>;
}

// ---------------------------------------------------------------------------
// Utilitaires
// ---------------------------------------------------------------------------

/// Millisecondes depuis l'époque Unix.
pub fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// FNV-1a 64 bits — déterministe, sans dépendance, stable entre exécutions.
fn fnv1a64(bytes: &[u8]) -> u64 {
    fnv1a64_step(0xcbf2_9ce4_8422_2325, bytes)
}

fn fnv1a64_step(mut h: u64, bytes: &[u8]) -> u64 {
    for b in bytes {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

/// CRC-32/ISO-HDLC (celui de zlib) — implémenté ici pour que le **format** du
/// bail ne dépende d'aucune crate : il est gelé, donc il doit être autonome.
fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = 0xffff_ffffu32;
    for b in bytes {
        crc ^= u32::from(*b);
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xedb8_8320 & mask);
        }
    }
    !crc
}

/// Nom d'hôte, sans dépendance : variable d'environnement, sinon `/etc/hostname`.
fn hostname() -> String {
    if let Ok(h) = std::env::var("HOSTNAME") {
        if !h.is_empty() {
            return h;
        }
    }
    std::fs::read_to_string("/etc/hostname")
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|_| "unknown".to_string())
}

// ---------------------------------------------------------------------------
// État vivant d'un détenteur
// ---------------------------------------------------------------------------

const STATE_LIVE: u8 = 0;
const STATE_FENCED: u8 = 1;
const STATE_RELEASED: u8 = 2;
const STATE_UNAVAILABLE: u8 = 3;

/// L'état partagé d'un détenteur : des atomes, pour que le chemin d'écriture ne
/// paie **qu'un load**.
#[derive(Debug)]
struct LeaseInner {
    state: std::sync::atomic::AtomicU8,
    /// Notre époque.
    mine: std::sync::atomic::AtomicU64,
    /// L'époque qui nous a périmés (`0` = aucune).
    fenced_by: std::sync::atomic::AtomicU64,
    expires_at_ms: std::sync::atomic::AtomicI64,
    heartbeat: std::sync::atomic::AtomicU64,
    /// Commits depuis le dernier contrôle disque.
    since_tripwire: std::sync::atomic::AtomicU64,
}

impl LeaseInner {
    fn new(epoch: u64, expires_at_ms: i64) -> Self {
        use std::sync::atomic::AtomicU64;
        Self {
            state: std::sync::atomic::AtomicU8::new(STATE_LIVE),
            mine: AtomicU64::new(epoch),
            fenced_by: AtomicU64::new(0),
            expires_at_ms: std::sync::atomic::AtomicI64::new(expires_at_ms),
            heartbeat: AtomicU64::new(1),
            since_tripwire: AtomicU64::new(0),
        }
    }

    fn fence(&self, by: u64) {
        use std::sync::atomic::Ordering;
        self.fenced_by.store(by, Ordering::Release);
        self.state.store(STATE_FENCED, Ordering::Release);
    }
}

/// Un détenteur vivant de bail.
///
/// Le verrou OS est tenu par ce handle : **le drop rend le bail**. C'est ce qui
/// fait qu'un crash libère l'exclusion sans intervention.
#[derive(Debug)]
pub struct WriterLeaseHandle {
    shard: ShardId,
    owner: OwnerId,
    inner: std::sync::Arc<LeaseInner>,
    ttl: Duration,
    skew: Duration,
    /// Le fichier de bail — le verrou vit tant qu'il est ouvert.
    /// `None` en mode volatil (aucun fichier possible).
    _file: Option<std::fs::File>,
    path: Option<PathBuf>,
    /// Au-delà de ce nombre de commits, on relit la trame (le « tripwire »).
    tripwire_every: u64,
}

impl WriterLeaseHandle {
    /// Le bail tel qu'il a été émis.
    pub fn lease(&self) -> WriterLease {
        use std::sync::atomic::Ordering;
        WriterLease {
            shard: self.shard.clone(),
            owner: self.owner.clone(),
            epoch: self.inner.mine.load(Ordering::Acquire),
            issued_at_ms: now_ms(),
            expires_at_ms: self.inner.expires_at_ms.load(Ordering::Acquire),
            heartbeat: self.inner.heartbeat.load(Ordering::Acquire),
        }
    }

    /// L'époque détenue — le jeton de fencing.
    pub fn epoch(&self) -> u64 {
        self.inner.mine.load(std::sync::atomic::Ordering::Acquire)
    }

    /// Le shard couvert.
    pub fn shard(&self) -> &ShardId {
        &self.shard
    }

    /// Le fichier de bail, s'il y en a un.
    pub fn path(&self) -> Option<&std::path::Path> {
        self.path.as_deref()
    }

    /// **Le contrôle du chemin chaud** : un seul load atomique.
    ///
    /// À appeler en **entrée** d'opération d'écriture, jamais seulement au
    /// commit : côté obrain le WAL est écrit *avant* la mutation, donc un
    /// contrôle tardif laisserait la trace d'un écrivain périmé sur le disque.
    pub fn assert_live(&self) -> Result<(), LeaseError> {
        use std::sync::atomic::Ordering;
        match self.inner.state.load(Ordering::Acquire) {
            STATE_LIVE => {
                // Le TTL est vérifié ici aussi : un bail expiré n'est plus un
                // droit d'écrire, même si personne n'a encore repris le verrou.
                let now = now_ms();
                let expires = self.inner.expires_at_ms.load(Ordering::Acquire);
                if expires + self.skew.as_millis() as i64 <= now {
                    self.inner.state.store(STATE_FENCED, Ordering::Release);
                    return Err(LeaseError::Fenced {
                        mine: self.epoch(),
                        current: self.epoch(),
                    });
                }
                Ok(())
            }
            STATE_FENCED => Err(LeaseError::Fenced {
                mine: self.epoch(),
                current: self.inner.fenced_by.load(Ordering::Acquire),
            }),
            STATE_UNAVAILABLE => Err(LeaseError::Unavailable(
                "bail indisponible (état interne)".to_string(),
            )),
            _ => Err(LeaseError::Unavailable(
                "bail déjà rendu (writer arrêté)".to_string(),
            )),
        }
    }

    /// **Le filet** : relit la trame sur le disque et compare.
    ///
    /// Une trame dont l'époque a **avancé** — ou dont le détenteur n'est plus
    /// nous — signifie qu'un autre écrivain a pris le relais : on se fige.
    /// Un appel sur `tripwire_every` commits, jamais à chaque écriture.
    pub fn tripwire(&self) -> Result<(), LeaseError> {
        use std::sync::atomic::Ordering;
        let Some(path) = &self.path else {
            // Mode volatil : rien à relire, l'exclusion est locale au processus.
            return self.assert_live();
        };
        let bytes = match read_frame(path)? {
            Some(b) => b,
            None => {
                self.inner.fence(0);
                return Err(LeaseError::Unavailable(
                    "le fichier de bail a disparu sous nos pieds".to_string(),
                ));
            }
        };
        let theirs = LeaseRecord::decode(&bytes, self.shard.clone())?;
        if theirs.owner.instance != self.owner.instance || theirs.epoch != self.epoch() {
            self.inner.fence(theirs.epoch);
            return Err(LeaseError::Fenced {
                mine: self.epoch(),
                current: theirs.epoch,
            });
        }
        self.inner.since_tripwire.store(0, Ordering::Release);
        Ok(())
    }

    /// Compte un commit et déclenche le contrôle disque au moment voulu.
    ///
    /// À appeler après chaque écriture réussie : c'est le seul endroit où le
    /// coût d'un `pread` est acceptable (tous les `tripwire_every` commits).
    pub fn on_commit(&self) -> Result<(), LeaseError> {
        use std::sync::atomic::Ordering;
        let n = self.inner.since_tripwire.fetch_add(1, Ordering::AcqRel) + 1;
        if n >= self.tripwire_every {
            self.tripwire()?;
        }
        self.assert_live()
    }

    /// Prolonge le bail et réécrit la trame.
    pub fn renew(&self) -> Result<WriterLease, LeaseError> {
        use std::sync::atomic::Ordering;
        self.assert_live()?;
        let expires = now_ms() + self.ttl.as_millis() as i64;
        self.inner.expires_at_ms.store(expires, Ordering::Release);
        let heartbeat = self.inner.heartbeat.fetch_add(1, Ordering::AcqRel) + 1;
        let lease = self.lease();
        if let Some(path) = &self.path {
            let bytes = LeaseRecord::encode(&lease);
            write_frame(path, &bytes)?;
        }
        tracing::debug!(epoch = lease.epoch, heartbeat, "bail renouvelé");
        Ok(lease)
    }

    /// Rend le bail (le verrou est libéré au drop).
    pub fn release(self) {
        use std::sync::atomic::Ordering;
        self.inner.state.store(STATE_RELEASED, Ordering::Release);
        drop(self);
    }

    /// **Couture de test** : simule la reprise du bail par un autre écrivain.
    #[doc(hidden)]
    pub fn fence_for_test(&self, by_epoch: u64) {
        self.inner.fence(by_epoch);
    }

    /// Réglage de test : fréquence du tripwire.
    #[doc(hidden)]
    pub fn with_tripwire_every(mut self, n: u64) -> Self {
        self.tripwire_every = n.max(1);
        self
    }
}

// ---------------------------------------------------------------------------
// Autorités
// ---------------------------------------------------------------------------

/// Bail adossé au **système de fichiers** : un verrou OS exclusif sur
/// `<store>/<shard_key>.lock`, et une époque persistée dans la trame.
///
/// # Pourquoi ce verrou ferme un trou réel
///
/// Obrain a un trou **intra-processus** : `obrain-substrate/src/file.rs:179-207`
/// rend la part de verrou existante, donc deux `ObrainDB::open()` du même
/// répertoire dans le même processus réussissent tous les deux. Un `flock` posé
/// sur un descripteur **distinct** (ce que fait ce module) les sépare, dans le
/// même processus comme entre processus.
#[derive(Debug, Clone)]
pub struct FileAuthority {
    /// Répertoire racine des baux (généralement le répertoire du store).
    pub root: PathBuf,
    /// Durée de vie d'un bail.
    pub ttl: Duration,
    /// Tolérance d'horloge.
    pub skew: Duration,
    /// Contrôle disque tous les N commits.
    pub tripwire_every: u64,
}

impl FileAuthority {
    /// Autorité de fichier sur un répertoire, avec des valeurs par défaut
    /// raisonnables (TTL 30 s, tolérance 5 s, tripwire tous les 256 commits).
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            ttl: Duration::from_secs(30),
            skew: Duration::from_secs(5),
            tripwire_every: 256,
        }
    }

    /// Le chemin du fichier de bail d'un shard.
    pub fn lease_path(&self, shard: &ShardId) -> PathBuf {
        self.root.join(format!("{}.lease", shard.key()))
    }

    /// Acquiert le bail d'un shard.
    ///
    /// **Refuse** si le verrou est déjà tenu — dans un autre processus comme
    /// dans celui-ci — et **refuse** aussi si la trame sur le disque est
    /// corrompue : repartir de zéro ferait *reculer* l'époque, ce qui lèverait
    /// un fencing en cours. La réparation est explicite
    /// ([`LeaseRecord::decode_lossy_epoch`]).
    pub fn acquire(
        &self,
        shard: &ShardId,
        owner: &OwnerId,
    ) -> Result<WriterLeaseHandle, LeaseError> {
        use std::fs::OpenOptions;

        if let StoreId::Volatile(_) = shard.store {
            return Err(LeaseError::Unavailable(
                "store volatil : pas de fichier de bail possible (utiliser MemoryAuthority)"
                    .to_string(),
            ));
        }
        std::fs::create_dir_all(&self.root)
            .map_err(|e| LeaseError::Unavailable(format!("répertoire de bail : {e}")))?;

        let path = self.lease_path(shard);
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&path)
            .map_err(|e| LeaseError::Unavailable(format!("ouverture du bail : {e}")))?;

        // L'EXCLUSION. Un échec ici n'est pas une erreur fatale : c'est le cas
        // normal d'un second écrivain. On ne distingue pas « tenu ailleurs »
        // d'une erreur d'E/S — on le DIT, avec le détail du système.
        if let Err(e) = file.try_lock() {
            return Err(LeaseError::LockRefused {
                detail: e.to_string(),
            });
        }

        // L'époque : strictement croissante. On lit la trame précédente.
        let previous = read_frame(&path)?;
        let epoch = match previous {
            Some(bytes) => {
                // À l'acquisition, la question de l'opérateur n'est pas « c'est
                // quoi ? » mais « que faire ? » : on joint le remède au
                // diagnostic, sans jamais repartir de l'époque 1 en silence.
                let prev = LeaseRecord::decode(&bytes, shard.clone()).map_err(|e| {
                    LeaseError::Unavailable(format!(
                        "trame de bail illisible ({e}) — réparation EXPLICITE requise \
                         (LeaseRecord::decode_lossy_epoch) ; repartir de l'époque 1 \
                         lèverait un fencing en cours"
                    ))
                })?;
                prev.epoch + 1
            }
            None => 1,
        };

        let expires_at_ms = now_ms() + self.ttl.as_millis() as i64;
        let lease = WriterLease {
            shard: shard.clone(),
            owner: owner.clone(),
            epoch,
            issued_at_ms: now_ms(),
            expires_at_ms,
            heartbeat: 1,
        };
        write_frame(&path, &LeaseRecord::encode(&lease))?;

        Ok(WriterLeaseHandle {
            shard: shard.clone(),
            owner: owner.clone(),
            inner: std::sync::Arc::new(LeaseInner::new(epoch, expires_at_ms)),
            ttl: self.ttl,
            skew: self.skew,
            _file: Some(file),
            path: Some(path),
            tripwire_every: self.tripwire_every.max(1),
        })
    }
}

impl LeaseAuthority for FileAuthority {
    fn kind(&self) -> AuthorityKind {
        AuthorityKind::File
    }

    fn current(&self, shard: &ShardId) -> Result<Option<WriterLease>, LeaseError> {
        match read_frame(&self.lease_path(shard))? {
            Some(bytes) => Ok(Some(LeaseRecord::decode(&bytes, shard.clone())?)),
            None => Ok(None),
        }
    }
}

/// Bail **en mémoire** : pour les stores volatils (`new_in_memory`) et les tests.
///
/// Aucun fencing réel n'est possible sans support persistant : l'époque est
/// monotone **dans ce processus**, et `kind()` vaut [`AuthorityKind::None`] —
/// publié tel quel, jamais présenté comme une garantie.
#[derive(Debug, Default)]
pub struct MemoryAuthority {
    epochs: std::sync::Mutex<std::collections::HashMap<String, u64>>,
    /// Durée de vie d'un bail.
    pub ttl: Duration,
    /// Tolérance d'horloge.
    pub skew: Duration,
}

impl MemoryAuthority {
    /// Autorité en mémoire (TTL 30 s, tolérance 5 s).
    pub fn new() -> Self {
        Self {
            epochs: std::sync::Mutex::new(std::collections::HashMap::new()),
            ttl: Duration::from_secs(30),
            skew: Duration::from_secs(5),
        }
    }

    /// Émet un bail volatil (époque monotone par shard, dans ce processus).
    pub fn acquire(
        &self,
        shard: &ShardId,
        owner: &OwnerId,
    ) -> Result<WriterLeaseHandle, LeaseError> {
        let key = shard.key();
        let epoch = {
            let mut epochs = self
                .epochs
                .lock()
                .map_err(|_| LeaseError::Unavailable("registre d'époques empoisonné".into()))?;
            let e = epochs.entry(key).or_insert(1);
            *e += 1;
            *e
        };
        let expires_at_ms = now_ms() + self.ttl.as_millis() as i64;
        Ok(WriterLeaseHandle {
            shard: shard.clone(),
            owner: owner.clone(),
            inner: std::sync::Arc::new(LeaseInner::new(epoch, expires_at_ms)),
            ttl: self.ttl,
            skew: self.skew,
            _file: None,
            path: None,
            tripwire_every: 256,
        })
    }
}

impl LeaseAuthority for MemoryAuthority {
    fn kind(&self) -> AuthorityKind {
        AuthorityKind::None
    }

    fn current(&self, shard: &ShardId) -> Result<Option<WriterLease>, LeaseError> {
        let epochs = self
            .epochs
            .lock()
            .map_err(|_| LeaseError::Unavailable("registre d'époques empoisonné".into()))?;
        Ok(epochs.get(&shard.key()).map(|epoch| WriterLease {
            shard: shard.clone(),
            owner: OwnerId::this_process(),
            epoch: *epoch,
            issued_at_ms: now_ms(),
            expires_at_ms: now_ms() + self.ttl.as_millis() as i64,
            heartbeat: 1,
        }))
    }
}

// ---------------------------------------------------------------------------
// Entrées/sorties de la trame
// ---------------------------------------------------------------------------

/// Lit la trame, ou `None` si le fichier est vide/absent.
///
/// Un fichier **partiellement** écrit (moins de 128 octets, mais non vide) est
/// une trame corrompue, pas un fichier neuf : on refuse plutôt que de repartir
/// de l'époque 1.
fn read_frame(path: &std::path::Path) -> Result<Option<[u8; LeaseRecord::SIZE]>, LeaseError> {
    use std::io::Read;
    let mut file = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(LeaseError::Unavailable(format!("lecture du bail : {e}"))),
    };
    let mut buf = [0u8; LeaseRecord::SIZE];
    let mut read = 0usize;
    while read < buf.len() {
        match file.read(&mut buf[read..]) {
            Ok(0) => break,
            Ok(n) => read += n,
            Err(e) => return Err(LeaseError::Unavailable(format!("lecture du bail : {e}"))),
        }
    }
    if read == 0 {
        return Ok(None);
    }
    if read < LeaseRecord::SIZE {
        return Err(LeaseError::Unavailable(format!(
            "trame de bail tronquée ({read}/{} octets) — réparation explicite requise \
             (LeaseRecord::decode_lossy_epoch), jamais un redémarrage d'époque en silence",
            LeaseRecord::SIZE
        )));
    }
    Ok(Some(buf))
}

/// Écrit la trame et force son écriture sur le disque.
///
/// L'époque ne doit pas pouvoir **reculer** après un crash : sans `sync_all`,
/// un redémarrage pourrait relire l'époque précédente et lever un fencing.
fn write_frame(path: &std::path::Path, bytes: &[u8; LeaseRecord::SIZE]) -> Result<(), LeaseError> {
    use std::io::Write;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(path)
        .map_err(|e| LeaseError::Unavailable(format!("écriture du bail : {e}")))?;
    file.write_all(bytes)
        .map_err(|e| LeaseError::Unavailable(format!("écriture du bail : {e}")))?;
    file.sync_all()
        .map_err(|e| LeaseError::Unavailable(format!("synchronisation du bail : {e}")))?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Tests — le format est GELÉ
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn lease(epoch: u64) -> WriterLease {
        WriterLease {
            shard: ShardId {
                store: StoreId::Dir(PathBuf::from("/tmp/store.obrain")),
                space: Some("tenant-a".into()),
            },
            owner: OwnerId {
                instance: Uuid::from_u128(0x0123_4567_89ab_cdef_0123_4567_89ab_cdef),
                process: 4242,
                host: "node-1".into(),
                started_at_ms: 1_700_000_000_000,
            },
            epoch,
            issued_at_ms: 1_700_000_000_000,
            expires_at_ms: 1_700_000_030_000,
            heartbeat: 7,
        }
    }

    #[test]
    fn the_frame_is_exactly_128_bytes_with_a_frozen_header() {
        let bytes = LeaseRecord::encode(&lease(1));
        assert_eq!(bytes.len(), 128);
        assert_eq!(&bytes[0..8], b"THISLSE1");
        assert_eq!(bytes[8], 1, "version du format");
        assert_eq!(
            u64::from_le_bytes(bytes[12..20].try_into().unwrap()),
            1,
            "l'époque est à offset fixe"
        );
        // Le CRC couvre bien les 120 premiers octets.
        let crc = u32::from_le_bytes(bytes[120..124].try_into().unwrap());
        assert_eq!(crc, crc32(&bytes[..120]));
    }

    #[test]
    fn encode_decode_roundtrips() {
        let l = lease(42);
        let decoded =
            LeaseRecord::decode(&LeaseRecord::encode(&l), l.shard.clone()).expect("trame valide");
        // Tout ce qui est un FAIT DE FENCING survit à l'aller-retour.
        assert_eq!(decoded.shard, l.shard);
        assert_eq!(decoded.epoch, 42);
        assert_eq!(decoded.issued_at_ms, l.issued_at_ms);
        assert_eq!(decoded.expires_at_ms, l.expires_at_ms);
        assert_eq!(decoded.heartbeat, l.heartbeat);
        assert_eq!(decoded.owner.instance, l.owner.instance);
        assert_eq!(decoded.owner.process, l.owner.process);
        assert_eq!(decoded.owner.host, "node-1");
        // …mais `started_at_ms` est un fait LOCAL (diagnostic), pas un fait de
        // fencing : il n'est pas persisté, et c'est délibéré.
        assert_eq!(
            decoded.owner.started_at_ms, 0,
            "l'heure de démarrage locale n'a rien à faire dans une trame partagée"
        );
    }

    #[test]
    fn a_single_flipped_bit_is_detected() {
        // C'est la propriété qui compte : une trame corrompue ne doit JAMAIS
        // être interprétée comme un bail valide.
        for offset in [8usize, 12, 20, 28, 36, 44, 60, 64, 72, 119] {
            let mut b = LeaseRecord::encode(&lease(1));
            b[offset] ^= 0x01;
            let decoded = LeaseRecord::decode(&b, lease(1).shard);
            assert!(
                matches!(decoded, Err(LeaseError::BadCrc)),
                "octet {offset} : la corruption doit être détectée, obtenu {decoded:?}"
            );
        }
    }

    #[test]
    fn wrong_magic_and_wrong_version_are_refused() {
        let mut b = LeaseRecord::encode(&lease(1));
        b[0] = b'X';
        assert_eq!(
            LeaseRecord::decode(&b, lease(1).shard),
            Err(LeaseError::BadMagic)
        );

        let mut b2 = LeaseRecord::encode(&lease(1));
        b2[8] = 99;
        // On recalcule le CRC pour que le refus vienne bien de la VERSION, pas du CRC.
        let crc = crc32(&b2[..120]);
        b2[120..124].copy_from_slice(&crc.to_le_bytes());
        assert_eq!(
            LeaseRecord::decode(&b2, lease(1).shard),
            Err(LeaseError::BadVersion(99))
        );
    }

    #[test]
    fn lossy_epoch_is_explicit_and_never_silent() {
        let mut b = LeaseRecord::encode(&lease(777));
        b[100] ^= 0xff; // casse le CRC sans toucher l'époque
        assert!(matches!(
            LeaseRecord::decode(&b, lease(1).shard),
            Err(LeaseError::BadCrc)
        ));
        // …mais l'époque reste lisible, EXPLICITEMENT, pour la réparation.
        assert_eq!(LeaseRecord::decode_lossy_epoch(&b), Some(777));

        // Une trame sans signature ne rend rien, même en mode lossy.
        let mut z = [0u8; 128];
        z[0] = b'X';
        assert_eq!(LeaseRecord::decode_lossy_epoch(&z), None);
    }

    #[test]
    fn shard_keys_are_stable_distinct_and_space_aware() {
        let base = ShardId {
            store: StoreId::Dir(PathBuf::from("/tmp/a.obrain")),
            space: None,
        };
        let same = base.clone();
        let other_store = ShardId {
            store: StoreId::Dir(PathBuf::from("/tmp/b.obrain")),
            space: None,
        };
        let other_space = ShardId {
            store: StoreId::Dir(PathBuf::from("/tmp/a.obrain")),
            space: Some("tenant-a".into()),
        };

        assert_eq!(base.key(), same.key(), "stable");
        assert_eq!(base.key().len(), 16, "16 caractères hexadécimaux");
        assert_ne!(base.key(), other_store.key(), "deux stores ≠ deux shards");
        assert_ne!(
            base.key(),
            other_space.key(),
            "deux espaces du même store sont deux shards distincts"
        );
        assert_ne!(
            StoreId::Volatile(1),
            StoreId::Dir(PathBuf::from("/tmp/1")),
            "un store volatil n'est pas un store sur disque"
        );
    }

    #[test]
    fn expiry_uses_a_skew_tolerance() {
        let l = lease(1); // expire à 1_700_000_030_000
        assert!(!l.is_expired_at(1_700_000_000_000, Duration::from_secs(5)));
        assert!(!l.is_expired_at(1_700_000_029_000, Duration::from_secs(5)));
        // À 3 s de l'expiration, une tolérance de 5 s protège encore le détenteur :
        // sans elle, on reprendrait le bail alors qu'il écrit peut-être.
        assert!(!l.is_expired_at(1_700_000_027_000, Duration::from_secs(5)));
        assert!(l.is_expired_at(1_700_000_036_000, Duration::from_secs(5)));
        assert!(l.is_expired_at(1_700_000_040_000, Duration::from_secs(5)));
        assert_eq!(l.remaining(1_700_000_025_000), Some(Duration::from_secs(5)));
        assert_eq!(l.remaining(1_700_000_031_000), None);
    }

    #[test]
    fn a_host_longer_than_the_frame_is_truncated_not_rejected() {
        let mut l = lease(1);
        l.owner.host = "x".repeat(100);
        let decoded =
            LeaseRecord::decode(&LeaseRecord::encode(&l), l.shard.clone()).expect("trame valide");
        assert_eq!(decoded.owner.host.len(), LeaseRecord::HOST_MAX);
        assert_eq!(
            decoded.epoch, l.epoch,
            "le fencing n'est pas affecté par l'hôte"
        );
    }

    #[test]
    fn owner_identity_is_stable_within_a_process() {
        let a = OwnerId::this_process();
        let b = OwnerId::this_process();
        assert_ne!(a.instance, b.instance, "chaque appel tire une instance");
        assert_eq!(a.process, std::process::id());
        assert!(a.started_at_ms > 0);
    }

    #[test]
    fn authority_kinds_are_published() {
        // Le défaut est l'absence de fencing : c'est le comportement d'aujourd'hui.
        assert_eq!(AuthorityKind::None, AuthorityKind::None);
        for k in [
            AuthorityKind::None,
            AuthorityKind::File,
            AuthorityKind::Consensus,
        ] {
            let json = serde_json::to_string(&k).unwrap();
            assert!(json.starts_with('"'));
        }
    }

    // ── Autorité de fichier ─────────────────────────────────────────────

    fn dir_shard(dir: &std::path::Path) -> ShardId {
        ShardId {
            store: StoreId::Dir(dir.to_path_buf()),
            space: None,
        }
    }

    #[test]
    fn the_first_lease_gets_epoch_one_and_a_readable_frame() {
        let dir = tempfile::tempdir().unwrap();
        let auth = FileAuthority::new(dir.path());
        let shard = dir_shard(dir.path());
        let handle = auth
            .acquire(&shard, &OwnerId::this_process())
            .expect("premier bail");

        assert_eq!(handle.epoch(), 1);
        let path = handle.path().expect("fichier de bail").to_path_buf();
        assert!(path.exists(), "le fichier de bail doit exister");

        // La trame est relisible, et l'autorité la publie.
        let current = auth.current(&shard).expect("lecture").expect("bail");
        assert_eq!(current.epoch, 1);
        assert_eq!(current.owner.instance, handle.lease().owner.instance);
    }

    /// **LE test qui manquait à obrain.** `obrain-substrate/src/file.rs:179-207`
    /// rend la part de verrou existante : deux ouvertures du même répertoire
    /// dans le MÊME processus réussissent toutes les deux. Ici, non.
    #[test]
    fn a_second_writer_is_refused_even_in_the_same_process() {
        let dir = tempfile::tempdir().unwrap();
        let auth = FileAuthority::new(dir.path());
        let shard = dir_shard(dir.path());

        let first = auth
            .acquire(&shard, &OwnerId::this_process())
            .expect("premier écrivain");
        let second = auth.acquire(&shard, &OwnerId::this_process());
        assert!(
            matches!(second, Err(LeaseError::LockRefused { .. })),
            "le second écrivain doit être REFUSÉ, obtenu {:?}",
            second.map(|h| h.epoch())
        );

        // Et le refus n'a rien écrit : l'époque du détenteur est intacte.
        let current = auth.current(&shard).expect("lecture").expect("bail");
        assert_eq!(current.epoch, 1, "un refus ne doit pas consommer d'époque");

        // Le premier détenteur est toujours vivant.
        first.assert_live().expect("le détenteur garde son bail");
    }

    #[test]
    fn epochs_are_monotone_across_holders() {
        let dir = tempfile::tempdir().unwrap();
        let auth = FileAuthority::new(dir.path());
        let shard = dir_shard(dir.path());

        let a = auth.acquire(&shard, &OwnerId::this_process()).unwrap();
        assert_eq!(a.epoch(), 1);
        a.release();

        let b = auth.acquire(&shard, &OwnerId::this_process()).unwrap();
        assert_eq!(b.epoch(), 2, "l'époque ne recule JAMAIS");
        b.release();

        let c = auth.acquire(&shard, &OwnerId::this_process()).unwrap();
        assert_eq!(c.epoch(), 3);
    }

    #[test]
    fn the_tripwire_detects_a_foreign_epoch() {
        let dir = tempfile::tempdir().unwrap();
        let auth = FileAuthority::new(dir.path());
        let shard = dir_shard(dir.path());
        let handle = auth.acquire(&shard, &OwnerId::this_process()).unwrap();
        assert_eq!(handle.epoch(), 1);
        handle.tripwire().expect("rien n'a bougé");

        // Un autre écrivain a pris le relais : on écrit une trame d'époque 2.
        let mut other = lease(2);
        other.shard = shard.clone();
        other.owner = OwnerId::this_process();
        other.owner.instance = Uuid::new_v4();
        write_frame(handle.path().unwrap(), &LeaseRecord::encode(&other)).unwrap();

        match handle.tripwire() {
            Err(LeaseError::Fenced { mine, current }) => {
                assert_eq!(mine, 1);
                assert_eq!(current, 2);
            }
            other => panic!("le tripwire devait figer ce writer, obtenu {other:?}"),
        }
        // Une fois figé, on le reste — même sur le chemin chaud.
        assert!(matches!(
            handle.assert_live(),
            Err(LeaseError::Fenced { .. })
        ));
    }

    #[test]
    fn a_fenced_writer_cannot_renew_or_release_into_a_new_lease() {
        let dir = tempfile::tempdir().unwrap();
        let auth = FileAuthority::new(dir.path());
        let shard = dir_shard(dir.path());
        let handle = auth.acquire(&shard, &OwnerId::this_process()).unwrap();
        handle.fence_for_test(99);
        assert!(matches!(
            handle.assert_live(),
            Err(LeaseError::Fenced { current: 99, .. })
        ));
        assert!(
            handle.renew().is_err(),
            "un writer périmé ne peut pas prolonger son bail"
        );
    }

    #[test]
    fn on_commit_triggers_the_tripwire_every_n_commits() {
        let dir = tempfile::tempdir().unwrap();
        let auth = FileAuthority::new(dir.path());
        let shard = dir_shard(dir.path());
        let handle = auth
            .acquire(&shard, &OwnerId::this_process())
            .unwrap()
            .with_tripwire_every(2);

        handle.on_commit().expect("premier commit");

        // Un autre écrivain prend le relais.
        let mut other = lease(7);
        other.shard = shard.clone();
        other.owner.instance = Uuid::new_v4();
        write_frame(handle.path().unwrap(), &LeaseRecord::encode(&other)).unwrap();

        // Le 2ᵉ commit déclenche le contrôle disque : on est figé.
        assert!(
            matches!(handle.on_commit(), Err(LeaseError::Fenced { .. })),
            "le tripwire doit mordre au 2ᵉ commit"
        );
    }

    #[test]
    fn renew_extends_the_lease_and_rewrites_the_frame() {
        let dir = tempfile::tempdir().unwrap();
        let auth = FileAuthority::new(dir.path());
        let shard = dir_shard(dir.path());
        let handle = auth.acquire(&shard, &OwnerId::this_process()).unwrap();
        let before = auth.current(&shard).unwrap().unwrap();

        let renewed = handle.renew().expect("renouvellement");
        assert!(renewed.heartbeat > before.heartbeat);
        assert!(renewed.expires_at_ms >= before.expires_at_ms);

        let after = auth.current(&shard).unwrap().unwrap();
        assert_eq!(after.epoch, 1, "renouveler ne change pas l'époque");
        assert_eq!(after.heartbeat, renewed.heartbeat);
    }

    #[test]
    fn an_expired_lease_is_refused_even_without_contention() {
        let dir = tempfile::tempdir().unwrap();
        let auth = FileAuthority {
            root: dir.path().to_path_buf(),
            ttl: Duration::ZERO, // expire immédiatement
            skew: Duration::ZERO,
            tripwire_every: 256,
        };
        let shard = dir_shard(dir.path());
        let handle = auth.acquire(&shard, &OwnerId::this_process()).unwrap();
        assert!(
            matches!(handle.assert_live(), Err(LeaseError::Fenced { .. })),
            "un bail expiré n'est plus un droit d'écrire, même sans concurrent"
        );
    }

    /// Une trame corrompue ne doit **jamais** faire repartir l'époque à 1 :
    /// cela lèverait un fencing en cours. La réparation est explicite.
    #[test]
    fn a_corrupt_frame_refuses_acquisition_instead_of_resetting_the_epoch() {
        let dir = tempfile::tempdir().unwrap();
        let auth = FileAuthority::new(dir.path());
        let shard = dir_shard(dir.path());
        let handle = auth.acquire(&shard, &OwnerId::this_process()).unwrap();
        assert_eq!(handle.epoch(), 1);
        let path = handle.path().unwrap().to_path_buf();
        handle.release();

        // On abîme la trame.
        let mut bytes = std::fs::read(&path).unwrap();
        bytes[40] ^= 0xff;
        std::fs::write(&path, &bytes).unwrap();

        match auth.acquire(&shard, &OwnerId::this_process()) {
            Err(LeaseError::Unavailable(detail)) => {
                assert!(
                    detail.contains("CRC") || detail.contains("corromp"),
                    "la raison doit nommer la corruption : {detail}"
                );
            }
            other => panic!(
                "attendu un refus explicite, obtenu {:?}",
                other.map(|h| h.epoch())
            ),
        }
    }

    #[test]
    fn a_truncated_frame_is_corruption_not_a_fresh_start() {
        let dir = tempfile::tempdir().unwrap();
        let auth = FileAuthority::new(dir.path());
        let shard = dir_shard(dir.path());
        let path = auth.lease_path(&shard);
        std::fs::write(&path, b"THISLSE1").unwrap(); // 8 octets seulement

        match auth.acquire(&shard, &OwnerId::this_process()) {
            Err(LeaseError::Unavailable(detail)) => {
                assert!(detail.contains("tronquée"), "message attendu : {detail}")
            }
            other => panic!("attendu un refus, obtenu {:?}", other.map(|h| h.epoch())),
        }
    }

    #[test]
    fn distinct_spaces_get_distinct_lease_files() {
        let dir = tempfile::tempdir().unwrap();
        let auth = FileAuthority::new(dir.path());
        let a = ShardId {
            store: StoreId::Dir(dir.path().to_path_buf()),
            space: None,
        };
        let b = ShardId {
            store: StoreId::Dir(dir.path().to_path_buf()),
            space: Some("tenant-a".into()),
        };
        assert_ne!(auth.lease_path(&a), auth.lease_path(&b));
        let ha = auth.acquire(&a, &OwnerId::this_process()).unwrap();
        let hb = auth
            .acquire(&b, &OwnerId::this_process())
            .expect("deux espaces = deux shards, donc deux baux simultanés");
        assert_eq!(ha.epoch(), 1);
        assert_eq!(hb.epoch(), 1, "les époques sont par shard");
    }

    #[test]
    fn the_memory_authority_declares_that_it_does_not_fence() {
        let auth = MemoryAuthority::new();
        let shard = ShardId {
            store: StoreId::Volatile(1),
            space: None,
        };
        assert_eq!(
            auth.kind(),
            AuthorityKind::None,
            "sans support persistant, aucun fencing réel n'est possible — et on le DIT"
        );
        let a = auth.acquire(&shard, &OwnerId::this_process()).unwrap();
        assert_eq!(a.epoch(), 2, "l'époque part de 1 puis s'incrémente");
        let b = auth.acquire(&shard, &OwnerId::this_process()).unwrap();
        assert_eq!(b.epoch(), 3, "monotone dans le processus");
        assert!(auth.current(&shard).unwrap().is_some());

        // Un store volatil ne peut pas avoir de bail de fichier.
        let file_auth = FileAuthority::new(tempfile::tempdir().unwrap().path());
        assert!(matches!(
            file_auth.acquire(&shard, &OwnerId::this_process()),
            Err(LeaseError::Unavailable(_))
        ));
    }
}
