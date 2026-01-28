//! End-to-end tests simulating a client using the this-rs framework
//!
//! These tests verify the complete flow from HTTP request to response,
//! including entity CRUD operations and link management.

use anyhow::Result;
use axum_test::TestServer;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use this::prelude::*;
use uuid::Uuid;

// =============================================================================
// Test Entities
// =============================================================================

impl_data_entity!(TestProduct, "product", ["name", "sku"], {
    sku: String,
    price: f64,
    category: String,
});

impl_data_entity!(TestCategory, "category", ["name"], {
    description: String,
});

// =============================================================================
// Test Stores
// =============================================================================

#[derive(Clone)]
struct ProductStore {
    data: Arc<RwLock<HashMap<Uuid, TestProduct>>>,
}

impl ProductStore {
    fn new() -> Self {
        Self {
            data: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    fn add(&self, product: TestProduct) {
        self.data.write().unwrap().insert(product.id, product);
    }

    fn get(&self, id: &Uuid) -> Option<TestProduct> {
        self.data.read().unwrap().get(id).cloned()
    }

    fn list(&self) -> Vec<TestProduct> {
        self.data.read().unwrap().values().cloned().collect()
    }

    fn delete(&self, id: &Uuid) -> Option<TestProduct> {
        self.data.write().unwrap().remove(id)
    }
}

impl Default for ProductStore {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl EntityFetcher for ProductStore {
    async fn fetch_as_json(&self, entity_id: &Uuid) -> Result<Value> {
        let product = self
            .get(entity_id)
            .ok_or_else(|| anyhow::anyhow!("Product not found: {}", entity_id))?;
        Ok(serde_json::to_value(product)?)
    }

    async fn list_as_json(&self, limit: Option<i32>, offset: Option<i32>) -> Result<Vec<Value>> {
        let all = self.list();
        let offset = offset.unwrap_or(0) as usize;
        let limit = limit.unwrap_or(20) as usize;
        let items: Vec<TestProduct> = all.into_iter().skip(offset).take(limit).collect();
        items
            .into_iter()
            .map(|p| serde_json::to_value(p).map_err(Into::into))
            .collect()
    }
}

#[async_trait::async_trait]
impl EntityCreator for ProductStore {
    async fn create_from_json(&self, data: Value) -> Result<Value> {
        let product = TestProduct::new(
            data["name"].as_str().unwrap_or("Product").to_string(),
            data["status"].as_str().unwrap_or("active").to_string(),
            data["sku"].as_str().unwrap_or("SKU-000").to_string(),
            data["price"].as_f64().unwrap_or(0.0),
            data["category"].as_str().unwrap_or("general").to_string(),
        );
        self.add(product.clone());
        Ok(serde_json::to_value(product)?)
    }
}

#[derive(Clone)]
struct CategoryStore {
    data: Arc<RwLock<HashMap<Uuid, TestCategory>>>,
}

impl CategoryStore {
    fn new() -> Self {
        Self {
            data: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    fn add(&self, category: TestCategory) {
        self.data.write().unwrap().insert(category.id, category);
    }

    fn get(&self, id: &Uuid) -> Option<TestCategory> {
        self.data.read().unwrap().get(id).cloned()
    }

    fn list(&self) -> Vec<TestCategory> {
        self.data.read().unwrap().values().cloned().collect()
    }
}

impl Default for CategoryStore {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl EntityFetcher for CategoryStore {
    async fn fetch_as_json(&self, entity_id: &Uuid) -> Result<Value> {
        let category = self
            .get(entity_id)
            .ok_or_else(|| anyhow::anyhow!("Category not found: {}", entity_id))?;
        Ok(serde_json::to_value(category)?)
    }

    async fn list_as_json(&self, limit: Option<i32>, offset: Option<i32>) -> Result<Vec<Value>> {
        let all = self.list();
        let offset = offset.unwrap_or(0) as usize;
        let limit = limit.unwrap_or(20) as usize;
        let items: Vec<TestCategory> = all.into_iter().skip(offset).take(limit).collect();
        items
            .into_iter()
            .map(|c| serde_json::to_value(c).map_err(Into::into))
            .collect()
    }
}

#[async_trait::async_trait]
impl EntityCreator for CategoryStore {
    async fn create_from_json(&self, data: Value) -> Result<Value> {
        let category = TestCategory::new(
            data["name"].as_str().unwrap_or("Category").to_string(),
            data["status"].as_str().unwrap_or("active").to_string(),
            data["description"].as_str().unwrap_or("").to_string(),
        );
        self.add(category.clone());
        Ok(serde_json::to_value(category)?)
    }
}

// =============================================================================
// Test Entity Store (aggregated)
// =============================================================================

#[derive(Clone)]
struct TestEntityStore {
    products: ProductStore,
    categories: CategoryStore,
}

impl TestEntityStore {
    fn new() -> Self {
        Self {
            products: ProductStore::new(),
            categories: CategoryStore::new(),
        }
    }
}

// =============================================================================
// Test Module
// =============================================================================

struct TestModule {
    store: TestEntityStore,
}

impl TestModule {
    fn new(store: TestEntityStore) -> Self {
        Self { store }
    }
}

impl Module for TestModule {
    fn name(&self) -> &str {
        "test-module"
    }

    fn version(&self) -> &str {
        "1.0.0"
    }

    fn entity_types(&self) -> Vec<&str> {
        vec!["product", "category"]
    }

    fn links_config(&self) -> Result<LinksConfig> {
        // Define configuration inline for tests
        let yaml = r#"
entities:
  - singular: product
    plural: products
  - singular: category
    plural: categories

links:
  - link_type: has_product
    source_type: category
    target_type: product
    forward_route_name: products
    reverse_route_name: category
    description: "Category contains products"
"#;
        LinksConfig::from_yaml_str(yaml)
    }

    fn register_entities(&self, registry: &mut EntityRegistry) {
        registry.register(Box::new(ProductDescriptor::new(self.store.products.clone())));
        registry.register(Box::new(CategoryDescriptor::new(
            self.store.categories.clone(),
        )));
    }

    fn get_entity_fetcher(&self, entity_type: &str) -> Option<Arc<dyn EntityFetcher>> {
        match entity_type {
            "product" => Some(Arc::new(self.store.products.clone())),
            "category" => Some(Arc::new(self.store.categories.clone())),
            _ => None,
        }
    }

    fn get_entity_creator(&self, entity_type: &str) -> Option<Arc<dyn EntityCreator>> {
        match entity_type {
            "product" => Some(Arc::new(self.store.products.clone())),
            "category" => Some(Arc::new(self.store.categories.clone())),
            _ => None,
        }
    }
}

// =============================================================================
// Entity Descriptors
// =============================================================================

struct ProductDescriptor {
    store: ProductStore,
}

impl ProductDescriptor {
    fn new(store: ProductStore) -> Self {
        Self { store }
    }
}

impl EntityDescriptor for ProductDescriptor {
    fn entity_type(&self) -> &str {
        "product"
    }

    fn plural(&self) -> &str {
        "products"
    }

    fn build_routes(&self) -> axum::Router {
        let store = self.store.clone();

        axum::Router::new()
            .route(
                "/products",
                axum::routing::get({
                    let store = store.clone();
                    move || {
                        let store = store.clone();
                        async move {
                            let products = store.list();
                            axum::Json(serde_json::to_value(products).unwrap())
                        }
                    }
                })
                .post({
                    let store = store.clone();
                    move |axum::Json(payload): axum::Json<Value>| {
                        let store = store.clone();
                        async move {
                            let product = TestProduct::new(
                                payload["name"].as_str().unwrap_or("Product").to_string(),
                                payload["status"].as_str().unwrap_or("active").to_string(),
                                payload["sku"].as_str().unwrap_or("SKU-000").to_string(),
                                payload["price"].as_f64().unwrap_or(0.0),
                                payload["category"].as_str().unwrap_or("general").to_string(),
                            );
                            store.add(product.clone());
                            axum::Json(serde_json::to_value(product).unwrap())
                        }
                    }
                }),
            )
            .route(
                "/products/{id}",
                axum::routing::get({
                    let store = store.clone();
                    move |axum::extract::Path(id): axum::extract::Path<Uuid>| {
                        let store = store.clone();
                        async move {
                            match store.get(&id) {
                                Some(product) => axum::Json(serde_json::to_value(product).unwrap()),
                                None => axum::Json(json!({"error": "Not found"})),
                            }
                        }
                    }
                })
                .delete({
                    let store = store.clone();
                    move |axum::extract::Path(id): axum::extract::Path<Uuid>| {
                        let store = store.clone();
                        async move {
                            match store.delete(&id) {
                                Some(_) => axum::Json(json!({"deleted": true})),
                                None => axum::Json(json!({"error": "Not found"})),
                            }
                        }
                    }
                }),
            )
    }
}

struct CategoryDescriptor {
    store: CategoryStore,
}

impl CategoryDescriptor {
    fn new(store: CategoryStore) -> Self {
        Self { store }
    }
}

impl EntityDescriptor for CategoryDescriptor {
    fn entity_type(&self) -> &str {
        "category"
    }

    fn plural(&self) -> &str {
        "categories"
    }

    fn build_routes(&self) -> axum::Router {
        let store = self.store.clone();

        axum::Router::new()
            .route(
                "/categories",
                axum::routing::get({
                    let store = store.clone();
                    move || {
                        let store = store.clone();
                        async move {
                            let categories = store.list();
                            axum::Json(serde_json::to_value(categories).unwrap())
                        }
                    }
                })
                .post({
                    let store = store.clone();
                    move |axum::Json(payload): axum::Json<Value>| {
                        let store = store.clone();
                        async move {
                            let category = TestCategory::new(
                                payload["name"].as_str().unwrap_or("Category").to_string(),
                                payload["status"].as_str().unwrap_or("active").to_string(),
                                payload["description"].as_str().unwrap_or("").to_string(),
                            );
                            store.add(category.clone());
                            axum::Json(serde_json::to_value(category).unwrap())
                        }
                    }
                }),
            )
            .route(
                "/categories/{id}",
                axum::routing::get({
                    let store = store.clone();
                    move |axum::extract::Path(id): axum::extract::Path<Uuid>| {
                        let store = store.clone();
                        async move {
                            match store.get(&id) {
                                Some(category) => {
                                    axum::Json(serde_json::to_value(category).unwrap())
                                }
                                None => axum::Json(json!({"error": "Not found"})),
                            }
                        }
                    }
                }),
            )
    }
}

// =============================================================================
// Helper function to create test server
// =============================================================================

async fn create_test_server() -> (TestServer, TestEntityStore, Arc<InMemoryLinkService>) {
    let store = TestEntityStore::new();
    let link_service = Arc::new(InMemoryLinkService::new());

    let module = TestModule::new(store.clone());

    let app = ServerBuilder::new()
        .with_link_service((*link_service).clone())
        .register_module(module)
        .expect("Failed to register module")
        .build()
        .expect("Failed to build app");

    let server = TestServer::new(app).expect("Failed to create test server");

    (server, store, link_service)
}

// =============================================================================
// Health Check Tests
// =============================================================================

mod health_tests {
    use super::*;

    #[tokio::test]
    async fn test_health_endpoint() {
        let (server, _, _) = create_test_server().await;

        let response = server.get("/health").await;
        response.assert_status_ok();

        let body: Value = response.json();
        assert_eq!(body["status"], "ok");
    }

    #[tokio::test]
    async fn test_healthz_endpoint() {
        let (server, _, _) = create_test_server().await;

        let response = server.get("/healthz").await;
        response.assert_status_ok();

        let body: Value = response.json();
        assert_eq!(body["status"], "ok");
    }
}

// =============================================================================
// Entity CRUD Tests
// =============================================================================

mod entity_crud_tests {
    use super::*;

    #[tokio::test]
    async fn test_list_products_empty() {
        let (server, _, _) = create_test_server().await;

        let response = server.get("/products").await;
        response.assert_status_ok();

        let body: Vec<Value> = response.json();
        assert!(body.is_empty());
    }

    #[tokio::test]
    async fn test_create_product() {
        let (server, _, _) = create_test_server().await;

        let response = server
            .post("/products")
            .json(&json!({
                "name": "Test Product",
                "sku": "TEST-001",
                "price": 99.99,
                "category": "electronics",
                "status": "active"
            }))
            .await;

        response.assert_status_ok();

        let body: Value = response.json();
        assert_eq!(body["name"], "Test Product");
        assert_eq!(body["sku"], "TEST-001");
        assert_eq!(body["price"], 99.99);
        assert!(body["id"].as_str().is_some());
    }

    #[tokio::test]
    async fn test_get_product_by_id() {
        let (server, store, _) = create_test_server().await;

        // Create a product directly in the store
        let product = TestProduct::new(
            "Direct Product".to_string(),
            "active".to_string(),
            "DIRECT-001".to_string(),
            50.0,
            "test".to_string(),
        );
        let product_id = product.id;
        store.products.add(product);

        // Get via API
        let response = server.get(&format!("/products/{}", product_id)).await;
        response.assert_status_ok();

        let body: Value = response.json();
        assert_eq!(body["name"], "Direct Product");
        assert_eq!(body["id"], product_id.to_string());
    }

    #[tokio::test]
    async fn test_delete_product() {
        let (server, store, _) = create_test_server().await;

        // Create a product
        let product = TestProduct::new(
            "To Delete".to_string(),
            "active".to_string(),
            "DEL-001".to_string(),
            10.0,
            "test".to_string(),
        );
        let product_id = product.id;
        store.products.add(product);

        // Verify it exists
        assert!(store.products.get(&product_id).is_some());

        // Delete via API
        let response = server.delete(&format!("/products/{}", product_id)).await;
        response.assert_status_ok();

        // Verify it's gone
        assert!(store.products.get(&product_id).is_none());
    }

    #[tokio::test]
    async fn test_list_products_with_data() {
        let (server, store, _) = create_test_server().await;

        // Add multiple products
        for i in 0..3 {
            let product = TestProduct::new(
                format!("Product {}", i),
                "active".to_string(),
                format!("SKU-{:03}", i),
                (i as f64) * 10.0,
                "test".to_string(),
            );
            store.products.add(product);
        }

        let response = server.get("/products").await;
        response.assert_status_ok();

        let body: Vec<Value> = response.json();
        assert_eq!(body.len(), 3);
    }
}

// =============================================================================
// Link Tests
// =============================================================================

mod link_tests {
    use super::*;

    #[tokio::test]
    async fn test_create_and_list_links() {
        let (server, store, link_service) = create_test_server().await;

        // Create a category
        let category = TestCategory::new(
            "Electronics".to_string(),
            "active".to_string(),
            "Electronic devices".to_string(),
        );
        let category_id = category.id;
        store.categories.add(category);

        // Create products
        let product1 = TestProduct::new(
            "Phone".to_string(),
            "active".to_string(),
            "PHONE-001".to_string(),
            999.99,
            "electronics".to_string(),
        );
        let product1_id = product1.id;
        store.products.add(product1);

        let product2 = TestProduct::new(
            "Laptop".to_string(),
            "active".to_string(),
            "LAPTOP-001".to_string(),
            1499.99,
            "electronics".to_string(),
        );
        let product2_id = product2.id;
        store.products.add(product2);

        // Create links: category -> products
        let link1 = LinkEntity::new("has_product", category_id, product1_id, None);
        link_service.create(link1).await.unwrap();

        let link2 = LinkEntity::new("has_product", category_id, product2_id, None);
        link_service.create(link2).await.unwrap();

        // List products for category via link route
        let response = server
            .get(&format!("/categories/{}/products", category_id))
            .await;
        response.assert_status_ok();

        let body: Value = response.json();

        // The response should contain the links with enriched entities
        if let Some(links) = body.as_array() {
            assert_eq!(links.len(), 2);
        } else if body["links"].is_array() {
            assert_eq!(body["links"].as_array().unwrap().len(), 2);
        }
    }

    #[tokio::test]
    async fn test_reverse_link_navigation() {
        let (server, store, link_service) = create_test_server().await;

        // Create a category
        let category = TestCategory::new(
            "Books".to_string(),
            "active".to_string(),
            "Book category".to_string(),
        );
        let category_id = category.id;
        store.categories.add(category);

        // Create a product
        let product = TestProduct::new(
            "Rust Book".to_string(),
            "active".to_string(),
            "BOOK-001".to_string(),
            49.99,
            "books".to_string(),
        );
        let product_id = product.id;
        store.products.add(product);

        // Create link: category -> product
        let link = LinkEntity::new("has_product", category_id, product_id, None);
        link_service.create(link).await.unwrap();

        // Navigate reverse: product -> category
        let response = server
            .get(&format!("/products/{}/category", product_id))
            .await;
        response.assert_status_ok();
    }

    #[tokio::test]
    async fn test_link_with_metadata() {
        let (_, store, link_service) = create_test_server().await;

        let category = TestCategory::new(
            "Featured".to_string(),
            "active".to_string(),
            "Featured items".to_string(),
        );
        let category_id = category.id;
        store.categories.add(category);

        let product = TestProduct::new(
            "Featured Product".to_string(),
            "active".to_string(),
            "FEAT-001".to_string(),
            199.99,
            "featured".to_string(),
        );
        let product_id = product.id;
        store.products.add(product);

        // Create link with metadata
        let link = LinkEntity::new(
            "has_product",
            category_id,
            product_id,
            Some(json!({
                "featured_since": "2025-01-01",
                "priority": 1,
                "promotion": "New Year Sale"
            })),
        );
        let created_link = link_service.create(link).await.unwrap();

        // Verify metadata was stored
        assert!(created_link.metadata.is_some());
        let metadata = created_link.metadata.unwrap();
        assert_eq!(metadata["priority"], 1);
        assert_eq!(metadata["promotion"], "New Year Sale");
    }
}

// =============================================================================
// Error Handling Tests
// =============================================================================

mod error_tests {
    use super::*;

    #[tokio::test]
    async fn test_get_nonexistent_product() {
        let (server, _, _) = create_test_server().await;

        let fake_id = Uuid::new_v4();
        let response = server.get(&format!("/products/{}", fake_id)).await;

        // Should still return 200 but with error in body (framework behavior)
        let body: Value = response.json();
        assert!(body.get("error").is_some() || body.get("id").is_none());
    }

    #[tokio::test]
    async fn test_invalid_uuid_in_path() {
        let (server, _, _) = create_test_server().await;

        let response = server.get("/products/not-a-uuid").await;

        // Should return 400 or 404
        assert!(response.status_code().is_client_error());
    }

    #[tokio::test]
    async fn test_link_to_nonexistent_entity() {
        let (_, store, link_service) = create_test_server().await;

        let category = TestCategory::new(
            "Test".to_string(),
            "active".to_string(),
            "Test".to_string(),
        );
        let category_id = category.id;
        store.categories.add(category);

        let fake_product_id = Uuid::new_v4();

        // Create link to non-existent product (link service allows this)
        let link = LinkEntity::new("has_product", category_id, fake_product_id, None);
        let result = link_service.create(link).await;

        // Link service doesn't validate entity existence
        assert!(result.is_ok());
    }
}

// =============================================================================
// Concurrent Access Tests
// =============================================================================

mod concurrency_tests {
    use super::*;

    #[tokio::test]
    async fn test_sequential_product_creation() {
        let (server, _, _) = create_test_server().await;

        // Create products sequentially (TestServer doesn't support Clone)
        for i in 0..10 {
            let response = server
                .post("/products")
                .json(&json!({
                    "name": format!("Sequential Product {}", i),
                    "sku": format!("SEQ-{:03}", i),
                    "price": (i as f64) * 10.0,
                    "category": "test",
                    "status": "active"
                }))
                .await;
            response.assert_status_ok();
        }

        // Verify all products were created
        let response = server.get("/products").await;
        let body: Vec<Value> = response.json();
        assert_eq!(body.len(), 10);
    }

    #[tokio::test]
    async fn test_concurrent_link_creation() {
        let (_, store, link_service) = create_test_server().await;

        let category = TestCategory::new(
            "Concurrent Test".to_string(),
            "active".to_string(),
            "Test".to_string(),
        );
        let category_id = category.id;
        store.categories.add(category);

        // Create multiple products
        let mut product_ids = vec![];
        for i in 0..20 {
            let product = TestProduct::new(
                format!("Product {}", i),
                "active".to_string(),
                format!("SKU-{:03}", i),
                10.0,
                "test".to_string(),
            );
            product_ids.push(product.id);
            store.products.add(product);
        }

        // Create links concurrently
        let mut handles = vec![];
        for product_id in product_ids {
            let ls = Arc::clone(&link_service);
            let handle = tokio::spawn(async move {
                let link = LinkEntity::new("has_product", category_id, product_id, None);
                ls.create(link).await
            });
            handles.push(handle);
        }

        for handle in handles {
            let result = handle.await.unwrap();
            assert!(result.is_ok());
        }

        // Verify all links were created
        let links = link_service
            .find_by_source(&category_id, Some("has_product"), None)
            .await
            .unwrap();
        assert_eq!(links.len(), 20);
    }
}

// =============================================================================
// Framework Integration Tests
// =============================================================================

mod framework_integration_tests {
    use super::*;

    #[tokio::test]
    async fn test_full_workflow() {
        let (server, _, _) = create_test_server().await;

        // 1. Create a category
        let response = server
            .post("/categories")
            .json(&json!({
                "name": "Workflow Test Category",
                "description": "Testing complete workflow",
                "status": "active"
            }))
            .await;
        response.assert_status_ok();
        let category: Value = response.json();
        let category_id = category["id"].as_str().unwrap();

        // 2. Create multiple products
        let mut product_ids = vec![];
        for i in 1..=3 {
            let response = server
                .post("/products")
                .json(&json!({
                    "name": format!("Workflow Product {}", i),
                    "sku": format!("WF-{:03}", i),
                    "price": (i as f64) * 100.0,
                    "category": "workflow",
                    "status": "active"
                }))
                .await;
            response.assert_status_ok();
            let product: Value = response.json();
            product_ids.push(product["id"].as_str().unwrap().to_string());
        }

        // 3. Verify products were created
        let response = server.get("/products").await;
        let products: Vec<Value> = response.json();
        assert_eq!(products.len(), 3);

        // 4. Verify category was created
        let response = server.get(&format!("/categories/{}", category_id)).await;
        response.assert_status_ok();
        let cat: Value = response.json();
        assert_eq!(cat["name"], "Workflow Test Category");

        // 5. Clean up - delete products
        for product_id in &product_ids {
            let response = server.delete(&format!("/products/{}", product_id)).await;
            response.assert_status_ok();
        }

        // 6. Verify products are gone
        let response = server.get("/products").await;
        let products: Vec<Value> = response.json();
        assert!(products.is_empty());
    }

    #[tokio::test]
    async fn test_entity_types_metadata() {
        let (server, store, _) = create_test_server().await;

        // Add some data
        let product = TestProduct::new(
            "Metadata Test".to_string(),
            "active".to_string(),
            "META-001".to_string(),
            10.0,
            "test".to_string(),
        );
        store.products.add(product.clone());

        // Verify entity type field is correctly set
        let response = server.get(&format!("/products/{}", product.id)).await;
        let body: Value = response.json();
        assert_eq!(body["type"], "product");
    }
}
