//! # this-descriptor
//!
//! The serializable description of a this-rs module, declared ONCE and consumed by every
//! surface: REST, GraphQL, gRPC, WebSocket on the server (this-rs), the code generator
//! (this-cli) and wasm32 fronts (Leptos). See `docs/rfc/module-descriptor.md`.
//!
//! - Types: [`ModuleDescriptor`], [`EntityDesc`], [`FieldDesc`], [`LinkDesc`],
//!   [`PermDesc`] / [`PolicyExpr`], [`ErrorDesc`] / [`ContractError`], [`I18nDecl`],
//!   [`Query`].
//! - Checks: [`ModuleDescriptor::check`], [`resolve::resolve`], [`compat::check_compat`].
//! - Pure engines: [`engine::validate`], [`engine::query`], [`engine::policy`],
//!   [`engine::i18n`].
//!
//! No tokio, axum or tonic in the default build: `cargo check -p this-descriptor --target
//! wasm32-unknown-unknown` is part of the acceptance.

// `ContractError` IS the contract that crosses every surface; boxing it in every `Result`
// would make each call site heavier for a cold path (errors are not the hot path here).
#![allow(clippy::result_large_err)]

pub mod check;
pub mod compat;
pub mod custom;
pub mod derive;
pub mod engine;
pub mod entity;
pub mod error;
pub mod field;
pub mod framework;
pub mod i18n;
#[cfg(feature = "leptos")]
/// `impl FromServerFnError for ContractError` (Leptos server functions).
pub mod leptos;
pub mod lifecycle;
pub mod link;
pub mod lock;
pub mod module;
pub mod naming;
pub mod perm;
pub mod query;
pub mod resolve;
pub mod runtime;
#[cfg(feature = "wami")]
/// `wami::statements`: PermDesc -> sandbox policy statements (Track 4).
pub mod wami;

pub use check::{CheckProfile, DescriptorIssue};
pub use custom::{Custom, CustomKind, CustomRegistry, CustomScope};
pub use engine::i18n::{I18nCatalog, LocaleHints};
pub use entity::{EntityDesc, EntityI18n, EntityKind, base_fields};
pub use error::{ContractError, ErrorCode, ErrorDesc, ErrorKind, FieldViolation};
pub use field::{
    Applies, FieldDesc, FieldI18n, FieldType, FieldTypeOf, TransformRule, TransformSpec,
    ValidatorRule, ValidatorSpec,
};
pub use i18n::{I18nDecl, Locale, MessageBundle, MessageDecl, MessageKey, MessageSyntax};
pub use lifecycle::ModuleState;
pub use link::LinkDesc;
pub use module::{CompiledEntity, DESCRIPTOR_FORMAT, DescriptorSet, ModuleDescriptor, ModuleReq};
pub use perm::{Operation, PermDesc, PolicyExpr, PrincipalView};
pub use query::{FilterExpr, FilterOp, Page, Query, QuerySchema, SortDir, SortSpec, Untyped};
pub use runtime::{Handle, RuntimeDesc, RuntimeLimits};
#[cfg(feature = "wami")]
pub use wami::{Effect, WamiStatement, statements};

/// Link between a Rust type and its descriptor. Implemented by `impl_data_entity!` and
/// `impl_data_entity_validated!` (this-rs); cached in a `OnceLock`, never rebuilt.
pub trait Described {
    fn entity_desc() -> &'static EntityDesc;
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn sample() -> ModuleDescriptor {
        let mut m = ModuleDescriptor::new("billing", "1.2.0");
        m.requires.push(ModuleReq {
            name: "users".into(),
            version: "^1".into(),
            optional: false,
        });
        let mut e = EntityDesc::new("invoice");
        e.fields = vec![
            FieldDesc::new("number", FieldType::String)
                .tag(16)
                .transform(TransformSpec::Trim, Applies::Always)
                .validator(
                    ValidatorSpec::StringLength { min: 3, max: 50 },
                    Applies::Create,
                )
                .filterable(&[FilterOp::Eq, FilterOp::StartsWith])
                .sortable(),
            FieldDesc::new("amount", FieldType::Decimal { scale: 2 }).validator(
                ValidatorSpec::MaxValue(serde_json::Number::from(1_000_000)),
                Applies::Always,
            ),
            FieldDesc::new(
                "customer",
                FieldType::Ref {
                    entity: "user".into(),
                },
            )
            .optional(),
            FieldDesc::new("tags", FieldType::List(Box::new(FieldType::String))).optional(),
        ];
        e.reserved_tags = vec![17];
        e.permissions.ops.insert(
            Operation::Create,
            PolicyExpr::parse("all:authenticated,role:billing").unwrap(),
        );
        e.permissions.ops.insert(
            Operation::Custom("invoice.pay".into()),
            PolicyExpr::parse("perm:invoice.pay").unwrap(),
        );
        m.entities.push(e);
        m.links.push(LinkDesc::new("billed_to", "invoice", "user"));
        m.errors.push(ErrorDesc {
            code: "billing.invoice.already_paid".into(),
            kind: ErrorKind::Conflict,
            message_key: "billing.invoice.already_paid".into(),
            params: vec!["id".into()],
            description: None,
        });
        m.i18n.keys.push(MessageDecl {
            key: "billing.invoice.already_paid".into(),
            params: vec!["id".into()],
        });
        m
    }

    #[test]
    fn descriptor_json_roundtrip() {
        // Includes a Custom operation as a map key (would fail with a derived enum key).
        let m = sample();
        let s = m.to_json_pretty();
        assert!(s.contains("\"invoice.pay\""));
        let back = ModuleDescriptor::from_json(&s).unwrap();
        assert_eq!(back, m);
        m.check(&CustomRegistry::default(), CheckProfile::Compiled)
            .unwrap();
    }

    #[test]
    fn descriptor_yaml_roundtrip() {
        let m = sample();
        let y = serde_yaml::to_string(&m).unwrap();
        let back: ModuleDescriptor = serde_yaml::from_str(&y).unwrap();
        assert_eq!(back, m);
        // Hand-written YAML: plural and routes derived, policies as strings.
        let hand = r#"
name: shop
version: 0.1.0
entities:
  - name: knife
    fields:
      - { name: price, ty: f64, validators: [ { rule: positive } ] }
    permissions:
      list: public
      delete: "any(admin_only, resolver:is_owner)"
"#;
        let mut d: ModuleDescriptor = serde_yaml::from_str(hand).unwrap();
        d.normalize();
        assert_eq!(d.entities[0].plural, "knives");
        assert_eq!(
            d.entities[0].permissions.effective(&Operation::Get),
            PolicyExpr::Authenticated
        );
        d.check(&CustomRegistry::default(), CheckProfile::Compiled)
            .unwrap();
        // Unknown policy token in YAML is a parse error, never `authenticated`.
        assert!(
            serde_yaml::from_str::<ModuleDescriptor>(&hand.replace("public", "publik")).is_err()
        );
    }

    #[test]
    fn json_shape_is_stable() {
        let f = FieldDesc::new("amount", FieldType::F64)
            .validator(ValidatorSpec::Positive, Applies::Create);
        assert_eq!(
            serde_json::to_value(&f).unwrap(),
            json!({"name": "amount", "ty": "f64", "validators": [{"rule": "positive", "on": "create"}]})
        );
    }
}
