//! # The menu, built from the views the modules declare
//!
//! The host no longer carries a hard-coded list of entries: every entry comes from a view
//! a module declares (`views[]` in its descriptor). This module is pure: it takes the
//! declarations and the set of loaded modules, and returns what to draw **and** what was
//! left out, with the reason. Nothing is dropped silently.
//!
//! ## Input shape
//!
//! [`DeclaredView`] reads the same JSON as `this_descriptor::ViewDesc` (`name`, `surface`,
//! `title`, `replaces`) and ignores the rest (`root`, `look`): the host can hand the
//! descriptor's `views` array over as it is, without this crate depending on the
//! descriptor.
//!
//! ## Order (deterministic, independent of the caller's order)
//!
//! 1. modules by name, ascending (byte order);
//! 2. inside a module, views in declaration order;
//! 3. a replacement takes the **slot** of the view it replaces (chains follow through:
//!    if `b` replaces `a` and `c` replaces `b`, `c` sits where `a` was).
//!
//! ## Identifiers
//!
//! An entry's identifier is its view `name`: it is what a route or a shortcut names, so it
//! is shared by every module of the app. Two modules cannot claim the same identifier:
//! the first claimant in the order above keeps it, every other claimant is **refused**
//! ([`Reason::IdTaken`]) and the refusal names the holder. The only way to take an
//! identifier over is to declare `replaces`.
//!
//! ## `replaces`
//!
//! - `"view"`: a view of the same module (what the descriptor checks today);
//! - `"module:view"`: a view of another module.
//!
//! A target can be replaced once; a second replacement is refused
//! ([`Reason::AlreadyReplaced`]). A replacement whose target is absent is still shown, as a
//! plain entry, and the miss is reported ([`Reason::ReplaceTargetMissing`], `shown: true`).

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

/// Where a view is drawn. Same names as `this_descriptor::Surface`.
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum Placement {
    /// The drawer (side menu).
    Sidebar,
    /// The main screen.
    #[default]
    Main,
    /// A panel next to or over the main content.
    Panel,
    /// The header bar.
    Header,
}

impl Placement {
    /// Is this placement part of the menu (drawer or header)?
    pub fn is_menu(self) -> bool {
        matches!(self, Placement::Sidebar | Placement::Header)
    }
}

/// One view as declared by a module. Unknown fields are ignored on purpose (see the module
/// documentation): the descriptor owns their validation.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeclaredView {
    pub name: String,
    #[serde(default)]
    pub surface: Placement,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub replaces: Option<String>,
}

impl DeclaredView {
    pub fn new(name: impl Into<String>, surface: Placement) -> Self {
        Self {
            name: name.into(),
            surface,
            title: None,
            replaces: None,
        }
    }

    pub fn titled(mut self, title: impl Into<String>) -> Self {
        self.title = Some(title.into());
        self
    }

    pub fn replacing(mut self, target: impl Into<String>) -> Self {
        self.replaces = Some(target.into());
        self
    }
}

/// The views one module declares.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModuleViews {
    pub module: String,
    #[serde(default)]
    pub views: Vec<DeclaredView>,
}

impl ModuleViews {
    pub fn new(module: impl Into<String>, views: Vec<DeclaredView>) -> Self {
        Self {
            module: module.into(),
            views,
        }
    }
}

/// A view, named by its module and its name.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct ViewRef {
    pub module: String,
    pub view: String,
}

impl ViewRef {
    pub fn new(module: impl Into<String>, view: impl Into<String>) -> Self {
        Self {
            module: module.into(),
            view: view.into(),
        }
    }
}

impl std::fmt::Display for ViewRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}:{}", self.module, self.view)
    }
}

/// An entry the host draws.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    /// The identifier shared by the app (the view name).
    pub id: String,
    /// The view that is drawn.
    pub source: ViewRef,
    pub placement: Placement,
    /// The declared title, or the view name when none is declared.
    pub title: String,
    /// The view this entry stands in for, when it is a replacement.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub replaces: Option<ViewRef>,
}

