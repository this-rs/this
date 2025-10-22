//! Module definition for the billing microservice
//!
//! This microservice manages orders, invoices, and payments.
//! It demonstrates how to structure a real-world microservice using the this-rs framework.

use crate::entities::{
    invoice::InvoiceDescriptor, order::OrderDescriptor, payment::PaymentDescriptor,
};
use crate::store::EntityStore;
use anyhow::Result;
use this::prelude::{EntityRegistry, LinksConfig, Module};

/// Billing microservice module
///
/// Handles the complete billing workflow:
/// - Orders: Customer orders
/// - Invoices: Billing documents generated from orders
/// - Payments: Payment transactions for invoices
pub struct BillingModule {
    store: EntityStore,
}

impl BillingModule {
    pub fn new(store: EntityStore) -> Self {
        Self { store }
    }
}

impl Module for BillingModule {
    fn name(&self) -> &str {
        "billing-service"
    }

    fn version(&self) -> &str {
        "1.0.0"
    }

    fn entity_types(&self) -> Vec<&str> {
        vec!["order", "invoice", "payment"]
    }

    fn links_config(&self) -> Result<LinksConfig> {
        // Load configuration from YAML file
        let config_path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/examples/microservice/config/links.yaml"
        );
        LinksConfig::from_yaml_file(config_path)
    }

    fn register_entities(&self, registry: &mut EntityRegistry) {
        // Register Order entity
        registry.register(Box::new(OrderDescriptor::new(self.store.orders.clone())));

        // Register Invoice entity
        registry.register(Box::new(InvoiceDescriptor::new(
            self.store.invoices.clone(),
        )));

        // Register Payment entity
        registry.register(Box::new(PaymentDescriptor::new(
            self.store.payments.clone(),
        )));
    }
}
