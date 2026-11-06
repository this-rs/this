//! Le substrat de l'agent : contexte et autorisations, écrits comme nœuds.
//!
//! `this_agent::substrate` décrit **quoi** (sections typées, budget honnête,
//! autorisations fail-closed) ; ce module dit **où** : dans un `ObrainStore`,
//! avec les mêmes conventions que le harness (`_uuid` + `_data`, champs recopiés
//! pour la requête).
//!
//! ```text
//! (:_context {_uuid: "<agent>#<id>", _agent, _kind, _priority, _at, _data})
//! (:_grant   {_uuid: "<agent>#<outil>", _agent, _tool, _granted, _source, _data})
//! ```
//!
//! L'identité inclut l'agent : deux agents peuvent avoir la même section ou la
//! même autorisation sans se marcher dessus, et un nœud se retrouve par
//! `MATCH (c:_context {_agent: 'atelier'})`.
//!
//! # Une écriture par lot
//!
//! Un contexte de vingt sections s'écrit en **un** commit : `commit_wal()` coûte
//! ~35 ms médians, et un assemblage sans son contexte complet ne sert à rien à
//! personne. Idem pour un jeu d'autorisations : à moitié appliqué, il accorde
//! ou retire au hasard.

use std::sync::Arc;

use anyhow::{Result, anyhow};
use obrain_common::types::{NodeId, Value as ObrainValue};
use obrain_core::graph::GraphStoreMut;

use this_agent::substrate::{CONTEXT, Context, GRANT, Grants, Node, Rendered};

use super::obrain::ObrainStore;

/// Le contexte et les autorisations d'un agent, dans un `ObrainStore`.
pub struct AgentSubstrate {
    store: Arc<ObrainStore>,
}

impl AgentSubstrate {
    pub fn new(store: Arc<ObrainStore>) -> Self {
        Self { store }
    }

    fn graph(&self) -> Arc<dyn GraphStoreMut> {
        self.store.db().graph_store_mut()
    }

