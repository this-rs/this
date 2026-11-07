//! Le harness dans le graphe : composition et journal, écrits comme nœuds.
//!
//! `this_agent::graph` décrit **quoi** écrire (labels, propriétés, conventions) ;
//! ce module dit **où** : dans un `ObrainStore`, sur le même modèle que le
//! backend d'entités (`_uuid` + `_data`, champs recopiés pour la requête).
//!
//! ```text
//! (:_harness {_uuid, _name, _version, _data})
//! (:_run     {_uuid, _goal, _stop, _tours, _data})
//!     ├─[:HAS_STEP {_index}]─▶ (:_step {_uuid, _run, _index, _tool, _refused,
//!     │                                 _failed, _ok, _out_excerpt, _out_hash, _data})
//!     └── …
//! ```
//!
//! L'étape est rattachée à son run **deux fois** : par l'arête `HAS_STEP`
//! (ce qu'un parcours de graphe suit) et par la propriété `_run` (ce que le
//! balayage de `load_run` lit). L'arête est unique par triplet : réécrire une
//! étape met à jour son `_index`, sans doublon.
//!
//! `_out_hash` est le SHA-256 (hex) de la sortie **complète** de l'outil, tel que
//! la boucle de `this-harness` l'a posé dans `Step::out_hash` avant de tronquer
//! le résumé. Pour une étape qui n'en porte pas (journal ancien), c'est le
//! SHA-256 de la sortie gardée (`this_agent::graph::step_output`), possiblement
//! tronquée. Dans les deux cas le format est le même : `sha2` est déjà une dépendance non
//! optionnelle du crate (`core::identity`).
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
use obrain_core::graph::{Direction, GraphStoreMut};
use sha2::{Digest, Sha256};

use this_agent::graph::{HARNESS, HAS_STEP, HarnessDoc, Node, RUN, STEP};
use this_agent::{Run, Step};

use super::obrain::ObrainStore;

/// Le masque d'un hôte : le texte tel qu'il peut être écrit dans le journal.
pub type MasqueFn = dyn Fn(&str) -> String + Send + Sync;

/// Les objets du harness, dans un `ObrainStore`.
pub struct HarnessGraph {
    store: Arc<ObrainStore>,
    /// Appliqué à tout texte libre d'un run avant écriture (`_goal`, `_out_excerpt`,
    /// chaînes de `_data`). Sans masque, le journal est écrit tel quel.
    masque: Option<Arc<MasqueFn>>,
}

impl HarnessGraph {
    pub fn new(store: Arc<ObrainStore>) -> Self {
        Self {
            store,
            masque: None,
        }
    }

