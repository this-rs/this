//! Integration tests to validate the fixes applied to the codebase:
//! 1. tokio::sync::RwLock instead of std::sync::RwLock (async-safe)
//! 2. OnceLock<String> instead of Box::leak (no memory leak)
//! 3. Entity trait without Service (SRP compliance)

use this::prelude::*;

// Test entity using the macro to verify OnceLock<String> works correctly
impl_data_entity!(
    FixTestEntity,
    "fix_test_entity",
    ["name"],
    {
        value: i32,
    }
);

impl_link_entity!(
    FixTestLink,
    "fix_test_link",
    {
        weight: f64,
    }
);

mod entity_trait_srp_tests {
    use super::*;

    /// Test that Entity trait no longer requires Service type
    /// This validates the SRP fix - Entity should not know about its Service
    #[test]
    fn test_entity_trait_has_no_service_type() {
        // If this compiles, the Entity trait no longer has type Service
        // The macro-generated entity should work without implementing service_from_host
        let entity = FixTestEntity::new("Test".to_string(), "active".to_string(), 42);

        assert_eq!(entity.name(), "Test");
        assert_eq!(entity.value, 42);
        assert_eq!(entity.entity_type(), "fix_test_entity");
    }

    /// Test that Link entity also works without Service type
    #[test]
    fn test_link_entity_has_no_service_type() {
        let link = FixTestLink::new(
            "connection".to_string(),
            Uuid::new_v4(),
            Uuid::new_v4(),
            "active".to_string(),
            0.5,
        );

        assert_eq!(link.link_type(), "connection");
        assert_eq!(link.weight, 0.5);
    }
}

mod memory_leak_fix_tests {
    use super::*;

    /// Test that resource_name() returns consistent values
    /// This validates that OnceLock<String> works correctly
    #[test]
    fn test_resource_name_consistency() {
        // Call multiple times to ensure OnceLock initialization is stable
        let name1 = FixTestEntity::resource_name();
        let name2 = FixTestEntity::resource_name();
        let name3 = FixTestEntity::resource_name();

        assert_eq!(name1, name2);
        assert_eq!(name2, name3);
        assert_eq!(name1, "fix_test_entities"); // Pluralized
    }

    /// Test that link entity resource_name also uses OnceLock correctly
    #[test]
    fn test_link_resource_name_consistency() {
        let name1 = FixTestLink::resource_name();
        let name2 = FixTestLink::resource_name();

        assert_eq!(name1, name2);
        assert_eq!(name1, "fix_test_links"); // Pluralized
    }

    /// Test singular name is still static
    #[test]
    fn test_singular_name_is_static() {
        assert_eq!(FixTestEntity::resource_name_singular(), "fix_test_entity");
        assert_eq!(FixTestLink::resource_name_singular(), "fix_test_link");
    }
}

mod async_rwlock_tests {
    use super::*;
    use std::sync::Arc;
    use this::storage::InMemoryLinkService;

    /// Test that InMemoryLinkService can be shared across async tasks
    #[tokio::test]
    async fn test_service_is_send_sync() {
        let service = Arc::new(InMemoryLinkService::new());

        // This should compile and run without issues
        // proving the service is Send + Sync
        let service_clone = Arc::clone(&service);
        let handle = tokio::spawn(async move {
            let link = LinkEntity::new("test", Uuid::new_v4(), Uuid::new_v4(), None);
            service_clone.create(link).await.unwrap()
        });

        let created_link = handle.await.unwrap();
        assert_eq!(created_link.link_type, "test");
    }

    /// Test that async RwLock allows concurrent readers
    #[tokio::test]
    async fn test_concurrent_readers_not_blocked() {
        let service = Arc::new(InMemoryLinkService::new());

        // Create initial data
        let link = LinkEntity::new("initial", Uuid::new_v4(), Uuid::new_v4(), None);
        service.create(link).await.unwrap();

        // Spawn many readers simultaneously
        let mut handles = vec![];
        for _ in 0..10 {
            let svc = Arc::clone(&service);
            handles.push(tokio::spawn(async move { svc.list().await.unwrap().len() }));
        }

        // All should complete with correct count
        for handle in handles {
            assert_eq!(handle.await.unwrap(), 1);
        }
    }

    /// Test that tokio RwLock doesn't cause deadlocks in async context
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn test_no_deadlock_under_contention() {
        let service = Arc::new(InMemoryLinkService::new());
        let mut handles = vec![];

        // Mix of reads and writes with high contention
        for i in 0..50 {
            let svc = Arc::clone(&service);
            if i % 3 == 0 {
                // Write
                handles.push(tokio::spawn(async move {
                    let link =
                        LinkEntity::new(&format!("type_{}", i), Uuid::new_v4(), Uuid::new_v4(), None);
                    svc.create(link).await.unwrap();
                    true
                }));
            } else {
                // Read
                handles.push(tokio::spawn(async move {
                    let _ = svc.list().await.unwrap();
                    true
                }));
            }
        }

        // All operations should complete without deadlock
        // The test will timeout if there's a deadlock
        for handle in handles {
            assert!(handle.await.unwrap());
        }
    }
}
