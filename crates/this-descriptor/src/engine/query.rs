//! Pure query engine: strict parsing of the legacy `?filter=&sort=`, schema check,
//! in-memory evaluation (filter BEFORE pagination).

use crate::engine::validate::cmp_numbers;
use crate::error::ContractError;
use crate::framework::codes;
use crate::query::{
    FilterExpr, FilterOp, MAX_LIMIT, Page, Query, QuerySchema, SortDir, SortSpec, Untyped,
};
use serde_json::Value;
use std::cmp::Ordering;

fn malformed(reason: impl Into<String>) -> ContractError {
    ContractError::framework(codes::QUERY_MALFORMED_FILTER).with_param("reason", reason.into())
}

/// Parse the legacy filter object `{"status": "active", "amount>": 100}`.
///
/// Strict: invalid JSON or a non-object is `this.query.malformed_filter` (the legacy
/// `QueryParams::filter_value` silently ignored it). Suffixes: `>`, `>=`, `<`, `<=`, `!=`;
/// an array value without suffix is `in`; no suffix = `eq`.
pub fn parse_filter_json(s: &str) -> Result<FilterExpr, ContractError> {
    let v: Value = serde_json::from_str(s).map_err(|e| malformed(e.to_string()))?;
    let Value::Object(obj) = v else {
        return Err(malformed("the filter must be a JSON object"));
    };
    let mut parts = Vec::new();
    for (k, value) in obj {
        let (field, op) = if let Some(f) = k.strip_suffix(">=") {
            (f, FilterOp::Gte)
        } else if let Some(f) = k.strip_suffix("<=") {
            (f, FilterOp::Lte)
        } else if let Some(f) = k.strip_suffix("!=") {
            (f, FilterOp::Ne)
        } else if let Some(f) = k.strip_suffix('>') {
            (f, FilterOp::Gt)
        } else if let Some(f) = k.strip_suffix('<') {
            (f, FilterOp::Lt)
        } else if value.is_array() {
            (k.as_str(), FilterOp::In)
        } else {
            (k.as_str(), FilterOp::Eq)
        };
        let field = field.trim();
        if field.is_empty() {
            return Err(malformed(format!("empty field in key `{k}`")));
        }
        parts.push(FilterExpr::Cmp {
            field: field.to_string(),
            op,
            value,
        });
    }
    Ok(if parts.len() == 1 {
        parts.pop().unwrap()
    } else {
        FilterExpr::And(parts)
    })
}

/// Parse the legacy sort: `field`, `field:asc`, `field:desc`, `-field`, comma separated.
pub fn parse_sort(s: &str) -> Result<Vec<SortSpec>, ContractError> {
    let bad =
        |r: String| ContractError::framework(codes::QUERY_MALFORMED_SORT).with_param("reason", r);
    s.split(',')
        .map(|part| {
            let part = part.trim();
            let (field, dir) = if let Some(f) = part.strip_prefix('-') {
                (f, SortDir::Desc)
            } else if let Some((f, d)) = part.split_once(':') {
                match d {
                    "asc" => (f, SortDir::Asc),
                    "desc" => (f, SortDir::Desc),
                    other => return Err(bad(format!("direction `{other}`"))),
                }
            } else {
                (part, SortDir::Asc)
            };
            if field.is_empty() {
                return Err(bad(format!("empty field in `{s}`")));
            }
            Ok(SortSpec {
                field: field.to_string(),
                dir,
            })
        })
        .collect()
}

