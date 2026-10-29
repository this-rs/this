//! Entities and their implicit base fields.

use crate::custom::Custom;
use crate::field::{FieldDesc, FieldType};
use crate::i18n::MessageKey;
use crate::naming::Pluralizer;
use crate::perm::PermDesc;
use crate::query::FilterOp;
use serde::{Deserialize, Serialize};
use std::sync::OnceLock;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EntityKind {
    #[default]
    Data,
    Link,
}

/// An entity of a module.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EntityDesc {
    /// Singular (`invoice`).
    pub name: String,
    /// THE source of the plural (`invoices`): routes, Mongo collection, links, CLI.
    /// Serialized explicitly; empty on input = `Pluralizer::pluralize(name)` (`normalize`).
    #[serde(default)]
    pub plural: String,
    #[serde(default)]
    pub kind: EntityKind,
    /// Declared fields. The base fields are implicit: [`base_fields`].
    #[serde(default)]
    pub fields: Vec<FieldDesc>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub indexed: Vec<String>,
    /// Tags of removed fields, never reused.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub reserved_tags: Vec<u32>,
    #[serde(default, skip_serializing_if = "PermDesc::is_empty")]
    pub permissions: PermDesc,
    #[serde(default, skip_serializing_if = "EntityI18n::is_empty")]
    pub i18n: EntityI18n,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub custom: Vec<Custom>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct EntityI18n {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<MessageKey>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label_plural: Option<MessageKey>,
}
impl EntityI18n {
    pub fn is_empty(&self) -> bool {
        self.label.is_none() && self.label_plural.is_none()
    }
}

impl EntityDesc {
    pub fn new(name: impl Into<String>) -> Self {
        let name = name.into();
        Self {
            plural: Pluralizer::pluralize(&name),
            name,
            kind: EntityKind::Data,
            fields: vec![],
            indexed: vec![],
            reserved_tags: vec![],
            permissions: PermDesc::default(),
            i18n: EntityI18n::default(),
            custom: vec![],
        }
    }

    /// Fill derived defaults (plural) and merge duplicated rules: a rule present with
    /// `on: Create` AND `on: Update` becomes one `Always` rule.
    pub fn normalize(&mut self) {
        if self.plural.is_empty() {
            self.plural = Pluralizer::pluralize(&self.name);
        }
        for f in &mut self.fields {
            merge_create_update(&mut f.validators);
            merge_create_update(&mut f.transforms);
        }
    }

    /// Declared field by name.
    pub fn field(&self, name: &str) -> Option<&FieldDesc> {
        self.fields.iter().find(|f| f.name == name)
    }

    /// Declared field, or base field of this kind.
    pub fn any_field(&self, name: &str) -> Option<&FieldDesc> {
        self.field(name)
            .or_else(|| base_fields(self.kind).iter().find(|f| f.name == name))
    }

    /// Base fields (unless overlaid by a declared field of the same name) then declared
    /// fields.
    pub fn all_fields(&self) -> impl Iterator<Item = &FieldDesc> {
        base_fields(self.kind)
            .iter()
            .filter(|b| self.field(&b.name).is_none())
            .chain(self.fields.iter())
    }

    /// Declared field, or a copy of the WRITABLE base field `name` added as an overlay (to
    /// carry validators / transforms on `name` or `status`). `None` if `name` is neither.
    pub fn field_or_overlay_mut(&mut self, name: &str) -> Option<&mut FieldDesc> {
        if self.field(name).is_none() {
            let base = base_fields(self.kind)
                .iter()
                .find(|b| b.name == name && !b.read_only && !b.immutable)?
                .clone();
            self.fields.push(base);
        }
        self.fields.iter_mut().find(|f| f.name == name)
    }

    pub fn has_capability(&self, name: &str) -> bool {
        self.custom.iter().any(|c| c.name == name)
    }
}

/// A per-operation rule (validator or transform).
trait OpRule {
    type Spec: PartialEq;
    fn spec(&self) -> &Self::Spec;
    fn on_mut(&mut self) -> &mut crate::field::Applies;
    fn on(&self) -> crate::field::Applies;
}
impl OpRule for crate::field::ValidatorRule {
    type Spec = crate::field::ValidatorSpec;
    fn spec(&self) -> &Self::Spec {
        &self.rule
    }
    fn on_mut(&mut self) -> &mut crate::field::Applies {
        &mut self.on
    }
    fn on(&self) -> crate::field::Applies {
        self.on
    }
}
impl OpRule for crate::field::TransformRule {
    type Spec = crate::field::TransformSpec;
    fn spec(&self) -> &Self::Spec {
        &self.rule
    }
    fn on_mut(&mut self) -> &mut crate::field::Applies {
        &mut self.on
    }
    fn on(&self) -> crate::field::Applies {
        self.on
    }
}

/// Merge `(rule, Create)` + `(rule, Update)` into `(rule, Always)` (order of the first kept).
fn merge_create_update<R: OpRule>(rules: &mut Vec<R>) {
    use crate::field::Applies;
    let mut i = 0;
    while i < rules.len() {
        if rules[i].on() == Applies::Create {
            let twin = (0..rules.len()).find(|&j| {
                j != i && rules[j].on() == Applies::Update && rules[j].spec() == rules[i].spec()
            });
            if let Some(j) = twin {
                *rules[i].on_mut() = Applies::Always;
                rules.remove(j);
                if j < i {
                    i -= 1;
                }
            }
        }
        i += 1;
    }
}

