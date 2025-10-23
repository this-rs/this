//! Order store implementation

use super::model::Order;
use anyhow::Result;
use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use this::prelude::*;
use serde_json;
use uuid::Uuid;

/// In-memory store for Order entities
#[derive(Clone)]
pub struct OrderStore {
    data: Arc<RwLock<HashMap<Uuid, Order>>>,
}

impl OrderStore {
    pub fn new() -> Self {
        Self {
            data: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub fn add(&self, order: Order) {
        self.data.write().unwrap().insert(order.id, order);
    }

    pub fn get(&self, id: &Uuid) -> Option<Order> {
        self.data.read().unwrap().get(id).cloned()
    }

    pub fn list(&self) -> Vec<Order> {
        self.data.read().unwrap().values().cloned().collect()
    }

    pub fn update(&self, order: Order) {
        self.data.write().unwrap().insert(order.id, order);
    }

    pub fn delete(&self, id: &Uuid) -> Option<Order> {
        self.data.write().unwrap().remove(id)
    }
}

impl Default for OrderStore {
    fn default() -> Self {
        Self::new()
    }
}

/// Implement EntityFetcher for OrderStore
///
/// This allows the link system to dynamically fetch Order entities
/// when enriching links.
#[async_trait::async_trait]
impl EntityFetcher for OrderStore {
    async fn fetch_as_json(&self, entity_id: &Uuid) -> Result<serde_json::Value> {
        let order = self
            .get(entity_id)
            .ok_or_else(|| anyhow::anyhow!("Order not found: {}", entity_id))?;

        // Serialize to JSON
        Ok(serde_json::to_value(order)?)
    }
}

/// Implement EntityCreator for OrderStore
///
/// This allows the link system to dynamically create Order entities
/// when creating linked entities.
#[async_trait::async_trait]
impl EntityCreator for OrderStore {
    async fn create_from_json(&self, entity_data: serde_json::Value) -> Result<serde_json::Value> {
        let order = Order::new(
            entity_data["number"].as_str().unwrap_or("ORD-000").to_string(),
            entity_data["status"].as_str().unwrap_or("active").to_string(),
            entity_data["number"].as_str().unwrap_or("ORD-000").to_string(),
            entity_data["amount"].as_f64().unwrap_or(0.0),
            entity_data["customer_name"].as_str().map(String::from),
            entity_data["notes"].as_str().map(String::from),
        );

        self.add(order.clone());
        Ok(serde_json::to_value(order)?)
    }
}
