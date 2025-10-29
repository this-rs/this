//! Billing Microservice Example with GraphQL
//!
//! This example demonstrates GraphQL exposure alongside REST:
//! - All entities are exposed via GraphQL
//! - Link queries and mutations available
//! - GraphQL playground at /graphql/playground

mod entities;
mod module;
mod store;

use anyhow::Result;
use axum::Router;
use entities::{Invoice, Order, Payment};
use module::BillingModule;
use std::sync::Arc;
use store::EntityStore;
use this::prelude::*;
use this::server::{GraphQLExposure, RestExposure};

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize tracing
    tracing_subscriber::fmt::init();

    // Create entity store and link service
    let entity_store = EntityStore::new();
    let link_service = InMemoryLinkService::new();

    // Populate with test data
    let link_service_arc = Arc::new(link_service);
    populate_test_data(&entity_store, link_service_arc.clone()).await?;

    // Create the billing module
    let module = BillingModule::new(entity_store);

    println!(
        "🚀 Starting {} v{} with GraphQL",
        module.name(),
        module.version()
    );
    println!("📦 Entities: {:?}", module.entity_types());

    // Build the server host (transport-agnostic)
    let host = Arc::new(
        ServerBuilder::new()
            .with_link_service((*link_service_arc).clone())
            .register_module(module)?
            .build_host()?,
    );

    // Build REST and GraphQL routers
    #[cfg(feature = "graphql")]
    let rest_router = RestExposure::build_router(host.clone(), vec![])?;

    #[cfg(feature = "graphql")]
    let graphql_router = GraphQLExposure::build_router(host.clone())?;

    #[cfg(not(feature = "graphql"))]
    {
        eprintln!("❌ GraphQL feature not enabled!");
        eprintln!("   Run with: cargo run --example microservice --features graphql");
        return Ok(());
    }

    #[cfg(feature = "graphql")]
    {
        // Combine routers
        let app = Router::new().merge(rest_router).merge(graphql_router);

        println!("\n🌐 Server running on http://127.0.0.1:3000");
        println!("\n📚 Available endpoints:");
        println!("\n  REST API:");
        println!("    GET    /health                          - Health check");
        println!("    GET    /orders                          - List orders");
        println!("    GET    /invoices                        - List invoices");
        println!("    GET    /payments                        - List payments");
        println!("\n  GraphQL API:");
        println!("    POST   /graphql                         - GraphQL endpoint");
        println!("    GET    /graphql/playground              - GraphQL Playground");
        println!(
            "    GET    /graphql/schema                  - GraphQL Schema (SDL, auto-generated)"
        );
        println!("\n  Example GraphQL queries:");
        println!("    # List all entity types");
        println!("    query {{ entityTypes }}");
        println!("\n    # Get an entity by ID");
        println!("    query {{");
        println!("      entity(id: \"<uuid>\", entityType: \"orders\") {{");
        println!("        id");
        println!("        type");
        println!("        name");
        println!("        data");
        println!("      }}");
        println!("    }}");
        println!("\n    # Get links for an entity");
        println!("    query {{");
        println!("      entityLinks(entityId: \"<uuid>\") {{");
        println!("        id");
        println!("        linkType");
        println!("        targetId");
        println!("      }}");
        println!("    }}");

        // Start server
        let listener = tokio::net::TcpListener::bind("127.0.0.1:3000").await?;
        axum::serve(listener, app).await?;
    }

    Ok(())
}

