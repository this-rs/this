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

use crate::storage::permission::ReadScope;
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
    /// **Combien d'enfants sont masqués** par le périmètre de lecture.
    ///
    /// Un lecteur qui n'a pas le droit de lire un label ne doit pas le voir — et
    /// ne doit pas non plus pouvoir le **deviner** par un compte. Le masquage est
    /// donc **dit**, jamais silencieux : `children` compte ce qui est lisible,
    /// `hidden` ce qui ne l'est pas.
    #[serde(default)]
    pub hidden: usize,
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
    /// Nœuds de ce niveau **masqués** par le périmètre de lecture.
    #[serde(default)]
    pub hidden: usize,
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
) -> Vec<(obrain_common::types::NodeId, String)> {
    let active = store.active_store();
    let mut children: Vec<(obrain_common::types::NodeId, String)> = Vec::new();
    for (target, edge_id) in active.edges_from(node, obrain_core::graph::Direction::Outgoing) {
        let Some(edge) = active.get_edge(edge_id) else {
            continue;
        };
        if spec
            .edge_types
            .iter()
            .any(|t| t.eq_ignore_ascii_case(edge.edge_type.as_str()))
        {
            children.push((target, edge.edge_type.as_str().to_string()));
        }
    }
    children.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)));
    children.dedup();
    children
}

/// Les labels d'un nœud (pour décider s'il est lisible).
fn labels_of(store: &ObrainStore, node: obrain_common::types::NodeId) -> Vec<String> {
    <ObrainStore as GraphBackend>::get_node(store, node)
        .map(|n| n.labels.iter().map(|l| l.as_str().to_string()).collect())
        .unwrap_or_default()
}

/// Les enfants **visibles** d'un nœud, et combien sont masqués.
fn visible_children(
    store: &ObrainStore,
    node: obrain_common::types::NodeId,
    spec: &HierarchySpec,
    scope: &ReadScope,
) -> (usize, usize, bool) {
    if scope.is_unrestricted() {
        return (contained_children(store, node, spec).len(), 0, false);
    }
    let mut visible = 0usize;
    let mut hidden = 0usize;
    let mut illisible = false;
    for (child, edge_type) in contained_children(store, node, spec) {
        // **Une relation non lisible n'est pas comptée.** La compter révélerait,
        // par un nombre, la structure de ce qu'on n'a pas le droit de voir : on
        // ne publie que le FAIT qu'il existe des relations hors périmètre.
        if !scope.may_read_edge(&edge_type) {
            illisible = true;
            continue;
        }
        if labels_of(store, child)
            .iter()
            .any(|l| scope.may_read_node(l))
        {
            visible += 1;
        } else {
            hidden += 1;
        }
    }
    (visible, hidden, illisible)
}

/// Décrit un nœud de la vue.
/// Décrit un nœud de la vue.
///
/// `enfants_connus` : les comptes (visibles, masqués) **déjà** calculés par un
/// parcours du parent. Les repasser évite de **reparcourir les enfants** — et
/// donc de relire leurs labels une seconde fois. Le banc a montré que ce double
/// parcours coûtait ×1,68 sur un arbre de 258 nœuds.
fn describe_node(
    store: &ObrainStore,
    id: obrain_common::types::NodeId,
    spec: &HierarchySpec,
    scope: &ReadScope,
    enfants_connus: Option<(usize, usize)>,
    labels_connus: Option<&Vec<String>>,
) -> HierarchyNode {
    // **Un nœud se lit une fois.** La vérification de visibilité lit ses labels ;
    // les relire pour le décrire doublait le coût de chaque nœud de la descente —
    // c'est là que passait l'essentiel des 2,2 µs par nœud, pas dans la
    // détection de cycle (que j'avais d'abord soupçonnée à tort : le banc l'a
    // démenti).
    let labels = match labels_connus {
        Some(labels) => labels.clone(),
        None => labels_of(store, id),
    };
    let properties = <ObrainStore as GraphBackend>::node_properties(store, id);
    // **Le compte passe par le périmètre.** Compter tous les enfants
    // révélerait, par un simple nombre, ce qu'un lecteur n'a pas le droit de
    // voir — la fuite la plus discrète : la structure sans le contenu.
    let (children, hidden) = enfants_connus.unwrap_or_else(|| {
        let (v, h, _) = visible_children(store, id, spec, scope);
        (v, h)
    });

    HierarchyNode {
        id: id.as_u64(),
        labels,
        name: name_of(&properties),
        children,
        expanded: false,
        hidden,
        drill_down: format!("/graph/hierarchy/{}?depth=1", id.as_u64()),
    }
}

