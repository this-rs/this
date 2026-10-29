//! `ModuleDescriptor::check`: everything that can be verified on ONE descriptor.
//!
//! Cross-module verifications (references to required modules, duplicates between
//! modules, versions) belong to `resolve` and `DescriptorSet::insert`.

use crate::custom::{CustomKind, CustomRegistry};
use crate::entity::{EntityKind, FIRST_USER_TAG, TENANT_ID, TENANT_ID_TAG, base_fields};
use crate::error::{ContractError, ErrorCode};
use crate::field::{Applies, FieldDesc, FieldType, TransformSpec, ValidatorSpec};
use crate::framework::{FRAMEWORK_MODULE, codes};
use crate::i18n::MessageSyntax;
use crate::module::{DESCRIPTOR_FORMAT, ModuleDescriptor};
use crate::naming::is_identifier;
use crate::perm::{Operation, PermDesc};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::fmt;

/// One problem found by `check()`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DescriptorIssue {
    /// `this.descriptor.<reason>`.
    pub code: ErrorCode,
    /// Where: `entities.invoice.fields.amount`.
    pub path: String,
    pub message: String,
}

impl fmt::Display for DescriptorIssue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} at {}: {}", self.code, self.path, self.message)
    }
}

/// Origin of the descriptor, which decides how strict the check is.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum CheckProfile {
    /// Compiled module (`Module::descriptor()` / macro): missing policies default to
    /// `authenticated` for CRUD (backward compatibility), `deny` for business operations.
    #[default]
    Compiled,
    /// Uploaded / stored module (`ModuleStore`): every CRUD operation of every entity needs
    /// an EXPLICIT policy, and `public` is refused unless `allow_public` is set by the host.
    Stored { allow_public: bool },
    /// The framework's own descriptor (namespace `this`).
    Framework,
}

/// Turn issues into one `this.descriptor.invalid` error.
pub fn issues_to_error(issues: &[DescriptorIssue]) -> ContractError {
    ContractError::framework(codes::DESCRIPTOR_INVALID).with_param(
        "issues",
        issues
            .iter()
            .map(|i| i.to_string())
            .collect::<Vec<_>>()
            .join("; "),
    )
}

struct Ctx<'a> {
    issues: Vec<DescriptorIssue>,
    reg: &'a CustomRegistry,
}

impl Ctx<'_> {
    fn push(&mut self, reason: &str, path: impl Into<String>, message: impl Into<String>) {
        self.issues.push(DescriptorIssue {
            code: ErrorCode::new(format!("this.descriptor.{reason}")),
            path: path.into(),
            message: message.into(),
        });
    }
    fn custom(&mut self, kind: CustomKind, name: &str, path: &str) {
        if !self.reg.knows(kind, name) {
            self.push(
                "custom_unknown",
                path,
                format!("{kind:?} `{name}` is not registered"),
            );
        }
    }
}

