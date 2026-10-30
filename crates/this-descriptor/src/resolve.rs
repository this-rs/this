//! Dependency resolution between modules: topological order, semver, cycles, duplicates,
//! and cross-module references. Independent of registration order.

use crate::error::ContractError;
use crate::field::FieldType;
use crate::framework::codes;
use crate::module::ModuleDescriptor;
use std::collections::{BTreeMap, BTreeSet};

/// Outcome of a successful resolution.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Resolution {
    /// Load order: every module after the modules it requires. Ties broken by name, so
    /// the order does not depend on registration order.
    pub order: Vec<String>,
    /// `(module, dependency)` optional dependencies that were absent.
    pub skipped_optional: Vec<(String, String)>,
}

/// Resolve `candidates` (modules to load) against `active` (modules already running).
///
/// Every error is reported (not only the first). Rules:
/// - same module name twice → `this.module.duplicate_module`;
/// - required module absent → `this.module.dependency_missing` (optional: skipped);
/// - version outside `VersionReq` (also for a present optional one) → `this.module.dependency_version`;
/// - cycle → `this.module.cycle`;
/// - entity declared by two modules → `this.module.duplicate_entity`;
/// - link end or `Ref` to an entity neither local nor in a (transitively) required module →
///   `this.module.reference_unknown`.
pub fn resolve(
    candidates: &[&ModuleDescriptor],
    active: &[&ModuleDescriptor],
) -> Result<Resolution, Vec<ContractError>> {
    let mut errors = Vec::new();
    let mut all: BTreeMap<&str, &ModuleDescriptor> = BTreeMap::new();
    for m in active.iter().chain(candidates.iter()) {
        if all.insert(m.name.as_str(), m).is_some() {
            errors.push(
                ContractError::framework(codes::MODULE_DUPLICATE_MODULE)
                    .with_param("module", &m.name),
            );
        }
    }

    // Entities across modules.
    let mut owner: BTreeMap<&str, &str> = BTreeMap::new();
    for m in all.values() {
        for e in &m.entities {
            if let Some(prev) = owner.insert(e.name.as_str(), m.name.as_str()) {
                errors.push(
                    ContractError::framework(codes::MODULE_DUPLICATE_ENTITY)
                        .with_param("entity", &e.name)
                        .with_param("modules", format!("{prev}, {}", m.name)),
                );
            }
        }
    }

    // Edges and versions.
    let mut skipped = Vec::new();
    let mut deps: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
    for m in all.values() {
        let entry = deps.entry(m.name.as_str()).or_default();
        for r in &m.requires {
            match all.get(r.name.as_str()) {
                None if r.optional => skipped.push((m.name.clone(), r.name.clone())),
                None => errors.push(
                    ContractError::framework(codes::MODULE_DEPENDENCY_MISSING)
                        .with_param("module", &m.name)
                        .with_param("dependency", &r.name),
                ),
                Some(dep) => {
                    let ok = match (semver::VersionReq::parse(&r.version), dep.semver()) {
                        (Ok(req), Some(v)) => req.matches(&v),
                        _ => false,
                    };
                    if !ok {
                        errors.push(
                            ContractError::framework(codes::MODULE_DEPENDENCY_VERSION)
                                .with_param("module", &m.name)
                                .with_param("dependency", &r.name)
                                .with_param("required", &r.version)
                                .with_param("found", &dep.version),
                        );
                    }
                    entry.insert(dep.name.as_str());
                }
            }
        }
    }

    // Kahn, smallest name first.
    let mut indeg: BTreeMap<&str, usize> = deps.iter().map(|(k, v)| (*k, v.len())).collect();
    let mut ready: BTreeSet<&str> = indeg
        .iter()
        .filter(|(_, d)| **d == 0)
        .map(|(k, _)| *k)
        .collect();
    let mut order = Vec::new();
    while let Some(n) = ready.iter().next().copied() {
        ready.remove(n);
        order.push(n.to_string());
        for (m, ds) in &deps {
            if ds.contains(n) {
                let d = indeg.get_mut(m).unwrap();
                *d -= 1;
                if *d == 0 {
                    ready.insert(*m);
                }
            }
        }
    }
    if order.len() < deps.len() {
        let stuck: Vec<&str> = indeg
            .iter()
            .filter(|(_, d)| **d > 0)
            .map(|(k, _)| *k)
            .collect();
        errors.push(
            ContractError::framework(codes::MODULE_CYCLE).with_param("modules", stuck.join(", ")),
        );
    }

    // References: local or in the transitive closure of `requires`.
    for m in all.values() {
        let mut reach: BTreeSet<&str> = BTreeSet::new();
        let mut stack = vec![m.name.as_str()];
        while let Some(x) = stack.pop() {
            if reach.insert(x)
                && let Some(ds) = deps.get(x)
            {
                stack.extend(ds.iter().copied());
            }
        }
        let visible = |entity: &str| owner.get(entity).is_some_and(|o| reach.contains(o));
        let mut refs: Vec<&str> = Vec::new();
        for l in &m.links {
            refs.push(&l.source);
            refs.push(&l.target);
        }
        for e in &m.entities {
            for f in &e.fields {
                collect_refs(&f.ty, &mut refs);
            }
        }
        for r in refs {
            if !visible(r) {
                errors.push(
                    ContractError::framework(codes::MODULE_REFERENCE_UNKNOWN)
                        .with_param("module", &m.name)
                        .with_param("entity", r),
                );
            }
        }
    }

    if errors.is_empty() {
        // Only the candidates' order matters to the caller, but keep active ones for context.
        let cand: BTreeSet<&str> = candidates.iter().map(|m| m.name.as_str()).collect();
        Ok(Resolution {
            order: order
                .into_iter()
                .filter(|n| cand.contains(n.as_str()))
                .collect(),
            skipped_optional: skipped,
        })
    } else {
        Err(errors)
    }
}

