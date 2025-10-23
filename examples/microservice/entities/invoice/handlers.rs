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
    // Use the generated new() method from impl_data_entity!
    let invoice = Invoice::new(
        payload["number"].as_str().unwrap_or("INV-000").to_string(), // name
        payload["status"].as_str().unwrap_or("active").to_string(),  // status
        payload["number"].as_str().unwrap_or("INV-000").to_string(), // number
        payload["amount"].as_f64().unwrap_or(0.0),                   // amount
        payload["due_date"].as_str().map(String::from),              // due_date
        payload["paid_at"].as_str().map(String::from),               // paid_at
    );
    
    state.store.add(invoice.clone());
    Ok(Json(invoice))
}

pub async fn update_invoice(
    State(state): State<InvoiceAppState>,
    Path(id): Path<String>,
    Json(payload): Json<Value>,
) -> Result<Json<Invoice>, StatusCode> {
    let id = Uuid::parse_str(&id).map_err(|_| StatusCode::BAD_REQUEST)?;
    
    let mut invoice = state
        .store
        .get(&id)
        .ok_or(StatusCode::NOT_FOUND)?;
    
    // Update fields if provided
    if let Some(name) = payload["name"].as_str() {
        invoice.name = name.to_string();
    }
    if let Some(number) = payload["number"].as_str() {
        invoice.number = number.to_string();
    }
    if let Some(amount) = payload["amount"].as_f64() {
        invoice.amount = amount;
    }
    if let Some(status) = payload["status"].as_str() {
        invoice.status = status.to_string();
    }
    if let Some(due_date) = payload["due_date"].as_str() {
        invoice.due_date = Some(due_date.to_string());
    }
    if let Some(paid_at) = payload["paid_at"].as_str() {
        invoice.paid_at = Some(paid_at.to_string());
    }
    
    invoice.touch(); // Update timestamp
    state.store.update(invoice.clone());
    Ok(Json(invoice))
}

pub async fn delete_invoice(
    State(state): State<InvoiceAppState>,
    Path(id): Path<String>,
) -> Result<StatusCode, StatusCode> {
    let id = Uuid::parse_str(&id).map_err(|_| StatusCode::BAD_REQUEST)?;
    
    state
        .store
        .delete(&id)
        .map(|_| StatusCode::NO_CONTENT)
        .ok_or(StatusCode::NOT_FOUND)
}
