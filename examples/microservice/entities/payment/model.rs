//! Payment entity model

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Payment entity
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Payment {
    // === Common fields (all entities) ===
    pub id: Uuid,
    pub tenant_id: Uuid,

    // === Standard fields (business entities) ===
    pub number: String, // Payment reference (e.g., "PAY-001")
    pub amount: f64,    // Payment amount
    pub status: String, // Payment status (pending, completed, failed)

    // === Payment-specific fields ===
    pub method: String, // Payment method (card, bank_transfer, cash)
    pub transaction_id: Option<String>,
}