/// **Descend d'un cran à la fois**, en s'arrêtant aux bornes.
pub fn describe(
    store: &ObrainStore,
    root: obrain_common::types::NodeId,
    spec: HierarchySpec,
) -> Hierarchy {
    describe_scoped(store, root, spec, &ReadScope::all())
}

/// **La même descente, sous un périmètre de lecture.**
///
/// Un principal ne voit que les nœuds dont il a le droit de lire le label, et ne
/// traverse que les relations dont il a le droit de lire le type. Ce qui est
/// masqué est **compté** (`hidden`) : personne ne peut déduire une structure
/// qu'il n'a pas le droit de voir, et personne ne peut croire qu'il a tout vu.
pub fn describe_scoped(
    store: &ObrainStore,
    root: obrain_common::types::NodeId,
    spec: HierarchySpec,
    scope: &ReadScope,
) -> Hierarchy {
    // **Un ensemble, pas un vecteur.** La détection de cycle testait
    // `visited.contains` — un parcours linéaire — pour CHAQUE enfant parcouru :
    // sur un arbre de 258 nœuds, cela faisait ~33 000 comparaisons par descente,
    // et le banc l'a montré (2,2 µs par nœud, dont l'essentiel là). Un ensemble
    // rend le test constant, et `insert` répond vrai la première fois, faux
    // ensuite : le cycle se détecte **en insérant**.
    let mut visited: std::collections::HashSet<obrain_common::types::NodeId> =
        std::collections::HashSet::new();
    visited.insert(root);
    let mut cycles: Vec<(u64, u64)> = Vec::new();
    let mut levels: Vec<HierarchyLevel> = Vec::new();
    let mut frontier: Vec<obrain_common::types::NodeId> = vec![root];

    let mut hidden_total = 0usize;
    let mut edge_illisible = false;
    // Les comptes par nœud, **calculés une fois** : la boucle de niveau sait
    // combien d'enfants chacun a vus, et `describe_node` le réutilise.
    let mut comptes: std::collections::HashMap<obrain_common::types::NodeId, (usize, usize)> =
        std::collections::HashMap::new();
    // Les labels lus pour vérifier la visibilité : on les garde pour décrire les
    // nœuds retenus, plutôt que de les relire.
    let mut labels_vus: std::collections::HashMap<obrain_common::types::NodeId, Vec<String>> =
        std::collections::HashMap::new();

    for depth in 1..=spec.max_depth {
        let mut current: Vec<obrain_common::types::NodeId> = Vec::new();
        let mut hidden_here = 0usize;
        for parent in &frontier {
            // Ce que CE parent voit : on le retient pour ne pas le recalculer.
            let mut vus_ici = 0usize;
            let mut masques_ici = 0usize;
            for (child, edge_type) in contained_children(store, *parent, &spec) {
                // **Le tissage, en lecture** : une relation dont on n'a pas le
                // droit de lire le type n'est ni traversée NI COMPTÉE — la
                // compter décrirait une structure hors périmètre. On retient
                // seulement qu'il en existe, pour le dire en note.
                if !scope.is_unrestricted() && !scope.may_read_edge(&edge_type) {
                    edge_illisible = true;
                    continue;
                }
                if !visited.insert(child) {
                    // **Cycle** : signalé, jamais parcouru. `insert` a répondu
                    // « déjà là » — et il n'a rien coûté de plus.
                    cycles.push((parent.as_u64(), child.as_u64()));
                    continue;
                }
                // **Le nœud lui-même** : le label décide.
                if !scope.is_unrestricted() {
                    let labels = labels_of(store, child);
                    let lisible = labels.iter().any(|l| scope.may_read_node(l));
                    labels_vus.insert(child, labels);
                    if !lisible {
                        // **Masqué, mais visité** : un nœud hors périmètre ne
                        // doit pas être redescendu — ni par un autre chemin.
                        masques_ici += 1;
                        continue;
                    }
                }
                current.push(child);
                vus_ici += 1;
            }
            comptes.insert(*parent, (vus_ici, masques_ici));
            hidden_here += masques_ici;
        }
        hidden_total += hidden_here;
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
            .map(|id| {
                // Le compte vient du parcours qui vient d'avoir lieu : on ne le
                // refait pas.
                describe_node(
                    store,
                    *id,
                    &spec,
                    scope,
                    comptes.get(id).copied(),
                    labels_vus.get(id),
                )
            })
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
            hidden: hidden_here,
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
    if edge_illisible {
        notes.push(
            "Des relations dont votre décision IAM ne couvre pas le TYPE ne sont ni parcourues ni \
             comptées : leur nombre n'est pas publié, parce qu'il décrirait une structure que vous \
             n'avez pas le droit de voir."
                .to_string(),
        );
    }
    if hidden_total > 0 {
        notes.push(format!(
            "{hidden_total} nœud(s) MASQUÉ(s) par le périmètre de lecture : votre décision IAM ne              couvre pas leur label, ou pas le type de la relation qui y mène. Le compte est              publié pour que le masquage soit visible, jamais déduit."
        ));
    }

    Hierarchy {
        root: describe_node(store, root, &spec, scope, None, None),
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

    /// **La granularité** : un principal ne voit que les labels qu'il a le droit
    /// de lire, ne traverse que les types de relations qu'il a le droit de lire,
    /// et ce qui est masqué est **compté** — jamais silencieux.
    #[test]
    fn a_read_scope_hides_what_the_principal_may_not_read() {
        let store = ObrainStore::new_in_memory();
        let session = store.db().session();
        let root = session.create_node_with_props(
            &["Group"],
            [("name", obrain_common::types::Value::from("racine"))],
        );
        // Un sous-groupe lisible, un nœud d'un autre label qui ne l'est pas.
        let visible = session.create_node_with_props(
            &["Group"],
            [("name", obrain_common::types::Value::from("groupe"))],
        );
        let secret = session.create_node_with_props(
            &["Secret"],
            [("name", obrain_common::types::Value::from("confidence"))],
        );
        session.create_edge(root, visible, "CONTAINS");
        session.create_edge(root, secret, "CONTAINS");
        store.commit_wal().expect("commit");

        // Périmètre : lire les `Group`, et lire/parcourir `CONTAINS`.
        let scope =
            ReadScope::from_permissions(["graph:read", "node:read:Group", "edge:read:CONTAINS"]);
        let vue = describe_scoped(&store, root, HierarchySpec::default(), &scope);

        let noms: Vec<&str> = vue.levels[0]
            .nodes
            .iter()
            .filter_map(|n| n.name.as_deref())
            .collect();
        assert_eq!(
            noms,
            vec!["groupe"],
            "seul le label lisible apparaît : {noms:?}"
        );
        assert_eq!(vue.levels[0].hidden, 1, "le nœud masqué est COMPTÉ");
        assert!(
            vue.notes.iter().any(|n| n.contains("MASQUÉ")),
            "et le masquage est expliqué : {:?}",
            vue.notes
        );

        // Un principal qui ne peut pas lire le type de relation ne la traverse
        // même pas : rien n'apparaît, et les deux enfants sont masqués.
        let sans_tissage = ReadScope::from_permissions(["node:read:Group"]);
        let vue = describe_scoped(&store, root, HierarchySpec::default(), &sans_tissage);
        assert!(
            vue.levels.is_empty(),
            "aucune relation traversable : {vue:?}"
        );
        assert_eq!(
            vue.root.hidden, 0,
            "la racine n'a pas d'enfants comptés : la relation n'est pas lue"
        );

        // Sans périmètre restrictif, tout est visible — le comportement d'avant.
        let vue = describe(&store, root, HierarchySpec::default());
        assert_eq!(vue.levels[0].nodes.len(), 2);
        assert_eq!(vue.levels[0].hidden, 0);
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
