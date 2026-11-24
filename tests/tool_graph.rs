//! `ToolGraph` sur un VRAI `ObrainStore` sur disque (répertoire temporaire) : conformité du
//! port `ToolRepo`, réouverture, garde de schéma, parcours du graphe, atomicité, échelle.
#![cfg(feature = "obrain")]

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use obrain_common::types::{PropertyKey, Value as ObrainValue};
use obrain_core::graph::Direction;
use serde_json::json;
use tempfile::TempDir;
use this::storage::{ObrainStore, ToolGraph};
use this_harness::{
    MAX_VERSIONS, RecipeStep, ToolDef, ToolRecord, ToolRepo, ToolStats, ToolStatus, steps_hash,
};

fn record(name: &str, description: &str, calls: &[&str], seq: u64) -> ToolRecord {
    let steps: Vec<RecipeStep> = calls
        .iter()
        .enumerate()
        .map(|(i, t)| RecipeStep {
            tool: (*t).to_string(),
            args: json!({ "n": i, "q": "{{texte}}", "nested": {"a": [1, 2.5, null, true]} }),
        })
        .collect();
    ToolRecord {
        name: name.to_string(),
        versions: vec![ToolDef {
            version: 1,
            description: description.to_string(),
            parameters: json!({
                "type": "object",
                "properties": {"texte": {"type": "string"}, "n": {"type": "integer"}},
                "required": ["texte"],
            }),
            steps_hash: steps_hash(&steps),
            steps,
            tags: vec!["essai".to_string(), "é accentué".to_string()],
            note: "création".to_string(),
            seq,
            stats: ToolStats {
                uses: 5,
                ok: 3,
                failed: 1,
                refused: 1,
                last_used: 42,
            },
        }],
        status: ToolStatus::Active,
        dropped: 0,
    }
}

fn open_at(dir: &std::path::Path) -> (Arc<ObrainStore>, ToolGraph) {
    let store = ObrainStore::open(dir).expect("store");
    let graph = ToolGraph::open(Arc::clone(&store)).expect("tool graph");
    (store, graph)
}

#[test]
fn la_suite_de_conformite_passe_sur_le_graphe() {
    let dirs: RefCell<Vec<TempDir>> = RefCell::new(Vec::new());
    futures::executor::block_on(this_harness::conformance::check_repo(&|| {
        let dir = TempDir::new().unwrap();
        let store = ObrainStore::open(dir.path()).unwrap();
        dirs.borrow_mut().push(dir);
        let graph: Rc<dyn ToolRepo> = Rc::new(ToolGraph::open(store).unwrap());
        graph
    }));
}

#[test]
fn tout_est_la_apres_reouverture_et_tick_ne_recule_pas() {
    let dir = TempDir::new().unwrap();
    let mut a = record(
        "alpha_un",
        "Liste les factures",
        &["task_list", "note_add"],
        1,
    );
    a.status = ToolStatus::Retired {
        reason: "obsolète".into(),
        replaced_by: Some("bravo_deux".into()),
    };
    let b = record("bravo_deux", "Autre outil", &["task_list"], 2);
    let last_tick;
    {
        let (store, graph) = open_at(dir.path());
        assert_eq!(graph.tick_sync().unwrap(), 1);
        graph.put_sync(&a).unwrap();
        graph.put_sync(&b).unwrap();
        graph.tick_sync().unwrap();
        last_tick = graph.tick_sync().unwrap();
        drop(graph);
        drop(store);
    }
    let (_store, graph) = open_at(dir.path());
    assert_eq!(graph.get_sync("alpha_un").unwrap(), Some(a));
    assert_eq!(graph.get_sync("bravo_deux").unwrap(), Some(b.clone()));
    assert_eq!((graph.len_sync(), graph.active_len_sync()), (2, 1));
    assert_eq!(
        graph
            .active_with_steps_sync(&b.current().steps_hash, None)
            .as_deref(),
        Some("bravo_deux"),
        "les index servent après réouverture"
    );
    assert!(
        graph.tick_sync().unwrap() > last_tick,
        "tick ne revient pas en arrière"
    );
}

