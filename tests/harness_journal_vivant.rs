//! Le journal **vivant** : un run pousse ses étapes une à une par le
//! `Recorder`, sans jamais construire le run complet d'abord, et le magasin
//! garde chaque étape au fil de l'eau — la preuve qu'un run interrompu (crash,
//! page fermée) n'est pas un run perdu.
//!
//! Le magasin est un **vrai** `ObrainStore` sur disque, comme le journal
//! « complet » de `harness_journal.rs` — pas un faux magasin.
#![cfg(feature = "obrain")]

use std::sync::Arc;

use obrain_core::graph::Direction;
use this::server::builder::ServerBuilder;
use this::storage::ObrainStore;
use this_agent::{Goal, Run, Step, Stop};
use this_harness::Recorder;

const CLE: &str = "sk-ant-api03-abcdefghijklmnopqrstuvwxyz";

/// Les arêtes `HAS_STEP` sortant du run, lues à même le graphe : `(étape
/// `_uuid`, `_index` de l'arête)`, triées.
fn has_step_edges(store: &ObrainStore, run_id: &str) -> Vec<(String, String)> {
    let g = store.db().graph_store_mut();
    let head = g
        .nodes_by_label("_run")
        .into_iter()
        .find(|id| {
            g.get_node(*id)
                .and_then(|n| {
                    n.get_property("_uuid")
                        .and_then(|v| v.as_str().map(str::to_string))
                })
                .as_deref()
                == Some(run_id)
        })
        .expect("le nœud de run est là");
    let mut out: Vec<(String, String)> = g
        .edges_from(head, Direction::Outgoing)
        .into_iter()
        .filter(|(_, e)| g.edge_type(*e).as_deref() == Some("HAS_STEP"))
        .map(|(dst, e)| {
            let uuid = g
                .get_node(dst)
                .and_then(|n| {
                    n.get_property("_uuid")
                        .and_then(|v| v.as_str().map(str::to_string))
                })
                .unwrap_or_default();
            let index = g
                .get_edge_property(e, &"_index".into())
                .and_then(|v| v.as_str().map(str::to_string))
                .unwrap_or_default();
            (uuid, index)
        })
        .collect();
    out.sort();
    out
}

#[tokio::test]
async fn un_run_vivant_pousse_ses_etapes_une_a_une_et_les_garde() {
    let dir = std::env::temp_dir().join(format!("this_hjv_{}", uuid::Uuid::new_v4()));
    let store: Arc<ObrainStore> = ObrainStore::open(&dir).unwrap();
    let host = ServerBuilder::new()
        .with_obrain_store(Arc::clone(&store))
        .without_write_fencing()
        .build_host()
        .unwrap();
    let recorder = host.harness_recorder().expect("un magasin est attaché");

    // Le run vit : chaque étape est poussée dès qu'elle est mesurée. On ne
    // construit JAMAIS le run complet d'abord — il n'existe encore nulle part.
    let mut run = Run::new(Goal::new(format!("configure {CLE}")), 6);
    run.record(Step::tool("install_module", 1840).state("a", "b"));
    recorder
        .step("run-v", &run, run.steps.last().unwrap())
        .await;
    run.record(Step::tool("ecrire_fichier", 3).resume(format!("OPENAI={CLE}")));
    recorder
        .step("run-v", &run, run.steps.last().unwrap())
        .await;
    run.record(Step::tool("lancer_build", 90).refused("hors périmètre"));
    recorder
        .step("run-v", &run, run.steps.last().unwrap())
        .await;

    // Avant même la fin : les trois étapes et leurs arêtes sont déjà là, dans
    // l'ordre — c'est ce qui distingue un journal vivant d'un journal recollé.
    let journal = host.harness_journal().unwrap();
    let vivant = journal.load_run("run-v").unwrap().expect("le run est là");
    assert_eq!(vivant.steps.len(), 3, "trois étapes déjà écrites");
    let outils: Vec<&str> = vivant.steps.iter().map(|s| s.tool.as_str()).collect();
    assert_eq!(
        outils,
        ["install_module", "ecrire_fichier", "lancer_build"],
        "l'ordre est celui de _index"
    );
    assert_eq!(
        has_step_edges(&store, "run-v"),
        vec![
            ("run-v#0".to_string(), "0".to_string()),
            ("run-v#1".to_string(), "1".to_string()),
            ("run-v#2".to_string(), "2".to_string())
        ],
        "une arête HAS_STEP par étape, dans le bon ordre"
    );

    // Le masque : la clé n'est écrite nulle part, ni dans l'excerpt ni dans le run.
    assert_eq!(vivant.goal.text, "configure [secret masqué]");
    assert_eq!(vivant.steps[1].resume, "OPENAI=[secret masqué]");
    let g = store.db().graph_store_mut();
    let textes: Vec<String> = ["_run", "_step"]
        .iter()
        .flat_map(|label| g.nodes_by_label(label))
        .filter_map(|id| g.get_node(id))
        .filter_map(|n| {
            for key in ["_goal", "_out_excerpt", "_data"] {
                if let Some(v) = n.get_property(key).and_then(|v| v.as_str()) {
                    return Some(v.to_string());
                }
            }
            None
        })
        .collect();
    assert!(
        textes.iter().all(|t| !t.contains("abcdefghij")),
        "aucun secret écrit : {textes:?}"
    );

    // La fin, elle, fige l'arrêt — et seulement elle.
    run.finish(Stop::Done);
    recorder.stopped("run-v", &run, &Stop::Done).await;
    let fini = journal.load_run("run-v").unwrap().unwrap();
    assert_eq!(fini.stop, Some(Stop::Done));
    assert_eq!(
        fini.steps.len(),
        3,
        "les étapes ne sont pas réécrites en double"
    );

    drop(host);
    drop(store);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn sans_magasin_le_recorder_est_absent_et_rien_ne_casse() {
    let host = ServerBuilder::new()
        .with_link_service(this::storage::InMemoryLinkService::new())
        .build_host()
        .unwrap();
    assert!(host.harness_recorder().is_none());
}
