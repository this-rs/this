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
use crate::runtime::RuntimeDesc;
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
    /// A single-principal host: a local app, a CLI, an embedded runtime. The module is
    /// written by the same principal that runs it, so there is nobody to grant to — a
    /// missing policy keeps the compiled default instead of being refused. Every STRUCTURAL
    /// rule still applies (names, types, references, base fields, reserved plurals, semver):
    /// what disappears is the admission model, not the validation.
    Local { allow_public: bool },
    /// The framework's own descriptor (namespace `this`).
    Framework,
}

/// Root route segments the framework already serves.
///
/// An entity's plural becomes its REST segment (`/invoices`), so a plural landing on one of these
/// either panics axum at mount time (`/health`, `/graph`: same path and method on two routers) or
/// silently shadows a framework route (`/events/{id}` vs `/events/stream`). Refused at build time
/// rather than discovered in production: `this.descriptor.reserved_plural`.
///
/// A host that adds a root route of its own MUST add the segment here in the same commit. Routes
/// added under the reserved `/_this` prefix need no entry.
pub const RESERVED_ROUTE_SEGMENTS: &[&str] = &[
    "_this",         // framework routes, `exposure::descriptor::PREFIX`
    "auth",          // /auth/token, /auth/keys, /auth/refresh, /auth/revoke
    "events",        // /events/stream (SSE)
    "graph",         // /graph, /graph/hierarchy/{id}, /graph/procedures...
    "health",        // /health
    "healthz",       // /healthz
    "notifications", // /notifications/{user_id}...
    "tenants",       // /tenants/{tenant_id}/data
    "ui",            // /ui/state/{portee}
];

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

