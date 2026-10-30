//! Integration tests for InMemory storage backends using the storage test harness.
//!
//! Invokes `data_service_tests!`, `link_service_tests!`, and `rest_integration_tests!`
//! to validate that InMemory storage backends fully conform to their contracts.

#[macro_use]
mod storage_harness;

use storage_harness::*;
use this::storage::{InMemoryDataService, InMemoryLinkService};

data_service_tests!(InMemoryDataService::<TestDataEntity>::new());
link_service_tests!(InMemoryLinkService::new());
rest_integration_tests!(InMemoryDataService::<TestDataEntity>::new());

// ---------------------------------------------------------------------------
// Native `list_query` (EntityFetcher) — filter/sort/page evaluated in the store
// ---------------------------------------------------------------------------

mod list_query_native_tests {
    use super::*;
    use serde_json::json;
    use this::core::EntityFetcher;
    use this::core::service::DataService;
    use this::descriptor::{FilterExpr, FilterOp, Query, SortDir};

    #[tokio::test]
    async fn in_memory_list_query_filters_sorts_and_paginates_natively() {
        let svc = InMemoryDataService::<TestDataEntity>::new();
        for e in sample_batch(25) {
            svc.create(e).await.unwrap();
        }

        // ages are 20..44; filter age >= 40, sort by name asc, page 2 of 2.
        let q = Query::new()
            .filter(FilterExpr::Cmp {
                field: "age".into(),
                op: FilterOp::Gte,
                value: json!(40),
            })
            .sort_by("name", SortDir::Asc)
            .page(2, 2);

        let (page, total) = svc.list_query(&q).await.expect("native list_query");

        assert_eq!(total, 5, "ages 40..44 match");
        assert_eq!(page.len(), 2);
        assert_eq!(page[0]["name"], json!("Entity_22"));
        assert_eq!(page[1]["name"], json!("Entity_23"));
    }

    #[tokio::test]
    async fn in_memory_list_query_native_override_is_not_bounded_by_the_default_cap() {
        let svc = InMemoryDataService::<TestDataEntity>::new();
        // 10_001 rows: the DEFAULT list_query would refuse above 10_000 with
        // `this.query.too_large`; the native override filters in the store itself.
        for i in 0..10_001i64 {
            let e = create_test_entity(
                &format!("Entity_{}", i),
                &format!("e{}@test.com", i),
                i,
                0.5,
                i % 2 == 0,
            );
            svc.create(e).await.unwrap();
        }

        let q = Query::new().filter(FilterExpr::Cmp {
            field: "age".into(),
            op: FilterOp::Eq,
            value: json!(5_000),
        });

        let (page, total) = svc
            .list_query(&q)
            .await
            .expect("native override ignores the default cap");

        assert_eq!(total, 1);
        assert_eq!(page.len(), 1);
        assert_eq!(page[0]["name"], json!("Entity_5000"));
    }
}
