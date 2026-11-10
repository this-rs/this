//! # What the host knows, and how a brick reads it
//!
//! `this-ui` never fetches. The host owns the rows and hands them over as a [`Snapshot`]:
//! one [`Feed`] per entity, plus the two facts no entity carries (whether the app is
//! online). A brick reads that snapshot through [`read`], which applies the brick's own
//! filter, its sort and its page — using the *same* filter semantics as the server
//! (`this_descriptor::engine::query::matches`), so a row that a server would return is a
//! row the front shows.
//!
//! A reading is never "nothing": it is rows, or the reason there are none. That is what
//! makes loading, emptiness, failure and refusal four distinct renders instead of one
//! blank screen.

use std::borrow::Cow;
use std::cmp::Ordering;
use std::collections::BTreeMap;

use serde_json::Value;
use this_descriptor::engine::query::{get_path, matches};
use this_descriptor::{Context, FilterExpr, FilterOp, SortDir, SortSpec};

use crate::views::text;

/// The rows of one entity, as the host last knew them.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum Feed {
    /// The host has not asked yet. Rendered as loading: a declared brick that shows
    /// nothing yet is waiting, not empty.
    #[default]
    Idle,
    /// The host asked, nothing has come back yet.
    Loading,
    /// The rows the host holds.
    Ready(Vec<Value>),
    /// The fetch failed; the message is the host's (it is the one that knows why).
    Failed(String),
    /// The permission was refused; the message says which requirement is missing.
    Denied(String),
}

/// Every feed, by entity name.
pub type Feeds = BTreeMap<String, Feed>;

/// The whole of what the renderer is allowed to know.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Snapshot {
    pub feeds: Feeds,
    /// Read by `Indicator::Connection`: the host is the one that can tell.
    pub online: bool,
    /// The values a filter may refer to with `{"$context": "<key>"}`: what the host knows
    /// and no row carries — the open conversation, the selected row, the signed-in person.
    /// The host documents its keys; a module uses them without knowing their values.
    pub context: Context,
}

impl Snapshot {
    /// A snapshot with no feed at all.
    pub fn new() -> Self {
        Self::default()
    }

    /// The rows of one entity, when they are loaded.
    pub fn rows(&self, entity: &str) -> Option<&[Value]> {
        match self.feeds.get(entity) {
            Some(Feed::Ready(rows)) => Some(rows.as_slice()),
            _ => None,
        }
    }

    /// Declare what is known about one entity.
    pub fn with(mut self, entity: impl Into<String>, feed: Feed) -> Self {
        self.feeds.insert(entity.into(), feed);
        self
    }

    /// Declare one value filters may refer to by `{"$context": key}`.
    pub fn with_context(mut self, key: impl Into<String>, value: Value) -> Self {
        self.context.insert(key.into(), value);
        self
    }

    /// A brick's filter, with the host's values in place of its context references. A
    /// filter without reference is borrowed, not copied.
    pub fn resolve<'f>(&self, filter: Option<&'f FilterExpr>) -> Option<Cow<'f, FilterExpr>> {
        let filter = filter?;
        Some(if filter.needs_context() {
            Cow::Owned(filter.resolve_context(&self.context))
        } else {
            Cow::Borrowed(filter)
        })
    }
}

/// The page of rows a brick draws, and where it sits.
#[derive(Clone, Debug, PartialEq)]
pub struct Rows<'a> {
    /// The rows to draw (one page, or everything when the brick does not paginate).
    pub rows: Vec<&'a Value>,
    /// Rows matching the filter, before paging.
    pub total: usize,
    /// Zero-based index of the page actually drawn (clamped into range).
    pub page: usize,
    /// Number of pages; always at least one.
    pub pages: usize,
}