/// Une heure locale `HH:MM` valide (00:00..23:59).
fn heure_locale_valide(at: &str) -> bool {
    let mut parts = at.split(':');
    let (Some(heures), Some(minutes), None) = (parts.next(), parts.next(), parts.next()) else {
        return false;
    };
    let (Ok(h), Ok(m)) = (heures.parse::<u32>(), minutes.parse::<u32>()) else {
        return false;
    };
    heures.len() == 2 && minutes.len() == 2 && h < 24 && m < 60
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

fn check_runtime(c: &mut Ctx, r: &RuntimeDesc, profile: CheckProfile) {
    if r.entry.trim().is_empty() {
        c.push(
            "runtime_entry",
            "runtime.entry",
            "`entry` must be a non-empty WASM export name",
        );
    }
    if matches!(profile, CheckProfile::Stored { .. }) && r.handles.is_empty() {
        c.push(
            "runtime_handles",
            "runtime.handles",
            "a stored WASM module must declare the handles it requests",
        );
    }
    let mut seen = BTreeSet::new();
    for h in &r.handles {
        if !seen.insert(h.to_string()) {
            c.push(
                "runtime_handles",
                "runtime.handles",
                format!("duplicate handle `{h}`"),
            );
        }
    }
    if r.limits.memory_bytes == Some(0) {
        c.push(
            "runtime_limits",
            "runtime.limits.memory_bytes",
            "`memory_bytes` must be > 0",
        );
    }
    if r.limits.cpu_time_ms == Some(0) {
        c.push(
            "runtime_limits",
            "runtime.limits.cpu_time_ms",
            "`cpu_time_ms` must be > 0",
        );
    }
}

impl ModuleDescriptor {
    /// Verify this descriptor. Fail-closed: anything undeclared or unregistered is an issue.
    pub fn check(
        &self,
        registry: &CustomRegistry,
        profile: CheckProfile,
    ) -> Result<(), Vec<DescriptorIssue>> {
        self.check_with_required(registry, profile, &[])
    }

    /// Same verdict as [`check`](Self::check), but the entities the views and actions may
    /// name also include those of the modules this one requires (transitively), looked up in
    /// `known`. `known` may hold every module of the host, `self` included: only the
    /// transitive `requires` are read. A module citing an entity of a module it does NOT
    /// require is still refused (`view_entity_unknown`, `action_entity_unknown`...).
    ///
    /// It is also what lets a view replace (`replaces`) a view of a module this one requires
    /// **directly**: same surface, a name only one required module declares. Without the
    /// required module in `known` (and so with [`check`](Self::check)) that `replaces` is
    /// refused, never accepted on trust. See [`crate::views::issues_with_required`] for the
    /// refusals and for what the contract cannot decide (two unrelated modules replacing the
    /// same view: the host arbitrates).
    /// `check` is `check_with_required(.., &[])`.
    pub fn check_with_required(
        &self,
        registry: &CustomRegistry,
        profile: CheckProfile,
        known: &[&ModuleDescriptor],
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
        if let Some(r) = &self.runtime {
            check_runtime(&mut c, r, profile);
        }
        if let Some(p) = &self.perm {
            for op in p.ops.keys() {
                if let Operation::Custom(name) = op {
                    let ok = name.contains('.') && name.split('.').all(is_identifier);
                    if !ok {
                        c.push(
                            "perm_operation",
                            format!("perm.{name}"),
                            format!(
                                "business operation `{name}` must be a non-empty dotted name like `invoice.pay`"
                            ),
                        );
                    }
                }
            }
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

        // Vues et actions : entités, champs, cibles, remplacements. Sans ces règles, l'hôte
        // dessinerait une liste vide ou un bouton mort, et la personne conclurait à une panne
        // au lieu d'un oubli. Le détail est dans `views::issues`.
        for (reason, path, message) in crate::views::issues_with_required(self, known) {
            c.push(reason, path, message);
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
            } else {
                if RESERVED_ROUTE_SEGMENTS.contains(&e.plural.as_str()) {
                    let plural = &e.plural;
                    c.push(
                        "reserved_plural",
                        &p,
                        format!("plural `{plural}` is already served by the framework at the root: `/{plural}`"),
                    );
                }
                if !plurals.insert(&e.plural) {
                    c.push(
                        "duplicate_plural",
                        &p,
                        format!("plural `{}` used twice", e.plural),
                    );
                }
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

        // jobs
        c.issues.extend(self.check_jobs(known));

        if c.issues.is_empty() {
            Ok(())
        } else {
            Err(c.issues)
        }
    }

    /// Verify ONLY the `jobs` section — the refusals a host can enforce without running
    /// the whole admission check: `job_name_duplicate`, `job_program_unknown`,
    /// `job_schedule_missing`, `job_schedule_conflict`, `job_needs_unknown`.
    ///
    /// `known` = the descriptors of the modules this one requires (transitively), so a
    /// `needs` entry may name their entities too; `&[]` means local entities only.
    pub fn check_jobs(&self, known: &[&ModuleDescriptor]) -> Vec<DescriptorIssue> {
        let mut issues = Vec::new();
        let mut noms = BTreeSet::new();
        let mut connues: BTreeSet<&str> = self.entities.iter().map(|e| e.name.as_str()).collect();
        for dep in crate::views::required_modules(self, known) {
            for e in &dep.entities {
                connues.insert(e.name.as_str());
            }
        }
        for (i, job) in self.jobs.iter().enumerate() {
            let p = format!("jobs[{i}]");
            let mut push = |raison: &str, chemin: &str, message: String| {
                issues.push(DescriptorIssue {
                    code: ErrorCode::new(format!("this.descriptor.{raison}")),
                    path: chemin.to_string(),
                    message,
                });
            };
            if !is_identifier(&job.name) {
                push(
                    "invalid_name",
                    &format!("{p}.name"),
                    format!("`{}`", job.name),
                );
            }
            if !noms.insert(&job.name) {
                push(
                    "job_name_duplicate",
                    &p,
                    format!("`{}` is declared twice", job.name),
                );
            }
            if !is_identifier(&job.program) {
                push(
                    "invalid_name",
                    &format!("{p}.program"),
                    format!("`{}`", job.program),
                );
            } else if !self.programs.iter().any(|pr| pr == &job.program) {
                push(
                    "job_program_unknown",
                    &p,
                    format!("`{}` is not a declared program", job.program),
                );
            }
            match (job.every_secs, job.at.as_deref()) {
                (None, None) => push(
                    "job_schedule_missing",
                    &p,
                    "declare `every_secs` or `at`".to_string(),
                ),
                (Some(_), Some(_)) => push(
                    "job_schedule_conflict",
                    &p,
                    "`every_secs` and `at` are mutually exclusive".to_string(),
                ),
                (Some(0), None) => push(
                    "job_schedule_missing",
                    &p,
                    "`every_secs` must be > 0".to_string(),
                ),
                (None, Some(at)) if !heure_locale_valide(at) => push(
                    "job_schedule_missing",
                    &p,
                    "`at` must be `HH:MM` (00:00..23:59)".to_string(),
                ),
                _ => {}
            }
            for besoin in &job.needs {
                if !connues.contains(besoin.as_str()) {
                    push(
                        "job_needs_unknown",
                        &p,
                        format!("`{besoin}` is neither local nor in a required module"),
                    );
                }
            }
        }
        issues
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
    // `public` reste refusé dans les deux régimes (un hôte local n'a pas de surface anonyme
    // non plus) ; c'est l'exigence de politique EXPLICITE qui n'a de sens que pour un module
    // déposé par un tiers.
    if let CheckProfile::Stored { allow_public } | CheckProfile::Local { allow_public } = profile {
        if entity && matches!(profile, CheckProfile::Stored { .. }) {
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
        if !allow_public
            && perms
                .ops
                .values()
                .any(|p| p.mentions_public() || p.may_admit_anonymous())
        {
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
    use crate::module::JobDesc;
    use crate::perm::PolicyExpr;
    use crate::runtime::{Handle, RuntimeDesc, RuntimeLimits};

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

    /// Le même descripteur, deux régimes. Un module DÉPOSÉ doit déclarer ses politiques ;
    /// une app à principal unique n'a personne à qui accorder, et n'a donc pas à les
    /// inventer. Les règles structurelles, elles, ne bougent pas d'un iota.
    #[test]
    fn un_hote_a_principal_unique_n_exige_pas_de_politique() {
        let sans_politique = r#"{"format":1,"name":"taches","version":"0.1.0",
            "entities":[{"name":"tache","plural":"taches","kind":"data",
              "fields":[{"name":"titre","ty":"string"}]}],
            "links":[]}"#;
        let m = ModuleDescriptor::from_json(sans_politique).expect("descripteur valide");

        let codes = codes_of(m.check(
            &reg(),
            CheckProfile::Stored {
                allow_public: false,
            },
        ));
        assert!(
            codes.iter().any(|c| c == "this.descriptor.policy_missing"),
            "un module déposé doit déclarer ses politiques : {codes:?}"
        );

        m.check(
            &reg(),
            CheckProfile::Local {
                allow_public: false,
            },
        )
        .expect("une app locale n'a pas de tiers à qui accorder");

        // `public` reste refusé : un hôte local n'a pas de surface anonyme non plus.
        let avec_public = sans_politique.replace(
            r#""kind":"data""#,
            r#""kind":"data","permissions":{"list":"public"}"#,
        );
        let m = ModuleDescriptor::from_json(&avec_public).expect("descripteur valide");
        let codes = codes_of(m.check(
            &reg(),
            CheckProfile::Local {
                allow_public: false,
            },
        ));
        assert!(
            codes.iter().any(|c| c == "this.descriptor.public_refused"),
            "public doit rester refusé en local : {codes:?}"
        );
    }

    #[test]
    fn perm_block_is_read_from_json() {
        let json = r#"{"format":1,"name":"billing","version":"1.0.0",
            "perm":{"invoice.pay":"authenticated","list":"authenticated"}}"#;
        let m = ModuleDescriptor::from_json(json).expect("descripteur valide");
        let perm = m.perm.as_ref().expect("perm present");
        assert_eq!(
            perm.ops.get(&Operation::Custom("invoice.pay".into())),
            Some(&crate::perm::PolicyExpr::Authenticated)
        );
        assert_eq!(perm.ops.len(), 2);
        assert!(m.check(&reg(), CheckProfile::Compiled).is_ok());
        let back = ModuleDescriptor::from_json(&m.to_json_pretty()).unwrap();
        assert_eq!(back, m);
    }

    #[test]
    fn descriptor_without_perm_reads_as_before() {
        let json = r#"{"format":1,"name":"billing","version":"1.0.0"}"#;
        let m = ModuleDescriptor::from_json(json).unwrap();
        assert_eq!(m.perm, None);
        assert!(!m.to_json_pretty().contains("perm"));
    }

    #[test]
    fn malformed_custom_operation_in_perm_is_refused() {
        for bad in ["", "pay", "invoice..pay", "Invoice.pay", ".pay"] {
            let mut m = ModuleDescriptor::new("billing", "1.0.0");
            let mut p = PermDesc::default();
            p.ops
                .insert(Operation::Custom(bad.into()), crate::perm::PolicyExpr::Deny);
            m.perm = Some(p);
            let codes = codes_of(m.check(&reg(), CheckProfile::Compiled));
            assert!(
                codes.iter().any(|c| c == "this.descriptor.perm_operation"),
                "`{bad}` should be refused, got {codes:?}"
            );
        }
        // And a malformed name in JSON never even deserializes.
        let json = r#"{"format":1,"name":"billing","version":"1.0.0","perm":{"pay":"deny"}}"#;
        assert!(ModuleDescriptor::from_json(json).is_err());
    }

    #[test]
    fn runtime_section_roundtrip_and_check() {
        let ok = ModuleDescriptor::new("billing", "1.0.0");
        assert!(ok.check(&reg(), CheckProfile::Compiled).is_ok());

        let mut d = ModuleDescriptor::new("billing", "1.0.0");
        d.runtime = Some(RuntimeDesc {
            entry: "run".into(),
            handles: vec![Handle::Graph, Handle::Inference],
            limits: RuntimeLimits {
                memory_bytes: Some(1 << 20),
                cpu_time_ms: Some(50),
                network: false,
            },
        });
        assert!(d.check(&reg(), CheckProfile::Compiled).is_ok());

        // JSON round-trip carries the runtime section as typed data, not verbatim.
        let s = d.to_json_pretty();
        let back = ModuleDescriptor::from_json(&s).unwrap();
        assert_eq!(back, d);

        // A stored module must declare its handles explicitly.
        let mut no_handles = d.clone();
        no_handles.runtime = Some(RuntimeDesc {
            entry: "run".into(),
            handles: vec![],
            limits: Default::default(),
        });
        let codes = codes_of(no_handles.check(
            &reg(),
            CheckProfile::Stored {
                allow_public: false,
            },
        ));
        assert!(codes.iter().any(|c| c == "this.descriptor.runtime_handles"));

        // An empty entry is refused.
        let mut no_entry = d;
        no_entry.runtime.as_mut().unwrap().entry = String::new();
        let codes = codes_of(no_entry.check(&reg(), CheckProfile::Compiled));
        assert!(codes.iter().any(|c| c == "this.descriptor.runtime_entry"));
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
    fn framework_route_segments_are_reserved_as_plurals() {
        // Every segment the framework serves at the root is refused as a plural, however it got
        // there: derived by `normalize` or written in YAML.
        for segment in RESERVED_ROUTE_SEGMENTS {
            let mut m = ModuleDescriptor::new("billing", "1.0.0");
            let mut e = EntityDesc::new("thing");
            e.plural = (*segment).to_string();
            m.entities.push(e);
            assert!(
                codes_of(m.check(&reg(), CheckProfile::Compiled))
                    .contains(&"this.descriptor.reserved_plural".into()),
                "`{segment}` accepted as a plural"
            );
        }
        // Derived, no YAML involved: `notification` -> `notifications`.
        let mut m = ModuleDescriptor::new("billing", "1.0.0");
        m.entities.push(EntityDesc::new("notification"));
        assert!(
            codes_of(m.check(&reg(), CheckProfile::Compiled))
                .contains(&"this.descriptor.reserved_plural".into())
        );
        // An ordinary plural is untouched, and the reserved check does not mask duplicates.
        module_with(vec![])
            .check(&reg(), CheckProfile::Compiled)
            .unwrap();
        let mut dup = ModuleDescriptor::new("billing", "1.0.0");
        dup.entities.push(EntityDesc::new("invoice"));
        dup.entities.push(EntityDesc::new("invoice"));
        assert!(
            codes_of(dup.check(&reg(), CheckProfile::Compiled))
                .contains(&"this.descriptor.duplicate_entity".into())
        );
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
                m.entities[0]
                    .permissions
                    .ops
                    .insert(op, PolicyExpr::parse(pol).unwrap());
            }
            assert!(
                codes_of(m.check(
                    &reg(),
                    CheckProfile::Stored {
                        allow_public: false
                    }
                ))
                .contains(&"this.descriptor.public_refused".into()),
                "`{pol}` admits anonymous callers: must be refused like `public`"
            );
        }
        // Guarded negations stay legal.
        for pol in [
            "all:authenticated,not:role:banned",
            "role:admin",
            "authenticated",
            "owner",
        ] {
            let mut m = module_with(vec![]);
            for op in Operation::CRUD {
                m.entities[0]
                    .permissions
                    .ops
                    .insert(op, PolicyExpr::parse(pol).unwrap());
            }
            let c = codes_of(m.check(
                &reg(),
                CheckProfile::Stored {
                    allow_public: false,
                },
            ));
            assert!(
                !c.contains(&"this.descriptor.public_refused".into()),
                "`{pol}` is guarded: {c:?}"
            );
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

    // ---------- check_with_required ----------

    fn requiring(name: &str, requires: &[&str], entity: Option<&str>) -> ModuleDescriptor {
        let mut m = ModuleDescriptor::new(name, "1.0.0");
        m.requires = requires
            .iter()
            .map(|n| crate::module::ModuleReq {
                name: n.to_string(),
                version: "*".into(),
                optional: false,
            })
            .collect();
        if let Some(entity) = entity {
            m.views.push(
                serde_json::from_value(serde_json::json!({
                    "name": "liste",
                    "surface": "main",
                    "root": {"type": "leaf", "brick": {"type": "list", "entity": entity, "label": "titre"}}
                }))
                .unwrap(),
            );
        }
        m
    }

    fn owner_of(name: &str, requires: &[&str], entity: &str) -> ModuleDescriptor {
        let mut m = requiring(name, requires, None);
        let mut e = EntityDesc::new(entity);
        e.fields = vec![FieldDesc::new("titre", crate::field::FieldType::String).tag(16)];
        m.entities.push(e);
        m
    }

    #[test]
    fn check_alone_still_refuses_the_entity_of_a_required_module() {
        let menu = requiring("menu", &["socle"], Some("conversation"));
        assert_eq!(
            codes_of(menu.check(&reg(), CheckProfile::Compiled)),
            vec!["this.descriptor.view_entity_unknown"]
        );
    }

    #[test]
    fn check_with_empty_known_is_check() {
        let menu = requiring("menu", &["socle"], Some("conversation"));
        assert_eq!(
            codes_of(menu.check_with_required(&reg(), CheckProfile::Compiled, &[])),
            codes_of(menu.check(&reg(), CheckProfile::Compiled)),
        );
    }

    #[test]
    fn check_with_required_accepts_the_entity_of_a_required_module() {
        let socle = owner_of("socle", &[], "conversation");
        let menu = requiring("menu", &["socle"], Some("conversation"));
        assert_eq!(
            codes_of(menu.check_with_required(&reg(), CheckProfile::Compiled, &[&socle, &menu])),
            Vec::<String>::new()
        );
    }

    #[test]
    fn check_with_required_follows_requires_transitively() {
        let socle = owner_of("socle", &[], "conversation");
        let milieu = requiring("milieu", &["socle"], None);
        let menu = requiring("menu", &["milieu"], Some("conversation"));
        assert_eq!(
            codes_of(menu.check_with_required(
                &reg(),
                CheckProfile::Compiled,
                &[&socle, &milieu, &menu]
            )),
            Vec::<String>::new()
        );
    }

    #[test]
    fn check_with_required_still_refuses_the_entity_of_a_module_not_required() {
        let socle = owner_of("socle", &[], "conversation");
        let menu = requiring("menu", &[], Some("conversation"));
        assert_eq!(
            codes_of(menu.check_with_required(&reg(), CheckProfile::Compiled, &[&socle, &menu])),
            vec!["this.descriptor.view_entity_unknown"]
        );
    }

    // ---------- jobs ----------

    fn avec_jobs(jobs: Vec<JobDesc>) -> ModuleDescriptor {
        let mut m = ModuleDescriptor::new("rappels", "1.0.0");
        m.programs = vec!["rappels".to_string()];
        m.jobs = jobs;
        m
    }

    fn job(program: &str) -> JobDesc {
        let mut j = JobDesc::new("relancer", program);
        j.every_secs = Some(3600);
        j
    }

    #[test]
    fn un_job_valide_passe() {
        let m = avec_jobs(vec![job("rappels")]);
        assert_eq!(
            codes_of(m.check(&reg(), CheckProfile::Compiled)),
            Vec::<String>::new()
        );
    }

    #[test]
    fn un_descripteur_avec_jobs_se_lit_et_se_valide_depuis_le_json() {
        let json = r#"{"format":1,"name":"rappels","version":"1.0.0",
            "programs":["rappels"],
            "jobs":[{"name":"relancer","program":"rappels","every_secs":3600}]}"#;
        let m = ModuleDescriptor::from_json(json).expect("descripteur valide");
        assert_eq!(m.programs, vec!["rappels".to_string()]);
        assert_eq!(m.jobs.len(), 1);
        assert_eq!(m.jobs[0].every_secs, Some(3600));
        assert_eq!(
            codes_of(m.check(&reg(), CheckProfile::Compiled)),
            Vec::<String>::new()
        );
    }

    #[test]
    fn job_name_duplicate_est_refuse() {
        let m = avec_jobs(vec![job("rappels"), job("rappels")]);
        assert_eq!(
            codes_of(m.check(&reg(), CheckProfile::Compiled)),
            vec!["this.descriptor.job_name_duplicate"]
        );
    }

    #[test]
    fn job_program_unknown_est_refuse() {
        let m = avec_jobs(vec![job("inconnu")]);
        assert_eq!(
            codes_of(m.check(&reg(), CheckProfile::Compiled)),
            vec!["this.descriptor.job_program_unknown"]
        );
    }

    #[test]
    fn job_schedule_missing_est_refuse() {
        let mut j = JobDesc::new("relancer", "rappels");
        j.every_secs = None;
        j.at = None;
        let m = avec_jobs(vec![j]);
        assert_eq!(
            codes_of(m.check(&reg(), CheckProfile::Compiled)),
            vec!["this.descriptor.job_schedule_missing"]
        );
    }

    #[test]
    fn job_schedule_conflict_est_refuse() {
        let mut j = JobDesc::new("relancer", "rappels");
        j.every_secs = Some(3600);
        j.at = Some("07:30".to_string());
        let m = avec_jobs(vec![j]);
        assert_eq!(
            codes_of(m.check(&reg(), CheckProfile::Compiled)),
            vec!["this.descriptor.job_schedule_conflict"]
        );
    }

    #[test]
    fn job_needs_unknown_est_refuse() {
        let mut j = JobDesc::new("relancer", "rappels");
        j.every_secs = Some(3600);
        j.needs = vec!["absent".to_string()];
        let m = avec_jobs(vec![j]);
        assert_eq!(
            codes_of(m.check(&reg(), CheckProfile::Compiled)),
            vec!["this.descriptor.job_needs_unknown"]
        );
    }

    #[test]
    fn job_at_valide_passe() {
        let mut j = JobDesc::new("relancer", "rappels");
        j.at = Some("07:30".to_string());
        let m = avec_jobs(vec![j]);
        assert_eq!(
            codes_of(m.check(&reg(), CheckProfile::Compiled)),
            Vec::<String>::new()
        );
    }

    #[test]
    fn job_at_mal_forme_est_refuse() {
        let mut j = JobDesc::new("relancer", "rappels");
        j.at = Some("7:30".to_string());
        let m = avec_jobs(vec![j]);
        assert_eq!(
            codes_of(m.check(&reg(), CheckProfile::Compiled)),
            vec!["this.descriptor.job_schedule_missing"]
        );
    }
}
