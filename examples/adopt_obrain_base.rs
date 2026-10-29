//! adopt_obrain_base — cette base obrain est-elle **adoptable** par this-rs ?
//!
//! # Pourquoi cet outil existe
//!
//! this-rs a deux façons de regarder une base obrain, et elles n'ont pas les
//! mêmes exigences :
//!
//! 1. **Le relief** (`GET /graph`) interroge le *moteur* — registre des
//!    procédures, état du substrat, couche simpliciale, espaces. Il ne présume
//!    **rien** du schéma : il fonctionne donc sur n'importe quelle base, y
//!    compris une base écrite par un autre outil.
//!
//! 2. **Les services de stockage** (`ObrainDataService`, `ObrainLinkService`)
//!    servent les entités de this-rs, et exigent une convention précise :
//!    - `_uuid` sur chaque nœud (l'index `Uuid → NodeId`),
//!    - `_data` = l'entité sérialisée en JSON,
//!    - `f_<champ>` pour les champs indexés (la recherche).
//!
//! Une base qui ne porte pas ces marques — le noyau Linux : nœuds
//! `Function`/`Field` à propriétés plates, arêtes `CALLS` sans aucune
//! propriété — est **lisible en relief mais non adoptable** :
//! `rebuild_index_for_label` insère alors 0 entrée, donc `get`/`list`/
//! `update`/`delete` rendent vide ou « introuvable ».
//!
//! Ce binaire le **mesure** au lieu de le supposer, et montre le contraste avec
//! la lecture native (GQL), qui prouve que la base, elle, va très bien.
//!
//! ```text
//! cargo run -p this-rs --features obrain --release --example adopt_obrain_base -- \
//!     /chemin/vers/base.obrain LABEL [LABEL…]
//! ```
//!
//! `LABEL` = le label des nœuds à tester (`Function` pour un graphe de code,
//! `Page`/`Problem` pour aletheia, `Session`/`Turn` pour un graphe d'agent).

use this::graph::{Relief, ReliefOptions};
use this::storage::ObrainStore;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let Some(path) = args.next() else {
        eprintln!("usage: adopt_obrain_base <store> <label> [label…]");
        std::process::exit(2);
    };
    let labels: Vec<String> = args.collect();
    if labels.is_empty() {
        eprintln!("il faut au moins un label à tester");
        std::process::exit(2);
    }

    // Lecture seule : on ne touche pas à la base qu'on diagnostique.
    let store = ObrainStore::open_read_only(&path)?;

    println!("base       : {path}");
    println!("substrat   : {}", store.is_substrate());
    println!("lecture    : seule (verrou partagé)");
    println!();

    // ── 1. La base, vue nativement (GQL) : elle va bien ──────────────────
    let native = store.db().execute("MATCH (n) RETURN count(n)")?;
    println!(
        "natif GQL  : {} nœud(s) dans la base",
        native
            .rows
            .first()
            .and_then(|r| r.first())
            .map(|v| format!("{v}"))
            .unwrap_or_else(|| "?".into())
    );

    // ── 2. Le relief : aucune présomption de schéma ──────────────────────
    let relief = Relief::describe(&store, ReliefOptions::default());
    println!(
        "relief     : {} procédures introspectées, {} familles ({} disponibles), {} pièges",
        relief.procedures.len(),
        relief.families.len(),
        relief.families.iter().filter(|f| f.available).count(),
        relief.traps.len()
    );
    println!(
        "simplexes  : configured = {} | maintenance = {}",
        relief.engine.simplex.configured, relief.engine.simplex.maintenance
    );
    println!();

    // ── 3. L'adoption par les services de stockage : la convention ───────
    println!("adoption par les services this-rs (convention _uuid / _data / f_*) :");
    let mut total = 0_usize;
    for label in &labels {
        // `rebuild_index_for_label` ne retient que les nœuds portant un `_uuid`
        // analysable : c'est la mesure exacte de l'adoptabilité.
        let entities = store.rebuild_index_for_label(label)?;
        let links = store.rebuild_link_index(label)?;
        total += entities + links;
        println!("  {label:<18} entités indexées : {entities:<7} liens indexés : {links}");
    }
    println!();

    // ── 4. L'identité canonique : la même entité, quel que soit le moteur ─
    //
    // Deux chiffres pour le même label. Le premier est la convention
    // historique (`_uuid`/`_data`) ; le second est l'identité DÉRIVÉE, qui ne
    // présume rien du schéma. Le second chemin est ADDITIF : il ne remplace pas
    // le premier, il le complète.
    use this::core::identity::IdentityRegistry;
    println!("identité canonique (convention PUIS propriétés plates) :");
    let registry = IdentityRegistry::obrain_default();
    let report = store.rebuild_identity_index(&registry);
    println!(
        "  schéma {} · espace {:?} · {} motif(s) · repli {:?}",
        report.scheme,
        report.space,
        report.motives.len(),
        report.fallback
    );
    for lr in &report.labels {
        if !labels.iter().any(|l| l == &lr.label) {
            continue;
        }
        println!(
            "  {:<18} par convention : {:<6} par identité dérivée : {:<6} ignorés : {} réservés : {} incohérents : {}",
            lr.label,
            lr.by_convention,
            lr.by_derivation,
            lr.skipped,
            lr.skipped_reserved,
            lr.malformed.len()
        );
    }
    if let Some(err) = &report.labels_error {
        println!("  note : {err}");
    }
    println!(
        "  TOTAL résolu : {} (convention) + {} (dérivée) = {}",
        report.labels.iter().map(|l| l.by_convention).sum::<usize>(),
        report.labels.iter().map(|l| l.by_derivation).sum::<usize>(),
        report.total_resolved
    );
    println!();

    let derived = report.labels.iter().map(|l| l.by_derivation).sum::<usize>();
    let convention = report.labels.iter().map(|l| l.by_convention).sum::<usize>();

    if total == 0 && derived > 0 {
        println!(
            "VERDICT    : base NON adoptable par la CONVENTION, mais adoptable par\n\
             \x20            IDENTITÉ DÉRIVÉE ({derived} entités). C'est exactement la\n\
             \x20            promesse « la même entité quel que soit le moteur » : le chemin\n\
             \x20            neuf est additif, il ne remplace rien."
        );
    } else if total == 0 {
        println!(
            "VERDICT    : base NON adoptable par les services de stockage, et aucune\n\
             \x20            identité dérivée (les nœuds n'ont ni `_uuid` ni nom naturel)."
        );
    } else {
        println!(
            "VERDICT    : base adoptable — {convention} entité(s) par convention,\n\
             \x20            {derived} par identité dérivée."
        );
    }

    Ok(())
}
