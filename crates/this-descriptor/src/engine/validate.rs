//! Pure validation engine: transforms, then presence, type and validators.
//!
//! Runs identically on the server (`ValidatingCreator`, REST validation layer) and in a
//! wasm32 front. Only active for entities that HAVE a descriptor.

use crate::custom::{ALLOW_UNKNOWN_FIELDS, CustomRegistry, CustomScope};
use crate::entity::base_fields;
use crate::error::{ContractError, FieldViolation};
use crate::field::{Applies, FieldDesc, TransformSpec, ValidatorSpec};
use crate::framework::codes;
use crate::module::CompiledEntity;
use serde_json::{Number, Value, json};
use std::cmp::Ordering;

/// Write operation being validated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriteOp {
    Create,
    Update,
}

impl WriteOp {
    fn applies(self) -> Applies {
        match self {
            WriteOp::Create => Applies::Create,
            WriteOp::Update => Applies::Update,
        }
    }
}

/// Which side runs the engine (decides what to do with server-only custom validators).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Side {
    #[default]
    Server,
    Client,
}

/// Context of a validation.
#[derive(Debug, Clone, Copy)]
pub struct ValidateCtx<'a> {
    pub registry: &'a CustomRegistry,
    pub side: Side,
}

/// Successful validation.
#[derive(Debug, Clone, PartialEq)]
pub struct ValidationOutcome {
    /// Payload after transforms and defaults.
    pub value: Value,
    /// Server-only custom validators NOT run on this side (`field:rule`). Always empty on
    /// the server.
    pub deferred: Vec<String>,
}

/// Validate `payload` for `op` against an entity.
///
/// Order: shape → unknown / read-only / immutable keys → transforms → presence (create:
/// absent required field; any op: explicit `null` on a non-optional field) → type →
/// validators. All violations are collected into ONE `this.validation.failed` error.
pub fn apply(
    entity: &CompiledEntity,
    op: WriteOp,
    payload: Value,
    ctx: &ValidateCtx<'_>,
) -> Result<ValidationOutcome, ContractError> {
    let Value::Object(mut obj) = payload else {
        return Err(
            ContractError::framework(codes::VALIDATION_FAILED).with_field(violation(
                "",
                codes::VALIDATION_NOT_OBJECT,
                &[],
            )),
        );
    };
    let desc = &entity.desc;
    let base = base_fields(desc.kind);
    let allow_unknown = desc.has_capability(ALLOW_UNKNOWN_FIELDS);
    let mut fields = Vec::new();
    let mut deferred = Vec::new();

    // Keys.
    for key in obj.keys() {
        let f = entity
            .field(key)
            .or_else(|| base.iter().find(|b| &b.name == key));
        match f {
            None if !allow_unknown => fields.push(violation(
                key,
                codes::VALIDATION_UNKNOWN_FIELD,
                &[("field", json!(key))],
            )),
            Some(f) if f.read_only => fields.push(violation(
                key,
                codes::VALIDATION_READ_ONLY,
                &[("field", json!(key))],
            )),
            Some(f) if f.immutable && op == WriteOp::Update => fields.push(violation(
                key,
                codes::VALIDATION_IMMUTABLE,
                &[("field", json!(key))],
            )),
            _ => {}
        }
    }

    // Transforms then checks, field by field (declared fields only; writable base fields
    // `name` / `status` are type-checked).
    for f in desc.fields.iter().chain(
        base.iter()
            .filter(|b| !b.read_only && !b.immutable && entity.field(&b.name).is_none()),
    ) {
        if f.read_only {
            // Server-managed: presence already refused above, never required.
            continue;
        }
        let present = obj.contains_key(&f.name);
        if present {
            let mut v = obj.remove(&f.name).unwrap_or(Value::Null);
            for t in f.transforms.iter().filter(|t| t.on.covers(op.applies())) {
                v = transform(&t.rule, v, ctx.registry)?;
            }
            obj.insert(f.name.clone(), v);
        } else if op == WriteOp::Create {
            if let Some(d) = &f.default {
                obj.insert(f.name.clone(), d.clone());
            }
        }
        let v = obj.get(&f.name);
        match v {
            None | Some(Value::Null) => {
                let explicit_null = matches!(v, Some(Value::Null));
                // Writable base fields (`name`, `status`) are filled by the legacy creators:
                // only an explicit `null` is refused. Declared fields follow `required_on`.
                let is_base = base.iter().any(|b| b.name == f.name);
                let required = (explicit_null && !f.optional)
                    || if is_base {
                        f.explicitly_required_on(op.applies())
                    } else {
                        f.required_on(op.applies())
                    };
                if required {
                    fields.push(violation(
                        &f.name,
                        codes::VALIDATION_REQUIRED,
                        &[("field", json!(f.name))],
                    ));
                }
            }
            Some(v) => {
                if !f.ty.accepts(v) {
                    fields.push(violation(
                        &f.name,
                        codes::VALIDATION_TYPE_MISMATCH,
                        &[("field", json!(f.name)), ("expected", json!(f.ty.label()))],
                    ));
                    continue;
                }
                for r in f.validators.iter().filter(|r| r.on.covers(op.applies())) {
                    run_validator(entity, f, &r.rule, v, ctx, &mut fields, &mut deferred)?;
                }
            }
        }
    }

    if fields.is_empty() {
        Ok(ValidationOutcome {
            value: Value::Object(obj),
            deferred,
        })
    } else {
        let mut e = ContractError::framework(codes::VALIDATION_FAILED);
        e.fields = fields;
        Err(e)
    }
}

