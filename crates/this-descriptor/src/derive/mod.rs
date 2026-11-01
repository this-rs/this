//! Derivations of protocol artifacts from descriptors (pure functions, wasm32-compatible).
//!
//! - [`graphql_sdl`] / `graphql`: GraphQL SDL (object type, enums, create/update input
//!   types). `optional` fields are nullable; required-on-create fields are `!` in the
//!   create input.
//! - [`proto_file`] / [`proto`]: `.proto` file. Fields are emitted with THEIR `proto_tag`,
//!   sorted by tag: declaration order never changes a number. The strict derivation
//!   refuses a field without a tag (`this.descriptor.invalid`); the lenient one (used by the
//!   `/grpc/proto` documentation endpoint) comments the field out instead of inventing a
//!   number. [`proto::assign_tags`] is the append-only allocation used by `descriptor.lock`.
//! - [`json_schema`] / [`json_schema::openapi`]: JSON Schema (2020-12) of a write payload,
//!   `required` = [`FieldDesc::required_on`](crate::FieldDesc::required_on), and a minimal
//!   OpenAPI 3.1 document of the CRUD routes.
//!
//! Note on gRPC: the served wire is `google.protobuf.Struct` (`proto/this_grpc.proto`);
//! `proto_file` produces CLIENT-CODEGEN DOCUMENTATION, not the served contract, until a
//! typed dynamic service exists. `Struct` carries numbers as doubles: I64/U64/Decimal lose
//! precision on that wire. [`typescript`] answers "what TypeScript type is this field?" from the
//! descriptor — the consumer is `this-cli`, which generates `api-client.ts`, and any front that
//! wants the wire type rather than a guess from the Rust struct (RFC front-back-nomenclature §6.3).

pub mod graphql;
pub mod json_schema;
pub mod proto;
pub mod typescript;

use crate::engine::validate::WriteOp;
use crate::entity::EntityDesc;
use crate::error::ContractError;
use crate::module::DescriptorSet;

/// GraphQL SDL of every entity of the set (types only: the root `Query` / `Mutation` are
/// host-specific and emitted by the host).
pub fn graphql_sdl(set: &DescriptorSet) -> Result<String, ContractError> {
    Ok(graphql::sdl_for(&entities_sorted(set)))
}

/// `.proto` file (documentation of a typed API). Strict: every field needs a `proto_tag`.
pub fn proto_file(set: &DescriptorSet, package: &str) -> Result<String, ContractError> {
    proto::file_for(&entities_sorted(set), package, proto::TagMode::Strict)
}

/// JSON Schema of the write payload of `op` (required = `FieldDesc::required_on`).
pub fn json_schema(entity: &EntityDesc, op: WriteOp) -> Result<serde_json::Value, ContractError> {
    Ok(json_schema::write_schema(entity, op))
}

/// Is `f` required in the write payload of `op`? THE rule of
/// [`engine::validate::apply`](crate::engine::validate::apply), shared by every derivation
/// (JSON Schema `required`, GraphQL `!` on input types) so that a document never promises
/// more, or less, than the server enforces:
/// - a base field (`name`, `status`, link `source_id`..., overlaid or not) is required only
///   through an explicit `required` rule (the legacy creators fill it);
/// - a declared field follows [`FieldDesc::required_on`](crate::FieldDesc::required_on).
pub fn payload_required(e: &EntityDesc, f: &crate::FieldDesc, op: crate::field::Applies) -> bool {
    let is_base = crate::entity::base_fields(e.kind)
        .iter()
        .any(|b| b.name == f.name);
    if is_base {
        f.explicitly_required_on(op)
    } else {
        f.required_on(op)
    }
}

/// Entities of the set, sorted by name (deterministic output).
pub(crate) fn entities_sorted(set: &DescriptorSet) -> Vec<&EntityDesc> {
    let mut v: Vec<&EntityDesc> = set.modules().flat_map(|m| m.entities.iter()).collect();
    v.sort_by(|a, b| a.name.cmp(&b.name));
    v
}

