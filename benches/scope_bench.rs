//! **Bancs d'essai des droits granulaires** — sans dépendance.
//!
//! ```bash
//! cargo bench --features obrain --bench scope_bench
//! cargo bench --features obrain --bench scope_bench -- --verifier
//! ```
//!
//! # Pourquoi ce fichier existe
//!
//! Les droits granulaires s'exercent **par nœud** et **par relation** : chaque
//! lecture de graphe demande « ai-je le droit de lire ce label », une fois par
//! enfant parcouru. C'est donc un chemin chaud, et un chemin chaud non mesuré est
//! une opinion.
//!
//! Ce banc mesure quatre choses, et rien d'autre :
//!
//! 1. **la correspondance d'une permission** (`may_read_node`), selon leur nombre ;
//! 2. **la construction d'un périmètre** depuis une décision IAM (par requête) ;
//! 3. **la descente multi-échelle** : sans périmètre, et sous périmètre ;
//! 4. **la forme du store** : sans périmètre, et sous périmètre.
//!
//! Les deux derniers comparent **le même travail** avec et sans filtrage : c'est
//! la seule façon de savoir ce que la granularité **coûte**, et de vérifier
//! qu'elle ne coûte **rien** quand elle ne s'applique pas (le chemin d'avant doit
//! rester intact).
//!
//! # Pas de `criterion`, et pourquoi
//!
//! Le dépôt construit hors ligne, sans dépendance de mesure : `criterion`
//! demanderait un téléchargement, et ses intervalles de confiance ne changeraient
//! pas la décision à prendre ici (un ordre de grandeur suffit à voir un `O(n)` sur
//! une correspondance qui devrait être directe). Le harnais est donc explicite :
//! échauffement, N tours, médiane, minimum — et le nombre de tours est publié.

use std::hint::black_box;
use std::sync::Arc;
use std::time::{Duration, Instant};

use this::graph::canvas::SimpleCanvasState;
use this::graph::hierarchy::{self, HierarchySpec};
use this::graph::relief::{Relief, ReliefOptions};
use this::graph::routes::ScopeResolver;
use this::graph::scope::IamScopeResolver;
use this::storage::iam::RoleGrant;
use this::storage::permission::{ReadScope, Scale};
use this::storage::{ObrainStore, permission};

// ===========================================================================
// Le harnais
// ===========================================================================

/// Le résultat d'une mesure : médiane, minimum, et le nombre de tours.
#[derive(Debug, Clone, Copy)]
struct Mesure {
    mediane: Duration,
    minimum: Duration,
    /// Nombre de tours : publié avec la mesure, parce qu'une médiane sans son
    /// nombre de tours ne dit pas si elle est stable.
    tours: usize,
}

impl Mesure {
    /// La médiane en microsecondes — l'unité du rapport.
    fn microsecondes(&self) -> f64 {
        self.mediane.as_secs_f64() * 1e6
    }
}

/// Mesure `f`, `tours` fois, après échauffement.
///
/// La médiane plutôt que la moyenne : un ordonnanceur qui reprend la main une
/// fois ne doit pas décider de la conclusion. Le minimum est publié avec elle —
/// l'écart entre les deux dit la stabilité de la machine.
fn mesurer(mut f: impl FnMut(), echauffement: usize, tours: usize) -> Mesure {
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
    Mesure {
        mediane: durees[durees.len() / 2],
        minimum: durees[0],
        tours,
    }
}

/// Une ligne du rapport.
struct Ligne {
    nom: String,
    mesure: Mesure,
    note: String,
}

fn ligne(nom: impl Into<String>, mesure: Mesure, note: impl Into<String>) -> Ligne {
    Ligne {
        nom: nom.into(),
        mesure,
        note: note.into(),
    }
}

fn rapporter(titre: &str, lignes: &[Ligne]) {
    println!(
        "\n── {titre} {}",
        "─".repeat(66usize.saturating_sub(titre.len()))
    );
    println!(
        "  {:<44} {:>10} {:>10}  note",
        "mesure", "médiane", "minimum"
    );
    for l in lignes {
        println!(
            "  {:<44} {:>9.2}µs {:>9.2}µs  {}",
            l.nom,
            l.mesure.microsecondes(),
            l.mesure.minimum.as_secs_f64() * 1e6,
            l.note
        );
    }
    // Le nombre de tours, publié : une médiane sans son nombre de tours ne dit
    // pas si elle est stable.
    if let Some(premiere) = lignes.first() {
        println!(
            "  ({} tours par mesure, après échauffement)",
            premiere.mesure.tours
        );
    }
}