impl ModuleDescriptor {
    /// Verify this descriptor. Fail-closed: anything undeclared or unregistered is an issue.
    pub fn check(
        &self,
        registry: &CustomRegistry,
        profile: CheckProfile,
    ) -> Result<(), Vec<DescriptorIssue>> {
        let mut c = Ctx {
            issues: vec![],
            reg: registry,
        };
        let ns = format!("{}.", self.name);

        if self.format > DESCRIPTOR_FORMAT {
            c.push(
                "format_unsupported",
                "format",
                format!("{} > {DESCRIPTOR_FORMAT}", self.format),
            );
        }
        if self.runtime.is_some() {
            c.issues.push(DescriptorIssue {
                code: ErrorCode::new(crate::framework::codes::MODULE_FORMAT_UNSUPPORTED),
                path: "runtime".into(),
                message: "the `runtime` section is reserved (not supported yet)".into(),
            });
        }
        if !is_identifier(&self.name) {
            c.push(
                "invalid_name",
                "name",
                format!("`{}` is not ^[a-z][a-z0-9_]*$", self.name),
            );
        }
        if self.name == FRAMEWORK_MODULE && profile != CheckProfile::Framework {
            c.push(
                "reserved_namespace",
                "name",
                "`this` is reserved for the framework",
            );
        }
        if semver::Version::parse(&self.version).is_err() {
            c.push(
                "invalid_version",
                "version",
                format!("`{}` is not semver", self.version),
            );
        }

        // requires
        let mut seen = BTreeSet::new();
        for (i, r) in self.requires.iter().enumerate() {
            let p = format!("requires[{i}]");
            if !is_identifier(&r.name) {
                c.push("invalid_name", &p, format!("`{}`", r.name));
            }
            if r.name == self.name {
                c.push("self_dependency", &p, "a module cannot require itself");
            }
            if !seen.insert(&r.name) {
                c.push(
                    "duplicate_requirement",
                    &p,
                    format!("`{}` required twice", r.name),
                );
            }
            if semver::VersionReq::parse(&r.version).is_err() {
                c.push("invalid_version_req", &p, format!("`{}`", r.version));
            }
        }

        // entities
        let mut names = BTreeSet::new();
        let mut plurals = BTreeSet::new();
        let local: BTreeSet<&str> = self.entities.iter().map(|e| e.name.as_str()).collect();
        for e in &self.entities {
            let p = format!("entities.{}", e.name);
            if !is_identifier(&e.name) {
                c.push("invalid_name", &p, format!("`{}`", e.name));
            }
            if !names.insert(&e.name) {
                c.push("duplicate_entity", &p, "declared twice");
            }
            if e.plural.is_empty() {
                c.push("plural_missing", &p, "plural is empty (call normalize)");
            } else if !plurals.insert(&e.plural) {
                c.push(
                    "duplicate_plural",
                    &p,
                    format!("plural `{}` used twice", e.plural),
                );
            }
            check_fields(&mut c, &p, e.kind, &e.fields, &e.reserved_tags, true);
            for idx in &e.indexed {
                if e.any_field(idx).is_none() {
                    c.push("unknown_field", format!("{p}.indexed"), format!("`{idx}`"));
                }
            }
            for cap in &e.custom {
                c.custom(CustomKind::Capability, &cap.name, &format!("{p}.custom"));
            }
            check_perms(
                &mut c,
                &format!("{p}.permissions"),
                &e.permissions,
                profile,
                true,
            );
            for key in [&e.i18n.label, &e.i18n.label_plural].into_iter().flatten() {
                check_key(&mut c, &ns, &key.0, &format!("{p}.i18n"), profile);
            }
        }

        // links
        for (i, l) in self.links.iter().enumerate() {
            let p = format!("links[{i}]");
            if !is_identifier(&l.link_type) {
                c.push("invalid_name", &p, format!("link_type `{}`", l.link_type));
            }
            for end in [&l.source, &l.target] {
                if !local.contains(end.as_str()) && self.requires.is_empty() {
                    c.push(
                        "reference_unknown",
                        &p,
                        format!("`{end}` is neither local nor in a required module"),
                    );
                }
            }
            check_fields(&mut c, &p, EntityKind::Link, &l.fields, &[], false);
            check_perms(
                &mut c,
                &format!("{p}.permissions"),
                &l.permissions,
                profile,
                false,
            );
        }

        // errors
        let mut codes_seen = BTreeSet::new();
        for (i, err) in self.errors.iter().enumerate() {
            let p = format!("errors[{i}]");
            if !err.code.0.starts_with(&ns) {
                c.push(
                    "code_namespace",
                    &p,
                    format!("`{}` must start with `{ns}`", err.code),
                );
            }
            if !codes_seen.insert(&err.code) {
                c.push("duplicate_error_code", &p, format!("`{}`", err.code));
            }
            check_key(&mut c, &ns, &err.message_key.0, &p, profile);
        }

        // i18n
        for (i, k) in self.i18n.keys.iter().enumerate() {
            check_key(&mut c, &ns, &k.key.0, &format!("i18n.keys[{i}]"), profile);
        }
        for cap in &self.custom {
            c.custom(CustomKind::Capability, &cap.name, "custom");
        }

        if c.issues.is_empty() {
            Ok(())
        } else {
            Err(c.issues)
        }
    }

    /// Reject Fluent bundles until an engine renders them.
    pub fn check_bundle_syntax(syntax: MessageSyntax) -> Result<(), DescriptorIssue> {
        match syntax {
            MessageSyntax::Simple => Ok(()),
            _ => Err(DescriptorIssue {
                code: ErrorCode::new("this.descriptor.syntax_unsupported"),
                path: "i18n".into(),
                message: "only the `simple` syntax is rendered today".into(),
            }),
        }
    }
}

