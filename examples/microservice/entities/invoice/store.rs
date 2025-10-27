//! Invoice store implementation

use super::model::Invoice;
use anyhow::Result;
use serde_json::{self, Value};
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
            entity_data["number"]
                .as_str()
                .unwrap_or("INV-000")
                .to_string(),
            entity_data["status"]
                .as_str()
                .unwrap_or("active")
                .to_string(),
            entity_data["number"]
                .as_str()
                .unwrap_or("INV-000")
                .to_string(),
            entity_data["amount"].as_f64().unwrap_or(0.0),
            entity_data["due_date"].as_str().map(String::from),
            entity_data["paid_at"].as_str().map(String::from),
        );

        self.add(invoice.clone());
        Ok(serde_json::to_value(invoice)?)
    }
}

/// Implement QueryableStore for InvoiceStore
impl QueryableStore<Invoice> for InvoiceStore {
    fn apply_filters(&self, data: Vec<Invoice>, filter: &Value) -> Vec<Invoice> {
        let mut result = data;

        if let Some(obj) = filter.as_object() {
            for (key, value) in obj {
                result = match key.as_str() {
                    "number" => result
                        .into_iter()
                        .filter(|i| i.number == value.as_str().unwrap_or(""))
                        .collect(),

                    "status" => result
                        .into_iter()
                        .filter(|i| i.status == value.as_str().unwrap_or(""))
                        .collect(),

                    "amount>" => {
                        let threshold = value.as_f64().unwrap_or(0.0);
                        result
                            .into_iter()
                            .filter(|i| i.amount > threshold)
                            .collect()
                    }

                    "amount<" => {
                        let threshold = value.as_f64().unwrap_or(f64::MAX);
                        result
                            .into_iter()
                            .filter(|i| i.amount < threshold)
                            .collect()
                    }

                    _ => result,
                };
            }
        }

        result
    }

    fn apply_sort(&self, mut data: Vec<Invoice>, sort: &str) -> Vec<Invoice> {
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

    fn list_all(&self) -> Vec<Invoice> {
        self.list()
    }
}
