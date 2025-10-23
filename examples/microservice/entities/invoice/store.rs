//! Invoice store implementation

use super::model::Invoice;
use anyhow::Result;
use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use this::prelude::*;
use serde_json;
use uuid::Uuid;

/// In-memory store for Invoice entities
#[derive(Clone)]
pub struct InvoiceStore {
    data: Arc<RwLock<HashMap<Uuid, Invoice>>>,
}

impl InvoiceStore {
    pub fn new() -> Self {
        Self {
            data: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub fn add(&self, invoice: Invoice) {
        self.data.write().unwrap().insert(invoice.id, invoice);
    }

    pub fn get(&self, id: &Uuid) -> Option<Invoice> {
        self.data.read().unwrap().get(id).cloned()
    }

    pub fn list(&self) -> Vec<Invoice> {
        self.data.read().unwrap().values().cloned().collect()
    }

    pub fn update(&self, invoice: Invoice) {
        self.data.write().unwrap().insert(invoice.id, invoice);
    }

    pub fn delete(&self, id: &Uuid) -> Option<Invoice> {
        self.data.write().unwrap().remove(id)
    }
}

impl Default for InvoiceStore {
    fn default() -> Self {
        Self::new()
    }
}

/// Implement EntityFetcher for InvoiceStore
#[async_trait::async_trait]
impl EntityFetcher for InvoiceStore {
    async fn fetch_as_json(&self, entity_id: &Uuid) -> Result<serde_json::Value> {
        let invoice = self
            .get(entity_id)
            .ok_or_else(|| anyhow::anyhow!("Invoice not found: {}", entity_id))?;

        // Serialize to JSON
        Ok(serde_json::to_value(invoice)?)
    }
}

/// Implement EntityCreator for InvoiceStore
#[async_trait::async_trait]
impl EntityCreator for InvoiceStore {
    async fn create_from_json(&self, entity_data: serde_json::Value) -> Result<serde_json::Value> {
        let invoice = Invoice::new(
            entity_data["number"].as_str().unwrap_or("INV-000").to_string(),
            entity_data["status"].as_str().unwrap_or("active").to_string(),
            entity_data["number"].as_str().unwrap_or("INV-000").to_string(),
            entity_data["amount"].as_f64().unwrap_or(0.0),
            entity_data["due_date"].as_str().map(String::from),
            entity_data["paid_at"].as_str().map(String::from),
        );

        self.add(invoice.clone());
        Ok(serde_json::to_value(invoice)?)
    }
}
