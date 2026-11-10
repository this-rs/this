//! # Where a view goes, and what it hides
//!
//! Two rules of the contract are decided here, before a single element exists:
//!
//! - **Replacement.** A view that declares `replaces` takes the place of the view it
//!   names; the replaced one is drawn nowhere. A chain (`c` replaces `a`, `a` replaces
//!   `b`) leaves only `c`. Two views replacing the same target is an error the descriptor
//!   check refuses; when it happens anyway the first in declaration order wins and a
//!   warning names it, so the render never depends on iteration order.
//! - **Placement.** The four surfaces only change the *frame* ([`Frame`]); a brick renders
//!   identically wherever it sits. That is why a module can move a list from the sidebar to
//!   the main area without touching its tree.
//!
//! Everything in this module is pure: no DOM, no reaction, no data.

use this_descriptor::ModuleDescriptor;
use this_descriptor::views::{Surface, ViewDesc};

use crate::views::text;

/// The order the four surfaces are drawn in: a sidebar first, then the banner, the main
/// area and the panel. Fixed, so two runs never disagree.
pub const SURFACES: [Surface; 5] = [
    Surface::Sidebar,
    Surface::Header,
    Surface::Main,
    Surface::Panel,
    Surface::Chat,
];

/// A view that is not drawn because another one replaced it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hidden {
    /// The replaced view.
    pub name: String,
    /// The view drawn in its place.
    pub by: String,
}

/// The views of a module, after replacement, in declaration order.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Portfolio {
    shown: Vec<ViewDesc>,
    hidden: Vec<Hidden>,
    warnings: Vec<String>,
}

impl Portfolio {
    /// Decide what is drawn. Never fails: a contradictory replacement is a warning and a
    /// deterministic choice, not a panic.
    pub fn of(views: &[ViewDesc]) -> Self {
        let mut replaced_by: Vec<(String, String)> = Vec::new();
        let mut warnings = Vec::new();
        for view in views {
            let Some(target) = view.replaces.as_deref() else {
                continue;
            };
            if target == view.name {
                warnings.push(format!("`{}` replaces itself; it is drawn", view.name));
                continue;
            }
            match replaced_by.iter_mut().find(|(name, _)| name == target) {
                Some((_, first)) => warnings.push(format!(
                    "`{target}` is replaced twice (`{first}`, `{}`); `{first}` wins",
                    view.name
                )),
                None => replaced_by.push((target.to_owned(), view.name.clone())),
            }
        }
        let mut shown = Vec::new();
        let mut hidden = Vec::new();
        for view in views {
            match replaced_by.iter().find(|(name, _)| name == &view.name) {
                Some((_, by)) => hidden.push(Hidden {
                    name: view.name.clone(),
                    by: by.clone(),
                }),
                None => shown.push(view.clone()),
            }
        }
        Self {
            shown,
            hidden,
            warnings,
        }
    }

    /// The views to draw, in declaration order.
    pub fn shown(&self) -> &[ViewDesc] {
        &self.shown
    }

    /// The views this one hid.
    pub fn hidden(&self) -> &[Hidden] {
        &self.hidden
    }

    /// Replacement contradictions, said rather than swallowed.
    pub fn warnings(&self) -> &[String] {
        &self.warnings
    }

    /// Nothing to draw at all.
    pub fn is_empty(&self) -> bool {
        self.shown.is_empty()
    }

    /// The views of one surface, in declaration order.
    pub fn at(&self, surface: Surface) -> Vec<&ViewDesc> {
        self.shown
            .iter()
            .filter(|view| view.surface == surface)
            .collect()
    }

    /// The drawn view of that name, if it survived replacement.
    pub fn view(&self, name: &str) -> Option<&ViewDesc> {
        self.shown.iter().find(|view| view.name == name)
    }