fn violation(path: &str, code: &str, params: &[(&str, Value)]) -> FieldViolation {
    let mut v = FieldViolation::new(path, code);
    for (k, p) in params {
        v.params.insert(k.to_string(), p.clone());
    }
    v.message = crate::framework::default_message(code, &v.params);
    v
}

fn transform(t: &TransformSpec, v: Value, reg: &CustomRegistry) -> Result<Value, ContractError> {
    Ok(match (t, v) {
        (TransformSpec::Trim, Value::String(s)) => Value::String(s.trim().to_string()),
        (TransformSpec::Uppercase, Value::String(s)) => Value::String(s.to_uppercase()),
        (TransformSpec::Lowercase, Value::String(s)) => Value::String(s.to_lowercase()),
        (TransformSpec::RoundDecimals(d), Value::Number(n))
            if n.as_f64().is_some() && !n.is_i64() && !n.is_u64() =>
        {
            let factor = 10_f64.powi(*d as i32);
            json!((n.as_f64().unwrap() * factor).round() / factor)
        }
        (TransformSpec::Custom(c), v) => match reg.transform(&c.name) {
            Some(f) => f(v, &c.params),
            None => {
                return Err(ContractError::framework(codes::INTERNAL)
                    .with_message(format!("custom transform `{}` not registered", c.name)));
            }
        },
        (_, v) => v,
    })
}

/// Compare two JSON numbers without losing integer precision.
pub fn cmp_numbers(a: &Number, b: &Number) -> Option<Ordering> {
    if let (Some(x), Some(y)) = (a.as_i64(), b.as_i64()) {
        return Some(x.cmp(&y));
    }
    if let (Some(x), Some(y)) = (a.as_u64(), b.as_u64()) {
        return Some(x.cmp(&y));
    }
    a.as_f64()?.partial_cmp(&b.as_f64()?)
}

/// Number carried by a value: JSON number, or decimal string.
fn number_of(v: &Value) -> Option<Number> {
    match v {
        Value::Number(n) => Some(n.clone()),
        Value::String(s) => s.parse::<f64>().ok().and_then(Number::from_f64),
        _ => None,
    }
}

fn is_email(s: &str) -> bool {
    let mut parts = s.splitn(2, '@');
    let (Some(local), Some(domain)) = (parts.next(), parts.next()) else {
        return false;
    };
    !local.is_empty()
        && !domain.contains('@')
        && domain.contains('.')
        && !domain.starts_with('.')
        && !domain.ends_with('.')
        && !s.chars().any(char::is_whitespace)
}

fn is_url(s: &str) -> bool {
    let rest = s
        .strip_prefix("https://")
        .or_else(|| s.strip_prefix("http://"));
    matches!(rest, Some(r) if !r.is_empty() && !r.starts_with('/') && !s.chars().any(char::is_whitespace))
}

