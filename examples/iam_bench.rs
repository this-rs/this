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
fn bench<T>(iters: u32, mut op: impl FnMut() -> T) -> f64 {
    let _ = op();
    let start = Instant::now();
    for _ in 0..iters {
        let v = op();
        std::hint::black_box(&v);
    }
    start.elapsed().as_secs_f64() / f64::from(iters)
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

    println!("décision par INDEX (matérialisée)   {}", dur(indexed));
    println!("décision par TRAVERSÉE (référence)  {}", dur(traversal));
    println!(
        "  → rapport                         ×{:.0}",
        traversal / indexed
    );
    println!();
    println!(
        "lecture : {:.0} décisions indexées par milliseconde",
        1000.0 / (indexed * 1000.0)
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
