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

    // Create entity store and link service
    let entity_store = EntityStore::new();
    let link_service = InMemoryLinkService::new();

    // Populate with test data (including links between entities)
    // We need to share the same link service instance
    let link_service_arc = Arc::new(link_service);
    populate_test_data(&entity_store, link_service_arc.clone()).await?;

    // Create the billing module
    let module = BillingModule::new(entity_store);

    println!("🚀 Starting {} v{}", module.name(), module.version());
    println!("📦 Entities: {:?}", module.entity_types());

    // Build the application with auto-generated routes
    // Important: Use the same link service instance with the test data
    let app = ServerBuilder::new()
        .with_link_service(link_service_arc.as_ref().clone())
        .register_module(module)?
        .build()?;

    // Start the server
    let listener = tokio::net::TcpListener::bind("127.0.0.1:3000")
        .await
        .unwrap();

    println!("\n🌐 Server running on http://127.0.0.1:3000");
    println!("\n📚 All routes auto-generated:");
    println!("\n  🔷 Entity CRUD Routes:");
    println!("    GET    /orders                          - List all orders");
    println!("    POST   /orders                          - Create a new order");
    println!("    GET    /orders/{{id}}                      - Get a specific order");
    println!("    GET    /invoices                        - List all invoices");
    println!("    POST   /invoices                        - Create a new invoice");
    println!("    GET    /invoices/{{id}}                    - Get a specific invoice");
    println!("    GET    /payments                        - List all payments");
    println!("    POST   /payments                        - Create a new payment");
    println!("    GET    /payments/{{id}}                    - Get a specific payment");
    println!("\n  🔗 Link Routes (Generic for all entities):");
    println!("    GET    /links/{{link_id}}                  - Get a specific link by ID");
    println!(
        "    GET    /{{entity}}/{{id}}/{{route_name}}        - List links (e.g. /orders/123/invoices)"
    );
    println!("    GET    /{{source}}/{{id}}/{{route_name}}/{{target_id}}  - Get a specific link");
    println!("    POST   /{{source}}/{{id}}/{{route_name}}/{{target_id}}  - Create a link");
    println!("    PUT    /{{source}}/{{id}}/{{route_name}}/{{target_id}}  - Update link metadata");
    println!("    DELETE /{{source}}/{{id}}/{{route_name}}/{{target_id}}  - Delete a link");
    println!(
        "    GET    /{{entity}}/{{id}}/links               - Introspection (list available link types)"
    );
    println!("\n  📋 Specific Link Routes (from config):");
    println!("    GET    /orders/{{id}}/invoices             - List invoices for an order");
    println!("    GET    /orders/{{id}}/invoices/{{invoice_id}} - Get specific order→invoice link");
    println!("    POST   /orders/{{id}}/invoices/{{invoice_id}} - Create order→invoice link");
    println!("    PUT    /orders/{{id}}/invoices/{{invoice_id}} - Update order→invoice link");
    println!("    DELETE /orders/{{id}}/invoices/{{invoice_id}} - Delete order→invoice link");
    println!("    GET    /invoices/{{id}}/order              - Get order for an invoice");
    println!("    GET    /invoices/{{id}}/payments           - List payments for an invoice");
    println!(
        "    GET    /invoices/{{id}}/payments/{{payment_id}} - Get specific invoice→payment link"
    );
    println!("    POST   /invoices/{{id}}/payments/{{payment_id}} - Create invoice→payment link");
    println!("    GET    /payments/{{id}}/invoice            - Get invoice for a payment");

    axum::serve(listener, app).await.unwrap();

    Ok(())
}

