//! Payment store implementation

use super::model::Payment;
use anyhow::Result;
use serde_json::{self, Value};
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

    async fn list_as_json(
        &self,
        limit: Option<i32>,
        offset: Option<i32>,
    ) -> Result<Vec<serde_json::Value>> {
        let all_payments = self.list();
        let offset = offset.unwrap_or(0) as usize;
        let limit = limit.unwrap_or(20) as usize;

        let payments: Vec<_> = all_payments.into_iter().skip(offset).take(limit).collect();

        payments
            .into_iter()
            .map(|payment| serde_json::to_value(payment).map_err(Into::into))
            .collect()
    }
}

/// Implement EntityCreator for PaymentStore
#[async_trait::async_trait]
impl EntityCreator for PaymentStore {
    async fn create_from_json(&self, entity_data: serde_json::Value) -> Result<serde_json::Value> {
        let payment = Payment::new(
            entity_data["number"]
                .as_str()
                .unwrap_or("PAY-000")
                .to_string(),
            entity_data["status"]
                .as_str()
                .unwrap_or("active")
                .to_string(),
            entity_data["number"]
                .as_str()
                .unwrap_or("PAY-000")
                .to_string(),
            entity_data["amount"].as_f64().unwrap_or(0.0),
            entity_data["method"].as_str().unwrap_or("card").to_string(),
            entity_data["transaction_id"].as_str().map(String::from),
        );

        self.add(payment.clone());
        Ok(serde_json::to_value(payment)?)
    }
}

/// Implement QueryableStore for PaymentStore
impl QueryableStore<Payment> for PaymentStore {
    fn apply_filters(&self, data: Vec<Payment>, filter: &Value) -> Vec<Payment> {
        let mut result = data;

        if let Some(obj) = filter.as_object() {
            for (key, value) in obj {
                result = match key.as_str() {
                    "number" => result
                        .into_iter()
                        .filter(|p| p.number == value.as_str().unwrap_or(""))
                        .collect(),

                    "status" => result
                        .into_iter()
                        .filter(|p| p.status == value.as_str().unwrap_or(""))
                        .collect(),

                    "method" => result
                        .into_iter()
                        .filter(|p| p.method == value.as_str().unwrap_or(""))
                        .collect(),

                    "amount>" => {
                        let threshold = value.as_f64().unwrap_or(0.0);
                        result
                            .into_iter()
                            .filter(|p| p.amount > threshold)
                            .collect()
                    }

                    "amount<" => {
                        let threshold = value.as_f64().unwrap_or(f64::MAX);
                        result
                            .into_iter()
                            .filter(|p| p.amount < threshold)
                            .collect()
                    }

                    _ => result,
                };
            }
        }

        result
    }

    fn apply_sort(&self, mut data: Vec<Payment>, sort: &str) -> Vec<Payment> {
        match sort {
            "number" | "number:asc" => data.sort_by(|a, b| a.number.cmp(&b.number)),
            "number:desc" => data.sort_by(|a, b| b.number.cmp(&a.number)),

            "amount" | "amount:asc" => {
                data.sort_by(|a, b| a.amount.partial_cmp(&b.amount).unwrap())
            }
            "amount:desc" => data.sort_by(|a, b| b.amount.partial_cmp(&a.amount).unwrap()),

            "created_at" | "created_at:asc" => data.sort_by(|a, b| a.created_at.cmp(&b.created_at)),
            "created_at:desc" => data.sort_by(|a, b| b.created_at.cmp(&a.created_at)),

            _ => {}
        }

        data
    }

    fn list_all(&self) -> Vec<Payment> {
        self.list()
    }
}