/// Why a declared view is not drawn as declared.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Reason {
    /// Its module is declared but not loaded (not installed, disabled, failed).
    ModuleNotLoaded,
    /// The module was declared twice; the second declaration is ignored as a whole.
    DuplicateModule,
    /// Another view replaces it.
    Replaced { by: ViewRef },
    /// Its `replaces` names a view that is not there (module absent or not loaded, or no
    /// such view). The view is still shown as a plain entry.
    ReplaceTargetMissing { target: String, module_loaded: bool },
    /// Its target is already replaced by another view.
    AlreadyReplaced { target: ViewRef, by: ViewRef },
    /// Its replacements loop back to itself.
    ReplaceCycle,
    /// Its identifier is already held by a view of another module (or the same one).
    IdTaken { id: String, held_by: ViewRef },
    /// It replaces a view that is itself not drawn (refused), so it has no slot.
    TargetNotDrawn { target: ViewRef },
}

/// A declared view that is not drawn as declared, and why.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Omission {
    pub view: ViewRef,
    pub reason: Reason,
    /// `true` when the view is drawn anyway (only [`Reason::ReplaceTargetMissing`]).
    pub shown: bool,
}

impl Omission {
    /// A sentence for the person, in the app's language.
    pub fn message(&self) -> String {
        let v = &self.view;
        match &self.reason {
            Reason::ModuleNotLoaded => {
                format!(
                    "« {} » n'est pas affiché : le module « {} » n'est pas chargé.",
                    v.view, v.module
                )
            }
            Reason::DuplicateModule => {
                format!(
                    "Le module « {} » est déclaré deux fois : la seconde déclaration est ignorée.",
                    v.module
                )
            }
            Reason::Replaced { by } => format!("« {v} » est remplacé par « {by} »."),
            Reason::ReplaceTargetMissing {
                target,
                module_loaded,
            } => {
                if *module_loaded {
                    format!(
                        "« {v} » devait remplacer « {target} », qui n'existe pas : il est affiché seul."
                    )
                } else {
                    format!(
                        "« {v} » devait remplacer « {target} », dont le module n'est pas chargé : il est affiché seul."
                    )
                }
            }
            Reason::AlreadyReplaced { target, by } => {
                format!("« {v} » est refusé : « {target} » est déjà remplacé par « {by} ».")
            }
            Reason::ReplaceCycle => {
                format!("« {v} » est refusé : ses remplacements forment une boucle.")
            }
            Reason::IdTaken { id, held_by } => {
                format!(
                    "« {v} » est refusé : l'identifiant « {id} » appartient déjà à « {held_by} »."
                )
            }
            Reason::TargetNotDrawn { target } => {
                format!(
                    "« {v} » n'est pas affiché : la vue qu'il remplace, « {target} », est refusée."
                )
            }
        }
    }
}

/// What the host draws, and what it left out.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Menu {
    /// Every drawn view, in the documented order, all placements together.
    pub entries: Vec<Entry>,
    /// Every declared view not drawn as declared, in the documented order.
    pub omissions: Vec<Omission>,
}

impl Menu {
    /// The entries of one placement, in order.
    pub fn slot(&self, placement: Placement) -> impl Iterator<Item = &Entry> {
        self.entries
            .iter()
            .filter(move |e| e.placement == placement)
    }

    /// The drawer and header entries: the menu proper.
    pub fn menu_entries(&self) -> impl Iterator<Item = &Entry> {
        self.entries.iter().filter(|e| e.placement.is_menu())
    }

    pub fn entry(&self, id: &str) -> Option<&Entry> {
        self.entries.iter().find(|e| e.id == id)
    }

    /// Why a view is not drawn as declared, if it is not.
    pub fn why(&self, view: &ViewRef) -> Option<&Omission> {
        self.omissions.iter().find(|o| &o.view == view)
    }

