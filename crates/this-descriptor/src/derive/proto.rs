//! `.proto` derived from an [`EntityDesc`]: numbers come from `proto_tag`, NEVER from the
//! position of a field (in the struct, in the descriptor or in a JSON sample).
//!
//! - Fields are emitted sorted by tag, so reordering declarations changes nothing.
//! - Framework tags 1..=15 not used by the message, and the entity's `reserved_tags`, are
//!   emitted as `reserved`, so a client generator can never reuse them.
//! - `Create<E>Request` / `Update<E>Request` reuse the entity's tags (the id of an update is
//!   tag 1, the tag of the read-only `id` field, absent from the writable set).
//! - `Enum` fields are `string` (a proto enum would number its values by position).
//!
//! Missing tag: [`TagMode::Strict`] refuses (`this.descriptor.invalid`), [`TagMode::Lenient`]
//! writes the field as a comment. Tags are allocated by [`assign_tags`] (append-only), the
//! function behind `descriptor.lock`.

use super::pascal_case;
use crate::entity::{EntityDesc, FIRST_USER_TAG, TENANT_ID, TENANT_ID_TAG};
use crate::error::ContractError;
use crate::field::{FieldDesc, FieldType};
use crate::framework::codes;
use std::collections::{BTreeMap, BTreeSet};

/// What to do with a field that has no `proto_tag`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TagMode {
    /// Refuse the derivation (`this.descriptor.invalid`, every missing field listed).
    Strict,
    /// Emit the field as a comment: no invented number ever reaches a client.
    Lenient,
}

pub const STRUCT_IMPORT: &str = "google/protobuf/struct.proto";

/// Messages and service of one entity.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ProtoEntity {
    /// Entity message, CRUD request/response messages and the CRUD service.
    pub text: String,
    /// Imports the text needs.
    pub imports: BTreeSet<&'static str>,
    /// Fields skipped for lack of a tag (Lenient mode).
    pub untagged: Vec<String>,
}

/// `(type, repeated)` of a field type; `imports` collects what the type needs.
pub fn proto_type(ty: &FieldType, imports: &mut BTreeSet<&'static str>) -> (String, bool) {
    let scalar = |t: &str| (t.to_string(), false);
    match ty {
        FieldType::String | FieldType::Text => scalar("string"),
        FieldType::Bool => scalar("bool"),
        FieldType::I32 => scalar("int32"),
        FieldType::I64 => scalar("int64"),
        FieldType::U32 => scalar("uint32"),
        FieldType::U64 => scalar("uint64"),
        FieldType::F32 => scalar("float"),
        FieldType::F64 => scalar("double"),
        // Decimal as string: no binary float in the contract.
        FieldType::Decimal { .. } => scalar("string"),
        FieldType::Uuid | FieldType::Ref { .. } => scalar("string"),
        FieldType::DateTime | FieldType::Date => scalar("string"),
        FieldType::Enum { .. } => scalar("string"),
        FieldType::List(inner) => {
            let (t, repeated) = proto_type(inner, imports);
            if repeated {
                // `repeated repeated` does not exist.
                imports.insert(STRUCT_IMPORT);
                ("google.protobuf.ListValue".into(), true)
            } else {
                (t, true)
            }
        }
        _ => {
            imports.insert(STRUCT_IMPORT);
            scalar("google.protobuf.Value")
        }
    }
}

fn field_line(f: &FieldDesc, tag: u32, imports: &mut BTreeSet<&'static str>) -> String {
    let (t, repeated) = proto_type(&f.ty, imports);
    let label = if repeated {
        "repeated "
    } else if f.optional {
        "optional "
    } else {
        ""
    };
    let mut line = format!("  {label}{t} {} = {tag}", f.name);
    if f.deprecated {
        line.push_str(" [deprecated = true]");
    }
    line.push(';');
    match &f.ty {
        FieldType::Enum { variants, .. } => {
            line.push_str(&format!(" // one of: {}", variants.join(", ")))
        }
        FieldType::Ref { entity } => line.push_str(&format!(" // id of {entity}")),
        FieldType::DateTime => line.push_str(" // RFC 3339"),
        FieldType::Date => line.push_str(" // YYYY-MM-DD"),
        FieldType::Decimal { scale } => line.push_str(&format!(" // decimal, scale {scale}")),
        _ => {}
    }
    line.push('\n');
    line
}