    /// Le même graphe, dont le journal passe par `masque` avant d'être écrit.
    ///
    /// C'est ici, au point d'écriture, que le masque doit vivre : tout chemin qui
    /// écrit un run (`record_run`, `append_step`) y passe, quel que soit l'appelant.
    /// Par exemple `this_agent::masque::masquer`.
    pub fn with_masque(mut self, masque: impl Fn(&str) -> String + Send + Sync + 'static) -> Self {
        self.masque = Some(Arc::new(masque));
        self
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
    fn upsert(&self, node: &Node) -> Result<NodeId> {
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
        Ok(node_id)
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

    /// Le nœud d'une étape, avec l'empreinte de sa sortie.
    fn step_node(&self, step: &Step, run_id: &str) -> Node {
        let mut node = match &self.masque {
            Some(masque) => this_agent::graph::step_node_with(step, run_id, masque.as_ref()),
            None => this_agent::graph::step_node(step, run_id),
        };
        // L'empreinte posée par la boucle porte sur la sortie COMPLÈTE ; à défaut
        // (journal ancien, étape écrite à la main), on hache ce que l'étape a
        // gardé — le résumé, possiblement tronqué.
        let hex = step.out_hash.clone().unwrap_or_else(|| {
            Sha256::digest(this_agent::graph::step_output(step).as_bytes())
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect()
        });
        node.props.push(("_out_hash".to_string(), hex));
        node
    }

    /// Pose l'arête `run -HAS_STEP-> step` (propriété `_index`), sans doublon.
    ///
    /// Une arête déjà présente pour ce triplet est réutilisée : seule sa
    /// propriété `_index` est réécrite.
    fn link_step(&self, run: NodeId, step: NodeId, index: u32) {
        let graph = self.graph();
        let existing = graph
            .edges_from(run, Direction::Outgoing)
            .into_iter()
            .find(|(dst, edge)| *dst == step && graph.edge_type(*edge).as_deref() == Some(HAS_STEP))
            .map(|(_, edge)| edge);
        let edge = existing.unwrap_or_else(|| graph.create_edge(run, step, HAS_STEP));
        graph.set_edge_property(
            edge,
            "_index",
            ObrainValue::from(index.to_string().as_str()),
        );
    }

    /// Le nœud de tête d'un run, avec le masque en vigueur (ou tel quel).
    fn run_head_node(&self, run: &Run, run_id: &str) -> Result<Node> {
        let nodes = match &self.masque {
            Some(masque) => this_agent::graph::run_nodes_with(run, run_id, masque.as_ref()),
            None => this_agent::graph::run_nodes(run, run_id),
        }
        .map_err(anyhow::Error::msg)?;
        nodes
            .into_iter()
            .find(|node| node.label == RUN)
            .ok_or_else(|| anyhow!("run {run_id} : pas de nœud de tête"))
    }

    /// Écrit un run **entier** : son nœud et toutes ses étapes, en un commit.
    pub fn record_run(&self, run: &Run, run_id: &str) -> Result<()> {
        self.writable()?;
        let head_id = self.upsert(&self.run_head_node(run, run_id)?)?;
        for step in &run.steps {
            let step_id = self.upsert(&self.step_node(step, run_id))?;
            self.link_step(head_id, step_id, step.index);
        }
        self.store.commit_wal()
    }

    /// Ajoute **une** étape à un run déjà écrit, et commite.
    ///
    /// C'est le chemin d'un run vivant : l'hôte voit les étapes arriver. Le
    /// nœud de run voit son `_stop` et son `_tours` mis à jour au passage, sans
    /// quoi le graphe raconterait un run éternellement « en cours ».
    ///
    /// Le nœud de run doit exister (`record_run` ou [`Self::append_step_live`]
    /// d'abord) : une étape sur un run inconnu est refusée, pas inventée.
    pub fn append_step(&self, run: &Run, run_id: &str, step: &Step) -> Result<()> {
        self.writable()?;
        // Le run d'abord : une étape sur un run inconnu est refusée avant
        // d'écrire quoi que ce soit (pas d'étape orpheline).
        let head_id = self
            .find(RUN, run_id)
            .ok_or_else(|| anyhow!("run {run_id} inconnu : record_run d'abord"))?;
        self.append_step_to(head_id, run, run_id, step)
    }

    /// [`Self::append_step`], en **matérialisant** le nœud de run au premier
    /// passage : c'est le chemin du `Recorder` vivant, qui ne connaît pas de
    /// « run démarré » — la première étape fait exister le run.
    ///
    /// Un run sans aucune étape (annulé avant le premier outil) n'est donc pas
    /// écrit par ce chemin ; `record_run` le couvre à l'arrêt.
    pub fn append_step_live(&self, run: &Run, run_id: &str, step: &Step) -> Result<()> {
        self.writable()?;
        let head_id = match self.find(RUN, run_id) {
            Some(id) => id,
            None => self.upsert(&self.run_head_node(run, run_id)?)?,
        };
        self.append_step_to(head_id, run, run_id, step)
    }

    /// La part commune des deux chemins d'ajout : l'étape, son arête, puis le
    /// nœud de run mis à jour (`_stop`, `_tours`, `_data`), en un commit.
    fn append_step_to(&self, head_id: NodeId, run: &Run, run_id: &str, step: &Step) -> Result<()> {
        let step_id = self.upsert(&self.step_node(step, run_id))?;
        self.link_step(head_id, step_id, step.index);
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
        let data = match &self.masque {
            Some(masque) => {
                this_agent::graph::masked_json(run, masque.as_ref()).map_err(anyhow::Error::msg)?
            }
            None => serde_json::to_string(run)?,
        };
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
            silence_ms: None,
            no_progress_ms: None,
            tool_timeout_ms: None,
            context_budget: None,
            approval: None,
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

    /// Les arêtes `HAS_STEP` sortant du run : (étape `_uuid`, `_index` de l'arête).
    fn has_step_edges(graph: &HarnessGraph, run_id: &str) -> Vec<(String, String)> {
        let g = graph.graph();
        let run = graph.find(RUN, run_id).expect("le run est là");
        let mut out: Vec<(String, String)> = g
            .edges_from(run, Direction::Outgoing)
            .into_iter()
            .filter(|(_, e)| g.edge_type(*e).as_deref() == Some(HAS_STEP))
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

    fn run_a_deux_etapes() -> Run {
        let mut run = Run::new(Goal::new("prouve"), 6);
        run.record(Step::tool("lire", 2).state("a", "b").resume("contenu lu"));
        run.record(Step::tool("ecrire", 3).refused("même version"));
        run
    }

    #[tokio::test]
    async fn record_run_ecrit_les_aretes_has_step() {
        let dir = temp_dir("edges");
        let store = ObrainStore::open(&dir).unwrap();
        let graph = HarnessGraph::new(Arc::clone(&store));
        assert_eq!(graph.graph().edge_count(), 0);

        graph.record_run(&run_a_deux_etapes(), "run-e").unwrap();
        assert!(graph.graph().edge_count() > 0, "des arêtes ont été écrites");
        assert_eq!(
            has_step_edges(&graph, "run-e"),
            vec![
                ("run-e#0".to_string(), "0".to_string()),
                ("run-e#1".to_string(), "1".to_string())
            ]
        );
        drop(graph);
        drop(store);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn reecrire_un_run_ne_duplique_pas_les_aretes() {
        let dir = temp_dir("dedup");
        let store = ObrainStore::open(&dir).unwrap();
        let graph = HarnessGraph::new(Arc::clone(&store));
        let mut run = run_a_deux_etapes();
        graph.record_run(&run, "run-d").unwrap();
        graph.record_run(&run, "run-d").unwrap();
        // Une étape réécrite par le chemin vivant ne double pas non plus.
        graph.append_step(&run, "run-d", &run.steps[1]).unwrap();
        assert_eq!(has_step_edges(&graph, "run-d").len(), 2, "pas de doublon");
        assert_eq!(graph.graph().edge_count(), 2);

        // Une étape nouvelle, elle, ajoute son arête.
        run.record(Step::tool("verifier", 1).resume("ok"));
        graph
            .append_step(&run, "run-d", run.steps.last().unwrap())
            .unwrap();
        assert_eq!(has_step_edges(&graph, "run-d").len(), 3);
        // Un run inconnu ne laisse ni étape ni arête.
        assert!(graph.append_step(&run, "run-x", &run.steps[0]).is_err());
        assert_eq!(graph.graph().edge_count(), 3);
        assert!(
            graph.find(STEP, "run-x#0").is_none(),
            "pas d'étape orpheline"
        );
        drop(graph);
        drop(store);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn les_aretes_has_step_survivent_a_la_reouverture() {
        let dir = temp_dir("edges_reopen");
        {
            let store = ObrainStore::open(&dir).unwrap();
            let graph = HarnessGraph::new(Arc::clone(&store));
            graph.record_run(&run_a_deux_etapes(), "run-r").unwrap();
            assert_eq!(has_step_edges(&graph, "run-r").len(), 2);
        }
        let store = ObrainStore::open(&dir).unwrap();
        let graph = HarnessGraph::new(Arc::clone(&store));
        assert_eq!(graph.graph().edge_count(), 2, "les arêtes sont sur disque");
        assert_eq!(
            has_step_edges(&graph, "run-r"),
            vec![
                ("run-r#0".to_string(), "0".to_string()),
                ("run-r#1".to_string(), "1".to_string())
            ],
            "l'arête et sa propriété _index se relisent après réouverture"
        );
        drop(graph);
        drop(store);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn une_etape_porte_ses_preuves_dans_le_graphe() {
        let dir = temp_dir("preuves");
        let store = ObrainStore::open(&dir).unwrap();
        let graph = HarnessGraph::new(Arc::clone(&store));
        graph.record_run(&run_a_deux_etapes(), "run-p").unwrap();
        let g = graph.graph();
        let prop = |uuid: &str, key: &str| {
            let id = graph.find(STEP, uuid).unwrap();
            g.get_node(id).and_then(|n| {
                n.get_property(key)
                    .and_then(|v| v.as_str().map(str::to_string))
            })
        };
        assert_eq!(prop("run-p#0", "_ok").as_deref(), Some("true"));
        assert_eq!(
            prop("run-p#0", "_out_excerpt").as_deref(),
            Some("contenu lu")
        );
        let attendu: String = Sha256::digest(b"contenu lu")
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        assert_eq!(prop("run-p#0", "_out_hash"), Some(attendu));
        assert_eq!(prop("run-p#1", "_ok").as_deref(), Some("false"));
        drop(graph);
        drop(store);
        std::fs::remove_dir_all(&dir).ok();
    }

    /// Une étape qui porte l'empreinte de sa sortie complète (posée par la
    /// boucle) l'écrit telle quelle : le résumé tronqué n'est pas re-haché.
    #[tokio::test]
    async fn le_out_hash_de_l_etape_fait_foi() {
        let dir = temp_dir("out-hash");
        let store = ObrainStore::open(&dir).unwrap();
        let graph = HarnessGraph::new(Arc::clone(&store));
        let mut run = Run::new(Goal::new("lire"), 5);
        run.record(
            Step::tool("recall", 1)
                .resume("début tronqué")
                .out_hash("abc123"),
        );
        graph.record_run(&run, "run-h").unwrap();
        let g = graph.graph();
        let id = graph.find(STEP, "run-h#0").unwrap();
        let hash = g.get_node(id).and_then(|n| {
            n.get_property("_out_hash")
                .and_then(|v| v.as_str().map(str::to_string))
        });
        assert_eq!(hash.as_deref(), Some("abc123"));
        drop(graph);
        drop(store);
        std::fs::remove_dir_all(&dir).ok();
    }

    const CLE: &str = "sk-ant-api03-abcdefghijklmnopqrstuvwxyz";

    /// Toutes les propriétés textuelles des nœuds `_run` et `_step` du magasin.
    fn textes_du_journal(graph: &HarnessGraph) -> Vec<String> {
        let g = graph.graph();
        let mut out = Vec::new();
        for label in [RUN, STEP] {
            for id in g.nodes_by_label(label) {
                let node = g.get_node(id).unwrap();
                for key in ["_goal", "_out_excerpt", "_data"] {
                    if let Some(v) = node.get_property(key).and_then(|v| v.as_str()) {
                        out.push(v.to_string());
                    }
                }
            }
        }
        out
    }

    fn run_avec_cle() -> Run {
        let mut run = Run::new(Goal::new(format!("utilise {CLE}")), 6);
        run.record(Step::tool("lire_env", 2).resume(format!("OPENAI={CLE}")));
        run
    }

    #[tokio::test]
    async fn un_journal_masque_n_ecrit_pas_la_cle() {
        let dir = temp_dir("masque");
        let store = ObrainStore::open(&dir).unwrap();
        let graph = HarnessGraph::new(Arc::clone(&store)).with_masque(this_agent::masque::masquer);
        let mut run = run_avec_cle();
        graph.record_run(&run, "run-m").unwrap();
        run.record(Step::tool("ecrire", 1).failed(format!("refus {CLE}")));
        graph
            .append_step(&run, "run-m", run.steps.last().unwrap())
            .unwrap();

        let textes = textes_du_journal(&graph);
        assert!(textes.len() >= 5, "{textes:?}");
        for t in &textes {
            assert!(!t.contains("abcdefghij"), "secret écrit : {t}");
        }
        let excerpt = graph
            .read_node(&Node {
                label: STEP,
                props: vec![
                    ("_uuid".to_string(), "run-m#0".to_string()),
                    ("_out_excerpt".to_string(), String::new()),
                ],
            })
            .unwrap();
        assert_eq!(excerpt.get("_out_excerpt"), Some("OPENAI=[secret masqué]"));
        let back = graph.load_run("run-m").unwrap().unwrap();
        assert_eq!(back.goal.text, "utilise [secret masqué]");
        assert_eq!(back.steps[0].resume, "OPENAI=[secret masqué]");
        assert_eq!(
            back.steps[1].failure.as_deref(),
            Some("refus [secret masqué]")
        );
        drop(graph);
        drop(store);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn sans_masque_le_journal_est_ecrit_tel_quel() {
        let dir = temp_dir("sans_masque");
        let store = ObrainStore::open(&dir).unwrap();
        let graph = HarnessGraph::new(Arc::clone(&store));
        let run = run_avec_cle();
        graph.record_run(&run, "run-s").unwrap();
        let back = graph.load_run("run-s").unwrap().unwrap();
        assert_eq!(back, run, "le run relu est le run écrit");
        assert!(textes_du_journal(&graph).iter().any(|t| t.contains(CLE)));
        drop(graph);
        drop(store);
        std::fs::remove_dir_all(&dir).ok();
    }
}
