//! Le journal du harnais mis en erreur : collisions d'id, secrets par les chemins
//! qui contournent le masque, masque qui panique, entrées hostiles. Vrai `ObrainStore`.
#![cfg(feature = "obrain")]

use std::sync::Arc;

use this::storage::{HarnessGraph, ObrainStore};
use this_agent::{Goal, Run, Step};
use this_harness::Recorder;

const CLE: &str = "sk-ant-api03-abcdefghijklmnopqrstuvwxyz";

fn dir() -> std::path::PathBuf {
    std::env::temp_dir().join(format!("this_rj_{}", uuid::Uuid::new_v4()))
}
fn journal(store: &Arc<ObrainStore>) -> HarnessGraph {
    HarnessGraph::new(Arc::clone(store)).with_masque(this_agent::masque::masquer)
}
fn run(goal: &str, tools: &[&str]) -> Run {
    let mut r = Run::new(Goal::new(goal), 6);
    for t in tools {
        r.record(Step::tool(*t, 1));
    }
    r.after_turn(true, false);
    r
}
/// Toutes les propriétés texte de tous les nœuds du journal.
fn tout_le_texte(store: &ObrainStore) -> String {
    let g = store.db().graph_store_mut();
    let mut out = String::new();
    for label in ["_run", "_step", "_harness"] {
        for id in g.nodes_by_label(label) {
            if let Some(n) = g.get_node(id) {
                for (_, v) in n.properties.iter() {
                    if let Some(s) = v.as_str() {
                        out.push_str(s);
                        out.push('\n');
                    }
                }
            }
        }
    }
    out
}

/// PERTE DE DONNÉES : un second run de même id écrasait le premier (goal, stop, _data).
#[test]
fn un_run_d_un_autre_but_ne_peut_pas_ecraser_un_run_existant() {
    let d = dir();
    let store = ObrainStore::open(&d).unwrap();
    let j = journal(&store);
    j.record_run(&run("premier but", &["a", "b"]), "r1")
        .unwrap();
    let err = j.record_run(&run("AUTRE but", &["c"]), "r1");
    assert!(err.is_err(), "écrasement silencieux du run r1");
    let back = j.load_run("r1").unwrap().unwrap();
    assert_eq!(back.goal.text, "premier but");
    assert_eq!(back.steps.len(), 2);
    assert_eq!(back.steps[0].tool, "a");
    // La réécriture légitime du MÊME run (vivant puis arrêt) reste permise.
    j.record_run(&run("premier but", &["a", "b"]), "r1")
        .unwrap();
    drop(j);
    drop(store);
    std::fs::remove_dir_all(&d).ok();
}

/// PERTE DE DONNÉES : `append_step_live` d'un autre run sur un id déjà arrêté
/// remplaçait les étapes (même `run#index`) et le `_data` du run fini.
#[test]
fn append_live_sur_un_run_deja_arrete_d_un_autre_but_est_refuse() {
    let d = dir();
    let store = ObrainStore::open(&d).unwrap();
    let j = journal(&store);
    j.record_run(&run("fini", &["a", "b"]), "r1").unwrap();
    let autre = run("intrus", &["zzz"]);
    assert!(j.append_step_live(&autre, "r1", &autre.steps[0]).is_err());
    let back = j.load_run("r1").unwrap().unwrap();
    assert_eq!(back.goal.text, "fini");
    assert_eq!(back.steps[0].tool, "a");
    drop(j);
    drop(store);
    std::fs::remove_dir_all(&d).ok();
}

/// Un run absent : `append_step` refuse sans rien écrire (pas d'étape orpheline).
#[test]
fn append_step_sur_un_run_absent_n_ecrit_rien() {
    let d = dir();
    let store = ObrainStore::open(&d).unwrap();
    let j = journal(&store);
    let r = run("g", &["a"]);
    assert!(j.append_step(&r, "absent", &r.steps[0]).is_err());
    assert!(tout_le_texte(&store).is_empty(), "nœud orphelin écrit");
    drop(j);
    drop(store);
    std::fs::remove_dir_all(&d).ok();
}

