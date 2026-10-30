//! Query parameters and pagination utilities

// `ContractError` is the shared transport error (same choice as `this-descriptor`).
#![allow(clippy::result_large_err)]

use crate::core::errors::ApiError;
use crate::descriptor::engine::query as engine;
use crate::descriptor::framework::codes;
use crate::descriptor::{ContractError, EntityDesc, Page, Query, QuerySchema};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Query parameters for pagination and filtering
///
/// This structure is used to extract pagination and filtering parameters
/// from URL query strings. All parameters have sensible defaults.
///
/// # Example
/// ```rust,ignore
/// // In handler:
/// pub async fn list_items(
///     Query(params): Query<QueryParams>,
/// ) -> Json<PaginatedResponse<Item>> {
///     // params.page defaults to 1
///     // params.limit defaults to 20
/// }
///
/// // Usage:
/// GET /items?page=2&limit=10
/// GET /items?filter={"status": "active"}
/// GET /items?page=1&limit=20&filter={"amount>": 100}&sort=created_at:desc
/// ```
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct QueryParams {
    /// Page number (starts at 1)
    #[serde(default = "default_page")]
    pub page: usize,

    /// Number of items per page
    #[serde(default = "default_limit")]
    pub limit: usize,

    /// Filters as JSON object
    ///
    /// # Format
    /// - Exact match: `{"field": "value"}`
    /// - Comparison: `{"field>": value, "field<": value, "field>=": value, "field<=": value}`
    ///
    /// # Example
    /// ```text
    /// filter={"status": "active", "amount>": 100, "customer_name": "Acme"}
    /// ```
    pub filter: Option<String>,

    /// Sort field and direction
    ///
    /// # Format
    /// - `field:asc` or `field` (ascending)
    /// - `field:desc` (descending)
    ///
    /// # Example
    /// ```text
    /// sort=amount:desc
    /// sort=created_at:asc
    /// ```
    pub sort: Option<String>,
}

fn default_page() -> usize {
    1
}

fn default_limit() -> usize {
    20
}

impl Default for QueryParams {
    fn default() -> Self {
        Self {
            page: default_page(),
            limit: default_limit(),
            filter: None,
            sort: None,
        }
    }
}

impl QueryParams {
    /// Get page number, ensuring minimum of 1
    pub fn page(&self) -> usize {
        self.page.max(1)
    }

    /// Get limit, ensuring it doesn't exceed the maximum
    pub fn limit(&self) -> usize {
        self.limit.clamp(1, 100) // Maximum 100 per page, minimum 1
    }

    /// Parse filter JSON string into Value
    ///
    /// **Lenient legacy accessor**: a malformed filter is silently `None` (the list is then
    /// NOT filtered). New code uses [`QueryParams::try_filter_value`],
    /// [`QueryParams::to_query`] or the [`ListQuery`] extractor, which refuse it.
    pub fn filter_value(&self) -> Option<Value> {
        self.filter
            .as_ref()
            .and_then(|s| serde_json::from_str(s).ok())
    }

    /// Strict filter: `Ok(None)` when absent or blank, `Err(this.query.malformed_filter)`
    /// when it is not a JSON object.
    pub fn try_filter_value(&self) -> Result<Option<Value>, ContractError> {
        match self.filter.as_deref().filter(|f| !f.trim().is_empty()) {
            None => Ok(None),
            Some(f) => {
                engine::parse_filter_json(f)?;
                Ok(serde_json::from_str(f).ok())
            }
        }
    }

    /// Typed query checked against the entity's descriptor (`filterable` operators,
    /// `sortable` fields, page size). Malformed or forbidden = `ContractError` (400).
    pub fn to_query(&self, entity: &EntityDesc) -> Result<Query, ContractError> {
        engine::parse_legacy(
            &QuerySchema::for_entity(entity),
            self.filter.as_deref(),
            self.sort.as_deref(),
            self.page(),
            self.limit(),
        )
    }

