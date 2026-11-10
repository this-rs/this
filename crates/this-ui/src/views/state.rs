//! # What a view is doing right now: waiting, empty, failed, refused
//!
//! A brick knows why it has nothing: the host is fetching, the fetch failed, the permission
//! was refused, or the rows simply are not there. Those are four different sentences for
//! the person reading the screen, and four different actions. This module folds the
//! readings of a whole tree into one verdict, so a view can be blocked as a whole when it
//! has nothing usable, and stay rendered (with per-brick tiles) when only part of it
//! struggles.
//!
//! Pure: the caller passes the tree and the snapshot.

use this_descriptor::views::{Brick, Node};

use crate::views::data::{self, Reading, Snapshot};
use crate::views::text;

/// The verdict for one view, rendered by [`crate::views::ViewBody`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ViewState {
    /// Everything the view reads is still on its way.
    Loading,
    /// Nothing is usable and at least one read was refused. The reason is the host's.
    Denied { entity: String, reason: String },
    /// Nothing is usable and at least one read failed. A retry makes sense.
    Failed { entity: String, reason: String },
    /// Nothing is usable because the host never provided the entity: a host bug, said.
    Missing { entity: String },
    /// The data is there and matches nothing.
    Empty { entity: Option<String> },
    /// Draw the tree.
    Ready,
}

/// The verdict of a view, folded from the readings of every brick it holds.
///
/// Priority, in order: a refusal beats a failure beats a missing entity beats a wait beats
/// an emptiness. A refusal comes first because it is the only one that needs the person's
/// decision. As soon as one brick has rows, the view draws itself and each struggling brick
/// shows its own tile instead.
pub fn view_state(root: &Node, snapshot: &Snapshot) -> ViewState {
    let mut readings: Vec<(String, Reading<'_>)> = Vec::new();
    collect(root, snapshot, &mut readings);
    if readings.is_empty() {
        // A view made only of status bricks (a banner, a device request): it draws.
        return ViewState::Ready;
    }
    if readings.iter().any(|(_, reading)| reading.has_rows()) {
        return ViewState::Ready;
    }
    // Un formulaire ou un champ n'a pas besoin de lignes : c'est justement ainsi qu'on en
    // CRÉE. Les cacher derrière « rien à afficher » rendait invisible le seul écran qui
    // permet d'écrire dans une entité vide — mesuré : un `Form` de création sur une entité
    // neuve ne s'affichait pas, et la personne n'avait aucun moyen d'ajouter la première
    // ligne. On dessine donc l'arbre, et chaque brique montre son propre état.
    if has_visible_writable_control(root, snapshot) {
        return ViewState::Ready;
    }
    if let Some((entity, reason)) = find(&readings, |reading| match reading {
        Reading::Denied(reason) => Some(*reason),
        _ => None,
    }) {
        return ViewState::Denied {
            entity,
            reason: reason.to_owned(),
        };
    }
    if let Some((entity, reason)) = find(&readings, |reading| match reading {
        Reading::Failed(reason) => Some(*reason),
        _ => None,
    }) {
        return ViewState::Failed {
            entity,
            reason: reason.to_owned(),
        };
    }
    if let Some((entity, _)) = find(&readings, |reading| match reading {
        Reading::Unknown => Some(text::UNKNOWN_ENTITY),
        _ => None,
    }) {
        return ViewState::Missing { entity };
    }
    // Nothing has rows and something is still on its way: the picture is not complete, so
    // saying "nothing to show" would be a lie. The view waits.
    if readings
        .iter()
        .any(|(_, reading)| matches!(reading, Reading::Loading))
    {
        return ViewState::Loading;
    }
    let entity = readings
        .iter()
        .find(|(_, reading)| matches!(reading, Reading::Empty))
        .map(|(entity, _)| entity.clone());
    ViewState::Empty { entity }
}

/// The reading of one brick, or `None` when the brick reads no entity at all.
pub fn brick_reading<'a>(brick: &Brick, snapshot: &'a Snapshot) -> Option<Reading<'a>> {
    let entity = brick.entity()?;
    Some(match brick {
        Brick::List {
            filter,
            sort,
            page_size,
            ..
        } => data::read(snapshot, entity, filter.as_ref(), sort, 0, *page_size),
        Brick::Queue { filter, sort, .. } => {
            data::read(snapshot, entity, filter.as_ref(), sort, 0, None)
        }
        _ => data::read(snapshot, entity, filter_of(brick), &[], 0, None),
    })
}

