//! Invoice store implementation

use super::model::Invoice;
use anyhow::Result;
use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use this::prelude::*;
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
}

impl Default for InvoiceStore {
    fn default() -> Self {
        Self::new()
    }
}

/// Implement EntityFetcher for InvoiceStore
///
/// This allows the link system to dynamically fetch Invoice entities
/// when enriching links.
#[async_trait::async_trait]
impl EntityFetcher for InvoiceStore {
    async fn fetch_as_json(
        &self,
        tenant_id: &Uuid,
        entity_id: &Uuid,
    ) -> Result<serde_json::Value> {
        let invoice = self
            .get(entity_id)
            .ok_or_else(|| anyhow::anyhow!("Invoice not found: {}", entity_id))?;

        // Verify tenant isolation
        if invoice.tenant_id != *tenant_id {
            anyhow::bail!("Invoice not found or access denied");
        }

        // Serialize to JSON
        Ok(serde_json::to_value(invoice)?)
    }
}
