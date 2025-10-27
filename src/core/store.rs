//! Store traits for filtering and sorting

use serde_json::Value;

/// Trait for stores that support filtering and sorting
///
/// Implement this trait for stores that support generic querying with
/// filters and sorting capabilities.
pub trait QueryableStore<T>: Send + Sync {
    /// Apply filters to a collection of entities
    ///
    /// # Parameters
    /// - `data`: Collection of entities to filter
    /// - `filter`: Filter criteria as JSON Value
    ///
    /// # Returns
    /// Filtered collection
    fn apply_filters(&self, data: Vec<T>, filter: &Value) -> Vec<T>;

    /// Apply sorting to a collection of entities
    ///
    /// # Parameters
    /// - `data`: Collection of entities to sort (will be modified)
    /// - `sort`: Sort expression (e.g., "field:asc" or "field:desc")
    ///
    /// # Returns
    /// Sorted collection
    fn apply_sort(&self, data: Vec<T>, sort: &str) -> Vec<T>;

    /// Get all entities (unfiltered, unsorted)
    fn list_all(&self) -> Vec<T>;
}
