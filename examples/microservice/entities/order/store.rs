//! Order store implementation

use super::model::Order;
use std::collections::HashMap;
use std::sync::{Arc, RwLock};
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