/// `reserved 8, 12 to 15;` (sorted, ranges collapsed).
fn reserved_line(tags: &BTreeSet<u32>) -> Option<String> {
    let mut parts = Vec::new();
    let mut it = tags.iter().copied().peekable();
    while let Some(start) = it.next() {
        let mut end = start;
        while it.peek() == Some(&(end + 1)) {
            end = it.next().unwrap_or(end);
        }
        parts.push(if start == end {
            start.to_string()
        } else {
            format!("{start} to {end}")
        });
    }
    (!parts.is_empty()).then(|| format!("  reserved {};\n", parts.join(", ")))
}

/// Largest protobuf field number (2^29 - 1).
pub const MAX_PROTO_TAG: u32 = 536_870_911;
/// Field numbers reserved by the protobuf implementation itself.
pub const PROTOBUF_RESERVED: std::ops::RangeInclusive<u32> = 19_000..=19_999;

/// Why `tag` cannot number a field in a `.proto` (`None` = valid).
pub fn invalid_tag_reason(tag: u32) -> Option<&'static str> {
    if tag == 0 || tag > MAX_PROTO_TAG {
        Some("outside 1..=536870911")
    } else if PROTOBUF_RESERVED.contains(&tag) {
        Some("in 19000..=19999 (reserved by protobuf)")
    } else {
        None
    }
}

