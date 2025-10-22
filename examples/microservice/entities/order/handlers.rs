//! Order HTTP handlers

use super::{model::Order, store::OrderStore};
use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::Json,
};
use serde_json::{json, Value};
use uuid::Uuid;

/// Order-specific AppState
#[derive(Clone)]
pub struct OrderAppState {
    pub store: OrderStore,
}

pub async fn list_orders(State(state): State<OrderAppState>) -> Json<Value> {
    let orders = state.store.list();
    Json(json!({
        "orders": orders,
        "count": orders.len()
    }))
}

pub async fn get_order(
    State(state): State<OrderAppState>,
    Path(id): Path<String>,
) -> Result<Json<Order>, StatusCode> {
    let id = Uuid::parse_str(&id).map_err(|_| StatusCode::BAD_REQUEST)?;
    state.store.get(&id).map(Json).ok_or(StatusCode::NOT_FOUND)
}

pub async fn create_order(
    State(state): State<OrderAppState>,
    Json(payload): Json<Value>,
) -> Result<Json<Order>, StatusCode> {
    let order = Order {
        id: Uuid::new_v4(),
        tenant_id: Uuid::new_v4(), // In real app, extract from auth context
        number: payload["number"].as_str().unwrap_or("ORD-000").to_string(),
        amount: payload["amount"].as_f64().unwrap_or(0.0),
        status: payload["status"].as_str().unwrap_or("pending").to_string(),
        customer_name: payload["customer_name"].as_str().map(String::from),
        notes: payload["notes"].as_str().map(String::from),
    };
    state.store.add(order.clone());
    Ok(Json(order))
}
