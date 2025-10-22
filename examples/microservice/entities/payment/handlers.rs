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
    let payment = Payment {
        id: Uuid::new_v4(),
        tenant_id: Uuid::new_v4(),
        number: payload["number"].as_str().unwrap_or("PAY-000").to_string(),
        amount: payload["amount"].as_f64().unwrap_or(0.0),
        status: payload["status"].as_str().unwrap_or("pending").to_string(),
        method: payload["method"].as_str().unwrap_or("card").to_string(),
        transaction_id: payload["transaction_id"].as_str().map(String::from),
    };
    state.store.add(payment.clone());
    Ok(Json(payment))
}
