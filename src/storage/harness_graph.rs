//! Le harness dans le graphe : composition et journal, écrits comme nœuds.
//!
//! `this_agent::graph` décrit **quoi** écrire (labels, propriétés, conventions) ;
//! ce module dit **où** : dans un `ObrainStore`, sur le même modèle que le
//! backend d'entités (`_uuid` + `_data`, champs recopiés pour la requête).
//!
//! ```text
//! (:_harness {_uuid, _name, _version, _data})
//! (:_run     {_uuid, _goal, _stop, _tours, _data})
//!     ├── (:_step {_uuid, _run, _index, _tool, _refused, _data})
//!     └── …
//! ```
//!
//! # Une écriture par lot, pas une par nœud
//!
//! `commit_wal()` coûte ~35 ms médians sur APFS (mesuré, voir le magasin de
//! lignes déclaratives) : committer chaque étape d'un run de vingt étapes
//! coûterait 700 ms pour un journal que personne ne lit avant la fin. Un run
//! **complet** s'écrit donc en un seul commit ; un run **vivant** s'ajoute étape
//! par étape, parce que c'est précisément l'intérêt de le voir arriver.
//!
//! # Un seul écrivain
//!
//! Le store est partagé (`Arc<ObrainStore>`), jamais rouvert : obrain n'admet
//! qu'une instance en écriture par base et par processus (`OBRAIN-X001`).
//!
//! # Ce qui n'est pas indexé
//!
//! La recherche d'un nœud se fait par balayage du label. Pour un harness — des
//! dizaines de compositions, des milliers d'étapes — c'est le bon compromis :
//! pas d'index à reconstruire après une réouverture, donc pas de piège. Le jour
//! où un journal dépasse la centaine de milliers d'étapes, `create_property_index`
//! sur `_run` est la sortie, et elle est explicite.

use std::sync::Arc;

use anyhow::{Result, anyhow};
use obrain_common::types::{NodeId, Value as ObrainValue};
use obrain_core::graph::GraphStoreMut;

use this_agent::graph::{HARNESS, HarnessDoc, Node, RUN, STEP};
use this_agent::{Run, Step};

use super::obrain::ObrainStore;

/// Les objets du harness, dans un `ObrainStore`.
pub struct HarnessGraph {
    store: Arc<ObrainStore>,
}

impl HarnessGraph {
    pub fn new(store: Arc<ObrainStore>) -> Self {
        Self { store }
    }

    fn graph(&self) -> Arc<dyn GraphStoreMut> {
        self.store.db().graph_store_mut()
    }

    fn writable(&self) -> Result<()> {
        if self.store.is_read_only() {
            return Err(anyhow!("harness graph: the database is read-only"));
        }
        self.store.check_write()
    }

    /// Trouve le nœud d'un label dont `_uuid` vaut `id`.
    fn find(&self, label: &str, id: &str) -> Option<NodeId> {
        let graph = self.graph();
        for node_id in graph.nodes_by_label(label) {
            let Some(node) = graph.get_node(node_id) else {
                continue;
            };
            if node.get_property("_uuid").and_then(|v| v.as_str()) == Some(id) {
                return Some(node_id);
            }
        }
        None
    }

    /// Relit un nœud sous la forme du modèle pur.
    fn read_node(&self, node: &Node) -> Option<Node> {
        let graph = self.graph();
        let id = self.find(node.label, node.get("_uuid")?)?;
        let found = graph.get_node(id)?;
        let mut props = Vec::new();
        for (key, _) in &node.props {
            if let Some(value) = found.get_property(key).and_then(|v| v.as_str()) {
                props.push((key.clone(), value.to_string()));
            }
        }
        Some(Node {
            label: node.label,
            props,
        })
    }

    /// Écrit un nœud : le met à jour s'il existe déjà, le crée sinon.
    ///
    /// `_data` est posé **en dernier** : une création interrompue laisse un
    /// nœud sans `_data`, que la lecture ignore — jamais une composition
    /// tronquée servie à l'hôte.
    fn upsert(&self, node: &Node) -> Result<()> {
        let graph = self.graph();
        let uuid = node
            .get("_uuid")
            .ok_or_else(|| anyhow!("nœud sans _uuid"))?;
        let node_id = match self.find(node.label, uuid) {
            Some(id) => id,
            None => graph.create_node(&[node.label]),
        };
        for (key, value) in node.props.iter().filter(|(key, _)| key != "_data") {
            graph.set_node_property(node_id, key, ObrainValue::from(value.as_str()));
        }
        if let Some((_, data)) = node.props.iter().find(|(key, _)| key == "_data") {
            graph.set_node_property(node_id, "_data", ObrainValue::from(data.as_str()));
        }
        Ok(())
    }

