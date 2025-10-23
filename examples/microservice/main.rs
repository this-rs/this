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
        .with_link_service((*link_service_arc).clone())
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
    println!("    PUT    /orders/{{id}}                      - Update an order");
    println!("    DELETE /orders/{{id}}                      - Delete an order");
    println!("    GET    /invoices                        - List all invoices");
    println!("    POST   /invoices                        - Create a new invoice");
    println!("    GET    /invoices/{{id}}                    - Get a specific invoice");
    println!("    PUT    /invoices/{{id}}                    - Update an invoice");
    println!("    DELETE /invoices/{{id}}                    - Delete an invoice");
    println!("    GET    /payments                        - List all payments");
    println!("    POST   /payments                        - Create a new payment");
    println!("    GET    /payments/{{id}}                    - Get a specific payment");
    println!("    PUT    /payments/{{id}}                    - Update a payment");
    println!("    DELETE /payments/{{id}}                    - Delete a payment");
    println!("\n  🔗 Link Routes (Generic for all entities):");
    println!("    GET    /links/{{link_id}}                  - Get a specific link by ID");
    println!(
        "    GET    /{{entity}}/{{id}}/{{route_name}}        - List links (e.g. /orders/123/invoices)"
    );
    println!(
        "    POST   /{{entity}}/{{id}}/{{route_name}}        - Create new entity + link automatically ✅"
    );
    println!("    GET    /{{source}}/{{id}}/{{route_name}}/{{target_id}}  - Get a specific link");
    println!("    POST   /{{source}}/{{id}}/{{route_name}}/{{target_id}}  - Create link between existing entities");
    println!("    PUT    /{{source}}/{{id}}/{{route_name}}/{{target_id}}  - Update link metadata");
    println!("    DELETE /{{source}}/{{id}}/{{route_name}}/{{target_id}}  - Delete a link");
    println!(
        "    GET    /{{entity}}/{{id}}/links               - Introspection (list available link types)"
    );
    println!("\n  📋 Specific Link Routes (from config):");
    println!("    GET    /orders/{{id}}/invoices             - List invoices for an order");
    println!("    POST   /orders/{{id}}/invoices             - Create new invoice + link ✅");
    println!("    GET    /orders/{{id}}/invoices/{{invoice_id}} - Get specific order→invoice link");
    println!("    POST   /orders/{{id}}/invoices/{{invoice_id}} - Link existing order & invoice");
    println!("    PUT    /orders/{{id}}/invoices/{{invoice_id}} - Update order→invoice link");
    println!("    DELETE /orders/{{id}}/invoices/{{invoice_id}} - Delete order→invoice link");
    println!("    GET    /invoices/{{id}}/order              - Get order for an invoice");
    println!("    GET    /invoices/{{id}}/payments           - List payments for an invoice");
    println!("    POST   /invoices/{{id}}/payments           - Create new payment + link ✅");
    println!(
        "    GET    /invoices/{{id}}/payments/{{payment_id}} - Get specific invoice→payment link"
    );
    println!("    POST   /invoices/{{id}}/payments/{{payment_id}} - Link existing invoice & payment");
    println!("    GET    /payments/{{id}}/invoice            - Get invoice for a payment");

    axum::serve(listener, app).await.unwrap();

    Ok(())
}

