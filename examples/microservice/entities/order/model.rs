//! Order entity model

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Order entity
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Order {
    // === Common fields (all entities) ===
    pub id: Uuid,
    pub tenant_id: Uuid,

    // === Standard fields (business entities) ===
    pub number: String, // Order number (e.g., "ORD-001")
    pub amount: f64,    // Total amount
    pub status: String, // Order status (pending, confirmed, cancelled)

    // === Order-specific fields ===
    pub customer_name: Option<String>,
    pub notes: Option<String>,
}
