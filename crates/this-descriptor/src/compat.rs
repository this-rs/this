//! Compatibility between two versions of a descriptor (proto tags).
//!
//! Called by `this descriptor check` (CLI), by `ModuleStore::put` against the highest
//! stored version, and by a test generated in each project: a tag published once is never
//! moved nor reused.

use crate::check::DescriptorIssue;
use crate::error::ErrorCode;
use crate::module::ModuleDescriptor;
use std::collections::BTreeMap;

/// Issues that make `next` incompatible with `prev`.
pub fn check_compat(prev: &ModuleDescriptor, next: &ModuleDescriptor) -> Vec<DescriptorIssue> {
    let mut out = Vec::new();
    let issue = |reason: &str, path: String, message: String| DescriptorIssue {
        code: ErrorCode::new(format!("this.descriptor.{reason}")),
        path,
        message,
    };
    for pe in &prev.entities {
        let Some(ne) = next.entity(&pe.name) else {
            continue;
        };
        let next_by_tag: BTreeMap<u32, &str> = ne
            .fields
            .iter()
            .filter_map(|f| f.proto_tag.map(|t| (t, f.name.as_str())))
            .collect();
        for pf in &pe.fields {
            let Some(tag) = pf.proto_tag else { continue };
            let path = format!("entities.{}.fields.{}", pe.name, pf.name);
            match ne.field(&pf.name) {
                Some(nf) if nf.proto_tag.is_some_and(|t| t != tag) => out.push(issue(
                    "tag_moved",
                    path,
                    format!("tag {tag} -> {}", nf.proto_tag.unwrap()),
                )),
                Some(_) => {}
                None => {
                    if let Some(other) = next_by_tag.get(&tag) {
                        out.push(issue(
                            "tag_reused",
                            path,
                            format!("tag {tag} reused by `{other}`"),
                        ));
                    } else if !ne.reserved_tags.contains(&tag) {
                        out.push(issue(
                            "tag_not_reserved",
                            path,
                            format!("removed field: add {tag} to reserved_tags"),
                        ));
                    }
                }
            }
        }
        // A tag `prev` reserved stays reserved: no field of `next` may take it (the field
        // that owned it may be gone from every version still around, so the walk over
        // `prev.fields` above cannot see the reuse).
        for nf in &ne.fields {
            if let Some(tag) = nf.proto_tag
                && pe.reserved_tags.contains(&tag)
            {
                out.push(issue(
                    "tag_reused",
                    format!("entities.{}.fields.{}", ne.name, nf.name),
                    format!("tag {tag} reserved by version {}", prev.version),
                ));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entity::EntityDesc;
    use crate::field::{FieldDesc, FieldType};

    fn v(fields: Vec<FieldDesc>, reserved: Vec<u32>) -> ModuleDescriptor {
        let mut m = ModuleDescriptor::new("billing", "1.0.0");
        let mut e = EntityDesc::new("invoice");
        e.fields = fields;
        e.reserved_tags = reserved;
        m.entities.push(e);
        m
    }

    #[test]
    fn tag_reuse_move_and_unreserved_removal_detected() {
        let prev = v(
            vec![
                FieldDesc::new("amount", FieldType::F64).tag(16),
                FieldDesc::new("note", FieldType::String).tag(17),
            ],
            vec![],
        );
        let reused = v(
            vec![
                FieldDesc::new("amount", FieldType::F64).tag(16),
                FieldDesc::new("memo", FieldType::String).tag(17),
            ],
            vec![],
        );
        assert_eq!(
            check_compat(&prev, &reused)[0].code.0,
            "this.descriptor.tag_reused"
        );
        let moved = v(
            vec![
                FieldDesc::new("amount", FieldType::F64).tag(18),
                FieldDesc::new("note", FieldType::String).tag(17),
            ],
            vec![],
        );
        assert_eq!(
            check_compat(&prev, &moved)[0].code.0,
            "this.descriptor.tag_moved"
        );
        let removed = v(
            vec![FieldDesc::new("amount", FieldType::F64).tag(16)],
            vec![],
        );
        assert_eq!(
            check_compat(&prev, &removed)[0].code.0,
            "this.descriptor.tag_not_reserved"
        );
        let reserved = v(
            vec![FieldDesc::new("amount", FieldType::F64).tag(16)],
            vec![17],
        );
        assert!(check_compat(&prev, &reserved).is_empty());
    }

    #[test]
    fn reserved_tag_of_prev_cannot_be_taken_by_next() {
        // v2 removed `note` (17) and reserved it; v3 gives 17 to `memo`. Against v2 alone
        // (v1 no longer stored, or the CLI comparing two adjacent versions) the reuse
        // must still be seen.
        let v2 = v(
            vec![FieldDesc::new("amount", FieldType::F64).tag(16)],
            vec![17],
        );
        let v3 = v(
            vec![
                FieldDesc::new("amount", FieldType::F64).tag(16),
                FieldDesc::new("memo", FieldType::String).tag(17),
            ],
            vec![],
        );
        let issues = check_compat(&v2, &v3);
        assert_eq!(issues.len(), 1, "{issues:?}");
        assert_eq!(issues[0].code.0, "this.descriptor.tag_reused");
        assert_eq!(issues[0].path, "entities.invoice.fields.memo");
        // Keeping the reservation and using a fresh tag is fine.
        let v3_ok = v(
            vec![
                FieldDesc::new("amount", FieldType::F64).tag(16),
                FieldDesc::new("memo", FieldType::String).tag(18),
            ],
            vec![17],
        );
        assert!(check_compat(&v2, &v3_ok).is_empty());
    }
}