#[test]
fn un_schema_inconnu_est_refuse() {
    let dir = TempDir::new().unwrap();
    {
        let store = ObrainStore::open(dir.path()).unwrap();
        let g = store.db().graph_store_mut();
        let meta = g.create_node(&["_tool_meta"]);
        g.set_node_property(meta, "_schema", ObrainValue::from(2_i64));
        g.set_node_property(meta, "_seq", ObrainValue::from(0_i64));
        store.commit_wal().unwrap();
    }
    let store = ObrainStore::open(dir.path()).unwrap();
    let err = ToolGraph::open(store).err().expect("doit refuser");
    assert!(err.contains("v2") && err.contains("v1"), "{err}");
}

#[test]
fn un_meta_sans_schema_est_refuse() {
    let dir = TempDir::new().unwrap();
    {
        let store = ObrainStore::open(dir.path()).unwrap();
        let g = store.db().graph_store_mut();
        let meta = g.create_node(&["_tool_meta"]);
        g.set_node_property(meta, "_seq", ObrainValue::from(0_i64));
        store.commit_wal().unwrap();
    }
    let store = ObrainStore::open(dir.path()).unwrap();
    assert!(ToolGraph::open(store).is_err());
}

#[test]
fn le_meta_est_cree_une_seule_fois() {
    let dir = TempDir::new().unwrap();
    {
        let (store, graph) = open_at(dir.path());
        drop(graph);
        drop(store);
    }
    let (store, _graph) = open_at(dir.path());
    assert_eq!(
        store
            .db()
            .graph_store_mut()
            .node_count_by_label("_tool_meta"),
        1
    );
}

#[test]
fn l_arete_calls_existe_et_le_parcours_repond() {
    let dir = TempDir::new().unwrap();
    let (store, graph) = open_at(dir.path());
    graph
        .put_sync(&record("alpha_un", "a", &["task_list", "note_add"], 1))
        .unwrap();
    graph
        .put_sync(&record("bravo_deux", "b", &["task_list"], 2))
        .unwrap();
    graph
        .put_sync(&record("charlie_trois", "c", &["note_add"], 3))
        .unwrap();

    // Le parcours bas niveau : l'arête CALLS existe, étape -> _host_tool.
    let g = store.db().graph_store_mut();
    let hosts = g.nodes_by_label("_host_tool");
    assert_eq!(hosts.len(), 2, "un _host_tool par nom, partagé");
    let task_list = hosts
        .iter()
        .copied()
        .find(|h| {
            g.get_node_property(*h, &PropertyKey::new("_name"))
                .and_then(|v| v.as_str().map(str::to_string))
                .as_deref()
                == Some("task_list")
        })
        .unwrap();
    let callers = g.edges_from(task_list, Direction::Incoming);
    assert_eq!(callers.len(), 2);
    for (_, edge) in &callers {
        assert_eq!(g.edge_type(*edge).as_deref(), Some("CALLS"));
    }
    assert_eq!(g.node_count_by_label("_tool_step"), 4);

    // « Quels outils d'auteur cassent si task_list disparaît ? »
    assert_eq!(
        graph.authored_calling("task_list"),
        ["alpha_un", "bravo_deux"]
    );
    assert_eq!(
        graph.authored_calling("note_add"),
        ["alpha_un", "charlie_trois"]
    );
    assert!(graph.authored_calling("inconnu").is_empty());

    // Un outil retiré, ou dont la version courante n'appelle plus X, ne casse plus.
    let mut gone = graph.get_sync("bravo_deux").unwrap().unwrap();
    gone.status = ToolStatus::Retired {
        reason: "r".into(),
        replaced_by: None,
    };
    graph.put_sync(&gone).unwrap();
    assert_eq!(graph.authored_calling("task_list"), ["alpha_un"]);
}

