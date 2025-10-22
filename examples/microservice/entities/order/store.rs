//! Order store implementation

use super::model::Order;
use anyhow::Result;
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
    async fn fetch_as_json(
        &self,
        tenant_id: &Uuid,
        entity_id: &Uuid,
    ) -> Result<serde_json::Value> {
        let order = self
            .get(entity_id)
            .ok_or_else(|| anyhow::anyhow!("Order not found: {}", entity_id))?;

        // Verify tenant isolation
        if order.tenant_id != *tenant_id {
            anyhow::bail!("Order not found or access denied");
        }

        // Serialize to JSON
        Ok(serde_json::to_value(order)?)
    }
}