/// Populate the store with test data
async fn populate_test_data(
    store: &EntityStore,
    link_service: Arc<InMemoryLinkService>,
) -> Result<()> {
    // Create orders using the generated new() method
    let order1 = Order::new(
        "ORD-001".to_string(),                           // name
        "pending".to_string(),                            // status
        "ORD-001".to_string(),                           // number
        1500.00,                                          // amount
        Some("Alice Smith".to_string()),                  // customer_name
        Some("Rush delivery".to_string()),                // notes
    );

    let order2 = Order::new(
        "ORD-002".to_string(),                           // name
        "confirmed".to_string(),                          // status
        "ORD-002".to_string(),                           // number
        2300.00,                                          // amount
        Some("Bob Johnson".to_string()),                  // customer_name
        None,                                             // notes
    );

    store.orders.add(order1.clone());
    store.orders.add(order2.clone());

    // Create invoices using the generated new() method
    let invoice1 = Invoice::new(
        "INV-001".to_string(),                            // name
        "sent".to_string(),                               // status
        "INV-001".to_string(),                            // number
        1500.00,                                          // amount
        Some("2025-11-15".to_string()),                   // due_date
        None,                                             // paid_at
    );

    let invoice2 = Invoice::new(
        "INV-002".to_string(),                            // name
        "paid".to_string(),                               // status
        "INV-002".to_string(),                            // number
        1500.00,                                          // amount
        Some("2025-11-20".to_string()),                   // due_date
        Some("2025-10-20".to_string()),                   // paid_at
    );

    let invoice3 = Invoice::new(
        "INV-003".to_string(),                            // name
        "draft".to_string(),                              // status
        "INV-003".to_string(),                            // number
        2300.00,                                          // amount
        Some("2025-12-01".to_string()),                   // due_date
        None,                                             // paid_at
    );

    store.invoices.add(invoice1.clone());
    store.invoices.add(invoice2.clone());
    store.invoices.add(invoice3.clone());

    // Create payments using the generated new() method
    let payment1 = Payment::new(
        "PAY-001".to_string(),                            // name
        "completed".to_string(),                          // status
        "PAY-001".to_string(),                            // number
        1500.00,                                          // amount
        "card".to_string(),                               // method
        Some("txn_1234567890".to_string()),               // transaction_id
    );

    let payment2 = Payment::new(
        "PAY-002".to_string(),                            // name
        "completed".to_string(),                          // status
        "PAY-002".to_string(),                            // number
        750.00,                                           // amount
        "bank_transfer".to_string(),                      // method
        Some("txn_0987654321".to_string()),               // transaction_id
    );

    store.payments.add(payment1.clone());
    store.payments.add(payment2.clone());

    println!("\n✅ Test data created:");
    println!("   📦 2 orders, 3 invoices, 2 payments");

    // Create links between entities using LinkEntity
    println!("\n🔗 Creating links between entities...");

    // Link order1 -> invoice1 (ORD-001 has invoice INV-001)
    let link1 = LinkEntity::new(
        "has_invoice",
        order1.id,
        invoice1.id,
        Some(serde_json::json!({
            "created_at": "2025-10-20T10:00:00Z",
            "created_by": "system",
            "invoice_type": "standard"
        })),
    );
    link_service.create(link1).await?;
    println!("   ✅ Order ORD-001 → Invoice INV-001");

    // Link order1 -> invoice2 (ORD-001 has another invoice INV-002)
    let link2 = LinkEntity::new(
        "has_invoice",
        order1.id,
        invoice2.id,
        Some(serde_json::json!({
            "created_at": "2025-10-21T14:30:00Z",
            "created_by": "system",
            "invoice_type": "partial"
        })),
    );
    link_service.create(link2).await?;
    println!("   ✅ Order ORD-001 → Invoice INV-002");

    // Link order2 -> invoice3 (ORD-002 has invoice INV-003)
    let link3 = LinkEntity::new(
        "has_invoice",
        order2.id,
        invoice3.id,
        Some(serde_json::json!({
            "created_at": "2025-10-22T09:15:00Z",
            "created_by": "system",
            "invoice_type": "standard"
        })),
    );
    link_service.create(link3).await?;
    println!("   ✅ Order ORD-002 → Invoice INV-003");

    // Link invoice2 -> payment1 (INV-002 is paid by PAY-001)
    let link4 = LinkEntity::new(
        "payment",
        invoice2.id,
        payment1.id,
        Some(serde_json::json!({
            "payment_date": "2025-10-20T15:45:00Z",
            "payment_status": "completed",
            "payment_method": "card",
            "transaction_id": "txn_1234567890"
        })),
    );
    link_service.create(link4).await?;
    println!("   ✅ Invoice INV-002 → Payment PAY-001");

    // Link invoice2 -> payment2 (INV-002 has partial payment PAY-002)
    let link5 = LinkEntity::new(
        "payment",
        invoice2.id,
        payment2.id,
        Some(serde_json::json!({
            "payment_date": "2025-10-21T11:20:00Z",
            "payment_status": "completed",
            "payment_method": "bank_transfer",
            "transaction_id": "txn_0987654321",
            "note": "Partial payment"
        })),
    );
    link_service.create(link5).await?;
    println!("   ✅ Invoice INV-002 → Payment PAY-002 (partial)");

    println!("\n🎉 Test data ready! You can now test the API:");
    println!("\n   📋 List Links:");
    println!("   • GET /orders/{}/invoices", order1.id);
    println!("   • GET /invoices/{}/order", invoice1.id);
    println!("   • GET /invoices/{}/payments", invoice2.id);
    println!("   • GET /payments/{}/invoice", payment1.id);
    println!("\n   🔗 Manipulate Links (NEW semantic URLs):");
    println!("   • POST   /orders/{}/invoices/{{invoice_id}}  - Link existing entities", order1.id);
    println!("   • POST   /orders/{}/invoices                 - Create new invoice + link ✅", order1.id);
    println!("   • PUT    /orders/{}/invoices/{}              - Update link metadata", order1.id, invoice1.id);
    println!("   • DELETE /orders/{}/invoices/{}              - Delete link", order1.id, invoice1.id);
    println!("\n   📝 Example curl commands:");
    println!("\n   # List invoices for an order (with enriched entities)");
    println!(
        "   curl http://127.0.0.1:3000/orders/{}/invoices | jq .",
        order1.id
    );
    println!("\n   # Get a specific link (order → invoice)");
    println!(
        "   curl http://127.0.0.1:3000/orders/{}/invoices/{} | jq .",
        order1.id, invoice1.id
    );
    println!("\n   # Create a NEW invoice and link it to an order automatically");
    println!("   curl -X POST -H 'Content-Type: application/json' \\");
    println!("     -d '{{");
    println!("       \"entity\": {{");
    println!("         \"number\": \"INV-999\",");
    println!("         \"amount\": 999.99,");
    println!("         \"status\": \"active\"");
    println!("       }},");
    println!("       \"metadata\": {{\"note\": \"Auto-created invoice\", \"priority\": \"high\"}}");
    println!("     }}' \\");
    println!(
        "     http://127.0.0.1:3000/orders/{}/invoices",
        order1.id
    );
    println!("\n   # Create a link between existing order and invoice");
    println!("   curl -X POST -H 'Content-Type: application/json' \\");
    println!("     -d '{{\"metadata\": {{\"note\": \"Test link\", \"priority\": \"high\"}}}}' \\");
    println!(
        "     http://127.0.0.1:3000/orders/{}/invoices/{}",
        order1.id, invoice3.id
    );
    println!("\n   # Update link metadata");
    println!("   curl -X PUT -H 'Content-Type: application/json' \\");
    println!("     -d '{{\"metadata\": {{\"status\": \"verified\"}}}}' \\");
    println!(
        "     http://127.0.0.1:3000/orders/{}/invoices/{}",
        order1.id, invoice1.id
    );
    println!("\n   # Delete a link");
    println!("   curl -X DELETE \\");
    println!(
        "     http://127.0.0.1:3000/orders/{}/invoices/{}",
        order1.id, invoice1.id
    );

    Ok(())
}
