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
//! Inputs: `<Entity>CreateInput` (writable fields) and `<Entity>UpdateInput` (writable and
//! mutable fields); `!` = [`payload_required`](super::payload_required), the rule the server
//! applies (a base field such as `name` is required only through an explicit rule). They
//! document the shape of the `data: JSON!` argument of the host's mutations.
//!
//! Names: an enum is never given a name already taken by an object or input type of the
//! document (entity types, `<Entity>CreateInput`, root types, scalars): it falls back to
//! `String` instead of producing a duplicate type definition.

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
    /// Type names an enum may not take (object, input and root types of the document).
    reserved: BTreeSet<String>,
}

/// Root type names of a GraphQL schema.
const ROOT_TYPES: &[&str] = &["Query", "Mutation", "Subscription"];

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

    /// Builder for a document that will hold the object and input types of `entities`:
    /// their names (and the root type names) are reserved, never taken by an enum.
    pub fn for_entities<'a>(entities: impl IntoIterator<Item = &'a EntityDesc>) -> Self {
        let mut b = Self::new();
        b.reserve(ROOT_TYPES.iter().copied());
        for e in entities {
            let p = pascal_case(&e.name);
            b.reserve([
                format!("{p}CreateInput"),
                format!("{p}UpdateInput"),
                p.clone(),
            ]);
        }
        b
    }

    /// Reserve type names (e.g. host types such as `NotificationList`): an enum that would
    /// take one of them is emitted as `String`.
    pub fn reserve<S: Into<String>>(&mut self, names: impl IntoIterator<Item = S>) {
        self.reserved.extend(names.into_iter().map(Into::into));
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
            && !self.reserved.contains(&gql)
            && !matches!(gql.as_str(), "String" | "Int" | "Float" | "Boolean" | "ID");
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
        for (suffix, op) in [
            ("CreateInput", Applies::Create),
            ("UpdateInput", Applies::Update),
        ] {
            let fields: Vec<&FieldDesc> = e
                .all_fields()
                .filter(|f| !f.read_only && !(op == Applies::Update && f.immutable))
                .collect();
            if fields.is_empty() {
                continue;
            }
            out.push_str(&format!("input {pascal}{suffix} {{\n"));
            for f in fields {
                let bang = if super::payload_required(e, f, op) {
                    "!"
                } else {
                    ""
                };
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
    let mut b = SdlBuilder::for_entities(entities.iter().copied());
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
        assert!(
            sdl.contains("  note: String\n"),
            "optional = nullable: {sdl}"
        );
        assert!(sdl.contains("  deleted_at: DateTime\n"), "{sdl}");
        assert!(sdl.contains("  id: ID!\n"), "{sdl}");
        assert!(sdl.contains("  count: Int64!\n"), "u32 exceeds Int: {sdl}");
        assert!(sdl.contains("  lines: [ID!]\n"), "{sdl}");
        assert!(sdl.contains("  state: InvoiceState!\n"), "{sdl}");
        assert!(
            sdl.contains("enum InvoiceState {\n  draft\n  sent\n}"),
            "{sdl}"
        );
        assert!(sdl.contains("scalar DateTime"), "{sdl}");
        assert!(sdl.contains("scalar Int64"), "{sdl}");
        assert!(
            !sdl.contains("scalar Decimal"),
            "unused scalar omitted: {sdl}"
        );
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
        assert!(
            section(&sdl, "type B {").contains("  s: String!\n"),
            "{sdl}"
        );
        assert!(
            section(&sdl, "type B {").contains("  t: String!\n"),
            "{sdl}"
        );
    }

    #[test]
    fn link_entity_sdl_has_immutable_fields_only_in_create_input() {
        let mut l = EntityDesc::new("membership");
        l.kind = crate::entity::EntityKind::Link;
        let sdl = sdl_for(&[&l]);
        let create = section(&sdl, "input MembershipCreateInput {");
        // Base fields are filled by the server (link route): present, never `!` without an
        // explicit rule (same rule as engine::validate::apply).
        assert!(create.contains("  source_id: ID\n"), "{create}");
        assert!(!section(&sdl, "input MembershipUpdateInput {").contains("source_id"));
    }

    #[test]
    fn sdl_enum_never_shadows_entity_type() {
        // Enum `invoice` on entity `invoice`, enum `query`, enum `order_create_input`:
        // each would define a type twice; they fall back to String.
        let en = |n: &str| FieldType::Enum {
            name: n.into(),
            variants: vec!["a".into()],
        };
        let mut inv = EntityDesc::new("invoice");
        inv.fields = vec![
            FieldDesc::new("kind", en("invoice")),
            FieldDesc::new("q", en("query")),
            FieldDesc::new("c", en("order_create_input")),
            FieldDesc::new("ok", en("invoice_kind")),
        ];
        let order = EntityDesc::new("order");
        let sdl = sdl_for(&[&inv, &order]);
        assert_eq!(sdl.matches("type Invoice {").count(), 1, "{sdl}");
        assert!(!sdl.contains("enum Invoice "), "{sdl}");
        assert!(!sdl.contains("enum Query "), "{sdl}");
        assert!(!sdl.contains("enum OrderCreateInput "), "{sdl}");
        assert!(sdl.contains("enum InvoiceKind {"), "{sdl}");
        let t = section(&sdl, "type Invoice {");
        assert!(t.contains("  kind: String!\n"), "{t}");
        assert!(t.contains("  q: String!\n"), "{t}");
        assert!(t.contains("  c: String!\n"), "{t}");
        assert!(t.contains("  ok: InvoiceKind!\n"), "{t}");
        // A host can reserve its own type names.
        let mut b = SdlBuilder::for_entities([&inv]);
        b.reserve(["InvoiceKind"]);
        assert_eq!(b.type_ref(&en("invoice_kind")), "String");
    }

    fn section<'a>(sdl: &'a str, start: &str) -> &'a str {
        let i = sdl
            .find(start)
            .unwrap_or_else(|| panic!("{start} in {sdl}"));
        let rest = &sdl[i..];
        &rest[..rest.find('}').unwrap() + 1]
    }
}
