//! # Le fil de changements : ce que le domaine publie, ce que l'interface applique
//!
//! Une écriture ne dit pas seulement « ça a marché » : elle dit **ce qui** a changé. C'est
//! ce qui permet à une interface de corriger un seul enregistrement au lieu de tout
//! recharger, et à une seconde fenêtre (ou un second appareil) de suivre sans interroger.
//!
//! Le contrat est volontairement minimal : `entity`, `id`, `op`, et l'enregistrement
//! après écriture (absent pour une suppression). Il ne dépend ni du transport (SSE,
//! événement Tauri, WebSocket) ni du module.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Une écriture, telle qu'elle est notifiée.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Change {
    pub entity: String,
    pub id: String,
    pub op: ChangeOp,
    /// L'enregistrement **après** écriture ; `None` pour une suppression.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub record: Option<Value>,
}

impl Change {
    pub fn created(entity: impl Into<String>, record: Value) -> Self {
        let id = record["id"].as_str().unwrap_or_default().to_string();
        Self {
            entity: entity.into(),
            id,
            op: ChangeOp::Created,
            record: Some(record),
        }
    }

    pub fn updated(entity: impl Into<String>, record: Value) -> Self {
        let id = record["id"].as_str().unwrap_or_default().to_string();
        Self {
            entity: entity.into(),
            id,
            op: ChangeOp::Updated,
            record: Some(record),
        }
    }

    pub fn deleted(entity: impl Into<String>, id: impl Into<String>) -> Self {
        Self {
            entity: entity.into(),
            id: id.into(),
            op: ChangeOp::Deleted,
            record: None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ChangeOp {
    Created,
    Updated,
    Deleted,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_creation_carries_its_record_and_id() {
        let change = Change::created("order", json!({ "id": "ord-1", "total": 10.0 }));
        assert_eq!(change.entity, "order");
        assert_eq!(change.id, "ord-1");
        assert_eq!(change.op, ChangeOp::Created);
        assert!(change.record.is_some());
    }

    #[test]
    fn a_deletion_carries_no_record() {
        let change = Change::deleted("order", "ord-1");
        assert_eq!(change.op, ChangeOp::Deleted);
        assert!(change.record.is_none());
        let raw = serde_json::to_string(&change).unwrap();
        assert!(!raw.contains("record"), "champ absent, pas null : {raw}");
    }

    #[test]
    fn the_wire_shape_is_the_contract() {
        let raw = serde_json::to_value(Change::updated(
            "product",
            json!({ "id": "prod-1", "stock": 4 }),
        ))
        .unwrap();
        assert_eq!(raw["entity"], "product");
        assert_eq!(raw["id"], "prod-1");
        assert_eq!(raw["op"], "updated");
        assert_eq!(raw["record"]["stock"], 4);
    }
}