fn check_key(c: &mut Ctx, ns: &str, key: &str, path: &str, profile: CheckProfile) {
    let ok =
        key.starts_with(ns) || (profile == CheckProfile::Framework && key.starts_with("this."));
    if !ok {
        c.push(
            "key_namespace",
            path,
            format!("message key `{key}` must start with `{ns}`"),
        );
    }
}

fn check_perms(c: &mut Ctx, path: &str, perms: &PermDesc, profile: CheckProfile, entity: bool) {
    if let CheckProfile::Stored { allow_public } = profile {
        if entity {
            for op in Operation::CRUD {
                if perms.policy(&op).is_none() {
                    c.push(
                        "policy_missing",
                        path,
                        format!("stored module: `{op}` needs an explicit policy"),
                    );
                }
            }
        }
        if !allow_public && perms.ops.values().any(|p| p.mentions_public() || p.may_admit_anonymous()) {
            c.push(
                "public_refused",
                path,
                "stored module: `public` needs the host option allow_public",
            );
        }
    }
}

fn check_fields(
    c: &mut Ctx,
    path: &str,
    kind: EntityKind,
    fields: &[FieldDesc],
    reserved: &[u32],
    is_entity: bool,
) {
    let mut names = BTreeSet::new();
    let mut tags = BTreeSet::new();
    for f in fields {
        let p = format!("{path}.fields.{}", f.name);
        if !is_identifier(&f.name) {
            c.push("invalid_name", &p, format!("`{}`", f.name));
        }
        if !names.insert(&f.name) {
            c.push("duplicate_field", &p, "declared twice");
        }
        let overlay = base_fields(kind)
            .iter()
            .find(|b| is_entity && b.name == f.name);
        if let Some(b) = overlay {
            // Overlay of a WRITABLE base field (`name`, `status`) to carry rules: allowed if
            // the type and write mode are unchanged.
            if b.read_only || b.immutable || b.ty != f.ty || b.read_only != f.read_only {
                c.push("base_field_redeclared", &p, "base fields are implicit (see base_fields); only name/status may be overlaid, same type");
            }
        }
        if let Some(tag) = f.proto_tag {
            let tenant = f.name == TENANT_ID && tag == TENANT_ID_TAG;
            // An overlay of a base field keeps the base field's reserved tag.
            let base_tag = overlay.and_then(|b| b.proto_tag);
            if let Some(b) = base_tag
                && b != tag
            {
                // Same field on the wire: a different number would split it in two.
                c.push(
                    "tag_reserved",
                    &p,
                    format!("overlay of base field `{}` must keep tag {b}", f.name),
                );
            } else if tag < FIRST_USER_TAG && !tenant && base_tag != Some(tag) {
                c.push(
                    "tag_reserved",
                    &p,
                    format!("tag {tag} < {FIRST_USER_TAG} is reserved"),
                );
            }
            if let Some(reason) = crate::derive::proto::invalid_tag_reason(tag) {
                c.push("tag_out_of_range", &p, format!("tag {tag} {reason}"));
            }
            if !tags.insert(tag) {
                c.push("duplicate_tag", &p, format!("tag {tag} used twice"));
            }
            if reserved.contains(&tag) {
                c.push("tag_reserved", &p, format!("tag {tag} is in reserved_tags"));
            }
        }
        if f.name == TENANT_ID && !f.read_only {
            c.push(
                "tenant_id_writable",
                &p,
                "tenant_id is set by the server: declare it read_only",
            );
        }
        if f.read_only && f.immutable {
            c.push("contradiction", &p, "read_only and immutable");
        }
        if let Some(d) = &f.default
            && !d.is_null()
            && !f.ty.accepts(d)
        {
            c.push(
                "default_type",
                &p,
                format!("default {d} is not a {}", f.ty.label()),
            );
        }
        check_type(c, &p, &f.ty);
        for r in &f.validators {
            check_validator(c, &p, f, &r.rule, r.on);
        }
        for t in &f.transforms {
            match &t.rule {
                TransformSpec::Custom(cu) => c.custom(CustomKind::Transform, &cu.name, &p),
                TransformSpec::RoundDecimals(_) if !f.ty.is_numeric() => {
                    c.push("validator_type", &p, "round_decimals needs a numeric field")
                }
                TransformSpec::Trim | TransformSpec::Uppercase | TransformSpec::Lowercase
                    if !f.ty.is_textual() =>
                {
                    c.push(
                        "validator_type",
                        &p,
                        "string transform on a non-string field",
                    )
                }
                _ => {}
            }
        }
    }
}

