//! Derivations of protocol artifacts from descriptors. Signatures frozen by the
//! foundation; implemented by the `schema` stream (they return `this.not_implemented`
//! until then).
//!
//! Note on gRPC: the served wire is `google.protobuf.Struct` (`proto/this_grpc.proto`);
//! `proto_file` produces CLIENT-CODEGEN DOCUMENTATION, not the served contract, until a
//! typed dynamic service exists. `Struct` carries numbers as doubles: I64/U64/Decimal lose
//! precision on that wire. TypeScript derivation is out of this batch (no consumer: the
//! front is Leptos).

use crate::engine::validate::WriteOp;
use crate::entity::EntityDesc;
use crate::error::ContractError;
use crate::module::DescriptorSet;

/// GraphQL SDL of every entity of the set.
pub fn graphql_sdl(_set: &DescriptorSet) -> Result<String, ContractError> {
    Err(ContractError::not_implemented("derive::graphql_sdl"))
}

/// `.proto` file (documentation of a typed API; fields need `proto_tag`).
pub fn proto_file(_set: &DescriptorSet, _package: &str) -> Result<String, ContractError> {
    Err(ContractError::not_implemented("derive::proto_file"))
}

/// JSON Schema of the write payload of `op` (required = `FieldDesc::required_on`).
pub fn json_schema(_entity: &EntityDesc, _op: WriteOp) -> Result<serde_json::Value, ContractError> {
    Err(ContractError::not_implemented("derive::json_schema"))
}