/// Messages + service of `e`.
///
/// A field whose tag cannot appear in a valid `.proto` (no tag, a number protobuf refuses,
/// or a number already taken by another field) is refused in [`TagMode::Strict`] and
/// written as a comment in [`TagMode::Lenient`]: the output is always a parseable file.
pub fn entity_proto(e: &EntityDesc, mode: TagMode) -> Result<ProtoEntity, ContractError> {
    let pascal = pascal_case(&e.name);
    let mut out = ProtoEntity::default();
    let mut tagged: Vec<(u32, &FieldDesc)> = Vec::new();
    let mut untagged: Vec<&FieldDesc> = Vec::new();
    // (field, reason) of the fields that have a tag but cannot use it.
    let mut bad: Vec<(&FieldDesc, String)> = Vec::new();
    let mut seen: BTreeMap<u32, &str> = BTreeMap::new();
    let mut fields: Vec<&FieldDesc> = e.all_fields().collect();
    // Duplicates are reported on the field whose NAME sorts last: deterministic whatever
    // the declaration order.
    fields.sort_by(|a, b| a.name.cmp(&b.name));
    for f in fields {
        match f.proto_tag {
            Some(t) => {
                if let Some(reason) = invalid_tag_reason(t) {
                    bad.push((f, format!("tag {t} {reason}")));
                } else if let Some(other) = seen.get(&t) {
                    bad.push((f, format!("tag {t} already used by `{other}`")));
                } else {
                    seen.insert(t, &f.name);
                    tagged.push((t, f));
                }
            }
            None => untagged.push(f),
        }
    }
    if mode == TagMode::Strict && (!untagged.is_empty() || !bad.is_empty()) {
        let mut issues: Vec<String> = Vec::new();
        if !untagged.is_empty() {
            let names: Vec<String> = untagged
                .iter()
                .map(|f| format!("{}.{}", e.name, f.name))
                .collect();
            issues.push(format!(
                "no proto_tag on {} (run `this descriptor lock`)",
                names.join(", ")
            ));
        }
        for (f, reason) in &bad {
            issues.push(format!("{}.{}: {reason}", e.name, f.name));
        }
        return Err(ContractError::framework(codes::DESCRIPTOR_INVALID)
            .with_param("issues", issues.join("; ")));
    }
    tagged.sort_by_key(|(t, _)| *t);
    out.untagged = untagged
        .iter()
        .map(|f| f.name.clone())
        .chain(bad.iter().map(|(f, _)| f.name.clone()))
        .collect();

    let used: BTreeSet<u32> = tagged.iter().map(|(t, _)| *t).collect();
    let mut reserved: BTreeSet<u32> = (1..FIRST_USER_TAG).filter(|t| !used.contains(t)).collect();
    reserved.extend(
        e.reserved_tags
            .iter()
            .filter(|t| !used.contains(t) && invalid_tag_reason(**t).is_none()),
    );

    let imports = &mut out.imports;
    let t = &mut out.text;
    t.push_str(&format!("// {pascal} entity ({})\n", e.plural));
    t.push_str(&format!("message {pascal} {{\n"));
    for (tag, f) in &tagged {
        t.push_str(&field_line(f, *tag, imports));
    }
    for f in &untagged {
        t.push_str(&format!(
            "  // {}: no proto_tag yet (run `this descriptor lock`)\n",
            f.name
        ));
    }
    for (f, reason) in &bad {
        t.push_str(&format!("  // {}: {reason}, not numbered\n", f.name));
    }
    if let Some(r) = reserved_line(&reserved) {
        t.push_str(&r);
    }
    t.push_str("}\n\n");

    t.push_str(&format!(
        "message Get{pascal}Request {{\n  string id = 1;\n}}\n\n"
    ));
    t.push_str(&format!(
        "message List{pascal}Request {{\n  int32 limit = 1;\n  int32 offset = 2;\n}}\n\n"
    ));
    t.push_str(&format!(
        "message List{pascal}Response {{\n  repeated {pascal} items = 1;\n  int32 total = 2;\n}}\n\n"
    ));
    t.push_str(&format!("message Create{pascal}Request {{\n"));
    for (tag, f) in tagged.iter().filter(|(_, f)| !f.read_only) {
        t.push_str(&field_line(f, *tag, imports));
    }
    t.push_str("}\n\n");
    t.push_str(&format!(
        "message Update{pascal}Request {{\n  string id = 1;\n"
    ));
    for (tag, f) in tagged.iter().filter(|(_, f)| !f.read_only && !f.immutable) {
        t.push_str(&field_line(f, *tag, imports));
    }
    t.push_str("}\n\n");
    t.push_str(&format!(
        "message Delete{pascal}Request {{\n  string id = 1;\n}}\n\n"
    ));
    t.push_str(&format!(
        "message Delete{pascal}Response {{\n  bool success = 1;\n}}\n\n"
    ));
    t.push_str(&service_block(&pascal));
    Ok(out)
}

/// CRUD service of an entity (same shape for described and sample-inferred entities).
pub fn service_block(pascal: &str) -> String {
    format!(
        "service {p}Service {{\n  rpc Get{p}(Get{p}Request) returns ({p});\n  rpc List{p}(List{p}Request) returns (List{p}Response);\n  rpc Create{p}(Create{p}Request) returns ({p});\n  rpc Update{p}(Update{p}Request) returns ({p});\n  rpc Delete{p}(Delete{p}Request) returns (Delete{p}Response);\n}}\n\n",
        p = pascal
    )
}

/// Complete file for `entities`.
pub fn file_for(
    entities: &[&EntityDesc],
    package: &str,
    mode: TagMode,
) -> Result<String, ContractError> {
    let mut imports = BTreeSet::new();
    let mut body = String::new();
    for e in entities {
        let p = entity_proto(e, mode)?;
        imports.extend(p.imports);
        body.push_str(&p.text);
    }
    let mut s = format!("syntax = \"proto3\";\n\npackage {package};\n\n");
    for i in imports {
        s.push_str(&format!("import \"{i}\";\n"));
    }
    if !s.ends_with("\n\n") {
        s.push('\n');
    }
    s.push_str(&body);
    Ok(s)
}

