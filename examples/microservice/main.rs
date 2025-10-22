//! Billing Microservice Example
//!
//! This example demonstrates the full power of the this-rs framework:
//! - Auto-generated CRUD routes for all entities
//! - Auto-generated link routes from configuration
//! - Zero boilerplate routing code
//!
//! Simply declare a module and all routes are created automatically!

mod entities;
mod module;
mod store;

use anyhow::Result;
use entities::{Invoice, Order, Payment};
use module::BillingModule;
use std::sync::Arc;
use store::EntityStore;
use this::prelude::*;
use uuid::Uuid;

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize tracing
    tracing_subscriber::fmt::init();

    // Create entity store and populate with test data
    let entity_store = EntityStore::new();
    populate_test_data(&entity_store)?;

    // Create the billing module
    let module = BillingModule::new(entity_store);

    println!("🚀 Starting {} v{}", module.name(), module.version());
    println!("📦 Entities: {:?}", module.entity_types());

    // Build the application with auto-generated routes
    let app = ServerBuilder::new()
        .with_link_service(InMemoryLinkService::new())
        .register_module(module)?
        .build()?;

    // Start the server
    let listener = tokio::net::TcpListener::bind("127.0.0.1:3000")
        .await
        .unwrap();

    println!("\n🌐 Server running on http://127.0.0.1:3000");
    println!("\n📚 All routes auto-generated:");
    println!("  - GET    /orders, /invoices, /payments");
    println!("  - POST   /orders, /invoices, /payments");
    println!("  - GET    /orders/:id, /invoices/:id, /payments/:id");
    println!("  - GET    /:entity/:id/:link_route");
    println!("  - POST   /:entity/:id/:link_type/:target/:target_id");
    println!("  - DELETE /:entity/:id/:link_type/:target/:target_id");
    println!("  - GET    /:entity/:id/links");

    axum::serve(listener, app).await.unwrap();

    Ok(())
}

/// Populate the store with test data
fn populate_test_data(store: &EntityStore) -> Result<()> {
    let tenant_id = Uuid::new_v4();

    // Create orders
    let order1 = Order {
        id: Uuid::new_v4(),
        tenant_id,
        number: "ORD-001".to_string(),
        amount: 1500.00,
        status: "pending".to_string(),
        customer_name: Some("Alice Smith".to_string()),
        notes: Some("Rush delivery".to_string()),
    };

    let order2 = Order {
        id: Uuid::new_v4(),
        tenant_id,
        number: "ORD-002".to_string(),
        amount: 2300.00,
        status: "confirmed".to_string(),
        customer_name: Some("Bob Johnson".to_string()),
        notes: None,
    };

    store.orders.add(order1);
    store.orders.add(order2);

    // Create invoices
    let invoice1 = Invoice {
        id: Uuid::new_v4(),
        tenant_id,
        number: "INV-001".to_string(),
        amount: 1500.00,
        status: "sent".to_string(),
        due_date: Some("2025-11-15".to_string()),
        paid_at: None,
    };

    let invoice2 = Invoice {
        id: Uuid::new_v4(),
        tenant_id,
        number: "INV-002".to_string(),
        amount: 1500.00,
        status: "paid".to_string(),
        due_date: Some("2025-11-20".to_string()),
        paid_at: Some("2025-10-20".to_string()),
    };

    let invoice3 = Invoice {
        id: Uuid::new_v4(),
        tenant_id,
        number: "INV-003".to_string(),
        amount: 2300.00,
        status: "draft".to_string(),
        due_date: Some("2025-12-01".to_string()),
        paid_at: None,
    };

    store.invoices.add(invoice1);
    store.invoices.add(invoice2);
    store.invoices.add(invoice3);

    // Create payments
    let payment1 = Payment {
        id: Uuid::new_v4(),
        tenant_id,
        number: "PAY-001".to_string(),
        amount: 1500.00,
        status: "completed".to_string(),
        method: "card".to_string(),
        transaction_id: Some("txn_1234567890".to_string()),
    };

    let payment2 = Payment {
        id: Uuid::new_v4(),
        tenant_id,
        number: "PAY-002".to_string(),
        amount: 750.00,
        status: "completed".to_string(),
        method: "bank_transfer".to_string(),
        transaction_id: Some("txn_0987654321".to_string()),
    };

    store.payments.add(payment1);
    store.payments.add(payment2);

    println!("\n✅ Test data created");

    Ok(())
}