fn run_validator(
    entity: &CompiledEntity,
    f: &FieldDesc,
    rule: &ValidatorSpec,
    v: &Value,
    ctx: &ValidateCtx<'_>,
    out: &mut Vec<FieldViolation>,
    deferred: &mut Vec<String>,
) -> Result<(), ContractError> {
    let name = f.name.as_str();
    let field = ("field", json!(name));
    match rule {
        ValidatorSpec::Required | ValidatorSpec::Optional => {}
        ValidatorSpec::Positive => {
            if let Some(n) = number_of(v) {
                if n.as_f64().is_some_and(|x| x <= 0.0) {
                    out.push(violation(
                        name,
                        codes::VALIDATION_POSITIVE,
                        &[field, ("value", v.clone())],
                    ));
                }
            }
        }
        ValidatorSpec::StringLength { min, max } => {
            if let Some(s) = v.as_str() {
                let len = s.len();
                if len < *min || len > *max {
                    out.push(violation(
                        name,
                        codes::VALIDATION_STRING_LENGTH,
                        &[
                            field,
                            ("min", json!(min)),
                            ("max", json!(max)),
                            ("len", json!(len)),
                        ],
                    ));
                }
            }
        }
        ValidatorSpec::MinValue(min) => {
            if let Some(n) = number_of(v) {
                if cmp_numbers(&n, min) == Some(Ordering::Less) {
                    out.push(violation(
                        name,
                        codes::VALIDATION_MIN_VALUE,
                        &[field, ("min", json!(min)), ("value", v.clone())],
                    ));
                }
            }
        }
        ValidatorSpec::MaxValue(max) => {
            if let Some(n) = number_of(v) {
                if cmp_numbers(&n, max) == Some(Ordering::Greater) {
                    out.push(violation(
                        name,
                        codes::VALIDATION_MAX_VALUE,
                        &[field, ("max", json!(max)), ("value", v.clone())],
                    ));
                }
            }
        }
        ValidatorSpec::InList(allowed) => {
            if !allowed.contains(v) {
                let shown: Vec<String> = allowed
                    .iter()
                    .map(|a| {
                        a.as_str()
                            .map(String::from)
                            .unwrap_or_else(|| a.to_string())
                    })
                    .collect();
                out.push(violation(
                    name,
                    codes::VALIDATION_IN_LIST,
                    &[
                        field,
                        ("allowed", json!(format!("{shown:?}"))),
                        ("value", v.clone()),
                    ],
                ));
            }
        }
        ValidatorSpec::DateFormat(fmt) => {
            if let Some(s) = v.as_str() {
                if chrono::NaiveDate::parse_from_str(s, fmt).is_err() {
                    out.push(violation(
                        name,
                        codes::VALIDATION_DATE_FORMAT,
                        &[field, ("format", json!(fmt)), ("value", v.clone())],
                    ));
                }
            }
        }
        ValidatorSpec::Pattern(p) => {
            #[cfg(feature = "pattern")]
            {
                let ok = match entity.patterns.get(p) {
                    Some(re) => v.as_str().is_none_or(|s| re.is_match(s)),
                    None => false,
                };
                if !ok {
                    out.push(violation(
                        name,
                        codes::VALIDATION_PATTERN,
                        &[field, ("pattern", json!(p))],
                    ));
                }
            }
            #[cfg(not(feature = "pattern"))]
            {
                let _ = entity;
                return Err(ContractError::framework(codes::INTERNAL)
                    .with_message(format!("pattern `{p}` needs the `pattern` feature")));
            }
        }
        ValidatorSpec::Email => {
            if v.as_str().is_some_and(|s| !is_email(s)) {
                out.push(violation(name, codes::VALIDATION_EMAIL, &[field]));
            }
        }
        ValidatorSpec::Url => {
            if v.as_str().is_some_and(|s| !is_url(s)) {
                out.push(violation(name, codes::VALIDATION_URL, &[field]));
            }
        }
        ValidatorSpec::Custom(c) => match ctx.registry.validator(&c.name) {
            Ok(Some((scope, func))) => {
                if scope == CustomScope::ServerOnly && ctx.side == Side::Client {
                    deferred.push(format!("{name}:{}", c.name));
                } else if let Err(mut viol) = func(name, v, &c.params) {
                    if viol.message.is_empty() {
                        viol.message = crate::framework::default_message(
                            codes::VALIDATION_CUSTOM,
                            &[
                                ("field".to_string(), json!(name)),
                                ("rule".to_string(), json!(c.name)),
                            ]
                            .into_iter()
                            .collect(),
                        );
                    }
                    out.push(viol);
                }
            }
            Ok(None) if ctx.side == Side::Client => deferred.push(format!("{name}:{}", c.name)),
            // Known by name only on the server, or unknown: fail-closed.
            Ok(None) | Err(()) => {
                return Err(
                    ContractError::framework(codes::INTERNAL).with_message(format!(
                        "custom validator `{}` has no implementation here",
                        c.name
                    )),
                );
            }
        },
    }
    Ok(())
}

