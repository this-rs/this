//! L'état du service : configuration, stockage, clés, runner, file d'attente — et le
//! worker qui exécute les builds UN À LA FOIS.

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;
use chrono::{DateTime, Utc};
use tokio::sync::{Mutex, mpsc};
use uuid::Uuid;

use crate::auth::hash_password;
use crate::config::ForgeConfig;
use crate::entities::{Build, Invitation};
use crate::runner::{BuildOutcome, BuildRunner, Diagnostic, SourceFile};
use crate::sign::{Attestation, ForgeKeys, b64url, canonical_json, sha256_hex};
use crate::store::Store;

pub const FORGE_VERSION: &str = env!("CARGO_PKG_VERSION");

pub struct Forge {
    pub cfg: ForgeConfig,
    pub store: Store,
    pub keys: ForgeKeys,
    pub runner: BuildRunner,
    queue: mpsc::UnboundedSender<Uuid>,
    /// Sérialise l'inscription : un code à usage unique ne se consomme pas deux fois.
    pub signup_lock: Mutex<()>,
    /// Hachage factice : `login` coûte le même temps pour un e-mail inconnu.
    pub dummy_hash: String,
}

impl Forge {
    /// Démarre la forge (dans un runtime tokio) : ouvre le stockage, charge ou crée les
    /// clés, reprend les builds interrompus et lance le worker.
    pub async fn start(cfg: ForgeConfig) -> Result<Arc<Self>> {
        std::fs::create_dir_all(cfg.data_dir.join("artifacts"))?;
        std::fs::create_dir_all(cfg.data_dir.join("sources"))?;
        let store = Store::from_config(&cfg)?;
        let keys = ForgeKeys::load_or_create(&cfg.signing_key_path)?;
        crate::entities::forge_descriptor_set()?; // le contrat du module est cohérent, ou on ne démarre pas
        let (tx, rx) = mpsc::unbounded_channel();
        let forge = Arc::new(Self {
            runner: BuildRunner::new(cfg.clone()),
            cfg,
            store,
            keys,
            queue: tx,
            signup_lock: Mutex::new(()),
            dummy_hash: tokio::task::spawn_blocking(|| hash_password("dummy-password-for-timing"))
                .await??,
        });
        forge.recover().await?;
        tokio::spawn(worker(forge.clone(), rx));
        Ok(forge)
    }

    pub fn artifact_path(&self, id: &Uuid) -> PathBuf {
        self.cfg
            .data_dir
            .join("artifacts")
            .join(format!("{id}.wasm"))
    }

    fn source_path(&self, id: &Uuid) -> PathBuf {
        self.cfg.data_dir.join("sources").join(format!("{id}.json"))
    }

    /// Met un build en file : ses sources sont écrites AVANT, pour survivre à un redémarrage.
    pub async fn enqueue(&self, id: Uuid, files: &[SourceFile]) -> Result<()> {
        std::fs::write(self.source_path(&id), serde_json::to_vec(files)?)?;
        self.queue
            .send(id)
            .map_err(|_| anyhow::anyhow!("worker arrêté"))
    }

    /// Au démarrage : un build `building` est mort avec le processus ; un `queued` reprend.
    async fn recover(&self) -> Result<()> {
        let mut pending: Vec<Build> = Vec::new();
        for mut b in self.store.builds.list().await? {
            match b.status.as_str() {
                "building" => {
                    b.status = "failed".into();
                    b.finished_at = Some(Utc::now().to_rfc3339());
                    b.diagnostics_json = serde_json::to_string(&[Diagnostic::error(
                        "build interrompu par un redémarrage de la forge",
                    )])?;
                    self.store.builds.update(&b.id.clone(), b).await?;
                }
                "queued" => pending.push(b),
                _ => {}
            }
        }
        pending.sort_by_key(|b| b.created_at);
        for b in pending {
            let id = b.id;
            if self.source_path(&id).exists() {
                let _ = self.queue.send(id);
            } else {
                self.fail(
                    b,
                    vec![Diagnostic::error("sources perdues pendant un redémarrage")],
                )
                .await?;
            }
        }
        Ok(())
    }

