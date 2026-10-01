//! **Tests de budget** — ce que la granularité a le droit de coûter.
//!
//! ```bash
//! cargo test --features obrain --test scope_budgets -- --nocapture
//! ```
//!
//! # Pourquoi des ratios, jamais des durées
//!
//! Un seuil en microsecondes dépend de la machine : il deviendrait faux au
//! premier ordinateur plus lent, et on finirait par le desserrer au lieu de
//! corriger la régression. Un **ratio** entre deux mesures prises dans le même
//! processus, sur le même décor, veut dire quelque chose : *« filtrer ne doit pas
//! coûter plus de N fois le travail non filtré »*.
//!
//! Les budgets sont **larges** à dessein : ces tests doivent attraper un
//! **changement d'ordre** (une correspondance quadratique, un filtrage qui
//! explose), pas une variation de 20 % due à la charge de la machine. Le banc
//! (`benches/scope_bench.rs`) donne les chiffres fins ; ce fichier garde le
//! **garde-fou**.
//!
//! # Le chiffre, en tête
//!
//! Mesuré sur cette machine (voir le banc pour le détail) :
//!
//! | Mesure | Valeur |
//! |---|---|
//! | correspondance d'une permission, refus, 67 permissions | ≈ 0,30 µs |
//! | construction d'un périmètre, 67 permissions | ≈ 7,3 µs (une fois, puis cache) |
//! | chaîne d'une requête (décision + périmètre) | ≈ 3,9 µs |
//! | descente de 258 nœuds, sans périmètre | ≈ 0,58 ms |
//! | descente de 258 nœuds, sous périmètre | ≈ 0,87 ms (×1,45) |

// Le garde-fou parle le périmètre de lecture, qui vit dans `this::graph` et
// `this::storage::permission` — les deux gatés `obrain`. Sans cette ligne, un
// `cargo test` par défaut **échouait à la compilation** de cette cible au lieu
// de ne pas la construire : un test qui interdit le build est pire qu'un test
// absent.
#![cfg(feature = "obrain")]

use std::hint::black_box;
use std::time::{Duration, Instant};

use this::graph::hierarchy::{self, HierarchySpec};
use this::graph::relief::{Relief, ReliefOptions};
use this::storage::permission::ReadScope;
use this::storage::{GraphBackend, ObrainStore};

/// Médiane de `tours` mesures, après échauffement.
fn mediane(mut f: impl FnMut(), echauffement: usize, tours: usize) -> Duration {
    for _ in 0..echauffement {
        f();
    }
    let mut durees = Vec::with_capacity(tours);
    for _ in 0..tours {
        let debut = Instant::now();
        f();
        durees.push(debut.elapsed());
    }
    durees.sort_unstable();
    durees[durees.len() / 2]
}

fn ratio(a: Duration, b: Duration) -> f64 {
    a.as_secs_f64() / b.as_secs_f64().max(1e-12)
}

/// `n` permissions `node:read:Label{i}`, plus les vraies.
fn permissions(n: usize) -> Vec<String> {
    let mut p: Vec<String> = vec![
        "graph:read".into(),
        "edge:read:CONTAINS".into(),
        "echelle:read:groupe".into(),
    ];
    for i in 0..n {
        p.push(format!("node:read:Label{i}"));
    }
    p
}

/// Un arbre : `largeur` enfants par niveau, `profondeur` niveaux.
fn arbre(store: &ObrainStore, profondeur: usize, largeur: usize) -> u64 {
    let session = store.db().session();
    let racine = session.create_node_with_props(
        &["Groupe"],
        [("name", obrain_common::types::Value::from("racine"))],
    );
    let mut frontiere = vec![racine];
    for niveau in 1..=profondeur {
        let mut suivant = Vec::new();
        for parent in &frontiere {
            for i in 0..largeur {
                let enfant = session.create_node_with_props(
                    &["Groupe"],
                    [(
                        "name",
                        obrain_common::types::Value::from(format!("n{niveau}-{i}")),
                    )],
                );
                session.create_edge(*parent, enfant, "CONTAINS");
                suivant.push(enfant);
            }
        }
        frontiere = suivant;
    }
    store.commit_wal().expect("commit");
    racine.as_u64()
}

