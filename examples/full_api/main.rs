//! Complete example with Axum server
//!
//! This example demonstrates:
//! - Setting up an Axum server with link routes
//! - Multiple entity types (User, Car, Company)
//! - Multiple link types between same entities (owner, driver)
//! - Bidirectional navigation

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use this::prelude::*;

// Using the new macro-based entity definitions
impl_data_entity!(User, "user", ["name", "email"], {
    email: String,
});

impl_data_entity!(Car, "car", ["name", "brand", "model"], {
    brand: String,
    model: String,
    year: i32,
});

impl_data_entity!(Company, "company", ["name", "registration_number"], {
    registration_number: String,
});

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize tracing
    tracing_subscriber::fmt::init();

    println!("🚀 this-rs Full API Example");
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
        entity_creators: Arc::new(HashMap::new()),
    };

    // Setup some test data
    println!("📋 Setting up test data...\n");

    // Create entities using the new macro-generated methods
    let alice = User::new(
        "Alice".to_string(),
        "active".to_string(),
        "alice@example.com".to_string(),
    );
    let bob = User::new(
        "Bob".to_string(),
        "active".to_string(),
        "bob@example.com".to_string(),
    );
    let tesla = Car::new(
        "Tesla Model 3".to_string(),
        "active".to_string(),
        "Tesla".to_string(),
        "Model 3".to_string(),
        2024,
    );
    let bmw = Car::new(
        "BMW 330i".to_string(),
        "active".to_string(),
        "BMW".to_string(),
        "330i".to_string(),
        2023,
    );
    let acme_corp = Company::new(
        "ACME Corp".to_string(),
        "active".to_string(),
        "123456789".to_string(),
    );

    println!("👥 Users:");
    println!("   - Alice: {}", alice.id);
    println!("   - Bob:   {}\n", bob.id);

    println!("🚗 Cars:");
    println!("   - Tesla: {}", tesla.id);
    println!("   - BMW:   {}\n", bmw.id);

    println!("🏢 Companies:");
    println!("   - ACME Corp: {}\n", acme_corp.id);

    // Create links
    println!("🔗 Creating links...");

    // Alice owns Tesla
    let link1 = LinkEntity::new("owner", alice.id, tesla.id, None);
    link_service.create(link1).await?;
    println!("   ✓ Alice owns Tesla");

    // Alice drives Tesla
    let link2 = LinkEntity::new("driver", alice.id, tesla.id, None);
    link_service.create(link2).await?;
    println!("   ✓ Alice drives Tesla");

    // Bob drives Tesla (shared car!)
    let link3 = LinkEntity::new(
        "driver",
        bob.id,
        tesla.id,
        Some(serde_json::json!({
            "permission_level": "limited",
            "max_speed": 120
        })),
    );
    link_service.create(link3).await?;
    println!("   ✓ Bob drives Tesla (with metadata)");

    // Bob owns BMW
    let link4 = LinkEntity::new("owner", bob.id, bmw.id, None);
    link_service.create(link4).await?;
    println!("   ✓ Bob owns BMW");

    // Alice works at ACME Corp
    let link5 = LinkEntity::new(
        "worker",
        alice.id,
        acme_corp.id,
        Some(serde_json::json!({
            "role": "Senior Developer",
            "start_date": "2024-01-01"
        })),
    );
    link_service.create(link5).await?;
    println!("   ✓ Alice works at ACME Corp\n");

    // Build the router
    let app = Router::new()
        // Link routes - list (forward and reverse)
        .route("/{entity_type}/{entity_id}/{route_name}", get(list_links))
        // Link routes - create and delete (direct)
        .route(
            "/{source_type}/{source_id}/{link_type}/{target_type}/{target_id}",
            post(create_link).delete(delete_link),
        )
        // Introspection
        .route(
            "/{entity_type}/{entity_id}/links",
            get(list_available_links),
        )
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
        "   curl http://localhost:3000/users/{}/cars-owned",
        alice.id
    );
    println!();
    println!("   # List drivers of Tesla");
    println!(
        "   curl http://localhost:3000/cars/{}/users-drivers",
        tesla.id
    );
    println!();
    println!("   # Discover available routes for Alice");
    println!("   curl http://localhost:3000/users/{}/links", alice.id);
    println!();

    let listener = tokio::net::TcpListener::bind(addr).await?;
    println!("✅ Server is ready! Press Ctrl+C to stop.\n");

    axum::serve(listener, app).await?;

    Ok(())
}
