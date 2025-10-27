//! Query parameters and pagination utilities

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
#[derive(Debug, Clone, Deserialize, Default)]
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
    /// ```
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
    /// ```
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
    pub fn filter_value(&self) -> Option<Value> {
        self.filter
            .as_ref()
            .and_then(|s| serde_json::from_str(s).ok())
    }
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
}