/// The outcome of a brick's read.
#[derive(Clone, Debug, PartialEq)]
pub enum Reading<'a> {
    /// Rows to draw.
    Rows(Rows<'a>),
    /// The entity is loaded and nothing matches.
    Empty,
    /// Waiting for the host.
    Loading,
    /// The host reported a failure.
    Failed(&'a str),
    /// The host reported a refusal.
    Denied(&'a str),
    /// The host knows no such entity: a bug in the host, said out loud.
    Unknown,
}

impl<'a> Reading<'a> {
    /// The rows, when there are some.
    pub fn rows(&self) -> Option<&Rows<'a>> {
        match self {
            Reading::Rows(rows) => Some(rows),
            _ => None,
        }
    }

    /// The reading carries rows to draw. An empty reading is loaded, but there is nothing
    /// to show, which is exactly what `ViewState::Empty` reports.
    pub fn has_rows(&self) -> bool {
        matches!(self, Reading::Rows(_))
    }
}

/// The reading of one brick: its filter, its sort, its page.
///
/// `page` is zero-based and clamped: a filter that shrinks the result set never leaves the
/// person on a page that does not exist.
pub fn read<'a>(
    snapshot: &'a Snapshot,
    entity: &str,
    filter: Option<&FilterExpr>,
    sort: &[SortSpec],
    page: usize,
    page_size: Option<u32>,
) -> Reading<'a> {
    let rows = match snapshot.feeds.get(entity) {
        Some(Feed::Ready(rows)) => rows,
        Some(Feed::Idle) | Some(Feed::Loading) => return Reading::Loading,
        Some(Feed::Failed(message)) => return Reading::Failed(message),
        Some(Feed::Denied(message)) => return Reading::Denied(message),
        None => return Reading::Unknown,
    };
    let filter = snapshot.resolve(filter);
    let mut selected: Vec<&Value> = rows
        .iter()
        .filter(|row| filter.as_deref().is_none_or(|filter| matches(filter, row)))
        .collect();
    let total = selected.len();
    if total == 0 {
        return Reading::Empty;
    }
    sort_rows(&mut selected, sort);
    let size = match page_size {
        None | Some(0) => None,
        Some(size) => Some(size as usize),
    };
    let Some(size) = size else {
        return Reading::Rows(Rows {
            rows: selected,
            total,
            page: 0,
            pages: 1,
        });
    };
    let pages = total.div_ceil(size).max(1);
    let page = page.min(pages - 1);
    let start = page.saturating_mul(size);
    Reading::Rows(Rows {
        rows: selected.into_iter().skip(start).take(size).collect(),
        total,
        page,
        pages,
    })
}

/// The verdict of a `when` node's condition, read from what the host holds.
///
/// A condition holds when at least one row of its entity satisfies its filter. Everything
/// else hides the node, but not for the same reason, and the host wants to tell them apart:
/// a false condition is the module speaking, an unreadable one is something to name in the log.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Condition {
    /// A row satisfies the filter: draw the node.
    Holds,
    /// Rows are there and none satisfies the filter: do not draw.
    Fails,
    /// There is nothing to evaluate against (not read yet, or no row at all): do not draw.
    /// A condition that cannot be evaluated never shows a button at random.
    Absent,
    /// The read failed, was refused, or names an entity the host never provided: do not
    /// draw, and say why in the log. The text names the cause.
    Unreadable(String),
}

impl Condition {
    /// Is the node drawn?
    pub fn holds(&self) -> bool {
        matches!(self, Condition::Holds)
    }
}

/// Evaluate a `when` condition against the snapshot, with the same filter semantics as a
/// brick's own read (context references included).
pub fn condition(snapshot: &Snapshot, entity: &str, filter: &FilterExpr) -> Condition {
    match snapshot.feeds.get(entity) {
        None => Condition::Unreadable(format!("{} : `{entity}`", text::UNKNOWN_ENTITY)),
        Some(Feed::Idle) | Some(Feed::Loading) => Condition::Absent,
        Some(Feed::Failed(why)) => Condition::Unreadable(format!("lecture en échec : {why}")),
        Some(Feed::Denied(why)) => Condition::Unreadable(format!("lecture refusée : {why}")),
        Some(Feed::Ready(rows)) if rows.is_empty() => Condition::Absent,
        Some(Feed::Ready(rows)) => {
            let filter = snapshot.resolve(Some(filter));
            let holds = rows
                .iter()
                .any(|row| filter.as_deref().is_none_or(|filter| matches(filter, row)));
            if holds {
                Condition::Holds
            } else {
                Condition::Fails
            }
        }
    }
}