/// SECRET : `_tool` n'était pas masqué (nom d'outil libre, vient du modèle).
#[test]
fn un_secret_dans_un_nom_d_outil_n_atteint_pas_le_graphe() {
    let d = dir();
    let store = ObrainStore::open(&d).unwrap();
    let j = journal(&store);
    j.record_run(&run("g", &[&format!("outil {CLE}")]), "r1")
        .unwrap();
    let texte = tout_le_texte(&store);
    assert!(!texte.contains(CLE), "secret dans le graphe :\n{texte}");
    drop(j);
    drop(store);
    std::fs::remove_dir_all(&d).ok();
}

/// SECRET : `_out_hash` recopiait `Step::out_hash` tel quel (une chaîne libre).
#[test]
fn un_secret_dans_out_hash_n_atteint_pas_le_graphe() {
    let d = dir();
    let store = ObrainStore::open(&d).unwrap();
    let j = journal(&store);
    let mut r = Run::new(Goal::new("g"), 6);
    let mut s = Step::tool("a", 1);
    s.out_hash = Some(CLE.to_string());
    r.record(s);
    r.after_turn(true, false);
    j.record_run(&r, "r1").unwrap();
    let texte = tout_le_texte(&store);
    assert!(!texte.contains(CLE), "secret dans le graphe :\n{texte}");
    drop(j);
    drop(store);
    std::fs::remove_dir_all(&d).ok();
}

/// Caractères de contrôle et unicode dans le nom d'outil : écrits et relus à l'identique.
#[test]
fn des_caracteres_de_controle_dans_un_nom_d_outil_survivent() {
    let d = dir();
    let store = ObrainStore::open(&d).unwrap();
    let j = journal(&store);
    let nom = "a\0b\n\r\t\u{1b}[31m\u{202e}é💥";
    j.record_run(&run("g", &[nom]), "r1").unwrap();
    assert_eq!(j.load_run("r1").unwrap().unwrap().steps[0].tool, nom);
    drop(j);
    drop(store);
    std::fs::remove_dir_all(&d).ok();
}

/// Un but de 10 Mo (avec une clé au milieu) : écrit, masqué, en temps borné.
#[test]
fn un_but_de_10_mo_est_ecrit_masque_en_temps_borne() {
    let d = dir();
    let store = ObrainStore::open(&d).unwrap();
    let j = journal(&store);
    let moitie = "x ".repeat(2_500_000);
    let goal = format!("{moitie}{CLE} {moitie}");
    let t = std::time::Instant::now();
    j.record_run(&run(&goal, &["a"]), "r1").unwrap();
    assert!(t.elapsed().as_secs() < 60, "{:?}", t.elapsed());
    assert!(!tout_le_texte(&store).contains(CLE));
    drop(j);
    drop(store);
    std::fs::remove_dir_all(&d).ok();
}

/// Un magasin en lecture seule : erreur propre, jamais une panique ni une écriture.
#[test]
fn un_magasin_en_lecture_seule_refuse_sans_paniquer() {
    let d = dir();
    {
        let store = ObrainStore::open(&d).unwrap();
        journal(&store).record_run(&run("g", &["a"]), "r1").unwrap();
    }
    let ro = ObrainStore::open_read_only(&d).unwrap();
    let j = journal(&ro);
    let r = run("g", &["a", "b"]);
    assert!(j.record_run(&r, "r2").is_err());
    assert!(j.append_step_live(&r, "r1", &r.steps[1]).is_err());
    assert!(j.load_run("r2").unwrap().is_none());
    drop(j);
    drop(ro);
    std::fs::remove_dir_all(&d).ok();
}

/// Un masque qui panique ne doit pas remonter dans la boucle du harnais : le contrat
/// du `Recorder` est qu'un échec d'enregistrement n'interrompt jamais le run.
#[test]
fn un_masque_qui_panique_n_arrete_pas_le_run() {
    let d = dir();
    let store = ObrainStore::open(&d).unwrap();
    let j = HarnessGraph::new(Arc::clone(&store)).with_masque(|_| panic!("masque cassé"));
    let rec = this::server::host::HostRecorder::new(j);
    let r = run("g", &["a"]);
    let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        futures_lite_block(async {
            rec.step("r1", &r, &r.steps[0]).await;
            rec.stopped("r1", &r, &this_agent::Stop::Done).await;
        })
    }));
    assert!(res.is_ok(), "la panique du masque a traversé le Recorder");
    drop(rec);
    drop(store);
    std::fs::remove_dir_all(&d).ok();
}

fn futures_lite_block<F: std::future::Future>(f: F) -> F::Output {
    tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap()
        .block_on(f)
}
