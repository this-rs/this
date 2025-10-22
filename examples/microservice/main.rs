//! Complete microservice example using This-RS framework
//!
//! This example demonstrates:
//! - Loading configuration from YAML
//! - Setting up entities with authorization
//! - Auto-registering CRUD routes
//! - Using the link system
//! - Module-based architecture
//! - Best practice: One folder per entity

mod entities;
mod module;
mod store;

use anyhow::Result;
use axum::{routing::get, Router};
use entities::{
    invoice::{create_invoice, get_invoice, list_invoices, InvoiceAppState},
    order::{create_order, get_order, list_orders, OrderAppState},
    payment::{create_payment, get_payment, list_payments, PaymentAppState},
    Invoice, Order, Payment,
};
use module::BillingModule;
use std::sync::Arc;
use store::EntityStore;
use this::prelude::*;
use uuid::Uuid;

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize tracing
    tracing_subscriber::fmt::init();

    // Load module configuration
    let module = BillingModule;
    let config = Arc::new(module.links_config()?);

    println!("🚀 Starting {} v{}", module.name(), module.version());
    println!("📦 Entities: {:?}", module.entity_types());

    // Create link service and registry
    let link_service = Arc::new(InMemoryLinkService::new());
    let registry = Arc::new(LinkRouteRegistry::new(config.clone()));

    // Create aggregated store for all entities
    let entity_store = EntityStore::new();

    // Populate test data
    let tenant_id = Uuid::new_v4();

    // Create test orders
    let order1_id = Uuid::new_v4();
    let order2_id = Uuid::new_v4();

    println!("\n📝 Creating test data:");
    println!("  Order 1: {}", order1_id);
    println!("  Order 2: {}", order2_id);

    let order1 = Order {
        id: order1_id,
        tenant_id,
        number: "ORD-001".to_string(),
        amount: 1500.00,
        status: "pending".to_string(),
        customer_name: Some("Alice Smith".to_string()),
        notes: Some("Rush delivery".to_string()),
    };
    let order2 = Order {
        id: order2_id,
        tenant_id,
        number: "ORD-002".to_string(),
        amount: 2300.00,
        status: "confirmed".to_string(),
        customer_name: Some("Bob Johnson".to_string()),
        notes: None,
    };
    entity_store.orders.add(order1);
    entity_store.orders.add(order2);

    // Create invoices
    let invoice1_id = Uuid::new_v4();
    let invoice2_id = Uuid::new_v4();
    let invoice3_id = Uuid::new_v4();

    println!("  Invoice 1: {}", invoice1_id);
    println!("  Invoice 2: {}", invoice2_id);
    println!("  Invoice 3: {}", invoice3_id);

    let invoice1 = Invoice {
        id: invoice1_id,
        tenant_id,
        number: "INV-001".to_string(),
        amount: 750.00,
        status: "sent".to_string(),
        due_date: Some("2025-11-15".to_string()),
        paid_at: None,
    };
    let invoice2 = Invoice {
        id: invoice2_id,
        tenant_id,
        number: "INV-002".to_string(),
        amount: 750.00,
        status: "paid".to_string(),
        due_date: Some("2025-11-10".to_string()),
        paid_at: Some("2025-10-20".to_string()),
    };
    let invoice3 = Invoice {
        id: invoice3_id,
        tenant_id,
        number: "INV-003".to_string(),
        amount: 2300.00,
        status: "draft".to_string(),
        due_date: Some("2025-12-01".to_string()),
        paid_at: None,
    };
    entity_store.invoices.add(invoice1);
    entity_store.invoices.add(invoice2);
    entity_store.invoices.add(invoice3);

    // Create payments
    let payment1_id = Uuid::new_v4();
    let payment2_id = Uuid::new_v4();

    println!("  Payment 1: {}", payment1_id);
    println!("  Payment 2: {}", payment2_id);

    let payment1 = Payment {
        id: payment1_id,
        tenant_id,
        number: "PAY-001".to_string(),
        amount: 750.00,
        status: "completed".to_string(),
        method: "card".to_string(),
        transaction_id: Some("txn_1234567890".to_string()),
    };
    let payment2 = Payment {
        id: payment2_id,
        tenant_id,
        number: "PAY-002".to_string(),
        amount: 750.00,
        status: "completed".to_string(),
        method: "bank_transfer".to_string(),
        transaction_id: Some("txn_0987654321".to_string()),
    };
    entity_store.payments.add(payment1);
    entity_store.payments.add(payment2);

    // Create links: Order 1 -> Invoice 1, Invoice 2
    link_service
        .create(
            &tenant_id,
            "has_invoice",
            EntityReference::new(order1_id, "order"),
            EntityReference::new(invoice1_id, "invoice"),
            None,
        )
        .await?;

    link_service
        .create(
            &tenant_id,
            "has_invoice",
            EntityReference::new(order1_id, "order"),
            EntityReference::new(invoice2_id, "invoice"),
            None,
        )
        .await?;

    // Create links: Order 2 -> Invoice 3
    link_service
        .create(
            &tenant_id,
            "has_invoice",
            EntityReference::new(order2_id, "order"),
            EntityReference::new(invoice3_id, "invoice"),
            None,
        )
        .await?;

    // Create links: Invoice 1 -> Payment 1, Invoice 2 -> Payment 2
    link_service
        .create(
            &tenant_id,
            "payment",
            EntityReference::new(invoice1_id, "invoice"),
            EntityReference::new(payment1_id, "payment"),
            None,
        )
        .await?;

    link_service
        .create(
            &tenant_id,
            "payment",
            EntityReference::new(invoice2_id, "invoice"),
            EntityReference::new(payment2_id, "payment"),
            None,
        )
        .await?;

    println!("\n✅ Test data created with links");

    // Create application state
    let link_app_state = AppState {
        link_service: link_service.clone(),
        registry: registry.clone(),
        config: config.clone(),
    };

    // Create entity-specific states
    let order_state = OrderAppState {
        store: entity_store.orders.clone(),
    };
    let invoice_state = InvoiceAppState {
        store: entity_store.invoices.clone(),
    };
    let payment_state = PaymentAppState {
        store: entity_store.payments.clone(),
    };

    // Build the router
    let app = Router::new()
        // === CRUD Routes for Entities ===
        .route("/orders", get(list_orders).post(create_order))
        .route("/orders/:id", get(get_order))
        .with_state(order_state)
        .route("/invoices", get(list_invoices).post(create_invoice))
        .route("/invoices/:id", get(get_invoice))
        .with_state(invoice_state)
        .route("/payments", get(list_payments).post(create_payment))
        .route("/payments/:id", get(get_payment))
        .with_state(payment_state)
        // === Link Routes (auto-generated from config) ===
        // These need to be after entity routes to avoid conflicts
        .route(
            "/:entity_type/:entity_id/:route_name",
            get({
                let state = link_app_state.clone();
                move |path, headers| list_links(axum::extract::State(state.clone()), path, headers)
            }),
        )
        .route(
            "/:source_type/:source_id/:link_type/:target_type/:target_id",
            axum::routing::post({
                let state = link_app_state.clone();
                move |path, headers, body| {
                    create_link(axum::extract::State(state.clone()), path, headers, body)
                }
            }),
        )
        .route(
            "/:source_type/:source_id/:link_type/:target_type/:target_id",
            axum::routing::delete({
                let state = link_app_state.clone();
                move |path, headers| delete_link(axum::extract::State(state.clone()), path, headers)
            }),
        )
        .route(
            "/:entity_type/:entity_id/links",
            get({
                let state = link_app_state.clone();
                move |path, headers| {
                    list_available_links(axum::extract::State(state.clone()), path, headers)
                }
            }),
        );

    // Start the server
    let listener = tokio::net::TcpListener::bind("127.0.0.1:3000")
        .await
        .unwrap();

    println!("\n🌐 Server running on http://127.0.0.1:3000");
    println!("\n📚 Available routes:");
    println!("\n  === Entity CRUD Routes ===");
    println!("  GET    /orders                         - List all orders");
    println!("  POST   /orders                         - Create a new order");
    println!("  GET    /orders/{{id}}                    - Get a specific order");
    println!("  GET    /invoices                       - List all invoices");
    println!("  POST   /invoices                       - Create a new invoice");
    println!("  GET    /invoices/{{id}}                  - Get a specific invoice");
    println!("  GET    /payments                       - List all payments");
    println!("  POST   /payments                       - Create a new payment");
    println!("  GET    /payments/{{id}}                  - Get a specific payment");

    println!("\n  === Link Routes (Bidirectional Navigation) ===");
    println!("  GET    /orders/{{id}}/invoices           - List invoices for an order");
    println!("  GET    /invoices/{{id}}/order            - Get order for an invoice");
    println!("  GET    /invoices/{{id}}/payments         - List payments for an invoice");
    println!("  GET    /payments/{{id}}/invoice          - Get invoice for a payment");
    println!("  POST   /orders/{{id}}/has_invoice/invoices/{{inv_id}} - Create link");
    println!("  DELETE /orders/{{id}}/has_invoice/invoices/{{inv_id}} - Delete link");
    println!("  GET    /orders/{{id}}/links              - Introspection (discover all links)");

    println!("\n🧪 Test commands:");

    println!("\n  === CRUD Operations ===");
    println!("  # List all orders");
    println!("  curl http://127.0.0.1:3000/orders");

    println!("\n  # Get specific order");
    println!("  curl http://127.0.0.1:3000/orders/{}", order1_id);

    println!("\n  # Create new order");
    println!(
        r#"  curl -X POST http://127.0.0.1:3000/orders -H "Content-Type: application/json" -d '{{"number":"ORD-003","amount":500.0,"status":"pending","customer_name":"Charlie Brown"}}'"#
    );

    println!("\n  # List all invoices");
    println!("  curl http://127.0.0.1:3000/invoices");

    println!("\n  # List all payments");
    println!("  curl http://127.0.0.1:3000/payments");

    println!("\n  === Link Navigation ===");
    println!("  # Get invoices for order 1");
    println!(
        "  curl -H 'X-Tenant-ID: {}' http://127.0.0.1:3000/orders/{}/invoices",
        tenant_id, order1_id
    );
    println!("\n  # Get order for invoice 1");
    println!(
        "  curl -H 'X-Tenant-ID: {}' http://127.0.0.1:3000/invoices/{}/order",
        tenant_id, invoice1_id
    );
    println!("\n  # Get payments for invoice 1");
    println!(
        "  curl -H 'X-Tenant-ID: {}' http://127.0.0.1:3000/invoices/{}/payments",
        tenant_id, invoice1_id
    );
    println!("\n  # Introspection - discover all links for order 1");
    println!(
        "  curl -H 'X-Tenant-ID: {}' http://127.0.0.1:3000/orders/{}/links",
        tenant_id, order1_id
    );

    axum::serve(listener, app).await.unwrap();

    Ok(())
}
