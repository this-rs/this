//! The verdict of the view checks as it was BEFORE `replaces` could name a view of a required
//! module, kept verbatim: the frozen reference of the differential tests. Test-only, and not
//! to be "fixed" — a difference with the live checks must be explained, not erased here.

use super::{Checks, Issue, ViewDesc, required_modules};
use crate::module::ModuleDescriptor;
use std::collections::{BTreeSet, HashMap};

/// The verdict of [`issues_with_required`](super::issues_with_required) as it was BEFORE `replaces` could name a view of a
/// required module: the frozen reference of the differential tests. Not to be "fixed".
pub(crate) fn issues_with_required_before(
    m: &ModuleDescriptor,
    known: &[&ModuleDescriptor],
) -> Vec<Issue> {
    let required = required_modules(m, known);
    let vues: Vec<&ViewDesc> = m.views.iter().collect();
    let mut c = Checks::new(m, vues);
    for dep in required {
        for e in &dep.entities {
            c.entities.entry(e.name.as_str()).or_insert(e);
        }
    }
    c.operations();
    c.actions();
    c.views();
    c.replacements_before();
    c.out
}

impl<'a> Checks<'a> {
    /// `replacements` as it was when `replaces` only knew the module's own views, verbatim.
    fn replacements_before(&mut self) {
        let vues = self.vues.clone();
        let mut remplacees: HashMap<&str, &str> = HashMap::new();
        let mut suivant: HashMap<&str, &str> = HashMap::new();
        for (i, v) in vues.iter().copied().enumerate() {
            let Some(cible) = v.replaces.as_deref() else {
                continue;
            };
            let p = format!("views[{i}].replaces");
            if cible == v.name {
                self.push(
                    "view_replaces_self",
                    &p,
                    format!("`{cible}` replaces itself"),
                );
                continue;
            }
            if !self.views.contains(cible) {
                self.push(
                    "view_target_unknown",
                    &p,
                    format!("`{cible}` is not a view of this module"),
                );
                continue;
            }
            if remplacees.insert(cible, v.name.as_str()).is_some() {
                self.push(
                    "view_replaced_twice",
                    &p,
                    format!("`{cible}` is replaced by more than one view"),
                );
            }
            suivant.insert(v.name.as_str(), cible);
        }
        for (i, v) in vues.iter().copied().enumerate() {
            let mut courant = v.name.as_str();
            let mut vu = BTreeSet::new();
            while let Some(&n) = suivant.get(courant) {
                if n == v.name {
                    self.push(
                        "view_replaces_cycle",
                        format!("views[{i}].replaces"),
                        format!("`{}` replaces itself through a chain", v.name),
                    );
                    break;
                }
                if !vu.insert(n) {
                    break;
                }
                courant = n;
            }
        }
    }
}