    // ---- compositions ------------------------------------------------------

    /// Écrit (ou remplace) une composition de harness.
    pub fn save(&self, doc: &HarnessDoc) -> Result<()> {
        self.writable()?;
        // Une composition acceptée s'écrit en un commit : elle n'a pas de sens
        // à moitié écrite.
        self.upsert(&doc.node().map_err(anyhow::Error::msg)?)?;
        self.store.commit_wal()
    }

    /// Relit une composition par son identité.
    pub fn load(&self, id: &str) -> Result<Option<HarnessDoc>> {
        let probe = Node {
            label: HARNESS,
            props: vec![
                ("_uuid".to_string(), id.to_string()),
                ("_data".to_string(), String::new()),
            ],
        };
        let Some(node) = self.read_node(&probe) else {
            return Ok(None);
        };
        HarnessDoc::from_node(&node)
            .map(Some)
            .map_err(anyhow::Error::msg)
    }

    /// Les compositions présentes : identité, nom, version.
    pub fn list(&self) -> Result<Vec<(String, String, u32)>> {
        let graph = self.graph();
        let mut out = Vec::new();
        for node_id in graph.nodes_by_label(HARNESS) {
            let Some(node) = graph.get_node(node_id) else {
                continue;
            };
            let s = |k: &str| {
                node.get_property(k)
                    .and_then(|v| v.as_str())
                    .map(str::to_string)
            };
            let (Some(id), Some(name)) = (s("_uuid"), s("_name")) else {
                continue;
            };
            let version = s("_version").and_then(|v| v.parse().ok()).unwrap_or(0);
            out.push((id, name, version));
        }
        out.sort();
        Ok(out)
    }

    // ---- journal -----------------------------------------------------------

    /// Écrit un run **entier** : son nœud et toutes ses étapes, en un commit.
    pub fn record_run(&self, run: &Run, run_id: &str) -> Result<()> {
        self.writable()?;
        for node in this_agent::graph::run_nodes(run, run_id).map_err(anyhow::Error::msg)? {
            self.upsert(&node)?;
        }
        self.store.commit_wal()
    }

    /// Ajoute **une** étape à un run déjà écrit, et commite.
    ///
    /// C'est le chemin d'un run vivant : l'hôte voit les étapes arriver. Le
    /// nœud de run voit son `_stop` et son `_tours` mis à jour au passage, sans
    /// quoi le graphe raconterait un run éternellement « en cours ».
    pub fn append_step(&self, run: &Run, run_id: &str, step: &Step) -> Result<()> {
        self.writable()?;
        self.upsert(&this_agent::graph::step_node(step, run_id))?;
        // Le nœud de run suit : `_stop`, `_tours`, et le décompte d'étapes.
        let mut head = self
            .read_node(&Node {
                label: RUN,
                props: vec![
                    ("_uuid".to_string(), run_id.to_string()),
                    ("_stop".to_string(), String::new()),
                    ("_tours".to_string(), String::new()),
                    ("_data".to_string(), String::new()),
                ],
            })
            .ok_or_else(|| anyhow!("run {run_id} inconnu : record_run d'abord"))?;
        let data = serde_json::to_string(run)?;
        let stop = run
            .stop
            .as_ref()
            .map(this_agent::Stop::code)
            .unwrap_or("running")
            .to_string();
        for (key, value) in [
            ("_stop", stop),
            ("_tours", run.tours.to_string()),
            ("_data", data),
        ] {
            if let Some(slot) = head.props.iter_mut().find(|(k, _)| k == key) {
                slot.1 = value;
            } else {
                head.props.push((key.to_string(), value));
            }
        }
        self.upsert(&head)?;
        self.store.commit_wal()
    }

    /// Relit un run et ses étapes, dans l'ordre de `_index`.
    pub fn load_run(&self, run_id: &str) -> Result<Option<Run>> {
        let graph = self.graph();
        let mut nodes = Vec::new();

        let probe = Node {
            label: RUN,
            props: vec![
                ("_uuid".to_string(), run_id.to_string()),
                ("_data".to_string(), String::new()),
            ],
        };
        let Some(head) = self.read_node(&probe) else {
            return Ok(None);
        };
        nodes.push(head);

        for node_id in graph.nodes_by_label(STEP) {
            let Some(node) = graph.get_node(node_id) else {
                continue;
            };
            if node.get_property("_run").and_then(|v| v.as_str()) != Some(run_id) {
                continue;
            }
            let mut props = Vec::new();
            for key in [
                "_uuid", "_run", "_index", "_tool", "_refused", "_ms", "_data",
            ] {
                if let Some(value) = node.get_property(key).and_then(|v| v.as_str()) {
                    props.push((key.to_string(), value.to_string()));
                }
            }
            nodes.push(Node { label: STEP, props });
        }

        this_agent::graph::run_from_nodes(&nodes).map_err(anyhow::Error::msg)
    }
}

