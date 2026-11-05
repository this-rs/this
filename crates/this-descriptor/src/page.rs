//! # Une page est *déclarée*, pas compilée
//!
//! L'écran d'une app this-rs se dérive aujourd'hui du descripteur : une entité
//! déclarée donne sa liste et son formulaire, sans que personne n'écrive
//! d'interface. Ce module étend la même idée à la **page** : un écran est une
//! valeur, un arbre de nœuds que la coquille dessine avec ses propres primitives.
//!
//! ```
//! use this_descriptor::page::{Node, PageDesc, PageSet, PageSlot};
//!
//! let pages = PageSet {
//!     pages: vec![PageDesc {
//!         id: "overview".into(),
//!         title: "Vue d'ensemble".into(),
//!         icon: Some("grid".into()),
//!         slot: PageSlot::Primary,
//!         order: 0,
//!         view: Node::Section {
//!             title: Some("Aujourd'hui".into()),
//!             children: vec![Node::Stat {
//!                 label: "Commandes".into(),
//!                 value: "3".into(),
//!                 hint: None,
//!             }],
//!         },
//!     }],
//! };
//! pages.check().expect("une page simple est valide");
//! ```
//!
//! ## Pourquoi un document à part, et non un champ du descripteur
//!
//! Le descripteur décrit le **domaine** : entités, liens, erreurs, politiques.
//! Il est versionné, et un changement de version avec le même numéro est
//! silencieusement ignoré par le store — c'est un piège connu. Une page, elle,
//! change pour des raisons qui n'ont rien à voir avec le domaine : un libellé,
//! un ordre, une tuile en plus. La mettre dans le descripteur obligerait à
//! bumper la version du module pour repeindre un écran.
//!
//! Un [`PageSet`] voyage donc **à côté** du descripteur, dans le même module,
//! et se remplace sans toucher au domaine.
//!
//! ## Ce que la validation garantit
//!
//! - un nœud inconnu est **refusé à la lecture** (`deny_unknown_fields` sur
//!   chaque variante) : un agent ne peut pas inventer une primitive ;
//! - un arbre trop profond ou trop large est refusé — une page engendrée ne
//!   peut pas noyer l'écran ;
//! - une liste qui vise une entité absente du module est refusée quand la liste
//!   des entités est connue ([`PageSet::check_against`]) ;
//! - une action qui vise une page absente est refusée de la même façon.
//!
//! Rien n'est réparé en silence : un identifiant mal formé est une erreur, pas
//! un identifiant corrigé.

use serde::{Deserialize, Serialize};

use crate::check::DescriptorIssue;
use crate::error::ErrorCode;
use crate::naming::is_identifier;

/// Profondeur maximale d'un arbre de page. Une page est un écran, pas un site.
pub const MAX_DEPTH: usize = 6;
/// Nombre maximal de nœuds dans une page.
pub const MAX_NODES: usize = 200;
/// Nombre maximal de colonnes d'une grille.
pub const MAX_COLUMNS: u8 = 4;
/// Longueur maximale d'un titre de page.
pub const MAX_TITLE: usize = 60;

/// Où la page se place dans la navigation de l'app.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PageSlot {
    /// La navigation principale : barre latérale sur desktop, onglets en bas.
    #[default]
    Primary,
    /// Un groupe secondaire (« outils »), derrière un pli.
    Secondary,
    /// Hors navigation : atteignable par une action ou un lien.
    Hidden,
}

impl PageSlot {
    pub fn as_str(self) -> &'static str {
        match self {
            PageSlot::Primary => "primary",
            PageSlot::Secondary => "secondary",
            PageSlot::Hidden => "hidden",
        }
    }
}

/// La teinte d'un avis. Le vocabulaire est celui du kit, pas celui d'une app.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Tone {
    Ok,
    Warn,
    Danger,
    Info,
    Muted,
}

/// Ce que fait une action : ouvrir une entité, ou aller sur une page.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum Target {
    /// Ouvre la liste (ou le formulaire de création) de cette entité.
    Entity { name: String },
    /// Va sur cette page, par son identifiant.
    Page { id: String },
}

