//! `this-forge serve | invite`.
//!
//!   this-forge serve  [--listen 127.0.0.1:8480] [--data-dir ./forge-data]
//!                     [--signing-key <chemin>] [--image this-forge-builder:1.93]
//!                     [--cpus 6] [--memory 4g] [--cpu-shares 1024] [--blkio-weight 500]
//!   this-forge invite [--data-dir ./forge-data] [--uses 1] [--note "…"]

use std::net::SocketAddr;

use this_forge::{Forge, ForgeConfig};

fn arg(args: &[String], name: &str) -> Option<String> {
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1).cloned())
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();
    let args: Vec<String> = std::env::args().skip(1).collect();
    let cmd = args.first().map(String::as_str).unwrap_or("serve");
    let mut cfg =
        ForgeConfig::new(arg(&args, "--data-dir").unwrap_or_else(|| "./forge-data".into()));
    if let Some(k) = arg(&args, "--signing-key") {
        cfg.signing_key_path = k.into();
    }
    if let Some(i) = arg(&args, "--image") {
        cfg.image = i;
    }
    if let Some(c) = arg(&args, "--cpus") {
        cfg.cpus = c.parse()?;
    }
    if let Some(m) = arg(&args, "--memory") {
        cfg.memory = m;
    }
    if let Some(c) = arg(&args, "--cpu-shares") {
        cfg.cpu_shares = c.parse()?;
    }
    if let Some(c) = arg(&args, "--blkio-weight") {
        cfg.blkio_weight = c.parse()?;
    }
    match cmd {
        "serve" => {
            let listen: SocketAddr = arg(&args, "--listen")
                .unwrap_or_else(|| "127.0.0.1:8480".into())
                .parse()?;
            let forge = Forge::start(cfg).await?;
            tracing::info!("clé de signature : {}", forge.keys.fingerprint());
            let app = this_forge::app(forge)?;
            let listener = tokio::net::TcpListener::bind(listen).await?;
            tracing::info!("this-forge écoute sur http://{listen}");
            axum::serve(listener, app).await?;
            Ok(())
        }
        "invite" => {
            let uses: i64 = arg(&args, "--uses")
                .map(|u| u.parse())
                .transpose()?
                .unwrap_or(1);
            let forge = Forge::start(cfg).await?;
            let code = forge.create_invitation(uses, arg(&args, "--note")).await?;
            println!("{code}");
            Ok(())
        }
        other => anyhow::bail!("commande inconnue `{other}` : serve | invite"),
    }
}
