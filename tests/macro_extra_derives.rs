//! Tests for the `extra_derives:` slot on `impl_data_entity!` and
//! `impl_data_entity_validated!`.
//!
//! These tests prove that downstream crates can attach additional derives
//! (e.g. `Hash`, `PartialEq`, `Eq`, third-party traits like `ts_rs::TS` or
//! `schemars::JsonSchema`) to macro-generated entity structs **without**
//! breaking the existing call-site syntax.
//!
//! We use `Hash`/`PartialEq`/`Eq` as proof traits because they are standard
//! library and don't require external crates. The forwarding mechanism is
//! identical for any derive path.

use this::prelude::*;

// ─────────────────────────────────────────────────────────────────────────
// 1. Backward-compat: `impl_data_entity!` without `extra_derives:` still works
// ─────────────────────────────────────────────────────────────────────────
//
// This is the pre-patch syntax. It must continue to compile and produce a
// struct with only the default derives (Debug, Clone, Serialize, Deserialize).
impl_data_entity!(
    LegacyEntity,
    "legacy_entity",
    ["name"],
    {
        value: i32,
    }
);

#[test]
fn legacy_syntax_still_works() {
    let e = LegacyEntity::new("test".to_string(), "active".to_string(), 42);
    assert_eq!(e.name, "test");
    assert_eq!(e.value, 42);
    // Debug + Clone are derived by default
    let _cloned = e.clone();
    let _ = format!("{:?}", e);
}

// ─────────────────────────────────────────────────────────────────────────
// 2. New slot: `extra_derives: [Hash, PartialEq, Eq]` is forwarded
// ─────────────────────────────────────────────────────────────────────────
//
// If the macro forwards correctly, the struct gets `Hash + PartialEq + Eq`
// and we can use it as a HashMap key. If forwarding is broken, this fails
// to compile.
impl_data_entity!(
    HashableEntity,
    "hashable_entity",
    ["name"],
    {
        value: i32,
    },
    extra_derives: [Hash, PartialEq, Eq]
);

#[test]
fn extra_derives_forwarded_on_data_entity() {
    use std::collections::HashSet;

    let mut set: HashSet<HashableEntity> = HashSet::new();
    let e = HashableEntity::new("a".to_string(), "active".to_string(), 1);
    let e_clone = e.clone();
    set.insert(e);

    // PartialEq + Hash work → HashSet sees the clone as the same entity
    // (well, same fields — id is regenerated via Uuid::new_v4 in `new()`,
    // so we just check the set is non-empty). What we really prove here:
    // the code COMPILES with HashSet<HashableEntity>, which requires
    // Hash + Eq to be present on the struct.
    assert_eq!(set.len(), 1);
    assert_eq!(e_clone.value, 1);
}

// ─────────────────────────────────────────────────────────────────────────
// 3. `impl_data_entity_validated!` also forwards `extra_derives:`
// ─────────────────────────────────────────────────────────────────────────
//
// Proves the validated wrapper passes the slot through to the inner
// `impl_data_entity!` invocation.
impl_data_entity_validated!(
    HashableValidatedEntity,
    "hashable_validated_entity",
    ["name"],
    {
        value: i32,
    },
    validate: {
        create: {
            value: [required],
        },
    },
    filters: {
        create: {
            value: [],
        },
    },
    extra_derives: [Hash, PartialEq, Eq]
);

#[test]
fn extra_derives_forwarded_on_validated_entity() {
    use std::collections::HashSet;

    let mut set: HashSet<HashableValidatedEntity> = HashSet::new();
    let e = HashableValidatedEntity::new("b".to_string(), "active".to_string(), 2);
    set.insert(e);
    assert_eq!(set.len(), 1);
}

// ─────────────────────────────────────────────────────────────────────────
// 4. Backward-compat for validated: pre-patch syntax still works
// ─────────────────────────────────────────────────────────────────────────
impl_data_entity_validated!(
    LegacyValidatedEntity,
    "legacy_validated_entity",
    ["name"],
    {
        value: i32,
    },
    validate: {
        create: {
            value: [required],
        },
    },
    filters: {
        create: {
            value: [],
        },
    }
);

#[test]
fn legacy_validated_syntax_still_works() {
    let e = LegacyValidatedEntity::new("c".to_string(), "active".to_string(), 3);
    assert_eq!(e.value, 3);
    let _cloned = e.clone();
}