fn check_type(c: &mut Ctx, p: &str, ty: &FieldType) {
    match ty {
        FieldType::Custom(cu) => c.custom(CustomKind::FieldType, &cu.name, p),
        FieldType::List(inner) => check_type(c, p, inner),
        FieldType::Enum { variants, .. } if variants.is_empty() => {
            c.push("enum_empty", p, "enum without variants")
        }
        _ => {}
    }
}

fn check_validator(c: &mut Ctx, p: &str, f: &FieldDesc, v: &ValidatorSpec, on: Applies) {
    match v {
        ValidatorSpec::Required if f.optional && on == Applies::Always => c.push(
            "contradiction",
            p,
            "optional field with `required` on every operation",
        ),
        ValidatorSpec::Optional if !f.optional && on == Applies::Always && f.default.is_none() => c
            .push(
                "contradiction",
                p,
                "non-optional field with `optional` on every operation: declare it optional",
            ),
        ValidatorSpec::Positive | ValidatorSpec::MinValue(_) | ValidatorSpec::MaxValue(_)
            if !f.ty.is_numeric() =>
        {
            c.push(
                "validator_type",
                p,
                format!("{} needs a numeric field", v.name()),
            )
        }
        ValidatorSpec::MinValue(n) | ValidatorSpec::MaxValue(n)
            if matches!(
                f.ty,
                FieldType::I32 | FieldType::I64 | FieldType::U32 | FieldType::U64
            ) && n.as_i64().is_none()
                && n.as_u64().is_none() =>
        {
            c.push(
                "validator_type",
                p,
                format!("bound {n} is not an integer for an integer field"),
            )
        }
        ValidatorSpec::StringLength { min, max } => {
            if !f.ty.is_textual() {
                c.push("validator_type", p, "string_length needs a string field");
            }
            if min > max {
                c.push("validator_args", p, format!("min {min} > max {max}"));
            }
        }
        ValidatorSpec::DateFormat(_) if !matches!(f.ty, FieldType::String | FieldType::Date) => c
            .push(
                "validator_type",
                p,
                "date_format needs a string or date field",
            ),
        ValidatorSpec::Email | ValidatorSpec::Url if !f.ty.is_textual() => c.push(
            "validator_type",
            p,
            format!("{} needs a string field", v.name()),
        ),
        ValidatorSpec::Pattern(pat) => {
            #[cfg(feature = "pattern")]
            if let Err(e) = regex::Regex::new(pat) {
                c.push("pattern_invalid", p, e.to_string());
            }
            #[cfg(not(feature = "pattern"))]
            c.push(
                "pattern_unsupported",
                p,
                format!("pattern `{pat}` needs the `pattern` feature of this-descriptor"),
            );
        }
        ValidatorSpec::Custom(cu) => c.custom(CustomKind::Validator, &cu.name, p),
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::custom::Custom;
    use crate::entity::EntityDesc;
    use crate::field::FieldDesc;
    use crate::perm::PolicyExpr;

    fn module_with(fields: Vec<FieldDesc>) -> ModuleDescriptor {
        let mut m = ModuleDescriptor::new("billing", "1.0.0");
        let mut e = EntityDesc::new("invoice");
        e.fields = fields;
        m.entities.push(e);
        m
    }
    fn codes_of(r: Result<(), Vec<DescriptorIssue>>) -> Vec<String> {
        r.err()
            .unwrap_or_default()
            .into_iter()
            .map(|i| i.code.0)
            .collect()
    }
    fn reg() -> CustomRegistry {
        CustomRegistry::default()
    }

    #[test]
    fn runtime_section_refused_until_track4() {
        let ok = ModuleDescriptor::new("billing", "1.0.0");
        assert!(ok.check(&reg(), CheckProfile::Compiled).is_ok());
        let parsed = ModuleDescriptor::from_json(
            r#"{"name":"billing","version":"1.0.0","runtime":{"kind":"wasm"}}"#,
        )
        .unwrap();
        assert!(
            parsed.runtime.is_some(),
            "runtime must be carried, not dropped"
        );
        let codes = codes_of(parsed.check(&reg(), CheckProfile::Compiled));
        assert_eq!(codes, vec!["this.module.format_unsupported".to_string()]);
    }

    #[test]
    fn max_value_integral_bound_is_accepted_on_an_integer_field() {
        use crate::field::{ValidatorRule, ValidatorSpec};
        let with = |ty: FieldType, rule: ValidatorSpec| {
            let mut f = FieldDesc::new("age", ty);
            f.validators.push(ValidatorRule {
                rule,
                on: Applies::Always,
            });
            module_with(vec![f])
        };
        // `max_value(150)` in the macro arrives as f64: it must stay an integer bound.
        for rule in [
            ValidatorSpec::max_value_f64(150.0),
            ValidatorSpec::min_value_f64(-3.0),
        ] {
            with(FieldType::I32, rule)
                .check(&reg(), CheckProfile::Compiled)
                .unwrap();
        }
        // A really fractional bound on an integer field stays refused; on a float field it is fine.
        assert_eq!(
            codes_of(
                with(FieldType::I32, ValidatorSpec::max_value_f64(150.5))
                    .check(&reg(), CheckProfile::Compiled)
            ),
            vec!["this.descriptor.validator_type"]
        );
        with(FieldType::F64, ValidatorSpec::max_value_f64(150.5))
            .check(&reg(), CheckProfile::Compiled)
            .unwrap();
    }

    #[test]
    fn check_accepts_untagged_fields() {
        let m = module_with(vec![FieldDesc::new("amount", FieldType::F64)]);
        m.check(&reg(), CheckProfile::Compiled).unwrap();
    }

    #[test]
    fn check_rejects_duplicate_proto_tag() {
        let m = module_with(vec![
            FieldDesc::new("a", FieldType::F64).tag(16),
            FieldDesc::new("b", FieldType::F64).tag(16),
        ]);
        assert!(
            codes_of(m.check(&reg(), CheckProfile::Compiled))
                .contains(&"this.descriptor.duplicate_tag".into())
        );
    }

    #[test]
    fn check_rejects_unencodable_tag_and_moved_overlay_tag() {
        for t in [19_000, 19_999, 536_870_912] {
            let m = module_with(vec![FieldDesc::new("a", FieldType::F64).tag(t)]);
            assert_eq!(
                codes_of(m.check(&reg(), CheckProfile::Compiled)),
                vec!["this.descriptor.tag_out_of_range"],
                "tag {t}"
            );
        }
        // An overlay of `status` (base tag 6) moved to 16 would be a second field on the wire.
        let m = module_with(vec![FieldDesc::new("status", FieldType::String).tag(16)]);
        assert_eq!(
            codes_of(m.check(&reg(), CheckProfile::Compiled)),
            vec!["this.descriptor.tag_reserved"]
        );
        let ok = module_with(vec![FieldDesc::new("status", FieldType::String).tag(6)]);
        ok.check(&reg(), CheckProfile::Compiled).unwrap();
    }

    #[test]
    fn check_rejects_tag_below_16() {
        let m = module_with(vec![FieldDesc::new("a", FieldType::F64).tag(7)]);
        assert!(
            codes_of(m.check(&reg(), CheckProfile::Compiled))
                .contains(&"this.descriptor.tag_reserved".into())
        );
        // tenant_id may use 8, but must be read-only.
        let t = module_with(vec![FieldDesc::new("tenant_id", FieldType::Uuid).tag(8)]);
        assert_eq!(
            codes_of(t.check(&reg(), CheckProfile::Compiled)),
            vec!["this.descriptor.tenant_id_writable"]
        );
        let ok = module_with(vec![
            FieldDesc::new("tenant_id", FieldType::Uuid)
                .tag(8)
                .read_only(),
        ]);
        ok.check(&reg(), CheckProfile::Compiled).unwrap();
    }

    #[test]
    fn check_rejects_unknown_custom() {
        let m = module_with(vec![FieldDesc::new("a", FieldType::String).validator(
            ValidatorSpec::Custom(Custom::named("billing.iban")),
            Applies::Always,
        )]);
        assert_eq!(
            codes_of(m.check(&reg(), CheckProfile::Compiled)),
            vec!["this.descriptor.custom_unknown"]
        );
        let mut r = reg();
        r.declare_server_only_validator("billing.iban");
        m.check(&r, CheckProfile::Compiled).unwrap();
    }

    #[test]
    fn check_rejects_required_on_optional_field() {
        let m = module_with(vec![
            FieldDesc::new("a", FieldType::String)
                .optional()
                .validator(ValidatorSpec::Required, Applies::Always),
        ]);
        assert_eq!(
            codes_of(m.check(&reg(), CheckProfile::Compiled)),
            vec!["this.descriptor.contradiction"]
        );
    }

    #[test]
    fn check_rejects_validator_on_wrong_type_and_base_field_redeclared() {
        let m = module_with(vec![
            FieldDesc::new("label", FieldType::String)
                .validator(ValidatorSpec::Positive, Applies::Always),
            FieldDesc::new("id", FieldType::Uuid),
        ]);
        let c = codes_of(m.check(&reg(), CheckProfile::Compiled));
        assert!(
            c.contains(&"this.descriptor.validator_type".into()),
            "{c:?}"
        );
        assert!(
            c.contains(&"this.descriptor.base_field_redeclared".into()),
            "{c:?}"
        );
    }

    #[test]
    fn check_enforces_namespaces() {
        let mut m = module_with(vec![]);
        m.errors.push(crate::error::ErrorDesc {
            code: "this.validation.required".into(),
            kind: crate::error::ErrorKind::Validation,
            message_key: "other.x".into(),
            params: vec![],
            description: None,
        });
        let c = codes_of(m.check(&reg(), CheckProfile::Compiled));
        assert!(c.contains(&"this.descriptor.code_namespace".into()));
        assert!(c.contains(&"this.descriptor.key_namespace".into()));
        let mut fw = ModuleDescriptor::new("this", "1.0.0");
        fw.entities.clear();
        assert!(
            codes_of(fw.check(&reg(), CheckProfile::Compiled))
                .contains(&"this.descriptor.reserved_namespace".into())
        );
    }

    #[test]
    fn stored_module_without_permissions_rejected() {
        let m = module_with(vec![]);
        let c = codes_of(m.check(
            &reg(),
            CheckProfile::Stored {
                allow_public: false,
            },
        ));
        assert_eq!(
            c.iter()
                .filter(|x| *x == "this.descriptor.policy_missing")
                .count(),
            5
        );
        let mut m2 = module_with(vec![]);
        for op in Operation::CRUD {
            m2.entities[0]
                .permissions
                .ops
                .insert(op, PolicyExpr::Public);
        }
        assert!(
            codes_of(m2.check(
                &reg(),
                CheckProfile::Stored {
                    allow_public: false
                }
            ))
            .contains(&"this.descriptor.public_refused".into())
        );
        m2.check(&reg(), CheckProfile::Stored { allow_public: true })
            .unwrap();
    }

    /// Review: `public` is refused in a stored module, but a NEGATION admits anonymous callers
    /// just as well (`not:deny`, `not:role:x`, `any:authenticated,not:authenticated`).
    #[test]
    fn stored_module_cannot_admit_anonymous_through_negation() {
        for pol in [
            "not:deny",
            "not:role:banned",
            "not:service_only",
            "not:admin_only",
            "any:authenticated,not:authenticated",
            "not(all(authenticated,role:x))",
            "not:resolver:blocked",
        ] {
            let mut m = module_with(vec![]);
            for op in Operation::CRUD {
                m.entities[0].permissions.ops.insert(op, PolicyExpr::parse(pol).unwrap());
            }
            assert!(
                codes_of(m.check(&reg(), CheckProfile::Stored { allow_public: false }))
                    .contains(&"this.descriptor.public_refused".into()),
                "`{pol}` admits anonymous callers: must be refused like `public`"
            );
        }
        // Guarded negations stay legal.
        for pol in ["all:authenticated,not:role:banned", "role:admin", "authenticated", "owner"] {
            let mut m = module_with(vec![]);
            for op in Operation::CRUD {
                m.entities[0].permissions.ops.insert(op, PolicyExpr::parse(pol).unwrap());
            }
            let c = codes_of(m.check(&reg(), CheckProfile::Stored { allow_public: false }));
            assert!(!c.contains(&"this.descriptor.public_refused".into()), "`{pol}` is guarded: {c:?}");
        }
    }

    #[test]
    fn check_rejects_link_to_unknown_entity_without_requires() {
        let mut m = module_with(vec![]);
        m.links
            .push(crate::link::LinkDesc::new("owner", "user", "invoice"));
        assert_eq!(
            codes_of(m.check(&reg(), CheckProfile::Compiled)),
            vec!["this.descriptor.reference_unknown"]
        );
    }
}
