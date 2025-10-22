//! Payment store implementation

use super::model::Payment;
use std::collections::HashMap;
use std::sync::{Arc, RwLock};
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
