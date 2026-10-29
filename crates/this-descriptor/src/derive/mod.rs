//! Derivations of protocol artifacts from descriptors (pure functions, wasm32-compatible).
//!
//! - [`graphql_sdl`] / [`graphql`]: GraphQL SDL (object type, enums, create/update input
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
//! precision on that wire. TypeScript derivation is out of this batch (no consumer: the
//! front is Leptos).

pub mod graphql;
pub mod json_schema;
pub mod proto;

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

    #[test]
    fn pascal_case_of_snake_names() {
        assert_eq!(pascal_case("order_item"), "OrderItem");
        assert_eq!(pascal_case("invoice"), "Invoice");
        assert_eq!(pascal_case("order_"), "Order");
    }
}
