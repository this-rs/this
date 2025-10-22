//! Entity descriptor for Order

use super::{create_order, get_order, handlers::OrderAppState, list_orders, store::OrderStore};
use axum::{routing::get, Router};
use this::prelude::EntityDescriptor;

/// Descriptor for the Order entity
pub struct OrderDescriptor {
    pub store: OrderStore,
}

impl OrderDescriptor {
    pub fn new(store: OrderStore) -> Self {
        Self { store }
    }
}

impl EntityDescriptor for OrderDescriptor {
    fn entity_type(&self) -> &str {
        "order"
    }

    fn plural(&self) -> &str {
        "orders"
    }

    fn build_routes(&self) -> Router {
        let state = OrderAppState {
            store: self.store.clone(),
        };

        Router::new()
            .route("/orders", get(list_orders).post(create_order))
            .route("/orders/{id}", get(get_order))
            .with_state(state)
    }
}
