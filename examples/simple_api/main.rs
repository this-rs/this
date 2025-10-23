//! Simple example demonstrating basic entity and link usage

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

#[tokio::main]
async fn main() -> Result<()> {
    println!("🚀 This-RS Simple Example\n");

    // Load configuration
    let config = Arc::new(LinksConfig::default_config());
    let registry = LinkRouteRegistry::new(config);

    // Create link service
    let link_service = InMemoryLinkService::new();

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

    println!("📋 Creating links...\n");

    // Alice owns a Tesla
    let link1 = LinkEntity::new("owner", alice.id, tesla.id, None);
    link_service.create(link1.clone()).await?;
    println!("✅ Created: Alice owns Tesla (link: {})", link1.id);

    // Alice also drives the Tesla
    let link2 = LinkEntity::new("driver", alice.id, tesla.id, None);
    link_service.create(link2.clone()).await?;
    println!("✅ Created: Alice drives Tesla (link: {})", link2.id);

    // Bob drives the Tesla (shared driver)
    let link3 = LinkEntity::new(
        "driver",
        bob.id,
        tesla.id,
        Some(serde_json::json!({
            "permission_level": "limited",
            "max_speed": 120
        })),
    );
    link_service.create(link3.clone()).await?;
    println!(
        "✅ Created: Bob drives Tesla with metadata (link: {})",
        link3.id
    );

    // Bob owns a BMW
    let link4 = LinkEntity::new("owner", bob.id, bmw.id, None);
    link_service.create(link4.clone()).await?;
    println!("✅ Created: Bob owns BMW (link: {})\n", link4.id);

    // Query examples
    println!("🔍 Querying links...\n");

    // Find all cars owned by Alice
    let alice_owned = link_service
        .find_by_source(&alice.id, Some("owner"), None)
        .await?;
    println!("🚗 Alice owns {} car(s)", alice_owned.len());

    // Find all cars driven by Alice
    let alice_driven = link_service
        .find_by_source(&alice.id, Some("driver"), None)
        .await?;
    println!("🚗 Alice drives {} car(s)", alice_driven.len());

    // Find all drivers of the Tesla
    let tesla_drivers = link_service
        .find_by_target(&tesla.id, Some("driver"), None)
        .await?;
    println!("👥 Tesla has {} driver(s)", tesla_drivers.len());

    // Show metadata example
    for link in tesla_drivers {
        if link.source_id == bob.id {
            if let Some(metadata) = &link.metadata {
                println!(
                    "   Bob's permissions: {}",
                    serde_json::to_string_pretty(metadata)?
                );
            }
        }
    }

    println!("\n🔍 Route Resolution Examples:\n");

    // Resolve routes using the registry
    let (def, direction) = registry.resolve_route("user", "cars-owned")?;
    println!("Route: /users/{{id}}/cars-owned");
    println!("  → Link type: {}", def.link_type);
    println!("  → Direction: {:?}", direction);
    println!("  → Connects: {} to {}\n", def.source_type, def.target_type);

    let (def, direction) = registry.resolve_route("car", "users-drivers")?;
    println!("Route: /cars/{{id}}/users-drivers");
    println!("  → Link type: {}", def.link_type);
    println!("  → Direction: {:?}", direction);
    println!("  → Connects: {} to {}", def.target_type, def.source_type);

    println!("\n✨ Example completed successfully!");

    Ok(())
}
