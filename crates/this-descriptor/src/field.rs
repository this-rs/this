//! Fields: type, presence, validators, input transforms, query capabilities.

use crate::custom::Custom;
use crate::i18n::MessageKey;
use crate::query::FilterOp;
use serde::{Deserialize, Serialize};

/// A field of an entity or a link.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FieldDesc {
    pub name: String,
    pub ty: FieldType,
    /// `Option<T>` in Rust, nullable elsewhere. A non-optional field without `default` is
    /// REQUIRED on create: presence is derived from here, `ValidatorSpec::Required` and
    /// `ValidatorSpec::Optional` are only per-operation overrides.
    #[serde(default, skip_serializing_if = "is_false")]
    pub optional: bool,
    /// Value used on create when the field is absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<serde_json::Value>,
    /// Protobuf tag. Optional: only `derive::proto` needs it, and the gRPC wire of this-rs
    /// is `google.protobuf.Struct` (field names, not tags). Assigned by `descriptor.lock`
    /// (CLI) or written explicitly; never assigned by declaration order.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proto_tag: Option<u32>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub validators: Vec<ValidatorRule>,
    /// Input transforms (trim, case...), run BEFORE validators. The macro grammar calls
    /// them `filters:`; here they are `transforms` so that "filter" only means query filters.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub transforms: Vec<TransformRule>,
    /// Query operators allowed on this field; empty = not filterable.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub filterable: Vec<FilterOp>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub sortable: bool,
    #[serde(default, skip_serializing_if = "FieldI18n::is_empty")]
    pub i18n: FieldI18n,
    /// Refused on create AND update (server-managed).
    #[serde(default, skip_serializing_if = "is_false")]
    pub read_only: bool,
    /// Writable on create, refused on update.
    #[serde(default, skip_serializing_if = "is_false")]
    pub immutable: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub deprecated: bool,
}

fn is_false(b: &bool) -> bool {
    !*b
}

impl FieldDesc {
    /// Minimal field: required, no rule.
    pub fn new(name: impl Into<String>, ty: FieldType) -> Self {
        Self {
            name: name.into(),
            ty,
            optional: false,
            default: None,
            proto_tag: None,
            validators: vec![],
            transforms: vec![],
            filterable: vec![],
            sortable: false,
            i18n: FieldI18n::default(),
            read_only: false,
            immutable: false,
            deprecated: false,
        }
    }
    pub fn optional(mut self) -> Self {
        self.optional = true;
        self
    }
    pub fn read_only(mut self) -> Self {
        self.read_only = true;
        self
    }
    pub fn immutable(mut self) -> Self {
        self.immutable = true;
        self
    }
    pub fn tag(mut self, tag: u32) -> Self {
        self.proto_tag = Some(tag);
        self
    }
    pub fn filterable(mut self, ops: &[FilterOp]) -> Self {
        self.filterable = ops.to_vec();
        self
    }
    pub fn sortable(mut self) -> Self {
        self.sortable = true;
        self
    }
    pub fn validator(mut self, rule: ValidatorSpec, on: Applies) -> Self {
        self.validators.push(ValidatorRule { rule, on });
        self
    }
    pub fn transform(mut self, rule: TransformSpec, on: Applies) -> Self {
        self.transforms.push(TransformRule { rule, on });
        self
    }

    /// Is the field required for `op` (absent or null → `this.validation.required`)?
    ///
    /// Create: explicit `Required` / `Optional` rule for create wins; otherwise
    /// `!optional && default.is_none()`. Update: only an explicit `Required` rule makes an
    /// ABSENT field required; an explicit `null` on a non-optional field is always refused.
    /// Required only through an explicit `Required` rule for `op` (base-field overlays).
    pub fn explicitly_required_on(&self, op: Applies) -> bool {
        let mut explicit = false;
        for r in self.validators.iter().filter(|r| r.on.covers(op)) {
            match r.rule {
                ValidatorSpec::Required => explicit = true,
                ValidatorSpec::Optional => explicit = false,
                _ => {}
            }
        }
        explicit
    }

    pub fn required_on(&self, op: Applies) -> bool {
        let mut explicit = None;
        for r in &self.validators {
            if r.on.covers(op) {
                match r.rule {
                    ValidatorSpec::Required => explicit = Some(true),
                    ValidatorSpec::Optional => explicit = Some(false),
                    _ => {}
                }
            }
        }
        match (explicit, op) {
            (Some(v), _) => v,
            (None, Applies::Create) => !self.optional && self.default.is_none(),
            (None, _) => false,
        }
    }
}

