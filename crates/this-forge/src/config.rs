//! Configuration de la forge. Tout a un défaut sûr ; rien n'est lu dans l'environnement
//! ici (c'est `main.rs` qui traduit les arguments), pour que les tests restent déterministes.

use std::path::PathBuf;
use std::time::Duration;

/// Où vivent les données des comptes.
#[derive(Debug, Clone)]
pub enum StoreKind {
    /// Tout en mémoire : tests, démonstrations. Perdu à l'arrêt.
    Memory,
    /// LMDB sous `data_dir/db/<entité>` (un environnement par entité).
    Lmdb,
}

/// Réglages d'une forge.
#[derive(Debug, Clone)]
pub struct ForgeConfig {
    /// Répertoire des données : artefacts (`artifacts/<id>.wasm`), LMDB (`db/`).
    pub data_dir: PathBuf,
    /// Fichier de la clé de signature Ed25519 (mode 0600). La clé des jetons est à côté,
    /// suffixée `.jwt`.
    pub signing_key_path: PathBuf,
    /// Stockage des entités.
    pub store: StoreKind,
    /// Image de build (cf. `docker/Dockerfile`).
    pub image: String,
    /// Binaire docker.
    pub docker_bin: String,
    /// Durée maximale d'un build.
    pub build_timeout: Duration,
    /// CPU alloués au conteneur.
    pub cpus: u32,
    /// Mémoire du conteneur (`--memory`).
    pub memory: String,
    /// Poids CPU relatif du conteneur (`--cpu-shares`, défaut docker 1024) : bas = cède
    /// devant les autres charges de la machine.
    pub cpu_shares: u32,
    /// Poids d'E/S du conteneur (`--blkio-weight`, 10..1000).
    pub blkio_weight: u16,
    /// Builds autorisés par compte et par jour UTC.
    pub builds_per_day: u32,
    /// Budget de durée de build par compte et par jour UTC (secondes de mur).
    pub cpu_seconds_per_day: u64,
    /// Builds en attente ou en cours par compte.
    pub max_pending_per_account: u32,
    /// Durée de vie d'un jeton d'accès.
    pub token_ttl: Duration,
    /// Durée de vie d'un jeton de rafraîchissement.
    pub refresh_ttl: Duration,
    /// Nombre maximal de fichiers sources d'un build.
    pub max_files: usize,
    /// Taille totale maximale des sources (octets).
    pub max_source_bytes: usize,
    /// Taille maximale d'un artefact accepté (octets).
    pub max_wasm_bytes: usize,
}

impl ForgeConfig {
    /// Défauts de bêta fermée, sous `data_dir`.
    pub fn new(data_dir: impl Into<PathBuf>) -> Self {
        let data_dir = data_dir.into();
        Self {
            signing_key_path: data_dir.join("forge-signing.key"),
            data_dir,
            store: StoreKind::Lmdb,
            image: "this-forge-builder:1.93".into(),
            docker_bin: "docker".into(),
            build_timeout: Duration::from_secs(180),
            cpus: 6,
            memory: "4g".into(),
            cpu_shares: 1024,
            blkio_weight: 500,
            builds_per_day: 20,
            cpu_seconds_per_day: 1800,
            max_pending_per_account: 2,
            token_ttl: Duration::from_secs(15 * 60),
            refresh_ttl: Duration::from_secs(30 * 24 * 3600),
            max_files: 64,
            max_source_bytes: 1024 * 1024,
            max_wasm_bytes: 4 * 1024 * 1024,
        }
    }
}