/// The first row a brick reads: `text`, `card`, `code_block`, `result`, `tool_step`.
pub fn first<'a>(
    snapshot: &'a Snapshot,
    entity: &str,
    filter: Option<&FilterExpr>,
) -> Option<&'a Value> {
    let rows = snapshot.rows(entity)?;
    let filter = snapshot.resolve(filter);
    rows.iter()
        .find(|row| filter.as_deref().is_none_or(|filter| matches(filter, row)))
}

/// The row a brick targets, when it targets exactly one: used to carry an identifier on a
/// delete or an update intent.
pub fn target_id(snapshot: &Snapshot, entity: &str, filter: Option<&FilterExpr>) -> Option<String> {
    first(snapshot, entity, filter)
        .and_then(|row| get_path(row, "id"))
        .and_then(|id| id.as_str())
        .map(str::to_owned)
}

/// The value at a dotted path (`target.name`).
pub fn value_of<'a>(row: &'a Value, path: &str) -> Option<&'a Value> {
    get_path(row, path)
}

/// A value as a person reads it. `None` and `null` become a dash, never an empty hole.
pub fn display(value: Option<&Value>) -> String {
    match value {
        None | Some(Value::Null) => text::DASH.to_owned(),
        Some(Value::String(s)) if s.is_empty() => text::DASH.to_owned(),
        Some(Value::String(s)) => s.clone(),
        Some(Value::Bool(true)) => text::YES.to_owned(),
        Some(Value::Bool(false)) => text::NO.to_owned(),
        Some(Value::Number(number)) => display_number(number),
        Some(Value::Array(items)) => {
            if items.is_empty() {
                text::DASH.to_owned()
            } else {
                items
                    .iter()
                    .map(|item| display(Some(item)))
                    .collect::<Vec<_>>()
                    .join(", ")
            }
        }
        Some(object @ Value::Object(_)) => object.to_string(),
    }
}

fn display_number(number: &serde_json::Number) -> String {
    if let Some(i) = number.as_i64() {
        return crate::model::count(i as f64);
    }
    if let Some(u) = number.as_u64() {
        return crate::model::count(u as f64);
    }
    match number.as_f64() {
        // French decimal separator; the grouping of the integer part is `model::count`'s job.
        Some(f) => {
            let plain = format!("{f}");
            match plain.split_once('.') {
                Some((int, dec)) => {
                    let int = int
                        .parse::<i64>()
                        .map(|grouped| crate::model::count(grouped as f64))
                        .unwrap_or_else(|_| int.to_owned());
                    format!("{int},{dec}")
                }
                None => plain,
            }
        }
        None => number.to_string(),
    }
}

/// The sum of a numeric field over the rows a brick reads. Non-numbers count as zero; a
/// sum of nothing is zero, which is a fact and not an error.
pub fn sum(rows: &[&Value], field: &str) -> f64 {
    rows.iter()
        .filter_map(|row| get_path(row, field))
        .filter_map(|value| match value {
            Value::Number(number) => number.as_f64(),
            Value::String(s) => s.parse::<f64>().ok(),
            _ => None,
        })
        .sum()
}

/// The search a filterable list runs on its own rows: `contains` on each field, OR-ed.
/// An empty needle means no filter at all.
pub fn search(fields: &[String], needle: &str) -> Option<FilterExpr> {
    let needle = needle.trim();
    if needle.is_empty() || fields.is_empty() {
        return None;
    }
    Some(FilterExpr::Or(
        fields
            .iter()
            .map(|field| FilterExpr::Cmp {
                field: field.clone(),
                op: FilterOp::Contains,
                value: Value::String(needle.to_owned()),
            })
            .collect(),
    ))
}

