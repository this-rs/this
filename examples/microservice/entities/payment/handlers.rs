//! Payment HTTP handlers

use super::{model::Payment, store::PaymentStore};
use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::Json,
};
use serde_json::{json, Value};
use uuid::Uuid;

/// Payment-specific AppState
#[derive(Clone)]
pub struct PaymentAppState {
    pub store: PaymentStore,
}

pub async fn list_payments(State(state): State<PaymentAppState>) -> Json<Value> {
    let payments = state.store.list();
    Json(json!({
        "payments": payments,
        "count": payments.len()
    }))
}

pub async fn get_payment(
    State(state): State<PaymentAppState>,
    Path(id): Path<String>,
) -> Result<Json<Payment>, StatusCode> {
    let id = Uuid::parse_str(&id).map_err(|_| StatusCode::BAD_REQUEST)?;
    state.store.get(&id).map(Json).ok_or(StatusCode::NOT_FOUND)
}

pub async fn create_payment(
    State(state): State<PaymentAppState>,
    Json(payload): Json<Value>,
) -> Result<Json<Payment>, StatusCode> {
    // Use the generated new() method from impl_data_entity!
    let payment = Payment::new(
        payload["number"].as_str().unwrap_or("PAY-000").to_string(), // name
        payload["status"].as_str().unwrap_or("active").to_string(),  // status
        payload["number"].as_str().unwrap_or("PAY-000").to_string(), // number
        payload["amount"].as_f64().unwrap_or(0.0),                   // amount
        payload["method"].as_str().unwrap_or("card").to_string(),    // method
        payload["transaction_id"].as_str().map(String::from),        // transaction_id
    );

    state.store.add(payment.clone());
    Ok(Json(payment))
}

pub async fn update_payment(
    State(state): State<PaymentAppState>,
    Path(id): Path<String>,
    Json(payload): Json<Value>,
) -> Result<Json<Payment>, StatusCode> {
    let id = Uuid::parse_str(&id).map_err(|_| StatusCode::BAD_REQUEST)?;

    let mut payment = state.store.get(&id).ok_or(StatusCode::NOT_FOUND)?;

    // Update fields if provided
    if let Some(name) = payload["name"].as_str() {
        payment.name = name.to_string();
    }
    if let Some(number) = payload["number"].as_str() {
        payment.number = number.to_string();
    }
    if let Some(amount) = payload["amount"].as_f64() {
        payment.amount = amount;
    }
    if let Some(status) = payload["status"].as_str() {
        payment.status = status.to_string();
    }
    if let Some(method) = payload["method"].as_str() {
        payment.method = method.to_string();
    }
    if let Some(transaction_id) = payload["transaction_id"].as_str() {
        payment.transaction_id = Some(transaction_id.to_string());
    }

    payment.touch(); // Update timestamp
    state.store.update(payment.clone());
    Ok(Json(payment))
}

pub async fn delete_payment(
    State(state): State<PaymentAppState>,
    Path(id): Path<String>,
) -> Result<StatusCode, StatusCode> {
    let id = Uuid::parse_str(&id).map_err(|_| StatusCode::BAD_REQUEST)?;

    state
        .store
        .delete(&id)
        .map(|_| StatusCode::NO_CONTENT)
        .ok_or(StatusCode::NOT_FOUND)
}