    /// Typed query WITHOUT a schema (entity without descriptor): filter and sort are parsed
    /// strictly (malformed = error), any field may be referenced.
    pub fn to_query_unchecked(&self) -> Result<Query, ContractError> {
        let mut q = Query::new().page(self.page() as u32, self.limit() as u32);
        if let Some(f) = self.filter.as_deref().filter(|f| !f.trim().is_empty()) {
            q.filter = Some(engine::parse_filter_json(f)?);
        }
        if let Some(s) = self.sort.as_deref().filter(|s| !s.trim().is_empty()) {
            q.sort = engine::parse_sort(s)?;
        }
        Ok(q)
    }
}

/// A list query ready to run (`EntityFetcher::list_query`).
///
/// Extractor: when the REST validation layer has seen a DESCRIBED entity, the query it
/// checked against the descriptor is reused; otherwise `?filter=&sort=&page=&limit=` is
/// parsed strictly without schema. Either way a malformed filter or sort is a 400
/// `ContractError`, never silently ignored.
///
/// ```rust,ignore
/// async fn list_invoices(State(s): State<AppState>, q: ListQuery) -> Result<Json<Value>, ApiError> {
///     let (rows, total) = s.fetcher.list_query(&q.0).await.map_err(|e| ApiError(from_anyhow(&e)))?;
///     ...
/// }
/// ```
#[derive(Debug, Clone, PartialEq)]
pub struct ListQuery(pub Query);

impl<S: Send + Sync> axum::extract::FromRequestParts<S> for ListQuery {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut axum::http::request::Parts,
        _state: &S,
    ) -> Result<Self, Self::Rejection> {
        if let Some(q) = parts.extensions.get::<ListQuery>() {
            return Ok(q.clone());
        }
        let params = axum::extract::Query::<QueryParams>::try_from_uri(&parts.uri)
            .map_err(|e| {
                ApiError(
                    ContractError::framework(codes::TRANSPORT_BAD_REQUEST)
                        .with_param("reason", e.to_string()),
                )
            })?
            .0;
        params.to_query_unchecked().map(ListQuery).map_err(ApiError)
    }
}

/// Build a list query from protocol arguments (GraphQL `filter`/`sort`, gRPC
/// `filter`/`sort`), with an OFFSET (not a page): checked against `entity` when it has a
/// descriptor, strict but schema-less otherwise. `limit = None` means `MAX_LIMIT`.
pub fn query_from_args(
    entity: Option<&EntityDesc>,
    filter: Option<Value>,
    sort: Option<&str>,
    limit: Option<u32>,
    offset: Option<u32>,
) -> Result<Query, ContractError> {
    let mut q = Query::new();
    if let Some(f) =
        filter.filter(|f| !f.is_null() && f.as_str().is_none_or(|s| !s.trim().is_empty()))
    {
        q.filter = Some(engine::parse_filter_value(f)?);
    }
    if let Some(s) = sort.filter(|s| !s.trim().is_empty()) {
        q.sort = engine::parse_sort(s)?;
    }
    q.page = Page::Cursor {
        after: offset.filter(|o| *o > 0).map(|o| o.to_string()),
        limit: limit
            .filter(|l| *l > 0)
            .unwrap_or(crate::descriptor::query::MAX_LIMIT)
            .min(crate::descriptor::query::MAX_LIMIT),
    };
    if let Some(e) = entity {
        engine::check(&QuerySchema::for_entity(e), &q)?;
    }
    Ok(q)
}

/// Paginated response structure
///
/// This structure wraps paginated data with metadata about pagination state.
#[derive(Debug, Serialize)]
pub struct PaginatedResponse<T> {
    /// The paginated data
    pub data: Vec<T>,

    /// Pagination metadata
    pub pagination: PaginationMeta,
}

/// Pagination metadata
#[derive(Debug, Serialize)]
pub struct PaginationMeta {
    /// Current page number (starts at 1)
    pub page: usize,

    /// Number of items per page
    pub limit: usize,

    /// Total number of items (after filters)
    pub total: usize,

