//! Complete example with Axum server
//!
//! This example demonstrates:
//! - Setting up an Axum server with link routes
//! - Multiple entity types (User, Car, Company)
//! - Multiple link types between same entities (owner, driver)
//! - Bidirectional navigation
//! - Multi-tenant isolation

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use this::prelude::*;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct User {
    id: Uuid,
    tenant_id: Uuid,
    name: String,
    email: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Car {
    id: Uuid,
    tenant_id: Uuid,
    brand: String,
    model: String,
    year: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Company {
    id: Uuid,
    tenant_id: Uuid,
    name: String,
    registration_number: String,
}

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize tracing
    tracing_subscriber::fmt::init();

    println!("🚀 This-RS Full API Example");
    println!("============================\n");

    // Load configuration
    let config = Arc::new(LinksConfig::from_yaml_file("links.yaml")?);
    println!("✅ Loaded configuration with:");
    println!("   - {} entities", config.entities.len());
    println!("   - {} link definitions\n", config.links.len());

    // Create services
    let link_service: Arc<dyn LinkService> = Arc::new(InMemoryLinkService::new());
    let registry = Arc::new(LinkRouteRegistry::new(config.clone()));

    // Create application state
    let app_state = AppState {
        link_service: link_service.clone(),
        config: config.clone(),
        registry: registry.clone(),
        entity_fetchers: Arc::new(HashMap::new()),
    };

    // Setup some test data
    let tenant_id = Uuid::new_v4();
    println!("📋 Setting up test data...");
    println!("   Tenant ID: {}\n", tenant_id);

    let alice_id = Uuid::new_v4();
    let bob_id = Uuid::new_v4();
    let tesla_id = Uuid::new_v4();
    let bmw_id = Uuid::new_v4();
    let acme_corp_id = Uuid::new_v4();

    println!("👥 Users:");
    println!("   - Alice: {}", alice_id);
    println!("   - Bob:   {}\n", bob_id);

    println!("🚗 Cars:");
    println!("   - Tesla: {}", tesla_id);
    println!("   - BMW:   {}\n", bmw_id);

    println!("🏢 Companies:");
    println!("   - ACME Corp: {}\n", acme_corp_id);

    // Create links
    println!("🔗 Creating links...");

    // Alice owns Tesla
    link_service
        .create(
            &tenant_id,
            "owner",
            EntityReference::new(alice_id, "user"),
            EntityReference::new(tesla_id, "car"),
            None,
        )
        .await?;
    println!("   ✓ Alice owns Tesla");

    // Alice drives Tesla
    link_service
        .create(
            &tenant_id,
            "driver",
            EntityReference::new(alice_id, "user"),
            EntityReference::new(tesla_id, "car"),
            None,
        )
        .await?;
    println!("   ✓ Alice drives Tesla");

    // Bob drives Tesla (shared car!)
    link_service
        .create(
            &tenant_id,
            "driver",
            EntityReference::new(bob_id, "user"),
            EntityReference::new(tesla_id, "car"),
            Some(serde_json::json!({
                "permission_level": "limited",
                "max_speed": 120
            })),
        )
        .await?;
    println!("   ✓ Bob drives Tesla (with metadata)");

    // Bob owns BMW
    link_service
        .create(
            &tenant_id,
            "owner",
            EntityReference::new(bob_id, "user"),
            EntityReference::new(bmw_id, "car"),
            None,
        )
        .await?;
    println!("   ✓ Bob owns BMW");

    // Alice works at ACME Corp
    link_service
        .create(
            &tenant_id,
            "worker",
            EntityReference::new(alice_id, "user"),
            EntityReference::new(acme_corp_id, "company"),
            Some(serde_json::json!({
                "role": "Senior Developer",
                "start_date": "2024-01-01"
            })),
        )
        .await?;
    println!("   ✓ Alice works at ACME Corp\n");

    // Build the router
    let app = Router::new()
        // Link routes - list (forward and reverse)
        .route("/:entity_type/:entity_id/:route_name", get(list_links))
        // Link routes - create and delete (direct)
        .route(
            "/:source_type/:source_id/:link_type/:target_type/:target_id",
            post(create_link).delete(delete_link),
        )
        // Introspection
        .route("/:entity_type/:entity_id/links", get(list_available_links))
        .with_state(app_state);

    // Start the server
    let addr = SocketAddr::from(([127, 0, 0, 1], 3000));
    println!("🌐 Server starting on http://{}\n", addr);

    println!("📖 Available endpoints:");
    println!("   GET  /users/{{id}}/cars-owned       - Cars owned by user");
    println!("   GET  /users/{{id}}/cars-driven      - Cars driven by user");
    println!("   GET  /users/{{id}}/companies-work   - Companies where user works");
    println!("   GET  /cars/{{id}}/users-owners      - Owners of a car");
    println!("   GET  /cars/{{id}}/users-drivers     - Drivers of a car");
    println!("   GET  /users/{{id}}/links            - All available routes for user");
    println!("   POST /users/{{id}}/owner/cars/{{id}} - Create ownership link");
    println!();

    println!("💡 Example requests:");
    println!("   # List cars owned by Alice");
    println!(
        "   curl -H 'X-Tenant-ID: {}' http://localhost:3000/users/{}/cars-owned",
        tenant_id, alice_id
    );
    println!();
    println!("   # List drivers of Tesla");
    println!(
        "   curl -H 'X-Tenant-ID: {}' http://localhost:3000/cars/{}/users-drivers",
        tenant_id, tesla_id
    );
    println!();
    println!("   # Discover available routes for Alice");
    println!(
        "   curl -H 'X-Tenant-ID: {}' http://localhost:3000/users/{}/links",
        tenant_id, alice_id
    );
    println!();

    let listener = tokio::net::TcpListener::bind(addr).await?;
    println!("✅ Server is ready! Press Ctrl+C to stop.\n");

    axum::serve(listener, app).await?;

    Ok(())
}
