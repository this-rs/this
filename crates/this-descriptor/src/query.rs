//! Typed queries: filter, sort, page; and the schema they are checked against.

use crate::entity::{EntityDesc, EntityKind, base_fields};
use crate::field::{FieldDesc, FieldType};
use crate::link::LinkDesc;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt;
use std::marker::PhantomData;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum FilterOp {
    Eq,
    Ne,
    Gt,
    Gte,
    Lt,
    Lte,
    In,
    Contains,
    StartsWith,
    IsNull,
}

impl FilterOp {
    pub fn as_str(self) -> &'static str {
        match self {
            FilterOp::Eq => "eq",
            FilterOp::Ne => "ne",
            FilterOp::Gt => "gt",
            FilterOp::Gte => "gte",
            FilterOp::Lt => "lt",
            FilterOp::Lte => "lte",
            FilterOp::In => "in",
            FilterOp::Contains => "contains",
            FilterOp::StartsWith => "starts_with",
            FilterOp::IsNull => "is_null",
        }
    }
}

/// Filter expression. `field` is a path: `amount`, or `target.name` on a link list.
///
/// A `Cmp` value may be a **context reference**, `{"$context": "<key>"}`: a value the
/// host provides when the view is drawn rather than one written in the module (the open
/// conversation, the signed-in person, the selected row…). It is what lets a module say
/// "the messages of the conversation that is open" without knowing which one that is, and
/// without the host knowing anything about the module. The host resolves it with
/// [`FilterExpr::resolve_context`] before matching; an unknown key resolves to `null`,
/// which matches no row. Which keys exist is the host's to document.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FilterExpr {
    And(Vec<FilterExpr>),
    Or(Vec<FilterExpr>),
    Not(Box<FilterExpr>),
    Cmp {
        field: String,
        op: FilterOp,
        value: serde_json::Value,
    },
}

/// The key of a context reference inside a `Cmp` value: `{"$context": "conversation"}`.
/// A `$` so that no plain object a module compares against can be mistaken for one.
pub const CONTEXT_REF: &str = "$context";

/// The values a host provides to filters at render time, by key.
pub type Context = BTreeMap<String, serde_json::Value>;

impl FilterExpr {
    /// The context key a value refers to, when it is a reference and nothing else.
    pub fn context_ref(value: &serde_json::Value) -> Option<&str> {
        let object = value.as_object()?;
        if object.len() != 1 {
            return None;
        }
        object.get(CONTEXT_REF)?.as_str()
    }

    /// Every context key this filter refers to, in reading order (duplicates kept).
    pub fn context_keys(&self) -> Vec<&str> {
        let mut keys = Vec::new();
        self.collect_context_keys(&mut keys);
        keys
    }