    async fn fail(&self, mut b: Build, diags: Vec<Diagnostic>) -> Result<()> {
        b.status = "failed".into();
        b.finished_at = Some(Utc::now().to_rfc3339());
        b.diagnostics_json = serde_json::to_string(&diags)?;
        b.updated_at = Utc::now();
        self.store.builds.update(&b.id.clone(), b).await?;
        Ok(())
    }

    /// Crée une invitation ; rend le code EN CLAIR (la seule fois où il existe).
    pub async fn create_invitation(&self, uses: i64, note: Option<String>) -> Result<String> {
        let mut raw = [0u8; 12];
        argon2::password_hash::rand_core::RngCore::fill_bytes(
            &mut argon2::password_hash::rand_core::OsRng,
            &mut raw,
        );
        let code = format!("forge-{}", b64url(&raw));
        let inv = Invitation::new(sha256_hex(code.as_bytes()), "active".into(), uses, note);
        self.store.invitations.create(inv).await?;
        Ok(code)
    }

    async fn run_one(&self, id: Uuid) -> Result<()> {
        let Some(mut build) = self.store.builds.get(&id).await? else {
            return Ok(());
        };
        if build.status != "queued" {
            return Ok(());
        }
        let files: Vec<SourceFile> = match std::fs::read(self.source_path(&id)) {
            Ok(b) => serde_json::from_slice(&b)?,
            Err(_) => {
                return self
                    .fail(build, vec![Diagnostic::error("sources introuvables")])
                    .await;
            }
        };
        build.status = "building".into();
        build.started_at = Some(Utc::now().to_rfc3339());
        build = self.store.builds.update(&id, build).await?;
        let toolchain = self.runner.toolchain().await;
        build.toolchain = toolchain.clone();
        let outcome = self.runner.run(&files).await;
        let _ = std::fs::remove_file(self.source_path(&id));
        let finished: DateTime<Utc> = Utc::now();
        match outcome {
            BuildOutcome::Failed {
                diagnostics,
                elapsed,
            } => {
                build.duration_ms = elapsed.as_millis() as i64;
                self.fail(build, diagnostics).await
            }
            BuildOutcome::Wasm {
                bytes,
                diagnostics,
                elapsed,
            } => {
                let wasm_sha = sha256_hex(&bytes);
                let built_at = finished.to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
                let att = Attestation {
                    account: &build.account_id,
                    built_at: &built_at,
                    forge_version: FORGE_VERSION,
                    module_name: &build.name,
                    module_version: &build.version,
                    source_sha256: &build.source_sha256,
                    toolchain: &toolchain,
                    wasm_sha256: &wasm_sha,
                };
                let canonical = att.canonical();
                std::fs::write(self.artifact_path(&id), &bytes)?;
                build.signature = Some(self.keys.sign(canonical.as_bytes()));
                build.manifest_b64 = Some(b64url(canonical.as_bytes()));
                build.wasm_sha256 = Some(wasm_sha);
                build.wasm_size = Some(bytes.len() as i64);
                build.status = "signed".into();
                build.finished_at = Some(finished.to_rfc3339());
                build.duration_ms = elapsed.as_millis() as i64;
                build.diagnostics_json = serde_json::to_string(&diagnostics)?;
                build.updated_at = Utc::now();
                self.store.builds.update(&id, build).await?;
                Ok(())
            }
        }
    }
}

/// Un seul build à la fois : la file est ce récepteur.
async fn worker(forge: Arc<Forge>, mut rx: mpsc::UnboundedReceiver<Uuid>) {
    while let Some(id) = rx.recv().await {
        if let Err(e) = forge.run_one(id).await {
            tracing::error!(build = %id, "build en erreur interne : {e:#}");
            if let Ok(Some(b)) = forge.store.builds.get(&id).await {
                let _ = forge
                    .fail(
                        b,
                        vec![Diagnostic::error(format!(
                            "erreur interne de la forge : {e}"
                        ))],
                    )
                    .await;
            }
        }
    }
}

/// Le JSON canonique d'une valeur — ré-exporté pour les clients qui vérifient.
pub fn canonical(v: &serde_json::Value) -> String {
    canonical_json(v)
}