/// Type of a field.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum FieldType {
    String,
    Text,
    Bool,
    I32,
    I64,
    U32,
    U64,
    F32,
    F64,
    /// Decimal carried as a JSON string or number; `scale` digits after the point.
    Decimal {
        scale: u8,
    },
    Uuid,
    /// RFC 3339 date-time.
    DateTime,
    /// `YYYY-MM-DD`.
    Date,
    Json,
    Enum {
        name: String,
        variants: Vec<String>,
    },
    List(Box<FieldType>),
    /// UUID of a declared entity (this module or a required one).
    Ref {
        entity: String,
    },
    Custom(Custom),
}

impl FieldType {
    pub fn is_numeric(&self) -> bool {
        matches!(
            self,
            FieldType::I32
                | FieldType::I64
                | FieldType::U32
                | FieldType::U64
                | FieldType::F32
                | FieldType::F64
                | FieldType::Decimal { .. }
        )
    }
    pub fn is_textual(&self) -> bool {
        matches!(
            self,
            FieldType::String | FieldType::Text | FieldType::Enum { .. }
        )
    }

    /// Short name used in `this.validation.type_mismatch` (`expected`).
    pub fn label(&self) -> String {
        match self {
            FieldType::Decimal { scale } => format!("decimal({scale})"),
            FieldType::Enum { name, .. } => format!("enum {name}"),
            FieldType::List(t) => format!("list<{}>", t.label()),
            FieldType::Ref { entity } => format!("ref<{entity}>"),
            FieldType::Custom(c) => c.name.clone(),
            other => serde_json::to_value(other)
                .ok()
                .and_then(|v| v.as_str().map(String::from))
                .unwrap_or_default(),
        }
    }

    /// Does a non-null JSON value have this type? `Custom` types accept anything (their
    /// validator decides).
    pub fn accepts(&self, v: &serde_json::Value) -> bool {
        use serde_json::Value as V;
        match (self, v) {
            (FieldType::String | FieldType::Text, V::String(_)) => true,
            (FieldType::Bool, V::Bool(_)) => true,
            (FieldType::I32, V::Number(n)) => n.as_i64().is_some_and(|x| i32::try_from(x).is_ok()),
            (FieldType::I64, V::Number(n)) => n.as_i64().is_some(),
            (FieldType::U32, V::Number(n)) => n.as_u64().is_some_and(|x| u32::try_from(x).is_ok()),
            (FieldType::U64, V::Number(n)) => n.as_u64().is_some(),
            (FieldType::F32 | FieldType::F64, V::Number(_)) => true,
            (FieldType::Decimal { .. }, V::Number(_)) => true,
            (FieldType::Decimal { .. }, V::String(s)) => {
                let s = s.strip_prefix('-').unwrap_or(s);
                let mut parts = s.splitn(2, '.');
                let int = parts.next().unwrap_or("");
                let frac = parts.next().unwrap_or("0");
                !int.is_empty()
                    && int.bytes().all(|b| b.is_ascii_digit())
                    && frac.bytes().all(|b| b.is_ascii_digit())
            }
            (FieldType::Uuid | FieldType::Ref { .. }, V::String(s)) => {
                uuid::Uuid::parse_str(s).is_ok()
            }
            (FieldType::DateTime, V::String(s)) => chrono::DateTime::parse_from_rfc3339(s).is_ok(),
            (FieldType::Date, V::String(s)) => {
                chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d").is_ok()
            }
            (FieldType::Json, _) => true,
            (FieldType::Enum { variants, .. }, V::String(s)) => variants.iter().any(|x| x == s),
            (FieldType::List(inner), V::Array(items)) => {
                items.iter().all(|i| !i.is_null() && inner.accepts(i))
            }
            (FieldType::Custom(_), _) => true,
            _ => false,
        }
    }
}

/// i18n keys of a field.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct FieldI18n {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<MessageKey>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub help: Option<MessageKey>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub placeholder: Option<MessageKey>,
}
impl FieldI18n {
    pub fn is_empty(&self) -> bool {
        self.label.is_none() && self.help.is_none() && self.placeholder.is_none()
    }
}