    /// The surfaces that hold at least one view, in drawing order.
    pub fn occupied(&self) -> Vec<Surface> {
        SURFACES
            .into_iter()
            .filter(|surface| !self.at(*surface).is_empty())
            .collect()
    }
}

/// The element that frames a surface. Only the frame changes with the placement.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Frame {
    /// The element name (`nav`, `header`, `main`, `aside`).
    pub tag: &'static str,
    /// The class that styles the frame.
    pub class: &'static str,
    /// A landmark role, when the element does not already carry one.
    pub role: Option<&'static str>,
    /// The accessible name of the landmark.
    pub label: &'static str,
}

/// The frame of one surface.
pub fn frame(surface: Surface) -> Frame {
    match surface {
        Surface::Sidebar => Frame {
            tag: "nav",
            class: "vw-surface vw-sidebar",
            role: None,
            label: "Navigation du module",
        },
        Surface::Chat => Frame {
            tag: "section",
            class: "vw-surface vw-chat",
            role: None,
            label: "Conversation du module",
        },
        Surface::Header => Frame {
            tag: "header",
            class: "vw-surface vw-header",
            role: Some("banner"),
            label: "En-tête du module",
        },
        Surface::Main => Frame {
            tag: "main",
            class: "vw-surface vw-main",
            role: None,
            label: "Contenu principal",
        },
        Surface::Panel => Frame {
            tag: "aside",
            class: "vw-surface vw-panel",
            role: Some("complementary"),
            label: "Panneau latéral",
        },
        // Surfaces d'un autre hôte : l'app les montre en aperçu, sans prétendre les rendre.
        Surface::Widget => Frame {
            tag: "section",
            class: "vw-surface vw-host vw-widget",
            role: None,
            label: "Widget (aperçu)",
        },
        Surface::LockWidget => Frame {
            tag: "section",
            class: "vw-surface vw-host vw-lock-widget",
            role: None,
            label: "Widget d'écran verrouillé (aperçu)",
        },
        Surface::Activity => Frame {
            tag: "section",
            class: "vw-surface vw-host vw-activity",
            role: None,
            label: "Activité (aperçu)",
        },
        Surface::Island => Frame {
            tag: "section",
            class: "vw-surface vw-host vw-island",
            role: None,
            label: "Dynamic Island (aperçu)",
        },
    }
}

/// The title of a view, or its name made readable (`invoice_detail` → `Invoice detail`).
pub fn title(view: &ViewDesc) -> String {
    match view.title.as_deref() {
        Some(title) if !title.is_empty() => title.to_owned(),
        _ => humanise(&view.name),
    }
}

/// Whether the view draws its title as a heading.
///
/// A bar is not a page: on [`Surface::Header`] the view's name is an identifier the author
/// chose for the contract (`entete`), not something to read, and a heading in a one-line bar
/// takes the room the bar's own content needs. The title stays on the wrapper as its
/// accessible name (`aria-label`); only the visible heading is dropped.
pub fn shows_title(view: &ViewDesc) -> bool {
    // Une barre n'est pas une page : son titre est le nom du contrat qui la dessine
    // (« Entete »), pas une information. Un fil non plus : il se lit par ses messages, et
    // « Fil » y occupait la première ligne pour ne rien dire.
    !matches!(view.surface, Surface::Header | Surface::Chat)
}

/// Turn a contract identifier into something a person can read: underscores become
/// spaces, the first letter is capitalised. The contract has no label field, so this is
/// the best a brick can do with a bare name.
pub fn humanise(identifier: &str) -> String {
    let spaced = identifier.replace('_', " ");
    let trimmed = spaced.trim();
    if trimmed.is_empty() {
        return text::DASH.to_owned();
    }
    let mut chars = trimmed.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => text::DASH.to_owned(),
    }
}

