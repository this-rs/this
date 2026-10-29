//! `GraphBackend` — la surface d'obrain dont un consommateur a besoin,
//! **implémentée par obrain et par this-rs**.
//!
//! # Pourquoi cette couture existe
//!
//! Un consommateur qui parle à obrain directement (ouvrir une base, lire des
//! nœuds, écrire des propriétés, indexer, committer) ne peut pas « passer à
//! this-rs » sans réécrire ses appels : `ObrainDB` et `ObrainStore` n'ont pas la
//! même forme. Ce trait est la **couture de bascule** — on écrit son code contre
//! `GraphBackend`, et on choisit l'implémentation.
//!
//! ```rust,ignore
//! fn pages<B: GraphBackend>(backend: &B) -> usize {
//!     backend.nodes_by_label("Page").len()
//! }
//!
//! let n = pages(store.db());       // obrain brut
//! let n = pages(&obrain_store);    // this-rs par-dessus obrain  ← même appel
//! ```
//!
//! # Ce que la bascule apporte
//!
//! `ObrainStore` honore le même contrat **et** ajoute, sans changer un seul
//! appel de lecture ou d'écriture :
//!
//! | apport | où |
//! |---|---|
//! | identité canonique, engine-agnostique | [`crate::core::identity`], `ObrainStore::rebuild_identity_index` |
//! | décision IAM matérialisée (lecture indexée) | `ObrainStore::iam` |
//! | bail d'écriture et jeton de fencing | `ObrainStore::enable_write_fencing_with` |
//! | journal ordonné et durable | [`crate::events::log_obrain`] |
//! | relief auto-descriptif publié en REST | `crate::graph::routes` |
//!
//! # Ce que le trait ne couvre **pas**, et pourquoi
//!
//! Les **espaces nommés** et le **bail** sont des apports de this-rs : obrain ne
//! les a pas. Les inclure ici avec un défaut silencieux laisserait croire
//! qu'`ObrainDB` sait les faire — ils restent donc accessibles directement sur
//! `ObrainStore`. Le contrat commun est exactement ce que les deux savent tenir.

use std::collections::BTreeMap;

use anyhow::{Result, anyhow};
use obrain_common::types::{NodeId, Value};
use obrain_core::Node;
use obrain_core::graph::Direction;
use obrain_engine::ObrainDB;

use crate::storage::ObrainStore;

/// La surface de graphe commune à obrain et à this-rs.
///
/// Object-safe à dessein : un consommateur peut choisir son backend **à
/// l'exécution** (`Box<dyn GraphBackend>`), pas seulement à la compilation.
pub trait GraphBackend: Send + Sync {
    /// Le store est-il adossé au substrat ? (condition de tout le relief
    /// topologique, et du journal durable)
    fn is_substrate(&self) -> bool;

    /// Nombre de nœuds vivants.
    fn node_count(&self) -> usize;

    /// Labels présents, d'après le moteur.
    ///
    /// Une liste vide n'est **pas** « aucun label » : elle peut vouloir dire
    /// « backend non introspectable ». Voir `storage::obrain_identity`.
    fn labels(&self) -> Vec<String>;

    /// Nœuds portant un label.
    fn nodes_by_label(&self, label: &str) -> Vec<NodeId>;

    /// Un nœud, s'il existe.
    fn get_node(&self, id: NodeId) -> Option<Node>;

    /// Les propriétés d'un nœud, clés en texte.
    fn node_properties(&self, id: NodeId) -> BTreeMap<String, Value>;

    /// Une propriété d'un nœud.
    fn node_property(&self, id: NodeId, key: &str) -> Option<Value>;

    /// Voisins d'un nœud dans une direction.
    fn neighbors(&self, id: NodeId, direction: Direction) -> Vec<NodeId>;

    /// Degré sortant.
    fn out_degree(&self, id: NodeId) -> usize;

