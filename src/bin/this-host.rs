//! # `this-host` — l'hôte de déploiement
//!
//! Une structure qu'on déploie, sur laquelle on rattache des projets qui
//! existent déjà : une base, une identité, un régime d'autorisation — déclarés
//! dans un fichier, jamais devinés.
//!
//! ```text
//! this-host --config deploy.yaml --dry-run   # le plan, sans ouvrir aucune base
//! this-host --config deploy.yaml             # la structure, en service
//! curl -s localhost:8080/ | jq               # le plan publié par la structure
//! curl -s localhost:8080/healthz
//! curl -s localhost:8080/attached/facturation/graph | jq '.engine'
//! ```
//!
//! Le plan publié par `GET /` dit **ce qui est rattaché et ce qui ne l'est
//! pas** — et il ne contient aucun secret. `--dry-run` rend le même document
//! sans ouvrir une seule base : c'est ce qu'un opérateur relit avant de
//! déployer.

use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use this::deploy::{DeploymentConfig, Plan};

struct Args {
    config: Option<PathBuf>,
    dry_run: bool,
    json: bool,
}

const USAGE: &str = "\
usage: this-host --config <deploy.yaml> [--dry-run] [--json]

  --config <chemin>  le fichier de déploiement (requis)
  --dry-run          publier le plan sans ouvrir aucune base, puis sortir
  --json             avec --dry-run : n'imprimer que le plan (JSON)
  -h, --help         cette aide
";

fn parse_args() -> Result<Args> {
    let mut args = Args {
        config: None,
        dry_run: false,
        json: false,
    };
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--config" | "-c" => {
                args.config = Some(PathBuf::from(
                    it.next()
                        .context("`--config` attend un chemin — ex. `--config deploy.yaml`")?,
                ));
            }
            "--dry-run" => args.dry_run = true,
            "--json" => args.json = true,
            "-h" | "--help" => {
                println!("{USAGE}");
                std::process::exit(0);
            }
            other => bail!("argument inconnu : `{other}`\n\n{USAGE}"),
        }
    }
    if args.config.is_none() {
        bail!("aucun fichier de déploiement : `--config <deploy.yaml>` est requis\n\n{USAGE}");
    }
    Ok(args)
}

fn main() -> Result<()> {
    let args = parse_args()?;
    let path = args
        .config
        .clone()
        .expect("`--config` vérifié par parse_args");

    // La validation lit la déclaration **entière** avant toute E/S de base :
    // une faute de frappe se voit ici, pas à la première requête.
    let config = DeploymentConfig::from_file(&path)?;

    if args.dry_run {
        let plan = config.plan();
        if args.json {
            println!("{}", serde_json::to_string_pretty(&plan)?);
        } else {
            print_plan(&plan);
            println!(
                "\n--dry-run : aucune base n'a été ouverte. Le même document est publié par \
                 `GET /` une fois la structure en service."
            );
        }
        return Ok(());
    }

    let host = config.build()?;
    print_plan(&host.plan);

    let bind = config.bind.clone();
    let base = format!("http://{bind}");

    let runtime = tokio::runtime::Runtime::new().context("runtime tokio")?;
    runtime.block_on(async move {
        // L'écoute est prise **dans** le runtime : convertir un socket std hors
        // du runtime panique (« there is no reactor running »).
        let listener = tokio::net::TcpListener::bind(&bind)
            .await
            .with_context(|| format!("écoute sur {bind} impossible"))?;

        println!("\nstructure en service sur {base}");
        println!("  GET /            le plan");
        println!("  GET /healthz     la sonde de vie");
        for attachment in &host.plan.attachments {
            println!(
                "  {} → {} ({}{})",
                attachment.mount,
                attachment.store.kind,
                attachment
                    .store
                    .path
                    .clone()
                    .unwrap_or_else(|| "en mémoire".to_string()),
                if attachment.store.read_only {
                    ", lecture seule"
                } else {
                    ""
                }
            );
        }

        axum::serve(listener, host.router)
            .with_graceful_shutdown(async {
                let _ = tokio::signal::ctrl_c().await;
                println!("\narrêt demandé — la structure se ferme");
            })
            .await
            .context("service HTTP")
    })
}

/// Imprime le plan tel qu'un opérateur le relit.
fn print_plan(plan: &Plan) {
    println!(
        "structure `{}` v{} — {}{}",
        plan.structure,
        plan.version,
        plan.bind,
        if plan.inspected {
            " (bases ouvertes)"
        } else {
            " (déclaration seule)"
        }
    );
    for attachment in &plan.attachments {
        println!(
            "\n  {} → {} ({})",
            attachment.mount, attachment.name, attachment.store.kind
        );
        if let Some(path) = &attachment.store.path {
            println!(
                "    base     : {path}{}",
                if attachment.store.read_only {
                    " [lecture seule]"
                } else {
                    " [ÉCRITURE]"
                }
            );
        }
        println!(
            "    droits   : {}{}",
            attachment.authorization.mode,
            attachment
                .authorization
                .policies
                .as_ref()
                .map(|p| format!(" (politiques : {p})"))
                .unwrap_or_default()
        );
        println!(
            "    identité : {}",
            match &attachment.identity {
                this::deploy::IdentityPlan::Native =>
                    "native (User/Owner → user_id, tenant_id)".to_string(),
                this::deploy::IdentityPlan::Mapped { principal, scope } => {
                    format!("{principal} / {scope}")
                }
            }
        );
        if let Some(exposure) = &attachment.exposure {
            println!(
                "    exposition : CORS {} ({} origines) · domaines {} ({} hôtes) · caddy {}",
                if exposure.cors_enabled {
                    "posé"
                } else {
                    "aucun"
                },
                exposure.cors_origins,
                if exposure.domains_guarded {
                    "gardés"
                } else {
                    "ouverts"
                },
                exposure.hosts_allowed,
                if exposure.caddy_delegated {
                    "délégué"
                } else {
                    "non délégué"
                },
            );
        }
        for warning in &attachment.warnings {
            println!("    ⚠ {warning}");
        }
    }
    if !plan.not_attached.is_empty() {
        println!("\n  ce que la structure ne rattache pas :");
        for item in &plan.not_attached {
            println!("    · {item}");
        }
    }
    if !plan.notes.is_empty() {
        println!("\n  propriétés de la structure :");
        for note in &plan.notes {
            println!("    · {note}");
        }
    }
}
