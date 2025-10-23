//! Payment store implementation

use super::model::Payment;
use anyhow::Result;
use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use this::prelude::*;
use serde_json;
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

    pub fn update(&self, payment: Payment) {
        self.data.write().unwrap().insert(payment.id, payment);
    }

    pub fn delete(&self, id: &Uuid) -> Option<Payment> {
        self.data.write().unwrap().remove(id)
    }
}

impl Default for PaymentStore {
    fn default() -> Self {
        Self::new()
    }
}

/// Implement EntityFetcher for PaymentStore
#[async_trait::async_trait]
impl EntityFetcher for PaymentStore {
    async fn fetch_as_json(&self, entity_id: &Uuid) -> Result<serde_json::Value> {
        let payment = self
            .get(entity_id)
            .ok_or_else(|| anyhow::anyhow!("Payment not found: {}", entity_id))?;

        // Serialize to JSON
        Ok(serde_json::to_value(payment)?)
    }
}

/// Implement EntityCreator for PaymentStore
#[async_trait::async_trait]
impl EntityCreator for PaymentStore {
    async fn create_from_json(&self, entity_data: serde_json::Value) -> Result<serde_json::Value> {
        let payment = Payment::new(
            entity_data["number"].as_str().unwrap_or("PAY-000").to_string(),
            entity_data["status"].as_str().unwrap_or("active").to_string(),
            entity_data["number"].as_str().unwrap_or("PAY-000").to_string(),
            entity_data["amount"].as_f64().unwrap_or(0.0),
            entity_data["method"].as_str().unwrap_or("card").to_string(),
            entity_data["transaction_id"].as_str().map(String::from),
        );

        self.add(payment.clone());
        Ok(serde_json::to_value(payment)?)
    }
}