    fn writable(&self) -> Result<()> {
        if self.store.is_read_only() {
            return Err(anyhow!("agent substrate: the database is read-only"));
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

    /// Écrit un nœud : mise à jour s'il existe, création sinon. `_data` en
    /// dernier, pour qu'une création interrompue ne soit jamais servie.
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

    /// Écrit (ou remplace) tout le contexte d'un agent, en un commit.
    ///
    /// Les sections retirées du `Context` sont **retirées du graphe** : un
    /// contexte qui ne fait qu'ajouter laisse des sections que l'agent ne voit
    /// plus mais qui restent interrogeables, donc deux vérités.
    pub fn save_context(&self, context: &Context) -> Result<()> {
        self.writable()?;
        let wanted: Vec<String> = context
            .nodes()
            .map_err(anyhow::Error::msg)?
            .into_iter()
            .map(|node| node.get("_uuid").unwrap_or_default().to_string())
            .collect();
        for node in context.nodes().map_err(anyhow::Error::msg)? {
            self.upsert(&node)?;
        }
        for node_id in self.graph().nodes_by_label(CONTEXT) {
            let Some(node) = self.graph().get_node(node_id) else {
                continue;
            };
            if node.get_property("_agent").and_then(|v| v.as_str()) != Some(&context.agent) {
                continue;
            }
            let uuid = node
                .get_property("_uuid")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string();
            if !wanted.contains(&uuid) {
                self.graph().delete_node(node_id);
            }
        }
        self.store.commit_wal()
    }

    /// Relit le contexte d'un agent. `base` n'est pas stocké avec les sections
    /// (c'est le brief de la composition) : l'appelant le fournit.
    pub fn context(&self, agent: &str, base: &str) -> Result<Context> {
        let mut nodes = Vec::new();
        let graph = self.graph();
        for node_id in graph.nodes_by_label(CONTEXT) {
            let Some(node) = graph.get_node(node_id) else {
                continue;
            };
            if node.get_property("_agent").and_then(|v| v.as_str()) != Some(agent) {
                continue;
            }
            let mut props = Vec::new();
            for key in ["_uuid", "_agent", "_kind", "_priority", "_at", "_data"] {
                if let Some(value) = node.get_property(key).and_then(|v| v.as_str()) {
                    props.push((key.to_string(), value.to_string()));
                }
            }
            nodes.push(Node {
                label: CONTEXT,
                props,
            });
        }
        Context::from_nodes(agent, base, &nodes).map_err(anyhow::Error::msg)
    }

    /// Le contexte assemblé, dans un budget de caractères.
    pub fn render(&self, agent: &str, base: &str, budget_chars: usize) -> Result<Rendered> {
        Ok(self.context(agent, base)?.render(budget_chars))
    }

    /// Écrit le jeu d'autorisations d'un agent, en un commit. Même règle que le
    /// contexte : ce qui n'est plus accordé est **retiré**, pas laissé derrière.
    pub fn save_grants(&self, agent: &str, grants: &Grants) -> Result<()> {
        self.writable()?;
        let wanted: Vec<String> = grants
            .grants
            .iter()
            .map(|grant| format!("{agent}#{}", grant.tool))
            .collect();
        for node in grants.nodes(agent).map_err(anyhow::Error::msg)? {
            self.upsert(&node)?;
        }
        for node_id in self.graph().nodes_by_label(GRANT) {
            let Some(node) = self.graph().get_node(node_id) else {
                continue;
            };
            if node.get_property("_agent").and_then(|v| v.as_str()) != Some(agent) {
                continue;
            }
            let uuid = node
                .get_property("_uuid")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string();
            if !wanted.contains(&uuid) {
                self.graph().delete_node(node_id);
            }
        }
        self.store.commit_wal()
    }

    /// Relit les autorisations d'un agent.
    pub fn grants(&self, agent: &str) -> Result<Grants> {
        let mut nodes = Vec::new();
        let graph = self.graph();
        for node_id in graph.nodes_by_label(GRANT) {
            let Some(node) = graph.get_node(node_id) else {
                continue;
            };
            if node.get_property("_agent").and_then(|v| v.as_str()) != Some(agent) {
                continue;
            }
            let mut props = Vec::new();
            for key in ["_uuid", "_agent", "_tool", "_granted", "_source", "_data"] {
                if let Some(value) = node.get_property(key).and_then(|v| v.as_str()) {
                    props.push((key.to_string(), value.to_string()));
                }
            }
            nodes.push(Node {
                label: GRANT,
                props,
            });
        }
        Grants::from_nodes(&nodes).map_err(anyhow::Error::msg)
    }
}

impl std::fmt::Debug for AgentSubstrate {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentSubstrate").finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use this_agent::substrate::{Grant, Section, SectionKind, Verdict};

    fn section(id: &str, priority: u8, at_ms: u64, body: &str) -> Section {
        Section {
            id: id.into(),
            kind: SectionKind::Fact,
            title: format!("titre {id}"),
            body: body.to_string(),
            priority,
            at_ms,
            source: "test".into(),
        }
    }

    fn temp_dir(tag: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("this_sub_{tag}_{}", uuid::Uuid::new_v4()))
    }

    #[tokio::test]
    async fn un_contexte_s_ecrit_se_relit_et_s_assemble() {
        let dir = temp_dir("ctx");
        let store = ObrainStore::open(&dir).unwrap();
        let substrate = AgentSubstrate::new(Arc::clone(&store));

        let mut ctx = Context::new("atelier", "tu es l'atelier");
        ctx.put(section(
            "descriptor",
            10,
            1_000,
            "deux entités : client, commande",
        ));
        ctx.put(section("registry", 20, 2_000, "shop-core 0.3.0 installé"));
        substrate.save_context(&ctx).unwrap();

        let back = substrate.context("atelier", "tu es l'atelier").unwrap();
        assert_eq!(back.sections.len(), 2);

        let rendered = substrate
            .render("atelier", "tu es l'atelier", 10_000)
            .unwrap();
        assert!(rendered.text.contains("tu es l'atelier"));
        assert!(
            rendered.text.contains("shop-core"),
            "le registre est bien injecté"
        );
        assert!(rendered.dropped.is_empty());

        // Un budget trop court coupe ET le dit.
        let serre = substrate.render("atelier", "tu es l'atelier", 30).unwrap();
        assert!(!serre.dropped.is_empty());
        assert!(serre.text.contains("omise"));
        drop(substrate);
        drop(store);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn une_section_retiree_disparait_du_graphe() {
        let dir = temp_dir("retrait");
        let store = ObrainStore::open(&dir).unwrap();
        let substrate = AgentSubstrate::new(Arc::clone(&store));

        let mut ctx = Context::new("atelier", "base");
        ctx.put(section("a", 10, 1, "un"));
        ctx.put(section("b", 20, 2, "deux"));
        substrate.save_context(&ctx).unwrap();
        assert_eq!(
            substrate.context("atelier", "base").unwrap().sections.len(),
            2
        );

        // On retire "b" : il ne doit pas rester un nœud fantôme interrogeable.
        let mut reduit = Context::new("atelier", "base");
        reduit.put(section("a", 10, 1, "un"));
        substrate.save_context(&reduit).unwrap();
        let back = substrate.context("atelier", "base").unwrap();
        assert_eq!(back.sections.len(), 1);
        assert_eq!(back.sections[0].id, "a");
        drop(substrate);
        drop(store);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn les_autorisations_sont_fail_closed_et_survivent() {
        let dir = temp_dir("grants");
        let (granted, retracted) = {
            let store = ObrainStore::open(&dir).unwrap();
            let substrate = AgentSubstrate::new(Arc::clone(&store));
            let mut grants = Grants::new();
            grants.put(Grant::granted("install_module", "user"));
            grants.put(Grant::refused(
                "run_program",
                "descriptor",
                "module non signé",
            ));
            substrate.save_grants("atelier", &grants).unwrap();
            drop(substrate);
            drop(store);
            ((), ())
        };
        let _ = (granted, retracted);

        let store = ObrainStore::open(&dir).unwrap();
        let substrate = AgentSubstrate::new(Arc::clone(&store));
        let back = substrate.grants("atelier").unwrap();
        assert_eq!(back.allows("install_module"), Verdict::Allow);
        match back.allows("run_program") {
            Verdict::Refuse(raison) => assert_eq!(raison, "module non signé"),
            Verdict::Allow => panic!("une autorisation retirée ne remonte pas"),
        }
        match back.allows("jamais_vu") {
            Verdict::Refuse(raison) => assert!(raison.contains("jamais_vu")),
            Verdict::Allow => panic!("fail-closed"),
        }
        drop(substrate);
        drop(store);
        std::fs::remove_dir_all(&dir).ok();
    }
}
