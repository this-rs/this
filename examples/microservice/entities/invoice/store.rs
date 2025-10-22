//! Invoice store implementation

use super::model::Invoice;
use std::collections::HashMap;
use std::sync::{Arc, RwLock};
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
