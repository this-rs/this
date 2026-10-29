//! GraphQL SDL derived from an [`EntityDesc`].
//!
//! Mapping (the field's `optional` decides nullability on the object type):
//!
//! | `FieldType` | GraphQL |
//! |---|---|
//! | `String`, `Text` | `String` |
//! | `Bool` | `Boolean` |
//! | `I32` | `Int` |
//! | `U32`, `I64`, `U64` | `Int64` (scalar: GraphQL `Int` is a signed 32-bit integer) |
//! | `F32`, `F64` | `Float` |
//! | `Decimal` | `Decimal` (scalar, string or number on the wire) |
//! | `Uuid`, `Ref` | `ID` |
//! | `DateTime` / `Date` | `DateTime` / `Date` (scalars) |
//! | `Json`, `Custom` | `JSON` (scalar) |
//! | `Enum` | `enum <PascalName>` (falls back to `String` if a variant is not a GraphQL name, or if another entity declares the same enum name with other variants) |
//! | `List(T)` | `[T!]` |
//!
//! Inputs: `<Entity>CreateInput` (writable fields; `!` = `required_on(Create)`) and
//! `<Entity>UpdateInput` (writable and mutable fields; `!` only for an explicit `required`
//! rule on update). They document the shape of the `data: JSON!` argument of the host's
//! mutations.

use super::pascal_case;
use crate::entity::EntityDesc;
use crate::field::{Applies, FieldDesc, FieldType};
use std::collections::{BTreeMap, BTreeSet};

/// Accumulates the shared definitions (scalars, enums) while entity blocks are derived, so
/// that several entities produce ONE valid document.
#[derive(Debug, Default, Clone)]
pub struct SdlBuilder {
    scalars: BTreeSet<&'static str>,
    enums: BTreeMap<String, Vec<String>>,
}

/// Scalars this derivation may reference, with their documentation.
const SCALARS: &[(&str, &str)] = &[
    ("Date", "Calendar date, `YYYY-MM-DD`."),
    ("DateTime", "RFC 3339 date-time."),
    (
        "Decimal",
        "Decimal number, carried as a JSON string or number.",
    ),
    (
        "Int64",
        "64-bit integer (also used for u32, which exceeds GraphQL Int).",
    ),
    ("JSON", "Arbitrary JSON value."),
];

