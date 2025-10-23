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
    // Use the generated new() method from impl_data_entity!
    let order = Order::new(
        payload["number"].as_str().unwrap_or("ORD-000").to_string(), // name
        payload["status"].as_str().unwrap_or("active").to_string(),  // status
        payload["number"].as_str().unwrap_or("ORD-000").to_string(), // number
        payload["amount"].as_f64().unwrap_or(0.0),                   // amount
        payload["customer_name"].as_str().map(String::from),         // customer_name
        payload["notes"].as_str().map(String::from),                 // notes
    );

    state.store.add(order.clone());
    Ok(Json(order))
}

pub async fn update_order(
    State(state): State<OrderAppState>,
    Path(id): Path<String>,
    Json(payload): Json<Value>,
) -> Result<Json<Order>, StatusCode> {
    let id = Uuid::parse_str(&id).map_err(|_| StatusCode::BAD_REQUEST)?;

    let mut order = state.store.get(&id).ok_or(StatusCode::NOT_FOUND)?;

    // Update fields if provided
    if let Some(name) = payload["name"].as_str() {
        order.name = name.to_string();
    }
    if let Some(number) = payload["number"].as_str() {
        order.number = number.to_string();
    }
    if let Some(amount) = payload["amount"].as_f64() {
        order.amount = amount;
    }
    if let Some(status) = payload["status"].as_str() {
        order.status = status.to_string();
    }
    if let Some(customer_name) = payload["customer_name"].as_str() {
        order.customer_name = Some(customer_name.to_string());
    }
    if let Some(notes) = payload["notes"].as_str() {
        order.notes = Some(notes.to_string());
    }

    order.touch(); // Update timestamp
    state.store.update(order.clone());
    Ok(Json(order))
}

pub async fn delete_order(
    State(state): State<OrderAppState>,
    Path(id): Path<String>,
) -> Result<StatusCode, StatusCode> {
    let id = Uuid::parse_str(&id).map_err(|_| StatusCode::BAD_REQUEST)?;

    state
        .store
        .delete(&id)
        .map(|_| StatusCode::NO_CONTENT)
        .ok_or(StatusCode::NOT_FOUND)
}
