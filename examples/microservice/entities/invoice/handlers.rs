//! Invoice HTTP handlers

use super::{model::Invoice, store::InvoiceStore};
use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::Json,
};
use serde_json::{json, Value};
use uuid::Uuid;

/// Invoice-specific AppState
#[derive(Clone)]
pub struct InvoiceAppState {
    pub store: InvoiceStore,
}

pub async fn list_invoices(State(state): State<InvoiceAppState>) -> Json<Value> {
    let invoices = state.store.list();
    Json(json!({
        "invoices": invoices,
        "count": invoices.len()
    }))
}

pub async fn get_invoice(
    State(state): State<InvoiceAppState>,
    Path(id): Path<String>,
) -> Result<Json<Invoice>, StatusCode> {
    let id = Uuid::parse_str(&id).map_err(|_| StatusCode::BAD_REQUEST)?;
    state.store.get(&id).map(Json).ok_or(StatusCode::NOT_FOUND)
}

pub async fn create_invoice(
    State(state): State<InvoiceAppState>,
    Json(payload): Json<Value>,
) -> Result<Json<Invoice>, StatusCode> {
    let invoice = Invoice {
        id: Uuid::new_v4(),
        tenant_id: Uuid::new_v4(),
        number: payload["number"].as_str().unwrap_or("INV-000").to_string(),
        amount: payload["amount"].as_f64().unwrap_or(0.0),
        status: payload["status"].as_str().unwrap_or("draft").to_string(),
        due_date: payload["due_date"].as_str().map(String::from),
        paid_at: payload["paid_at"].as_str().map(String::from),
    };
    state.store.add(invoice.clone());
    Ok(Json(invoice))
}