/// What a button for `action` reads — one answer, for every place the host draws one (a
/// `button`, a `notice`'s gesture, a row action, a composer action, a form's submit, the
/// empty tile's create).
///
/// **Precedence**: the brick's own `label` (`brick_label`), then the `label` the module
/// declared on the action, then the action name [`humanise`]d. A declared label is interface
/// text and is shown as written — accents and all (`Épingler cette clé`); only a blank one
/// is ignored, so a button is never drawn without a word.
pub fn action_label(module: &ModuleDescriptor, action: &str, brick_label: Option<&str>) -> String {
    let declared = module
        .actions
        .iter()
        .find(|a| a.name() == action)
        .and_then(|a| a.label());
    [brick_label, declared]
        .into_iter()
        .flatten()
        .find(|label| !label.trim().is_empty())
        .map(str::to_owned)
        .unwrap_or_else(|| humanise(action))
}

#[cfg(test)]
mod tests {
    use super::*;
    use this_descriptor::views::Node;

    fn view(name: &str, surface: Surface, replaces: Option<&str>) -> ViewDesc {
        ViewDesc {
            name: name.into(),
            surface,
            title: None,
            replaces: replaces.map(str::to_owned),
            look: None,
            root: Node::Stack { children: vec![] },
        }
    }

    fn names(portfolio: &Portfolio) -> Vec<String> {
        portfolio
            .shown()
            .iter()
            .map(|view| view.name.clone())
            .collect()
    }

    /// Une barre et un fil ne portent pas de titre visible : le titre y serait le nom
    /// interne de la vue (« Entete », « Fil »). Une page et un panneau, si.
    #[test]
    fn only_a_page_or_a_panel_draws_a_heading() {
        assert!(!shows_title(&view("entete", Surface::Header, None)));
        assert!(!shows_title(&view("fil", Surface::Chat, None)));
        for surface in [Surface::Main, Surface::Sidebar, Surface::Panel] {
            assert!(shows_title(&view("detail", surface, None)), "{surface:?}");
        }
        // The accessible name survives: the title is still computed.
        assert_eq!(title(&view("entete", Surface::Header, None)), "Entete");
        assert_eq!(title(&view("fil", Surface::Chat, None)), "Fil");
    }

    #[test]
    fn a_replacing_view_hides_the_one_it_names() {
        let portfolio = Portfolio::of(&[
            view("detail", Surface::Main, None),
            view("detail_v2", Surface::Main, Some("detail")),
        ]);
        assert_eq!(names(&portfolio), vec!["detail_v2"]);
        assert_eq!(
            portfolio.hidden(),
            &[Hidden {
                name: "detail".into(),
                by: "detail_v2".into()
            }]
        );
        assert!(portfolio.warnings().is_empty());
    }

    #[test]
    fn a_replacement_chain_leaves_only_its_last_link() {
        let portfolio = Portfolio::of(&[
            view("a", Surface::Main, Some("b")),
            view("b", Surface::Main, Some("c")),
            view("c", Surface::Main, None),
        ]);
        assert_eq!(names(&portfolio), vec!["a"]);
        assert_eq!(portfolio.hidden().len(), 2);
    }

    #[test]
    fn two_views_replacing_the_same_target_hide_it_once_and_warn() {
        // The descriptor check refuses this shape; when it arrives anyway, the replaced
        // view is still drawn nowhere — once — and both replacements are drawn, in
        // declaration order, with a warning that names the contradiction.
        let portfolio = Portfolio::of(&[
            view("x", Surface::Main, Some("target")),
            view("y", Surface::Main, Some("target")),
            view("target", Surface::Main, None),
        ]);
        assert_eq!(names(&portfolio), vec!["x", "y"]);
        assert_eq!(portfolio.hidden().len(), 1);
        assert_eq!(portfolio.hidden()[0].name, "target");
        assert_eq!(portfolio.warnings().len(), 1);
        assert!(portfolio.warnings()[0].contains("replaced twice"));
    }

