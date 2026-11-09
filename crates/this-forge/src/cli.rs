//! La ligne de commande `this-forge serve | invite`, hors de `main.rs` pour être testée.
//!
//!   this-forge serve  [--listen 127.0.0.1:8480] [--data-dir ./forge-data]
//!                     [--signing-key <chemin>] [--image this-forge-builder:1.93]
//!                     [--cpus 6] [--memory 4g] [--cpu-shares 1024] [--blkio-weight 500]
//!   this-forge invite [--data-dir ./forge-data] [--uses 1] [--note "…"]
//!   this-forge --version | --help

use std::io::Write;
use std::net::SocketAddr;

use anyhow::{Context, Result};

use crate::{Forge, ForgeConfig};

pub const USAGE: &str = "\
this-forge serve  [--listen 127.0.0.1:8480] [--data-dir ./forge-data]
                  [--signing-key <chemin>] [--image this-forge-builder:1.93]
                  [--cpus 6] [--memory 4g] [--cpu-shares 1024] [--blkio-weight 500]
this-forge invite [--data-dir ./forge-data] [--uses 1] [--note \"…\"]
this-forge --version | --help
";

/// Ce que la ligne de commande demande, une fois les arguments lus.
#[derive(Debug)]
pub enum Command {
    Serve {
        cfg: ForgeConfig,
        listen: SocketAddr,
    },
    Invite {
        cfg: ForgeConfig,
        uses: i64,
        note: Option<String>,
    },
    Version,
    Help,
}

fn arg(args: &[String], name: &str) -> Option<String> {
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1).cloned())
}

fn num<T: std::str::FromStr>(args: &[String], name: &str) -> Result<Option<T>>
where
    T::Err: std::error::Error + Send + Sync + 'static,
{
    arg(args, name)
        .map(|v| {
            v.parse::<T>()
                .with_context(|| format!("{name} : valeur illisible `{v}`"))
        })
        .transpose()
}

/// Lit les arguments (sans le nom du programme). Sans sous-commande : `serve`.
pub fn parse(args: &[String]) -> Result<Command> {
    let cmd = args.first().map(String::as_str).unwrap_or("serve");
    match cmd {
        "--version" | "-V" | "version" => return Ok(Command::Version),
        "--help" | "-h" | "help" => return Ok(Command::Help),
        "serve" | "invite" => {}
        other => anyhow::bail!("commande inconnue `{other}` : serve | invite"),
    }
    let mut cfg =
        ForgeConfig::new(arg(args, "--data-dir").unwrap_or_else(|| "./forge-data".into()));
    if let Some(k) = arg(args, "--signing-key") {
        cfg.signing_key_path = k.into();
    }
    if let Some(i) = arg(args, "--image") {
        cfg.image = i;
    }
    if let Some(c) = num(args, "--cpus")? {
        cfg.cpus = c;
    }
    if let Some(m) = arg(args, "--memory") {
        cfg.memory = m;
    }
    if let Some(c) = num(args, "--cpu-shares")? {
        cfg.cpu_shares = c;
    }
    if let Some(c) = num(args, "--blkio-weight")? {
        cfg.blkio_weight = c;
    }
    if cmd == "serve" {
        let listen = arg(args, "--listen").unwrap_or_else(|| "127.0.0.1:8480".into());
        let listen = listen
            .parse()
            .with_context(|| format!("--listen : adresse illisible `{listen}`"))?;
        Ok(Command::Serve { cfg, listen })
    } else {
        Ok(Command::Invite {
            uses: num(args, "--uses")?.unwrap_or(1),
            note: arg(args, "--note"),
            cfg,
        })
    }
}

/// Exécute une commande. `serve` ne rend la main qu'en cas d'erreur.
pub async fn execute(cmd: Command, out: &mut (dyn Write + Send)) -> Result<()> {
    match cmd {
        Command::Version => {
            writeln!(out, "this-forge {}", crate::service::FORGE_VERSION)?;
            Ok(())
        }
        Command::Help => {
            out.write_all(USAGE.as_bytes())?;
            Ok(())
        }
        Command::Serve { cfg, listen } => {
            let forge = Forge::start(cfg).await?;
            tracing::info!("clé de signature : {}", forge.keys.fingerprint());
            let app = crate::app(forge)?;
            let listener = tokio::net::TcpListener::bind(listen)
                .await
                .with_context(|| format!("impossible d'écouter sur {listen}"))?;
            tracing::info!("this-forge écoute sur http://{listen}");
            axum::serve(listener, app).await?;
            Ok(())
        }
        Command::Invite { cfg, uses, note } => {
            let forge = Forge::start(cfg).await?;
            let code = forge.create_invitation(uses, note).await?;
            writeln!(out, "{code}")?;
            Ok(())
        }
    }
}

/// `parse` puis `execute` : tout ce que fait `main`, sauf installer les journaux.
pub async fn run(args: &[String], out: &mut (dyn Write + Send)) -> Result<()> {
    execute(parse(args)?, out).await
}