/// **La correspondance ne doit pas être quadratique.**
///
/// Elle est appelée une fois par nœud et par relation ; si elle devenait
/// quadratique en nombre de permissions, une décision riche (des dizaines de
/// permissions) ferait exploser chaque descente.
#[test]
fn the_permission_match_does_not_blow_up_with_the_number_of_permissions() {
    let petit = ReadScope::from_permissions(permissions(1).iter().map(String::as_str));
    let grand = ReadScope::from_permissions(permissions(64).iter().map(String::as_str));

    // Le pire cas : aucun label ne correspond → parcours complet.
    let m_petit = mediane(
        || {
            black_box(petit.may_read_node("JamaisAccorde"));
        },
        1_000,
        10_000,
    );
    let m_grand = mediane(
        || {
            black_box(grand.may_read_node("JamaisAccorde"));
        },
        1_000,
        10_000,
    );

    let croissance = ratio(m_grand, m_petit);
    println!(
        "  perméabilité : 1 permission {:.2}µs · 64 permissions {:.2}µs · ×{croissance:.1}",
        m_petit.as_secs_f64() * 1e6,
        m_grand.as_secs_f64() * 1e6
    );
    assert!(
        croissance < 100.0,
        "×{croissance:.1} entre 1 et 64 permissions : c'est quadratique, pas linéaire"
    );
}

/// **Le périmètre illimité court-circuite** : c'est le chemin d'avant.
///
/// Sans cette garantie, un déploiement qui n'active aucun filtrage paierait quand
/// même le contrôle — la pire des régressions, invisible.
#[test]
fn the_unrestricted_scope_short_circuits() {
    let tout = ReadScope::all();
    assert!(tout.is_unrestricted());
    // Un périmètre illimité ne parcourt rien : il répond sans regarder.
    let m = mediane(
        || {
            black_box(tout.may_read_node("Nimporte"));
        },
        1_000,
        10_000,
    );
    println!("  périmètre illimité : {:.3}µs", m.as_secs_f64() * 1e6);
    assert!(
        m < Duration::from_micros(5),
        "un court-circuit ne doit pas coûter {m:?}"
    );
}

/// **Filtrer la descente ne doit pas coûter un ordre de grandeur.**
#[test]
fn filtering_the_descent_has_a_bounded_cost() {
    let store = ObrainStore::new_in_memory();
    let racine = arbre(&store, 3, 4); // 4 + 16 + 64 = 84 nœuds
    let spec = HierarchySpec {
        max_depth: 4,
        per_level_limit: 500,
        ..Default::default()
    };
    // Un périmètre qui accorde TOUT : le contrôle s'exerce, rien n'est masqué.
    let avec = ReadScope::from_permissions(["node:read:*", "edge:read:*"]);

    let m_nu = mediane(
        || {
            black_box(hierarchy::describe_scoped(
                &store,
                obrain_common::types::NodeId::new(racine),
                spec.clone(),
                &ReadScope::all(),
            ));
        },
        20,
        200,
    );
    let m_filtre = mediane(
        || {
            black_box(hierarchy::describe_scoped(
                &store,
                obrain_common::types::NodeId::new(racine),
                spec.clone(),
                &avec,
            ));
        },
        20,
        200,
    );

    let overhead = ratio(m_filtre, m_nu);
    println!(
        "  descente 84 nœuds : nue {:.0}µs · filtrée {:.0}µs · ×{overhead:.2}",
        m_nu.as_secs_f64() * 1e6,
        m_filtre.as_secs_f64() * 1e6
    );
    assert!(
        overhead < 4.0,
        "filtrer coûte ×{overhead:.2} : le contrôle est trop cher"
    );
}