/// Un nœud de l'arbre. Chaque variante correspond à une primitive du kit.
///
/// `deny_unknown_fields` n'est pas décoratif : c'est la garantie qu'un nœud
/// engendré avec un champ inventé est refusé à la frontière, et non ignoré
/// jusqu'à produire un écran vide qu'on croirait juste.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Node {
    /// Un bloc titré qui empile ses enfants.
    Section {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        title: Option<String>,
        children: Vec<Node>,
    },
    /// Une grille de tuiles côte à côte.
    Grid { columns: u8, children: Vec<Node> },
    /// Un chiffre qui compte, avec son libellé et une précision.
    Stat {
        label: String,
        value: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        hint: Option<String>,
    },
    /// Un paragraphe.
    Text { value: String },
    /// Un avis coloré, qui dit un état.
    Notice { tone: Tone, text: String },
    /// La liste d'une entité du module, dessinée par le kit.
    List {
        entity: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        limit: Option<u32>,
    },
    /// Un bouton qui mène quelque part.
    Action {
        label: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        target: Option<Target>,
    },
}

impl Node {
    /// Le nom de la variante, tel qu'il s'écrit dans le document.
    pub fn kind(&self) -> &'static str {
        match self {
            Node::Section { .. } => "section",
            Node::Grid { .. } => "grid",
            Node::Stat { .. } => "stat",
            Node::Text { .. } => "text",
            Node::Notice { .. } => "notice",
            Node::List { .. } => "list",
            Node::Action { .. } => "action",
        }
    }

    fn children(&self) -> &[Node] {
        match self {
            Node::Section { children, .. } | Node::Grid { children, .. } => children,
            _ => &[],
        }
    }
}

/// Une page : son identité (où elle se range) et son contenu.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PageDesc {
    /// Identifiant de la page, `^[a-z][a-z0-9_]*$`, unique dans le module.
    pub id: String,
    pub title: String,
    /// Nom d'icône connu du kit ; absent = pas d'icône.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    #[serde(default)]
    pub slot: PageSlot,
    /// Ordre d'affichage ; à égalité, l'ordre de déclaration tranche.
    #[serde(default)]
    pub order: i32,
    pub view: Node,
}

/// L'ensemble des pages d'un module.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PageSet {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pages: Vec<PageDesc>,
}

impl PageSet {
    /// Vérifie la structure : identifiants, tailles, bornes. Ne connaît pas le
    /// domaine, donc ne peut pas vérifier qu'une entité visée existe.
    pub fn check(&self) -> Result<(), Vec<DescriptorIssue>> {
        self.validate(None)
    }

    /// Vérifie aussi les cibles : chaque `list` doit viser une entité du module,
    /// chaque `action` une page déclarée ou une entité.
    pub fn check_against(&self, entities: &[String]) -> Result<(), Vec<DescriptorIssue>> {
        self.validate(Some(entities))
    }

    /// Toutes les pages d'un emplacement, dans l'ordre d'affichage.
    pub fn slot(&self, slot: PageSlot) -> Vec<&PageDesc> {
        let mut out: Vec<&PageDesc> = self.pages.iter().filter(|p| p.slot == slot).collect();
        out.sort_by_key(|p| p.order);
        out
    }

    fn validate(&self, entities: Option<&[String]>) -> Result<(), Vec<DescriptorIssue>> {
        let mut issues = Vec::new();
        let mut seen: Vec<&str> = Vec::new();

        for (index, page) in self.pages.iter().enumerate() {
            let path = format!("pages[{index}]");

            if !is_identifier(&page.id) {
                push(
                    &mut issues,
                    "page_invalid_id",
                    format!("{path}.id"),
                    format!("`{}` is not ^[a-z][a-z0-9_]*$", page.id),
                );
            }
            if seen.contains(&page.id.as_str()) {
                push(
                    &mut issues,
                    "page_duplicate_id",
                    format!("{path}.id"),
                    format!("`{}` is declared twice", page.id),
                );
            }
            seen.push(&page.id);

            if page.title.trim().is_empty() {
                push(
                    &mut issues,
                    "page_empty_title",
                    format!("{path}.title"),
                    "a page needs a title",
                );
            } else if page.title.chars().count() > MAX_TITLE {
                push(
                    &mut issues,
                    "page_title_too_long",
                    format!("{path}.title"),
                    format!("{} > {MAX_TITLE} characters", page.title.chars().count()),
                );
            }

            let mut count = 0usize;
            walk(
                &page.view,
                &format!("{path}.view"),
                1,
                &mut count,
                &mut issues,
                &self.pages,
                entities,
            );
        }

        if issues.is_empty() {
            Ok(())
        } else {
            Err(issues)
        }
    }
}