#[test]
fn remplacer_ne_laisse_aucun_orphelin() {
    let dir = TempDir::new().unwrap();
    let (store, graph) = open_at(dir.path());
    let g = store.db().graph_store_mut();
    let count = |label: &str| g.node_count_by_label(label);

    // 25 versions : le plafond en garde MAX_VERSIONS, les plus anciennes sortent.
    let mut rec = record("alpha_un", "a", &["task_list", "outil_rare"], 1);
    for v in 2..=25_u32 {
        let mut def = rec.versions.last().unwrap().clone();
        def.version = v;
        def.seq = u64::from(v);
        rec.versions.push(def);
        while rec.versions.len() > MAX_VERSIONS {
            rec.versions.remove(0);
            rec.dropped += 1;
        }
    }
    graph.put_sync(&rec).unwrap();
    assert_eq!(count("_tool_version"), MAX_VERSIONS);
    assert_eq!(count("_tool_step"), MAX_VERSIONS * 2);
    assert_eq!(count("_host_tool"), 2);

    // Remplacer par un enregistrement à une version et une étape : tout le reste disparaît.
    let small = record("alpha_un", "petit", &["task_list"], 99);
    graph.put_sync(&small).unwrap();
    assert_eq!(graph.get_sync("alpha_un").unwrap(), Some(small));
    assert_eq!(graph.len_sync(), 1);
    assert_eq!(count("_tool"), 1);
    assert_eq!(count("_tool_version"), 1);
    assert_eq!(count("_tool_step"), 1);
    assert_eq!(
        count("_host_tool"),
        1,
        "outil_rare n'est plus appelé : sorti"
    );
}

#[test]
fn un_schema_de_forme_libre_revient_exact() {
    let dir = TempDir::new().unwrap();
    let (_store, graph) = open_at(dir.path());
    let mut rec = record("alpha_un", "a", &["task_list"], 1);
    for params in [
        json!({"type": "object", "properties": {}}),
        json!({"type": "object", "properties": {"x": {"type": "string", "description": "d"}},
               "extra": [1, {"y": null}]}),
        json!({"type": "object", "properties": {"a": {"type": "string"}}, "required": []}),
        json!("pas un objet"),
    ] {
        rec.versions[0].parameters = params;
        graph.put_sync(&rec).unwrap();
        assert_eq!(graph.get_sync("alpha_un").unwrap(), Some(rec.clone()));
    }
}

#[test]
fn les_arguments_d_etape_reviennent_exacts() {
    let dir = TempDir::new().unwrap();
    let (_store, graph) = open_at(dir.path());
    let mut rec = record("alpha_un", "a", &["task_list", "note_add", "x_y"], 1);
    rec.versions[0].steps[0].args = serde_json::Value::Null;
    rec.versions[0].steps[1].args = json!({});
    rec.versions[0].steps[2].args =
        json!({"u": "é\n\"guillemets\"", "big": 18446744073709551615u64});
    graph.put_sync(&rec).unwrap();
    assert_eq!(graph.get_sync("alpha_un").unwrap(), Some(rec));
}

#[test]
fn la_source_de_candidats_injectee_est_filtree() {
    let dir = TempDir::new().unwrap();
    let store = ObrainStore::open(dir.path()).unwrap();
    let graph = ToolGraph::open(store)
        .unwrap()
        .with_candidate_source(Arc::new(|_q, _k| {
            vec![
                "charlie_trois".to_string(), // retiré
                "fantome".to_string(),       // inconnu
                "bravo_deux".to_string(),
                "bravo_deux".to_string(), // doublon
                "alpha_un".to_string(),
            ]
        }));
    graph
        .put_sync(&record("alpha_un", "a", &["task_list"], 1))
        .unwrap();
    graph
        .put_sync(&record("bravo_deux", "b", &["task_list"], 2))
        .unwrap();
    let mut c = record("charlie_trois", "c", &["task_list"], 3);
    c.status = ToolStatus::Retired {
        reason: "r".into(),
        replaced_by: None,
    };
    graph.put_sync(&c).unwrap();
    assert_eq!(
        graph.candidates_sync("peu importe", 5),
        ["bravo_deux", "alpha_un"]
    );
    assert_eq!(graph.candidates_sync("peu importe", 1), ["bravo_deux"]);
}

