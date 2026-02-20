//! Shared test harness for storage backend testing
//!
//! Provides `TestDataEntity` implementing `Entity + Data` with fields covering
//! all `FieldValue` variants, `TestLinkEntity` implementing `Entity + Link`,
//! and helper functions for creating test data.
//!
//! # Usage
//!
//! From any integration test file in `tests/`:
//! ```rust,ignore
//! mod storage_harness;
//! use storage_harness::*;
//! ```

#![allow(dead_code)]

use anyhow::Result;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use uuid::Uuid;

use this::core::entity::{Data, Entity, Link};
use this::core::field::FieldValue;
use this::core::link::LinkEntity;

// ---------------------------------------------------------------------------
// TestDataEntity — covers all FieldValue variants for thorough testing
// ---------------------------------------------------------------------------

/// A test entity with fields spanning all `FieldValue` variants.
///
/// Fields:
/// - `name`: String (also used by `Data::name()`)
/// - `email`: String (for search/filter testing)
/// - `age`: i64 (Integer variant)
/// - `score`: f64 (Float variant)
/// - `active`: bool (Boolean variant)
/// - `id`: Uuid (Uuid variant)
/// - `created_at`: DateTime (DateTime variant)
/// - Unknown fields return `None` (Null testing)
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TestDataEntity {
    pub id: Uuid,
    pub entity_type: String,
    pub name: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub deleted_at: Option<DateTime<Utc>>,
    pub status: String,
    // Custom fields for search testing (cover all FieldValue variants)
    pub email: String,
    pub age: i64,
    pub score: f64,
    pub active: bool,
}

impl Entity for TestDataEntity {
    type Service = ();

    fn resource_name() -> &'static str {
        "test_data_entities"
    }

    fn resource_name_singular() -> &'static str {
        "test_data_entity"
    }

    fn service_from_host(
        _host: &Arc<dyn std::any::Any + Send + Sync>,
    ) -> Result<Arc<Self::Service>> {
        Ok(Arc::new(()))
    }

    fn id(&self) -> Uuid {
        self.id
    }

    fn entity_type(&self) -> &str {
        &self.entity_type
    }

    fn created_at(&self) -> DateTime<Utc> {
        self.created_at
    }

    fn updated_at(&self) -> DateTime<Utc> {
        self.updated_at
    }

    fn deleted_at(&self) -> Option<DateTime<Utc>> {
        self.deleted_at
    }

    fn status(&self) -> &str {
        &self.status
    }
}

impl Data for TestDataEntity {
    fn name(&self) -> &str {
        &self.name
    }

    fn indexed_fields() -> &'static [&'static str] {
        &["name", "email", "age", "score", "active"]
    }

    fn field_value(&self, field: &str) -> Option<FieldValue> {
        match field {
            "name" => Some(FieldValue::String(self.name.clone())),
            "email" => Some(FieldValue::String(self.email.clone())),
            "age" => Some(FieldValue::Integer(self.age)),
            "score" => Some(FieldValue::Float(self.score)),
            "active" => Some(FieldValue::Boolean(self.active)),
            "id" => Some(FieldValue::Uuid(self.id)),
            "created_at" => Some(FieldValue::DateTime(self.created_at)),
            "status" => Some(FieldValue::String(self.status.clone())),
            _ => None,
        }
    }
}

// ---------------------------------------------------------------------------
// TestLinkEntity — implements Entity + Link traits
// ---------------------------------------------------------------------------

/// A test link entity implementing `Entity + Link` for generic link testing.
///
/// Note: `LinkService` uses the concrete `LinkEntity` struct, not this type.
/// This is for testing code that is generic over `L: Link`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TestLinkEntity {
    pub id: Uuid,
    pub entity_type: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub deleted_at: Option<DateTime<Utc>>,
    pub status: String,
    pub source_id: Uuid,
    pub target_id: Uuid,
    pub link_type: String,
}

impl Entity for TestLinkEntity {
    type Service = ();

    fn resource_name() -> &'static str {
        "test_links"
    }

    fn resource_name_singular() -> &'static str {
        "test_link"
    }

    fn service_from_host(
        _host: &Arc<dyn std::any::Any + Send + Sync>,
    ) -> Result<Arc<Self::Service>> {
        Ok(Arc::new(()))
    }

    fn id(&self) -> Uuid {
        self.id
    }

    fn entity_type(&self) -> &str {
        &self.entity_type
    }

    fn created_at(&self) -> DateTime<Utc> {
        self.created_at
    }

    fn updated_at(&self) -> DateTime<Utc> {
        self.updated_at
    }

    fn deleted_at(&self) -> Option<DateTime<Utc>> {
        self.deleted_at
    }

    fn status(&self) -> &str {
        &self.status
    }
}

impl Link for TestLinkEntity {
    fn source_id(&self) -> Uuid {
        self.source_id
    }

    fn target_id(&self) -> Uuid {
        self.target_id
    }

    fn link_type(&self) -> &str {
        &self.link_type
    }
}

// ---------------------------------------------------------------------------
// Helper functions — TestDataEntity creation
// ---------------------------------------------------------------------------

