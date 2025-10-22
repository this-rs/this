//! Simple example demonstrating basic entity and link usage

use std::sync::Arc;
use this::prelude::*;

#[allow(dead_code)]
#[derive(Debug, Clone, Serialize, Deserialize)]
struct User {
    id: Uuid,
    tenant_id: Uuid,
    name: String,
    email: String,
}

#[allow(dead_code)]
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Car {
    id: Uuid,
    tenant_id: Uuid,
    brand: String,
    model: String,
    year: i32,
}

// Note: The impl_data_entity! macro would be used here, but macros
// don't work well in examples. In real code:
//
// impl_data_entity!(User, "user", ["name", "email"]);
// impl_data_entity!(Car, "car", ["brand", "model"]);

#[tokio::main]
async fn main() -> Result<()> {
    println!("🚀 This-RS Simple Example\n");

    // Load configuration
    let config = Arc::new(LinksConfig::default_config());
    let registry = LinkRouteRegistry::new(config);

    // Create link service
    let link_service = InMemoryLinkService::new();

    // Setup data
    let tenant_id = Uuid::new_v4();
    let alice_id = Uuid::new_v4();
    let bob_id = Uuid::new_v4();
    let tesla_id = Uuid::new_v4();
    let bmw_id = Uuid::new_v4();

    println!("📋 Creating links...\n");

    // Alice owns a Tesla
    let link1 = link_service
        .create(
            &tenant_id,
            "owner",
            EntityReference::new(alice_id, "user"),
            EntityReference::new(tesla_id, "car"),
            None,
        )
        .await?;
    println!("✅ Created: Alice owns Tesla (link: {})", link1.id);

    // Alice also drives the Tesla
    let link2 = link_service
        .create(
            &tenant_id,
            "driver",
            EntityReference::new(alice_id, "user"),
            EntityReference::new(tesla_id, "car"),
            None,
        )
        .await?;
    println!("✅ Created: Alice drives Tesla (link: {})", link2.id);

    // Bob drives the Tesla (shared driver)
    let link3 = link_service
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
    println!(
        "✅ Created: Bob drives Tesla with metadata (link: {})",
        link3.id
    );

    // Bob owns a BMW
    let link4 = link_service
        .create(
            &tenant_id,
            "owner",
            EntityReference::new(bob_id, "user"),
            EntityReference::new(bmw_id, "car"),
            None,
        )
        .await?;
    println!("✅ Created: Bob owns BMW (link: {})\n", link4.id);

    // Query examples
    println!("🔍 Querying links...\n");

    // Find all cars owned by Alice
    let alice_owned = link_service
        .find_by_source(&tenant_id, &alice_id, "user", Some("owner"), Some("car"))
        .await?;
    println!("🚗 Alice owns {} car(s)", alice_owned.len());

    // Find all cars driven by Alice
    let alice_driven = link_service
        .find_by_source(&tenant_id, &alice_id, "user", Some("driver"), Some("car"))
        .await?;
    println!("🚗 Alice drives {} car(s)", alice_driven.len());

    // Find all drivers of the Tesla
    let tesla_drivers = link_service
        .find_by_target(&tenant_id, &tesla_id, "car", Some("driver"), Some("user"))
        .await?;
    println!("👥 Tesla has {} driver(s)", tesla_drivers.len());

    // Show metadata example
    for link in tesla_drivers {
        if link.source.id == bob_id {
            if let Some(metadata) = link.metadata {
                println!(
                    "   Bob's permissions: {}",
                    serde_json::to_string_pretty(&metadata)?
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