impl SdlBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    /// GraphQL type expression of `ty`, WITHOUT the outer `!`.
    pub fn type_ref(&mut self, ty: &FieldType) -> String {
        match ty {
            FieldType::String | FieldType::Text => "String".into(),
            FieldType::Bool => "Boolean".into(),
            FieldType::I32 => "Int".into(),
            FieldType::U32 | FieldType::I64 | FieldType::U64 => self.scalar("Int64"),
            FieldType::F32 | FieldType::F64 => "Float".into(),
            FieldType::Decimal { .. } => self.scalar("Decimal"),
            FieldType::Uuid | FieldType::Ref { .. } => "ID".into(),
            FieldType::DateTime => self.scalar("DateTime"),
            FieldType::Date => self.scalar("Date"),
            FieldType::Json | FieldType::Custom(_) => self.scalar("JSON"),
            FieldType::Enum { name, variants } => self.enum_ref(name, variants),
            FieldType::List(inner) => format!("[{}!]", self.type_ref(inner)),
            // `FieldType` is non_exhaustive: an unknown future type is documented as JSON.
            #[allow(unreachable_patterns)]
            _ => self.scalar("JSON"),
        }
    }

    fn scalar(&mut self, name: &'static str) -> String {
        self.scalars.insert(name);
        name.to_string()
    }

    fn enum_ref(&mut self, name: &str, variants: &[String]) -> String {
        let gql = pascal_case(name);
        let valid = !gql.is_empty()
            && !variants.is_empty()
            && variants.iter().all(|v| is_graphql_name(v))
            && !SCALARS.iter().any(|(s, _)| *s == gql)
            && !matches!(
                gql.as_str(),
                "String" | "Int" | "Float" | "Boolean" | "ID"
            );
        if !valid {
            return "String".into();
        }
        match self.enums.get(&gql) {
            Some(existing) if existing.as_slice() != variants => "String".into(),
            Some(_) => gql,
            None => {
                self.enums.insert(gql.clone(), variants.to_vec());
                gql
            }
        }
    }

    /// Lines `name: Type[!]` of the object type (base fields then declared fields).
    pub fn object_fields(&mut self, e: &EntityDesc) -> Vec<String> {
        e.all_fields()
            .map(|f| {
                let bang = if f.optional { "" } else { "!" };
                let dep = if f.deprecated { " @deprecated" } else { "" };
                format!("{}: {}{}{}", f.name, self.type_ref(&f.ty), bang, dep)
            })
            .collect()
    }

    /// `type <Entity> { ... }` with optional extra lines (relations).
    pub fn object_type(&mut self, e: &EntityDesc, extra: &[String]) -> String {
        let mut s = format!("type {} {{\n", pascal_case(&e.name));
        for line in self.object_fields(e).iter().chain(extra) {
            s.push_str("  ");
            s.push_str(line);
            s.push('\n');
        }
        s.push('}');
        s
    }

    /// `<Entity>CreateInput` and `<Entity>UpdateInput` (an empty input is omitted: GraphQL
    /// forbids it).
    pub fn input_types(&mut self, e: &EntityDesc) -> String {
        let pascal = pascal_case(&e.name);
        let mut out = String::new();
        for (suffix, op) in [("CreateInput", Applies::Create), ("UpdateInput", Applies::Update)] {
            let fields: Vec<&FieldDesc> = e
                .all_fields()
                .filter(|f| !f.read_only && !(op == Applies::Update && f.immutable))
                .collect();
            if fields.is_empty() {
                continue;
            }
            out.push_str(&format!("input {pascal}{suffix} {{\n"));
            for f in fields {
                let bang = if f.required_on(op) { "!" } else { "" };
                out.push_str(&format!("  {}: {}{}\n", f.name, self.type_ref(&f.ty), bang));
            }
            out.push_str("}\n\n");
        }
        out
    }

    /// Scalar and enum definitions referenced so far.
    pub fn definitions(&self) -> String {
        let mut s = String::new();
        for (name, doc) in SCALARS {
            if self.scalars.contains(name) {
                s.push_str(&format!("\"{doc}\"\nscalar {name}\n\n"));
            }
        }
        for (name, variants) in &self.enums {
            s.push_str(&format!("enum {name} {{\n"));
            for v in variants {
                s.push_str(&format!("  {v}\n"));
            }
            s.push_str("}\n\n");
        }
        s
    }
}

/// `[_A-Za-z][_0-9A-Za-z]*`, not `true`/`false`/`null` (GraphQL enum values).
fn is_graphql_name(s: &str) -> bool {
    let mut c = s.chars();
    matches!(c.next(), Some(f) if f == '_' || f.is_ascii_alphabetic())
        && c.all(|x| x == '_' || x.is_ascii_alphanumeric())
        && !matches!(s, "true" | "false" | "null")
}