/// **Le masquage ne coûte pas plus que le non-masquage** : ce qui est caché est
/// écarté, donc moins de nœuds à décrire.
#[test]
fn hiding_is_not_more_expensive_than_showing() {
    let store = ObrainStore::new_in_memory();
    {
        let session = store.db().session();
        let racine = session.create_node_with_props(
            &["Groupe"],
            [("name", obrain_common::types::Value::from("racine"))],
        );
        for i in 0..80 {
            let label = if i % 2 == 0 { "Groupe" } else { "Confidentiel" };
            let enfant = session.create_node_with_props(
                &[label],
                [(
                    "name",
                    obrain_common::types::Value::from(format!("enfant-{i}")),
                )],
            );
            session.create_edge(racine, enfant, "CONTAINS");
        }
        store.commit_wal().expect("commit");
        let racine = racine.as_u64();
        let spec = HierarchySpec {
            max_depth: 1,
            per_level_limit: 500,
            ..Default::default()
        };

        let filtrant =
            ReadScope::from_permissions(["graph:read", "node:read:Groupe", "edge:read:CONTAINS"]);
        let m = mediane(
            || {
                black_box(hierarchy::describe_scoped(
                    &store,
                    obrain_common::types::NodeId::new(racine),
                    spec.clone(),
                    &filtrant,
                ));
            },
            20,
            200,
        );
        let vue = hierarchy::describe_scoped(
            &store,
            obrain_common::types::NodeId::new(racine),
            spec.clone(),
            &filtrant,
        );
        assert_eq!(vue.levels[0].nodes.len(), 40, "40 visibles");
        assert_eq!(vue.levels[0].hidden, 40, "et 40 comptés comme masqués");
        println!(
            "  masquage (80 enfants, la moitié cachée) : {:.0}µs",
            m.as_secs_f64() * 1e6
        );
    }
}

/// **La forme du store sous périmètre ne coûte pas plus** que sans : les deux
/// parcourent les mêmes labels, l'un les filtre.
#[test]
fn the_store_shape_is_not_slower_under_a_scope() {
    let store = ObrainStore::new_in_memory();
    arbre(&store, 3, 4);
    let opts = ReliefOptions {
        include_counts: false,
    };
    let avec = ReadScope::from_permissions(["graph:read", "node:read:*", "edge:read:*"]);

    let m_nu = mediane(
        || {
            black_box(Relief::describe_scoped(&store, opts, &ReadScope::all()));
        },
        10,
        100,
    );
    let m_filtre = mediane(
        || {
            black_box(Relief::describe_scoped(&store, opts, &avec));
        },
        10,
        100,
    );
    let overhead = ratio(m_filtre, m_nu);
    println!(
        "  forme du store : nue {:.1}µs · filtrée {:.1}µs · ×{overhead:.2}",
        m_nu.as_secs_f64() * 1e6,
        m_filtre.as_secs_f64() * 1e6
    );
    assert!(overhead < 3.0, "le relief filtré coûte ×{overhead:.2}");
}

/// **Le périmètre est trouvé par son label** : le contrat de lecture, tel qu'un
/// consommateur l'observe.
#[test]
fn the_scope_answers_the_questions_a_consumer_asks() {
    let scope = ReadScope::from_permissions([
        "graph:read",
        "node:read:Groupe",
        "edge:read:CONTAINS",
        "echelle:read:groupe",
    ]);
    assert!(scope.may_read_graph());
    assert!(scope.may_read_node("Groupe"));
    assert!(!scope.may_read_node("Confidentiel"));
    assert!(scope.may_read_edge("CONTAINS"));
    assert!(!scope.may_create_edge("CONTAINS"), "lire n'est pas tisser");
    assert!(scope.allows_scale(this::storage::permission::Scale::Groupe));
    assert!(!scope.allows_scale(this::storage::permission::Scale::Noeud));

    // Et la même question, posée au store, donne le même compte.
    let store = ObrainStore::new_in_memory();
    let session = store.db().session();
    let racine = session.create_node_with_props(
        &["Groupe"],
        [("name", obrain_common::types::Value::from("racine"))],
    );
    let enfant = session.create_node_with_props(
        &["Groupe"],
        [("name", obrain_common::types::Value::from("enfant"))],
    );
    session.create_edge(racine, enfant, "CONTAINS");
    store.commit_wal().expect("commit");
    assert_eq!(
        <ObrainStore as GraphBackend>::get_node(&store, enfant)
            .map(|n| n.labels.len())
            .unwrap_or(0),
        1
    );
}