/// When a rule applies. `Always` = create and update.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Applies {
    #[default]
    Always,
    Create,
    Update,
}

impl Applies {
    /// Does a rule declared `self` apply to operation `op` (`Create` or `Update`)?
    pub fn covers(self, op: Applies) -> bool {
        self == Applies::Always || self == op || op == Applies::Always
    }
    /// Macro grammar: `create` / `update`. Anything else is refused.
    pub fn from_macro_op(op: &str) -> Option<Applies> {
        match op {
            "create" => Some(Applies::Create),
            "update" => Some(Applies::Update),
            _ => None,
        }
    }
}

fn is_always(a: &Applies) -> bool {
    *a == Applies::Always
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ValidatorRule {
    pub rule: ValidatorSpec,
    #[serde(default, skip_serializing_if = "is_always")]
    pub on: Applies,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TransformRule {
    pub rule: TransformSpec,
    #[serde(default, skip_serializing_if = "is_always")]
    pub on: Applies,
}

/// Validators (= grammar of `add_validators_for_field!` + additions).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum ValidatorSpec {
    /// Presence override for one operation (see [`FieldDesc::required_on`]).
    Required,
    /// Presence override for one operation.
    Optional,
    /// `> 0`.
    Positive,
    /// Length in BYTES (`str::len`), as the legacy validator.
    StringLength {
        min: usize,
        max: usize,
    },
    /// Inclusive bound. `serde_json::Number`: integers keep their precision beyond 2^53.
    MinValue(serde_json::Number),
    MaxValue(serde_json::Number),
    InList(Vec<serde_json::Value>),
    /// strftime format, parsed with `chrono::NaiveDate` (legacy semantics).
    DateFormat(String),
    /// Regular expression (feature `pattern`; without it `check()` refuses the descriptor).
    Pattern(String),
    Email,
    Url,
    Custom(Custom),
}

impl ValidatorSpec {
    /// Stable name (`positive`, `string_length`...), also the suffix of its error code.
    pub fn name(&self) -> &str {
        match self {
            ValidatorSpec::Required => "required",
            ValidatorSpec::Optional => "optional",
            ValidatorSpec::Positive => "positive",
            ValidatorSpec::StringLength { .. } => "string_length",
            ValidatorSpec::MinValue(_) => "min_value",
            ValidatorSpec::MaxValue(_) => "max_value",
            ValidatorSpec::InList(_) => "in_list",
            ValidatorSpec::DateFormat(_) => "date_format",
            ValidatorSpec::Pattern(_) => "pattern",
            ValidatorSpec::Email => "email",
            ValidatorSpec::Url => "url",
            ValidatorSpec::Custom(c) => &c.name,
        }
    }
}

/// Input transforms.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum TransformSpec {
    Trim,
    Uppercase,
    Lowercase,
    RoundDecimals(u32),
    Custom(Custom),
}

// ---------------------------------------------------------------------------
// Rust type -> FieldType, for the `impl_data_entity!` family (macro_rules!).
// ---------------------------------------------------------------------------

/// Maps a Rust type to its `FieldType`. Implemented for the usual types; the macros use
/// [`FieldProbe`] (autoref specialisation), so a type WITHOUT an impl still compiles and
/// is described as `FieldType::Json` (additive: no existing entity breaks).
pub trait FieldTypeOf {
    fn field_type() -> FieldType;
    /// `true` for `Option<T>`.
    fn optional() -> bool {
        false
    }
}

macro_rules! simple_field_type {
    ($($t:ty => $v:expr),* $(,)?) => {
        $(impl FieldTypeOf for $t { fn field_type() -> FieldType { $v } })*
    };
}
simple_field_type!(
    String => FieldType::String,
    bool => FieldType::Bool,
    i8 => FieldType::I32, i16 => FieldType::I32, i32 => FieldType::I32,
    i64 => FieldType::I64,
    u8 => FieldType::U32, u16 => FieldType::U32, u32 => FieldType::U32,
    u64 => FieldType::U64,
    f32 => FieldType::F32, f64 => FieldType::F64,
    uuid::Uuid => FieldType::Uuid,
    chrono::NaiveDate => FieldType::Date,
    serde_json::Value => FieldType::Json,
);
impl<Tz: chrono::TimeZone> FieldTypeOf for chrono::DateTime<Tz> {
    fn field_type() -> FieldType {
        FieldType::DateTime
    }
}
impl<T: FieldTypeOf> FieldTypeOf for Option<T> {
    fn field_type() -> FieldType {
        T::field_type()
    }
    fn optional() -> bool {
        true
    }
}
impl<T: FieldTypeOf> FieldTypeOf for Vec<T> {
    fn field_type() -> FieldType {
        FieldType::List(Box::new(T::field_type()))
    }
}