    fn collect_context_keys<'a>(&'a self, keys: &mut Vec<&'a str>) {
        match self {
            FilterExpr::And(v) | FilterExpr::Or(v) => {
                for e in v {
                    e.collect_context_keys(keys);
                }
            }
            FilterExpr::Not(e) => e.collect_context_keys(keys),
            FilterExpr::Cmp { value, .. } => keys.extend(Self::context_ref(value)),
        }
    }

    /// Whether this filter needs a context to be matched.
    pub fn needs_context(&self) -> bool {
        !self.context_keys().is_empty()
    }

    /// The same filter with every context reference replaced by the host's value. A key
    /// the host does not provide becomes `null`: the comparison then matches nothing, and
    /// the row set is empty rather than wrong.
    pub fn resolve_context(&self, context: &Context) -> FilterExpr {
        match self {
            FilterExpr::And(v) => {
                FilterExpr::And(v.iter().map(|e| e.resolve_context(context)).collect())
            }
            FilterExpr::Or(v) => {
                FilterExpr::Or(v.iter().map(|e| e.resolve_context(context)).collect())
            }
            FilterExpr::Not(e) => FilterExpr::Not(Box::new(e.resolve_context(context))),
            FilterExpr::Cmp { field, op, value } => FilterExpr::Cmp {
                field: field.clone(),
                op: *op,
                value: match Self::context_ref(value) {
                    Some(key) => context.get(key).cloned().unwrap_or(serde_json::Value::Null),
                    None => value.clone(),
                },
            },
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SortDir {
    #[default]
    Asc,
    Desc,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SortSpec {
    pub field: String,
    #[serde(default)]
    pub dir: SortDir,
}

/// Maximum page size (same clamp as `QueryParams::limit`).
pub const MAX_LIMIT: u32 = 100;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Page {
    /// `page` starts at 1.
    Offset { page: u32, limit: u32 },
    Cursor {
        #[serde(default)]
        after: Option<String>,
        limit: u32,
    },
}

impl Default for Page {
    fn default() -> Self {
        Page::Offset { page: 1, limit: 20 }
    }
}

/// Untyped marker for `Query<Untyped>` (legacy parsing, link lists, dynamic modules).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Untyped;

/// A query over an entity `T` (a `Described` type, or [`Untyped`]).
///
/// The marker lets a front write `Query::<Invoice>::new().filter(..)` and the server call
/// `q.check()` against `Invoice::entity_desc()`. The impls are written by hand (no derive)
/// so that `T` needs no bound at all.
#[derive(Serialize, Deserialize)]
#[serde(bound = "")]
pub struct Query<T = Untyped> {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filter: Option<FilterExpr>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sort: Vec<SortSpec>,
    #[serde(default)]
    pub page: Page,
    #[serde(skip)]
    _entity: PhantomData<fn() -> T>,
}

impl<T> Query<T> {
    pub fn new() -> Self {
        Self {
            filter: None,
            sort: vec![],
            page: Page::default(),
            _entity: PhantomData,
        }
    }
    pub fn filter(mut self, f: FilterExpr) -> Self {
        self.filter = Some(match self.filter.take() {
            None => f,
            Some(FilterExpr::And(mut v)) => {
                v.push(f);
                FilterExpr::And(v)
            }
            Some(prev) => FilterExpr::And(vec![prev, f]),
        });
        self
    }
    pub fn sort_by(mut self, field: &str, dir: SortDir) -> Self {
        self.sort.push(SortSpec {
            field: field.into(),
            dir,
        });
        self
    }
    pub fn page(mut self, page: u32, limit: u32) -> Self {
        self.page = Page::Offset { page, limit };
        self
    }
    /// Drop the marker.
    pub fn untyped(self) -> Query<Untyped> {
        Query {
            filter: self.filter,
            sort: self.sort,
            page: self.page,
            _entity: PhantomData,
        }
    }
}

impl<T: crate::Described> Query<T> {
    /// Check against `T::entity_desc()` (fields filterable, operators allowed, sortable).
    pub fn check(&self) -> Result<(), crate::ContractError> {
        let schema = QuerySchema::for_entity(T::entity_desc());
        crate::engine::query::check(&schema, self)
    }
}

impl<T> Default for Query<T> {
    fn default() -> Self {
        Self::new()
    }
}
impl<T> Clone for Query<T> {
    fn clone(&self) -> Self {
        Self {
            filter: self.filter.clone(),
            sort: self.sort.clone(),
            page: self.page.clone(),
            _entity: PhantomData,
        }
    }
}
impl<T> PartialEq for Query<T> {
    fn eq(&self, o: &Self) -> bool {
        self.filter == o.filter && self.sort == o.sort && self.page == o.page
    }
}
impl<T> fmt::Debug for Query<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Query")
            .field("filter", &self.filter)
            .field("sort", &self.sort)
            .field("page", &self.page)
            .finish()
    }
}

/// What a query may reference: path -> (type, allowed operators, sortable).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct QuerySchema {
    pub fields: BTreeMap<String, QueryField>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct QueryField {
    pub ty: FieldType,
    pub ops: Vec<crate::query::FilterOp>,
    pub sortable: bool,
}

impl QuerySchema {
    fn add(&mut self, prefix: &str, f: &FieldDesc) {
        self.fields.insert(
            format!("{prefix}{}", f.name),
            QueryField {
                ty: f.ty.clone(),
                ops: f.filterable.clone(),
                sortable: f.sortable,
            },
        );
    }

    /// Base fields + declared fields of an entity.
    pub fn for_entity(e: &EntityDesc) -> Self {
        let mut s = Self::default();
        for f in e.all_fields() {
            s.add("", f);
        }
        s
    }

    /// Schema of an (enriched) link list: link base fields, link attributes, and the
    /// fields of both ends under `source.` / `target.` (the legacy `apply_link_filters`
    /// compares such dotted paths on the serialized `EnrichedLink`).
    pub fn for_links(
        link: &LinkDesc,
        source: Option<&EntityDesc>,
        target: Option<&EntityDesc>,
    ) -> Self {
        let mut s = Self::default();
        for f in base_fields(EntityKind::Link)
            .iter()
            .chain(link.fields.iter())
        {
            s.add("", f);
        }
        for (prefix, end) in [("source.", source), ("target.", target)] {
            if let Some(e) = end {
                for f in e.all_fields() {
                    s.add(prefix, f);
                }
            }
        }
        s
    }
}

#[cfg(test)]
mod context_tests {
    use super::*;
    use crate::engine::query::matches;
    use serde_json::json;

    fn open_conversation() -> FilterExpr {
        FilterExpr::Cmp {
            field: "conversation".into(),
            op: FilterOp::Eq,
            value: json!({ CONTEXT_REF: "conversation" }),
        }
    }

    #[test]
    fn a_context_reference_is_one_key_under_dollar_context_and_nothing_else() {
        assert_eq!(
            FilterExpr::context_ref(&json!({ "$context": "conversation" })),
            Some("conversation")
        );
        assert_eq!(FilterExpr::context_ref(&json!("conversation")), None);
        assert_eq!(FilterExpr::context_ref(&json!({ "context": "x" })), None);
        assert_eq!(
            FilterExpr::context_ref(&json!({ "$context": "x", "other": 1 })),
            None,
            "an object with more keys is a value the module compares against"
        );
        assert_eq!(FilterExpr::context_ref(&json!({ "$context": 3 })), None);
    }

    #[test]
    fn resolving_replaces_the_reference_by_the_host_value_everywhere_in_the_tree() {
        let filter = FilterExpr::And(vec![
            open_conversation(),
            FilterExpr::Not(Box::new(FilterExpr::Cmp {
                field: "role".into(),
                op: FilterOp::Eq,
                value: json!("module"),
            })),
        ]);
        assert_eq!(filter.context_keys(), vec!["conversation"]);
        assert!(filter.needs_context());
        let mut context = Context::new();
        context.insert("conversation".into(), json!("c-42"));
        let resolved = filter.resolve_context(&context);
        assert!(!resolved.needs_context());
        let row_in = json!({ "conversation": "c-42", "role": "agent" });
        let row_out = json!({ "conversation": "c-7", "role": "agent" });
        let row_module = json!({ "conversation": "c-42", "role": "module" });
        assert!(matches(&resolved, &row_in));
        assert!(!matches(&resolved, &row_out));
        assert!(!matches(&resolved, &row_module));
    }

    #[test]
    fn a_key_the_host_does_not_provide_matches_nothing_rather_than_everything() {
        let resolved = open_conversation().resolve_context(&Context::new());
        assert_eq!(
            resolved,
            FilterExpr::Cmp {
                field: "conversation".into(),
                op: FilterOp::Eq,
                value: serde_json::Value::Null,
            }
        );
        assert!(!matches(&resolved, &json!({ "conversation": "c-42" })));
        assert!(!matches(&resolved, &json!({})));
    }

    #[test]
    fn a_reference_survives_json_as_written_by_a_module() {
        let json =
            r#"{"cmp":{"field":"conversation","op":"eq","value":{"$context":"conversation"}}}"#;
        let parsed: FilterExpr = serde_json::from_str(json).unwrap();
        assert_eq!(parsed, open_conversation());
        assert_eq!(serde_json::to_string(&parsed).unwrap(), json);
    }
}
