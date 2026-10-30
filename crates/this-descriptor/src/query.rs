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