fn walk(
    node: &Node,
    path: &str,
    depth: usize,
    count: &mut usize,
    issues: &mut Vec<DescriptorIssue>,
    pages: &[PageDesc],
    entities: Option<&[String]>,
) {
    *count += 1;

    if depth > MAX_DEPTH {
        push(
            issues,
            "page_too_deep",
            path,
            format!("depth > {MAX_DEPTH}"),
        );
        // On s'arrête là : descendre ne produirait qu'un message par niveau.
        return;
    }
    if *count > MAX_NODES {
        if *count == MAX_NODES + 1 {
            push(
                issues,
                "page_too_large",
                path,
                format!("more than {MAX_NODES} nodes"),
            );
        }
        return;
    }

    match node {
        Node::Grid { columns, children } => {
            if *columns == 0 || *columns > MAX_COLUMNS {
                push(
                    issues,
                    "page_bad_columns",
                    format!("{path}.columns"),
                    format!("{columns} is not in 1..={MAX_COLUMNS}"),
                );
            }
            let _ = children;
        }
        Node::List { entity, limit } => {
            if entity.trim().is_empty() {
                push(
                    issues,
                    "page_empty_entity",
                    format!("{path}.entity"),
                    "a list needs an entity",
                );
            }
            if let Some(limit) = limit
                && (*limit == 0 || *limit > 500)
            {
                push(
                    issues,
                    "page_bad_limit",
                    format!("{path}.limit"),
                    format!("{limit} is not in 1..=500"),
                );
            }
            if let Some(known) = entities
                && !known.iter().any(|e| e == entity)
            {
                push(
                    issues,
                    "page_unknown_entity",
                    format!("{path}.entity"),
                    format!("`{entity}` is not an entity of this module"),
                );
            }
        }
        Node::Action { target, .. } => {
            if let Some(Target::Page { id }) = target
                && !pages.iter().any(|p| &p.id == id)
            {
                push(
                    issues,
                    "page_unknown_target",
                    format!("{path}.target"),
                    format!("`{id}` is not a page of this set"),
                );
            }
            if let Some(Target::Entity { name }) = target
                && let Some(known) = entities
                && !known.iter().any(|e| e == name)
            {
                push(
                    issues,
                    "page_unknown_entity",
                    format!("{path}.target"),
                    format!("`{name}` is not an entity of this module"),
                );
            }
        }
        _ => {}
    }

    for (index, child) in node.children().iter().enumerate() {
        walk(
            child,
            &format!("{path}.children[{index}]"),
            depth + 1,
            count,
            issues,
            pages,
            entities,
        );
    }
}