fn collect_refs<'a>(ty: &'a FieldType, out: &mut Vec<&'a str>) {
    match ty {
        FieldType::Ref { entity } => out.push(entity),
        FieldType::List(inner) => collect_refs(inner, out),
        _ => {}
    }
}

/// Modules of `active` that require `name` (an unload must be refused while any is active).
pub fn dependents<'a>(name: &str, active: &[&'a ModuleDescriptor]) -> Vec<&'a str> {
    active
        .iter()
        .filter(|m| m.requires.iter().any(|r| r.name == name))
        .map(|m| m.name.as_str())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entity::EntityDesc;
    use crate::module::ModuleReq;

    fn m(name: &str, v: &str, reqs: &[(&str, &str, bool)]) -> ModuleDescriptor {
        let mut d = ModuleDescriptor::new(name, v);
        d.requires = reqs
            .iter()
            .map(|(n, r, o)| ModuleReq {
                name: n.to_string(),
                version: r.to_string(),
                optional: *o,
            })
            .collect();
        d
    }
    fn codes(r: Result<Resolution, Vec<ContractError>>) -> Vec<String> {
        r.unwrap_err().into_iter().map(|e| e.code.0).collect()
    }

    #[test]
    fn resolve_orders_by_dependencies() {
        let a = m("auth", "1.2.0", &[]);
        let b = m("billing", "1.0.0", &[("auth", "^1.1", false)]);
        let c = m(
            "crm",
            "2.0.0",
            &[("billing", "^1", false), ("auth", "^1", false)],
        );
        let r = resolve(&[&c, &b, &a], &[]).unwrap();
        assert_eq!(r.order, ["auth", "billing", "crm"]);
    }

    #[test]
    fn resolve_independent_of_registration_order() {
        let a = m("auth", "1.2.0", &[]);
        let b = m("billing", "1.0.0", &[("auth", "^1", false)]);
        let z = m("zeta", "1.0.0", &[]);
        let r1 = resolve(&[&b, &z, &a], &[]).unwrap();
        let r2 = resolve(&[&a, &b, &z], &[]).unwrap();
        assert_eq!(r1, r2);
        assert_eq!(r1.order, ["auth", "billing", "zeta"]);
    }

    #[test]
    fn resolve_detects_cycle() {
        let a = m("a", "1.0.0", &[("b", "*", false)]);
        let b = m("b", "1.0.0", &[("a", "*", false)]);
        assert_eq!(codes(resolve(&[&a, &b], &[])), vec![codes::MODULE_CYCLE]);
    }

    #[test]
    fn resolve_rejects_version_mismatch_and_missing() {
        let a = m("auth", "2.0.0", &[]);
        let b = m(
            "billing",
            "1.0.0",
            &[("auth", "^1", false), ("ledger", "^1", false)],
        );
        let c = codes(resolve(&[&a, &b], &[]));
        assert!(c.contains(&codes::MODULE_DEPENDENCY_VERSION.to_string()));
        assert!(c.contains(&codes::MODULE_DEPENDENCY_MISSING.to_string()));
    }

    #[test]
    fn optional_dependency_skipped_but_version_checked_when_present() {
        let b = m("billing", "1.0.0", &[("audit", "^1", true)]);
        let r = resolve(&[&b], &[]).unwrap();
        assert_eq!(r.skipped_optional, vec![("billing".into(), "audit".into())]);
        let audit = m("audit", "3.0.0", &[]);
        assert_eq!(
            codes(resolve(&[&b], &[&audit])),
            vec![codes::MODULE_DEPENDENCY_VERSION]
        );
    }

    #[test]
    fn duplicate_module_and_entity_are_errors() {
        let mut a = m("a", "1.0.0", &[]);
        a.entities.push(EntityDesc::new("invoice"));
        let mut b = m("b", "1.0.0", &[]);
        b.entities.push(EntityDesc::new("invoice"));
        assert_eq!(
            codes(resolve(&[&a, &b], &[])),
            vec![codes::MODULE_DUPLICATE_ENTITY]
        );
        let a2 = m("a", "1.1.0", &[]);
        assert!(
            codes(resolve(&[&a2], &[&a])).contains(&codes::MODULE_DUPLICATE_MODULE.to_string())
        );
    }

    #[test]
    fn references_must_be_local_or_required() {
        let mut users = m("users", "1.0.0", &[]);
        users.entities.push(EntityDesc::new("user"));
        let mut billing = m("billing", "1.0.0", &[]);
        billing.entities.push(EntityDesc::new("invoice"));
        billing
            .links
            .push(crate::link::LinkDesc::new("owner", "user", "invoice"));
        assert_eq!(
            codes(resolve(&[&users, &billing], &[])),
            vec![codes::MODULE_REFERENCE_UNKNOWN]
        );
        billing.requires.push(ModuleReq {
            name: "users".into(),
            version: "^1".into(),
            optional: false,
        });
        resolve(&[&users, &billing], &[]).unwrap();
    }

    #[test]
    fn dependents_lists_active_requirers() {
        let a = m("auth", "1.0.0", &[]);
        let b = m("billing", "1.0.0", &[("auth", "^1", false)]);
        assert_eq!(dependents("auth", &[&a, &b]), vec!["billing"]);
    }
}
