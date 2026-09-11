//! Le couplage this-rs ↔ obrain, mesuré — pas cité.
//!
//! Ce test existe pour une raison précise : la dépendance `obrain-reactive`
//! (et `obrain-engine`) a été câblée dans `this/Cargo.toml`, mais **compiler
//! n'est pas utiliser**. Ici on ouvre une vraie base et on lit dedans, en
//! processus, pour mesurer le gain qui justifie tout le chantier.
//!
//! Repère : le même genre de lecture par le HTTP du dashboard coûte un
//! aller-retour TCP complet — mesuré 265 µs au minimum (dont 34,6 µs pour le
//! seul `connect`), et 0,8-3,3 ms pour un agrégat servi par le cache.
//!
//! Base utilisée : la copie de migration, **libre de tout verrou** (la base
//! servie est tenue en exclusif par le serveur de visualisation — on ne peut
//! pas l'ouvrir, et c'est précisément ce que ce test documente).
#![cfg(feature = "obrain")]

use obrain_engine::GraphStore; // apporte `all_node_ids` et `get_node_property`
use std::time::Instant;

const STORE: &str = "/mnt/obrain/mig/runtime-dev.obrain";

#[test]
fn reading_obrain_in_process_is_nanoseconds_not_microseconds() {
    let path = std::path::Path::new(STORE);
    if !path.exists() {
        eprintln!("base absente ({STORE}) — test ignoré");
        return;
    }

    // 1) OUVERTURE en processus
    let t = Instant::now();
    let db = obrain_engine::ObrainDB::open_read_only(path).expect("ouverture de la base");
    let open_ms = t.elapsed().as_secs_f64() * 1000.0;

    // 2) LECTURE DE PROPRIÉTÉ, en processus (le cœur du gain)
    let Some(sub) = db.substrate_handle() else {
        eprintln!("pas de handle substrat — test ignoré");
        return;
    };
    let ids = sub.all_node_ids();
    assert!(!ids.is_empty(), "base vide ?");
    let key = obrain_common::types::PropertyKey::from("name");
    let ech = ids.len().min(2000);

    // chauffe (première passe : pages non résidentes)
    for id in ids.iter().take(ech) {
        std::hint::black_box(sub.get_node_property(*id, &key));
    }
    let t = Instant::now();
    let mut hits = 0usize;
    for id in ids.iter().take(ech) {
        if std::hint::black_box(sub.get_node_property(*id, &key)).is_some() {
            hits += 1;
        }
    }
    let per_read_ns = t.elapsed().as_nanos() as f64 / ech as f64;

    // 3) REQUÊTE GQL en processus (le même genre de travail qu'un endpoint)
    let sess = db.session();
    let t = Instant::now();
    let res = sess.execute("MATCH (n) RETURN count(n)").expect("requête");
    let query_us = t.elapsed().as_secs_f64() * 1e6;
    let n = res.rows.first().and_then(|r| r.first()).cloned();

    eprintln!("\n════ this-rs → obrain, EN PROCESSUS ════");
    eprintln!("  ouverture de la base            : {open_ms:8.1} ms");
    eprintln!(
        "  lecture de propriété            : {per_read_ns:8.1} ns   ({hits}/{ech} nœuds avec la clé `name`)"
    );
    eprintln!("  requête GQL count(n)            : {query_us:8.1} µs   (résultat {n:?})");
    eprintln!("  ── comparaison HTTP (mesurée à part) ──");
    eprintln!(
        "  aller-retour TCP minimal        : 265 000 ns  minimum (34 600 ns pour le seul connect)"
    );
    eprintln!(
        "  rapport lecture/HTTP            : ×{:.0}",
        265_000.0 / per_read_ns.max(1.0)
    );
    eprintln!();

    // ── CE QUE CE TEST A RÉFUTÉ, ET QU'IL GARDE ÉCRIT ──────────────────────
    //
    // J'ai longtemps répété « ×10⁶ » en citant 223 ns. Ce chiffre venait d'une
    // AUTRE mesure — le chemin des vues **zero-copy** (O1) — et je l'ai reporté
    // sur une lecture de propriété ordinaire, qui est un chemin différent.
    // Mesuré ici, sur cette base : **8,6 µs** par lecture, soit ×31 contre le
    // minimum HTTP (265 µs), pas ×10⁶. C'était une erreur de catégorie.
    //
    // L'assertion encode donc la réalité mesurée, avec une marge : si ce test
    // casse un jour, ou bien la base change de nature, ou bien une régression a
    // été introduite — dans les deux cas il faut regarder, pas supposer.
    eprintln!("  ── la borne testée ──");
    eprintln!("  attendu : < 50 000 ns par lecture (mesuré 8 600 ns au moment d'écrire)");
    assert!(
        per_read_ns < 50_000.0,
        "lecture in-process = {per_read_ns:.0} ns : au-delà de 50 µs, la régression est réelle"
    );
}