    /// The omissions that hide a view (everything except a shown replacement miss).
    pub fn hidden(&self) -> impl Iterator<Item = &Omission> {
        self.omissions.iter().filter(|o| !o.shown)
    }
}

/// Builds the menu. `loaded` names the modules that are loaded now; a module present in
/// `declarations` but absent from `loaded` contributes no entry, only omissions.
pub fn build_menu<'a, I>(declarations: &[ModuleViews], loaded: I) -> Menu
where
    I: IntoIterator<Item = &'a str>,
{
    let loaded: BTreeSet<&str> = loaded.into_iter().collect();
    let mut menu = Menu::default();

    // 1. modules by name, the first declaration of a module wins, the rest is reported
    let mut modules: BTreeMap<&str, &ModuleViews> = BTreeMap::new();
    let mut duplicates: Vec<&ModuleViews> = Vec::new();
    for decl in declarations {
        if modules.contains_key(decl.module.as_str()) {
            duplicates.push(decl);
        } else {
            modules.insert(decl.module.as_str(), decl);
        }
    }

    // 2. the declared views in order; views of modules not loaded are reported
    let mut ordered: Vec<(ViewRef, &DeclaredView)> = Vec::new();
    for (module, decl) in &modules {
        for view in &decl.views {
            let r = ViewRef::new(*module, view.name.as_str());
            if loaded.contains(module) {
                ordered.push((r, view));
            } else {
                menu.omissions.push(Omission {
                    view: r,
                    reason: Reason::ModuleNotLoaded,
                    shown: false,
                });
            }
        }
    }
    let mut duplicates_sorted = duplicates;
    duplicates_sorted.sort_by(|a, b| a.module.cmp(&b.module));
    for decl in duplicates_sorted {
        for view in &decl.views {
            menu.omissions.push(Omission {
                view: ViewRef::new(decl.module.as_str(), view.name.as_str()),
                reason: Reason::DuplicateModule,
                shown: false,
            });
        }
    }

    let present: BTreeSet<&ViewRef> = ordered.iter().map(|(r, _)| r).collect();
    let declared_modules: BTreeSet<&str> = modules.keys().copied().collect();

    // 3. resolve replacements: target -> replacer (first one wins)
    let mut replaced_by: BTreeMap<ViewRef, ViewRef> = BTreeMap::new();
    let mut refused: BTreeSet<ViewRef> = BTreeSet::new();
    let mut unresolved: BTreeSet<ViewRef> = BTreeSet::new();
    for (r, view) in &ordered {
        let Some(raw) = view.replaces.as_deref() else {
            continue;
        };
        let target = match raw.split_once(':') {
            Some((module, name)) => ViewRef::new(module, name),
            None => ViewRef::new(r.module.as_str(), raw),
        };
        if !present.contains(&target) || &target == r {
            let module_loaded = loaded.contains(target.module.as_str())
                && declared_modules.contains(target.module.as_str());
            menu.omissions.push(Omission {
                view: r.clone(),
                reason: Reason::ReplaceTargetMissing {
                    target: raw.to_string(),
                    module_loaded,
                },
                shown: true,
            });
            unresolved.insert(r.clone());
            continue;
        }
        if let Some(by) = replaced_by.get(&target) {
            menu.omissions.push(Omission {
                view: r.clone(),
                reason: Reason::AlreadyReplaced {
                    target: target.clone(),
                    by: by.clone(),
                },
                shown: false,
            });
            refused.insert(r.clone());
            continue;
        }
        replaced_by.insert(target, r.clone());
    }

    // cycles: every view on a loop of replacements is refused, and its edges removed
    let mut cyclic: BTreeSet<ViewRef> = BTreeSet::new();
    for start in replaced_by.keys() {
        let mut seen: Vec<&ViewRef> = vec![start];
        let mut cur = start;
        while let Some(next) = replaced_by.get(cur) {
            if let Some(pos) = seen.iter().position(|s| *s == next) {
                cyclic.extend(seen[pos..].iter().map(|s| (*s).clone()));
                break;
            }
            seen.push(next);
            cur = next;
        }
    }
    if !cyclic.is_empty() {
        replaced_by.retain(|target, by| !cyclic.contains(target) && !cyclic.contains(by));
        for (r, _) in &ordered {
            if cyclic.contains(r) {
                menu.omissions.push(Omission {
                    view: r.clone(),
                    reason: Reason::ReplaceCycle,
                    shown: false,
                });
                refused.insert(r.clone());
            }
        }
    }

    // a replacer that was accepted only shows through its target's slot
    let replacers: BTreeSet<&ViewRef> = replaced_by.values().collect();
    let by_ref: BTreeMap<&ViewRef, &DeclaredView> = ordered.iter().map(|(r, v)| (r, *v)).collect();

    // 4. lay out slots, then claim identifiers in that order
    let mut holders: BTreeMap<String, ViewRef> = BTreeMap::new();
    for (r, _) in &ordered {
        if refused.contains(r) || (replacers.contains(r) && !unresolved.contains(r)) {
            continue;
        }
        // follow the replacement chain to the view that is finally drawn here
        let mut drawn = r.clone();
        let mut stands_for: Option<ViewRef> = None;
        while let Some(by) = replaced_by.get(&drawn) {
            menu.omissions.push(Omission {
                view: drawn.clone(),
                reason: Reason::Replaced { by: by.clone() },
                shown: false,
            });
            stands_for = Some(drawn.clone());
            drawn = by.clone();
        }
        let Some(view) = by_ref.get(&drawn) else {
            continue;
        };
        let id = view.name.clone();
        // a replacement may reuse the identifier of what it replaces
        if let Some(holder) = holders.get(&id) {
            menu.omissions.push(Omission {
                view: drawn.clone(),
                reason: Reason::IdTaken {
                    id,
                    held_by: holder.clone(),
                },
                shown: false,
            });
            continue;
        }
        holders.insert(id.clone(), drawn.clone());
        menu.entries.push(Entry {
            id,
            title: view.title.clone().unwrap_or_else(|| view.name.clone()),
            placement: view.surface,
            source: drawn,
            replaces: stands_for,
        });
    }

    // 5. nothing is dropped silently: a replacer whose target never got a slot
    for (r, _) in &ordered {
        let drawn = menu.entries.iter().any(|e| &e.source == r);
        let reported = menu.omissions.iter().any(|o| &o.view == r);
        if !drawn && !reported {
            let target = replaced_by
                .iter()
                .find(|(_, by)| *by == r)
                .map(|(t, _)| t.clone())
                .unwrap_or_else(|| r.clone());
            menu.omissions.push(Omission {
                view: r.clone(),
                reason: Reason::TargetNotDrawn { target },
                shown: false,
            });
        }
    }

    menu
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(name: &str, p: Placement) -> DeclaredView {
        DeclaredView::new(name, p)
    }

    fn ids(menu: &Menu) -> Vec<String> {
        menu.entries
            .iter()
            .map(|e| format!("{}", e.source))
            .collect()
    }

    #[test]
    fn entries_come_from_declared_views_in_module_then_declaration_order() {
        let decls = vec![
            ModuleViews::new(
                "notes",
                vec![
                    v("notes", Placement::Sidebar),
                    v("tags", Placement::Sidebar),
                ],
            ),
            ModuleViews::new(
                "chat",
                vec![
                    v("conversations", Placement::Sidebar),
                    v("status", Placement::Header),
                ],
            ),
        ];
        let menu = build_menu(&decls, ["notes", "chat"]);
        assert_eq!(
            ids(&menu),
            [
                "chat:conversations",
                "chat:status",
                "notes:notes",
                "notes:tags"
            ]
        );
        assert!(menu.omissions.is_empty());
        let drawer: Vec<_> = menu
            .slot(Placement::Sidebar)
            .map(|e| e.id.as_str())
            .collect();
        assert_eq!(drawer, ["conversations", "notes", "tags"]);
        assert_eq!(menu.slot(Placement::Header).count(), 1);
    }

    #[test]
    fn the_order_does_not_depend_on_the_callers_order() {
        let a = ModuleViews::new("a", vec![v("x", Placement::Sidebar)]);
        let b = ModuleViews::new("b", vec![v("y", Placement::Sidebar)]);
        let one = build_menu(&[a.clone(), b.clone()], ["a", "b"]);
        let two = build_menu(&[b, a], ["b", "a"]);
        assert_eq!(one, two);
    }

    #[test]
    fn a_view_of_a_module_not_loaded_is_hidden_with_its_reason() {
        let decls = vec![
            ModuleViews::new("chat", vec![v("conversations", Placement::Sidebar)]),
            ModuleViews::new("billing", vec![v("invoices", Placement::Sidebar)]),
        ];
        let menu = build_menu(&decls, ["chat"]);
        assert_eq!(ids(&menu), ["chat:conversations"]);
        let why = menu
            .why(&ViewRef::new("billing", "invoices"))
            .expect("reported");
        assert_eq!(why.reason, Reason::ModuleNotLoaded);
        assert!(!why.shown);
        assert!(why.message().contains("billing"), "{}", why.message());
        assert!(why.message().contains("invoices"), "{}", why.message());
    }

    #[test]
    fn the_title_falls_back_to_the_view_name() {
        let decls = vec![ModuleViews::new(
            "m",
            vec![
                v("a", Placement::Sidebar).titled("Alpha"),
                v("b", Placement::Sidebar),
            ],
        )];
        let menu = build_menu(&decls, ["m"]);
        assert_eq!(menu.entry("a").map(|e| e.title.as_str()), Some("Alpha"));
        assert_eq!(menu.entry("b").map(|e| e.title.as_str()), Some("b"));
    }

    #[test]
    fn a_replacement_in_the_same_module_takes_the_slot_of_its_target() {
        let decls = vec![ModuleViews::new(
            "m",
            vec![
                v("old", Placement::Sidebar),
                v("other", Placement::Sidebar),
                v("new", Placement::Sidebar).replacing("old"),
            ],
        )];
        let menu = build_menu(&decls, ["m"]);
        assert_eq!(ids(&menu), ["m:new", "m:other"]);
        assert_eq!(menu.entries[0].replaces, Some(ViewRef::new("m", "old")));
        assert_eq!(
            menu.why(&ViewRef::new("m", "old")).map(|o| &o.reason),
            Some(&Reason::Replaced {
                by: ViewRef::new("m", "new")
            })
        );
    }

    #[test]
    fn a_module_can_replace_a_view_of_another_module_and_reuse_its_id() {
        let decls = vec![
            ModuleViews::new(
                "chat",
                vec![
                    v("conversations", Placement::Sidebar),
                    v("search", Placement::Sidebar),
                ],
            ),
            ModuleViews::new(
                "better_chat",
                vec![v("conversations", Placement::Sidebar).replacing("chat:conversations")],
            ),
        ];
        let menu = build_menu(&decls, ["chat", "better_chat"]);
        assert_eq!(ids(&menu), ["better_chat:conversations", "chat:search"]);
        assert_eq!(
            menu.entry("conversations")
                .map(|e| e.source.module.as_str()),
            Some("better_chat")
        );
        assert!(
            menu.hidden()
                .all(|o| matches!(o.reason, Reason::Replaced { .. }))
        );
    }

    #[test]
    fn replacement_chains_follow_through() {
        let decls = vec![
            ModuleViews::new("a", vec![v("home", Placement::Main)]),
            ModuleViews::new("b", vec![v("home_b", Placement::Main).replacing("a:home")]),
            ModuleViews::new(
                "c",
                vec![v("home_c", Placement::Main).replacing("b:home_b")],
            ),
        ];
        let menu = build_menu(&decls, ["a", "b", "c"]);
        assert_eq!(ids(&menu), ["c:home_c"]);
        assert_eq!(menu.entries[0].replaces, Some(ViewRef::new("b", "home_b")));
        assert_eq!(menu.hidden().count(), 2);
    }

    #[test]
    fn a_second_replacement_of_the_same_target_is_refused() {
        let decls = vec![
            ModuleViews::new("a", vec![v("home", Placement::Main)]),
            ModuleViews::new("b", vec![v("b_home", Placement::Main).replacing("a:home")]),
            ModuleViews::new("c", vec![v("c_home", Placement::Main).replacing("a:home")]),
        ];
        let menu = build_menu(&decls, ["a", "b", "c"]);
        assert_eq!(ids(&menu), ["b:b_home"]);
        let why = menu.why(&ViewRef::new("c", "c_home")).expect("refused");
        assert_eq!(
            why.reason,
            Reason::AlreadyReplaced {
                target: ViewRef::new("a", "home"),
                by: ViewRef::new("b", "b_home")
            }
        );
    }

    #[test]
    fn a_replacement_whose_target_is_absent_is_shown_and_reported() {
        let decls = vec![
            ModuleViews::new(
                "a",
                vec![v("x", Placement::Sidebar).replacing("ghost:view")],
            ),
            ModuleViews::new("b", vec![v("y", Placement::Sidebar).replacing("nope")]),
            ModuleViews::new("off", vec![v("z", Placement::Sidebar)]),
            ModuleViews::new("c", vec![v("w", Placement::Sidebar).replacing("off:z")]),
        ];
        let menu = build_menu(&decls, ["a", "b", "c"]);
        assert_eq!(ids(&menu), ["a:x", "b:y", "c:w"]);
        let why = menu.why(&ViewRef::new("c", "w")).expect("reported");
        assert!(why.shown);
        assert_eq!(
            why.reason,
            Reason::ReplaceTargetMissing {
                target: "off:z".into(),
                module_loaded: false
            }
        );
        let why = menu.why(&ViewRef::new("b", "y")).expect("reported");
        assert_eq!(
            why.reason,
            Reason::ReplaceTargetMissing {
                target: "nope".into(),
                module_loaded: true
            }
        );
    }

    #[test]
    fn two_modules_claiming_the_same_id_is_refused_never_overwritten() {
        let decls = vec![
            ModuleViews::new("zeta", vec![v("inbox", Placement::Sidebar)]),
            ModuleViews::new(
                "alpha",
                vec![v("inbox", Placement::Sidebar).titled("Alpha inbox")],
            ),
        ];
        let menu = build_menu(&decls, ["zeta", "alpha"]);
        assert_eq!(
            ids(&menu),
            ["alpha:inbox"],
            "first in module order keeps it"
        );
        let why = menu.why(&ViewRef::new("zeta", "inbox")).expect("refused");
        assert_eq!(
            why.reason,
            Reason::IdTaken {
                id: "inbox".into(),
                held_by: ViewRef::new("alpha", "inbox")
            }
        );
        assert!(!why.shown);
        assert!(why.message().contains("alpha:inbox"), "{}", why.message());
    }

    #[test]
    fn replacement_cycles_are_refused() {
        let decls = vec![
            ModuleViews::new("a", vec![v("x", Placement::Sidebar).replacing("b:y")]),
            ModuleViews::new("b", vec![v("y", Placement::Sidebar).replacing("a:x")]),
            ModuleViews::new("c", vec![v("z", Placement::Sidebar)]),
        ];
        let menu = build_menu(&decls, ["a", "b", "c"]);
        assert_eq!(ids(&menu), ["c:z"]);
        assert_eq!(
            menu.why(&ViewRef::new("a", "x")).map(|o| &o.reason),
            Some(&Reason::ReplaceCycle)
        );
        assert_eq!(
            menu.why(&ViewRef::new("b", "y")).map(|o| &o.reason),
            Some(&Reason::ReplaceCycle)
        );
    }

    #[test]
    fn a_module_declared_twice_keeps_its_first_declaration() {
        let decls = vec![
            ModuleViews::new("m", vec![v("first", Placement::Sidebar)]),
            ModuleViews::new("m", vec![v("second", Placement::Sidebar)]),
        ];
        let menu = build_menu(&decls, ["m"]);
        assert_eq!(ids(&menu), ["m:first"]);
        assert_eq!(
            menu.why(&ViewRef::new("m", "second")).map(|o| &o.reason),
            Some(&Reason::DuplicateModule)
        );
    }

    #[test]
    fn descriptor_view_json_is_read_as_is() {
        // the shape of `this_descriptor::ViewDesc`, `root` and `look` included
        let raw = r#"[
            { "name": "conversations", "surface": "sidebar", "title": "Conversations",
              "look": { "accent": "primary" },
              "root": { "type": "leaf", "brick": { "type": "list", "entity": "conversation" } } },
            { "name": "home", "root": { "type": "leaf", "brick": { "type": "text", "text": "hi" } } },
            { "name": "home2", "surface": "main", "replaces": "home",
              "root": { "type": "leaf", "brick": { "type": "text", "text": "hi" } } }
        ]"#;
        let views: Vec<DeclaredView> = serde_json::from_str(raw).expect("descriptor views");
        assert_eq!(views[0].surface, Placement::Sidebar);
        assert_eq!(
            views[1].surface,
            Placement::Main,
            "default surface is main, as in the descriptor"
        );
        let menu = build_menu(&[ModuleViews::new("chat", views)], ["chat"]);
        assert_eq!(
            menu.menu_entries()
                .map(|e| e.id.as_str())
                .collect::<Vec<_>>(),
            ["conversations"]
        );
        assert_eq!(
            menu.entry("home2").and_then(|e| e.replaces.clone()),
            Some(ViewRef::new("chat", "home"))
        );
    }

    #[test]
    fn replacing_a_refused_view_is_reported_not_lost() {
        // b:y is refused (a:x already replaced by a:w); c:z replaces b:y
        let decls = vec![
            ModuleViews::new(
                "a",
                vec![
                    v("x", Placement::Main),
                    v("w", Placement::Main).replacing("x"),
                ],
            ),
            ModuleViews::new("b", vec![v("y", Placement::Main).replacing("a:x")]),
            ModuleViews::new("c", vec![v("z", Placement::Main).replacing("b:y")]),
        ];
        let menu = build_menu(&decls, ["a", "b", "c"]);
        assert_eq!(ids(&menu), ["a:w"]);
        for r in [ViewRef::new("b", "y"), ViewRef::new("c", "z")] {
            assert!(menu.why(&r).is_some(), "{r} must be reported");
        }
    }

    #[test]
    fn every_omission_has_a_message_for_the_person() {
        let decls = vec![
            ModuleViews::new("a", vec![v("x", Placement::Sidebar).replacing("b:y")]),
            ModuleViews::new("b", vec![v("y", Placement::Sidebar).replacing("a:x")]),
            ModuleViews::new("c", vec![v("dup", Placement::Sidebar)]),
            ModuleViews::new(
                "d",
                vec![
                    v("dup", Placement::Sidebar),
                    v("r", Placement::Sidebar).replacing("ghost"),
                ],
            ),
            ModuleViews::new("off", vec![v("o", Placement::Sidebar)]),
        ];
        let menu = build_menu(&decls, ["a", "b", "c", "d"]);
        assert!(menu.omissions.len() >= 5);
        for o in &menu.omissions {
            let m = o.message();
            assert!(!m.is_empty() && m.contains(&o.view.module), "{m}");
        }
    }
}
