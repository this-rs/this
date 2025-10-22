//! Entity descriptor for Invoice

use super::{
    create_invoice, get_invoice, handlers::InvoiceAppState, list_invoices, store::InvoiceStore,
};
use axum::{routing::get, Router};
use this::prelude::EntityDescriptor;

/// Descriptor for the Invoice entity
pub struct InvoiceDescriptor {
    pub store: InvoiceStore,
}

impl InvoiceDescriptor {
    pub fn new(store: InvoiceStore) -> Self {
        Self { store }
    }
}

impl EntityDescriptor for InvoiceDescriptor {
    fn entity_type(&self) -> &str {
        "invoice"
    }

    fn plural(&self) -> &str {
        "invoices"
    }

    fn build_routes(&self) -> Router {
        let state = InvoiceAppState {
            store: self.store.clone(),
        };

        Router::new()
            .route("/invoices", get(list_invoices).post(create_invoice))
            .route("/invoices/:id", get(get_invoice))
            .with_state(state)
    }
}