/// Compose the declared filter with the one the person typed.
pub fn and(declared: Option<&FilterExpr>, local: Option<FilterExpr>) -> Option<FilterExpr> {
    match (declared, local) {
        (None, None) => None,
        (Some(declared), None) => Some(declared.clone()),
        (None, Some(local)) => Some(local),
        (Some(declared), Some(local)) => Some(FilterExpr::And(vec![declared.clone(), local])),
    }
}

/// Sort a selection with the same rules as the server: numbers by value, strings and
/// dates lexicographically, booleans false before true, absent and null last.
fn sort_rows(rows: &mut Vec<&Value>, sort: &[SortSpec]) {
    if sort.is_empty() {
        return;
    }
    rows.sort_by(|a, b| {
        for spec in sort {
            let order = match (get_path(a, &spec.field), get_path(b, &spec.field)) {
                (Some(x), Some(y)) => {
                    let order = compare(x, y);
                    // Only a comparison between two present values is reversed: a row
                    // without the value does not become the first row because of `desc`.
                    if spec.dir == SortDir::Desc {
                        order.reverse()
                    } else {
                        order
                    }
                }
                (Some(_), None) => Ordering::Less,
                (None, Some(_)) => Ordering::Greater,
                (None, None) => Ordering::Equal,
            };
            if order != Ordering::Equal {
                return order;
            }
        }
        Ordering::Equal
    });
}

