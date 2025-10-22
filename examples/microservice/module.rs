//! Module definition for the billing microservice
//!
//! This microservice manages orders, invoices, and payments.
//! It demonstrates how to structure a real-world microservice using the this-rs framework.

use anyhow::Result;
use this::prelude::{LinksConfig, Module};

/// Billing microservice module
///
/// Handles the complete billing workflow:
/// - Orders: Customer orders
/// - Invoices: Billing documents generated from orders
/// - Payments: Payment transactions for invoices
pub struct BillingModule;

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
}