/// Populate the store with test data
async fn populate_test_data(
    store: &EntityStore,
    link_service: Arc<InMemoryLinkService>,
) -> Result<()> {
    // Use a fixed tenant ID for easier testing
    let tenant_id = Uuid::parse_str("e2e92411-5568-4436-a388-464c649a5a97").expect("Invalid UUID");

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

    store.orders.add(order1.clone());
    store.orders.add(order2.clone());

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

    store.invoices.add(invoice1.clone());
    store.invoices.add(invoice2.clone());
    store.invoices.add(invoice3.clone());

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

    store.payments.add(payment1.clone());
    store.payments.add(payment2.clone());

    println!("\n✅ Test data created:");
    println!("   📦 2 orders, 3 invoices, 2 payments");
    println!("   🔑 Tenant ID: {}", tenant_id);

    // Create links between entities
    println!("\n🔗 Creating links between entities...");

    // Link order1 -> invoice1 (ORD-001 has invoice INV-001)
    link_service
        .create(
            &tenant_id,
            "has_invoice",
            EntityReference::new(order1.id, "order"),
            EntityReference::new(invoice1.id, "invoice"),
            Some(serde_json::json!({
                "created_at": "2025-10-20T10:00:00Z",
                "created_by": "system",
                "invoice_type": "standard"
            })),
        )
        .await?;
    println!("   ✅ Order ORD-001 → Invoice INV-001");

    // Link order1 -> invoice2 (ORD-001 has another invoice INV-002)
    link_service
        .create(
            &tenant_id,
            "has_invoice",
            EntityReference::new(order1.id, "order"),
            EntityReference::new(invoice2.id, "invoice"),
            Some(serde_json::json!({
                "created_at": "2025-10-21T14:30:00Z",
                "created_by": "system",
                "invoice_type": "partial"
            })),
        )
        .await?;
    println!("   ✅ Order ORD-001 → Invoice INV-002");

    // Link order2 -> invoice3 (ORD-002 has invoice INV-003)
    link_service
        .create(
            &tenant_id,
            "has_invoice",
            EntityReference::new(order2.id, "order"),
            EntityReference::new(invoice3.id, "invoice"),
            Some(serde_json::json!({
                "created_at": "2025-10-22T09:15:00Z",
                "created_by": "system",
                "invoice_type": "standard"
            })),
        )
        .await?;
    println!("   ✅ Order ORD-002 → Invoice INV-003");

    // Link invoice2 -> payment1 (INV-002 is paid by PAY-001)
    link_service
        .create(
            &tenant_id,
            "payment",
            EntityReference::new(invoice2.id, "invoice"),
            EntityReference::new(payment1.id, "payment"),
            Some(serde_json::json!({
                "payment_date": "2025-10-20T15:45:00Z",
                "payment_status": "completed",
                "payment_method": "card",
                "transaction_id": "txn_1234567890"
            })),
        )
        .await?;
    println!("   ✅ Invoice INV-002 → Payment PAY-001");

    // Link invoice2 -> payment2 (INV-002 has partial payment PAY-002)
    link_service
        .create(
            &tenant_id,
            "payment",
            EntityReference::new(invoice2.id, "invoice"),
            EntityReference::new(payment2.id, "payment"),
            Some(serde_json::json!({
                "payment_date": "2025-10-21T11:20:00Z",
                "payment_status": "completed",
                "payment_method": "bank_transfer",
                "transaction_id": "txn_0987654321",
                "note": "Partial payment"
            })),
        )
        .await?;
    println!("   ✅ Invoice INV-002 → Payment PAY-002 (partial)");

    println!("\n🎉 Test data ready! You can now test the API:");
    println!("\n   💡 Tenant ID: {}", tenant_id);
    println!("\n   📋 List Links:");
    println!("   • GET /orders/{}/invoices", order1.id);
    println!("   • GET /invoices/{}/order", invoice1.id);
    println!("   • GET /invoices/{}/payments", invoice2.id);
    println!("   • GET /payments/{}/invoice", payment1.id);
    println!("\n   🔗 Manipulate Links (NEW semantic URLs):");
    println!("   • POST   /orders/{}/invoices/{{invoice_id}}", order1.id);
    println!("   • PUT    /orders/{}/invoices/{}", order1.id, invoice1.id);
    println!("   • DELETE /orders/{}/invoices/{}", order1.id, invoice1.id);
    println!("\n   📝 Example curl commands:");
    println!("\n   # List invoices for an order");
    println!(
        "   curl -H 'X-Tenant-ID: {}' http://127.0.0.1:3000/orders/{}/invoices | jq .",
        tenant_id, order1.id
    );
    println!("\n   # Get a specific link (order → invoice)");
    println!(
        "   curl -H 'X-Tenant-ID: {}' http://127.0.0.1:3000/orders/{}/invoices/{} | jq .",
        tenant_id, order1.id, invoice1.id
    );
    println!("\n   # Create a new link (order → invoice)");
    println!(
        "   curl -X POST -H 'X-Tenant-ID: {}' -H 'Content-Type: application/json' \\",
        tenant_id
    );
    println!("     -d '{{\"metadata\": {{\"note\": \"Test link\"}}}}' \\",);
    println!(
        "     http://127.0.0.1:3000/orders/{}/invoices/{{new_invoice_id}}",
        order1.id
    );
    println!("\n   # Update link metadata");
    println!(
        "   curl -X PUT -H 'X-Tenant-ID: {}' -H 'Content-Type: application/json' \\",
        tenant_id
    );
    println!("     -d '{{\"metadata\": {{\"status\": \"verified\"}}}}' \\",);
    println!(
        "     http://127.0.0.1:3000/orders/{}/invoices/{}",
        order1.id, invoice1.id
    );
    println!("\n   # Delete a link");
    println!("   curl -X DELETE -H 'X-Tenant-ID: {}' \\", tenant_id);
    println!(
        "     http://127.0.0.1:3000/orders/{}/invoices/{}",
        order1.id, invoice1.id
    );

    Ok(())
}