fn push(
    issues: &mut Vec<DescriptorIssue>,
    reason: &str,
    path: impl Into<String>,
    message: impl Into<String>,
) {
    issues.push(DescriptorIssue {
        code: ErrorCode::new(format!("this.page.{reason}")),
        path: path.into(),
        message: message.into(),
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn page(id: &str, view: Node) -> PageDesc {
        PageDesc {
            id: id.into(),
            title: "Titre".into(),
            icon: None,
            slot: PageSlot::Primary,
            order: 0,
            view,
        }
    }

    fn set(pages: Vec<PageDesc>) -> PageSet {
        PageSet { pages }
    }

    fn codes(result: Result<(), Vec<DescriptorIssue>>) -> Vec<String> {
        result.unwrap_err().into_iter().map(|i| i.code.0).collect()
    }

    #[test]
    fn a_minimal_page_is_valid() {
        let s = set(vec![page(
            "overview",
            Node::Text {
                value: "Bonjour".into(),
            },
        )]);
        assert!(s.check().is_ok());
    }

    #[test]
    fn a_json_document_round_trips() {
        let json = r#"{
            "pages": [{
                "id": "overview",
                "title": "Vue",
                "slot": "primary",
                "order": 3,
                "view": { "kind": "grid", "columns": 2, "children": [
                    { "kind": "stat", "label": "Ventes", "value": "3" },
                    { "kind": "list", "entity": "order", "limit": 5 }
                ]}
            }]
        }"#;
        let pages: PageSet = serde_json::from_str(json).expect("document valide");
        assert_eq!(pages.pages.len(), 1);
        assert!(pages.check().is_ok());
        // Aller-retour par la valeur, pas par le texte : la sérialisation peut
        // omettre un champ optionnel sans que le document ait changé de sens.
        let again: PageSet = serde_json::from_str(&serde_json::to_string(&pages).unwrap()).unwrap();
        assert_eq!(again, pages);
    }

    #[test]
    fn an_unknown_node_kind_is_refused_at_read() {
        let json = r#"{"pages":[{"id":"a","title":"A","view":{"kind":"chart","series":[]}}]}"#;
        let err = serde_json::from_str::<PageSet>(json).unwrap_err().to_string();
        assert!(err.contains("unknown variant") || err.contains("chart"), "{err}");
    }

    #[test]
    fn an_invented_field_is_refused_at_read() {
        let json = r#"{"pages":[{"id":"a","title":"A","view":{"kind":"text","value":"x","color":"red"}}]}"#;
        let err = serde_json::from_str::<PageSet>(json).unwrap_err().to_string();
        assert!(err.contains("unknown field") || err.contains("color"), "{err}");
    }

    #[test]
    fn ids_are_identifiers_and_unique() {
        let s = set(vec![
            page("Bad-Id", Node::Text { value: "x".into() }),
            page("ok", Node::Text { value: "x".into() }),
            page("ok", Node::Text { value: "x".into() }),
        ]);
        let c = codes(s.check());
        assert!(c.contains(&"this.page.page_invalid_id".to_string()), "{c:?}");
        assert!(c.contains(&"this.page.page_duplicate_id".to_string()), "{c:?}");
    }

    #[test]
    fn a_page_needs_a_title() {
        let mut p = page("a", Node::Text { value: "x".into() });
        p.title = "   ".into();
        let c = codes(set(vec![p]).check());
        assert_eq!(c, vec!["this.page.page_empty_title"]);
    }

    #[test]
    fn an_empty_set_is_valid() {
        assert!(PageSet::default().check().is_ok());
    }

    #[test]
    fn a_list_facing_an_unknown_entity_is_refused_when_entities_are_known() {
        let s = set(vec![page(
            "a",
            Node::List {
                entity: "invoice".into(),
                limit: None,
            },
        )]);
        assert!(s.check().is_ok(), "sans la liste des entités, on ne peut pas savoir");
        let c = codes(s.check_against(&["order".to_string()]));
        assert_eq!(c, vec!["this.page.page_unknown_entity"]);
        assert!(s.check_against(&["invoice".to_string()]).is_ok());
    }

    #[test]
    fn an_action_facing_an_unknown_page_is_refused() {
        let s = set(vec![page(
            "a",
            Node::Action {
                label: "Aller".into(),
                target: Some(Target::Page {
                    id: "ghost".into(),
                }),
            },
        )]);
        let c = codes(s.check());
        assert_eq!(c, vec!["this.page.page_unknown_target"]);
    }

    #[test]
    fn an_action_can_target_a_declared_page() {
        let s = set(vec![
            page(
                "a",
                Node::Action {
                    label: "Aller".into(),
                    target: Some(Target::Page { id: "b".into() }),
                },
            ),
            page("b", Node::Text { value: "x".into() }),
        ]);
        assert!(s.check().is_ok());
    }

    #[test]
    fn a_grid_outside_one_to_four_columns_is_refused() {
        for columns in [0u8, 5u8] {
            let s = set(vec![page(
                "a",
                Node::Grid {
                    columns,
                    children: vec![],
                },
            )]);
            let c = codes(s.check());
            assert_eq!(c, vec!["this.page.page_bad_columns"], "{columns}");
        }
    }

    #[test]
    fn a_too_deep_tree_is_refused_without_flooding() {
        let mut node = Node::Text { value: "x".into() };
        for _ in 0..MAX_DEPTH {
            node = Node::Section {
                title: None,
                children: vec![node],
            };
        }
        let c = codes(set(vec![page("a", node)]).check());
        assert_eq!(c, vec!["this.page.page_too_deep"], "{c:?}");
    }

    #[test]
    fn a_too_large_tree_is_refused_once() {
        let children: Vec<Node> = (0..MAX_NODES)
            .map(|i| Node::Text {
                value: format!("{i}"),
            })
            .collect();
        let s = set(vec![page(
            "a",
            Node::Section {
                title: None,
                children,
            },
        )]);
        let c = codes(s.check());
        assert_eq!(c, vec!["this.page.page_too_large"], "{c:?}");
    }

    #[test]
    fn a_bad_limit_is_refused() {
        let s = set(vec![page(
            "a",
            Node::List {
                entity: "order".into(),
                limit: Some(0),
            },
        )]);
        let c = codes(s.check());
        assert_eq!(c, vec!["this.page.page_bad_limit"]);
    }

    #[test]
    fn slots_are_ordered_and_filtered() {
        let mut a = page("a", Node::Text { value: "x".into() });
        a.order = 2;
        let mut b = page("b", Node::Text { value: "x".into() });
        b.order = 1;
        let mut c = page("c", Node::Text { value: "x".into() });
        c.slot = PageSlot::Hidden;
        let s = set(vec![a, b, c]);
        let ids: Vec<&str> = s.slot(PageSlot::Primary).iter().map(|p| p.id.as_str()).collect();
        assert_eq!(ids, vec!["b", "a"]);
        assert_eq!(s.slot(PageSlot::Hidden).len(), 1);
        assert!(s.slot(PageSlot::Secondary).is_empty());
    }
}
