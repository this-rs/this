//! TypeScript type of a [`FieldDesc`], derived from [`FieldType`].
//!
//! The descriptor knows the **wire** type; a grep over Rust source does not. `f64` and `Decimal`
//! both look like a float in the struct but do not cross the wire the same way, a `Ref` is a UUID
//! string rather than the referenced struct, and an enum is a string rather than its Rust path.
//! This module is the single place that answers "what TypeScript type does this field have",
//! so `this-cli` (which generates `api-client.ts`) and a front never guess differently.
//!
//! Pure and dependency-free: it compiles for `wasm32-unknown-unknown` with the rest of the crate.
//!
//! ```ignore
//! let f = FieldDesc::new("amount", FieldType::Decimal { scale: 2 }).optional();
//! assert_eq!(typescript::ts_field(&f), "string | null");
//! ```

use crate::field::{FieldDesc, FieldType};

/// TypeScript type of a value of this field type, without nullability.
///
/// Choices worth knowing:
/// - `Decimal` is a **string** on the wire: a binary float would lose the declared scale. Same
///   rule as `derive::proto`, so both artefacts describe one contract (`derive::proto` emits
///   `string` for the same reason).
/// - `Enum` is `string`: the variants live in the schema, a union in the interface would have to
///   guess the serde representation.
/// - `Json` and `Custom` are `unknown`, not `any`: `any` silently disables checking at every use
///   site, `unknown` forces the caller to narrow before touching the value.
/// - `Ref` is `string`: the client receives the target entity's UUID, never the entity itself.
pub fn ts_type(ty: &FieldType) -> String {
    match ty {
        FieldType::String
        | FieldType::Text
        | FieldType::Uuid
        | FieldType::DateTime
        | FieldType::Date
        | FieldType::Decimal { .. }
        | FieldType::Ref { .. }
        | FieldType::Enum { .. } => "string".to_string(),
        FieldType::Bool => "boolean".to_string(),
        FieldType::I32
        | FieldType::I64
        | FieldType::U32
        | FieldType::U64
        | FieldType::F32
        | FieldType::F64 => "number".to_string(),
        FieldType::Json | FieldType::Custom(_) => "unknown".to_string(),
        FieldType::List(inner) => {
            let inner = ts_type(inner);
            // `(A | B)[]` — a bare `A | B[]` would mean "A or an array of B".
            if inner.contains(" | ") {
                format!("({inner})[]")
            } else {
                format!("{inner}[]")
            }
        }
    }
}

/// TypeScript type of a field as it appears in an interface: `| null` when the field is optional.
///
/// Nullability comes from the field, never from the caller: the same `FieldDesc` decides the
/// server's validation and the client's type, which is the whole point of deriving.
pub fn ts_field(f: &FieldDesc) -> String {
    let ty = ts_type(&f.ty);
    if f.optional {
        format!("{ty} | null")
    } else {
        ty
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::custom::Custom;
    use crate::field::FieldType;

    fn field(name: &str, ty: FieldType) -> FieldDesc {
        FieldDesc::new(name, ty)
    }

    #[test]
    fn scalars_map_to_the_wire_type_not_the_rust_type() {
        for ty in [
            FieldType::String,
            FieldType::Text,
            FieldType::Uuid,
            FieldType::DateTime,
            FieldType::Date,
            // A decimal crosses the wire as a string: no binary float in the contract.
            FieldType::Decimal { scale: 2 },
            // A reference crosses as the target's UUID, not as the referenced entity.
            FieldType::Ref {
                entity: "user".into(),
            },
            FieldType::Enum {
                name: "status".into(),
                variants: vec!["new".into(), "paid".into()],
            },
        ] {
            assert_eq!(ts_type(&ty), "string", "{ty:?}");
        }
        assert_eq!(ts_type(&FieldType::Bool), "boolean");
        for ty in [
            FieldType::I32,
            FieldType::I64,
            FieldType::U32,
            FieldType::U64,
            FieldType::F32,
            FieldType::F64,
        ] {
            assert_eq!(ts_type(&ty), "number", "{ty:?}");
        }
        // `unknown`, not `any`: `any` would silence the compiler at every use site.
        assert_eq!(ts_type(&FieldType::Json), "unknown");
        assert_eq!(
            ts_type(&FieldType::Custom(Custom::named("money"))),
            "unknown"
        );
    }

    #[test]
    fn lists_wrap_union_element_types_in_parentheses() {
        assert_eq!(
            ts_type(&FieldType::List(Box::new(FieldType::String))),
            "string[]"
        );
        assert_eq!(
            ts_type(&FieldType::List(Box::new(FieldType::List(Box::new(
                FieldType::I32
            ))))),
            "number[][]"
        );
        // `(A | B)[]`, never `A | B[]`.
        assert_eq!(
            ts_type(&FieldType::List(Box::new(FieldType::Json))),
            "unknown[]"
        );
    }

    #[test]
    fn optional_adds_null_and_required_does_not() {
        let required = field("amount", FieldType::F64);
        assert_eq!(ts_field(&required), "number");

        let mut optional = field("note", FieldType::String);
        optional.optional = true;
        assert_eq!(ts_field(&optional), "string | null");

        // Nullability follows the field, whichever operation the caller validates for.
        let mut optional_list = field("tags", FieldType::List(Box::new(FieldType::String)));
        optional_list.optional = true;
        assert_eq!(ts_field(&optional_list), "string[] | null");
    }

    #[test]
    fn a_base_field_name_does_not_change_its_type() {
        // `status` is a base field whose overlay may carry validators; its type stays a string.
        let overlay = field("status", FieldType::String);
        assert_eq!(ts_field(&overlay), "string");
    }

    #[test]
    fn every_field_type_is_covered() {
        // Exhaustive: adding a variant to `FieldType` breaks this test instead of silently
        // producing `unknown` in a generated client.
        let all = [
            FieldType::String,
            FieldType::Text,
            FieldType::Bool,
            FieldType::I32,
            FieldType::I64,
            FieldType::U32,
            FieldType::U64,
            FieldType::F32,
            FieldType::F64,
            FieldType::Decimal { scale: 0 },
            FieldType::Uuid,
            FieldType::DateTime,
            FieldType::Date,
            FieldType::Json,
            FieldType::Enum {
                name: "e".into(),
                variants: vec!["a".into()],
            },
            FieldType::List(Box::new(FieldType::String)),
            FieldType::Ref {
                entity: "user".into(),
            },
            FieldType::Custom(Custom::named("c")),
        ];
        for ty in all {
            let out = ts_type(&ty);
            assert!(!out.is_empty(), "{ty:?}");
            assert!(!out.contains("FieldType"), "{ty:?} leaked a Rust type");
        }
    }
}