    /// Total number of pages
    pub total_pages: usize,

    /// Whether there is a next page
    pub has_next: bool,

    /// Whether there is a previous page
    pub has_prev: bool,
}

impl PaginationMeta {
    /// Create pagination metadata from calculation
    pub fn new(page: usize, limit: usize, total: usize) -> Self {
        // Ensure limit is at least 1 to avoid division by zero
        let limit = limit.max(1);
        let total_pages = if total == 0 { 0 } else { total.div_ceil(limit) }; // Ceiling division
        let start = (page - 1) * limit;

        Self {
            page,
            limit,
            total,
            total_pages,
            has_next: start + limit < total,
            has_prev: page > 1,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_query_params_defaults() {
        let params = QueryParams::default();
        assert_eq!(params.page(), 1);
        assert_eq!(params.limit(), 20);
    }

    #[test]
    fn test_pagination_meta() {
        let meta = PaginationMeta::new(1, 20, 145);
        assert_eq!(meta.total, 145);
        assert_eq!(meta.total_pages, 8);
        assert!(!meta.has_prev);
        assert!(meta.has_next);
    }

    // --- QueryParams::page edge cases ---

    #[test]
    fn test_query_params_page_zero_clamps_to_one() {
        let params = QueryParams {
            page: 0,
            ..Default::default()
        };
        assert_eq!(params.page(), 1);
    }

    #[test]
    fn test_query_params_page_positive_unchanged() {
        let params = QueryParams {
            page: 5,
            ..Default::default()
        };
        assert_eq!(params.page(), 5);
    }

    // --- QueryParams::limit edge cases ---

    #[test]
    fn test_query_params_limit_zero_clamps_to_one() {
        let params = QueryParams {
            limit: 0,
            ..Default::default()
        };
        assert_eq!(params.limit(), 1);
    }

    #[test]
    fn test_query_params_limit_over_100_clamps_to_100() {
        let params = QueryParams {
            limit: 101,
            ..Default::default()
        };
        assert_eq!(params.limit(), 100);
    }

    #[test]
    fn test_query_params_limit_within_range() {
        let params = QueryParams {
            limit: 50,
            ..Default::default()
        };
        assert_eq!(params.limit(), 50);
    }

    // --- filter_value ---

    #[test]
    fn test_filter_value_valid_json_object() {
        let params = QueryParams {
            filter: Some(r#"{"status": "active"}"#.to_string()),
            ..Default::default()
        };
        let value = params
            .filter_value()
            .expect("valid JSON should parse successfully");
        assert_eq!(value["status"], "active");
    }

    #[test]
    fn test_filter_value_invalid_json_returns_none() {
        let params = QueryParams {
            filter: Some("not-json".to_string()),
            ..Default::default()
        };
        assert!(params.filter_value().is_none());
    }

    fn invoice_desc() -> EntityDesc {
        use crate::descriptor::{FieldDesc, FieldType, FilterOp};
        let mut e = EntityDesc::new("invoice");
        e.fields = vec![
            FieldDesc::new("amount", FieldType::F64)
                .filterable(&[FilterOp::Gt])
                .sortable(),
            FieldDesc::new("note", FieldType::String),
        ];
        e
    }

    #[test]
    fn malformed_filter_is_error_not_ignored() {
        let params = QueryParams {
            filter: Some("not-json".to_string()),
            ..Default::default()
        };
        // The legacy accessor still swallows it (kept for compatibility)...
        assert!(params.filter_value().is_none());
        // ...every typed path refuses it.
        for err in [
            params.try_filter_value().unwrap_err(),
            params.to_query(&invoice_desc()).unwrap_err(),
            params.to_query_unchecked().unwrap_err(),
        ] {
            assert_eq!(err.code.as_str(), "this.query.malformed_filter");
            assert_eq!(err.kind.http_status(), 400);
        }
    }

    #[test]
    fn to_query_checks_against_descriptor_and_keeps_paging() {
        let params = QueryParams {
            page: 2,
            limit: 500,
            filter: Some(r#"{"amount>": 10}"#.into()),
            sort: Some("amount:desc".into()),
        };
        let q = params.to_query(&invoice_desc()).unwrap();
        assert_eq!(
            q.page,
            Page::Offset {
                page: 2,
                limit: 100
            }
        );
        assert_eq!(q.sort[0].field, "amount");
        let bad = QueryParams {
            sort: Some("note".into()),
            ..Default::default()
        };
        assert_eq!(
            bad.to_query(&invoice_desc()).unwrap_err().code.as_str(),
            "this.query.field_not_sortable"
        );
        // Without a descriptor any field may be referenced, but the syntax stays strict.
        assert!(bad.to_query_unchecked().is_ok());
    }

    #[test]
    fn query_from_args_offset_and_schema() {
        let q = query_from_args(
            Some(&invoice_desc()),
            Some(serde_json::json!({"amount>": 1})),
            Some("-amount"),
            Some(5),
            Some(10),
        )
        .unwrap();
        assert_eq!(
            q.page,
            Page::Cursor {
                after: Some("10".into()),
                limit: 5
            }
        );
        assert!(
            query_from_args(Some(&invoice_desc()), Some("{x".into()), None, None, None).is_err()
        );
        // Blank / null filter = no filter.
        let q = query_from_args(None, Some(Value::Null), Some(" "), None, None).unwrap();
        assert!(q.filter.is_none() && q.sort.is_empty());
    }

    #[tokio::test]
    async fn list_query_extractor_parses_strictly_or_reuses_layer_query() {
        use axum::extract::FromRequestParts;
        let (mut parts, _) = axum::http::Request::builder()
            .uri("/orders?filter=%7Boops")
            .body(())
            .unwrap()
            .into_parts();
        let err = ListQuery::from_request_parts(&mut parts, &())
            .await
            .unwrap_err();
        assert_eq!(err.0.code.as_str(), "this.query.malformed_filter");

        let (mut parts, _) = axum::http::Request::builder()
            .uri("/orders?filter=%7Boops")
            .body(())
            .unwrap()
            .into_parts();
        let checked = ListQuery(Query::new().page(1, 3));
        parts.extensions.insert(checked.clone());
        assert_eq!(
            ListQuery::from_request_parts(&mut parts, &())
                .await
                .unwrap(),
            checked
        );
    }

    #[test]
    fn test_filter_value_none_returns_none() {
        let params = QueryParams {
            filter: None,
            ..Default::default()
        };
        assert!(params.filter_value().is_none());
    }

    // --- PaginationMeta edge cases ---

    #[test]
    fn test_pagination_meta_total_zero() {
        let meta = PaginationMeta::new(1, 20, 0);
        assert_eq!(meta.total_pages, 0);
        assert!(!meta.has_next);
        assert!(!meta.has_prev);
    }

    #[test]
    fn test_pagination_meta_last_page() {
        // 100 items, 20 per page => 5 pages. Page 5 is the last.
        let meta = PaginationMeta::new(5, 20, 100);
        assert_eq!(meta.total_pages, 5);
        assert!(!meta.has_next);
        assert!(meta.has_prev);
    }

    #[test]
    fn test_pagination_meta_single_page() {
        let meta = PaginationMeta::new(1, 20, 10);
        assert_eq!(meta.total_pages, 1);
        assert!(!meta.has_next);
        assert!(!meta.has_prev);
    }

    #[test]
    fn test_pagination_meta_middle_page() {
        let meta = PaginationMeta::new(3, 10, 50);
        assert_eq!(meta.total_pages, 5);
        assert!(meta.has_next);
        assert!(meta.has_prev);
    }

    #[test]
    fn test_pagination_meta_limit_zero_treated_as_one() {
        // Limit 0 should be clamped to 1 to avoid division by zero
        let meta = PaginationMeta::new(1, 0, 10);
        assert_eq!(meta.limit, 1);
        assert_eq!(meta.total_pages, 10);
    }
}
