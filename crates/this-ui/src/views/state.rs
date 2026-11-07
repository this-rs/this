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
        | Brick::Button { filter, .. }
        | Brick::List { filter, .. }
        | Brick::Card { filter, .. }
        | Brick::Form { filter, .. }
        | Brick::Field { filter, .. }
        | Brick::Relation { filter, .. }
        | Brick::Chat { filter, .. }
        | Brick::ToolStep { filter, .. }
        | Brick::Queue { filter, .. }
        | Brick::CodeBlock { filter, .. }
        | Brick::Result { filter, .. }
        | Brick::Metrics { filter, .. }
        | Brick::Program { filter, .. } => filter.as_ref(),
        Brick::Indicator { filter, .. } => filter.as_ref(),
        Brick::Banner { .. } | Brick::CapabilityRequest { .. } => None,
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

fn collect<'a>(node: &Node, snapshot: &'a Snapshot, out: &mut Vec<(String, Reading<'a>)>) {
    match node {
        Node::Leaf { brick } => {
            let (Some(entity), Some(reading)) = (brick.entity(), brick_reading(brick, snapshot))
            else {
                return;
            };
            out.push((entity.to_owned(), reading));
        }
        Node::Stack { children }
        | Node::Row { children }
        | Node::Drawer { children, .. }
        | Node::Modal { children, .. } => {
            for child in children {
                collect(child, snapshot, out);
            }
        }
        Node::Tabs { tabs } => {
            for tab in tabs {
                for child in &tab.children {
                    collect(child, snapshot, out);
                }
            }
        }
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
}