/// Write mode of a base field.
const RO: u8 = 1;
const IMM: u8 = 2;

fn base(name: &str, ty: FieldType, tag: u32, mode: u8, optional: bool) -> FieldDesc {
    let mut f = FieldDesc::new(name, ty).tag(tag);
    f.optional = optional;
    f.read_only = mode == RO;
    f.immutable = mode == IMM;
    f
}

/// Implicit base fields injected by `impl_data_entity!` / `impl_link_entity!`, with their
/// write semantics and reserved tags (1..=15 are reserved for the framework).
///
/// - Data: `id`, `type`, `created_at`, `updated_at`, `deleted_at` (read-only, server
///   managed), `status`, `name` (writable).
/// - Link: the same first five, `status`, then `link_type`, `source_id`, `target_id`
///   (immutable: set on create, refused on update).
///
/// `tenant_id` is NOT a base field (the macro does not inject it): it is a reserved NAME
/// (tag 8 by convention). When declared it must be `read_only` (the server sets it from the
/// tenant context; a client-supplied tenant would move a row across tenants).
pub fn base_fields(kind: EntityKind) -> &'static [FieldDesc] {
    static DATA: OnceLock<Vec<FieldDesc>> = OnceLock::new();
    static LINK: OnceLock<Vec<FieldDesc>> = OnceLock::new();
    let common = || {
        let range = [
            FilterOp::Eq,
            FilterOp::Gt,
            FilterOp::Gte,
            FilterOp::Lt,
            FilterOp::Lte,
        ];
        vec![
            base("id", FieldType::Uuid, 1, RO, false).filterable(&[FilterOp::Eq, FilterOp::In]),
            base("type", FieldType::String, 2, RO, false),
            base("created_at", FieldType::DateTime, 3, RO, false)
                .filterable(&range)
                .sortable(),
            base("updated_at", FieldType::DateTime, 4, RO, false)
                .filterable(&range)
                .sortable(),
            base("deleted_at", FieldType::DateTime, 5, RO, true).filterable(&[FilterOp::IsNull]),
            base("status", FieldType::String, 6, 0, false)
                .filterable(&[FilterOp::Eq, FilterOp::Ne, FilterOp::In])
                .sortable(),
        ]
    };
    match kind {
        EntityKind::Data => DATA.get_or_init(|| {
            let mut v = common();
            v.push(
                base("name", FieldType::String, 7, 0, false)
                    .filterable(&[FilterOp::Eq, FilterOp::Contains, FilterOp::StartsWith])
                    .sortable(),
            );
            v
        }),
        EntityKind::Link => LINK.get_or_init(|| {
            let mut v = common();
            v.push(base("link_type", FieldType::String, 9, IMM, false).filterable(&[FilterOp::Eq]));
            v.push(
                base("source_id", FieldType::Uuid, 10, IMM, false)
                    .filterable(&[FilterOp::Eq, FilterOp::In]),
            );
            v.push(
                base("target_id", FieldType::Uuid, 11, IMM, false)
                    .filterable(&[FilterOp::Eq, FilterOp::In]),
            );
            v
        }),
    }
}

/// Reserved field name (not injected, tag 8 by convention, must be read-only).
pub const TENANT_ID: &str = "tenant_id";
/// Tag reserved for `tenant_id`.
pub const TENANT_ID_TAG: u32 = 8;
/// First tag available for declared fields.
pub const FIRST_USER_TAG: u32 = 16;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::field::{Applies, ValidatorSpec};

    #[test]
    fn base_fields_read_only_semantics() {
        let data = base_fields(EntityKind::Data);
        let names: Vec<_> = data.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(
            names,
            [
                "id",
                "type",
                "created_at",
                "updated_at",
                "deleted_at",
                "status",
                "name"
            ]
        );
        for f in data {
            let ro = matches!(
                f.name.as_str(),
                "id" | "type" | "created_at" | "updated_at" | "deleted_at"
            );
            assert_eq!(f.read_only, ro, "{}", f.name);
            assert!(f.proto_tag.unwrap() < FIRST_USER_TAG);
        }
        let link = base_fields(EntityKind::Link);
        assert!(link.iter().any(|f| f.name == "source_id" && f.immutable));
        assert!(!link.iter().any(|f| f.name == "name"));
    }

    #[test]
    fn normalize_fills_plural_and_merges_create_update_rules() {
        let mut e: EntityDesc = serde_json::from_value(serde_json::json!({
            "name": "knife",
            "fields": [{ "name": "amount", "ty": "f64", "validators": [
                { "rule": "positive", "on": "create" },
                { "rule": "positive", "on": "update" },
                { "rule": "required", "on": "create" }
            ]}]
        }))
        .unwrap();
        e.normalize();
        assert_eq!(e.plural, "knives");
        let v = &e.fields[0].validators;
        assert_eq!(v.len(), 2);
        assert_eq!(v[0].rule, ValidatorSpec::Positive);
        assert_eq!(v[0].on, Applies::Always);
        assert_eq!(v[1].on, Applies::Create);
    }
}
