//! Entity descriptor for Payment

use super::{
    create_payment, delete_payment, get_payment, handlers::PaymentAppState, list_payments,
    store::PaymentStore, update_payment,
};
use axum::{Router, routing::get};
use this::prelude::EntityDescriptor;

/// Descriptor for the Payment entity
pub struct PaymentDescriptor {
    pub store: PaymentStore,
}

impl PaymentDescriptor {
    pub fn new(store: PaymentStore) -> Self {
        Self { store }
    }
}

impl EntityDescriptor for PaymentDescriptor {
    fn entity_type(&self) -> &str {
        "payment"
    }

    fn plural(&self) -> &str {
        "payments"
    }

    fn build_routes(&self) -> Router {
        let state = PaymentAppState {
            store: self.store.clone(),
        };

        Router::new()
            .route("/payments", get(list_payments).post(create_payment))
            .route(
                "/payments/{id}",
                get(get_payment).put(update_payment).delete(delete_payment),
            )
            .with_state(state)
    }
}