fn compare(a: &Value, b: &Value) -> Ordering {
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => match (x.as_f64(), y.as_f64()) {
            (Some(x), Some(y)) => x.partial_cmp(&y).unwrap_or(Ordering::Equal),
            _ => Ordering::Equal,
        },
        (Value::String(x), Value::String(y)) => x.cmp(y),
        (Value::Bool(x), Value::Bool(y)) => x.cmp(y),
        (Value::Null, Value::Null) => Ordering::Equal,
        // An absent value is always last, whichever direction: a row without a value does
        // not become the first row because the sort was reversed.
        (Value::Null, _) => Ordering::Greater,
        (_, Value::Null) => Ordering::Less,
        _ => a.to_string().cmp(&b.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn snapshot(rows: Vec<Value>) -> Snapshot {
        Snapshot::new().with("invoice", Feed::Ready(rows))
    }

    fn invoices() -> Snapshot {
        snapshot(vec![
            json!({"id": "c", "amount": 30, "name": "Gamma"}),
            json!({"id": "a", "amount": 10, "name": "Alpha"}),
            json!({"id": "b", "amount": 20, "name": "Beta"}),
        ])
    }

    fn spec(field: &str, dir: SortDir) -> SortSpec {
        SortSpec {
            field: field.into(),
            dir,
        }
    }

    fn ids(reading: &Reading<'_>) -> Vec<String> {
        reading
            .rows()
            .map(|rows| {
                rows.rows
                    .iter()
                    .filter_map(|row| row["id"].as_str().map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default()
    }

    #[test]
    fn an_unknown_entity_is_said_out_loud() {
        let empty = Snapshot::new();
        assert_eq!(read(&empty, "ghost", None, &[], 0, None), Reading::Unknown);
        assert_eq!(first(&empty, "ghost", None), None);
    }

    #[test]
    fn each_feed_state_maps_to_its_own_reading() {
        let idle = Snapshot::new().with("x", Feed::Idle);
        assert_eq!(read(&idle, "x", None, &[], 0, None), Reading::Loading);

        let loading = Snapshot::new().with("x", Feed::Loading);
        assert_eq!(read(&loading, "x", None, &[], 0, None), Reading::Loading);

        let failed = Snapshot::new().with("x", Feed::Failed("boom".into()));
        assert_eq!(
            read(&failed, "x", None, &[], 0, None),
            Reading::Failed("boom")
        );

        let denied = Snapshot::new().with("x", Feed::Denied("perm:invoice.list".into()));
        assert_eq!(
            read(&denied, "x", None, &[], 0, None),
            Reading::Denied("perm:invoice.list")
        );

        let ready = Snapshot::new().with("x", Feed::Ready(vec![]));
        assert_eq!(read(&ready, "x", None, &[], 0, None), Reading::Empty);
    }

    #[test]
    fn the_declared_filter_selects_rows_like_the_server_would() {
        let snapshot = invoices();
        let filter = FilterExpr::Cmp {
            field: "amount".into(),
            op: FilterOp::Gte,
            value: json!(20),
        };
        let reading = read(&snapshot, "invoice", Some(&filter), &[], 0, None);
        let mut ids = ids(&reading);
        ids.sort();
        assert_eq!(ids, vec!["b", "c"]);
    }

    #[test]
    fn a_filter_that_matches_nothing_is_empty_not_loading() {
        let snapshot = invoices();
        let filter = FilterExpr::Cmp {
            field: "amount".into(),
            op: FilterOp::Gt,
            value: json!(1000),
        };
        assert_eq!(
            read(&snapshot, "invoice", Some(&filter), &[], 0, None),
            Reading::Empty
        );
    }

    #[test]
    fn the_declared_sort_orders_rows_and_reverses_cleanly() {
        let snapshot = invoices();
        let ascending = read(
            &snapshot,
            "invoice",
            None,
            &[spec("name", SortDir::Asc)],
            0,
            None,
        );
        assert_eq!(ids(&ascending), vec!["a", "b", "c"]);

        let descending = read(
            &snapshot,
            "invoice",
            None,
            &[spec("name", SortDir::Desc)],
            0,
            None,
        );
        assert_eq!(ids(&descending), vec!["c", "b", "a"]);
    }

    #[test]
    fn a_missing_value_stays_last_in_both_directions() {
        let snapshot = snapshot(vec![
            json!({"id": "with", "amount": 5}),
            json!({"id": "without"}),
        ]);
        let ascending = read(
            &snapshot,
            "invoice",
            None,
            &[spec("amount", SortDir::Asc)],
            0,
            None,
        );
        assert_eq!(ids(&ascending), vec!["with", "without"]);
        let descending = read(
            &snapshot,
            "invoice",
            None,
            &[spec("amount", SortDir::Desc)],
            0,
            None,
        );
        assert_eq!(ids(&descending), vec!["with", "without"]);
    }

    #[test]
    fn paging_splits_and_clamps() {
        let snapshot = invoices();
        let first_page = read(
            &snapshot,
            "invoice",
            None,
            &[spec("name", SortDir::Asc)],
            0,
            Some(2),
        );
        let rows = first_page.rows().expect("rows");
        assert_eq!(rows.rows.len(), 2);
        assert_eq!((rows.page, rows.pages, rows.total), (0, 2, 3));

        let beyond = read(
            &snapshot,
            "invoice",
            None,
            &[spec("name", SortDir::Asc)],
            9,
            Some(2),
        );
        let rows = beyond.rows().expect("rows");
        assert_eq!(rows.page, 1);
        assert_eq!(rows.rows.len(), 1);

        let no_paging = read(&snapshot, "invoice", None, &[], 0, Some(0));
        let rows = no_paging.rows().expect("rows");
        assert_eq!((rows.rows.len(), rows.pages), (3, 1));
    }

    #[test]
    fn values_are_displayed_french_and_never_hollow() {
        assert_eq!(display(None), text::DASH);
        assert_eq!(display(Some(&Value::Null)), text::DASH);
        assert_eq!(display(Some(&json!(""))), text::DASH);
        assert_eq!(display(Some(&json!("texte"))), "texte");
        assert_eq!(display(Some(&json!(true))), text::YES);
        assert_eq!(display(Some(&json!(false))), text::NO);
        assert_eq!(display(Some(&json!(1234))), "1\u{202f}234");
        assert_eq!(display(Some(&json!(12.5))), "12,5");
        assert_eq!(display(Some(&json!(["a", "b"]))), "a, b");
        assert_eq!(display(Some(&json!([]))), text::DASH);
        assert_eq!(display(Some(&json!({"k": 1}))), "{\"k\":1}");
    }

    #[test]
    fn a_sum_ignores_what_is_not_a_number() {
        let rows = [
            json!({"amount": 2}),
            json!({"amount": "3.5"}),
            json!({"amount": null}),
            json!({"other": 9}),
        ];
        let refs: Vec<&Value> = rows.iter().collect();
        assert_eq!(sum(&refs, "amount"), 5.5);
        assert_eq!(sum(&[], "amount"), 0.0);
    }

    #[test]
    fn the_local_search_is_or_ed_over_its_fields() {
        assert_eq!(search(&["name".into()], "   "), None);
        assert_eq!(search(&[], "abc"), None);
        let filter = search(&["name".into(), "note".into()], " abc ").expect("filter");
        match &filter {
            FilterExpr::Or(parts) => assert_eq!(parts.len(), 2),
            _ => panic!("expected Or"),
        }

        let snapshot = invoices();
        let filter = search(&["name".into()], "et");
        let reading = read(&snapshot, "invoice", filter.as_ref(), &[], 0, None);
        assert_eq!(ids(&reading), vec!["b"]);
    }

    #[test]
    fn composing_never_loses_the_declared_filter() {
        let declared = FilterExpr::Cmp {
            field: "a".into(),
            op: FilterOp::Eq,
            value: json!(1),
        };
        assert_eq!(and(None, None), None);
        assert_eq!(and(Some(&declared), None), Some(declared.clone()));
        let composed = and(Some(&declared), search(&["b".into()], "x")).expect("composed");
        match composed {
            FilterExpr::And(parts) => assert_eq!(parts.len(), 2),
            _ => panic!("expected And"),
        }
    }

    #[test]
    fn a_targeted_row_gives_its_identifier() {
        let snapshot = invoices();
        let filter = FilterExpr::Cmp {
            field: "amount".into(),
            op: FilterOp::Eq,
            value: json!(20),
        };
        assert_eq!(
            target_id(&snapshot, "invoice", Some(&filter)),
            Some("b".to_owned())
        );
        assert_eq!(target_id(&snapshot, "ghost", None), None);
    }

    #[test]
    fn a_context_reference_in_a_filter_reads_the_host_value_at_read_time() {
        let filter = FilterExpr::Cmp {
            field: "conversation".into(),
            op: FilterOp::Eq,
            value: json!({ "$context": "conversation" }),
        };
        let rows = vec![
            json!({"id": "m1", "conversation": "c-1"}),
            json!({"id": "m2", "conversation": "c-2"}),
            json!({"id": "m3", "conversation": "c-1"}),
        ];
        let open_c1 = Snapshot::new()
            .with("message", Feed::Ready(rows.clone()))
            .with_context("conversation", json!("c-1"));
        let reading = read(&open_c1, "message", Some(&filter), &[], 0, None);
        assert_eq!(ids(&reading), vec!["m1", "m3"]);
        assert_eq!(
            first(&open_c1, "message", Some(&filter)).and_then(|row| row.get("id")),
            Some(&json!("m1"))
        );

        // The same filter, another context: the module wrote nothing new.
        let open_c2 = Snapshot::new()
            .with("message", Feed::Ready(rows.clone()))
            .with_context("conversation", json!("c-2"));
        assert_eq!(
            ids(&read(&open_c2, "message", Some(&filter), &[], 0, None)),
            vec!["m2"]
        );

        // No such key in the context: nothing, said as `Empty`, never every row.
        let no_context = Snapshot::new().with("message", Feed::Ready(rows));
        assert_eq!(
            read(&no_context, "message", Some(&filter), &[], 0, None),
            Reading::Empty
        );
        assert_eq!(first(&no_context, "message", Some(&filter)), None);
    }

    #[test]
    fn a_filter_without_reference_is_borrowed_not_copied() {
        let plain = FilterExpr::Cmp {
            field: "amount".into(),
            op: FilterOp::Gt,
            value: json!(0),
        };
        let snapshot = invoices().with_context("conversation", json!("c-1"));
        assert!(matches!(
            snapshot.resolve(Some(&plain)),
            Some(Cow::Borrowed(_))
        ));
        assert!(snapshot.resolve(None).is_none());
    }
}
