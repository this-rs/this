//! **Vue multi-échelle** : voir les sous-groupes des sous-groupes, à toute
//! profondeur — sans jamais perdre le lecteur.
//!
//! # Le principe
//!
//! Un graphe contient des groupes qui contiennent des groupes. Une interface
//! utile ne demande pas « donne-moi tout » (le volume explose, et personne ne
//! lit) : elle **descend d'un niveau à la fois**, et à chaque niveau elle dit
//! **combien** il y a en dessous, ce qui a été montré, et où continuer.
//!
//! # Ce que la vue garantit
//!
//! * **Bornée** : profondeur maximale, et un plafond par niveau.
//! * **Sans boucle** : un nœud déjà visité n'est pas redescendu ; un cycle est
//!   *signalé* au lieu d'être parcouru indéfiniment.
//! * **Sans silence** : `truncated` est publié par niveau, et `total` dit ce
//!   qu'il y a réellement — on ne peut pas confondre « c'est tout » avec « on a
//!   coupé ».
//! * **Par adjacence** : on ne lit que les arêtes du nœud courant, filtrées par
//!   type. Aucun balayage du graphe : le coût suit ce qui est **montré**, pas la
//!   taille de la base.
//!
//! # Pour un moteur d'inférence
//!
//! [`Hierarchy::outline`] rend la même information en **texte indenté** — la
//! forme qu'un modèle lit le mieux — avec, à chaque branche non descendue,
//! l'adresse à demander pour continuer.

use serde::{Deserialize, Serialize};

use crate::storage::{GraphBackend, ObrainStore};

/// Ce qu'on considère comme une relation **de contenance**.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HierarchySpec {
    /// Les types d'arêtes qui expriment « contient » (parent → enfant).
    #[serde(default = "default_edge_types")]
    pub edge_types: Vec<String>,

    /// Profondeur maximale.
    #[serde(default = "default_max_depth")]
    pub max_depth: usize,

    /// Nombre maximal de nœuds montrés **par niveau**.
    #[serde(default = "default_limit")]
    pub per_level_limit: usize,
}

fn default_edge_types() -> Vec<String> {
    [
        "CONTAINS",
        "HAS_SUBGROUP",
        "PART_OF",
        "SUBGROUP_OF",
        "MEMBER_OF",
    ]
    .iter()
    .map(|s| (*s).to_string())
    .collect()
}

fn default_max_depth() -> usize {
    5
}

fn default_limit() -> usize {
    50
}

impl Default for HierarchySpec {
    fn default() -> Self {
        Self {
            edge_types: default_edge_types(),
            max_depth: default_max_depth(),
            per_level_limit: default_limit(),
        }
    }
}

/// Un nœud tel qu'il apparaît dans la vue.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HierarchyNode {
    /// Identifiant du nœud dans le store.
    pub id: u64,
    /// Ses labels.
    pub labels: Vec<String>,
    /// Son nom lisible, s'il en a un.
    pub name: Option<String>,
    /// **Combien d'enfants il a** — même si aucun n'est montré.
    pub children: usize,
    /// Le nœud a-t-il été **descendu** (ses enfants sont au niveau suivant) ?
    pub expanded: bool,
    /// Adresse à demander pour continuer depuis ce nœud.
    pub drill_down: String,
}

/// Un niveau de la vue.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HierarchyLevel {
    /// Profondeur (1 = enfants de la racine).
    pub depth: usize,
    /// Les nœuds montrés à ce niveau.
    pub nodes: Vec<HierarchyNode>,
    /// Nombre total de nœuds à ce niveau, avant plafond.
    pub total: usize,
    /// **A-t-on coupé ?** Jamais tu.
    pub truncated: bool,
}

/// La vue complète.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hierarchy {
    /// Le nœud de départ.
    pub root: HierarchyNode,
    /// Les niveaux, du plus proche au plus profond.
    pub levels: Vec<HierarchyLevel>,
    /// Profondeur effectivement atteinte.
    pub depth: usize,
    /// Cycles rencontrés : `(nœud, ancêtre déjà visité)`.
    pub cycles: Vec<(u64, u64)>,
    /// Ce qui a été appliqué.
    pub spec: HierarchySpec,
    /// Notes de lecture (bornes atteintes, cycles).
    pub notes: Vec<String>,
}

