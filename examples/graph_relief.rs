//! graph_relief — publier le relief d'obrain sur REST.
//!
//! Le plus petit serveur qui montre la propriété intéressante : **this-rs ne
//! tient aucune liste du relief**. Il attache un store, et le document se
//! décrit tout seul depuis le registre du moteur.
//!
//! ```text
//! cargo run -p this-rs --features obrain --example graph_relief
//! curl -s localhost:8123/graph | jq '.procedures | length'
//! curl -s localhost:8123/graph/procedures | jq '.families[] | select(.available==false)'
//! curl -s "localhost:8123/graph?counts=true" | jq '.engine.simplex'
//! ```
//!
//! # Servir un VRAI store, sans le muter
//!
//! ```text
//! cargo run -p this-rs --features obrain --release --example graph_relief -- \
//!     127.0.0.1:8124 --store /chemin/vers/store.obrain --read-only
//! ```
//!
//! `--read-only` ouvre le store par `ObrainDB::open_read_only` : mutations
//! refusées au niveau moteur, verrou partagé — on peut donc publier le relief
//! d'une base qu'un autre processus détient. Sans store, l'exemple tourne sur
//! un store en mémoire (rien à installer, rien à nettoyer).

use std::path::PathBuf;

use this::server::ServerBuilder;
use this::storage::{ObrainLinkService, ObrainStore};

struct Args {
    addr: String,
    store: Option<PathBuf>,
    read_only: bool,
}

fn parse_args() -> Args {
    let mut args = Args {
        addr: "127.0.0.1:8123".to_string(),
        store: None,
        read_only: false,
    };
    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        match a.as_str() {
            "--store" => args.store = it.next().map(PathBuf::from),
            "--read-only" => args.read_only = true,
            "-h" | "--help" => {
                println!("usage: graph_relief [ADDR] [--store PATH] [--read-only]");
                std::process::exit(0);
            }
            other => args.addr = other.to_string(),
        }
    }
    args
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = parse_args();
    let addr = args.addr;

    // Aucune écriture sur un store réel : l'espace de démonstration n'est créé
    // que sur le store en mémoire.
    let store = match &args.store {
        Some(path) if args.read_only => ObrainStore::open_read_only(path)?,
        Some(path) => ObrainStore::open(path)?,
        None => ObrainStore::new_in_memory(),
    };

    if args.store.is_none() {
        match store.create_space("demo") {
            Ok(created) => tracing::info!("espace `demo` créé: {created}"),
            Err(e) => tracing::warn!("création de l'espace `demo` impossible: {e}"),
        }
        store.use_graph("demo");
    }

    let links = ObrainLinkService::new(store.clone());

    match &args.store {
        Some(path) => println!("relief de {} sur http://{addr}", path.display()),
        None => println!("relief d'obrain (store en mémoire) sur http://{addr}"),
    }
    println!("  GET /graph              le document complet");
    println!("  GET /graph?counts=true  + compteurs de simplexes (balayage complet)");
    println!("  GET /graph/procedures   la surface d'appel");
    println!("  GET /graph/spaces       l'espace actif et les espaces connus");
    println!("  GET /health             liveness");

    ServerBuilder::new()
        .with_link_service(links)
        .with_graph_store(store)
        .serve(&addr)
        .await
}
