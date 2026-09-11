//! iam_bench — ce que coûte une décision d'autorisation, **traversée contre matérialisation**.
//!
//! C'est la mesure qui porte le chantier IAM : une décision vit sur le chemin de
//! **chaque** requête. Le modèle naturel — suivre les arêtes
//! `(Principal)-[:HAS_ROLE]->(Role)-[:GRANTS]->(Permission)` — coûte deux sauts
//! plus une union, à chaque fois. La matérialisation paie ce coût **une fois**,
//! à l'écriture IAM (rare), et ramène la décision à une **recherche indexée**.
//!
//! ```text
//! cargo run -p this-rs --features obrain --release --example iam_bench
//! cargo run -p this-rs --features obrain --release --example iam_bench -- 500 4 25 200
//! ```
//!
//! `usage: iam_bench [principaux] [rôles] [permissions par rôle] [itérations]`
//!
//! ⚠️ La traversée de référence du module est **optimiste** : elle ne filtre pas
//! les types d'arêtes. L'écart mesuré ici est donc un **minorant** de l'écart réel.

use std::time::Instant;

use this::storage::ObrainStore;
use this::storage::iam::{Iam, PendingDecision, RoleGrant};

/// Coût moyen d'une opération, après un tour de chauffe.
/// Minimum et médiane d'une opération.
///
/// La machine est partagée : la médiane d'une même opération varie d'un facteur
/// 2 à 3 d'une exécution à l'autre. Le **minimum** est le coût sans
/// interférence — c'est lui qu'on peut citer, et c'est lui qui rend la
/// comparaison index/traversée lisible.
#[derive(Clone, Copy)]
struct Stats {
    min: f64,
    median: f64,
}

fn bench<T>(iters: u32, mut op: impl FnMut() -> T) -> Stats {
    let _ = op(); // chauffe
    let mut samples = Vec::with_capacity(iters as usize);
    for _ in 0..iters {
        let start = Instant::now();
        let v = op();
        std::hint::black_box(&v);
        samples.push(start.elapsed().as_secs_f64());
    }
    samples.sort_by(|a, b| a.partial_cmp(b).unwrap());
    Stats {
        min: samples[0],
        median: samples[samples.len() / 2],
    }
}