/// Legacy `QueryParams` → checked `Query`. Malformed filter or sort is an ERROR, never
/// ignored; then [`check`] against `schema`.
pub fn parse_legacy(
    schema: &QuerySchema,
    filter: Option<&str>,
    sort: Option<&str>,
    page: usize,
    limit: usize,
) -> Result<Query<Untyped>, ContractError> {
    let mut q = Query::<Untyped>::new();
    if let Some(f) = filter.filter(|f| !f.trim().is_empty()) {
        q.filter = Some(parse_filter_json(f)?);
    }
    if let Some(s) = sort.filter(|s| !s.trim().is_empty()) {
        q.sort = parse_sort(s)?;
    }
    q.page = Page::Offset {
        page: page.max(1).min(u32::MAX as usize) as u32,
        limit: limit.clamp(1, MAX_LIMIT as usize) as u32,
    };
    check(schema, &q)?;
    Ok(q)
}

/// Every filtered field is in the schema with the operator allowed; every sort field is
/// sortable; the page size is within `MAX_LIMIT`.
pub fn check<T>(schema: &QuerySchema, q: &Query<T>) -> Result<(), ContractError> {
    fn walk(schema: &QuerySchema, e: &FilterExpr) -> Result<(), ContractError> {
        match e {
            FilterExpr::And(v) | FilterExpr::Or(v) => v.iter().try_for_each(|x| walk(schema, x)),
            FilterExpr::Not(x) => walk(schema, x),
            FilterExpr::Cmp { field, op, value } => {
                let f = schema.fields.get(field).ok_or_else(|| {
                    ContractError::framework(codes::QUERY_FIELD_NOT_FILTERABLE)
                        .with_param("field", field)
                })?;
                if f.ops.is_empty() {
                    return Err(ContractError::framework(codes::QUERY_FIELD_NOT_FILTERABLE)
                        .with_param("field", field));
                }
                if !f.ops.contains(op) {
                    return Err(ContractError::framework(codes::QUERY_OP_NOT_ALLOWED)
                        .with_param("field", field)
                        .with_param("op", op.as_str()));
                }
                let ok = match op {
                    FilterOp::In => value
                        .as_array()
                        .is_some_and(|a| a.iter().all(|x| f.ty.accepts(x))),
                    FilterOp::IsNull => value.is_boolean(),
                    FilterOp::Contains | FilterOp::StartsWith => value.is_string(),
                    _ => f.ty.accepts(value),
                };
                if !ok {
                    return Err(malformed(format!(
                        "value {value} does not fit `{field}` ({})",
                        op.as_str()
                    )));
                }
                Ok(())
            }
        }
    }
    if let Some(f) = &q.filter {
        walk(schema, f)?;
    }
    for s in &q.sort {
        if !schema.fields.get(&s.field).is_some_and(|f| f.sortable) {
            return Err(ContractError::framework(codes::QUERY_FIELD_NOT_SORTABLE)
                .with_param("field", &s.field));
        }
    }
    let limit = match &q.page {
        Page::Offset { limit, .. } | Page::Cursor { limit, .. } => *limit,
    };
    if limit == 0 || limit > MAX_LIMIT {
        return Err(
            ContractError::framework(codes::QUERY_PAGE_OUT_OF_RANGE).with_param("max", MAX_LIMIT)
        );
    }
    Ok(())
}

/// Value at a dotted path (`target.name`).
pub fn get_path<'a>(row: &'a Value, path: &str) -> Option<&'a Value> {
    path.split('.').try_fold(row, |v, seg| v.get(seg))
}

fn cmp_values(a: &Value, b: &Value) -> Option<Ordering> {
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => cmp_numbers(x, y),
        // RFC 3339 dates and plain strings compare lexicographically.
        (Value::String(x), Value::String(y)) => Some(x.cmp(y)),
        (Value::Bool(x), Value::Bool(y)) => Some(x.cmp(y)),
        _ => None,
    }
}

