//! #287 (case f) : le masquage n'est PAS dans le port `Recorder` — et ce choix se
//! CONSTATE.
//!
//! La boucle du harnais pousse les étapes telles que l'outil les a produites : c'est
//! au point d'écriture que les secrets se masquent, donc dans l'implémentation. Un
//! `Recorder` sans masque écrit ce que l'outil a rendu, secret compris. Le défaut n'est
//! pas acceptable pour autant : ce qui est unacceptable, c'est qu'il soit SILENCIEUX.
//!
//! Ces deux tests montrent les deux branches du contrat, avec le même run et le même
//! magasin (un vrai `ObrainStore` sur disque) :
//!
//! 1. `HarnessGraph::new` → aucun masque : le texte est écrit tel quel, et
//!    `masque_installe()` le dit ;
//! 2. `ServerHost::harness_recorder()` → masque installé au point d'écriture, et
//!    `masque_installe()` le dit aussi.
//!
//! Aucune règle de permission n'est touchée ici : c'est un contrat de journalisation.
#![cfg(feature = "obrain")]

use std::sync::Arc;

use this::server::builder::ServerBuilder;
use this::server::host::HostRecorder;
use this::storage::{HarnessGraph, ObrainStore};
use this_agent::{Goal, Run, Step, Stop};
use this_harness::Recorder;

const CLE: &str = "sk-ant-api03-abcdefghijklmnopqrstuvwxyz";

fn run_de_test() -> Run {
    let mut run = Run::new(Goal::new(format!("configure la clé {CLE}")), 4);
    run.record(Step::tool("ecrire_fichier", 12).resume(format!("OPENAI_API_KEY={CLE}")));
    run.finish(Stop::Done);
    run
}

fn magasin(prefixe: &str) -> Arc<ObrainStore> {
    let dir = std::env::temp_dir().join(format!("{prefixe}_{}", uuid::Uuid::new_v4()));
    ObrainStore::open(&dir).unwrap()
}

/// Les textes qu'un journal a réellement écrits, lus à même le graphe : le but du run,
/// l'extrait, et la charge `_data` de chaque nœud.
fn textes_ecrits(store: &ObrainStore) -> Vec<String> {
    let g = store.db().graph_store_mut();
    let mut out = Vec::new();
    for label in ["_run", "_step"] {
        for id in g.nodes_by_label(label) {
            let Some(n) = g.get_node(id) else { continue };
            for key in ["_goal", "_out_excerpt", "_data"] {
                if let Some(v) = n.get_property(key).and_then(|v| v.as_str()) {
                    out.push(v.to_string());
                }
            }
        }
    }
    out
}

#[tokio::test]
async fn un_recorder_sans_masque_ecrit_tel_quel_et_la_garde_le_dit() {
    let store = magasin("this_jm_sans");
    let journal = HarnessGraph::new(Arc::clone(&store));
    assert!(
        !journal.is_mask_installed(),
        "HarnessGraph::new n'installe aucun masque : le choix doit être constatable"
    );
    let recorder = HostRecorder::new(journal);
    assert!(
        !recorder.is_mask_installed(),
        "le constat traverse le Recorder de l'hôte : rien n'est masqué en sous-main"
    );

    let run = run_de_test();
    recorder
        .step("run-sans", &run, run.steps.last().unwrap())
        .await;
    recorder.stopped("run-sans", &run, &Stop::Done).await;

    let relu = HarnessGraph::new(Arc::clone(&store))
        .load_run("run-sans")
        .unwrap()
        .expect("le run est écrit");
    assert_eq!(
        relu.goal.text,
        format!("configure la clé {CLE}"),
        "sans masque, le journal écrit ce que l'outil a rendu"
    );
    assert!(
        textes_ecrits(&store).iter().any(|t| t.contains(CLE)),
        "la clé est dans le graphe : c'est le choix explicite d'un hôte qui n'installe \
         pas de masque, pas un oubli que la garde laisserait passer en silence"
    );
}

#[tokio::test]
async fn le_chemin_de_l_hote_masque_et_le_dit() {
    let store = magasin("this_jm_avec");
    let host = ServerBuilder::new()
        .with_obrain_store(Arc::clone(&store))
        .without_write_fencing()
        .build_host()
        .unwrap();
    let recorder = host.harness_recorder().expect("un magasin est attaché");
    assert!(
        recorder.is_mask_installed(),
        "le chemin de l'hôte (harness_recorder) passe par harness_journal et masque"
    );

    let run = run_de_test();
    recorder
        .step("run-avec", &run, run.steps.last().unwrap())
        .await;
    recorder.stopped("run-avec", &run, &Stop::Done).await;

    let relu = host
        .harness_journal()
        .unwrap()
        .load_run("run-avec")
        .unwrap()
        .expect("le run est écrit");
    assert_eq!(relu.goal.text, "configure la clé [secret masqué]");
    assert_eq!(relu.steps[0].resume, "OPENAI_API_KEY=[secret masqué]");
    assert!(
        textes_ecrits(&store)
            .iter()
            .all(|t| !t.contains("abcdefghij")),
        "le masque est appliqué au point d'écriture, pas à la relecture"
    );
}