/// Autoref-specialisation probe used by the macros:
/// `(&&FieldProbe::<T>::new()).probe_field_type()` picks the `FieldTypeOf` impl when it
/// exists, else the fallback (`Json`, non optional).
pub struct FieldProbe<T: ?Sized>(std::marker::PhantomData<fn() -> Box<T>>);

impl<T: ?Sized> FieldProbe<T> {
    #[allow(clippy::new_without_default)]
    pub fn new() -> Self {
        Self(std::marker::PhantomData)
    }
}

/// Specific impl (`T: FieldTypeOf`), reached through one more autoref.
pub trait ProbeKnown {
    fn probe_field_type(&self) -> (FieldType, bool);
}
impl<T: FieldTypeOf> ProbeKnown for &FieldProbe<T> {
    fn probe_field_type(&self) -> (FieldType, bool) {
        (T::field_type(), T::optional())
    }
}
/// Fallback for any type.
pub trait ProbeFallback {
    fn probe_field_type(&self) -> (FieldType, bool);
}
impl<T: ?Sized> ProbeFallback for FieldProbe<T> {
    fn probe_field_type(&self) -> (FieldType, bool) {
        (FieldType::Json, false)
    }
}

/// Constructors used by the `impl_data_entity_validated!` translation (this-rs macros), so
/// generated code needs no `serde_json` in the user's crate.
impl ValidatorSpec {
    pub fn max_value_f64(v: f64) -> Self {
        ValidatorSpec::MaxValue(serde_json::Number::from_f64(v).unwrap_or_else(|| 0.into()))
    }
    pub fn min_value_f64(v: f64) -> Self {
        ValidatorSpec::MinValue(serde_json::Number::from_f64(v).unwrap_or_else(|| 0.into()))
    }
    pub fn in_list_strings(values: Vec<String>) -> Self {
        ValidatorSpec::InList(values.into_iter().map(serde_json::Value::String).collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[allow(dead_code)]
    struct Opaque;

    #[test]
    fn probe_specialises_known_types_and_falls_back() {
        #[allow(unused_imports)]
        use super::{ProbeFallback, ProbeKnown};
        assert_eq!(
            (&&FieldProbe::<f64>::new()).probe_field_type(),
            (FieldType::F64, false)
        );
        assert_eq!(
            (&&FieldProbe::<Option<String>>::new()).probe_field_type(),
            (FieldType::String, true)
        );
        assert_eq!(
            (&&FieldProbe::<Vec<uuid::Uuid>>::new()).probe_field_type(),
            (FieldType::List(Box::new(FieldType::Uuid)), false)
        );
        assert_eq!(
            FieldProbe::<Opaque>::new().probe_field_type(),
            (FieldType::Json, false)
        );
    }

    #[test]
    fn presence_is_derived_from_optional() {
        let f = FieldDesc::new("amount", FieldType::F64);
        assert!(f.required_on(Applies::Create));
        assert!(!f.required_on(Applies::Update));
        assert!(!f.clone().optional().required_on(Applies::Create));
        let g = f.validator(ValidatorSpec::Optional, Applies::Create);
        assert!(!g.required_on(Applies::Create));
    }

    #[test]
    fn type_check_accepts_and_rejects() {
        use serde_json::json;
        assert!(FieldType::F64.accepts(&json!(1)));
        assert!(!FieldType::F64.accepts(&json!("1")));
        assert!(!FieldType::I32.accepts(&json!(1.5)));
        assert!(FieldType::Decimal { scale: 2 }.accepts(&json!("-12.50")));
        assert!(!FieldType::Decimal { scale: 2 }.accepts(&json!("12,5")));
        assert!(FieldType::Date.accepts(&json!("2026-10-29")));
        assert!(FieldType::Uuid.accepts(&json!("67e55044-10b1-426f-9247-bb680e5fe0c8")));
        assert!(!FieldType::Uuid.accepts(&json!("nope")));
    }
}
