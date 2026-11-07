//! Le journal du harness, par l'API publique de l'hôte : écrire un run, le relire.
//!
//! Le magasin est un **vrai** `ObrainStore` sur disque (répertoire temporaire),
//! rouvert entre l'écriture et la relecture : ce n'est pas un faux magasin.
#![cfg(feature = "obrain")]

use std::sync::Arc;

use obrain_core::graph::Direction;
use this::server::builder::ServerBuilder;
use this::storage::ObrainStore;
use this_agent::{Goal, Run, Step};

const CLE: &str = "sk-ant-api03-abcdefghijklmnopqrstuvwxyz";

fn run_de_test() -> Run {
    let mut run = Run::new(Goal::new(format!("installe le module {CLE}")), 6);
    run.record(Step::tool("install_module", 1840).state("a", "b"));
    run.record(Step::tool("ecrire_fichier", 3).refused("hors périmètre"));
    run.record(Step::tool("lancer_build", 90).failed("exit 1"));
    run.after_turn(true, false);
    run
}

#[tokio::test]
async fn un_run_s_ecrit_par_l_hote_et_se_relit_apres_reouverture() {
    let dir = std::env::temp_dir().join(format!("this_hj_{}", uuid::Uuid::new_v4()));
    let run = run_de_test();
    {
        let store: Arc<ObrainStore> = ObrainStore::open(&dir).unwrap();
        let host = ServerBuilder::new()
            .with_obrain_store(Arc::clone(&store))
            .without_write_fencing()
            .build_host()
            .unwrap();
        assert!(host.record_harness_run(&run, "run-j").unwrap());
    }

    // Réouverture : la persistance est celle du disque.
    let store = ObrainStore::open(&dir).unwrap();
    let host = ServerBuilder::new()
        .with_obrain_store(Arc::clone(&store))
        .without_write_fencing()
        .build_host()
        .unwrap();
    let back = host
        .harness_journal()
        .unwrap()
        .load_run("run-j")
        .unwrap()
        .expect("le run est là");
    assert_eq!(back.steps.len(), 3);
    assert_eq!(back.goal.text, "installe le module [secret masqué]");
    assert_eq!(back.tours, run.tours);
    assert_eq!(back.stop, run.stop);
    assert!(back.steps[1].refused);
    assert!(back.steps[2].failure.is_some());

    // Propriétés interrogeables et arêtes, lues directement dans le graphe.
    let g = store.db().graph_store_mut();
    let prop = |label: &str, uuid: &str, key: &str| -> Option<String> {
        g.nodes_by_label(label).into_iter().find_map(|id| {
            let n = g.get_node(id)?;
            (n.get_property("_uuid").and_then(|v| v.as_str()) == Some(uuid))
                .then(|| {
                    n.get_property(key)
                        .and_then(|v| v.as_str())
                        .map(String::from)
                })
                .flatten()
        })
    };
    assert_eq!(
        prop("_run", "run-j", "_goal").as_deref(),
        Some("installe le module [secret masqué]")
    );
    assert!(prop("_run", "run-j", "_stop").is_some());
    assert_eq!(
        prop("_run", "run-j", "_tours").as_deref(),
        Some(run.tours.to_string().as_str())
    );
    let step = |i: u32, key: &str| prop("_step", &format!("run-j#{i}"), key);
    assert_eq!(step(1, "_tool").as_deref(), Some("ecrire_fichier"));
    assert_eq!(step(1, "_index").as_deref(), Some("1"));
    assert_eq!(step(1, "_refused").as_deref(), Some("true"));
    assert_eq!(step(2, "_failed").as_deref(), Some("true"));
    assert_eq!(step(0, "_run").as_deref(), Some("run-j"));

    let head = g
        .nodes_by_label("_run")
        .into_iter()
        .find(|id| {
            g.get_node(*id)
                .and_then(|n| {
                    n.get_property("_uuid")
                        .and_then(|v| v.as_str().map(String::from))
                })
                .as_deref()
                == Some("run-j")
        })
        .unwrap();
    let has_step = g
        .edges_from(head, Direction::Outgoing)
        .into_iter()
        .filter(|(_, e)| g.edge_type(*e).as_deref() == Some("HAS_STEP"))
        .count();
    assert_eq!(has_step, 3, "une arête HAS_STEP par étape");

    drop(host);
    drop(store);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn sans_magasin_rien_n_est_ecrit_et_rien_ne_casse() {
    let host = ServerBuilder::new()
        .with_link_service(this::storage::InMemoryLinkService::new())
        .build_host()
        .unwrap();
    assert!(host.harness_journal().is_none());
    assert!(!host.record_harness_run(&run_de_test(), "run-x").unwrap());
}