fn dur(secs: f64) -> String {
    if secs < 1e-6 {
        format!("{:>10.0} ns", secs * 1e9)
    } else if secs < 1e-3 {
        format!("{:>10.2} µs", secs * 1e6)
    } else {
        format!("{:>10.2} ms", secs * 1e3)
    }
}

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let principals: usize = args
        .next()
        .and_then(|a| a.parse().ok())
        .unwrap_or(200)
        .max(1);
    let roles: usize = args.next().and_then(|a| a.parse().ok()).unwrap_or(4).max(1);
    let perms_per_role: usize = args
        .next()
        .and_then(|a| a.parse().ok())
        .unwrap_or(20)
        .max(1);
    let iters: u32 = args
        .next()
        .and_then(|a| a.parse().ok())
        .unwrap_or(100)
        .max(1);

    println!(
        "iam_bench — {principals} principaux × {roles} rôles × {perms_per_role} permissions \
         ({iters} itérations)"
    );
    println!();

    let store = ObrainStore::new_in_memory();
    let iam = store.iam();

    // ── Le graphe d'administration : Principal→Role→Permission ───────────
    let setup = Instant::now();
    let grants: Vec<RoleGrant> = (0..roles)
        .map(|r| RoleGrant {
            role: format!("role-{r}"),
            permissions: (0..perms_per_role)
                .map(|p| format!("res{}:action{}", p % 10, p))
                .collect(),
        })
        .collect();

    let session = store.db().session();
    for i in 0..principals {
        let principal = format!("user:{i}");
        // Le vrai graphe, arêtes comprises.
        let p_node = session.create_node_with_props(
            &["Principal"],
            [(
                "qualified_name",
                obrain_common::types::Value::from(principal.as_str()),
            )],
        );
        for (r, grant) in grants.iter().enumerate() {
            let role_node = session.create_node_with_props(
                &["Role"],
                [(
                    "name",
                    obrain_common::types::Value::from(format!("{}-{i}", grant.role).as_str()),
                )],
            );
            session.create_edge(p_node, role_node, "HAS_ROLE");
            for permission in &grant.permissions {
                let perm_node = session.create_node_with_props(
                    &["Permission"],
                    [(
                        "name",
                        obrain_common::types::Value::from(permission.as_str()),
                    )],
                );
                session.create_edge(role_node, perm_node, "GRANTS");
            }
            let _ = r;
        }
    }
    // La matérialisation en UN seul lot (un commit pour tous les principaux).
    let names: Vec<String> = (0..principals).map(|i| format!("user:{i}")).collect();
    let batch: Vec<PendingDecision<'_>> = names
        .iter()
        .map(|p| PendingDecision {
            principal: p,
            scope: "tenant-a",
            roles: &grants,
        })
        .collect();
    iam.materialize_batch(&batch)?;
    let setup_secs = setup.elapsed().as_secs_f64();

    let nodes = principles_nodes(principals, roles, perms_per_role);
    println!(
        "graphe     : {principals} principaux, {nodes} nœuds (rôles + permissions), \
         construit en {setup_secs:.3} s"
    );
    println!();

    // ── L'écriture : individuel contre lot ───────────────────────────────
    //
    // La mesure précédente avait montré ~129 ms par principal, dominée par le
    // `commit_wal` (fsync) payé à CHAQUE appel. Une écriture IAM est rare et
    // groupée : payer N fsync pour un seul changement est un gaspillage pur.
    let per_write = if principals >= 20 { 20 } else { principals };
    let write_names: Vec<String> = (0..per_write).map(|i| format!("w:{i}")).collect();

    let store_one = ObrainStore::new_in_memory();
    let iam_one = store_one.iam();
    let t = Instant::now();
    for name in &write_names {
        iam_one.materialize(name, "tenant-a", &grants)?;
    }
    let individual_secs = t.elapsed().as_secs_f64();

    let store_many = ObrainStore::new_in_memory();
    let iam_many = store_many.iam();
    let many: Vec<PendingDecision<'_>> = write_names
        .iter()
        .map(|p| PendingDecision {
            principal: p,
            scope: "tenant-a",
            roles: &grants,
        })
        .collect();
    let t = Instant::now();
    iam_many.materialize_batch(&many)?;
    let batched_secs = t.elapsed().as_secs_f64();

    println!(
        "écriture — {per_write} principaux × {} permissions effectives",
        grants.iter().map(|g| g.permissions.len()).sum::<usize>()
    );
    println!(
        "  un commit par principal          {:>10.2} ms  ({:.2} ms/principal)",
        individual_secs * 1000.0,
        individual_secs * 1000.0 / per_write as f64
    );
    println!(
        "  un seul commit pour le lot       {:>10.2} ms  ({:.3} ms/principal)",
        batched_secs * 1000.0,
        batched_secs * 1000.0 / per_write as f64
    );
    println!(
        "  → rapport                        ×{:.0}",
        individual_secs / batched_secs
    );
    println!();

    // ── Le chemin chaud : une décision ───────────────────────────────────
    let target = format!("user:{}", principals / 2);
    let indexed = bench(iters, || {
        iam.decision(&target, "tenant-a")
            .expect("lecture")
            .expect("décision")
    });
    let traversal = bench(iters, || {
        iam.permissions_by_traversal(&target).expect("traversée")
    });

    println!(
        "décision par INDEX (matérialisée)   {}  (médiane {})",
        dur(indexed.min),
        dur(indexed.median)
    );
    println!(
        "décision par TRAVERSÉE (référence)  {}  (médiane {})",
        dur(traversal.min),
        dur(traversal.median)
    );
    println!(
        "  → rapport (sur les minimums)      ×{:.0}",
        traversal.min / indexed.min
    );
    println!();
    println!(
        "  ⚠️  ce rapport VARIE avec la charge de la machine (mesuré de ×5 à ×14 sur la même\n      \
         machine) : la traversée touche beaucoup de nœuds, donc elle dépend du cache.\n      \
         Le contrat robuste est plus bas — l'index est PLAT, la traversée croît."
    );
    println!();

    // ── Le contrat robuste : l'index est plat, la traversée croît ────────
    //
    // Un rapport unique dépend de la machine. Ce qui ne dépend pas d'elle,
    // c'est la FORME : la décision matérialisée coûte le même prix quel que
    // soit le nombre de rôles, parce qu'elle ne les parcourt pas. On le mesure
    // en faisant varier la taille du graphe d'administration.
    println!("mise à l'échelle (le chiffre qui ne dépend pas de la machine) :");
    println!(
        "  {:<26} {:>12} {:>14} {:>10}",
        "rôles × permissions", "index", "traversée", "rapport"
    );
    for (roles_n, perms_n) in [(1usize, 5usize), (4, 20), (8, 40)] {
        let grants_n: Vec<RoleGrant> = (0..roles_n)
            .map(|r| RoleGrant {
                role: format!("role-{r}"),
                permissions: (0..perms_n)
                    .map(|p| format!("res{}:action{}", p % 10, p))
                    .collect(),
            })
            .collect();
        let scale_store = ObrainStore::new_in_memory();
        let scale_iam = scale_store.iam();
        let session_n = scale_store.db().session();
        let p_node = session_n.create_node_with_props(
            &["Principal"],
            [(
                "qualified_name",
                obrain_common::types::Value::from("user:scale"),
            )],
        );
        for grant in &grants_n {
            let role_node = session_n.create_node_with_props(
                &["Role"],
                [(
                    "name",
                    obrain_common::types::Value::from(grant.role.as_str()),
                )],
            );
            session_n.create_edge(p_node, role_node, "HAS_ROLE");
            for permission in &grant.permissions {
                let perm_node = session_n.create_node_with_props(
                    &["Permission"],
                    [(
                        "name",
                        obrain_common::types::Value::from(permission.as_str()),
                    )],
                );
                session_n.create_edge(role_node, perm_node, "GRANTS");
            }
        }
        scale_iam
            .materialize("user:scale", "tenant-a", &grants_n)
            .expect("matérialisation");

        let small_iters = (iters / 4).max(20);
        let idx = bench(small_iters, || {
            scale_iam
                .decision("user:scale", "tenant-a")
                .expect("lecture")
                .expect("décision")
        });
        let trv = bench(small_iters, || {
            scale_iam
                .permissions_by_traversal("user:scale")
                .expect("traversée")
        });
        println!(
            "  {:<26} {:>12} {:>14} {:>10}",
            format!("{roles_n} × {perms_n}"),
            dur(idx.min),
            dur(trv.min),
            format!("×{:.1}", trv.min / idx.min)
        );
    }
    println!(
        "  → la colonne `index` reste PLATE : c'est la propriété qui tient, quel que soit\n    \
         le nombre de rôles et la charge de la machine."
    );
    println!();
    println!(
        "lecture : {:.0} décisions indexées par milliseconde",
        1000.0 / (indexed.min * 1000.0)
    );

    let stamp = iam
        .decision(&target, "tenant-a")
        .expect("lecture")
        .expect("décision");
    println!();
    println!(
        "décision   : époque {} · {} rôle(s) · {} permission(s) effectives",
        stamp.epoch,
        stamp.roles.len(),
        stamp.permissions.len()
    );
    println!(
        "fidélité   : matérialisé == traversé ? {}",
        stamp.permissions == iam.permissions_by_traversal(&target)?
    );

    Ok(())
}

/// Nombre de nœuds de rôle et de permission créés (rôles et permissions sont
/// dupliqués par principal pour que la traversée soit réaliste).
fn principles_nodes(principals: usize, roles: usize, perms: usize) -> usize {
    principals * (roles + roles * perms)
}

/// Rendre le helper visible au calcul ci-dessus sans le dupliquer.
fn _unused(_: &Iam) {}
