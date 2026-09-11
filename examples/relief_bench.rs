//! relief_bench — ce que coûte la publication du relief d'obrain.
//!
//! Le relief est servi en HTTP : il doit donc être **bon marché par requête**.
//! Ce banc mesure les trois chemins qui vivent derrière `GET /graph`, et met
//! en évidence celui qui ne l'est pas :
//!
//! 1. `describe()` sans compteurs — le chemin par défaut de la route ;
//! 2. `describe()` avec compteurs — un **balayage complet** de la zone
//!    simpliciale (obrain ne tient aucun compteur O(1)) ;
//! 3. la bascule d'espace, et la reprise du cache qui la suit.
//!
//! Le graphe de banc est une chaîne de triangles : `n` nœuds, arêtes `(i,i+1)`
//! et `(i,i+2)` — donc `n-2` triangles. On peut ainsi faire varier la taille et
//! regarder ce qui est constant et ce qui est linéaire.
//!
//! ```text
//! cargo run -p this-rs --features obrain --release --example relief_bench
//! cargo run -p this-rs --features obrain --release --example relief_bench -- 8000 20
//! ```
//!
//! `usage: relief_bench [noeuds] [iterations]`

use std::time::Instant;

use obrain_common::types::{NodeId, Value as ObrainValue};
use obrain_substrate::simplex_passes::SimplexPassesSpec;
use this::graph::{Relief, ReliefOptions};
use this::storage::ObrainStore;

/// Mesure le coût moyen d'une opération, après un tour de chauffe.
fn bench<T>(iters: u32, mut op: impl FnMut() -> T) -> f64 {
    let _ = op(); // chauffe : paie les initialisations paresseuses hors mesure
    let start = Instant::now();
    for _ in 0..iters {
        let v = op();
        std::hint::black_box(&v);
    }
    start.elapsed().as_secs_f64() / f64::from(iters)
}

/// Rend une durée lisible, à l'échelle de ce qu'on mesure.
fn dur(secs: f64) -> String {
    if secs < 1e-6 {
        format!("{:>10.0} ns", secs * 1e9)
    } else if secs < 1e-3 {
        format!("{:>10.2} µs", secs * 1e6)
    } else {
        format!("{:>10.2} ms", secs * 1e3)
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let n: usize = args
        .next()
        .and_then(|a| a.parse().ok())
        .unwrap_or(2_000)
        .max(4);
    let iters: u32 = args
        .next()
        .and_then(|a| a.parse().ok())
        .unwrap_or(20)
        .max(1);

    println!("relief_bench — coût de la publication du relief (n={n}, {iters} itérations)");
    println!();

    let store = ObrainStore::new_in_memory();

    // ── Construction du graphe ──────────────────────────────────────────
    // Chemin brut (session obrain) plutôt que les services this-rs : ce banc
    // mesure la LECTURE du relief, pas le coût d'écriture du framework (qui
    // paie un commit WAL par mutation, cf. `weave_bench` pour ce chiffre-là).
    let setup = Instant::now();
    let session = store.db().session();
    let ids: Vec<NodeId> = (0..n)
        .map(|i| {
            session.create_node_with_props(
                &["bench"],
                [("_uuid", ObrainValue::from(format!("bench-{i}")))],
            )
        })
        .collect();
    for i in 0..n.saturating_sub(2) {
        session.create_edge(ids[i], ids[i + 1], "knows");
        session.create_edge(ids[i], ids[i + 2], "knows");
    }
    let setup_secs = setup.elapsed().as_secs_f64();

    // ── Passes simpliciales : une fois, puis plus jamais ────────────────
    let sub = store.db().substrate_handle().expect("substrat");
    sub.configurer_passes_simpliciales(&SimplexPassesSpec {
        clique_edge_type: "knows".into(),
        clique_labels: Vec::new(),
        dispatch: None,
        maintenance: true,
    })?;
    let mat = Instant::now();
    let stats = sub.materialiser_simplexes()?;
    let mat_secs = mat.elapsed().as_secs_f64();
    sub.flush()?;

    println!(
        "graphe     : {n} nœuds, {} arêtes, construit en {setup_secs:.3} s",
        2 * n.saturating_sub(2)
    );
    println!(
        "passes     : {} triangles matérialisés en {mat_secs:.3} s (une seule fois)",
        stats.triangles
    );
    println!();

    // ── 1. Le chemin par défaut de `GET /graph` ─────────────────────────
    let plain = bench(iters, || Relief::describe(&store, ReliefOptions::default()));

    // ── 2. `GET /graph?counts=true` : le balayage ───────────────────────
    let counted = bench(iters, || {
        Relief::describe(
            &store,
            ReliefOptions {
                include_counts: true,
            },
        )
    });

    println!("GET /graph                 {}", dur(plain));
    println!("GET /graph?counts=true     {}", dur(counted));
    println!(
        "  → rapport                ×{:.1}  (le balayage simplex, pas l'introspection)",
        counted / plain
    );
    println!();

    // ── 3. Espaces : bascule, puis reprise du cache ─────────────────────
    store.create_space("bench-a")?;
    store.create_space("bench-b")?;
    let switch = bench(iters * 10, || {
        store.use_graph("bench-a");
        store.use_graph("bench-b");
    });

    // Le graphe du banc vit dans le graphe PAR DÉFAUT : sans ce retour, la
    // reprise mesurerait un espace vide (et rendrait un chiffre flatteur qui
    // ne dit rien).
    store.reset_graph();
    let rebuild = bench(iters, || store.rebuild_index_for_label("bench"));

    println!("use_graph (×2, invalidation)  {}", dur(switch / 2.0));
    println!("rebuild_index_for_label(bench) {}", dur(rebuild));
    println!();

    // ── Ce que le document publie ──────────────────────────────────────
    let relief = Relief::describe(
        &store,
        ReliefOptions {
            include_counts: true,
        },
    );
    let counts = relief.engine.simplex.counts.expect("compteurs");
    println!(
        "document   : {} procédures introspectées, {} familles déclarées, {} pièges",
        relief.procedures.len(),
        relief.families.len(),
        relief.traps.len()
    );
    println!(
        "simplexes  : {} triangles, {} K4, {} K5, {} hyperarêtes",
        counts.triangles, counts.k4, counts.k5, counts.hyperedges
    );
    println!("espace     : {:?}", relief.space);

    Ok(())
}
