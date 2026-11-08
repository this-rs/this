//! Le runner de build : écrit l'arbre du client dans un répertoire temporaire, y pose le
//! gabarit IMPOSÉ (Cargo.toml + Cargo.lock de la forge), lance le conteneur sans réseau et
//! rend soit un `.wasm` éprouvé, soit des diagnostics structurés.

use std::path::Path;
use std::process::Stdio;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::process::Command;
use tokio::sync::OnceCell;

use crate::config::ForgeConfig;
use crate::wasm_check::check_abi;

/// Le gabarit : même source de vérité que `docker/Dockerfile` (qui vendore ces dépendances).
pub const TEMPLATE_CARGO_TOML: &str = include_str!("../docker/template/Cargo.toml");
pub const TEMPLATE_CARGO_LOCK: &str = include_str!("../docker/template/Cargo.lock");

/// Un fichier source du client.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourceFile {
    pub path: String,
    pub content: String,
}

/// `{file, line, column, level, message}` — la forme que lit un agent.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Diagnostic {
    pub file: String,
    pub line: u32,
    pub column: u32,
    pub level: String,
    pub message: String,
}

impl Diagnostic {
    pub fn error(message: impl Into<String>) -> Self {
        Self {
            file: String::new(),
            line: 0,
            column: 0,
            level: "error".into(),
            message: message.into(),
        }
    }
}

/// Résultat d'un build.
#[derive(Debug)]
pub enum BuildOutcome {
    Wasm {
        bytes: Vec<u8>,
        diagnostics: Vec<Diagnostic>,
        elapsed: Duration,
    },
    Failed {
        diagnostics: Vec<Diagnostic>,
        elapsed: Duration,
    },
}

/// Refus de validation d'une soumission : `(code stable, message lisible)`.
pub type Refusal = (&'static str, String);

/// Valide l'arbre soumis. Le client ne fournit QUE des sources `src/**.rs` : pas de
/// `Cargo.toml`, pas de `build.rs`, pas de `.cargo/config` — c'est ce qui rend le gabarit
/// imposé réellement imposé.
pub fn validate_files(files: &[SourceFile], cfg: &ForgeConfig) -> Result<(), Refusal> {
    if files.is_empty() {
        return Err(("invalid_files", "aucun fichier".into()));
    }
    if files.len() > cfg.max_files {
        return Err((
            "invalid_files",
            format!("{} fichiers, maximum {}", files.len(), cfg.max_files),
        ));
    }
    let total: usize = files.iter().map(|f| f.content.len()).sum();
    if total > cfg.max_source_bytes {
        return Err((
            "invalid_files",
            format!(
                "sources trop volumineuses : {total} octets, maximum {}",
                cfg.max_source_bytes
            ),
        ));
    }
    let mut seen = std::collections::HashSet::new();
    for f in files {
        let p = f.path.as_str();
        let ok_chars = p
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.' | '/'));
        let segments_ok = p.split('/').all(|s| !s.is_empty() && s != "." && s != "..");
        if !p.starts_with("src/") || !p.ends_with(".rs") || !ok_chars || !segments_ok {
            return Err((
                "invalid_files",
                format!(
                    "chemin refusé : `{p}`. Seuls des fichiers `src/**/*.rs` sont acceptés ; \
                     Cargo.toml, build.rs et la configuration cargo sont imposés par la forge"
                ),
            ));
        }
        if !seen.insert(p) {
            return Err(("invalid_files", format!("chemin en double : `{p}`")));
        }
    }
    if !files.iter().any(|f| f.path == "src/lib.rs") {
        return Err((
            "invalid_files",
            "`src/lib.rs` manque : c'est la racine du module".into(),
        ));
    }
    Ok(())
}

/// Valide un nom de module : `[a-z][a-z0-9_-]{0,63}`.
pub fn valid_module_name(n: &str) -> bool {
    let mut c = n.chars();
    matches!(c.next(), Some('a'..='z'))
        && n.len() <= 64
        && c.all(|x| x.is_ascii_lowercase() || x.is_ascii_digit() || x == '_' || x == '-')
}

/// Valide une version `MAJOR.MINOR.PATCH` (suffixe `-pre` / `+build` admis).
pub fn valid_version(v: &str) -> bool {
    let core = v.split(['-', '+']).next().unwrap_or("");
    let parts: Vec<&str> = core.split('.').collect();
    v.len() <= 64
        && parts.len() == 3
        && parts
            .iter()
            .all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()))
        && v[core.len()..]
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '+'))
}

/// Le runner. Sans état de build : la file d'attente est ailleurs (`queue.rs`).
pub struct BuildRunner {
    cfg: ForgeConfig,
    toolchain: OnceCell<String>,
}