impl std::fmt::Debug for HarnessGraph {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HarnessGraph").finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use this_agent::graph::{Slot, SlotKind};
    use this_agent::{Goal, Step};

    fn doc(version: u32) -> HarnessDoc {
        HarnessDoc {
            id: "h-atelier".into(),
            name: "atelier".into(),
            version,
            brief: "tu es l'atelier".into(),
            slots: vec![Slot {
                kind: SlotKind::Policy,
                module: "harness-default".into(),
                version: "0.1.0".into(),
                entry: "policy".into(),
            }],
            max_tours: 12,
            max_relances: 3,
            budget_ms: None,
        }
    }

    fn temp_dir(tag: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("this_hg_{tag}_{}", uuid::Uuid::new_v4()))
    }

    #[tokio::test]
    async fn une_composition_s_ecrit_et_se_relit() {
        let dir = temp_dir("doc");
        let store = ObrainStore::open(&dir).unwrap();
        let graph = HarnessGraph::new(Arc::clone(&store));
        graph.save(&doc(1)).unwrap();

        let back = graph
            .load("h-atelier")
            .unwrap()
            .expect("la composition est là");
        assert_eq!(back, doc(1));
        assert_eq!(
            graph.list().unwrap(),
            vec![("h-atelier".to_string(), "atelier".to_string(), 1)]
        );

        // Une nouvelle version remplace la précédente, sans doublon.
        graph.save(&doc(2)).unwrap();
        assert_eq!(graph.list().unwrap().len(), 1, "pas de second nœud");
        assert_eq!(graph.load("h-atelier").unwrap().unwrap().version, 2);
        drop(graph);
        drop(store);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn une_composition_survit_a_la_reouverture() {
        let dir = temp_dir("reopen");
        {
            let store = ObrainStore::open(&dir).unwrap();
            HarnessGraph::new(Arc::clone(&store)).save(&doc(3)).unwrap();
        }
        let store = ObrainStore::open(&dir).unwrap();
        let graph = HarnessGraph::new(Arc::clone(&store));
        assert_eq!(graph.load("h-atelier").unwrap().unwrap().version, 3);
        drop(graph);
        drop(store);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn un_run_se_relit_avec_ses_etapes_dans_l_ordre() {
        let dir = temp_dir("run");
        let store = ObrainStore::open(&dir).unwrap();
        let graph = HarnessGraph::new(Arc::clone(&store));

        let mut run = Run::new(Goal::new("installe le module conversation"), 6);
        run.record(Step::tool("install_module", 1840).state("a", "b"));
        run.record(Step::tool("list_modules", 3).refused("même version"));
        run.after_turn(true, false);

        graph.record_run(&run, "run-1").unwrap();
        let back = graph.load_run("run-1").unwrap().expect("le run est là");
        assert_eq!(back.goal.text, run.goal.text);
        assert_eq!(back.tours, run.tours);
        assert_eq!(back.stop, run.stop);
        let tools: Vec<&str> = back.steps.iter().map(|s| s.tool.as_str()).collect();
        assert_eq!(tools, ["install_module", "list_modules"]);
        assert!(back.steps[1].refused);
        assert!(back.steps[0].a_change(), "les empreintes ont suivi");
        drop(graph);
        drop(store);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn une_etape_ajoutee_met_le_run_a_jour() {
        let dir = temp_dir("step");
        let store = ObrainStore::open(&dir).unwrap();
        let graph = HarnessGraph::new(Arc::clone(&store));

        let mut run = Run::new(Goal::new("avance"), 6);
        graph.record_run(&run, "run-2").unwrap();
        assert_eq!(graph.load_run("run-2").unwrap().unwrap().steps.len(), 0);

        // Le run vit : une étape arrive, puis une autre.
        for (name, ms) in [("a", 5u64), ("b", 7)] {
            run.record(Step::tool(name, ms).state("x", "y"));
            graph
                .append_step(&run, "run-2", run.steps.last().unwrap())
                .unwrap();
        }
        let back = graph.load_run("run-2").unwrap().unwrap();
        assert_eq!(back.steps.len(), 2);
        assert_eq!(back.steps[1].tool, "b");

        // Une étape sur un run inconnu est refusée, pas inventée.
        assert!(
            graph
                .append_step(&run, "run-inconnu", run.steps.last().unwrap())
                .is_err()
        );
        drop(graph);
        drop(store);
        std::fs::remove_dir_all(&dir).ok();
    }
}