/// SDL of `entities` (types, enums, scalars, inputs; no root types).
pub fn sdl_for(entities: &[&EntityDesc]) -> String {
    let mut b = SdlBuilder::new();
    let mut body = String::new();
    for e in entities {
        body.push_str(&b.object_type(e, &[]));
        body.push_str("\n\n");
        body.push_str(&b.input_types(e));
    }
    let mut out = b.definitions();
    out.push_str(&body);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::field::ValidatorSpec;

    fn invoice() -> EntityDesc {
        let mut e = EntityDesc::new("invoice");
        e.fields = vec![
            FieldDesc::new("amount", FieldType::F64),
            FieldDesc::new("note", FieldType::Text).optional(),
            FieldDesc::new("count", FieldType::U32),
            FieldDesc::new(
                "state",
                FieldType::Enum {
                    name: "invoice_state".into(),
                    variants: vec!["draft".into(), "sent".into()],
                },
            ),
            FieldDesc::new("lines", FieldType::List(Box::new(FieldType::Uuid))).optional(),
            FieldDesc::new("tenant_id", FieldType::Uuid).read_only(),
            FieldDesc::new("discount", FieldType::F64)
                .validator(ValidatorSpec::Required, Applies::Update),
        ];
        e
    }

    #[test]
    fn sdl_marks_optional_nullable() {
        let sdl = sdl_for(&[&invoice()]);
        assert!(sdl.contains("type Invoice {\n"), "{sdl}");
        assert!(sdl.contains("  amount: Float!\n"), "{sdl}");
        assert!(sdl.contains("  note: String\n"), "optional = nullable: {sdl}");
        assert!(sdl.contains("  deleted_at: DateTime\n"), "{sdl}");
        assert!(sdl.contains("  id: ID!\n"), "{sdl}");
        assert!(sdl.contains("  count: Int64!\n"), "u32 exceeds Int: {sdl}");
        assert!(sdl.contains("  lines: [ID!]\n"), "{sdl}");
        assert!(sdl.contains("  state: InvoiceState!\n"), "{sdl}");
        assert!(sdl.contains("enum InvoiceState {\n  draft\n  sent\n}"), "{sdl}");
        assert!(sdl.contains("scalar DateTime"), "{sdl}");
        assert!(sdl.contains("scalar Int64"), "{sdl}");
        assert!(!sdl.contains("scalar Decimal"), "unused scalar omitted: {sdl}");
        // Inputs: read-only excluded, required on create = `!`, update all nullable except
        // an explicit `required` on update.
        let create = section(&sdl, "input InvoiceCreateInput {");
        assert!(create.contains("  amount: Float!\n"), "{create}");
        assert!(create.contains("  note: String\n"), "{create}");
        assert!(!create.contains("tenant_id"), "{create}");
        assert!(!create.contains("  id:"), "{create}");
        let update = section(&sdl, "input InvoiceUpdateInput {");
        assert!(update.contains("  amount: Float\n"), "{update}");
        assert!(update.contains("  discount: Float!\n"), "{update}");
    }

    #[test]
    fn sdl_enum_conflict_and_invalid_variant_fall_back_to_string() {
        let mut a = EntityDesc::new("a");
        a.fields.push(FieldDesc::new(
            "s",
            FieldType::Enum {
                name: "state".into(),
                variants: vec!["x".into()],
            },
        ));
        let mut b = EntityDesc::new("b");
        b.fields.push(FieldDesc::new(
            "s",
            FieldType::Enum {
                name: "state".into(),
                variants: vec!["y".into()],
            },
        ));
        b.fields.push(FieldDesc::new(
            "t",
            FieldType::Enum {
                name: "kind".into(),
                variants: vec!["not-a-name".into()],
            },
        ));
        let sdl = sdl_for(&[&a, &b]);
        assert_eq!(sdl.matches("enum State {").count(), 1, "{sdl}");
        assert!(section(&sdl, "type B {").contains("  s: String!\n"), "{sdl}");
        assert!(section(&sdl, "type B {").contains("  t: String!\n"), "{sdl}");
    }

    #[test]
    fn link_entity_sdl_has_immutable_fields_only_in_create_input() {
        let mut l = EntityDesc::new("membership");
        l.kind = crate::entity::EntityKind::Link;
        let sdl = sdl_for(&[&l]);
        assert!(section(&sdl, "input MembershipCreateInput {").contains("source_id: ID!"));
        assert!(!section(&sdl, "input MembershipUpdateInput {").contains("source_id"));
    }

    fn section<'a>(sdl: &'a str, start: &str) -> &'a str {
        let i = sdl.find(start).unwrap_or_else(|| panic!("{start} in {sdl}"));
        let rest = &sdl[i..];
        &rest[..rest.find('}').unwrap() + 1]
    }
}