/// `order_item` -> `OrderItem`.
pub fn pascal_case(s: &str) -> String {
    s.split('_')
        .filter(|p| !p.is_empty())
        .map(|p| {
            let mut c = p.chars();
            match c.next() {
                Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
                None => String::new(),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::custom::CustomRegistry;
    use crate::engine::validate::{Side, ValidateCtx, apply_desc};
    use crate::entity::EntityKind;
    use crate::field::{Applies, FieldDesc, FieldType, ValidatorSpec};
    use crate::framework::codes;
    use std::collections::BTreeSet;

    /// Required fields of `op` according to the JSON Schema, the GraphQL input type and the
    /// server engine (`apply` on an empty payload): the three must agree.
    fn required_three_ways(e: &EntityDesc, op: WriteOp) -> [BTreeSet<String>; 3] {
        let schema = json_schema(e, op).unwrap();
        let from_schema: BTreeSet<String> = schema["required"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().to_string())
            .collect();
        let suffix = match op {
            WriteOp::Create => "CreateInput",
            WriteOp::Update => "UpdateInput",
        };
        let sdl = graphql::sdl_for(&[e]);
        let start = format!("input {}{suffix} {{\n", pascal_case(&e.name));
        let from_sdl: BTreeSet<String> = match sdl.find(&start) {
            Some(i) => sdl[i + start.len()..]
                .lines()
                .take_while(|l| *l != "}")
                .filter(|l| l.ends_with('!'))
                .map(|l| l.trim().split(':').next().unwrap().to_string())
                .collect(),
            None => BTreeSet::new(),
        };
        let reg = CustomRegistry::default();
        let ctx = ValidateCtx {
            registry: &reg,
            side: Side::Server,
        };
        let from_engine: BTreeSet<String> = match apply_desc(e, op, serde_json::json!({}), &ctx) {
            Ok(_) => BTreeSet::new(),
            Err(err) => err
                .fields
                .into_iter()
                .filter(|f| f.code.as_str() == codes::VALIDATION_REQUIRED)
                .map(|f| f.path)
                .collect(),
        };
        [from_schema, from_sdl, from_engine]
    }

    #[test]
    fn derive_required_matches_engine() {
        let mut data = EntityDesc::new("invoice");
        data.fields = vec![
            FieldDesc::new("amount", FieldType::F64),
            FieldDesc::new("note", FieldType::Text).optional(),
            {
                let mut f = FieldDesc::new("currency", FieldType::String);
                f.default = Some(serde_json::json!("EUR"));
                f
            },
            FieldDesc::new("reason", FieldType::String)
                .optional()
                .validator(ValidatorSpec::Required, Applies::Update),
            FieldDesc::new("draft", FieldType::Bool)
                .validator(ValidatorSpec::Optional, Applies::Create),
            // Overlay of the base `name`: required only through its explicit rule.
            FieldDesc::new("name", FieldType::String)
                .tag(7)
                .validator(ValidatorSpec::Required, Applies::Create),
            FieldDesc::new("tenant_id", FieldType::Uuid).read_only(),
        ];
        let mut link = EntityDesc::new("membership");
        link.kind = EntityKind::Link;
        link.fields = vec![FieldDesc::new("role", FieldType::String)];
        for e in [&data, &link] {
            for op in [WriteOp::Create, WriteOp::Update] {
                let [schema, sdl, engine] = required_three_ways(e, op);
                assert_eq!(schema, engine, "{} {op:?}: JSON Schema vs engine", e.name);
                assert_eq!(sdl, engine, "{} {op:?}: GraphQL input vs engine", e.name);
            }
        }
        // And the expected content (the test proves something, not just agreement on {}).
        let [create, ..] = required_three_ways(&data, WriteOp::Create);
        assert_eq!(
            create,
            ["amount", "name"].iter().map(|s| s.to_string()).collect()
        );
        let [update, ..] = required_three_ways(&data, WriteOp::Update);
        assert_eq!(update, ["reason".to_string()].into_iter().collect());
        let [link_create, ..] = required_three_ways(&link, WriteOp::Create);
        assert_eq!(link_create, ["role".to_string()].into_iter().collect());
    }

    #[test]
    fn pascal_case_of_snake_names() {
        assert_eq!(pascal_case("order_item"), "OrderItem");
        assert_eq!(pascal_case("invoice"), "Invoice");
        assert_eq!(pascal_case("order_"), "Order");
    }
}
