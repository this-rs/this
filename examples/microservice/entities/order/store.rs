//! Order store implementation

use super::model::Order;
use anyhow::Result;
use serde_json::{self, Value};
use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use this::prelude::*;
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

    async fn list_as_json(
        &self,
        limit: Option<i32>,
        offset: Option<i32>,
    ) -> Result<Vec<serde_json::Value>> {
        let all_orders = self.list();
        let offset = offset.unwrap_or(0) as usize;
        let limit = limit.unwrap_or(20) as usize;

        let orders: Vec<Order> = all_orders.into_iter().skip(offset).take(limit).collect();

        orders
            .into_iter()
            .map(|order| serde_json::to_value(order).map_err(Into::into))
            .collect()
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
            entity_data["number"]
                .as_str()
                .unwrap_or("ORD-000")
                .to_string(),
            entity_data["status"]
                .as_str()
                .unwrap_or("active")
                .to_string(),
            entity_data["number"]
                .as_str()
                .unwrap_or("ORD-000")
                .to_string(),
            entity_data["amount"].as_f64().unwrap_or(0.0),
            entity_data["customer_name"].as_str().map(String::from),
            entity_data["notes"].as_str().map(String::from),
        );

        self.add(order.clone());
        Ok(serde_json::to_value(order)?)
    }
}

/// Implement QueryableStore for OrderStore
///
/// This allows filtering and sorting of orders with generic query parameters.
impl QueryableStore<Order> for OrderStore {
    fn apply_filters(&self, data: Vec<Order>, filter: &Value) -> Vec<Order> {
        let mut result = data;

        if let Some(obj) = filter.as_object() {
            for (key, value) in obj {
                result = match key.as_str() {
                    // Exact matches
                    "number" => result
                        .into_iter()
                        .filter(|o| o.number == value.as_str().unwrap_or(""))
                        .collect(),

                    "status" => result
                        .into_iter()
                        .filter(|o| o.status == value.as_str().unwrap_or(""))
                        .collect(),

                    "customer_name" => result
                        .into_iter()
                        .filter(|o| {
                            o.customer_name
                                .as_ref()
                                .map(|n| n == value.as_str().unwrap_or(""))
                                .unwrap_or(false)
                        })
                        .collect(),

                    // Comparisons
                    "amount>" => {
                        let threshold = value.as_f64().unwrap_or(0.0);
                        result
                            .into_iter()
                            .filter(|o| o.amount > threshold)
                            .collect()
                    }

                    "amount<" => {
                        let threshold = value.as_f64().unwrap_or(f64::MAX);
                        result
                            .into_iter()
                            .filter(|o| o.amount < threshold)
                            .collect()
                    }

                    "amount>=" => {
                        let threshold = value.as_f64().unwrap_or(0.0);
                        result
                            .into_iter()
                            .filter(|o| o.amount >= threshold)
                            .collect()
                    }

                    "amount<=" => {
                        let threshold = value.as_f64().unwrap_or(f64::MAX);
                        result
                            .into_iter()
                            .filter(|o| o.amount <= threshold)
                            .collect()
                    }

                    _ => result,
                };
            }
        }

        result
    }

    fn apply_sort(&self, mut data: Vec<Order>, sort: &str) -> Vec<Order> {
        match sort {
            "number" | "number:asc" => data.sort_by(|a, b| a.number.cmp(&b.number)),
            "number:desc" => data.sort_by(|a, b| b.number.cmp(&a.number)),

            "amount" | "amount:asc" => {
                data.sort_by(|a, b| a.amount.partial_cmp(&b.amount).unwrap())
            }
            "amount:desc" => data.sort_by(|a, b| b.amount.partial_cmp(&a.amount).unwrap()),

            "created_at" | "created_at:asc" => data.sort_by(|a, b| a.created_at.cmp(&b.created_at)),
            "created_at:desc" => data.sort_by(|a, b| b.created_at.cmp(&a.created_at)),

            "updated_at" | "updated_at:asc" => data.sort_by(|a, b| a.updated_at.cmp(&b.updated_at)),
            "updated_at:desc" => data.sort_by(|a, b| b.updated_at.cmp(&a.updated_at)),

            _ => {}
        }

        data
    }

    fn list_all(&self) -> Vec<Order> {
        self.list()
    }
}
