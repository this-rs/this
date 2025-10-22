//! Payment store implementation

use super::model::Payment;
use anyhow::Result;
use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use this::prelude::*;
use uuid::Uuid;

/// In-memory store for Payment entities
#[derive(Clone)]
pub struct PaymentStore {
    data: Arc<RwLock<HashMap<Uuid, Payment>>>,
}

impl PaymentStore {
    pub fn new() -> Self {
        Self {
            data: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub fn add(&self, payment: Payment) {
        self.data.write().unwrap().insert(payment.id, payment);
    }

    pub fn get(&self, id: &Uuid) -> Option<Payment> {
        self.data.read().unwrap().get(id).cloned()
    }

    pub fn list(&self) -> Vec<Payment> {
        self.data.read().unwrap().values().cloned().collect()
    }
}

impl Default for PaymentStore {
    fn default() -> Self {
        Self::new()
    }
}

/// Implement EntityFetcher for PaymentStore
///
/// This allows the link system to dynamically fetch Payment entities
/// when enriching links.
#[async_trait::async_trait]
impl EntityFetcher for PaymentStore {
    async fn fetch_as_json(
        &self,
        tenant_id: &Uuid,
        entity_id: &Uuid,
    ) -> Result<serde_json::Value> {
        let payment = self
            .get(entity_id)
            .ok_or_else(|| anyhow::anyhow!("Payment not found: {}", entity_id))?;

        // Verify tenant isolation
        if payment.tenant_id != *tenant_id {
            anyhow::bail!("Payment not found or access denied");
        }

        // Serialize to JSON
        Ok(serde_json::to_value(payment)?)
    }
}