fn filter_of(brick: &Brick) -> Option<&this_descriptor::FilterExpr> {
    match brick {
        Brick::Text { filter, .. }
        | Brick::Caption { filter, .. }
        | Brick::Button { filter, .. }
        | Brick::List { filter, .. }
        | Brick::Card { filter, .. }
        | Brick::Form { filter, .. }
        | Brick::Field { filter, .. }
        | Brick::Relation { filter, .. }
        | Brick::Chat { filter, .. }
        | Brick::Message { filter, .. }
        | Brick::ToolStep { filter, .. }
        | Brick::Question { filter, .. }
        | Brick::Notice { filter, .. }
        | Brick::Queue { filter, .. }
        | Brick::CodeBlock { filter, .. }
        | Brick::Result { filter, .. }
        | Brick::Metrics { filter, .. }
        | Brick::Program { filter, .. } => filter.as_ref(),
        Brick::Indicator { filter, .. } => filter.as_ref(),
        Brick::Banner { .. }
        | Brick::CapabilityRequest { .. }
        | Brick::Composer { .. }
        | Brick::Settings { .. }
        | Brick::Secret { .. } => None,
    }
}

fn find<'a>(
    readings: &[(String, Reading<'a>)],
    pick: impl Fn(&Reading<'a>) -> Option<&'a str>,
) -> Option<(String, &'a str)> {
    readings
        .iter()
        .find_map(|(entity, reading)| pick(reading).map(|reason| (entity.clone(), reason)))
}

/// L'arbre porte-t-il de quoi ÉCRIRE (un formulaire, un champ éditable) ?
///
/// Distinct d'« a des lignes » : un formulaire de création, lui, n'en a aucune par nature.
///
/// Les nœuds `when` sont comptés comme vrais : voir [`has_visible_writable_control`] pour la
/// réponse devant un snapshot.
pub fn has_writable_control(node: &Node) -> bool {
    writable(node, None)
}

/// Comme [`has_writable_control`], mais un nœud `when` dont la condition ne tient pas ne
/// compte pas : un formulaire masqué n'est pas un formulaire qu'on peut remplir.
pub fn has_visible_writable_control(node: &Node, snapshot: &Snapshot) -> bool {
    writable(node, Some(snapshot))
}

fn writable(node: &Node, snapshot: Option<&Snapshot>) -> bool {
    let any = |children: &[Node]| children.iter().any(|c| writable(c, snapshot));
    match node {
        Node::Leaf { brick } => matches!(**brick, Brick::Form { .. } | Brick::Field { .. }),
        Node::Stack { children }
        | Node::Row { children }
        | Node::Drawer { children, .. }
        | Node::Modal { children, .. } => any(children),
        Node::Tabs { tabs } => tabs.iter().any(|t| any(&t.children)),
        Node::When {
            entity,
            filter,
            child,
        } => visible(snapshot, entity, filter) && writable(child, snapshot),
    }
}

/// La condition d'un nœud `when` tient-elle ? Sans snapshot, on suppose que oui.
fn visible(
    snapshot: Option<&Snapshot>,
    entity: &str,
    filter: &this_descriptor::FilterExpr,
) -> bool {
    snapshot.is_none_or(|snapshot| data::condition(snapshot, entity, filter).holds())
}

/// The bricks the tree draws for this snapshot, in reading order: those under a `when`
/// whose condition does not hold are not in it. This is the one place the walk and the
/// renderer's `when` agree on (both ask [`data::condition`]); the verdict of the view and
/// the tests read it here.
pub fn drawn<'a>(node: &'a Node, snapshot: &Snapshot) -> Vec<&'a Brick> {
    let mut out = Vec::new();
    walk(node, snapshot, &mut out);
    out
}

