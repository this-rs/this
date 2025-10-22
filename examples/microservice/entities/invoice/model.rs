//! Invoice entity model

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Invoice entity
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Invoice {
    // === Common fields (all entities) ===
    pub id: Uuid,
    pub tenant_id: Uuid,

    // === Standard fields (business entities) ===
    pub number: String, // Invoice number (e.g., "INV-001")
    pub amount: f64,    // Invoice amount
    pub status: String, // Invoice status (draft, sent, paid, overdue)

    // === Invoice-specific fields ===
    pub due_date: Option<String>,
    pub paid_at: Option<String>,
}