    /// Nœuds dont une propriété vaut une valeur — **indexé** si l'index existe.
    fn find_nodes_by_property(&self, property: &str, value: &Value) -> Vec<NodeId>;

    /// Crée un nœud avec ses propriétés.
    fn create_node(&self, labels: &[&str], props: Vec<(&str, Value)>) -> NodeId;

    /// Écrit une propriété sur un nœud existant.
    fn set_node_property(&self, id: NodeId, key: &str, value: Value);

    /// Crée une arête typée.
    fn create_edge(&self, from: NodeId, to: NodeId, edge_type: &str);

    /// Rend les mutations durables.
    ///
    /// **C'est l'appel qu'on oublie** : sans lui, une écriture vit dans le WAL
    /// et pas dans l'état — un redémarrage la perd.
    fn commit(&self) -> Result<()>;

    /// Pose un index de propriété.
    fn create_property_index(&self, property: &str);

    /// L'index existe-t-il ?
    fn has_property_index(&self, property: &str) -> bool;

    /// Retire un index de propriété.
    fn drop_property_index(&self, property: &str) -> bool;
}

/// obrain **brut** : le point de départ d'un consommateur.
impl GraphBackend for ObrainDB {
    fn is_substrate(&self) -> bool {
        self.substrate_handle().is_some()
    }

    fn node_count(&self) -> usize {
        self.store().node_count()
    }

    fn labels(&self) -> Vec<String> {
        self.store().all_labels()
    }

    fn nodes_by_label(&self, label: &str) -> Vec<NodeId> {
        self.store().nodes_by_label(label)
    }

    fn get_node(&self, id: NodeId) -> Option<Node> {
        self.store().get_node(id)
    }

