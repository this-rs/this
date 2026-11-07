//! # The view contract, rendered
//!
//! `this-descriptor::views` says what a module wants to show: a tree of containers, bricks
//! and named actions. This module is the other half — the one that puts it on a screen.
//!
//! ```ignore
//! let data = RwSignal::new(Snapshot::new().with("invoice", Feed::Ready(rows)));
//! view! {
//!     <Views
//!         descriptor=descriptor
//!         data=data
//!         on_intent=Callback::new(|intent: Intent| { /* the host decides */ })
//!     />
//! }
//! ```
//!
//! ## It draws, it does not act
//!
//! A click produces an [`Intent`]; executing it is the host's business. `this-ui` opens no
//! connection, writes no storage, calls no native function — which is why the same
//! rendering serves a server-backed app, a Tauri shell and a test, and why a module's
//! interface can be moved between them without being rewritten.
//!
//! ## What the host must supply
//!
//! - a [`ModuleDescriptor`] (the views and the declared actions come from it);
//! - a [`Snapshot`]: one [`Feed`] per entity, plus `online`. The host owns fetching, and
//!   says why something failed or was refused;
//! - a `Callback<Intent>`.
//!
//! ## What is decided here, and is testable without a DOM
//!
//! - [`Portfolio`] — which views are drawn after `replaces`, in which order, on which
//!   surface ([`placement`]);
//! - [`Snapshot`] reading — filter, sort and page, with the server's own filter semantics
//!   ([`data`]);
//! - [`ViewState`] — loading, empty, failed, refused, missing ([`state`]);
//! - [`CSS`] — the sheet that makes clickable zones 44 px and focus visible ([`style`]).
//!
//! Everything else is Leptos views; the tests that need a DOM are left to the host.

pub mod data;
#[cfg(test)]
mod integration;
pub mod intent;
pub mod placement;
pub mod render;
pub mod state;
pub mod style;
pub mod text;

pub use data::{Feed, Feeds, Reading, Rows, Snapshot};
pub use intent::{FieldValue, Intent, LinkOp, Values};
pub use placement::{Frame, Hidden, Portfolio, SURFACES};
pub use render::{Ctx, render_brick, render_node, render_view};
pub use state::ViewState;
pub use style::CSS;

use leptos::prelude::*;
use this_descriptor::ModuleDescriptor;
use this_descriptor::views::{Surface, ViewDesc};

use crate::views::placement::frame;
use crate::views::style::class;

/// Every surface of a module, in the fixed order, with replacements already applied.
///
/// A view declaring `replaces` is drawn, the view it names is not — once, and in the
/// declaration order of the module, so two runs never disagree.
///
/// The descriptor is read once per mount: a host that swaps the module remounts this
/// component rather than mutating a prop.
#[component]
pub fn Views(
    descriptor: ModuleDescriptor,
    #[prop(into)] data: Signal<Snapshot>,
    on_intent: Callback<Intent>,
) -> impl IntoView {
    let portfolio = Portfolio::of(&descriptor.views);
    let module = StoredValue::new(descriptor);
    let regions = portfolio
        .occupied()
        .into_iter()
        .map(|surface| {
            let views: Vec<ViewDesc> = portfolio.at(surface).into_iter().cloned().collect();
            surface_frame(surface, module, data, on_intent, views)
        })
        .collect_view();
    let nothing = portfolio
        .is_empty()
        .then(|| view! { <p class=class::EMPTY>{text::NOTHING}</p> }.into_any());
    view! {
        <>
            {regions}
            {nothing}
        </>
    }
    .into_any()
}

/// One view, drawn by the host where it wants it (inside its own panel, its own page…).
#[component]
pub fn ViewBody(
    descriptor: ModuleDescriptor,
    view: ViewDesc,
    #[prop(default = Surface::Main)] surface: Surface,
    #[prop(into)] data: Signal<Snapshot>,
    on_intent: Callback<Intent>,
) -> impl IntoView {
    let module = StoredValue::new(descriptor);
    let name = view.name.clone();
    let ctx = Ctx::new(module, data, on_intent, surface, name);
    render_view(ctx, &view)
}

/// The sheet the renderer needs: 44 px targets, a visible focus ring, reduced motion.
pub fn css() -> &'static str {
    style::CSS
}

fn surface_frame(
    surface: Surface,
    module: StoredValue<ModuleDescriptor>,
    data: Signal<Snapshot>,
    on_intent: Callback<Intent>,
    views: Vec<ViewDesc>,
) -> AnyView {
    let frame = frame(surface);
    let body = views
        .into_iter()
        .map(|view| {
            let ctx = Ctx::new(module, data, on_intent, surface, view.name.clone());
            render_view(ctx, &view)
        })
        .collect_view();
    let class_name = frame.class;
    let label = frame.label;
    match surface {
        Surface::Sidebar => view! {
            <nav class=class_name aria-label=label>{body}</nav>
        }
        .into_any(),
        Surface::Header => view! {
            <header class=class_name aria-label=label>{body}</header>
        }
        .into_any(),
        Surface::Main => view! {
            <main class=class_name aria-label=label>{body}</main>
        }
        .into_any(),
        Surface::Panel => view! {
            <aside class=class_name aria-label=label>{body}</aside>
        }
        .into_any(),
        // Les surfaces d'un autre hôte : l'app les montre comme un aperçu neutre. Ce n'est
        // pas là qu'elles se dessinent — le widget et l'activité sont peints par l'OS, dans
        // un autre processus, à partir des mêmes briques.
        Surface::Widget | Surface::LockWidget | Surface::Activity | Surface::Island => view! {
            <section class=class_name aria-label=label>{body}</section>
        }
        .into_any(),
    }
}