    #[test]
    fn a_view_replacing_itself_is_drawn_and_warned() {
        let portfolio = Portfolio::of(&[view("solo", Surface::Main, Some("solo"))]);
        assert_eq!(names(&portfolio), vec!["solo"]);
        assert_eq!(portfolio.warnings().len(), 1);
    }

    #[test]
    fn the_order_of_the_drawn_views_is_the_declaration_order() {
        let portfolio = Portfolio::of(&[
            view("z", Surface::Main, None),
            view("hidden", Surface::Main, None),
            view("replacement", Surface::Main, Some("hidden")),
            view("a", Surface::Main, None),
        ]);
        assert_eq!(names(&portfolio), vec!["z", "replacement", "a"]);
    }

    #[test]
    fn a_surface_only_holds_its_own_views() {
        let portfolio = Portfolio::of(&[
            view("menu", Surface::Sidebar, None),
            view("bar", Surface::Header, None),
            view("body", Surface::Main, None),
            view("side", Surface::Panel, None),
        ]);
        assert_eq!(portfolio.at(Surface::Sidebar).len(), 1);
        assert_eq!(portfolio.at(Surface::Header).len(), 1);
        assert_eq!(portfolio.at(Surface::Main).len(), 1);
        assert_eq!(portfolio.at(Surface::Panel).len(), 1);
        assert_eq!(
            portfolio.occupied(),
            vec![
                Surface::Sidebar,
                Surface::Header,
                Surface::Main,
                Surface::Panel
            ]
        );
        assert!(Portfolio::of(&[]).is_empty());
    }

    #[test]
    fn each_surface_has_its_own_frame_and_a_name() {
        for surface in SURFACES {
            let frame = frame(surface);
            assert!(frame.class.starts_with("vw-surface"));
            assert!(!frame.label.is_empty());
        }
        assert_eq!(frame(Surface::Main).tag, "main");
        assert_eq!(frame(Surface::Sidebar).tag, "nav");
        assert_eq!(frame(Surface::Header).tag, "header");
        assert_eq!(frame(Surface::Panel).tag, "aside");
        assert_eq!(frame(Surface::Header).role, Some("banner"));
        assert_eq!(frame(Surface::Sidebar).role, None);
        assert_ne!(frame(Surface::Main).class, frame(Surface::Panel).class);
    }

    #[test]
    fn a_title_falls_back_to_the_readable_name() {
        let mut with_title = view("invoices", Surface::Main, None);
        with_title.title = Some("Factures".into());
        assert_eq!(title(&with_title), "Factures");
        assert_eq!(
            title(&view("invoice_detail", Surface::Main, None)),
            "Invoice detail"
        );
        assert_eq!(humanise(""), text::DASH);
    }

    fn module_with(actions: Vec<this_descriptor::Action>) -> ModuleDescriptor {
        let mut m = ModuleDescriptor::new("keys", "1.0.0");
        m.actions = actions;
        m
    }

    fn pin(label: Option<&str>) -> this_descriptor::Action {
        this_descriptor::Action::Navigate {
            name: "epingler_cle".into(),
            label: label.map(str::to_owned),
            view: "keys".into(),
            module: None,
        }
    }

    #[test]
    fn a_label_wins_over_the_actions_which_wins_over_the_humanised_name() {
        let with = module_with(vec![pin(Some("Épingler cette clé"))]);
        let without = module_with(vec![pin(None)]);
        // brick > action > humanised name
        assert_eq!(
            action_label(&with, "epingler_cle", Some("Garder")),
            "Garder"
        );
        assert_eq!(
            action_label(&with, "epingler_cle", None),
            "Épingler cette clé"
        );
        assert_eq!(action_label(&without, "epingler_cle", None), "Epingler cle");
        // An action the module never declared still reads something.
        assert_eq!(action_label(&without, "ghost_action", None), "Ghost action");
        // A blank label never draws a wordless button: it falls through.
        assert_eq!(
            action_label(&with, "epingler_cle", Some("  ")),
            "Épingler cette clé"
        );
    }
}