impl BuildRunner {
    pub fn new(cfg: ForgeConfig) -> Self {
        Self {
            cfg,
            toolchain: OnceCell::new(),
        }
    }

    /// `rustc 1.93.1`, lu UNE fois dans l'image (c'est ce qui compile, pas l'hôte).
    pub async fn toolchain(&self) -> String {
        self.toolchain
            .get_or_init(|| async {
                let out = Command::new(&self.cfg.docker_bin)
                    .args([
                        "run",
                        "--rm",
                        "--network",
                        "none",
                        &self.cfg.image,
                        "rustc",
                        "-V",
                    ])
                    .output()
                    .await;
                match out {
                    Ok(o) if o.status.success() => {
                        let s = String::from_utf8_lossy(&o.stdout);
                        s.split_whitespace().take(2).collect::<Vec<_>>().join(" ")
                    }
                    _ => "rustc unknown".to_string(),
                }
            })
            .await
            .clone()
    }

    pub async fn run(&self, files: &[SourceFile]) -> BuildOutcome {
        self.run_with_extra(files, &[]).await
    }

    /// Comme `run`, avec des fichiers ajoutés APRÈS le gabarit, hors validation. Réservé
    /// aux tests qui prouvent que le conteneur — et non la validation — tient (ex. un
    /// `build.rs` qui tente le réseau).
    #[doc(hidden)]
    pub async fn run_with_extra(
        &self,
        files: &[SourceFile],
        extra: &[(&str, &str)],
    ) -> BuildOutcome {
        let start = Instant::now();
        match self.run_inner(files, extra, start).await {
            Ok(o) => o,
            Err(e) => BuildOutcome::Failed {
                diagnostics: vec![Diagnostic::error(format!(
                    "la forge n'a pas pu lancer le build : {e}"
                ))],
                elapsed: start.elapsed(),
            },
        }
    }

    async fn run_inner(
        &self,
        files: &[SourceFile],
        extra: &[(&str, &str)],
        start: Instant,
    ) -> anyhow::Result<BuildOutcome> {
        let tmp = tempfile::tempdir()?;
        let root = tmp.path().canonicalize()?;
        let crate_dir = root.join("crate");
        let out_dir = root.join("out");
        std::fs::create_dir_all(crate_dir.join("src"))?;
        std::fs::create_dir_all(&out_dir)?;
        std::fs::write(crate_dir.join("Cargo.toml"), TEMPLATE_CARGO_TOML)?;
        std::fs::write(crate_dir.join("Cargo.lock"), TEMPLATE_CARGO_LOCK)?;
        for f in files {
            write_file(&crate_dir, &f.path, &f.content)?;
        }
        for (p, c) in extra {
            write_file(&crate_dir, p, c)?;
        }
        // Le conteneur tourne sous l'uid de la forge : il peut écrire dans `out/` et rien
        // d'autre que ses tmpfs.
        let (uid, gid) = owner_of(&root)?;
        let name = format!("forge-{}", uuid::Uuid::new_v4());
        let script = "cargo build --offline --locked --release --target wasm32-unknown-unknown \
                      --message-format=json; rc=$?; \
                      [ -f /tmp/target/wasm32-unknown-unknown/release/module.wasm ] \
                      && cp /tmp/target/wasm32-unknown-unknown/release/module.wasm /out/module.wasm; \
                      exit $rc";
        let mut cmd = Command::new(&self.cfg.docker_bin);
        cmd.args([
            "run",
            "--rm",
            "--name",
            &name,
            "--network",
            "none",
            "--read-only",
        ])
        .args(["--cap-drop", "ALL", "--security-opt", "no-new-privileges"])
        .args(["--cpus", &self.cfg.cpus.to_string()])
        .args(["--cpu-shares", &self.cfg.cpu_shares.to_string()])
        .args(["--blkio-weight", &self.cfg.blkio_weight.to_string()])
        .args([
            "--memory",
            &self.cfg.memory,
            "--memory-swap",
            &self.cfg.memory,
        ])
        .args(["--pids-limit", "256", "--user", &format!("{uid}:{gid}")])
        .args(["--tmpfs", "/tmp:rw,exec,nosuid,size=2g"])
        .arg("-v")
        .arg(format!("{}:/crate:ro", crate_dir.display()))
        .arg("-v")
        .arg(format!("{}:/out:rw", out_dir.display()))
        .args([
            "-e",
            "CARGO_TARGET_DIR=/tmp/target",
            "-e",
            "HOME=/tmp",
            "-w",
            "/crate",
        ])
        .arg(&self.cfg.image)
        .args(["sh", "-c", script])
        .stdin(Stdio::null())
        .kill_on_drop(true);
        let result = tokio::time::timeout(self.cfg.build_timeout, cmd.output()).await;
        let output = match result {
            Ok(o) => o?,
            Err(_) => {
                // `kill_on_drop` tue le client docker, pas le conteneur : on le tue nous-mêmes.
                let _ = Command::new(&self.cfg.docker_bin)
                    .args(["kill", &name])
                    .output()
                    .await;
                return Ok(BuildOutcome::Failed {
                    diagnostics: vec![Diagnostic::error(format!(
                        "build interrompu : durée maximale de {} s dépassée",
                        self.cfg.build_timeout.as_secs()
                    ))],
                    elapsed: start.elapsed(),
                });
            }
        };
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        let mut diagnostics = parse_cargo_json(&stdout);
        let elapsed = start.elapsed();
        if !output.status.success() {
            if !diagnostics.iter().any(|d| d.level == "error") {
                // Pas de message structuré (build script, docker, mémoire…) : la fin de stderr.
                let tail: String = stderr
                    .chars()
                    .rev()
                    .take(2000)
                    .collect::<Vec<_>>()
                    .into_iter()
                    .rev()
                    .collect();
                diagnostics.push(Diagnostic::error(tail.trim().to_string()));
            }
            return Ok(BuildOutcome::Failed {
                diagnostics,
                elapsed,
            });
        }
        let wasm_path = out_dir.join("module.wasm");
        let bytes = match std::fs::read(&wasm_path) {
            Ok(b) => b,
            Err(_) => {
                diagnostics.push(Diagnostic::error(
                    "le build a réussi mais n'a produit aucun module.wasm",
                ));
                return Ok(BuildOutcome::Failed {
                    diagnostics,
                    elapsed,
                });
            }
        };
        if bytes.len() > self.cfg.max_wasm_bytes {
            diagnostics.push(Diagnostic::error(format!(
                "artefact trop gros : {} octets, maximum {}",
                bytes.len(),
                self.cfg.max_wasm_bytes
            )));
            return Ok(BuildOutcome::Failed {
                diagnostics,
                elapsed,
            });
        }
        if let Err(violations) = check_abi(&bytes) {
            diagnostics.extend(
                violations
                    .into_iter()
                    .map(|v| Diagnostic::error(format!("ABI : {}", v.0))),
            );
            return Ok(BuildOutcome::Failed {
                diagnostics,
                elapsed,
            });
        }
        Ok(BuildOutcome::Wasm {
            bytes,
            diagnostics,
            elapsed,
        })
    }
}