    fn node_properties(&self, id: NodeId) -> BTreeMap<String, Value> {
        self.get_node(id)
            .map(|node| {
                node.properties_as_btree()
                    .into_iter()
                    .map(|(key, value)| (key.as_str().to_string(), value))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// **Une** propriété, sans construire la carte complète.
    ///
    /// L'implémentation naïve (`self.node_properties(id).get(key).cloned()`)
    /// payait, pour lire une clé, une seconde carte B-tree avec des clés `String`
    /// allouées et la copie de **toutes** les valeurs — mesuré 4,01 µs contre
    /// 1,19 µs pour la recherche indexée qui la précède. Le moteur expose
    /// `Node::get_property`, qui répond directement.
    fn node_property(&self, id: NodeId, key: &str) -> Option<Value> {
        self.get_node(id)
            .and_then(|node| node.get_property(key).cloned())
    }

    fn neighbors(&self, id: NodeId, direction: Direction) -> Vec<NodeId> {
        self.store().neighbors(id, direction)
    }

    fn out_degree(&self, id: NodeId) -> usize {
        self.store().out_degree(id)
    }

    fn find_nodes_by_property(&self, property: &str, value: &Value) -> Vec<NodeId> {
        ObrainDB::find_nodes_by_property(self, property, value)
    }

    fn create_node(&self, labels: &[&str], props: Vec<(&str, Value)>) -> NodeId {
        self.session().create_node_with_props(labels, props)
    }

    fn set_node_property(&self, id: NodeId, key: &str, value: Value) {
        self.store().set_node_property(id, key, value);
    }

    fn create_edge(&self, from: NodeId, to: NodeId, edge_type: &str) {
        self.session().create_edge(from, to, edge_type);
    }

    fn commit(&self) -> Result<()> {
        ObrainDB::commit_wal(self).map_err(|e| anyhow!("commit_wal : {e}"))
    }

    fn create_property_index(&self, property: &str) {
        ObrainDB::create_property_index(self, property);
    }

    fn has_property_index(&self, property: &str) -> bool {
        ObrainDB::has_property_index(self, property)
    }

    fn drop_property_index(&self, property: &str) -> bool {
        ObrainDB::drop_property_index(self, property)
    }
}

/// **this-rs par-dessus obrain** : le même contrat, avec l'identité, l'IAM, le
/// bail et le journal en plus.
///
/// Toutes les opérations de graphe passent par l'**espace actif** : un
/// consommateur qui bascule vers this-rs n'a donc rien à savoir des espaces
/// nommés pour que ses lectures restent correctes.
impl GraphBackend for ObrainStore {
    fn is_substrate(&self) -> bool {
        ObrainStore::is_substrate(self)
    }

    fn node_count(&self) -> usize {
        self.active_store().node_count()
    }

    fn labels(&self) -> Vec<String> {
        self.active_store().all_labels()
    }

    fn nodes_by_label(&self, label: &str) -> Vec<NodeId> {
        self.active_store().nodes_by_label(label)
    }

    fn get_node(&self, id: NodeId) -> Option<Node> {
        self.session().get_node(id)
    }

    fn node_properties(&self, id: NodeId) -> BTreeMap<String, Value> {
        self.get_node(id)
            .map(|node| {
                node.properties_as_btree()
                    .into_iter()
                    .map(|(key, value)| (key.as_str().to_string(), value))
                    .collect()
            })
            .unwrap_or_default()
    }

    fn node_property(&self, id: NodeId, key: &str) -> Option<Value> {
        self.active_store()
            .get_node_property(id, &obrain_common::types::PropertyKey::new(key))
    }

    fn neighbors(&self, id: NodeId, direction: Direction) -> Vec<NodeId> {
        self.active_store().neighbors(id, direction)
    }

    fn out_degree(&self, id: NodeId) -> usize {
        self.active_store().out_degree(id)
    }

    fn find_nodes_by_property(&self, property: &str, value: &Value) -> Vec<NodeId> {
        // **Espace actif**, comme toutes les autres lectures du contrat : routé
        // vers la racine, cet appel ignorait l'espace courant et deux espaces se
        // répondaient l'un l'autre (le contrat se contredisait lui-même :
        // `nodes_by_label` voyait le nœud, `find_nodes_by_property` non).
        self.active_store().find_nodes_by_property(property, value)
    }

    fn create_node(&self, labels: &[&str], props: Vec<(&str, Value)>) -> NodeId {
        self.session().create_node_with_props(labels, props)
    }

    fn set_node_property(&self, id: NodeId, key: &str, value: Value) {
        self.active_store().set_node_property(id, key, value);
    }

    fn create_edge(&self, from: NodeId, to: NodeId, edge_type: &str) {
        self.session().create_edge(from, to, edge_type);
    }

    fn commit(&self) -> Result<()> {
        ObrainStore::commit_wal(self)
    }

    fn create_property_index(&self, property: &str) {
        self.db().create_property_index(property);
    }

    fn has_property_index(&self, property: &str) -> bool {
        self.db().has_property_index(property)
    }

    fn drop_property_index(&self, property: &str) -> bool {
        self.db().drop_property_index(property)
    }
}

// ---------------------------------------------------------------------------
// Tests — la propriété qui compte : le MÊME code sur les deux backends
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// Le test qui justifie le trait : une fonction écrite **une fois** contre
    /// `GraphBackend` donne le même résultat sur obrain brut et sur this-rs.
    ///
    /// Sans cette preuve, « on peut basculer ses interfaces » resterait une
    /// intention.
    #[test]
    fn the_same_consumer_code_runs_on_both_backends() {
        /// Un consommateur quelconque : il ne connaît que le trait.
        fn recenser<B: GraphBackend>(backend: &B) -> (usize, usize, usize) {
            let pages = backend.nodes_by_label("Page");
            let noms: Vec<String> = pages
                .iter()
                .filter_map(|id| backend.node_property(*id, "qualified_name"))
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect();
            let trouves =
                backend.find_nodes_by_property("qualified_name", &Value::from("entity:mp_hodge"));
            (backend.node_count(), noms.len(), trouves.len())
        }

        // Écriture identique dans les deux cas, via l'API du trait.
        let ecrire = |backend: &dyn GraphBackend| {
            backend.create_property_index("qualified_name");
            backend.create_node(
                &["Page"],
                vec![
                    ("qualified_name", Value::from("entity:mp_hodge")),
                    ("titre", Value::from("Hodge")),
                ],
            );
            backend.create_node(
                &["Page"],
                vec![("qualified_name", Value::from("entity:autre"))],
            );
            backend.commit().expect("commit");
        };

        // (1) obrain brut.
        let raw = ObrainDB::new_in_memory();
        ecrire(&raw);
        let sur_obrain = recenser(&raw);

        // (2) this-rs par-dessus obrain — MÊMES appels, même fonction `recenser`.
        let wrapped = ObrainStore::new_in_memory();
        ecrire(wrapped.as_ref());
        let sur_this_rs = recenser(wrapped.as_ref());

        assert_eq!(
            sur_obrain, sur_this_rs,
            "le même consommateur doit voir la même chose sur les deux backends"
        );
        assert_eq!(sur_obrain, (2, 2, 1), "deux pages, dont une seule cible");
    }

    /// La bascule **à l'exécution** : le consommateur choisit son backend.
    #[test]
    fn the_backend_can_be_chosen_at_runtime() {
        fn compter(backend: &dyn GraphBackend) -> usize {
            backend.node_count()
        }

        let raw = ObrainDB::new_in_memory();
        raw.create_node_with_props(&["Page"], [("qualified_name", Value::from("a"))]);
        raw.commit_wal().expect("commit");

        let wrapped = ObrainStore::new_in_memory();
        wrapped
            .session()
            .create_node_with_props(&["Page"], [("qualified_name", Value::from("a"))]);

        let backends: Vec<&dyn GraphBackend> = vec![&raw, wrapped.as_ref()];
        let comptes: Vec<usize> = backends.iter().map(|b| compter(*b)).collect();
        assert_eq!(
            comptes,
            vec![1, 1],
            "les deux backends sont interchangeables"
        );
    }

    /// Le contrat est **honoré**, pas approximé : ce que le trait promet
    /// (index, lecture, écriture, commit) fonctionne des deux côtés.
    #[test]
    fn the_common_contract_is_honoured_on_both_sides() {
        let raw = ObrainDB::new_in_memory();
        let wrapped = ObrainStore::new_in_memory();
        let backends: Vec<&dyn GraphBackend> = vec![&raw, wrapped.as_ref()];

        for backend in backends {
            assert!(
                backend.is_substrate(),
                "depuis T17, tout store est substrat-backed"
            );
            assert_eq!(backend.node_count(), 0);

            backend.create_property_index("qualified_name");
            assert!(backend.has_property_index("qualified_name"));

            let a = backend.create_node(&["Page"], vec![("qualified_name", Value::from("a"))]);
            let b = backend.create_node(&["Page"], vec![("qualified_name", Value::from("b"))]);
            backend.create_edge(a, b, "POINTS_TO");
            backend.commit().expect("commit");

            assert_eq!(backend.node_count(), 2);
            assert_eq!(backend.labels(), vec!["Page".to_string()]);
            assert_eq!(backend.nodes_by_label("Page").len(), 2);
            assert_eq!(backend.out_degree(a), 1);
            assert_eq!(backend.neighbors(a, Direction::Outgoing), vec![b]);
            assert_eq!(
                backend.node_property(a, "qualified_name"),
                Some(Value::from("a"))
            );
            assert_eq!(
                backend
                    .find_nodes_by_property("qualified_name", &Value::from("b"))
                    .len(),
                1,
                "l'index doit rendre la recherche exacte"
            );

            backend.set_node_property(a, "titre", Value::from("A"));
            backend.commit().expect("commit");
            assert_eq!(backend.node_property(a, "titre"), Some(Value::from("A")));

            assert!(backend.drop_property_index("qualified_name"));
            assert!(!backend.has_property_index("qualified_name"));
        }
    }
}