#[test]
fn la_capacite_est_imposee() {
    let dir = TempDir::new().unwrap();
    let store = ObrainStore::open(dir.path()).unwrap();
    let graph = ToolGraph::open(store).unwrap().with_capacity(1);
    graph
        .put_sync(&record("alpha_un", "a", &["task_list"], 1))
        .unwrap();
    assert!(
        graph
            .put_sync(&record("bravo_deux", "b", &["task_list"], 2))
            .is_err()
    );
    assert_eq!(graph.len_sync(), 1);
    // Remplacer un outil existant reste permis à pleine capacité.
    graph
        .put_sync(&record("alpha_un", "a2", &["task_list"], 3))
        .unwrap();
}

#[test]
fn un_put_refuse_ne_change_rien() {
    let dir = TempDir::new().unwrap();
    let a = record("alpha_un", "a", &["task_list"], 1);
    {
        let (store, graph) = open_at(dir.path());
        graph.put_sync(&a).unwrap();
        drop(graph);
        drop(store);
    }
    // Lecture seule : l'écriture est refusée AVANT de toucher au graphe.
    let store = ObrainStore::open_read_only(dir.path()).unwrap();
    let graph = ToolGraph::open(Arc::clone(&store)).unwrap();
    let mut other = a.clone();
    other.versions[0].description = "autre".into();
    assert!(graph.put_sync(&other).is_err());
    assert!(
        graph
            .put_sync(&record("bravo_deux", "b", &["x_y"], 2))
            .is_err()
    );
    assert_eq!(graph.get_sync("alpha_un").unwrap(), Some(a));
    assert_eq!(graph.len_sync(), 1);
    assert_eq!(
        store
            .db()
            .graph_store_mut()
            .node_count_by_label("_host_tool"),
        1
    );
}

#[test]
fn un_outil_illisible_est_une_erreur_pas_une_absence() {
    let dir = TempDir::new().unwrap();
    let (store, graph) = open_at(dir.path());
    graph
        .put_sync(&record("alpha_un", "a", &["task_list"], 1))
        .unwrap();
    let g = store.db().graph_store_mut();
    let step = g.nodes_by_label("_tool_step")[0];
    g.remove_node_property(step, "_args");
    let err = graph.get_sync("alpha_un").unwrap_err();
    assert!(err.contains("illisible"), "{err}");
    assert_eq!(graph.get_sync("inconnu").unwrap(), None);
}

