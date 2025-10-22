//! Aggregated store for all entities
//!
//! This module provides a unified store that contains all entity stores.
//! Used by generic CRUD handlers.

use crate::entities::{
    invoice::store::InvoiceStore, order::store::OrderStore, payment::store::PaymentStore,
};

/// Aggregated store containing all entity stores
#[derive(Clone)]
pub struct EntityStore {
    pub orders: OrderStore,
    pub invoices: InvoiceStore,
    pub payments: PaymentStore,
}

impl EntityStore {
    pub fn new() -> Self {
        Self {
            orders: OrderStore::new(),
            invoices: InvoiceStore::new(),
            payments: PaymentStore::new(),
        }
    }
}

impl Default for EntityStore {
    fn default() -> Self {
        Self::new()
    }
}