// ===========================================================================
// Les décors
// ===========================================================================

/// `n` permissions de la forme `node:read:L{i}`, plus les vraies.
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
        [
            ("name", obrain_common::types::Value::from("racine")),
            (
                "qualified_name",
                obrain_common::types::Value::from("racine"),
            ),
        ],
    );
    let mut frontiere = vec![racine];
    for niveau in 1..=profondeur {
        let mut suivant = Vec::new();
        for parent in &frontiere {
            for i in 0..largeur {
                let enfant = session.create_node_with_props(
                    &["Groupe"],
                    [
                        (
                            "name",
                            obrain_common::types::Value::from(format!("n{niveau}-{i}")),
                        ),
                        (
                            "qualified_name",
                            obrain_common::types::Value::from(format!("racine/n{niveau}-{i}")),
                        ),
                    ],
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

// ===========================================================================
// 1. La correspondance d'une permission
// ===========================================================================

fn banc_correspondance() {
    let mut lignes = Vec::new();

    for n in [1usize, 8, 64] {
        let perms = permissions(n);
        let scope = ReadScope::from_permissions(perms.iter().map(String::as_str));
        let tours = 20_000;

        // Le cas favorable : le premier label est accordé.
        let m = mesurer(
            || {
                black_box(scope.may_read_node("Label0"));
            },
            2_000,
            tours,
        );
        lignes.push(ligne(
            format!("may_read_node — {} permission(s), label accordé", n + 3),
            m,
            "trouvé tôt : une seule comparaison",
        ));

        // Le cas défavorable : aucun label ne correspond — c'est le pire cas
        // d'une correspondance linéaire, et il est atteint à chaque nœud masqué.
        let m = mesurer(
            || {
                black_box(scope.may_read_node("JamaisAccorde"));
            },
            2_000,
            tours,
        );
        lignes.push(ligne(
            format!("may_read_node — {} permission(s), label refusé", n + 3),
            m,
            "parcours complet : le coût du refus",
        ));
    }

    // Le périmètre illimité : il doit court-circuiter, pas parcourir.
    let tout = ReadScope::all();
    let m = mesurer(
        || {
            black_box(tout.may_read_node("Nimporte"));
        },
        2_000,
        20_000,
    );
    lignes.push(ligne(
        "may_read_node — périmètre illimité",
        m,
        "court-circuit : c'est le chemin d'AVANT",
    ));

    rapporter("1. Correspondance d'une permission", &lignes);
}

// ===========================================================================
// 2. La construction d'un périmètre (par requête)
// ===========================================================================

fn banc_construction() {
    let mut lignes = Vec::new();
    for n in [1usize, 8, 64] {
        let perms = permissions(n);
        let m = mesurer(
            || {
                black_box(ReadScope::from_permissions(
                    perms.iter().map(String::as_str),
                ));
            },
            2_000,
            5_000,
        );
        lignes.push(ligne(
            format!("ReadScope::from_permissions — {} permission(s)", n + 3),
            m,
            "une fois par requête : analyse + allocation",
        ));
    }

    // Le coût de l'ANALYSE seule, pour situer l'allocation.
    let bruts = permissions(8);
    let m = mesurer(
        || {
            for p in &bruts {
                black_box(permission::Permission::parse(p));
            }
        },
        2_000,
        5_000,
    );
    lignes.push(ligne(
        "Permission::parse × 11",
        m,
        "l'analyse, sans la construction",
    ));

    rapporter("2. Construction du périmètre", &lignes);
}

// ===========================================================================
// 3. La descente multi-échelle : avec et sans périmètre
// ===========================================================================

fn banc_hierarchie() {
    let store = ObrainStore::new_in_memory();
    let racine = arbre(&store, 3, 6); // 6 + 36 + 216 = 258 nœuds
    let spec = HierarchySpec {
        max_depth: 4,
        per_level_limit: 500,
        ..Default::default()
    };

    let sans = ReadScope::all();
    // Un périmètre qui accorde le label : le filtrage s'exerce, mais rien n'est
    // masqué — c'est le coût PUR du contrôle.
    let avec = ReadScope::from_permissions([
        "graph:read",
        "node:read:*",
        "edge:read:*",
        "echelle:read:noeud",
    ]);

    let m_sans = mesurer(
        || {
            black_box(hierarchy::describe_scoped(
                &store,
                obrain_common::types::NodeId::new(racine),
                spec.clone(),
                &sans,
            ));
        },
        20,
        200,
    );
    let m_avec = mesurer(
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

    let rapport = m_avec.mediane.as_secs_f64() / m_sans.mediane.as_secs_f64();
    let lignes = vec![
        ligne(
            "describe — sans périmètre (258 nœuds)",
            m_sans,
            "le chemin d'AVANT : il ne doit pas ralentir",
        ),
        ligne(
            "describe — sous périmètre (tout accordé)",
            m_avec,
            format!("contrôle pur : ×{rapport:.2}"),
        ),
    ];
    rapporter("3. Descente multi-échelle", &lignes);
    println!("  → le contrôle coûte ×{rapport:.2} ; le filtrage lui-même est mesuré ci-dessous");

    // Le filtrage RÉEL : la moitié des labels hors périmètre.
    let store2 = ObrainStore::new_in_memory();
    {
        let session = store2.db().session();
        let racine = session.create_node_with_props(
            &["Groupe"],
            [("name", obrain_common::types::Value::from("racine"))],
        );
        for i in 0..60 {
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
        store2.commit_wal().expect("commit");

        let filtrant =
            ReadScope::from_permissions(["graph:read", "node:read:Groupe", "edge:read:CONTAINS"]);
        let racine = racine.as_u64();
        let m_filtre = mesurer(
            || {
                black_box(hierarchy::describe_scoped(
                    &store2,
                    obrain_common::types::NodeId::new(racine),
                    spec.clone(),
                    &filtrant,
                ));
            },
            20,
            200,
        );
        let vue = hierarchy::describe_scoped(
            &store2,
            obrain_common::types::NodeId::new(racine),
            spec.clone(),
            &filtrant,
        );
        println!(
            "  → filtrage réel (60 enfants, la moitié masquée) : {:.2}µs · publiés {} · masqués {}",
            m_filtre.mediane.as_secs_f64() * 1e6,
            vue.levels[0].nodes.len(),
            vue.levels[0].hidden
        );
    }
}

// ===========================================================================
// 4. La forme du store
// ===========================================================================

fn banc_relief() {
    let store = ObrainStore::new_in_memory();
    arbre(&store, 3, 6);

    let opts = ReliefOptions {
        include_counts: true,
    };
    let sans = ReadScope::all();
    let avec = ReadScope::from_permissions([
        "graph:read",
        "node:read:*",
        "edge:read:*",
        "echelle:read:noeud",
    ]);

    let m_sans = mesurer(
        || {
            black_box(Relief::describe_scoped(&store, opts, &sans));
        },
        10,
        100,
    );
    let m_avec = mesurer(
        || {
            black_box(Relief::describe_scoped(&store, opts, &avec));
        },
        10,
        100,
    );

    let rapport = m_avec.mediane.as_secs_f64() / m_sans.mediane.as_secs_f64();
    rapporter(
        "4. Forme du store (relief)",
        &[
            ligne(
                "Relief::describe — sans périmètre",
                m_sans,
                "le chemin d'AVANT",
            ),
            ligne(
                "Relief::describe — sous périmètre (tout accordé)",
                m_avec,
                format!("contrôle pur : ×{rapport:.2}"),
            ),
        ],
    );
}

// ===========================================================================
// 5. La chaîne complète d'une requête (contexte → décision → périmètre)
// ===========================================================================

fn banc_chaine() {
    let store = ObrainStore::new_in_memory();
    arbre(&store, 2, 4);
    let utilisateur = uuid::Uuid::from_u128(3);
    let tenant = uuid::Uuid::from_u128(9);
    store
        .iam()
        .materialize(
            &utilisateur.to_string(),
            &tenant.to_string(),
            &[RoleGrant {
                role: "lecteur".into(),
                permissions: permissions(8),
            }],
        )
        .expect("matérialisation");

    let resolver = IamScopeResolver::new(store.iam());
    let contexte = this::core::auth::AuthContext::User {
        user_id: utilisateur,
        tenant_id: tenant,
        roles: vec![],
        scopes: Vec::new(),
    };

    let m = mesurer(
        || {
            black_box(resolver.scope_for(Some(&contexte)));
        },
        100,
        1_000,
    );
    rapporter(
        "5. Chaîne d'une requête (contexte → décision → périmètre)",
        &[ligne(
            "scope_for — décision matérialisée (11 permissions)",
            m,
            "ce que paie CHAQUE requête filtrée",
        )],
    );
}

// ===========================================================================
// Le budget : un test de non-régression, pas seulement un chiffre
// ===========================================================================

/// **Les budgets**, en ratios — jamais en temps absolu.
///
/// Un seuil en microsecondes dépendrait de la machine et deviendrait faux au
/// premier ordinateur plus lent. Un **ratio** entre deux mesures faites dans le
/// même processus sur le même décor, lui, veut dire quelque chose : « filtrer ne
/// doit pas coûter plus de N fois le travail non filtré ».
fn verifier_budgets() -> Result<Vec<(String, f64, f64)>, String> {
    let mut resultats = Vec::new();

    // Sans périmètre : le chemin d'avant ne doit pas être dégradé. On le compare
    // à lui-même via `ReadScope::all()`, qui doit court-circuiter.
    let tout = ReadScope::all();
    let scope = ReadScope::from_permissions(permissions(8).iter().map(String::as_str));
    let m_tout = mesurer(
        || {
            black_box(tout.may_read_node("Label0"));
        },
        1_000,
        20_000,
    );
    let m_scoped = mesurer(
        || {
            black_box(scope.may_read_node("Label0"));
        },
        1_000,
        20_000,
    );
    let ratio = m_scoped.mediane.as_secs_f64() / m_tout.mediane.as_secs_f64().max(1e-12);
    resultats.push(("matching : accordé vs illimité".into(), ratio, 1_000.0));

    // Une correspondance ne doit pas être quadratique : de 1 à 64 permissions,
    // le refus ne doit pas exploser de plus d'un facteur ~100 (large : on veut
    // détecter un O(n²), pas pinailler sur une constante).
    let petit = ReadScope::from_permissions(permissions(1).iter().map(String::as_str));
    let grand = ReadScope::from_permissions(permissions(64).iter().map(String::as_str));
    let m_petit = mesurer(
        || {
            black_box(petit.may_read_node("JamaisAccorde"));
        },
        1_000,
        10_000,
    );
    let m_grand = mesurer(
        || {
            black_box(grand.may_read_node("JamaisAccorde"));
        },
        1_000,
        10_000,
    );
    let croissance = m_grand.mediane.as_secs_f64() / m_petit.mediane.as_secs_f64().max(1e-12);
    resultats.push((
        "matching : refus, 64 vs 1 permissions".into(),
        croissance,
        80.0,
    ));

    // La descente filtrée par rapport à la descente nue : le filtrage regarde
    // chaque enfant, il a donc le droit de coûter — mais pas un ordre de grandeur.
    let store = ObrainStore::new_in_memory();
    let racine = arbre(&store, 3, 4);
    let spec = HierarchySpec {
        max_depth: 4,
        per_level_limit: 500,
        ..Default::default()
    };
    let avec = ReadScope::from_permissions(["node:read:*", "edge:read:*"]);
    let m_nu = mesurer(
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
    let m_filtre = mesurer(
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
    let overhead = m_filtre.mediane.as_secs_f64() / m_nu.mediane.as_secs_f64().max(1e-12);
    resultats.push(("descente : filtrée vs nue".into(), overhead, 3.0));

    for (nom, ratio, budget) in &resultats {
        let verdict = if *ratio <= *budget { "✓" } else { "✗" };
        println!("  {verdict} {nom:<44} ×{ratio:>8.2}  (budget ×{budget})");
    }
    let depasses: Vec<String> = resultats
        .iter()
        .filter(|(_, ratio, budget)| ratio > budget)
        .map(|(nom, ratio, budget)| format!("{nom} : ×{ratio:.2} > ×{budget}"))
        .collect();
    if depasses.is_empty() {
        Ok(resultats)
    } else {
        Err(depasses.join(" · "))
    }
}

fn main() {
    let verifier = std::env::args().any(|a| a == "--verifier");

    println!("\n╔══════════════════════════════════════════════════════════════════════╗");
    println!("║  Droits granulaires — bancs d'essai                                  ║");
    println!("╚══════════════════════════════════════════════════════════════════════╝");

    banc_correspondance();
    banc_construction();
    banc_hierarchie();
    banc_relief();
    banc_chaine();

    println!("\n── Budgets (ratios) ─────────────────────────────────────────────────");
    match verifier_budgets() {
        Ok(_) => {
            println!("\n✅ Tous les budgets sont tenus.\n");
        }
        Err(raison) => {
            println!("\n❌ Budget dépassé : {raison}\n");
            if verifier {
                std::process::exit(1);
            }
        }
    }

    // Le décor utilisé par les bancs de la chaîne complète, pour mémoire.
    let canvas = Arc::new(SimpleCanvasState::new());
    canvas.set_scale("une-portee", Scale::Groupe);
    println!(
        "  (rappel : le vocabulaire d'échelle va de {:?} à {:?})\n",
        Scale::ALL.first().map(|s| s.as_str()),
        Scale::ALL.last().map(|s| s.as_str())
    );
}