fn write_file(root: &Path, rel: &str, content: &str) -> std::io::Result<()> {
    let p = root.join(rel);
    if let Some(d) = p.parent() {
        std::fs::create_dir_all(d)?;
    }
    std::fs::write(p, content)
}

#[cfg(unix)]
fn owner_of(p: &Path) -> anyhow::Result<(u32, u32)> {
    use std::os::unix::fs::MetadataExt;
    let m = std::fs::metadata(p)?;
    Ok((m.uid(), m.gid()))
}
#[cfg(not(unix))]
fn owner_of(_: &Path) -> anyhow::Result<(u32, u32)> {
    Ok((1000, 1000))
}

/// Analyse la sortie `--message-format=json` de cargo : un diagnostic par message du
/// compilateur qui porte une position (le résumé « aborting due to… » n'en porte pas).
pub fn parse_cargo_json(stdout: &str) -> Vec<Diagnostic> {
    let mut out = Vec::new();
    for line in stdout.lines() {
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if v.get("reason").and_then(Value::as_str) != Some("compiler-message") {
            continue;
        }
        let m = &v["message"];
        let level = m["level"].as_str().unwrap_or("error");
        if !matches!(level, "error" | "warning") {
            continue;
        }
        let text = m["message"].as_str().unwrap_or("").to_string();
        let primary = m["spans"].as_array().and_then(|s| {
            s.iter()
                .find(|s| s["is_primary"].as_bool() == Some(true))
                .or_else(|| s.first())
        });
        let (file, line_no, col) = match primary {
            Some(s) => (
                s["file_name"].as_str().unwrap_or("").to_string(),
                s["line_start"].as_u64().unwrap_or(0) as u32,
                s["column_start"].as_u64().unwrap_or(0) as u32,
            ),
            None if level == "error" && text.starts_with("aborting due to") => continue,
            None if level == "warning" && text.contains("generated") => continue,
            None => (String::new(), 0, 0),
        };
        let message = match m["code"]["code"].as_str() {
            Some(c) => format!("{c}: {text}"),
            None => text,
        };
        out.push(Diagnostic {
            file,
            line: line_no,
            column: col,
            level: level.to_string(),
            message,
        });
        if out.len() >= 100 {
            break;
        }
    }
    out
}