/// Populate the store with test data
async fn populate_test_data(
    entity_store: &EntityStore,
    link_service: Arc<InMemoryLinkService>,
) -> Result<()> {
    // Create Orders
    let order1 = Order::new(
        "ORD-001".to_string(),                         // name
        "pending".to_string(),                         // status
        "ORD-001".to_string(),                         // number
        999.99,                                        // amount
        Some("Alice Smith".to_string()),               // customer_name
        Some("Premium SaaS Subscription".to_string()), // notes
    );
    let order2 = Order::new(
        "ORD-002".to_string(),                          // name
        "confirmed".to_string(),                        // status
        "ORD-002".to_string(),                          // number
        4999.99,                                        // amount
        Some("Bob Johnson".to_string()),                // customer_name
        Some("Enterprise Support Package".to_string()), // notes
    );

    entity_store.orders.add(order1.clone());
    entity_store.orders.add(order2.clone());

    // Create Invoices
    let invoice1 = Invoice::new(
        "INV-001".to_string(),          // name
        "sent".to_string(),             // status
        "INV-001".to_string(),          // number
        999.99,                         // amount
        Some("2024-01-15".to_string()), // due_date
        None,                           // paid_at
    );
    let invoice2 = Invoice::new(
        "INV-002".to_string(),          // name
        "paid".to_string(),             // status
        "INV-002".to_string(),          // number
        999.99,                         // amount
        Some("2024-02-15".to_string()), // due_date
        Some("2024-02-10".to_string()), // paid_at
    );
    let invoice3 = Invoice::new(
        "INV-003".to_string(),          // name
        "sent".to_string(),             // status
        "INV-003".to_string(),          // number
        4999.99,                        // amount
        Some("2024-01-20".to_string()), // due_date
        None,                           // paid_at
    );

    entity_store.invoices.add(invoice1.clone());
    entity_store.invoices.add(invoice2.clone());
    entity_store.invoices.add(invoice3.clone());

    // Create Payments
    let payment1 = Payment::new(
        "PAY-001".to_string(),              // name
        "completed".to_string(),            // status
        "PAY-001".to_string(),              // number
        999.99,                             // amount
        "credit_card".to_string(),          // method
        Some("txn_1234567890".to_string()), // transaction_id
    );
    let payment2 = Payment::new(
        "PAY-002".to_string(),              // name
        "completed".to_string(),            // status
        "PAY-002".to_string(),              // number
        999.99,                             // amount
        "bank_transfer".to_string(),        // method
        Some("txn_2345678901".to_string()), // transaction_id
    );
    let payment3 = Payment::new(
        "PAY-003".to_string(),              // name
        "completed".to_string(),            // status
        "PAY-003".to_string(),              // number
        4999.99,                            // amount
        "credit_card".to_string(),          // method
        Some("txn_3456789012".to_string()), // transaction_id
    );

    entity_store.payments.add(payment1.clone());
    entity_store.payments.add(payment2.clone());
    entity_store.payments.add(payment3.clone());

    // Create links between orders and invoices
    use this::core::link::LinkEntity;

    let link1 = LinkEntity::new(
        "has_invoice".to_string(),
        order1.id,
        invoice1.id,
        Some(serde_json::json!({"note": "Monthly subscription"})),
    );
    let link2 = LinkEntity::new(
        "has_invoice".to_string(),
        order1.id,
        invoice2.id,
        Some(serde_json::json!({"note": "Monthly subscription"})),
    );
    let link3 = LinkEntity::new(
        "has_invoice".to_string(),
        order2.id,
        invoice3.id,
        Some(serde_json::json!({"note": "Enterprise support"})),
    );

    link_service.create(link1).await?;
    link_service.create(link2).await?;
    link_service.create(link3).await?;

    // Create links between invoices and payments
    let link4 = LinkEntity::new(
        "payment".to_string(),
        invoice1.id,
        payment1.id,
        Some(serde_json::json!({"processed": true})),
    );
    let link5 = LinkEntity::new(
        "payment".to_string(),
        invoice2.id,
        payment2.id,
        Some(serde_json::json!({"processed": true})),
    );
    let link6 = LinkEntity::new(
        "payment".to_string(),
        invoice3.id,
        payment3.id,
        Some(serde_json::json!({"processed": true})),
    );

    link_service.create(link4).await?;
    link_service.create(link5).await?;
    link_service.create(link6).await?;

    Ok(())
}