/// Convenience: validate against an `EntityDesc` without a prebuilt `CompiledEntity`
/// (compiles patterns on every call; servers use `DescriptorSet::compiled`).
pub fn apply_desc(
    entity: &crate::entity::EntityDesc,
    op: WriteOp,
    payload: Value,
    ctx: &ValidateCtx<'_>,
) -> Result<ValidationOutcome, ContractError> {
    let compiled = CompiledEntity::new("", entity.clone())?;
    apply(&compiled, op, payload, ctx)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::custom::CustomRegistry;
    use crate::entity::EntityDesc;
    use crate::field::{FieldDesc, FieldType};
    use std::sync::Arc;

    fn invoice() -> CompiledEntity {
        let mut e = EntityDesc::new("invoice");
        e.fields = vec![
            FieldDesc::new("number", FieldType::String)
                .transform(TransformSpec::Trim, Applies::Always)
                .transform(TransformSpec::Uppercase, Applies::Always)
                .validator(
                    ValidatorSpec::StringLength { min: 3, max: 50 },
                    Applies::Always,
                ),
            FieldDesc::new("amount", FieldType::F64)
                .validator(ValidatorSpec::Positive, Applies::Always),
            FieldDesc::new("due", FieldType::String)
                .optional()
                .validator(
                    ValidatorSpec::DateFormat("%Y-%m-%d".into()),
                    Applies::Always,
                ),
            FieldDesc::new("tenant_id", FieldType::Uuid).read_only(),
        ];
        CompiledEntity::new("billing", e).unwrap()
    }
    fn ctx(r: &CustomRegistry) -> ValidateCtx<'_> {
        ValidateCtx {
            registry: r,
            side: Side::Server,
        }
    }
    fn codes(e: ContractError) -> Vec<String> {
        e.fields
            .into_iter()
            .map(|f| format!("{}:{}", f.path, f.code.0))
            .collect()
    }

    #[test]
    fn required_detects_absent_field_on_create() {
        let r = CustomRegistry::default();
        let e = apply(
            &invoice(),
            WriteOp::Create,
            json!({"number": "abc"}),
            &ctx(&r),
        )
        .unwrap_err();
        assert_eq!(e.code.as_str(), codes::VALIDATION_FAILED);
        assert_eq!(codes(e), vec!["amount:this.validation.required"]);
    }

    #[test]
    fn update_validates_only_present_fields_but_refuses_explicit_null() {
        let r = CustomRegistry::default();
        apply(&invoice(), WriteOp::Update, json!({"amount": 3}), &ctx(&r)).unwrap();
        let e = apply(
            &invoice(),
            WriteOp::Update,
            json!({"amount": null}),
            &ctx(&r),
        )
        .unwrap_err();
        assert_eq!(codes(e), vec!["amount:this.validation.required"]);
        apply(&invoice(), WriteOp::Update, json!({"due": null}), &ctx(&r)).unwrap();
    }

    #[test]
    fn transforms_run_before_validators() {
        let r = CustomRegistry::default();
        let out = apply(
            &invoice(),
            WriteOp::Create,
            json!({"number": "  ab1 ", "amount": 1}),
            &ctx(&r),
        )
        .unwrap();
        assert_eq!(out.value["number"], "AB1");
        let e = apply(
            &invoice(),
            WriteOp::Create,
            json!({"number": "  ab  ", "amount": 1}),
            &ctx(&r),
        )
        .unwrap_err();
        assert_eq!(codes(e), vec!["number:this.validation.string_length"]);
    }

    #[test]
    fn read_only_and_unknown_fields_rejected() {
        let r = CustomRegistry::default();
        let e = apply(
            &invoice(),
            WriteOp::Create,
            json!({"number": "abc", "amount": 1, "id": "x", "tenant_id": "y", "zzz": 1}),
            &ctx(&r),
        )
        .unwrap_err();
        let c = codes(e);
        assert!(c.contains(&"id:this.validation.read_only".into()), "{c:?}");
        assert!(
            c.contains(&"tenant_id:this.validation.read_only".into()),
            "{c:?}"
        );
        assert!(
            c.contains(&"zzz:this.validation.unknown_field".into()),
            "{c:?}"
        );
    }

    #[test]
    fn base_fields_read_only_on_update() {
        let r = CustomRegistry::default();
        let e = apply(
            &invoice(),
            WriteOp::Update,
            json!({"created_at": "2026-01-01T00:00:00Z"}),
            &ctx(&r),
        )
        .unwrap_err();
        assert_eq!(codes(e), vec!["created_at:this.validation.read_only"]);
        apply(
            &invoice(),
            WriteOp::Update,
            json!({"name": "n", "status": "paid"}),
            &ctx(&r),
        )
        .unwrap();
    }

    #[test]
    fn wrong_type_is_validation_not_internal() {
        let r = CustomRegistry::default();
        let e = apply(
            &invoice(),
            WriteOp::Create,
            json!({"number": "abc", "amount": "12"}),
            &ctx(&r),
        )
        .unwrap_err();
        assert_eq!(e.kind, crate::error::ErrorKind::Validation);
        assert_eq!(codes(e), vec!["amount:this.validation.type_mismatch"]);
    }

    #[test]
    fn date_format_matches_legacy_validator() {
        // Same cases as core/validation/validators.rs::date_format (chrono NaiveDate).
        let r = CustomRegistry::default();
        for (s, ok) in [
            ("2026-10-29", true),
            ("2026-13-01", false),
            ("29/10/2026", false),
            ("2024-02-29", true),
        ] {
            let res = apply(&invoice(), WriteOp::Update, json!({"due": s}), &ctx(&r));
            assert_eq!(res.is_ok(), ok, "{s}");
        }
    }

    #[test]
    fn min_max_keep_integer_precision() {
        let big = (1u64 << 53) + 1;
        let mut e = EntityDesc::new("x");
        e.fields = vec![FieldDesc::new("n", FieldType::U64).validator(
            ValidatorSpec::MaxValue(Number::from(1u64 << 53)),
            Applies::Always,
        )];
        let c = CompiledEntity::new("m", e).unwrap();
        let r = CustomRegistry::default();
        assert!(apply(&c, WriteOp::Create, json!({"n": big}), &ctx(&r)).is_err());
        assert!(apply(&c, WriteOp::Create, json!({"n": 1u64 << 53}), &ctx(&r)).is_ok());
    }

    #[test]
    fn client_profile_defers_server_only_custom() {
        let mut e = EntityDesc::new("x");
        e.fields = vec![FieldDesc::new("iban", FieldType::String).validator(
            ValidatorSpec::Custom(crate::custom::Custom::named("billing.iban_unique")),
            Applies::Always,
        )];
        let c = CompiledEntity::new("m", e).unwrap();
        let mut client = CustomRegistry::default();
        client.declare_server_only_validator("billing.iban_unique");
        let out = apply(
            &c,
            WriteOp::Create,
            json!({"iban": "FR76"}),
            &ValidateCtx {
                registry: &client,
                side: Side::Client,
            },
        )
        .unwrap();
        assert_eq!(out.deferred, vec!["iban:billing.iban_unique"]);
        // On the server, a name without implementation is fail-closed.
        assert!(apply(&c, WriteOp::Create, json!({"iban": "FR76"}), &ctx(&client)).is_err());
        let mut server = CustomRegistry::default();
        server.register_validator(
            "billing.iban_unique",
            CustomScope::ServerOnly,
            Arc::new(|f, _, _| Err(FieldViolation::new(f, "billing.iban.taken"))),
        );
        let e = apply(&c, WriteOp::Create, json!({"iban": "FR76"}), &ctx(&server)).unwrap_err();
        assert_eq!(codes(e), vec!["iban:billing.iban.taken"]);
    }
}