/// Does `row` match `expr`? A missing path never matches (except `is_null: true`).
pub fn matches(expr: &FilterExpr, row: &Value) -> bool {
    match expr {
        FilterExpr::And(v) => v.iter().all(|e| matches(e, row)),
        FilterExpr::Or(v) => v.iter().any(|e| matches(e, row)),
        FilterExpr::Not(e) => !matches(e, row),
        FilterExpr::Cmp { field, op, value } => {
            let got = get_path(row, field);
            match op {
                FilterOp::IsNull => {
                    let is_null = got.is_none_or(Value::is_null);
                    value.as_bool() == Some(is_null)
                }
                _ => {
                    let Some(got) = got.filter(|g| !g.is_null()) else {
                        return false;
                    };
                    match op {
                        FilterOp::Eq => got == value,
                        FilterOp::Ne => got != value,
                        FilterOp::Gt => cmp_values(got, value) == Some(Ordering::Greater),
                        FilterOp::Gte => matches!(
                            cmp_values(got, value),
                            Some(Ordering::Greater | Ordering::Equal)
                        ),
                        FilterOp::Lt => cmp_values(got, value) == Some(Ordering::Less),
                        FilterOp::Lte => matches!(
                            cmp_values(got, value),
                            Some(Ordering::Less | Ordering::Equal)
                        ),
                        FilterOp::In => value.as_array().is_some_and(|a| a.contains(got)),
                        FilterOp::Contains => match (got, value) {
                            (Value::String(g), Value::String(v)) => g.contains(v.as_str()),
                            (Value::Array(g), v) => g.contains(v),
                            _ => false,
                        },
                        FilterOp::StartsWith => {
                            matches!((got, value), (Value::String(g), Value::String(v)) if g.starts_with(v.as_str()))
                        }
                        FilterOp::IsNull => unreachable!(),
                    }
                }
            }
        }
    }
}