/// Create a `TestDataEntity` with a random ID and sensible defaults.
///
/// All timestamps are set to `Utc::now()`, status to `"active"`.
pub fn create_test_entity(
    name: &str,
    email: &str,
    age: i64,
    score: f64,
    active: bool,
) -> TestDataEntity {
    let now = Utc::now();
    TestDataEntity {
        id: Uuid::new_v4(),
        entity_type: "test_data".to_string(),
        name: name.to_string(),
        created_at: now,
        updated_at: now,
        deleted_at: None,
        status: "active".to_string(),
        email: email.to_string(),
        age,
        score,
        active,
    }
}

/// Create a `TestDataEntity` with a specific ID for deterministic testing.
pub fn create_test_entity_with_id(
    id: Uuid,
    name: &str,
    email: &str,
    age: i64,
    score: f64,
    active: bool,
) -> TestDataEntity {
    let now = Utc::now();
    TestDataEntity {
        id,
        entity_type: "test_data".to_string(),
        name: name.to_string(),
        created_at: now,
        updated_at: now,
        deleted_at: None,
        status: "active".to_string(),
        email: email.to_string(),
        age,
        score,
        active,
    }
}

/// Generate a batch of `n` diverse test entities with varied field values.
///
/// Useful for list/pagination/search testing. Each entity has a unique name
/// and varied age/score/active values.
pub fn sample_batch(n: usize) -> Vec<TestDataEntity> {
    (0..n)
        .map(|i| {
            create_test_entity(
                &format!("Entity_{}", i),
                &format!("entity_{}@test.com", i),
                (20 + i as i64) % 100,           // ages: 20, 21, 22, ..., wraps at 100
                (i as f64) * 1.5 + 0.5,          // scores: 0.5, 2.0, 3.5, 5.0, ...
                i % 2 == 0,                       // alternating active: true, false, true, ...
            )
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Helper functions — Link creation (using concrete LinkEntity)
// ---------------------------------------------------------------------------

/// Create a `LinkEntity` (for use with `LinkService`).
///
/// This wraps `LinkEntity::new()` with a simpler signature for tests.
pub fn create_test_link(
    source_id: Uuid,
    target_id: Uuid,
    link_type: &str,
) -> LinkEntity {
    LinkEntity::new(link_type, source_id, target_id, None)
}

/// Create a `LinkEntity` with JSON metadata.
pub fn create_test_link_with_metadata(
    source_id: Uuid,
    target_id: Uuid,
    link_type: &str,
    metadata: serde_json::Value,
) -> LinkEntity {
    LinkEntity::new(link_type, source_id, target_id, Some(metadata))
}

/// Create a `TestLinkEntity` (for code generic over `L: Link`).
pub fn create_test_link_entity(
    source_id: Uuid,
    target_id: Uuid,
    link_type: &str,
) -> TestLinkEntity {
    let now = Utc::now();
    TestLinkEntity {
        id: Uuid::new_v4(),
        entity_type: "test_link".to_string(),
        created_at: now,
        updated_at: now,
        deleted_at: None,
        status: "active".to_string(),
        source_id,
        target_id,
        link_type: link_type.to_string(),
    }
}

// ---------------------------------------------------------------------------
// Assertions helpers
// ---------------------------------------------------------------------------

/// Assert that a `TestDataEntity` has the expected name.
pub fn assert_entity_name(entity: &TestDataEntity, expected_name: &str) {
    assert_eq!(
        entity.name, expected_name,
        "Expected entity name '{}', got '{}'",
        expected_name, entity.name
    );
}

/// Assert that a list contains exactly `n` entities.
pub fn assert_count<T>(list: &[T], expected: usize) {
    assert_eq!(
        list.len(),
        expected,
        "Expected {} items, got {}",
        expected,
        list.len()
    );
}

/// Assert that a `FieldValue` matches the expected variant.
pub fn assert_field_value_string(fv: &FieldValue, expected: &str) {
    match fv {
        FieldValue::String(s) => assert_eq!(s, expected),
        other => panic!("Expected FieldValue::String(\"{}\"), got {:?}", expected, other),
    }
}

pub fn assert_field_value_integer(fv: &FieldValue, expected: i64) {
    match fv {
        FieldValue::Integer(i) => assert_eq!(*i, expected),
        other => panic!("Expected FieldValue::Integer({}), got {:?}", expected, other),
    }
}

pub fn assert_field_value_float(fv: &FieldValue, expected: f64) {
    match fv {
        FieldValue::Float(f) => assert!(
            (*f - expected).abs() < f64::EPSILON,
            "Expected FieldValue::Float({}), got FieldValue::Float({})",
            expected,
            f
        ),
        other => panic!("Expected FieldValue::Float({}), got {:?}", expected, other),
    }
}

pub fn assert_field_value_boolean(fv: &FieldValue, expected: bool) {
    match fv {
        FieldValue::Boolean(b) => assert_eq!(*b, expected),
        other => panic!("Expected FieldValue::Boolean({}), got {:?}", expected, other),
    }
}

pub fn assert_field_value_uuid(fv: &FieldValue, expected: Uuid) {
    match fv {
        FieldValue::Uuid(u) => assert_eq!(*u, expected),
        other => panic!("Expected FieldValue::Uuid({}), got {:?}", expected, other),
    }
}