fn walk<'a>(node: &'a Node, snapshot: &Snapshot, out: &mut Vec<&'a Brick>) {
    match node {
        Node::Leaf { brick } => out.push(brick),
        Node::Stack { children }
        | Node::Row { children }
        | Node::Drawer { children, .. }
        | Node::Modal { children, .. } => {
            for child in children {
                walk(child, snapshot, out);
            }
        }
        Node::Tabs { tabs } => {
            for tab in tabs {
                for child in &tab.children {
                    walk(child, snapshot, out);
                }
            }
        }
        // Un nœud masqué ne lit rien qui compte : sa lecture ne fait pas le verdict de la
        // vue (une liste en échec sous une condition fausse n'est pas une vue en échec).
        // La lecture de la condition elle-même ne compte pas non plus : elle ne dessine
        // rien. Si tout est masqué, il ne reste aucune lecture et la vue se dessine, vide
        // (`ViewState::Ready`), sans erreur.
        Node::When {
            entity,
            filter,
            child,
        } => {
            if data::condition(snapshot, entity, filter).holds() {
                walk(child, snapshot, out);
            }
        }
    }
}

fn collect<'a>(node: &Node, snapshot: &'a Snapshot, out: &mut Vec<(String, Reading<'a>)>) {
    for brick in drawn(node, snapshot) {
        let (Some(entity), Some(reading)) = (brick.entity(), brick_reading(brick, snapshot)) else {
            continue;
        };
        out.push((entity.to_owned(), reading));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::views::data::Feed;
    use serde_json::json;
    use this_descriptor::views::{Brick, Node};

    fn leaf(brick: Brick) -> Node {
        Node::Leaf {
            brick: Box::new(brick),
        }
    }

    fn list(entity: &str) -> Node {
        leaf(Brick::List {
            entity: entity.into(),
            filter: None,
            label: "name".into(),
            subtitle: None,
            empty: None,
            sort: vec![],
            filterable: vec![],
            page_size: None,
            on_click: None,
            row_actions: vec![],
        })
    }

    fn banner() -> Node {
        leaf(Brick::Banner {
            tone: this_descriptor::views::Tone::Info,
            text: "hello".into(),
        })
    }

    fn two_lists() -> Node {
        Node::Stack {
            children: vec![list("invoice"), list("customer")],
        }
    }

    /// Une entité neuve n'a aucune ligne : c'est exactement l'état où l'on veut CRÉER la
    /// première. La porte « rien à afficher » cachait le formulaire de création — mesuré :
    /// aucune façon d'ajouter la première ligne depuis l'interface.
    #[test]
    fn un_formulaire_se_dessine_sur_une_entite_vide() {
        let form = leaf(Brick::Form {
            entity: "tache".into(),
            filter: None,
            fields: vec![this_descriptor::views::FormField {
                field: "titre".into(),
                label: None,
                read_only: false,
            }],
            submit: None,
        });
        let arbre = Node::Stack {
            children: vec![list("tache"), form],
        };
        let snapshot = Snapshot::new().with("tache", Feed::Ready(Vec::new()));
        assert_eq!(
            view_state(&arbre, &snapshot),
            ViewState::Ready,
            "le formulaire de création doit s'afficher même sans aucune ligne"
        );
    }

    /// La règle ne doit pas ouvrir la porte à tout : une liste vide reste « vide ».
    #[test]
    fn une_liste_vide_reste_vide() {
        let snapshot = Snapshot::new().with("invoice", Feed::Ready(Vec::new()));
        assert!(matches!(
            view_state(&list("invoice"), &snapshot),
            ViewState::Empty { .. }
        ));
    }

    #[test]
    fn a_view_of_status_bricks_always_draws() {
        let snapshot = Snapshot::new();
        assert_eq!(view_state(&banner(), &snapshot), ViewState::Ready);
    }

    #[test]
    fn everything_pending_is_loading() {
        let snapshot = Snapshot::new()
            .with("invoice", Feed::Loading)
            .with("customer", Feed::Loading);
        assert_eq!(view_state(&two_lists(), &snapshot), ViewState::Loading);
    }

    #[test]
    fn one_pending_read_keeps_a_half_empty_view_loading() {
        // Nothing has rows and one section is still coming: the view waits, because
        // "nothing to show" would be a lie while an answer is on its way.
        let snapshot = Snapshot::new()
            .with("invoice", Feed::Ready(vec![]))
            .with("customer", Feed::Loading);
        assert_eq!(view_state(&two_lists(), &snapshot), ViewState::Loading);
    }

    #[test]
    fn one_usable_read_draws_the_view_even_if_another_fails() {
        let snapshot = Snapshot::new()
            .with("invoice", Feed::Ready(vec![json!({"name": "a"})]))
            .with("customer", Feed::Failed("boom".into()));
        assert_eq!(view_state(&two_lists(), &snapshot), ViewState::Ready);
    }

    #[test]
    fn a_refusal_beats_a_failure() {
        let snapshot = Snapshot::new()
            .with("invoice", Feed::Failed("boom".into()))
            .with("customer", Feed::Denied("perm:customer.list".into()));
        assert_eq!(
            view_state(&two_lists(), &snapshot),
            ViewState::Denied {
                entity: "customer".into(),
                reason: "perm:customer.list".into()
            }
        );
    }

    #[test]
    fn a_refusal_beats_a_wait() {
        let snapshot = Snapshot::new()
            .with("invoice", Feed::Loading)
            .with("customer", Feed::Denied("nope".into()));
        assert_eq!(
            view_state(&two_lists(), &snapshot),
            ViewState::Denied {
                entity: "customer".into(),
                reason: "nope".into()
            }
        );
    }

    #[test]
    fn a_failure_with_nothing_usable_is_reported_with_its_entity() {
        let snapshot = Snapshot::new()
            .with("invoice", Feed::Failed("timeout".into()))
            .with("customer", Feed::Loading);
        assert_eq!(
            view_state(&two_lists(), &snapshot),
            ViewState::Failed {
                entity: "invoice".into(),
                reason: "timeout".into()
            }
        );
    }

    #[test]
    fn an_entity_the_host_never_provided_is_named() {
        let snapshot = Snapshot::new().with("invoice", Feed::Loading);
        assert_eq!(
            view_state(&two_lists(), &snapshot),
            ViewState::Missing {
                entity: "customer".into()
            }
        );
    }

    #[test]
    fn loaded_but_empty_is_empty_and_names_its_entity() {
        let snapshot = Snapshot::new()
            .with("invoice", Feed::Ready(vec![]))
            .with("customer", Feed::Ready(vec![]));
        assert_eq!(
            view_state(&two_lists(), &snapshot),
            ViewState::Empty {
                entity: Some("invoice".into())
            }
        );
    }

    #[test]
    fn tabs_and_nested_containers_are_walked() {
        let snapshot = Snapshot::new().with("invoice", Feed::Denied("no".into()));
        let tree = Node::Tabs {
            tabs: vec![this_descriptor::views::Tab {
                label: "a".into(),
                children: vec![Node::Row {
                    children: vec![list("invoice")],
                }],
            }],
        };
        assert_eq!(
            view_state(&tree, &snapshot),
            ViewState::Denied {
                entity: "invoice".into(),
                reason: "no".into()
            }
        );
    }

    #[test]
    fn a_brick_that_reads_no_entity_has_no_reading() {
        let snapshot = Snapshot::new();
        assert_eq!(
            brick_reading(
                &Brick::Banner {
                    tone: this_descriptor::views::Tone::Info,
                    text: "x".into()
                },
                &snapshot
            ),
            None
        );
    }

    fn when(entity: &str, field: &str, value: serde_json::Value, child: Node) -> Node {
        Node::When {
            entity: entity.into(),
            filter: this_descriptor::FilterExpr::Cmp {
                field: field.into(),
                op: this_descriptor::FilterOp::Eq,
                value,
            },
            child: Box::new(child),
        }
    }

    fn session(connected: bool) -> Snapshot {
        Snapshot::new().with(
            "session",
            Feed::Ready(vec![json!({"id": "s", "connected": connected})]),
        )
    }

    fn button(action: &str) -> Node {
        leaf(Brick::Button {
            entity: "session".into(),
            filter: None,
            action: action.into(),
            label: None,
        })
    }

    fn actions(node: &Node, snapshot: &Snapshot) -> Vec<String> {
        drawn(node, snapshot)
            .into_iter()
            .map(|b| match b {
                Brick::Button { action, .. } => action.clone(),
                other => format!("{other:?}"),
            })
            .collect()
    }

    #[test]
    fn a_false_conditional_node_is_not_drawn_and_a_true_one_is() {
        let tree = Node::Stack {
            children: vec![
                when("session", "connected", json!(true), button("sign_out")),
                when("session", "connected", json!(false), button("sign_in")),
            ],
        };
        assert_eq!(actions(&tree, &session(true)), ["sign_out"]);
        assert_eq!(actions(&tree, &session(false)), ["sign_in"]);
    }

    #[test]
    fn a_whole_group_disappears_with_its_condition() {
        let group = Node::Row {
            children: vec![button("a"), button("b")],
        };
        let tree = Node::Stack {
            children: vec![
                when("session", "connected", json!(true), group),
                button("always"),
            ],
        };
        assert_eq!(actions(&tree, &session(true)), ["a", "b", "always"]);
        assert_eq!(actions(&tree, &session(false)), ["always"]);
    }

    #[test]
    fn a_condition_that_cannot_be_evaluated_draws_nothing() {
        let node = when("session", "connected", json!(true), button("sign_out"));
        let no_rows = Snapshot::new().with("session", Feed::Ready(vec![]));
        let loading = Snapshot::new().with("session", Feed::Loading);
        let idle = Snapshot::new().with("session", Feed::Idle);
        let failed = Snapshot::new().with("session", Feed::Failed("boom".into()));
        let denied = Snapshot::new().with("session", Feed::Denied("perm".into()));
        for (name, snapshot) in [
            ("no row", no_rows),
            ("loading", loading),
            ("idle", idle),
            ("failed", failed),
            ("denied", denied),
            ("entity never provided", Snapshot::new()),
        ] {
            assert!(
                actions(&node, &snapshot).is_empty(),
                "{name}: an unevaluable condition must not show a button"
            );
        }
    }

    #[test]
    fn the_verdicts_of_a_condition_name_their_cause() {
        use crate::views::data::{Condition, condition};
        let f = this_descriptor::FilterExpr::Cmp {
            field: "connected".into(),
            op: this_descriptor::FilterOp::Eq,
            value: json!(true),
        };
        assert_eq!(condition(&session(true), "session", &f), Condition::Holds);
        assert_eq!(condition(&session(false), "session", &f), Condition::Fails);
        assert_eq!(
            condition(
                &Snapshot::new().with("session", Feed::Ready(vec![])),
                "session",
                &f
            ),
            Condition::Absent
        );
        assert!(matches!(
            condition(&Snapshot::new().with("session", Feed::Failed("boom".into())), "session", &f),
            Condition::Unreadable(why) if why.contains("boom")
        ));
        assert!(matches!(
            condition(&Snapshot::new(), "session", &f),
            Condition::Unreadable(_)
        ));
    }

    #[test]
    fn a_tree_whose_conditional_nodes_are_all_hidden_is_ready_not_an_error() {
        let tree = Node::Stack {
            children: vec![
                when("session", "connected", json!(true), list("invoice")),
                when("session", "connected", json!(true), button("sign_out")),
            ],
        };
        // The hidden list would have FAILED; hidden, it does not make the view fail.
        let snapshot = session(false).with("invoice", Feed::Failed("boom".into()));
        assert_eq!(view_state(&tree, &snapshot), ViewState::Ready);
        // And the same tree, shown, does report what it reads.
        let only_list = when("session", "connected", json!(true), list("invoice"));
        let shown = session(true).with("invoice", Feed::Failed("boom".into()));
        assert!(matches!(
            view_state(&only_list, &shown),
            ViewState::Failed { .. }
        ));
        assert_eq!(view_state(&only_list, &snapshot), ViewState::Ready);
    }

    #[test]
    fn a_hidden_form_does_not_make_the_view_ready() {
        let form = leaf(Brick::Form {
            entity: "invoice".into(),
            filter: None,
            fields: vec![],
            submit: None,
        });
        let tree = when("session", "connected", json!(true), form);
        let hidden = session(false).with("invoice", Feed::Ready(vec![]));
        // Nothing drawn, nothing read: the view draws (empty) rather than erroring.
        assert_eq!(view_state(&tree, &hidden), ViewState::Ready);
        assert!(!has_visible_writable_control(&tree, &hidden));
        assert!(has_visible_writable_control(&tree, &session(true)));
        assert!(has_writable_control(&tree));
    }
}