impl Hierarchy {
    /// La vue est-elle vide (aucun enfant) ?
    pub fn is_empty(&self) -> bool {
        self.levels.iter().all(|l| l.nodes.is_empty())
    }

    /// **Le rendu texte**, pour un moteur d'inférence.
    pub fn outline(&self) -> String {
        let mut out = String::new();
        out.push_str(&format!(
            "{} ({} enfant(s))\n",
            self.root
                .name
                .clone()
                .unwrap_or_else(|| format!("#{}", self.root.id)),
            self.root.children
        ));
        for level in &self.levels {
            let indent = "  ".repeat(level.depth);
            for node in &level.nodes {
                let name = node.name.clone().unwrap_or_else(|| format!("#{}", node.id));
                out.push_str(&format!("{indent}- {name} ({})\n", node.children));
                if node.children > 0 && !node.expanded {
                    out.push_str(&format!("{indent}  ↳ {}\n", node.drill_down));
                }
            }
            if level.truncated {
                out.push_str(&format!(
                    "{indent}… {} nœud(x) au total à ce niveau, {} montré(s) : \
                     augmentez `limit` pour voir la suite\n",
                    level.total,
                    level.nodes.len()
                ));
            }
        }
        if !self.cycles.is_empty() {
            out.push_str(&format!(
                "cycles signalés (non parcourus) : {}\n",
                self.cycles
                    .iter()
                    .map(|(from, to)| format!("#{from} → #{to}"))
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        out
    }
}

/// Le nom lisible d'un nœud : la première propriété qui en porte un.
fn name_of(
    properties: &std::collections::BTreeMap<String, obrain_common::types::Value>,
) -> Option<String> {
    for key in ["name", "title", "qualified_name", "label", "slug"] {
        if let Some(value) = properties.get(key).and_then(|v| v.as_str()) {
            return Some(value.to_string());
        }
    }
    None
}

/// Les enfants **de contenance** d'un nœud : arêtes sortantes dont le type est
/// dans la spécification. Par adjacence uniquement.
fn contained_children(
    store: &ObrainStore,
    node: obrain_common::types::NodeId,
    spec: &HierarchySpec,
) -> Vec<obrain_common::types::NodeId> {
    let active = store.active_store();
    let mut children = Vec::new();
    for (target, edge_id) in active.edges_from(node, obrain_core::graph::Direction::Outgoing) {
        let Some(edge) = active.get_edge(edge_id) else {
            continue;
        };
        if spec
            .edge_types
            .iter()
            .any(|t| t.eq_ignore_ascii_case(edge.edge_type.as_str()))
        {
            children.push(target);
        }
    }
    children.sort_unstable();
    children.dedup();
    children
}

/// Décrit un nœud de la vue.
fn describe_node(
    store: &ObrainStore,
    id: obrain_common::types::NodeId,
    spec: &HierarchySpec,
) -> HierarchyNode {
    let labels = <ObrainStore as GraphBackend>::get_node(store, id)
        .map(|node| {
            node.labels
                .iter()
                .map(|l| l.as_str().to_string())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let properties = <ObrainStore as GraphBackend>::node_properties(store, id);
    let children = contained_children(store, id, spec).len();

    HierarchyNode {
        id: id.as_u64(),
        labels,
        name: name_of(&properties),
        children,
        expanded: false,
        drill_down: format!("/graph/hierarchy/{}?depth=1", id.as_u64()),
    }
}

/// **Descend d'un cran à la fois**, en s'arrêtant aux bornes.
pub fn describe(
    store: &ObrainStore,
    root: obrain_common::types::NodeId,
    spec: HierarchySpec,
) -> Hierarchy {
    let mut visited: Vec<obrain_common::types::NodeId> = vec![root];
    let mut cycles: Vec<(u64, u64)> = Vec::new();
    let mut levels: Vec<HierarchyLevel> = Vec::new();
    let mut frontier: Vec<obrain_common::types::NodeId> = vec![root];

    for depth in 1..=spec.max_depth {
        let mut current: Vec<obrain_common::types::NodeId> = Vec::new();
        for parent in &frontier {
            for child in contained_children(store, *parent, &spec) {
                if visited.contains(&child) {
                    // **Cycle** : signalé, jamais parcouru.
                    cycles.push((parent.as_u64(), child.as_u64()));
                    continue;
                }
                visited.push(child);
                current.push(child);
            }
        }
        if current.is_empty() {
            break;
        }
        current.sort_unstable();
        current.dedup();

        let total = current.len();
        let truncated = total > spec.per_level_limit;
        current.truncate(spec.per_level_limit);

        let nodes = current
            .iter()
            .map(|id| describe_node(store, *id, &spec))
            .collect::<Vec<_>>();

        // Le niveau suivant ne descend que les nœuds **montrés**.
        let mut next: Vec<obrain_common::types::NodeId> = nodes
            .iter()
            .filter(|n| n.children > 0)
            .map(|n| obrain_common::types::NodeId::new(n.id))
            .collect();
        next.sort_unstable();
        next.dedup();

        levels.push(HierarchyLevel {
            depth,
            nodes,
            total,
            truncated,
        });
        frontier = next;
    }

    let mut notes = vec![format!(
        "profondeur {} · plafond {} par niveau · arêtes de contenance : {}",
        spec.max_depth,
        spec.per_level_limit,
        spec.edge_types.join(", ")
    )];
    if levels.iter().any(|l| l.truncated) {
        notes.push(
            "au moins un niveau est TRONQUÉ : `total` dit combien il y en a réellement, \
             `limit` combien sont montrés"
                .to_string(),
        );
    }
    if !cycles.is_empty() {
        notes.push(format!(
            "{} cycle(s) signalé(s) et NON parcouru(s) : le graphe se referme",
            cycles.len()
        ));
    }

    Hierarchy {
        root: describe_node(store, root, &spec),
        depth: levels.len(),
        levels,
        cycles,
        spec,
        notes,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::ObrainStore;
    use obrain_common::types::NodeId;

    /// Un arbre de groupes : `profondeur` niveaux, `largeur` enfants par niveau.
    fn tree(store: &ObrainStore, profondeur: usize, largeur: usize) -> NodeId {
        let session = store.db().session();
        let root = session.create_node_with_props(
            &["Group"],
            [("name", obrain_common::types::Value::from("racine"))],
        );
        let mut frontier = vec![root];
        for niveau in 1..=profondeur {
            let mut next = Vec::new();
            for parent in &frontier {
                for i in 0..largeur {
                    let child = session.create_node_with_props(
                        &["Group"],
                        [(
                            "name",
                            obrain_common::types::Value::from(format!("g{niveau}-{i}")),
                        )],
                    );
                    session.create_edge(*parent, child, "CONTAINS");
                    next.push(child);
                }
            }
            frontier = next;
        }
        store.commit_wal().expect("commit");
        root
    }

    /// **L'exigence** : voir les sous-groupes des sous-groupes des sous-groupes.
    #[test]
    fn subgroups_of_subgroups_are_visible_at_every_level() {
        let store = ObrainStore::new_in_memory();
        let root = tree(&store, 4, 2);
        let vue = describe(&store, root, HierarchySpec::default());

        assert_eq!(vue.depth, 4, "quatre niveaux descendus");
        assert_eq!(vue.levels[0].nodes.len(), 2, "2 enfants au niveau 1");
        assert_eq!(vue.levels[1].nodes.len(), 4, "4 au niveau 2");
        assert_eq!(vue.levels[2].nodes.len(), 8, "8 au niveau 3");
        assert_eq!(vue.levels[3].nodes.len(), 16, "16 au niveau 4");

        assert!(
            vue.levels[3].nodes.iter().all(|n| n.children == 0),
            "les feuilles n'ont pas d'enfants"
        );
        assert!(
            vue.levels[0].nodes.iter().all(|n| n.children == 2),
            "chaque groupe du niveau 1 a 2 enfants"
        );
    }

    /// La profondeur est **bornée**, et ce qui reste à voir est **dit**.
    #[test]
    fn a_bounded_depth_reports_what_remains() {
        let store = ObrainStore::new_in_memory();
        let root = tree(&store, 4, 2);
        let spec = HierarchySpec {
            max_depth: 2,
            ..Default::default()
        };
        let vue = describe(&store, root, spec);

        assert_eq!(vue.depth, 2, "on s'arrête à la profondeur demandée");
        assert!(
            vue.levels[1]
                .nodes
                .iter()
                .all(|n| n.children == 2 && !n.expanded),
            "un nœud non descendu dit quand même combien il a d'enfants"
        );
        assert!(
            vue.levels[1].nodes[0]
                .drill_down
                .contains("/graph/hierarchy/"),
            "et donne l'adresse pour continuer"
        );
    }

    /// Le plafond par niveau est appliqué, et la troncature **publiée**.
    #[test]
    fn a_level_cap_is_reported_never_silent() {
        let store = ObrainStore::new_in_memory();
        let root = tree(&store, 2, 10);
        let spec = HierarchySpec {
            per_level_limit: 3,
            max_depth: 2,
            ..Default::default()
        };
        let vue = describe(&store, root, spec);

        assert_eq!(vue.levels[0].nodes.len(), 3, "montrés : le plafond");
        assert_eq!(vue.levels[0].total, 10, "total réel : publié");
        assert!(vue.levels[0].truncated, "et la troncature est DITE");
        assert!(
            vue.notes.iter().any(|n| n.contains("TRONQUÉ")),
            "les notes le répètent : {:?}",
            vue.notes
        );
        assert!(
            !vue.levels[1].nodes.is_empty(),
            "le niveau 2 descend les nœuds montrés : {:?}",
            vue.levels[1]
        );
    }

    /// **Un cycle ne fait pas boucler la vue** : il est signalé.
    #[test]
    fn a_cycle_is_reported_and_never_followed_forever() {
        let store = ObrainStore::new_in_memory();
        let a = tree(&store, 0, 0);
        let session = store.db().session();
        let b = session.create_node_with_props(
            &["Group"],
            [("name", obrain_common::types::Value::from("b"))],
        );
        session.create_edge(a, b, "CONTAINS");
        session.create_edge(b, a, "PART_OF");
        store.commit_wal().expect("commit");

        let vue = describe(&store, a, HierarchySpec::default());
        assert!(
            !vue.cycles.is_empty(),
            "le retour vers un ancêtre doit être SIGNALÉ : {vue:?}"
        );
        assert!(
            vue.depth <= 2,
            "et la descente s'arrête : profondeur {}",
            vue.depth
        );
        assert!(
            vue.notes.iter().any(|n| n.contains("cycle")),
            "les notes l'expliquent : {:?}",
            vue.notes
        );
    }

    /// Seuls les types de contenance comptent.
    #[test]
    fn only_containment_edges_create_subgroups() {
        let store = ObrainStore::new_in_memory();
        let session = store.db().session();
        let root = session.create_node_with_props(
            &["Group"],
            [("name", obrain_common::types::Value::from("racine"))],
        );
        let enfant = session.create_node_with_props(
            &["Group"],
            [("name", obrain_common::types::Value::from("enfant"))],
        );
        let autre = session.create_node_with_props(
            &["Group"],
            [("name", obrain_common::types::Value::from("autre"))],
        );
        session.create_edge(root, enfant, "CONTAINS");
        session.create_edge(root, autre, "CITES");
        store.commit_wal().expect("commit");

        let vue = describe(&store, root, HierarchySpec::default());
        assert_eq!(vue.levels[0].nodes.len(), 1, "une seule contenance");
        assert_eq!(
            vue.levels[0].nodes[0].name.as_deref(),
            Some("enfant"),
            "et c'est le bon"
        );
    }

    /// Le rendu texte : lisible par un modèle, avec les adresses pour descendre.
    #[test]
    fn the_outline_is_readable_and_carries_the_drill_down_addresses() {
        let store = ObrainStore::new_in_memory();
        let root = tree(&store, 2, 2);
        let spec = HierarchySpec {
            max_depth: 1,
            ..Default::default()
        };
        let vue = describe(&store, root, spec);
        let outline = vue.outline();

        assert!(outline.contains("racine (2 enfant"), "{outline}");
        assert!(outline.contains("g1-0"), "{outline}");
        assert!(
            outline.contains("↳ /graph/hierarchy/"),
            "un nœud non descendu porte son adresse : {outline}"
        );
    }

    /// Une racine sans enfants : la vue est vide, et le dit — pas d'erreur.
    #[test]
    fn a_leaf_root_produces_an_empty_view() {
        let store = ObrainStore::new_in_memory();
        let root = tree(&store, 0, 0);
        let vue = describe(&store, root, HierarchySpec::default());
        assert!(vue.is_empty());
        assert_eq!(vue.depth, 0);
        assert!(vue.outline().contains("0 enfant"), "{}", vue.outline());
    }
}