/// In-memory execution: filter, THEN sort, THEN paginate. Returns `(page, total_matching)`.
///
/// This is the documented O(n) default of `EntityFetcher::list_query`; a backend that can
/// push the query down overrides it.
pub fn apply_in_memory<T>(q: &Query<T>, rows: Vec<Value>) -> (Vec<Value>, usize) {
    let mut rows: Vec<Value> = match &q.filter {
        Some(f) => rows.into_iter().filter(|r| matches(f, r)).collect(),
        None => rows,
    };
    if !q.sort.is_empty() {
        rows.sort_by(|a, b| {
            for s in &q.sort {
                let o = match (get_path(a, &s.field), get_path(b, &s.field)) {
                    (Some(x), Some(y)) => cmp_values(x, y).unwrap_or(Ordering::Equal),
                    (Some(_), None) => Ordering::Less,
                    (None, Some(_)) => Ordering::Greater,
                    (None, None) => Ordering::Equal,
                };
                let o = if s.dir == SortDir::Desc {
                    o.reverse()
                } else {
                    o
                };
                if o != Ordering::Equal {
                    return o;
                }
            }
            Ordering::Equal
        });
    }
    let total = rows.len();
    let (skip, limit) = match &q.page {
        Page::Offset { page, limit } => (
            ((*page).max(1) as usize - 1) * *limit as usize,
            *limit as usize,
        ),
        // Cursor = opaque offset encoded as a decimal string by the server.
        Page::Cursor { after, limit } => (
            after
                .as_deref()
                .and_then(|c| c.parse::<usize>().ok())
                .unwrap_or(0),
            *limit as usize,
        ),
    };
    (rows.into_iter().skip(skip).take(limit).collect(), total)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entity::EntityDesc;
    use crate::field::{FieldDesc, FieldType};
    use crate::link::LinkDesc;
    use serde_json::json;

    fn schema() -> QuerySchema {
        let mut e = EntityDesc::new("invoice");
        e.fields = vec![
            FieldDesc::new("amount", FieldType::F64)
                .filterable(&[FilterOp::Gt, FilterOp::Eq])
                .sortable(),
            FieldDesc::new("note", FieldType::String),
        ];
        QuerySchema::for_entity(&e)
    }

    #[test]
    fn malformed_filter_is_error_not_ignored() {
        let e = parse_legacy(&schema(), Some("{not json"), None, 1, 20).unwrap_err();
        assert_eq!(e.code.as_str(), codes::QUERY_MALFORMED_FILTER);
        let e = parse_legacy(&schema(), Some("[1,2]"), None, 1, 20).unwrap_err();
        assert_eq!(e.code.as_str(), codes::QUERY_MALFORMED_FILTER);
    }

    #[test]
    fn legacy_filter_and_sort_parse_and_check() {
        let q = parse_legacy(
            &schema(),
            Some(r#"{"amount>": 100, "status": "paid"}"#),
            Some("amount:desc"),
            2,
            500,
        )
        .unwrap();
        assert_eq!(
            q.page,
            Page::Offset {
                page: 2,
                limit: 100
            }
        );
        assert_eq!(
            q.sort,
            vec![SortSpec {
                field: "amount".into(),
                dir: SortDir::Desc
            }]
        );
    }

    #[test]
    fn field_not_filterable_op_not_allowed_sort_not_sortable() {
        let s = schema();
        assert_eq!(
            parse_legacy(&s, Some(r#"{"note": "x"}"#), None, 1, 20)
                .unwrap_err()
                .code
                .as_str(),
            codes::QUERY_FIELD_NOT_FILTERABLE
        );
        assert_eq!(
            parse_legacy(&s, Some(r#"{"amount<": 3}"#), None, 1, 20)
                .unwrap_err()
                .code
                .as_str(),
            codes::QUERY_OP_NOT_ALLOWED
        );
        assert_eq!(
            parse_legacy(&s, None, Some("note"), 1, 20)
                .unwrap_err()
                .code
                .as_str(),
            codes::QUERY_FIELD_NOT_SORTABLE
        );
        assert_eq!(
            parse_legacy(&s, None, Some("amount:up"), 1, 20)
                .unwrap_err()
                .code
                .as_str(),
            codes::QUERY_MALFORMED_SORT
        );
        assert_eq!(
            parse_legacy(&s, Some(r#"{"amount>": "big"}"#), None, 1, 20)
                .unwrap_err()
                .code
                .as_str(),
            codes::QUERY_MALFORMED_FILTER
        );
    }

    #[test]
    fn link_filter_on_target_field_still_accepted() {
        let mut car = EntityDesc::new("car");
        car.fields = vec![];
        let schema =
            QuerySchema::for_links(&LinkDesc::new("owner", "user", "car"), None, Some(&car));
        let q = parse_legacy(&schema, Some(r#"{"target.name": "Clio"}"#), None, 1, 20).unwrap();
        let rows = vec![
            json!({"target": {"name": "Clio"}}),
            json!({"target": {"name": "Golf"}}),
        ];
        let (page, total) = apply_in_memory(&q, rows);
        assert_eq!((page.len(), total), (1, 1));
    }

    #[test]
    fn filter_applies_before_pagination() {
        let rows: Vec<Value> = (1..=10).map(|i| json!({"amount": i})).collect();
        let q = Query::<Untyped>::new()
            .filter(FilterExpr::Cmp {
                field: "amount".into(),
                op: FilterOp::Gt,
                value: json!(6),
            })
            .sort_by("amount", SortDir::Desc)
            .page(1, 2);
        check(&schema(), &q).unwrap();
        let (page, total) = apply_in_memory(&q, rows);
        assert_eq!(total, 4);
        assert_eq!(page, vec![json!({"amount": 10}), json!({"amount": 9})]);
    }

    #[test]
    fn typed_query_json_roundtrip() {
        let q = Query::<Untyped>::new()
            .filter(FilterExpr::Not(Box::new(FilterExpr::Cmp {
                field: "status".into(),
                op: FilterOp::In,
                value: json!(["a", "b"]),
            })))
            .page(3, 10);
        let s = serde_json::to_string(&q).unwrap();
        let back: Query<Untyped> = serde_json::from_str(&s).unwrap();
        assert_eq!(back, q);
    }
}