/// `field -> tag` of the declared fields that have one (content of a `descriptor.lock`
/// entry).
pub fn tag_map(e: &EntityDesc) -> BTreeMap<String, u32> {
    e.fields
        .iter()
        .filter_map(|f| f.proto_tag.map(|t| (f.name.clone(), t)))
        .collect()
}

/// Give a tag to every declared field without one, append-only:
/// 1. an overlay of a base field (`name`, `status`) gets the BASE tag (6, 7): it is the
///    same field on the wire;
/// 2. a field already in `previous` (the lock) gets its previous tag back, unless another
///    field already holds that number (then it is treated as new);
/// 3. `tenant_id` gets [`TENANT_ID_TAG`] (if free);
/// 4. the others, in NAME order (never declaration order), get `max + 1`, where `max` covers
///    the tags in use, `reserved_tags`, every tag of `previous` (a removed field's tag is
///    never reused) and 15; numbers protobuf refuses (19000..=19999) are skipped.
///
/// Returns the newly assigned `(field, tag)`.
pub fn assign_tags(e: &mut EntityDesc, previous: &BTreeMap<String, u32>) -> Vec<(String, u32)> {
    let mut assigned = Vec::new();
    let kind = e.kind;
    let mut used: BTreeSet<u32> = e.fields.iter().filter_map(|f| f.proto_tag).collect();
    for f in e.fields.iter_mut().filter(|f| f.proto_tag.is_none()) {
        let base_tag = crate::entity::base_fields(kind)
            .iter()
            .find(|b| b.name == f.name)
            .and_then(|b| b.proto_tag);
        let tag = base_tag
            .or_else(|| previous.get(&f.name).copied())
            .or_else(|| (f.name == TENANT_ID).then_some(TENANT_ID_TAG))
            .filter(|t| !used.contains(t));
        if let Some(t) = tag {
            f.proto_tag = Some(t);
            used.insert(t);
            assigned.push((f.name.clone(), t));
        }
    }
    let first_free = e
        .fields
        .iter()
        .filter_map(|f| f.proto_tag)
        .chain(e.reserved_tags.iter().copied())
        .chain(previous.values().copied())
        .fold(FIRST_USER_TAG - 1, u32::max)
        + 1;
    let mut fresh: Vec<usize> = (0..e.fields.len())
        .filter(|&i| e.fields[i].proto_tag.is_none())
        .collect();
    fresh.sort_by(|&a, &b| e.fields[a].name.cmp(&e.fields[b].name));
    let numbers = (first_free..).filter(|t| invalid_tag_reason(*t).is_none());
    for (tag, i) in numbers.zip(fresh) {
        e.fields[i].proto_tag = Some(tag);
        assigned.push((e.fields[i].name.clone(), tag));
    }
    assigned
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::check::CheckProfile;
    use crate::custom::CustomRegistry;
    use crate::module::ModuleDescriptor;

    fn invoice(order: &[usize]) -> EntityDesc {
        let all = [
            FieldDesc::new("amount", FieldType::F64).tag(16),
            FieldDesc::new("note", FieldType::Text).optional().tag(18),
            FieldDesc::new("lines", FieldType::List(Box::new(FieldType::Uuid))).tag(19),
            FieldDesc::new("tenant_id", FieldType::Uuid)
                .read_only()
                .tag(8),
            FieldDesc::new("meta", FieldType::Json).optional().tag(20),
        ];
        let mut e = EntityDesc::new("invoice");
        e.fields = order.iter().map(|&i| all[i].clone()).collect();
        e.reserved_tags = vec![17];
        e
    }

    #[test]
    fn proto_tags_stable_across_field_reorder() {
        let a = entity_proto(&invoice(&[0, 1, 2, 3, 4]), TagMode::Strict).unwrap();
        let b = entity_proto(&invoice(&[4, 2, 0, 3, 1]), TagMode::Strict).unwrap();
        assert_eq!(a, b, "declaration order never changes the output");
        assert!(a.text.contains("  double amount = 16;\n"), "{}", a.text);
        assert!(
            a.text.contains("  optional string note = 18;\n"),
            "{}",
            a.text
        );
        assert!(
            a.text.contains("  repeated string lines = 19;\n"),
            "{}",
            a.text
        );
        assert!(a.text.contains("  string tenant_id = 8;\n"), "{}", a.text);
        assert!(a.imports.contains(STRUCT_IMPORT));
    }

    #[test]
    fn proto_emits_reserved_tags() {
        let p = entity_proto(&invoice(&[0, 1, 2, 3, 4]), TagMode::Strict).unwrap();
        let msg = &p.text[..p.text.find("}\n").unwrap()];
        // Data base fields use 1..=7, tenant_id 8: 9..=15 reserved for the framework, 17
        // removed.
        assert!(msg.contains("  reserved 9 to 15, 17;\n"), "{msg}");
        // Without tenant_id, 8 is reserved too.
        let p = entity_proto(&invoice(&[0, 1]), TagMode::Strict).unwrap();
        assert!(p.text.contains("  reserved 8 to 15, 17;\n"), "{}", p.text);
        // Read-only fields are not in the create request; update keeps the same tags.
        let create = &p.text[p.text.find("message CreateInvoiceRequest").unwrap()..];
        let create = &create[..create.find('}').unwrap()];
        assert!(create.contains("double amount = 16;"), "{create}");
        assert!(!create.contains(" id = 1;"), "{create}");
        assert!(create.contains("string status = 6;"), "{create}");
    }

    #[test]
    fn proto_requires_tags_only_when_deriving() {
        let mut e = invoice(&[0, 1]);
        e.fields.push(FieldDesc::new("late", FieldType::Bool));
        // check() accepts untagged fields...
        let mut m = ModuleDescriptor::new("billing", "1.0.0");
        m.entities.push(e.clone());
        m.check(&CustomRegistry::default(), CheckProfile::Compiled)
            .unwrap();
        // ...the strict derivation refuses them, the lenient one comments them out.
        let err = entity_proto(&e, TagMode::Strict).unwrap_err();
        assert_eq!(err.code.as_str(), codes::DESCRIPTOR_INVALID);
        assert!(
            err.params["issues"]
                .as_str()
                .unwrap()
                .contains("invoice.late")
        );
        let p = entity_proto(&e, TagMode::Lenient).unwrap();
        assert!(p.text.contains("  // late: no proto_tag yet"), "{}", p.text);
        assert!(!p.text.contains("bool late ="), "{}", p.text);
        assert_eq!(p.untagged, vec!["late".to_string()]);
    }

    #[test]
    fn assign_tags_is_append_only_and_order_independent() {
        let mk = |names: &[&str]| {
            let mut e = EntityDesc::new("invoice");
            e.fields = names
                .iter()
                .map(|n| FieldDesc::new(*n, FieldType::String))
                .collect();
            e
        };
        let mut a = mk(&["zeta", "alpha", "tenant_id"]);
        let mut b = mk(&["tenant_id", "alpha", "zeta"]);
        assign_tags(&mut a, &BTreeMap::new());
        assign_tags(&mut b, &BTreeMap::new());
        assert_eq!(tag_map(&a), tag_map(&b));
        let lock = tag_map(&a);
        assert_eq!(lock["alpha"], 16);
        assert_eq!(lock["zeta"], 17);
        assert_eq!(lock["tenant_id"], TENANT_ID_TAG);
        // Remove `alpha`, add `beta`: `beta` never takes 16 back, `zeta` keeps 17.
        let mut c = mk(&["beta", "zeta"]);
        let new = assign_tags(&mut c, &lock);
        assert_eq!(tag_map(&c)["zeta"], 17);
        assert_eq!(tag_map(&c)["beta"], 18);
        assert_eq!(new.len(), 2);
    }

    #[test]
    fn proto_refuses_out_of_range_tags() {
        // A descriptor that did not go through check() (runtime YAML, hand-built set): the
        // derivation itself never writes a number protobuf refuses, nor the same number twice.
        let mut e = invoice(&[0, 1]);
        e.fields
            .push(FieldDesc::new("bad", FieldType::Bool).tag(19_500));
        e.fields
            .push(FieldDesc::new("zero", FieldType::Bool).tag(0));
        e.fields
            .push(FieldDesc::new("twin", FieldType::Bool).tag(16));
        e.reserved_tags.push(19_001);
        let err = entity_proto(&e, TagMode::Strict).unwrap_err();
        let issues = err.params["issues"].as_str().unwrap().to_string();
        assert_eq!(err.code.as_str(), codes::DESCRIPTOR_INVALID);
        assert!(issues.contains("invoice.bad: tag 19500"), "{issues}");
        assert!(issues.contains("invoice.zero: tag 0"), "{issues}");
        // `amount` < `twin` by name: the duplicate is reported on `twin`, whatever the order.
        assert!(
            issues.contains("invoice.twin: tag 16 already used by `amount`"),
            "{issues}"
        );
        let mut reordered = e.clone();
        reordered.fields.reverse();
        let a = entity_proto(&e, TagMode::Lenient).unwrap();
        let b = entity_proto(&reordered, TagMode::Lenient).unwrap();
        assert_eq!(a, b);
        assert!(a.text.contains("  double amount = 16;\n"), "{}", a.text);
        assert!(!a.text.contains("= 19500;"), "{}", a.text);
        assert!(!a.text.contains("= 0;"), "{}", a.text);
        assert!(!a.text.contains("bool twin = 16;"), "{}", a.text);
        assert!(!a.text.contains("19001"), "{}", a.text);
        assert!(
            a.text
                .contains("  // twin: tag 16 already used by `amount`"),
            "{}",
            a.text
        );
        assert_eq!(a.untagged, vec!["bad", "twin", "zero"]);
    }

    #[test]
    fn assign_tags_keeps_base_tag_on_overlays_and_never_collides() {
        let mut e = EntityDesc::new("invoice");
        // Overlay of `status` declared without a tag (e.g. from YAML): base tag 6, not 16.
        e.fields = vec![
            FieldDesc::new("status", FieldType::String),
            FieldDesc::new("amount", FieldType::F64),
            FieldDesc::new("explicit", FieldType::F64).tag(16),
        ];
        // The lock says amount = 16, but `explicit` now holds 16: amount gets a new number.
        let lock: BTreeMap<String, u32> = [("amount".to_string(), 16)].into();
        assign_tags(&mut e, &lock);
        let m = tag_map(&e);
        assert_eq!(m["status"], 6);
        assert_eq!(m["explicit"], 16);
        assert_eq!(m["amount"], 17);
        let mut module = ModuleDescriptor::new("billing", "1.0.0");
        module.entities.push(e.clone());
        module
            .check(&CustomRegistry::default(), CheckProfile::Compiled)
            .unwrap();
        entity_proto(&e, TagMode::Strict).unwrap();
        // Allocation skips the numbers protobuf reserves.
        let mut big = EntityDesc::new("big");
        big.fields = vec![FieldDesc::new("a", FieldType::Bool)];
        big.reserved_tags = vec![18_999];
        assign_tags(&mut big, &BTreeMap::new());
        assert_eq!(tag_map(&big)["a"], 20_000);
    }

    #[test]
    fn proto_file_strict_from_set() {
        let mut m = ModuleDescriptor::new("billing", "1.0.0");
        m.entities.push(invoice(&[0, 1, 2, 3, 4]));
        let mut set = crate::module::DescriptorSet::new();
        set.insert(m).unwrap();
        let f = crate::derive::proto_file(&set, "billing.v1").unwrap();
        assert!(f.starts_with("syntax = \"proto3\";\n\npackage billing.v1;\n\nimport \"google/protobuf/struct.proto\";\n\n"), "{f}");
        assert!(f.contains("service InvoiceService {"), "{f}");
    }
}