/// Mesure d'échelle, opt-in : `cargo test --release --features obrain --test tool_graph --
/// --ignored --nocapture mesure_d_echelle` (`TOOLGRAPH_SIZES=100,1000,10000` pour changer les
/// paliers). UN seul store rempli progressivement ; à chaque palier, les coûts sont mesurés
/// SÉPARÉMENT : commit seul, `put` complet (donc écriture = put - tick), recherche de doublon,
/// puis lectures.
#[test]
#[ignore = "mesure d'échelle : lente, opt-in"]
fn mesure_d_echelle() {
    use std::time::{Duration, Instant};
    fn du(path: &std::path::Path) -> u64 {
        std::fs::read_dir(path)
            .map(|rd| {
                rd.flatten()
                    .map(|e| match e.metadata() {
                        Ok(m) if m.is_dir() => du(&e.path()),
                        Ok(m) => m.len(),
                        Err(_) => 0,
                    })
                    .sum()
            })
            .unwrap_or(0)
    }
    fn avg(n: u32, mut f: impl FnMut()) -> Duration {
        let t = Instant::now();
        for _ in 0..n {
            f();
        }
        t.elapsed() / n
    }
    let sizes: Vec<usize> = std::env::var("TOOLGRAPH_SIZES")
        .ok()
        .map(|s| s.split(',').filter_map(|n| n.trim().parse().ok()).collect())
        .unwrap_or_else(|| vec![100, 1_000, 10_000]);
    let mk = |i: usize| {
        let mut r = record(
            &format!(
                "outil_{i}_{}",
                ["factures", "notes", "cles", "taches"][i % 4]
            ),
            &format!(
                "Outil numéro {i} qui traite des {}",
                ["factures", "notes", "clés", "tâches"][i % 4]
            ),
            &[&format!("hote_{}", i % 50), "task_list"],
            0,
        );
        let mut v2 = r.versions[0].clone();
        v2.version = 2;
        v2.description.push_str(" (v2)");
        v2.steps[0].args = json!({ "i": i });
        v2.steps_hash = steps_hash(&v2.steps);
        r.versions[0].steps[0].args = json!({ "i": i, "v": 1 });
        r.versions[0].steps_hash = steps_hash(&r.versions[0].steps);
        r.versions.push(v2);
        for (k, d) in r.versions.iter_mut().enumerate() {
            d.seq = (i * 2 + k) as u64;
        }
        r
    };
    let dir = TempDir::new().unwrap();
    let store = ObrainStore::open(dir.path()).unwrap();
    let graph = ToolGraph::open(Arc::clone(&store))
        .unwrap()
        .with_capacity(usize::MAX);
    let (mut filled, mut insert_total) = (0_usize, Duration::ZERO);
    for n in sizes {
        // Remplissage par lots de 100 : un commit par lot.
        let t = Instant::now();
        while filled < n {
            let end = (filled + 100).min(n);
            let batch: Vec<ToolRecord> = (filled..end).map(&mk).collect();
            graph.put_many_sync(&batch).unwrap();
            filled = end;
        }
        let batch_time = t.elapsed();
        insert_total += batch_time;
        println!("\n== {n} outils (2 versions, 2 étapes) ==");
        println!(
            "remplissage jusqu'ici : {insert_total:?} ; ce palier : {batch_time:?} (lots de 100, un commit par lot)"
        );

        // Décomposition : le commit d'abord, puis le put complet.
        let commit_empty = avg(10, || store.commit_wal().unwrap());
        let tick = avg(10, || {
            graph.tick_sync().unwrap();
        });
        let probe = mk(n / 2);
        let put = avg(10, || graph.put_sync(&probe).unwrap());
        println!("commit_wal sans rien de neuf : {commit_empty:?}");
        println!("tick (1 propriété + 1 commit) : {tick:?}");
        println!(
            "put d'UN outil (remplacement, 1 commit) : {put:?} ; écriture seule ~ put - tick = {:?}",
            put.saturating_sub(tick)
        );
        let fresh = n + 1_000_000;
        let mut next = fresh;
        let put_new = avg(10, || {
            graph.put_sync(&mk(next)).unwrap();
            next += 1;
        });
        println!("put d'UN outil neuf (1 commit) : {put_new:?}");

        println!(
            "get : {:?}",
            avg(50, || assert_eq!(
                graph.get_sync(&probe.name).unwrap().as_ref(),
                Some(&probe)
            ))
        );
        let hash = probe.current().steps_hash.clone();
        println!(
            "active_with_steps (doublon, par index _steps_hash) : {:?}",
            avg(50, || assert!(
                graph.active_with_steps_sync(&hash, None).is_some()
            ))
        );
        println!(
            "candidates (balayage) : {:?}",
            avg(3, || assert!(
                !graph
                    .candidates_sync("outil factures numéro", 10)
                    .is_empty()
            ))
        );
        println!(
            "recent(10) (balayage) : {:?}",
            avg(3, || {
                assert_eq!(graph.recent_sync(10).len(), 10);
            })
        );
        println!(
            "len : {:?} ; active_len : {:?}",
            avg(3, || {
                graph.len_sync();
            }),
            avg(3, || {
                graph.active_len_sync();
            })
        );
        println!(
            "authored_calling : {:?}",
            avg(3, || {
                assert!(!graph.authored_calling("hote_7").is_empty());
            })
        );
        println!("taille sur disque : {} Ko", du(dir.path()) / 1024);
        filled = filled.max(n);
        let _ = fresh;
    }
}
