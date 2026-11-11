//! # Drawing a view: containers, bricks, and the four states of a missing row
//!
//! [`render_view`] takes a [`ViewDesc`] and produces the interface. Nothing else: no fetch,
//! no storage, no global state. What it needs, the host passes in a [`Ctx`]; what it wants
//! done, it emits as an [`Intent`].
//!
//! Three rules are visible in the code below:
//!
//! - **No `unwrap`, no panic, no empty screen.** A brick whose variant or whose arguments
//!   the renderer cannot honour draws a [`fallback`] that names what is missing. A reading
//!   that is not rows draws its own tile (loading, empty, failed, refused).
//! - **The frame is the surface's business.** A brick does not know whether it sits in a
//!   sidebar or in the header; only the wrapper differs ([`placement::frame`]).
//! - **A label always exists.** The contract carries no label for an action or a field, so
//!   [`placement::humanise`] turns an identifier into something readable and the raw name
//!   stays in `title` and `data-*` for the host's diagnostics.

use std::sync::atomic::{AtomicUsize, Ordering};

use leptos::prelude::*;
use serde_json::Value;
use this_descriptor::views::{
    Action, ArgKind, Brick, Click, Column, ColumnSource, DeviceCapability, FormField, Grant, Input,
    Node, ProgramArg, RelationMode, Run, SettingsSection, Signal as IndicatorSignal, Surface, Tab,
    Tone, ViewDesc,
};
use this_descriptor::{FilterExpr, ModuleDescriptor, SortSpec};
use wasm_bindgen::JsCast;

use crate::design::{self, Tone as DesignTone};
use crate::model::Kind as FieldKind;
use crate::views::chart;
use crate::views::data::{self, Feed, Reading, Snapshot};
use crate::views::image;
use crate::views::intent::{FieldValue, Intent, LinkOp, SecretValue, Values};
use crate::views::markdown;
use crate::views::placement;
use crate::views::state::{self, ViewState};
use crate::views::style::{self, class};
use crate::views::text;

/// Everything a brick needs, and nothing that would let it act on its own.
#[derive(Clone, Copy)]
pub struct Ctx {
    /// The module, for the field types and the declared actions.
    pub module: StoredValue<ModuleDescriptor>,
    /// What the host knows right now.
    pub data: Signal<Snapshot>,
    /// Where an intention goes.
    pub on_intent: Callback<Intent>,
    /// Where the view is drawn.
    pub surface: Surface,
    /// The view being drawn; a `modal`'s close button names it in a `Dismiss`.
    pub view: StoredValue<String>,
}

impl Ctx {
    /// Build a context. The host normally gets one from [`Views`] or [`ViewBody`].
    pub fn new(
        module: StoredValue<ModuleDescriptor>,
        data: Signal<Snapshot>,
        on_intent: Callback<Intent>,
        surface: Surface,
        view: impl Into<String>,
    ) -> Self {
        Self {
            module,
            data,
            on_intent,
            surface,
            view: StoredValue::new(view.into()),
        }
    }

    /// What a button for `action` reads: see [`placement::action_label`].
    fn label_of(&self, action: &str, brick_label: Option<&str>) -> String {
        self.module
            .try_with_value(|module| placement::action_label(module, action, brick_label))
            .unwrap_or_else(|| placement::humanise(action))
    }
}

/// A process-wide counter, so two controls on the same screen never share an `id`.
fn next_id(prefix: &str) -> String {
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    format!("{prefix}-{}", NEXT.fetch_add(1, Ordering::Relaxed))
}

// ---------------------------------------------------------------- entry points

/// Draw one view: its title, its tree, and — when the tree cannot be drawn — one block
/// that says why.
///
/// The tree is built **once** and hidden behind a gate while the data is unusable. That is
/// deliberate: a tab selection, a half-typed form or a chat draft must survive an update of
/// the rows, and rebuilding the tree on every change is exactly how an interface manages to
/// lose what someone was typing.
pub fn render_view(ctx: Ctx, view: &ViewDesc) -> AnyView {
    let title = placement::title(view);
    let heading = placement::shows_title(view).then(|| title.clone());
    let classes = view_classes(view);
    let label = title.clone();
    let root = view.root.clone();
    let gate_root = view.root.clone();
    let state = Memo::new(move |_| state::view_state(&gate_root, &ctx.data.get()));
    let tree = render_node(ctx, &root);
    view! {
        <section class=classes aria-label=label>
            {heading.map(|heading| view! { <h2 class=class::VIEW_TITLE>{heading}</h2> })}
            {move || gate_tile(ctx, state.get())}
            <div class=class::VIEW_BODY hidden=move || state.get() != ViewState::Ready>
                {tree}
            </div>
        </section>
    }
    .into_any()
}

/// The classes of the view wrapper: the declared look, nothing invented.
fn view_classes(view: &ViewDesc) -> String {
    let look = view.look.unwrap_or_default();
    format!(
        "{} {} {} {}",
        class::VIEW,
        style::accent(look.accent),
        style::layout(look.layout),
        style::transition(look.transition)
    )
}

/// The block a view shows instead of its tree, when it has no tree to show.
fn gate_tile(ctx: Ctx, state: ViewState) -> Option<AnyView> {
    match state {
        ViewState::Ready => None,
        ViewState::Loading => Some(loading_tile()),
        ViewState::Denied { entity, reason } => Some(denied_tile(ctx, &entity, &reason)),
        ViewState::Failed { entity, reason } => Some(failed_tile(ctx, &entity, &reason)),
        ViewState::Missing { entity } => Some(missing_tile(&entity)),
        ViewState::Empty { entity } => Some(empty_tile(
            ctx,
            entity.as_deref().unwrap_or_default(),
            text::NOTHING,
        )),
    }
}

/// Draw one node of the tree.
pub fn render_node(ctx: Ctx, node: &Node) -> AnyView {
    match node {
        Node::Stack { children } => grouped(ctx, children, class::STACK),
        Node::Row { children } => grouped(ctx, children, class::ROW),
        Node::Tabs { tabs } => tabbed(ctx, tabs),
        Node::Drawer { label, children } => drawer(ctx, label, children),
        Node::Modal { title, children } => modal(ctx, title, children),
        Node::Leaf { brick } => render_brick(ctx, brick),
        Node::Html { path, height, data } => super::html::render_html(ctx, path, *height, data),
        Node::When {
            entity,
            filter,
            child,
        } => conditional(ctx, entity, filter, child),
    }
}

/// A `when` node: the child is drawn only while the condition holds, evaluated against the
/// snapshot BEFORE drawing. The verdict is a `Memo` of a bool, so a snapshot that changes
/// without flipping the condition does not rebuild the child (a form keeps its focus).
///
/// False, absent or unreadable data draws nothing — never a button at random. An unreadable
/// condition (failed or refused read, entity the host never provided) is named in the log,
/// once per distinct cause, not on every snapshot.
fn conditional(ctx: Ctx, entity: &str, filter: &FilterExpr, child: &Node) -> AnyView {
    let entity = entity.to_owned();
    let filter = filter.clone();
    let child = child.clone();
    let named = StoredValue::new(None::<String>);
    let shown = Memo::new(move |_| {
        let verdict = ctx
            .data
            .with(|snapshot| data::condition(snapshot, &entity, &filter));
        let cause = match &verdict {
            data::Condition::Unreadable(why) => Some(why.clone()),
            _ => None,
        };
        if named.get_value() != cause {
            if let Some(why) = &cause {
                log_refusal(&format!(
                    "when `{entity}` : condition non évaluable, nœud masqué ({why})"
                ));
            }
            named.set_value(cause);
        }
        verdict.holds()
    });
    view! { {move || shown.get().then(|| render_node(ctx, &child))} }.into_any()
}

/// Name a refusal in the host's log: the browser console on the device, stderr elsewhere.
pub(crate) fn log_refusal(message: &str) {
    #[cfg(target_arch = "wasm32")]
    web_sys::console::warn_1(&message.into());
    #[cfg(not(target_arch = "wasm32"))]
    eprintln!("this-ui: {message}");
}

/// A line in the host's log: the browser console on the device, stderr elsewhere.
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) fn log_line(message: &str) {
    #[cfg(target_arch = "wasm32")]
    web_sys::console::log_1(&message.into());
    #[cfg(not(target_arch = "wasm32"))]
    eprintln!("this-ui: {message}");
}

fn grouped(ctx: Ctx, children: &[Node], class_name: &'static str) -> AnyView {
    let items = children
        .iter()
        .map(|child| render_node(ctx, child))
        .collect_view();
    view! { <div class=class_name>{items}</div> }.into_any()
}

fn tabbed(ctx: Ctx, tabs: &[Tab]) -> AnyView {
    if tabs.is_empty() {
        return fallback("`tabs` sans onglet");
    }
    let selected = RwSignal::new(0usize);
    let labels: Vec<String> = tabs.iter().map(|tab| tab.label.clone()).collect();
    let children: Vec<Vec<Node>> = tabs.iter().map(|tab| tab.children.clone()).collect();
    let head_id = next_id("vw-tablist");
    let buttons = labels
        .iter()
        .enumerate()
        .map(|(index, label)| {
            let tab_id = format!("{head_id}-{index}");
            let panel_id = format!("{head_id}-panel-{index}");
            let label = label.clone();
            view! {
                <button
                    class=class::TAB
                    role="tab"
                    id=tab_id
                    aria-controls=panel_id
                    aria-selected=move || (selected.get() == index).to_string()
                    on:click=move |_| selected.set(index)
                >
                    {label}
                </button>
            }
        })
        .collect_view();
    let panels: Vec<AnyView> = children
        .into_iter()
        .enumerate()
        .map(|(index, nodes)| {
            let tab_id = format!("{head_id}-{index}");
            let panel_id = format!("{head_id}-panel-{index}");
            view! {
                <div
                    class=class::TABPANEL
                    role="tabpanel"
                    id=panel_id
                    aria-labelledby=tab_id
                    hidden=move || (selected.get() != index).then_some("hidden")
                >
                    {nodes.iter().map(|node| render_node(ctx, node)).collect_view()}
                </div>
            }
            .into_any()
        })
        .collect();
    let panels = panels.into_iter().collect_view();
    view! {
        <div class=class::TABS>
            <div class=class::TABLIST role="tablist">{buttons}</div>
            {panels}
        </div>
    }
    .into_any()
}

fn drawer(ctx: Ctx, label: &str, children: &[Node]) -> AnyView {
    let items = children
        .iter()
        .map(|child| render_node(ctx, child))
        .collect_view();
    let summary = label.to_owned();
    view! {
        <details class=class::DRAWER>
            <summary>{summary}</summary>
            <div class=class::DRAWER_BODY>{items}</div>
        </details>
    }
    .into_any()
}

fn modal(ctx: Ctx, title: &str, children: &[Node]) -> AnyView {
    let items = children
        .iter()
        .map(|child| render_node(ctx, child))
        .collect_view();
    let heading = title.to_owned();
    let label = heading.clone();
    // La feuille s'ouvre dessinée ; la refermer (croix, fond, Échap) est un geste LOCAL du
    // rendu — la personne n'est jamais enfermée, même si le module n'a pas prévu de bouton.
    // L'hôte en est averti par `Dismiss`. L'état DÉCLARÉ (ouvert/fermé) vit dans la condition
    // d'un `when` qui enveloppe la feuille, fermée par une action du module.
    let open = RwSignal::new(true);
    let dismiss = move || {
        if let Some(intent) = dismiss_intent(&ctx.view.get_value(), open.get_untracked()) {
            open.set(false);
            ctx.on_intent.run(intent);
        }
    };
    let escape = window_event_listener(leptos::ev::keydown, move |ev: web_sys::KeyboardEvent| {
        if is_escape(&ev.key()) {
            dismiss();
        }
    });
    on_cleanup(move || escape.remove());
    view! {
        <div class=class::MODAL_LAYER hidden=move || (!open.get()).then_some("hidden")>
            <div class=class::MODAL_BACKDROP on:click=move |_| dismiss()></div>
            <section class=class::MODAL role="dialog" aria-modal="true" aria-label=label>
                <div class=class::MODAL_HEAD>
                    <h3>{heading}</h3>
                    <button class=class::ICON_BUTTON aria-label=text::CLOSE title=text::CLOSE
                        on:click=move |_| dismiss()>
                        <span aria-hidden="true">"×"</span>
                    </button>
                </div>
                <div class=class::MODAL_BODY>{items}</div>
            </section>
        </div>
    }
    .into_any()
}

/// L'intention qu'émet un geste de fermeture (croix, fond, Échap) : l'hôte est averti que la
/// feuille a été refermée. Une feuille déjà fermée n'émet rien — un second geste ne réveille
/// pas un `Dismiss` fantôme.
fn dismiss_intent(view: &str, was_open: bool) -> Option<Intent> {
    was_open.then(|| Intent::Dismiss {
        view: view.to_owned(),
    })
}

/// La touche qui referme une feuille : `Escape`. (Le clic sur le fond est un événement DOM
/// posé sur la couche, pas une décision testable sans navigateur ; la décision de la touche,
/// elle, l'est.)
fn is_escape(key: &str) -> bool {
    key == "Escape"
}

// ---------------------------------------------------------------- tiles

fn loading_tile() -> AnyView {
    view! {
        <div class=format!("{} {}", class::STATE, class::LOADING) role="status" aria-busy="true">
            <span>{text::LOADING}</span>
            <span class=class::SKELETON></span>
            <span class=class::SKELETON></span>
        </div>
    }
    .into_any()
}

fn failed_tile(ctx: Ctx, entity: &str, reason: &str) -> AnyView {
    let target = entity.to_owned();
    let message = reason.to_owned();
    view! {
        <div class=format!("{} {}", class::STATE, class::FAILED) role="alert">
            <strong>{message}</strong>
            <button class=class::BUTTON on:click=move |_| {
                ctx.on_intent.run(Intent::Reload { entity: target.clone() });
            }>{text::RETRY}</button>
        </div>
    }
    .into_any()
}

fn denied_tile(ctx: Ctx, entity: &str, reason: &str) -> AnyView {
    let target = entity.to_owned();
    let message = reason.to_owned();
    view! {
        <div class=format!("{} {}", class::STATE, class::DENIED) role="note">
            <strong>{text::DENIED}</strong>
            <span>{message}</span>
            <button class=class::BUTTON on:click=move |_| {
                ctx.on_intent.run(Intent::RequestAccess { entity: target.clone() });
            }>{text::ASK_ACCESS}</button>
        </div>
    }
    .into_any()
}

fn missing_tile(entity: &str) -> AnyView {
    view! {
        <div class=format!("{} {}", class::STATE, class::MISSING) role="note">
            <span>{format!("{} : `{entity}`", text::UNKNOWN_ENTITY)}</span>
        </div>
    }
    .into_any()
}

/// The empty block, with the create action when the module declares one for that entity:
/// an empty screen is the only place where "create the first one" makes sense.
fn empty_tile(ctx: Ctx, entity: &str, message: &str) -> AnyView {
    let message = message.to_owned();
    let create = ctx
        .module
        .try_with_value(|module| {
            module.actions.iter().find_map(|action| match action {
                Action::Create {
                    name,
                    entity: target,
                    fields,
                    ..
                } if target == entity => Some((name.clone(), target.clone(), fields.clone())),
                _ => None,
            })
        })
        .flatten();
    let button = create.map(|(name, target, fields)| {
        let label = ctx.label_of(&name, None);
        view! {
            <button class=class::BUTTON title=name.clone() on:click=move |_| {
                ctx.on_intent.run(Intent::Create {
                    action: name.clone(),
                    entity: target.clone(),
                    fields: fields.clone(),
                    values: Vec::new(),
                });
            }>{label}</button>
        }
        .into_any()
    });
    view! {
        <div class=format!("{} {}", class::STATE, class::EMPTY) role="status">
            <span>{message}</span>
            {button}
        </div>
    }
    .into_any()
}

/// A readable fallback: what the renderer could not draw, said in words.
pub(crate) fn fallback_public(what: &str) -> AnyView {
    fallback(what)
}

fn fallback(what: &str) -> AnyView {
    view! {
        <div class=class::UNRENDERED role="note">
            <strong>{text::UNRENDERED}</strong>
            <span>{format!(" : {what}")}</span>
        </div>
    }
    .into_any()
}

/// The tile for any reading that is not rows. The `Rows` arm is unreachable by construction
/// — it returns a fallback instead of panicking, because a renderer never takes the screen
/// hostage for a mistake.
pub(crate) fn tile_for(ctx: Ctx, entity: &str, reading: &Reading<'_>, when_empty: &str) -> AnyView {
    match reading {
        Reading::Loading => loading_tile(),
        Reading::Empty => empty_tile(ctx, entity, when_empty),
        Reading::Failed(reason) => failed_tile(ctx, entity, reason),
        Reading::Denied(reason) => denied_tile(ctx, entity, reason),
        Reading::Unknown => missing_tile(entity),
        Reading::Rows(_) => fallback("lecture inattendue (lignes sur une tuile d'état)"),
    }
}

// ---------------------------------------------------------------- bricks

/// Draw one brick of the tree.
///
/// The last arm catches a variant the contract gained after this renderer was written; it
/// draws a readable fallback instead of breaking the build or the screen.
pub fn render_brick(ctx: Ctx, brick: &Brick) -> AnyView {
    #[allow(unreachable_patterns)]
    match brick {
        Brick::Banner {
            tone,
            text: message,
        } => banner(*tone, message),
        Brick::CapabilityRequest { capability, action } => {
            capability_request(ctx, *capability, action)
        }
        Brick::Settings { sections } => settings(ctx, sections),
        Brick::Secret { name, label, hint } => {
            secret_input(ctx, name, label.as_deref(), hint.as_deref())
        }
        Brick::Composer { .. } => match ComposerSpec::of(brick) {
            Some(spec) => composer(ctx, &spec, composer_slot(&spec)),
            None => fallback(&describe_brick(brick)),
        },
        Brick::Text {
            entity,
            filter,
            field,
        } => value_brick(ctx, entity, filter.as_ref(), field, |value| {
            view! { <p class=class::TEXT>{value}</p> }.into_any()
        }),
        Brick::Caption {
            entity,
            filter,
            field,
        } => value_brick(ctx, entity, filter.as_ref(), field, |value| {
            view! { <small class=class::CAPTION>{value}</small> }.into_any()
        }),
        Brick::Chart {
            entity,
            field,
            kind,
            filter,
            label,
            height,
        } => chart(
            ctx,
            entity,
            filter.as_ref(),
            field,
            kind,
            label.as_deref(),
            *height,
        ),
        Brick::Button {
            entity,
            filter,
            action,
            label,
        } => button(ctx, entity, filter.as_ref(), action, label.as_deref()),
        Brick::Indicator {
            signal,
            entity,
            filter,
            field,
            duration,
        } => indicator(
            ctx,
            *signal,
            entity.as_deref(),
            filter.as_ref(),
            field.as_deref(),
            duration.as_deref(),
        ),
        Brick::List {
            entity,
            filter,
            label,
            subtitle,
            empty,
            sort,
            filterable,
            page_size,
            on_click,
            row_actions,
        } => list(
            ctx,
            entity,
            filter.as_ref(),
            label,
            subtitle.as_deref(),
            empty.as_deref(),
            sort,
            filterable,
            *page_size,
            on_click.as_ref(),
            row_actions,
        ),
        Brick::Card {
            entity,
            filter,
            fields,
        } => card(ctx, entity, filter.as_ref(), fields),
        Brick::Form {
            entity,
            filter,
            fields,
            submit,
        } => form(ctx, entity, filter.as_ref(), fields, submit.as_deref()),
        Brick::Field {
            entity,
            filter,
            field,
            input,
            target,
        } => field_brick(
            ctx,
            entity,
            filter.as_ref(),
            field,
            *input,
            target.as_deref(),
        ),
        Brick::Relation {
            entity,
            filter,
            link,
            mode,
        } => relation(ctx, entity, filter.as_ref(), link, *mode),
        Brick::Chat { entity, filter } => chat(ctx, entity, filter.as_ref()),
        Brick::Message {
            entity,
            filter,
            markdown,
            page_size,
        } => message(ctx, entity, filter.as_ref(), *markdown, *page_size),
        Brick::Notice {
            tone,
            entity,
            filter,
            field,
            action,
            label,
        } => notice(
            ctx,
            *tone,
            entity,
            filter.as_ref(),
            field,
            action.as_deref(),
            label.as_deref(),
        ),
        Brick::Question {
            entity,
            filter,
            text,
            options,
            answer,
        } => question_card(
            ctx,
            entity,
            filter.as_ref(),
            text,
            options,
            answer.as_deref(),
        ),
        Brick::ToolStep {
            entity,
            filter,
            field,
        } => tool_step(ctx, entity, filter.as_ref(), field),
        Brick::Queue {
            entity,
            filter,
            label,
            sort,
        } => queue(ctx, entity, filter.as_ref(), label, sort),
        Brick::CodeBlock {
            entity,
            filter,
            field,
            language,
        } => {
            let class_name = match language.as_deref() {
                Some(language) if !language.is_empty() => {
                    format!("language-{}", language.replace(' ', "-"))
                }
                _ => String::new(),
            };
            value_brick(ctx, entity, filter.as_ref(), field, move |value| {
                let class_name = class_name.clone();
                view! {
                    <pre class=class::CODE><code class=class_name>{value}</code></pre>
                }
                .into_any()
            })
        }
        Brick::Result {
            entity,
            filter,
            field,
        } => value_brick(ctx, entity, filter.as_ref(), field, |value| {
            view! { <pre class=class::RESULT>{value}</pre> }.into_any()
        }),
        Brick::Metrics {
            entity,
            filter,
            field,
        } => metrics(ctx, entity, filter.as_ref(), field.as_deref()),
        Brick::Program {
            entity,
            filter,
            args,
            run,
            columns,
        } => program(ctx, entity, filter.as_ref(), args, *run, columns),
        Brick::Image { .. } => image::render_image(ctx, brick),
        other => fallback(&describe_brick(other)),
    }
}

/// The name of a brick the renderer does not know, taken from its debug form.
fn describe_brick(brick: &Brick) -> String {
    let raw = format!("{brick:?}");
    let name = raw
        .split([' ', '{', '('])
        .next()
        .unwrap_or_default()
        .to_owned();
    format!("brique `{name}`")
}

/// A banner needs no data: its tone and its text are declared.
fn banner(tone: Tone, message: &str) -> AnyView {
    let (class_name, role) = match tone {
        Tone::Error => (class::TONE_ERROR, "alert"),
        Tone::Warning => (class::TONE_WARNING, "status"),
        Tone::Success => (class::TONE_SUCCESS, "status"),
        Tone::Info => (class::TONE_INFO, "status"),
    };
    let message = message.to_owned();
    view! {
        <p class=format!("{} {}", class::BANNER, class_name) role=role>{message}</p>
    }
    .into_any()
}

/// The label of a device capability, in words a person reads.
fn capability_label(capability: DeviceCapability) -> &'static str {
    match capability {
        DeviceCapability::Camera => "Caméra",
        DeviceCapability::Microphone => "Microphone",
        DeviceCapability::Location => "Localisation",
        DeviceCapability::Contacts => "Contacts",
        DeviceCapability::Calendar => "Calendrier",
        DeviceCapability::Health => "Santé",
        DeviceCapability::Bluetooth => "Bluetooth",
        DeviceCapability::Gyroscope => "Gyroscope",
        DeviceCapability::MotionActivity => "Activité physique",
        DeviceCapability::Nfc => "NFC",
        DeviceCapability::LocationAlways => "Localisation en arrière-plan",
        DeviceCapability::Speech => "Reconnaissance vocale",
        DeviceCapability::FaceId => "Face ID",
        DeviceCapability::Photos => "Photothèque",
        DeviceCapability::PhotosPick => "Sélecteur de photos",
        DeviceCapability::FilesPick => "Fichiers",
        DeviceCapability::Clipboard => "Presse-papiers",
        DeviceCapability::Share => "Partage",
        DeviceCapability::Notifications => "Notifications",
        DeviceCapability::LiveActivity => "Activité en direct",
        DeviceCapability::LocalNetwork => "Réseau local",
        DeviceCapability::Screen => "Partage d'écran",
        DeviceCapability::DeviceMotion => "Mouvement de l'appareil",
    }
}

fn capability_request(ctx: Ctx, capability: DeviceCapability, action: &str) -> AnyView {
    let label = capability_label(capability).to_owned();
    let allow_action = action.to_owned();
    let deny_action = action.to_owned();
    view! {
        <div class=class::CAPABILITY role="group" aria-label=label.clone()>
            <strong>{label.clone()}</strong>
            <div class=class::CAPABILITY_ACTIONS>
                <button class=class::BUTTON on:click=move |_| {
                    ctx.on_intent.run(Intent::Capability {
                        action: allow_action.clone(),
                        capability,
                        grant: Grant::Allow,
                    });
                }>{text::ALLOW}</button>
                <button class=class::BUTTON on:click=move |_| {
                    ctx.on_intent.run(Intent::Capability {
                        action: deny_action.clone(),
                        capability,
                        grant: Grant::Deny,
                    });
                }>{text::DENY}</button>
            </div>
        </div>
    }
    .into_any()
}

/// La saisie d'un secret : un champ masqué, un bouton, jamais une valeur qui repart vers
/// le module. La personne tape ; à l'envoi, l'intention [`Intent::SecretEntry`] porte le nom
/// et la valeur, le champ se vide, et rien ne reste dans le DOM ni dans un `Debug`.
fn secret_input(ctx: Ctx, name: &str, label: Option<&str>, hint: Option<&str>) -> AnyView {
    let name = name.to_owned();
    let label = label
        .map(str::to_owned)
        .unwrap_or_else(|| placement::humanise(&name));
    let hint = hint.map(str::to_owned);
    let id = next_id("vw-secret");
    let value = RwSignal::new(String::new());
    let submit_name = name.clone();
    view! {
        <div class=class::FIELD>
            <label class=class::LABEL for=id.clone() title=name.clone()>{label}</label>
            <input class=class::INPUT type="password" id=id.clone() autocomplete="off"
                prop:value=move || value.get()
                on:input=move |ev| value.set(event_target_value(&ev)) />
            {hint.map(|hint| view! { <span class=class::HINT>{hint}</span> }.into_any())}
            <button class=class::BUTTON
                disabled=move || value.get().is_empty()
                on:click=move |_| {
                    let secret = value.get_untracked();
                    if secret.is_empty() {
                        return;
                    }
                    ctx.on_intent.run(Intent::SecretEntry {
                        name: submit_name.clone(),
                        value: SecretValue(secret),
                    });
                    value.set(String::new());
                }>
                {text::SEND}
            </button>
        </div>
    }
    .into_any()
}

/// A brick that shows one value of one row.
fn value_brick<F>(
    ctx: Ctx,
    entity: &str,
    filter: Option<&FilterExpr>,
    field: &str,
    wrap: F,
) -> AnyView
where
    F: Fn(String) -> AnyView + Send + Sync + 'static,
{
    let entity = entity.to_owned();
    let filter = filter.cloned();
    let field = field.to_owned();
    (move || {
        let snapshot = ctx.data.get();
        match data::read(&snapshot, &entity, filter.as_ref(), &[], 0, None) {
            Reading::Rows(rows) => {
                let value = rows
                    .rows
                    .first()
                    .copied()
                    .and_then(|row| data::value_of(row, &field));
                wrap(data::display(value))
            }
            other => tile_for(ctx, &entity, &other, text::NOTHING),
        }
    })
    .into_any()
}

/// Cette action a-t-elle besoin de la ligne sous le bouton pour avoir un sens ?
///
/// `Update` et `Delete` écrivent une ligne : oui. `Invoke` aussi **quand l'opération vise
/// une entité** — sans son `id`, le module reçoit un appel qu'il ne peut pas situer
/// (mesuré sur l'appareil : un bouton « terminer » posé sur une ligne envoyait `{}`, le
/// module refusait faute d'identifiant, l'interface semblait morte). Une opération de
/// module (`entity` absente) n'a pas besoin de ligne : elle reste active sur une entité vide.
pub(crate) fn action_needs_row(declared: Option<&Action>) -> bool {
    match declared {
        Some(Action::Update { .. }) | Some(Action::Delete { .. }) => true,
        Some(Action::Invoke { entity, .. }) => entity.is_some(),
        _ => false,
    }
}

fn button(
    ctx: Ctx,
    entity: &str,
    filter: Option<&FilterExpr>,
    action: &str,
    brick_label: Option<&str>,
) -> AnyView {
    let label = ctx.label_of(action, brick_label);
    let declared = ctx
        .module
        .try_with_value(|module| module.actions.iter().find(|a| a.name() == action).cloned())
        .flatten();
    let is_delete = matches!(declared, Some(Action::Delete { .. }));
    let needs_row = action_needs_row(declared.as_ref());
    let entity = entity.to_owned();
    let name = action.to_owned();
    let filter_click = filter.cloned();

    if is_delete {
        return confirm_delete(ctx, &entity, filter_click, &name, &label);
    }

    let entity_disabled = entity.clone();
    let filter_disabled = filter_click.clone();
    let disabled = move || {
        let snapshot = ctx.data.get();
        match snapshot.feeds.get(&entity_disabled) {
            Some(Feed::Denied(_)) => true,
            _ => {
                needs_row
                    && data::target_id(&snapshot, &entity_disabled, filter_disabled.as_ref())
                        .is_none()
            }
        }
    };
    view! {
        <button class=class::BUTTON title=name.clone() disabled=disabled
            on:click=move |_| {
                let snapshot = ctx.data.get();
                let id = if needs_row {
                    data::target_id(&snapshot, &entity, filter_click.as_ref())
                } else {
                    None
                };
                let intent = action_intent(ctx, &name, Some(&entity), id, Vec::new());
                ctx.on_intent.run(intent);
            }>
            {label}
        </button>
    }
    .into_any()
}

/// A destructive action asks once before it goes: a click that deletes without a second
/// beat is a click nobody meant.
fn confirm_delete(
    ctx: Ctx,
    entity: &str,
    filter: Option<FilterExpr>,
    name: &str,
    label: &str,
) -> AnyView {
    let armed = RwSignal::new(false);
    let entity = entity.to_owned();
    let name = name.to_owned();
    let label = label.to_owned();
    let label_idle = label.clone();
    view! {
        <div class=class::CONFIRM>
            <button class=class::BUTTON title=name.clone()
                on:click=move |_| {
                    if armed.get_untracked() {
                        armed.set(false);
                        let snapshot = ctx.data.get();
                        let id = data::target_id(&snapshot, &entity, filter.as_ref());
                        let intent = action_intent(ctx, &name, Some(&entity), id, Vec::new());
                        ctx.on_intent.run(intent);
                    } else {
                        armed.set(true);
                    }
                }>
                {move || if armed.get() { text::CONFIRM.to_owned() } else { label_idle.clone() }}
            </button>
            {move || {
                armed
                    .get()
                    .then(|| {
                        view! {
                            <button class=class::BUTTON on:click=move |_| armed.set(false)>
                                {text::CANCEL}
                            </button>
                        }
                    })
            }}
        </div>
    }
    .into_any()
}

/// Turn an action name into the intent it means, using the module's own declarations.
fn action_intent(
    ctx: Ctx,
    name: &str,
    entity: Option<&str>,
    id: Option<String>,
    values: Values,
) -> Intent {
    let declared = ctx
        .module
        .try_with_value(|module| {
            module
                .actions
                .iter()
                .find(|action| action.name() == name)
                .cloned()
        })
        .flatten();
    intent_of(declared, name, entity, id, values)
}

/// The intent a declared action means: pure, so it is measured without a reactive owner.
pub(crate) fn intent_of(
    declared: Option<Action>,
    name: &str,
    entity: Option<&str>,
    id: Option<String>,
    values: Values,
) -> Intent {
    #[allow(unreachable_patterns)]
    match declared {
        Some(Action::Create {
            name,
            entity,
            fields,
            ..
        }) => Intent::Create {
            action: name,
            entity,
            fields,
            values,
        },
        Some(Action::Update {
            name,
            entity,
            fields,
            ..
        }) => Intent::Update {
            action: name,
            entity,
            fields,
            id,
            values,
        },
        Some(Action::Delete { name, entity, .. }) => Intent::Delete {
            action: name,
            entity,
            id,
        },
        Some(Action::Navigate {
            name, view, module, ..
        }) => Intent::Navigate {
            view,
            action: Some(name),
            module,
        },
        Some(Action::RequestSecret { name, secret, .. }) => Intent::Secret {
            action: name,
            secret,
        },
        Some(Action::AgentTool {
            name,
            op,
            entity,
            text,
            ..
        }) => Intent::Tool {
            action: name,
            op,
            entity,
            text: text.clone(),
        },
        Some(Action::DeviceCapability {
            name,
            capability,
            grant,
            ..
        }) => Intent::Capability {
            action: name,
            capability,
            grant,
        },
        // Une surface que l'hôte garde : le module l'offre, l'hôte l'ouvre. L'intention
        // porte la surface, pas le nom du module : l'hôte seul sait où elle vit.
        Some(Action::Host { name, surface, .. }) => Intent::Host {
            action: name,
            surface,
        },
        Some(Action::Invoke {
            name,
            operation,
            entity,
            ..
        }) => Intent::Invoke {
            action: name,
            operation,
            entity,
            id,
            values,
        },
        // Action inconnue d'une surface qui n'a pas classé : l'opération retombe sur le
        // nom de l'action, et la ligne (si elle existe) est quand même transmise — c'est
        // ce qui distingue « pas encore branché » de « perdu en route ».
        _ => Intent::Invoke {
            action: name.to_owned(),
            operation: name.to_owned(),
            entity: entity.map(str::to_owned),
            id,
            values,
        },
    }
}

// ---------------------------------------------------------------------------------------
// the composer
// ---------------------------------------------------------------------------------------

/// What a declared `composer` brick offers, read once. The host builds its REAL field from
/// it — this crate draws no keyboard, sends nothing and stops nothing.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ComposerSpec {
    /// The field's invitation; `None` = the host's.
    pub placeholder: Option<String>,
    /// The module's actions drawn around the field, in order.
    pub actions: Vec<String>,
    /// Whether the composer accepts attachments (button, drop, paste).
    pub attachments: bool,
}

impl ComposerSpec {
    /// The spec of a `composer` brick; `None` for any other brick.
    pub fn of(brick: &Brick) -> Option<Self> {
        match brick {
            Brick::Composer {
                placeholder,
                actions,
                attachments,
            } => Some(Self {
                placeholder: placeholder.clone(),
                actions: actions.clone(),
                attachments: *attachments,
            }),
            _ => None,
        }
    }

    /// The first composer of a tree, in reading order (tabs included); `None` = the tree
    /// declares none, and the host keeps its own.
    pub fn find(node: &Node) -> Option<Self> {
        match node {
            Node::Leaf { brick } => Self::of(brick),
            Node::Stack { children }
            | Node::Row { children }
            | Node::Drawer { children, .. }
            | Node::Modal { children, .. } => children.iter().find_map(Self::find),
            Node::Tabs { tabs } => tabs
                .iter()
                .flat_map(|tab| tab.children.iter())
                .find_map(Self::find),
            Node::When { child, .. } => Self::find(child),
            Node::Html { .. } => None,
        }
    }
}

/// The tree without its composer leaves, for a host that draws the composer in its own
/// place (under the thread) and the rest of the view in the thread. `None` = nothing is
/// left to draw: the view WAS its composer.
pub fn without_composer(node: &Node) -> Option<Node> {
    let keep =
        |children: &[Node]| -> Vec<Node> { children.iter().filter_map(without_composer).collect() };
    let kept = match node {
        Node::Leaf { brick } if matches!(brick.as_ref(), Brick::Composer { .. }) => return None,
        Node::Leaf { .. } | Node::Html { .. } => return Some(node.clone()),
        Node::Stack { children } => Node::Stack {
            children: keep(children),
        },
        Node::Row { children } => Node::Row {
            children: keep(children),
        },
        Node::Drawer { label, children } => Node::Drawer {
            label: label.clone(),
            children: keep(children),
        },
        Node::Modal { title, children } => Node::Modal {
            title: title.clone(),
            children: keep(children),
        },
        Node::Tabs { tabs } => Node::Tabs {
            tabs: tabs
                .iter()
                .map(|tab| Tab {
                    label: tab.label.clone(),
                    children: keep(&tab.children),
                })
                .collect(),
        },
        Node::When {
            entity,
            filter,
            child,
        } => Node::When {
            entity: entity.clone(),
            filter: filter.clone(),
            child: Box::new(without_composer(child)?),
        },
    };
    let empty = match &kept {
        Node::Stack { children }
        | Node::Row { children }
        | Node::Drawer { children, .. }
        | Node::Modal { children, .. } => children.is_empty(),
        Node::Tabs { tabs } => tabs.iter().all(|tab| tab.children.is_empty()),
        Node::Leaf { .. } | Node::When { .. } | Node::Html { .. } => false,
    };
    (!empty).then_some(kept)
}

/// The intent a composer action emits: the same as a `button`'s, with no row (a composer
/// reads none — the contract refuses the actions that would need one).
pub fn composer_intent(module: &ModuleDescriptor, action: &str) -> Intent {
    let declared = module.actions.iter().find(|a| a.name() == action).cloned();
    intent_of(declared, action, None, None, Vec::new())
}

/// The intent a settings line emits: the one its action means, with no row.
pub fn settings_intent(module: &ModuleDescriptor, action: &str) -> Intent {
    composer_intent(module, action)
}

/// A settings screen: a titled section, then one line per item. A line is a button, so it
/// is 44 px tall and reachable; clicking it emits the intent of its action. The brick never
/// draws a value — a secret's line only asks the host to collect it.
fn settings(ctx: Ctx, sections: &[SettingsSection]) -> AnyView {
    let sections = sections
        .iter()
        .map(|section| {
            let title = section.title.clone();
            let hint = section.hint.clone();
            let items = section
                .items
                .iter()
                .map(|item| {
                    let name = item.action.clone();
                    let label = item.label.clone();
                    let hint = item.hint.clone();
                    view! {
                        <button class=format!("{} {}", class::BUTTON, class::SETTINGS_ITEM)
                            type="button" title=name.clone() data-action=name.clone()
                            on:click=move |_| {
                                let intent = ctx
                                    .module
                                    .try_with_value(|module| settings_intent(module, &name));
                                if let Some(intent) = intent {
                                    ctx.on_intent.run(intent);
                                }
                            }>
                            <span>{label}</span>
                            {hint.map(|h| view! { <span class=class::SETTINGS_HINT>{h}</span> })}
                        </button>
                    }
                })
                .collect_view();
            view! {
                <section class=class::SETTINGS_SECTION>
                    <h3 class=class::SETTINGS_TITLE>{title}</h3>
                    {hint.map(|h| view! { <p class=class::SETTINGS_HINT>{h}</p> })}
                    {items}
                </section>
            }
        })
        .collect_view();
    view! { <div class=class::SETTINGS>{sections}</div> }.into_any()
}

/// The module's actions, as buttons for the host to place around its field. Each click
/// emits the intent the action means — the host runs it by the same path as any view.
pub fn composer_actions(ctx: Ctx, actions: &[String]) -> AnyView {
    let buttons = actions
        .iter()
        .map(|action| {
            let name = action.clone();
            let label = ctx.label_of(action, None);
            view! {
                <button class=class::BUTTON type="button" title=name.clone() data-action=name.clone()
                    on:click=move |_| {
                        let intent = ctx
                            .module
                            .try_with_value(|module| composer_intent(module, &name));
                        if let Some(intent) = intent {
                            ctx.on_intent.run(intent);
                        }
                    }>
                    {label}
                </button>
            }
        })
        .collect_view();
    view! { <div class=class::COMPOSER_ACTIONS>{buttons}</div> }.into_any()
}

/// The composer zone: the host's `field`, and the module's actions beside it. The zone says
/// what was declared in `data-*` so the host's style can follow it.
pub fn composer(ctx: Ctx, spec: &ComposerSpec, field: AnyView) -> AnyView {
    let actions = (!spec.actions.is_empty()).then(|| composer_actions(ctx, &spec.actions));
    view! {
        <div class=class::COMPOSER data-attachments=spec.attachments.to_string()>
            {field}
            {actions}
        </div>
    }
    .into_any()
}

/// What `render_brick` puts where the field goes: an empty slot the host fills, carrying the
/// declared invitation. Never a fake field — a textarea that sends nothing would lie.
fn composer_slot(spec: &ComposerSpec) -> AnyView {
    let placeholder = spec
        .placeholder
        .clone()
        .unwrap_or_else(|| text::COMPOSER.to_owned());
    view! {
        <div class=class::COMPOSER_SLOT data-placeholder=placeholder.clone() aria-label=placeholder></div>
    }
    .into_any()
}

fn indicator(
    ctx: Ctx,
    signal: IndicatorSignal,
    entity: Option<&str>,
    filter: Option<&FilterExpr>,
    field: Option<&str>,
    duration: Option<&str>,
) -> AnyView {
    if signal == IndicatorSignal::Connection {
        return view! {
            <span class=class::INDICATOR aria-live="polite">
                {move || {
                    let online = ctx.data.get().online;
                    let text = if online { text::ONLINE } else { text::OFFLINE };
                    view! {
                        <span class=class::DOT aria-hidden="true"></span>
                        <span>{text}</span>
                    }
                }}
            </span>
        }
        .into_any();
    }
    let Some(entity) = entity else {
        return fallback("indicateur sans entité");
    };
    let entity = entity.to_owned();
    let filter = filter.cloned();
    let field = field.map(str::to_owned);
    let duration = duration.map(str::to_owned);
    (move || {
        let snapshot = ctx.data.get();
        match data::read(&snapshot, &entity, filter.as_ref(), &[], 0, None) {
            Reading::Rows(rows) => {
                let first = rows.rows.first().copied();
                match signal {
                    IndicatorSignal::Badge => {
                        let value = match field.as_deref() {
                            Some(field) => {
                                data::display(first.and_then(|row| data::value_of(row, field)))
                            }
                            None => crate::model::count(rows.total as f64),
                        };
                        design::badge(value, DesignTone::Info)
                    }
                    IndicatorSignal::Counter => {
                        let value = match field.as_deref() {
                            Some(field) => {
                                data::display(first.and_then(|row| data::value_of(row, field)))
                            }
                            None => crate::model::count(rows.total as f64),
                        };
                        view! {
                            <span class=class::INDICATOR><strong>{value}</strong></span>
                        }
                        .into_any()
                    }
                    _ => {
                        // Une activité SANS phase n'est pas une activité : au repos, il n'y a
                        // rien à dire. Sans cette règle, un indicateur d'activité posé dans un
                        // fil resterait affiché en permanence avec un tiret — et « — » ne veut
                        // pas dire « au repos », il veut dire « je ne sais pas ». Même règle
                        // que l'avis : un champ vide ne s'annonce pas.
                        let phase = match field.as_deref() {
                            Some(field) => {
                                annonce(first.and_then(|row| data::value_of(row, field)))
                                    .unwrap_or_default()
                            }
                            None => text::DASH.to_owned(),
                        };
                        if phase.trim().is_empty() {
                            return ().into_any();
                        }
                        let elapsed = duration.as_deref().map(|field| {
                            data::display(first.and_then(|row| data::value_of(row, field)))
                        });
                        view! {
                            <span class=format!("{} {}", class::INDICATOR, class::ACTIVITY)>
                                <span class=class::DOT aria-hidden="true"></span>
                                <span>{phase}</span>
                                {elapsed.map(|value| {
                                    view! { <span class=class::HINT>{value}</span> }.into_any()
                                })}
                            </span>
                        }
                        .into_any()
                    }
                }
            }
            other => tile_for(ctx, &entity, &other, text::NOTHING),
        }
    })
    .into_any()
}

// ---------------------------------------------------------------- descriptor lookups

/// The kind of a field, declared or base (`id`, `created_at`…).
fn field_kind(
    module: StoredValue<ModuleDescriptor>,
    entity: &str,
    field: &str,
) -> Option<FieldKind> {
    module
        .try_with_value(|module| {
            let entity = module.entities.iter().find(|e| e.name == entity)?;
            if let Some(found) = entity.fields.iter().find(|f| f.name == field) {
                return Some(FieldKind::of(&found.ty));
            }
            this_descriptor::base_fields(entity.kind)
                .iter()
                .find(|f| f.name == field)
                .map(|f| FieldKind::of(&f.ty))
        })
        .flatten()
}

/// The field among `preferred` that the entity declares, or nothing.
///
/// Unlike [`display_field`] there is NO fallback: a brick that reads a role, a state or a
/// journal must not read the content because the module named none of them. A fallback is
/// right for the thing that must always show something (the text of a bubble); it is wrong
/// for anything else, where reading the wrong field is worse than reading none.
fn named_field(
    module: StoredValue<ModuleDescriptor>,
    entity: &str,
    preferred: &[&str],
) -> Option<String> {
    module
        .try_with_value(|module| {
            let entity = module.entities.iter().find(|e| e.name == entity)?;
            preferred
                .iter()
                .find(|wanted| entity.fields.iter().any(|f| f.name == **wanted))
                .map(|wanted| (*wanted).to_owned())
        })
        .flatten()
}

/// The field to show as the name of a row: the first preferred name that exists, else the
/// first short text field, else `id`. The contract carries no label, so this is a choice,
/// and it is made in one place.
fn display_field(
    module: StoredValue<ModuleDescriptor>,
    entity: &str,
    preferred: &[&str],
) -> String {
    module
        .try_with_value(|module| {
            let entity = module.entities.iter().find(|e| e.name == entity)?;
            for wanted in preferred {
                if let Some(found) = entity.fields.iter().find(|f| f.name == *wanted) {
                    return Some(found.name.clone());
                }
            }
            entity
                .fields
                .iter()
                .find(|f| {
                    matches!(
                        f.ty,
                        this_descriptor::FieldType::String | this_descriptor::FieldType::Text
                    )
                })
                .map(|f| f.name.clone())
        })
        .flatten()
        .unwrap_or_else(|| "id".to_owned())
}

/// The target of a relation and the field of the target that points back. A declared
/// `Ref` field wins; otherwise the convention `<source>_id` is tried.
fn link_of(
    module: StoredValue<ModuleDescriptor>,
    entity: &str,
    link: &str,
) -> Option<(String, String)> {
    module
        .try_with_value(|module| {
            let declared = module
                .links
                .iter()
                .find(|l| l.link_type == link && l.source == entity)?;
            let target = declared.target.clone();
            let back = module
                .entities
                .iter()
                .find(|e| e.name == target)
                .and_then(|e| {
                    // The field of the target that points back at the source entity.
                    e.fields.iter().find_map(|f| match &f.ty {
                        this_descriptor::FieldType::Ref { entity: points_at }
                            if points_at == entity =>
                        {
                            Some(f.name.clone())
                        }
                        _ => None,
                    })
                })
                .unwrap_or_else(|| format!("{entity}_id"));
            Some((target, back))
        })
        .flatten()
}

/// A row's value as a form value.
fn value_from(row: Option<&Value>, field: &str) -> FieldValue {
    match row.and_then(|row| data::value_of(row, field)) {
        None | Some(Value::Null) => FieldValue::Empty,
        Some(Value::Bool(flag)) => FieldValue::Flag(*flag),
        Some(value) => FieldValue::Text(data::display(Some(value))),
    }
}

/// A form value as text.
fn text_of(value: &FieldValue) -> String {
    match value {
        FieldValue::Text(text) | FieldValue::File(text) => text.clone(),
        FieldValue::Flag(true) => text::YES.to_owned(),
        FieldValue::Flag(false) => text::NO.to_owned(),
        FieldValue::Empty => String::new(),
    }
}

// ---------------------------------------------------------------- controls

/// The control a field is edited with.
#[derive(Clone, Debug, PartialEq)]
enum Control {
    Text,
    Numeric,
    Multiline,
    File,
    Toggle,
    Choice(Vec<String>),
    Reference(String),
}

fn control_of_input(input: Input, target: Option<&str>) -> Control {
    #[allow(unreachable_patterns)]
    match input {
        Input::Text => Control::Text,
        Input::Multiline => Control::Multiline,
        Input::File => Control::File,
        Input::Toggle => Control::Toggle,
        Input::Reference => match target {
            Some(target) if !target.is_empty() => Control::Reference(target.to_owned()),
            // A reference with no target is a declaration `views::issues` already refuses;
            // drawing a text input keeps the screen usable instead of an empty select.
            _ => Control::Text,
        },
        _ => Control::Text,
    }
}

fn control_of_kind(kind: Option<FieldKind>) -> Control {
    match kind {
        Some(FieldKind::Bool) => Control::Toggle,
        Some(FieldKind::Int | FieldKind::Decimal) => Control::Numeric,
        Some(FieldKind::Enum(variants)) => Control::Choice(variants),
        Some(FieldKind::Ref(target)) => Control::Reference(target),
        Some(FieldKind::Json | FieldKind::List(_)) => Control::Multiline,
        _ => Control::Text,
    }
}

/// One labelled control. Every control carries a `<label for>`, a value and, when the
/// surface asked for it, a commit callback.
#[allow(clippy::too_many_arguments)]
fn control(
    ctx: Ctx,
    id: &str,
    name: &str,
    label: &str,
    kind: Control,
    read_only: bool,
    initial: FieldValue,
    value: RwSignal<FieldValue>,
    commit: Option<Callback<FieldValue>>,
) -> AnyView {
    let id = id.to_owned();
    let label = label.to_owned();
    let name = name.to_owned();
    let set = move |next: FieldValue| {
        value.set(next.clone());
        if let Some(commit) = commit {
            commit.run(next);
        }
    };
    let hint =
        read_only.then(|| view! { <span class=class::HINT>{text::READ_ONLY}</span> }.into_any());
    let body = match kind {
        Control::Text => {
            let text = text_of(&initial);
            view! {
                <input class=class::INPUT type="text" id=id.clone() value=text readonly=read_only
                    on:change=move |ev| set(FieldValue::Text(event_target_value(&ev))) />
            }
            .into_any()
        }
        Control::Numeric => {
            let text = text_of(&initial);
            view! {
                <input class=class::INPUT type="number" step="any" id=id.clone() value=text
                    readonly=read_only
                    on:change=move |ev| set(FieldValue::Text(event_target_value(&ev))) />
            }
            .into_any()
        }
        Control::Multiline => {
            let text = text_of(&initial);
            view! {
                <textarea class=class::TEXTAREA id=id.clone() readonly=read_only
                    on:change=move |ev| set(FieldValue::Text(event_target_value(&ev)))>{text}</textarea>
            }
            .into_any()
        }
        Control::File => view! {
            <input class=class::FILE type="file" id=id.clone() disabled=read_only
                on:change=move |ev| {
                    let input = event_target::<web_sys::HtmlInputElement>(&ev);
                    set(FieldValue::File(file_name(&input)));
                } />
        }
        .into_any(),
        Control::Toggle => {
            let checked = matches!(initial, FieldValue::Flag(true));
            view! {
                <input class=class::TOGGLE type="checkbox" id=id.clone() checked=checked
                    disabled=read_only
                    on:change=move |ev| set(FieldValue::Flag(event_target_checked(&ev))) />
            }
            .into_any()
        }
        Control::Choice(variants) => {
            let current = text_of(&initial);
            let options = variants
                .iter()
                .map(|variant| {
                    let selected = *variant == current;
                    view! { <option value=variant.clone() selected=selected>{variant.clone()}</option> }
                })
                .collect_view();
            view! {
                <select class=class::SELECT id=id.clone() disabled=read_only
                    on:change=move |ev| set(FieldValue::Text(event_target_value(&ev)))>
                    {options}
                </select>
            }
            .into_any()
        }
        Control::Reference(target) => {
            let target_options = target.clone();
            let target_field = target.clone();
            (move || {
                let snapshot = ctx.data.get();
                let label_field =
                    display_field(ctx.module, &target_field, &["name", "title", "label"]);
                match snapshot.feeds.get(&target_options) {
                    Some(Feed::Ready(rows)) => rows
                        .iter()
                        .map(|row| {
                            let id = data::display(data::value_of(row, "id"));
                            let text = data::display(data::value_of(row, &label_field));
                            view! { <option value=id>{text}</option> }.into_any()
                        })
                        .collect_view()
                        .into_any(),
                    Some(Feed::Denied(reason)) => view! {
                        <option disabled=true>{format!("{} : {reason}", text::DENIED)}</option>
                    }
                    .into_any(),
                    Some(Feed::Failed(reason)) => view! {
                        <option disabled=true>{format!("{} : {reason}", text::RETRY)}</option>
                    }
                    .into_any(),
                    _ => view! { <option disabled=true>{text::LOADING}</option> }.into_any(),
                }
            })
            .into_any()
        }
    };
    view! {
        <div class=class::FIELD>
            <label class=class::LABEL for=id.clone() title=name.clone()>{label}</label>
            {body}
            {hint}
        </div>
    }
    .into_any()
}

/// The name of the file chosen in a file input, read from the DOM. The bytes never leave
/// the input: the host reads them itself.
fn file_name(input: &web_sys::HtmlInputElement) -> String {
    let files = js_sys::Reflect::get(input, &wasm_bindgen::JsValue::from_str("files")).ok();
    let Some(files) = files else {
        return String::new();
    };
    let first = js_sys::Reflect::get(&files, &wasm_bindgen::JsValue::from_f64(0.0)).ok();
    let Some(first) = first else {
        return String::new();
    };
    js_sys::Reflect::get(&first, &wasm_bindgen::JsValue::from_str("name"))
        .ok()
        .and_then(|name| name.as_string())
        .unwrap_or_default()
}

// ---------------------------------------------------------------- data bricks

#[allow(clippy::too_many_arguments)]
fn list(
    ctx: Ctx,
    entity: &str,
    filter: Option<&FilterExpr>,
    label: &str,
    subtitle: Option<&str>,
    empty: Option<&str>,
    sort: &[SortSpec],
    filterable: &[String],
    page_size: Option<u32>,
    on_click: Option<&Click>,
    row_actions: &[String],
) -> AnyView {
    let entity = entity.to_owned();
    let filter = filter.cloned();
    let label = label.to_owned();
    let subtitle = subtitle.map(str::to_owned);
    let empty = empty.unwrap_or(text::NOTHING).to_owned();
    let sort: Vec<SortSpec> = sort.to_vec();
    let filterable: Vec<String> = filterable.to_vec();
    let on_click = on_click.cloned();
    let row_actions: Vec<String> = row_actions.to_vec();
    let page = RwSignal::new(0usize);
    let needle = RwSignal::new(String::new());

    let search = (!filterable.is_empty()).then(|| {
        view! {
            <div class=class::LIST_TOOLS>
                <input class=class::SEARCH type="search" placeholder=text::SEARCH
                    aria-label=text::SEARCH
                    on:input=move |ev| {
                        needle.set(event_target_value(&ev));
                        page.set(0);
                    } />
            </div>
        }
        .into_any()
    });

    let items = {
        let entity = entity.clone();
        let filter = filter.clone();
        let label = label.clone();
        let subtitle = subtitle.clone();
        let empty = empty.clone();
        let sort = sort.clone();
        let filterable = filterable.clone();
        let on_click = on_click.clone();
        let row_actions = row_actions.clone();
        move || {
            let snapshot = ctx.data.get();
            let local = data::search(&filterable, &needle.get());
            let combined = data::and(filter.as_ref(), local);
            match data::read(
                &snapshot,
                &entity,
                combined.as_ref(),
                &sort,
                page.get(),
                page_size,
            ) {
                Reading::Rows(rows) => {
                    let rows_view = rows
                        .rows
                        .iter()
                        .map(|row| {
                            let title = data::display(data::value_of(row, &label));
                            let sub = subtitle
                                .as_deref()
                                .map(|field| data::display(data::value_of(row, field)));
                            let sub = sub.map(|sub| {
                                view! { <span class=class::LIST_SUB>{sub}</span> }.into_any()
                            });
                            // Un bouton par action de ligne, chacun visant CETTE ligne. Un
                            // clic dessus n'ouvre pas la ligne : le bouton est un frère du
                            // bouton de `on_click`, pas un enfant.
                            let actions = row_actions
                                .iter()
                                .map(|action| {
                                    let name = action.clone();
                                    let action_label = ctx.label_of(action, None);
                                    let action_entity = entity.clone();
                                    let row_id = data::value_of(row, "id")
                                        .and_then(|id| id.as_str())
                                        .map(str::to_owned);
                                    let disabled = row_id.is_none();
                                    view! {
                                        <button class=class::BUTTON title=name.clone()
                                            disabled=disabled
                                            on:click=move |_| {
                                                let intent = action_intent(
                                                    ctx,
                                                    &name,
                                                    Some(&action_entity),
                                                    row_id.clone(),
                                                    Vec::new(),
                                                );
                                                ctx.on_intent.run(intent);
                                            }>
                                            {action_label}
                                        </button>
                                    }
                                    .into_any()
                                })
                                .collect_view();
                            let actions = (!row_actions.is_empty()).then(|| {
                                view! { <span class=class::LIST_ACTIONS>{actions}</span> }
                                    .into_any()
                            });
                            match &on_click {
                                None => view! {
                                    <li class=class::LIST_ROW>
                                        <span class=class::LIST_LABEL>{title}</span>
                                        {sub}
                                        {actions}
                                    </li>
                                }
                                .into_any(),
                                Some(click) => {
                                    let id = data::display(data::value_of(row, "id"));
                                    let intent = click_intent(click, &entity, &id);
                                    view! {
                                        <li class=class::LIST_ROW>
                                            <button class=class::LIST_ROW_CLICK
                                                on:click=move |_| ctx.on_intent.run(intent.clone())>
                                                <span class=class::LIST_LABEL>{title}</span>
                                                {sub}
                                            </button>
                                            {actions}
                                        </li>
                                    }
                                    .into_any()
                                }
                            }
                        })
                        .collect_view();
                    let pager = (rows.pages > 1).then(|| {
                        let info = format!("{} / {}", rows.page + 1, rows.pages);
                        view! {
                            <div class=class::PAGER>
                                <button class=class::BUTTON aria-label=text::PREVIOUS
                                    disabled=move || page.get() == 0
                                    on:click=move |_| page.update(|p| *p = p.saturating_sub(1))>
                                    {text::PREVIOUS}
                                </button>
                                <span class=class::PAGER_INFO>{info}</span>
                                <button class=class::BUTTON aria-label=text::NEXT
                                    disabled=move || page.get() + 1 >= rows.pages
                                    on:click=move |_| page.update(|p| *p += 1)>
                                    {text::NEXT}
                                </button>
                            </div>
                        }
                        .into_any()
                    });
                    view! {
                        <>
                            <ul class=class::LIST>{rows_view}</ul>
                            {pager}
                        </>
                    }
                    .into_any()
                }
                other => tile_for(ctx, &entity, &other, &empty),
            }
        }
    };

    view! {
        <div class=class::LIST>
            {search}
            {items}
        </div>
    }
    .into_any()
}

fn click_intent(click: &Click, entity: &str, id: &str) -> Intent {
    #[allow(unreachable_patterns)]
    match click {
        Click::Select => Intent::Select {
            entity: entity.to_owned(),
            id: id.to_owned(),
        },
        Click::OpenDetail => Intent::Open {
            entity: entity.to_owned(),
            id: id.to_owned(),
        },
        Click::Navigate { view } => Intent::Navigate {
            view: view.clone(),
            action: None,
            module: None,
        },
        _ => Intent::Select {
            entity: entity.to_owned(),
            id: id.to_owned(),
        },
    }
}

fn card(ctx: Ctx, entity: &str, filter: Option<&FilterExpr>, fields: &[String]) -> AnyView {
    let entity = entity.to_owned();
    let filter = filter.cloned();
    let fields: Vec<String> = fields.to_vec();
    (move || {
        let snapshot = ctx.data.get();
        match data::read(&snapshot, &entity, filter.as_ref(), &[], 0, None) {
            Reading::Rows(rows) => {
                let row = rows.rows.first().copied();
                let entries = fields
                    .iter()
                    .map(|field| {
                        let label = placement::humanise(field);
                        let value = data::display(row.and_then(|row| data::value_of(row, field)));
                        view! {
                            <div class=class::CARD_ROW>
                                <dt class=class::CARD_KEY>{label}</dt>
                                <dd class=class::CARD_VALUE>{value}</dd>
                            </div>
                        }
                        .into_any()
                    })
                    .collect_view();
                view! { <dl class=class::CARD>{entries}</dl> }.into_any()
            }
            other => tile_for(ctx, &entity, &other, text::NOTHING),
        }
    })
    .into_any()
}

fn form(
    ctx: Ctx,
    entity: &str,
    filter: Option<&FilterExpr>,
    fields: &[FormField],
    submit: Option<&str>,
) -> AnyView {
    if fields.is_empty() {
        return fallback("`form` sans champ");
    }
    let entity = entity.to_owned();
    let filter = filter.cloned();
    let id_base = next_id("vw-form");
    let snapshot = ctx.data.get_untracked();
    let row = data::first(&snapshot, &entity, filter.as_ref());
    let mut controls: Vec<(String, RwSignal<FieldValue>)> = Vec::new();
    let nodes: Vec<AnyView> = fields
        .iter()
        .map(|field| {
            let initial = value_from(row, &field.field);
            let signal = RwSignal::new(initial.clone());
            let kind = field_kind(ctx.module, &entity, &field.field);
            let label = field
                .label
                .clone()
                .unwrap_or_else(|| placement::humanise(&field.field));
            controls.push((field.field.clone(), signal));
            control(
                ctx,
                &format!("{id_base}-{}", field.field),
                &field.field,
                &label,
                control_of_kind(kind),
                field.read_only,
                initial,
                signal,
                None,
            )
        })
        .collect();
    let nodes = nodes.into_iter().collect_view();

    // A value typed by the person is never overwritten; a field still empty is filled when
    // the row finally arrives.
    let filler = controls.clone();
    let fill_entity = entity.clone();
    let fill_filter = filter.clone();
    Effect::new(move |_| {
        let snapshot = ctx.data.get();
        let row = data::first(&snapshot, &fill_entity, fill_filter.as_ref());
        for (name, signal) in &filler {
            if signal.get_untracked() == FieldValue::Empty {
                let value = value_from(row, name);
                if value != FieldValue::Empty {
                    signal.set(value);
                }
            }
        }
    });

    let submit_name = submit.map(str::to_owned);
    let submit_label = submit_name
        .as_deref()
        .map(|name| ctx.label_of(name, None))
        .unwrap_or_default();
    let submit_value = submit_name.clone();
    let on_submit = {
        let controls = controls.clone();
        let entity = entity.clone();
        let filter = filter.clone();
        move |ev: web_sys::SubmitEvent| {
            ev.prevent_default();
            let Some(name) = submit_value.as_deref() else {
                return;
            };
            let snapshot = ctx.data.get();
            let id = data::target_id(&snapshot, &entity, filter.as_ref());
            let values: Values = controls
                .iter()
                .map(|(field, signal)| (field.clone(), signal.get_untracked()))
                .collect();
            let intent = action_intent(ctx, name, Some(&entity), id, values);
            ctx.on_intent.run(intent);
        }
    };
    let button = submit_name.map(|_| {
        let label = submit_label.clone();
        view! { <button class=class::BUTTON type="submit">{label}</button> }.into_any()
    });
    view! {
        <form class=class::FORM on:submit=on_submit>
            {nodes}
            {button}
        </form>
    }
    .into_any()
}

fn field_brick(
    ctx: Ctx,
    entity: &str,
    filter: Option<&FilterExpr>,
    field: &str,
    input: Input,
    target: Option<&str>,
) -> AnyView {
    let id = next_id("vw-field");
    let entity = entity.to_owned();
    let filter = filter.cloned();
    let snapshot = ctx.data.get_untracked();
    let row = data::first(&snapshot, &entity, filter.as_ref());
    let initial = value_from(row, field);
    let signal = RwSignal::new(initial.clone());
    let label = placement::humanise(field);
    let commit_entity = entity.clone();
    let commit_filter = filter.clone();
    let commit_field = field.to_owned();
    let commit = Callback::new(move |value: FieldValue| {
        let snapshot = ctx.data.get();
        let id = data::target_id(&snapshot, &commit_entity, commit_filter.as_ref());
        ctx.on_intent.run(Intent::Edit {
            entity: commit_entity.clone(),
            id,
            field: commit_field.clone(),
            value,
        });
    });

    let fill_filter = filter.clone();
    let fill_entity = entity.clone();
    let fill_field = field.to_owned();
    Effect::new(move |_| {
        let snapshot = ctx.data.get();
        if signal.get_untracked() == FieldValue::Empty {
            let value = value_from(
                data::first(&snapshot, &fill_entity, fill_filter.as_ref()),
                &fill_field,
            );
            if value != FieldValue::Empty {
                signal.set(value);
            }
        }
    });

    let element = control(
        ctx,
        &id,
        field,
        &label,
        control_of_input(input, target),
        false,
        initial,
        signal,
        Some(commit),
    );
    // An incomplete declaration is said, not swallowed: `views::issues` refuses it, and a
    // host that skips the check still learns why the control is not what the module meant.
    if input == Input::Reference && target.is_none() {
        return view! {
            <div class=class::FIELD>
                {element}
                <p class=class::HINT>{text::INCOMPLETE_REFERENCE}</p>
            </div>
        }
        .into_any();
    }
    element
}

fn relation(
    ctx: Ctx,
    entity: &str,
    filter: Option<&FilterExpr>,
    link: &str,
    mode: RelationMode,
) -> AnyView {
    let Some((target, back)) = link_of(ctx.module, entity, link) else {
        return fallback(&format!("relation `{link}` non déclarée pour `{entity}`"));
    };
    let entity = entity.to_owned();
    let filter = filter.cloned();
    let chosen = RwSignal::new(String::new());
    let heading = placement::humanise(link);
    let link_name = link.to_owned();
    let target_label = display_field(ctx.module, &target, &["name", "title", "label"]);
    let editing = mode == RelationMode::Edit;

    let body = move || {
        let snapshot = ctx.data.get();
        let target = target.clone();
        let Some(source) = data::target_id(&snapshot, &entity, filter.as_ref()) else {
            return tile_for(ctx, &entity, &Reading::Empty, text::NOTHING);
        };
        match snapshot.feeds.get(&target) {
            Some(Feed::Denied(reason)) => denied_tile(ctx, &target, reason),
            Some(Feed::Failed(reason)) => failed_tile(ctx, &target, reason),
            Some(Feed::Ready(rows)) => {
                let linked: Vec<&Value> = rows
                    .iter()
                    .filter(|row| data::display(data::value_of(row, &back)) == source)
                    .collect();
                let mut items: Vec<AnyView> = linked
                    .iter()
                    .map(|row| {
                        let text = data::display(data::value_of(row, &target_label));
                        let linked_id = data::display(data::value_of(row, "id"));
                        let unlink = editing.then(|| {
                            let entity = entity.clone();
                            let link_name = link_name.clone();
                            let source = source.clone();
                            view! {
                                <button class=class::BUTTON aria-label=text::UNLINK
                                    on:click=move |_| ctx.on_intent.run(Intent::Link {
                                        entity: entity.clone(),
                                        id: Some(source.clone()),
                                        link: link_name.clone(),
                                        target: Some(linked_id.clone()),
                                        op: LinkOp::Remove,
                                    })>
                                    {text::UNLINK}
                                </button>
                            }
                        });
                        view! {
                            <li class=class::RELATION_ITEM>
                                <span>{text}</span>
                                {unlink}
                            </li>
                        }
                        .into_any()
                    })
                    .collect();
                if linked.is_empty() {
                    items.push(view! { <li class=class::HINT>{text::NO_LINK}</li> }.into_any());
                }
                let picker = editing.then(|| {
                    let options_target = target.clone();
                    let options_label = target_label.clone();
                    let pick = link_name.clone();
                    let pick_entity = entity.clone();
                    let pick_source = source.clone();
                    view! {
                        <div class=class::LIST_TOOLS>
                            <select class=class::SELECT aria-label=text::LINK
                                on:change=move |ev| chosen.set(event_target_value(&ev))>
                                {move || {
                                    let snapshot = ctx.data.get();
                                    match snapshot.feeds.get(&options_target) {
                                        Some(Feed::Ready(rows)) => rows
                                            .iter()
                                            .map(|row| {
                                                let id = data::display(data::value_of(row, "id"));
                                                let text = data::display(data::value_of(row, &options_label));
                                                view! { <option value=id>{text}</option> }.into_any()
                                            })
                                            .collect_view()
                                            .into_any(),
                                        _ => view! {
                                            <option disabled=true>{text::LOADING}</option>
                                        }
                                        .into_any(),
                                    }
                                }}
                            </select>
                            <button class=class::BUTTON on:click=move |_| {
                                let target = chosen.get_untracked();
                                if target.is_empty() {
                                    return;
                                }
                                ctx.on_intent.run(Intent::Link {
                                    entity: pick_entity.clone(),
                                    id: Some(pick_source.clone()),
                                    link: pick.clone(),
                                    target: Some(target),
                                    op: LinkOp::Add,
                                });
                            }>{text::LINK}</button>
                        </div>
                    }
                    .into_any()
                });
                view! {
                    <>
                        <ul class=class::LIST>{items}</ul>
                        {picker}
                    </>
                }
                .into_any()
            }
            _ => loading_tile(),
        }
    };

    view! {
        <div class=class::RELATION>
            <strong>{heading}</strong>
            {body}
        </div>
    }
    .into_any()
}

fn chat(ctx: Ctx, entity: &str, filter: Option<&FilterExpr>) -> AnyView {
    let entity = entity.to_owned();
    let filter = filter.cloned();
    let content = display_field(
        ctx.module,
        &entity,
        &["content", "text", "body", "message", "contenu"],
    );
    let author = display_field(ctx.module, &entity, &["role", "author", "sender", "from"]);
    let draft = RwSignal::new(String::new());
    let send_entity = entity.clone();

    let log = move || {
        let snapshot = ctx.data.get();
        match data::read(&snapshot, &entity, filter.as_ref(), &[], 0, None) {
            Reading::Rows(rows) => {
                let messages = rows
                    .rows
                    .iter()
                    .map(|row| {
                        let text = data::display(data::value_of(row, &content));
                        let who = data::value_of(row, &author).map(|value| data::display(Some(value)));
                        view! {
                            <li class=class::MESSAGE>
                                {who.map(|who| {
                                    view! { <span class=class::MESSAGE_META>{who}</span> }.into_any()
                                })}
                                <span>{text}</span>
                            </li>
                        }
                        .into_any()
                    })
                    .collect_view();
                view! { <ul class=class::CHAT_LOG>{messages}</ul> }.into_any()
            }
            other => tile_for(ctx, &entity, &other, text::NOTHING),
        }
    };

    view! {
        <div class=class::CHAT>
            {log}
            <div class=class::COMPOSER>
                <textarea class=class::TEXTAREA aria-label=text::COMPOSER
                    placeholder=text::COMPOSER
                    on:input=move |ev| draft.set(event_target_value(&ev))></textarea>
                <button class=class::BUTTON on:click=move |_| {
                    let text = draft.get_untracked();
                    if text.trim().is_empty() {
                        return;
                    }
                    ctx.on_intent.run(Intent::Send {
                        entity: send_entity.clone(),
                        text,
                    });
                    draft.set(String::new());
                }>{text::SEND}</button>
            </div>
        </div>
    }
    .into_any()
}

/// L'état du fil, sans emprunt au `Snapshot` : ce que la fenêtre décide, déjà rangé. Le
/// `Memo` qui le porte ne notifie le fil que quand CET état change — un message qui change
/// en place (son contenu, son journal) garde les mêmes ids et ne reconstruit pas le fil.
#[derive(Clone, Debug, PartialEq)]
enum FilEtat {
    Lignes { ids: Vec<String>, hidden: usize },
    Vide,
    Chargement,
    Echec(String),
    Refuse(String),
    Inconnu,
}

/// Les champs qu'une bulle lit sur sa ligne, résolus UNE fois sur le module puis relus sur
/// chaque ligne sans repasser par le descripteur.
#[derive(Clone, Debug, PartialEq)]
struct ChampsBulle {
    content: String,
    author: Option<String>,
    role: Option<String>,
    state: Option<String>,
    steps: Option<String>,
}

/// Une bulle dessinée pour UNE ligne, mémorisée par son `id` : tant que la ligne ne change
/// pas, le `Memo` rend la même valeur et le rendu de la bulle ne s'exécute pas, même si une
/// AUTRE ligne du fil a changé. C'est la frontière qui rend le fil « par ligne ».
fn bubble_ligne(
    ctx: Ctx,
    entity: String,
    id: String,
    champs: ChampsBulle,
    markdown: bool,
) -> AnyView {
    let ligne = Memo::new(move |_| {
        let snapshot = ctx.data.get();
        ligne_en_bulle(&snapshot, &entity, &id, &champs, markdown)
    });
    view! {
        {move || match ligne.get() {
            Some(row) => bubble(row),
            None => ().into_any(),
        }}
    }
    .into_any()
}

/// Une bulle par ligne lue : son auteur, son contenu, et les étapes d'outil que la ligne
/// porte. Filtrée sur une ligne, c'est UN message ; filtrée sur la conversation ouverte
/// (`{"$context": "conversation"}`), c'est le fil, dans l'ordre des lignes.
///
/// Trois décisions y sont prises, et une seule fois pour tous les hôtes :
///
/// - **le contenu est du markdown quand le module le dit** (`markdown: true`) ; sinon c'est
///   du texte, et ce qui est écrit est ce qui se lit ;
/// - **les étapes d'outil sont lues dans un champ de la ligne** (une liste JSON d'objets) :
///   `{"outil": "…", "etat": "fait|en cours|refusé|échec", "resume": "…", "ms": 12}`, plus
///   `{"dit": "…"}` pour une phrase écrite pendant le tour et `{"avis": "…"}` pour un avis.
///   Une ligne qui n'en a pas n'en dessine pas : rien n'est inventé ;
/// - **un fil se lit par le bas** : avec `page_size`, la brique ne dessine que les derniers
///   messages, et un bouton élargit la fenêtre vers le haut sans rien perdre de ce qui est
///   déjà affiché.
fn message(
    ctx: Ctx,
    entity: &str,
    filter: Option<&FilterExpr>,
    markdown: bool,
    page_size: Option<u32>,
) -> AnyView {
    let entity = entity.to_owned();
    let filter = filter.cloned();
    // Pas de repli pour un rôle, un état, un auteur : une brique qui lit un rôle ne doit pas
    // lire le contenu parce qu'elle n'a pas trouvé de rôle. `display_field` a un repli, et
    // c'est ce qu'il faut pour un CONTENU (quelque chose plutôt que rien) ; pas pour le reste.
    let champs = ChampsBulle {
        content: display_field(
            ctx.module,
            &entity,
            &["contenu", "content", "text", "body", "message"],
        ),
        author: named_field(
            ctx.module,
            &entity,
            &["auteur", "author", "sender", "from", "nom", "name"],
        ),
        role: named_field(ctx.module, &entity, &["role", "kind", "type"]),
        state: named_field(ctx.module, &entity, &["etat", "state", "status"]),
        steps: named_field(
            ctx.module,
            &entity,
            &["journal", "etapes", "steps", "outils", "tools"],
        ),
    };
    let window = page_size.map(|size| size as usize).unwrap_or(0);
    let fenetre = RwSignal::new(window);

    // Les ids à dessiner, dans l'ordre du fil. Un message qui change EN PLACE ne change pas
    // cette liste : le `Memo` rend la même valeur et le fil ne se reconstruit pas. Seule la
    // fenêtre (`fenetre`) ou l'arrivée/départ d'une ligne la fait changer.
    let entite = entity.clone();
    let fenetre_ouverte = Memo::new(move |_| {
        let snapshot = ctx.data.get();
        match data::read(&snapshot, &entite, filter.as_ref(), &[], 0, None) {
            Reading::Rows(rows) => {
                let total = rows.rows.len();
                let (hidden, _) = if window == 0 {
                    (0, total)
                } else {
                    thread_window(total, fenetre.get())
                };
                let ids = rows.rows[hidden..]
                    .iter()
                    .filter_map(|row| data::value_of(row, "id").and_then(Value::as_str))
                    .map(str::to_owned)
                    .collect();
                FilEtat::Lignes { ids, hidden }
            }
            Reading::Empty => FilEtat::Vide,
            Reading::Loading => FilEtat::Chargement,
            Reading::Failed(why) => FilEtat::Echec(why.to_owned()),
            Reading::Denied(why) => FilEtat::Refuse(why.to_owned()),
            Reading::Unknown => FilEtat::Inconnu,
        }
    });

    let log = move || match fenetre_ouverte.get() {
        FilEtat::Lignes { ids, hidden } => {
            let older = (hidden > 0).then(|| {
                let label = format!("{} · {hidden} {}", text::OLDER, text::OLDER_HIDDEN);
                let pas = window.max(1);
                view! {
                    <button class=class::OLDER on:click=move |_| {
                        fenetre.update(|seen| *seen += pas);
                    }>{label}</button>
                }
            });
            let bubbles = ids
                .into_iter()
                .map(|id| bubble_ligne(ctx, entity.clone(), id, champs.clone(), markdown))
                .collect_view();
            view! {
                <div class=class::CHAT>
                    {older}
                    <div class=class::CHAT_LOG>{bubbles}</div>
                </div>
            }
            .into_any()
        }
        FilEtat::Vide => empty_tile(ctx, &entity, text::NOTHING),
        FilEtat::Chargement => loading_tile(),
        FilEtat::Echec(why) => failed_tile(ctx, &entity, &why),
        FilEtat::Refuse(why) => denied_tile(ctx, &entity, &why),
        FilEtat::Inconnu => missing_tile(&entity),
    };
    view! { {log} }.into_any()
}

/// Le fil d'une fenêtre : combien de messages restent cachés en haut. Une fenêtre de `0`
/// (ou plus grande que le fil) ne cache rien.
///
/// Pure, et publique, parce que c'est la règle qui compte — élargir la fenêtre ne doit
/// jamais perdre un message déjà affiché — et qu'une règle se mesure sans DOM.
pub fn thread_hidden(total: usize, window: usize) -> usize {
    total.saturating_sub(window)
}

/// Les deux indices d'une fenêtre de fil : `(cachés, dessinés)`. Le fil se lit par le bas :
/// les messages dessinés sont les DERNIERS, et la fenêtre ne fait que grandir vers le haut.
pub fn thread_window(total: usize, window: usize) -> (usize, usize) {
    let hidden = thread_hidden(total, window);
    (hidden, total - hidden)
}

/// Un message prêt à dessiner : ce que la ligne porte, déjà trié. `PartialEq` est ce qui
/// permet au `Memo` d'une bulle de ne pas redessiner quand une AUTRE ligne change.
#[derive(Clone, Debug, PartialEq)]
struct RowView {
    role: String,
    state: String,
    author: Option<String>,
    text: String,
    markdown: bool,
    entries: Vec<StepEntry>,
}

fn row_view(
    role: &str,
    state: &str,
    author: Option<String>,
    text: &str,
    markdown: bool,
    entries: &[StepEntry],
) -> RowView {
    RowView {
        role: role.to_owned(),
        state: state.to_owned(),
        author,
        text: text.to_owned(),
        markdown,
        entries: entries.to_vec(),
    }
}

/// Ce qu'une ligne devient quand on la dessine seule : elle ne lit QUE sa ligne (par `id`),
/// pas le feed. C'est la frontière qui permet à une bulle de changer sans redessiner les
/// autres : le `Memo` qui l'appelle ne rend une valeur nouvelle que si CETTE ligne a changé.
fn ligne_en_bulle(
    snapshot: &Snapshot,
    entity: &str,
    id: &str,
    champs: &ChampsBulle,
    markdown: bool,
) -> Option<RowView> {
    let row = snapshot.ligne(entity, id)?;
    let text = data::display(data::value_of(row, &champs.content));
    let who = champs
        .author
        .as_deref()
        .and_then(|field| match data::value_of(row, field) {
            None | Some(Value::Null) => None,
            Some(Value::String(s)) if s.is_empty() => None,
            Some(value) => Some(data::display(Some(value))),
        });
    let role_value = champs
        .role
        .as_deref()
        .map(|field| data::value_of(row, field))
        .and_then(|value| value.and_then(Value::as_str).map(str::to_owned))
        .unwrap_or_default();
    let state_value = champs
        .state
        .as_deref()
        .map(|field| data::value_of(row, field))
        .and_then(|value| value.and_then(Value::as_str).map(str::to_owned))
        .unwrap_or_default();
    let entries = champs
        .steps
        .as_deref()
        .and_then(|field| data::value_of(row, field))
        .map(step_entries)
        .unwrap_or_default();
    Some(row_view(
        &role_value,
        &state_value,
        who,
        &text,
        markdown,
        &entries,
    ))
}

/// Une entrée du journal d'une bulle : une étape d'outil, une phrase du tour, un avis.
#[derive(Clone, Debug, PartialEq)]
pub enum StepEntry {
    /// Un appel d'outil : son nom, son état, son résumé, sa durée.
    Tool {
        name: String,
        state: String,
        resume: String,
        ms: i64,
    },
    /// Ce que le modèle a écrit pendant le tour.
    Said(String),
    /// Un avis posé par l'hôte pendant le tour.
    Noted(String),
}

/// Les entrées d'un champ de journal : une liste d'objets, ou son texte JSON.
///
/// Un champ vide, un texte qui n'est pas du JSON, un objet sans nom d'outil : rien n'est
/// dessiné. Le rendu d'un journal n'est pas l'endroit où deviner.
pub fn step_entries(value: &Value) -> Vec<StepEntry> {
    let parsed;
    let list = match value {
        Value::Array(items) => items,
        Value::String(raw) if raw.trim_start().starts_with('[') => {
            match serde_json::from_str::<Value>(raw) {
                Ok(Value::Array(items)) => {
                    parsed = items;
                    &parsed
                }
                _ => return Vec::new(),
            }
        }
        _ => return Vec::new(),
    };
    list.iter()
        .filter_map(|event| {
            // Deux écritures pour la même chose, et les deux se lisent : la forme `genre` de
            // l'Atelier (`{"genre":"dit","texte":"…"}`) et la forme courte (`{"dit":"…"}`),
            // qui est celle qu'un module écrira naturellement.
            match event.get("genre").and_then(Value::as_str) {
                Some("dit") => {
                    return field_text(event, &["texte", "text", "dit"]).map(StepEntry::Said);
                }
                Some("avis") => {
                    return field_text(event, &["texte", "text", "avis"]).map(StepEntry::Noted);
                }
                _ => {}
            }
            if let Some(said) = field_text(event, &["dit"]) {
                return Some(StepEntry::Said(said));
            }
            if let Some(note) = field_text(event, &["avis"]) {
                return Some(StepEntry::Noted(note));
            }
            let name = field_text(event, &["outil", "tool", "name", "nom"])?;
            let state = match (
                event.get("en_cours"),
                event.get("refus"),
                event.get("echec"),
            ) {
                (Some(Value::Bool(true)), _, _) => "en cours".to_owned(),
                (_, Some(Value::Bool(true)), _) => "refusé".to_owned(),
                (_, _, Some(Value::Bool(true))) => "échec".to_owned(),
                _ => {
                    field_text(event, &["etat", "state", "status"]).unwrap_or_else(|| "fait".into())
                }
            };
            Some(StepEntry::Tool {
                name,
                state,
                resume: field_text(event, &["resume", "result", "resultat", "detail"])
                    .unwrap_or_default(),
                ms: event.get("ms").and_then(Value::as_i64).unwrap_or(0),
            })
        })
        .collect()
}

/// Le premier de `names` dont la valeur est une chaîne non vide.
fn field_text(value: &Value, names: &[&str]) -> Option<String> {
    names.iter().find_map(|name| {
        value
            .get(*name)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|text| !text.is_empty())
            .map(str::to_owned)
    })
}

/// The classes of one bubble, from its role and its state. A module message is neither the
/// person nor the agent; a message still being written is not a finished answer; a refused
/// one says so.
fn bubble_class(role: &str, state: &str) -> String {
    let mut classes = vec![class::MESSAGE];
    match role {
        "utilisateur" | "user" | "personne" => classes.push(class::MESSAGE_USER),
        "module" => classes.push(class::MESSAGE_MODULE),
        _ => {}
    }
    if state == "en_cours" || state == "running" || state == "live" {
        classes.push(class::MESSAGE_LIVE);
    }
    classes.join(" ")
}

/// La prose d'une bulle, hors étapes : `None` quand le journal porte déjà le texte.
///
/// Le journal d'une bulle range, dans l'ordre, ce que le modèle a dit (`dit`) et les
/// appels d'outil. Quand il contient au moins une phrase, le champ `contenu` en est la
/// copie fondue : le rendre en plus ferait apparaître le texte deux fois, et les outils
/// seraient groupés après tout le texte au lieu de s'intercaler entre les phrases.
fn prose_de_bulle(texte: &str, entrees: &[StepEntry]) -> Option<String> {
    let le_journal_porte_le_texte = entrees.iter().any(|e| matches!(e, StepEntry::Said(_)));
    if le_journal_porte_le_texte || texte.trim().is_empty() {
        None
    } else {
        Some(texte.to_string())
    }
}

/// Une durée courte, lisible d'un coup d'œil : « 12 ms », « 1,2 s », « 45 s ».
fn duree_lisible(ms: i64) -> String {
    if ms < 1000 {
        format!("{ms} ms")
    } else if ms < 10_000 {
        format!("{:.1} s", ms as f64 / 1000.0)
    } else {
        format!("{} s", ms / 1000)
    }
}

/// Ce qu'une bulle dessine, entrée par entrée. Une phrase et un avis restent seuls ; les
/// appels d'outil CONSECUTIFS se replient en UN groupe. L'ordre chronologique du journal
/// est conservé : une phrase entre deux outils coupe le groupe.
#[derive(Clone, Debug, PartialEq)]
enum Segment {
    Said(String),
    Noted(String),
    Tools(Vec<StepEntry>),
}

/// Découpe les entrées d'une bulle en segments : les outils qui se suivent forment un
/// groupe, une phrase ou un avis reste à sa place.
fn segmente(entries: &[StepEntry]) -> Vec<Segment> {
    let mut segments: Vec<Segment> = Vec::new();
    let mut outils: Vec<StepEntry> = Vec::new();
    for entry in entries {
        if matches!(entry, StepEntry::Tool { .. }) {
            outils.push(entry.clone());
            continue;
        }
        if !outils.is_empty() {
            segments.push(Segment::Tools(std::mem::take(&mut outils)));
        }
        segments.push(match entry {
            StepEntry::Said(texte) => Segment::Said(texte.clone()),
            StepEntry::Noted(texte) => Segment::Noted(texte.clone()),
            StepEntry::Tool { .. } => unreachable!("déjà filtré"),
        });
    }
    if !outils.is_empty() {
        segments.push(Segment::Tools(outils));
    }
    segments
}

/// Le libellé d'un groupe d'outils : « 3 outils · lire, écrire, compiler · 1,2 s ». Sans
/// durée, la ligne ne nomme que les outils.
fn tool_group_label(tools: &[StepEntry]) -> String {
    let n = tools.len();
    let noms: Vec<&str> = tools
        .iter()
        .filter_map(|entry| match entry {
            StepEntry::Tool { name, .. } => Some(name.as_str()),
            _ => None,
        })
        .collect();
    let total_ms: i64 = tools
        .iter()
        .filter_map(|entry| match entry {
            StepEntry::Tool { ms, .. } => Some(*ms),
            _ => None,
        })
        .sum();
    let mot = if n == 1 { "outil" } else { "outils" };
    let mut label = format!("{n} {mot} · {}", noms.join(", "));
    if total_ms > 0 {
        label.push_str(&format!(" · {}", duree_lisible(total_ms)));
    }
    label
}

/// Une étape d'outil, dessinée telle quelle : son état, son nom, sa durée, son résultat.
/// Le résultat n'est PAS déversé par défaut : il ne s'ouvre qu'à l'intérieur d'un groupe.
fn tool_item(tool: StepEntry) -> AnyView {
    match tool {
        StepEntry::Tool {
            name,
            state,
            resume,
            ms,
        } => {
            let tone = step_tone(&state);
            let time = (ms > 0).then(|| {
                view! {
                    <span class=class::STEP_TIME>{format!("{ms} ms")}</span>
                }
            });
            let detail = (!resume.is_empty()).then(|| {
                let short: String = resume.chars().take(400).collect();
                view! { <span class=class::STEP_RESUME>{short}</span> }
            });
            view! {
                <li class=class::STEP>
                    {design::badge(state, tone)}
                    <span class=class::STEP_NAME>{name}</span>
                    {time}
                    {detail}
                </li>
            }
            .into_any()
        }
        _ => ().into_any(),
    }
}

/// One bubble: author, prose, tool steps.
fn bubble(row: RowView) -> AnyView {
    let RowView {
        role,
        state,
        author,
        text,
        markdown: as_markdown,
        entries,
    } = row;
    let class_name = bubble_class(&role, &state);
    let prose = prose_de_bulle(&text, &entries).map(|texte| {
        if as_markdown {
            view! { <div class=class::MESSAGE_BODY inner_html=markdown::markdown(&texte)></div> }
                .into_any()
        } else {
            view! { <p class=class::MESSAGE_BODY>{texte}</p> }.into_any()
        }
    });
    let steps = (!entries.is_empty()).then(|| {
        // Les outils CONSECUTIFS se replient en UNE ligne ; une phrase les coupe, comme
        // dans le journal. Le résultat brut d'un outil n'est pas déversé par défaut : il
        // s'ouvre dans le groupe, sur demande.
        let items = segmente(&entries)
            .into_iter()
            .map(|segment| match segment {
                Segment::Said(said) => view! {
                    <li class=class::MESSAGE_BODY inner_html=markdown::markdown(&said)></li>
                }
                .into_any(),
                Segment::Noted(note) => view! {
                    <li class=class::STEP role="status">{note}</li>
                }
                .into_any(),
                Segment::Tools(tools) => {
                    let label = tool_group_label(&tools);
                    let details = tools.into_iter().map(tool_item).collect_view();
                    view! {
                        <li class=class::STEP_GROUP>
                            <details>
                                <summary class=class::STEP_GROUP_SUMMARY>
                                    <span class=class::STEP_NAME>{label}</span>
                                </summary>
                                <ol class=class::STEPS>{details}</ol>
                            </details>
                        </li>
                    }
                    .into_any()
                }
            })
            .collect_view();
        view! { <ol class=class::STEPS>{items}</ol> }.into_any()
    });
    view! {
        <div class=class_name>
            {author.map(|author| {
                view! { <span class=class::MESSAGE_META>{author}</span> }.into_any()
            })}
            {prose}
            {steps}
        </div>
    }
    .into_any()
}

/// Ce qu'un avis annonce, ou `None` quand il n'a rien à dire.
///
/// C'est LA règle de la brique `notice`, isolée pour être mesurée sans DOM : une valeur
/// absente, nulle ou blanche n'annonce rien. Le contrat n'a pas de brique conditionnelle, et
/// aucun filtre ne sait dire « si ce champ est vide » ; c'est donc ici, et une seule fois,
/// que se décide ce qui apparaît et ce qui disparaît.
pub fn annonce(value: Option<&Value>) -> Option<String> {
    match value {
        None | Some(Value::Null) => None,
        Some(Value::String(text)) if text.trim().is_empty() => None,
        Some(Value::String(text)) => Some(text.trim().to_owned()),
        Some(other) => Some(data::display(Some(other))),
    }
}

/// Un avis lu dans une ligne : une bannière dont le texte est un champ.
///
/// Le point qui compte : **un champ vide n'annonce rien**. Les états d'une brique parlent de
/// la LECTURE (chargement, vide, échec, refus) et aucun filtre ne dit « si ce champ est
/// vide » ; sans cette règle, un bandeau d'arrêt resterait affiché après la reprise, et un
/// bouton « Continuer » resterait sous un fil qui n'attend plus rien. Une lecture en échec,
/// elle, se dit toujours.
fn notice(
    ctx: Ctx,
    tone: Tone,
    entity: &str,
    filter: Option<&FilterExpr>,
    field: &str,
    action: Option<&str>,
    label: Option<&str>,
) -> AnyView {
    let entity = entity.to_owned();
    let filter = filter.cloned();
    let field = field.to_owned();
    let action = action.map(str::to_owned);
    let label = label.map(str::to_owned);
    (move || {
        let snapshot = ctx.data.get();
        let reading = data::read(&snapshot, &entity, filter.as_ref(), &[], 0, None);
        let (tone, message) = match &reading {
            Reading::Rows(rows) => {
                let Some(row) = rows.rows.first().copied() else {
                    return ().into_any();
                };
                let Some(text) = annonce(data::value_of(row, &field)) else {
                    return ().into_any();
                };
                (tone, text)
            }
            // Rien à annoncer tant que rien n'est arrivé : un avis n'est pas une tuile.
            Reading::Loading | Reading::Empty => return ().into_any(),
            Reading::Failed(reason) => (Tone::Error, (*reason).to_owned()),
            Reading::Denied(reason) => (Tone::Error, (*reason).to_owned()),
            Reading::Unknown => {
                return fallback(&format!("avis : {entity} ({})", text::UNKNOWN_ENTITY)).into_any();
            }
        };
        let id = match &reading {
            Reading::Rows(rows) => rows
                .rows
                .first()
                .and_then(|row| data::value_of(row, "id"))
                .and_then(Value::as_str)
                .map(str::to_owned),
            _ => None,
        };
        let (class_name, role) = match tone {
            Tone::Error => (class::TONE_ERROR, "alert"),
            Tone::Warning => (class::TONE_WARNING, "status"),
            Tone::Success => (class::TONE_SUCCESS, "status"),
            Tone::Info => (class::TONE_INFO, "status"),
        };
        let gesture = action.clone().map(|name| {
            let label = ctx.label_of(&name, label.as_deref());
            let intent = action_intent(ctx, &name, Some(&entity), id.clone(), Vec::new());
            view! {
                <button class=class::BUTTON title=name.clone()
                    on:click=move |_| ctx.on_intent.run(intent.clone())>{label}</button>
            }
            .into_any()
        });
        view! {
            <div class=format!("{} {}", class::BANNER, class_name) role=role>
                <span>{message}</span>
                {gesture}
            </div>
        }
        .into_any()
    })
    .into_any()
}

/// La carte d'une question posée par l'agent : son texte, ses choix, la réponse déjà donnée.
///
/// Les lignes viennent de l'hôte (le tour en cours les tient, aucune table ne les range) ;
/// la carte vient du module. Un clic — ou une réponse tapée — émet une intention de réponse
/// que l'hôte remet au guichet de l'agent. Aucune action du module n'est appelée.
fn question_card(
    ctx: Ctx,
    entity: &str,
    filter: Option<&FilterExpr>,
    text_field: &str,
    options_field: &str,
    answer_field: Option<&str>,
) -> AnyView {
    let entity = entity.to_owned();
    let filter = filter.cloned();
    let text_field = text_field.to_owned();
    let options_field = options_field.to_owned();
    let answer_field = answer_field.map(str::to_owned);
    (move || {
        let snapshot = ctx.data.get();
        match data::read(&snapshot, &entity, filter.as_ref(), &[], 0, None) {
            Reading::Rows(rows) => {
                let cards = rows
                    .rows
                    .iter()
                    .map(|row| {
                        let id = data::value_of(row, "id")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_owned();
                        let question = data::value_of(row, &text_field)
                            .map(|value| data::display(Some(value)))
                            .unwrap_or_default();
                        let answer = answer_field
                            .as_deref()
                            .and_then(|field| data::value_of(row, field))
                            .and_then(Value::as_str)
                            .map(str::trim)
                            .filter(|text| !text.is_empty())
                            .map(str::to_owned);
                        let choices: Vec<String> = data::value_of(row, &options_field)
                            .map(choice_list)
                            .unwrap_or_default();
                        let entity_click = entity.clone();
                        let already = answer.is_some();
                        let buttons = choices.into_iter().map(|choice| {
                            let (entity_click, id_click) =
                                (entity_click.clone(), id.clone());
                            let label = choice.clone();
                            view! {
                                <button class=class::QUESTION_CHOICE
                                    disabled=already
                                    on:click=move |_| {
                                        ctx.on_intent.run(Intent::Answer {
                                            entity: entity_click.clone(),
                                            id: id_click.clone(),
                                            choice: choice.clone(),
                                        });
                                    }>{label}</button>
                            }
                            .into_any()
                        });
                        let free = answer.is_none().then(|| {
                            let field_id = format!("vw-answer-{}", sanitise(&id));
                            let entity_key =
                                (entity.clone(), id.clone(), field_id.clone());
                            let entity_send =
                                (entity.clone(), id.clone(), field_id.clone());
                            view! {
                                <div class=class::QUESTION_FREE>
                                    <input class=class::INPUT
                                        id=field_id.clone()
                                        type="text"
                                        aria-label=text::ANSWER_FREE
                                        placeholder=text::ANSWER_FREE
                                        on:keydown=move |ev: web_sys::KeyboardEvent| {
                                            if ev.key() != "Enter" {
                                                return;
                                            }
                                            if let Some(value) = input_value(&entity_key.2) {
                                                ctx.on_intent.run(Intent::Answer {
                                                    entity: entity_key.0.clone(),
                                                    id: entity_key.1.clone(),
                                                    choice: value,
                                                });
                                            }
                                        } />
                                    <button class=class::BUTTON aria-label=text::ANSWER
                                        title=text::ANSWER
                                        on:click=move |_| {
                                            if let Some(value) = input_value(&entity_send.2) {
                                                ctx.on_intent.run(Intent::Answer {
                                                    entity: entity_send.0.clone(),
                                                    id: entity_send.1.clone(),
                                                    choice: value,
                                                });
                                            }
                                        }>{text::ANSWER}</button>
                                </div>
                            }
                            .into_any()
                        });
                        let answered = answer.map(|answer| {
                            view! {
                                <p class=class::QUESTION_ANSWER>{format!("{} : {answer}", text::ANSWERED)}</p>
                            }
                            .into_any()
                        });
                        view! {
                            <div class=class::QUESTION role="group" aria-label=text::QUESTION>
                                <p class=class::QUESTION_TEXT>{question}</p>
                                <div class=class::QUESTION_CHOICES>{buttons.collect_view()}</div>
                                {answered}
                                {free}
                            </div>
                        }
                        .into_any()
                    })
                    .collect_view();
                view! { <div class=class::CHAT>{cards}</div> }.into_any()
            }
            Reading::Empty | Reading::Loading => ().into_any(),
            other => tile_for(ctx, &entity, &other, text::NOTHING),
        }
    })
    .into_any()
}

/// Les réponses possibles d'une ligne : une liste de textes, ou son texte JSON. Un texte
/// qui n'est pas une liste se lit comme UN choix — il ne disparaît pas.
fn choice_list(value: &Value) -> Vec<String> {
    match value {
        Value::Array(items) => items
            .iter()
            .filter_map(|item| match item {
                Value::String(text) => Some(text.clone()),
                other => other.as_str().map(str::to_owned),
            })
            .collect(),
        Value::String(raw) => match serde_json::from_str::<Value>(raw) {
            Ok(Value::Array(items)) => choice_list(&Value::Array(items)),
            _ if !raw.trim().is_empty() => vec![raw.clone()],
            _ => Vec::new(),
        },
        _ => Vec::new(),
    }
}

/// Un identifiant qui peut servir d'`id` HTML.
fn sanitise(raw: &str) -> String {
    raw.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}

/// La valeur du champ `id`, lue dans le document de l'hôte.
fn input_value(id: &str) -> Option<String> {
    let element = web_sys::window()?.document()?.get_element_by_id(id)?;
    let element: web_sys::HtmlInputElement = element.dyn_into().ok()?;
    let value = element.value();
    (!value.trim().is_empty()).then(|| value.trim().to_owned())
}

/// The tone that reads a tool step's state without the colour carrying the meaning alone.
fn step_tone(state: &str) -> DesignTone {
    match state.to_ascii_lowercase().as_str() {
        "ok" | "done" | "success" | "succeeded" | "completed" => DesignTone::Ok,
        "error" | "failed" | "failure" | "refused" => DesignTone::Danger,
        "running" | "in_progress" | "active" => DesignTone::Info,
        "waiting" | "pending" | "queued" => DesignTone::Warn,
        _ => DesignTone::Muted,
    }
}

fn tool_step(ctx: Ctx, entity: &str, filter: Option<&FilterExpr>, field: &str) -> AnyView {
    let entity = entity.to_owned();
    let filter = filter.cloned();
    let field = field.to_owned();
    let name_field = display_field(ctx.module, &entity, &["name", "tool", "title", "step"]);
    (move || {
        let snapshot = ctx.data.get();
        match data::read(&snapshot, &entity, filter.as_ref(), &[], 0, None) {
            Reading::Rows(rows) => {
                let row = rows.rows.first().copied();
                let step_state = data::display(row.and_then(|row| data::value_of(row, &field)));
                let name = data::display(row.and_then(|row| data::value_of(row, &name_field)));
                view! {
                    <div class=class::TOOLSTEP>
                        {design::badge(step_state.clone(), step_tone(&step_state))}
                        <span>{name}</span>
                    </div>
                }
                .into_any()
            }
            other => tile_for(ctx, &entity, &other, text::NOTHING),
        }
    })
    .into_any()
}

fn queue(
    ctx: Ctx,
    entity: &str,
    filter: Option<&FilterExpr>,
    label: &str,
    sort: &[SortSpec],
) -> AnyView {
    let entity = entity.to_owned();
    let filter = filter.cloned();
    let label = label.to_owned();
    let sort: Vec<SortSpec> = sort.to_vec();
    (move || {
        let snapshot = ctx.data.get();
        match data::read(&snapshot, &entity, filter.as_ref(), &sort, 0, None) {
            Reading::Rows(rows) => {
                let items = rows
                    .rows
                    .iter()
                    .enumerate()
                    .map(|(index, row)| {
                        let text = data::display(data::value_of(row, &label));
                        view! {
                            <li class=class::QUEUE_ITEM>
                                <span class=class::QUEUE_INDEX>{(index + 1).to_string()}</span>
                                <span>{text}</span>
                            </li>
                        }
                        .into_any()
                    })
                    .collect_view();
                view! { <ol class=class::QUEUE>{items}</ol> }.into_any()
            }
            other => tile_for(ctx, &entity, &other, text::QUEUE_EMPTY),
        }
    })
    .into_any()
}

/// A bounded chart: one point per row of the reading, in read order. Rows whose field holds
/// no number are skipped; if none holds one, the state says so instead of an empty frame.
fn chart(
    ctx: Ctx,
    entity: &str,
    filter: Option<&FilterExpr>,
    field: &str,
    kind: &str,
    label: Option<&str>,
    height: Option<u32>,
) -> AnyView {
    let entity = entity.to_owned();
    let filter = filter.cloned();
    let field = field.to_owned();
    let kind = kind.to_owned();
    let label = label.map(str::to_owned);
    (move || {
        let snapshot = ctx.data.get();
        let reading = data::read(&snapshot, &entity, filter.as_ref(), &[], 0, None);
        let values: Vec<f64> = match &reading {
            Reading::Rows(rows) => rows
                .rows
                .iter()
                .filter_map(|row| match data::value_of(row, &field) {
                    Some(Value::Number(n)) => n.as_f64(),
                    Some(Value::String(s)) => s.parse::<f64>().ok(),
                    _ => None,
                })
                .collect(),
            _ => Vec::new(),
        };
        // The container always says how many points it drew and which field it plots, even
        // when that is none: a measuring host (`apercu`) must see an empty chart.
        let points = values.len().to_string();
        if values.is_empty() {
            let why = match reading {
                Reading::Rows(_) => text::CHART_NO_NUMBER,
                _ => text::CHART_NO_ROW,
            };
            let body = match reading {
                Reading::Rows(_) | Reading::Empty => empty_tile(ctx, &entity, why),
                other => tile_for(ctx, &entity, &other, why),
            };
            return view! {
                <div class=class::CHART data-chart-points=points data-chart-field=field.clone()>
                    {body}
                </div>
            }
            .into_any();
        }
        let drawn = chart::draw(&kind, &values, height, label.as_deref());
        let title = label
            .clone()
            .map(|t| view! { <p class=class::CHART_TITLE>{t}</p> });
        view! {
            <div class=class::CHART data-chart-points=points data-chart-field=field.clone()>
                {title}
                <div class=class::CHART_SCROLL inner_html=drawn.svg></div>
            </div>
        }
        .into_any()
    })
    .into_any()
}

fn metrics(ctx: Ctx, entity: &str, filter: Option<&FilterExpr>, field: Option<&str>) -> AnyView {
    let entity = entity.to_owned();
    let filter = filter.cloned();
    let field = field.map(str::to_owned);
    (move || {
        let snapshot = ctx.data.get();
        let label = field
            .as_deref()
            .map(placement::humanise)
            .unwrap_or_else(|| placement::humanise(&entity));
        match data::read(&snapshot, &entity, filter.as_ref(), &[], 0, None) {
            Reading::Rows(rows) => {
                let value = match field.as_deref() {
                    Some(field) => crate::model::count(data::sum(&rows.rows, field)),
                    None => crate::model::count(rows.total as f64),
                };
                view! { <div class=class::METRICS>{design::kpi(&label, value, String::new())}</div> }
                    .into_any()
            }
            // Zero rows is a number, not an emptiness: a counter of nothing says zero.
            Reading::Empty => view! {
                <div class=class::METRICS>{design::kpi(&label, text::ZERO.to_owned(), String::new())}</div>
            }
            .into_any(),
            other => tile_for(ctx, &entity, &other, text::NOTHING),
        }
    })
    .into_any()
}

fn program(
    ctx: Ctx,
    entity: &str,
    filter: Option<&FilterExpr>,
    args: &[ProgramArg],
    run: Run,
    columns: &[Column],
) -> AnyView {
    let entity_owned = entity.to_owned();
    let filter = filter.cloned();
    let id_base = next_id("vw-program");
    let values: Vec<(String, RwSignal<FieldValue>)> = args
        .iter()
        .map(|arg| (arg.name.clone(), RwSignal::new(FieldValue::Empty)))
        .collect();
    let controls: Vec<AnyView> = args
        .iter()
        .zip(values.iter())
        .map(|(arg, (name, signal))| {
            let kind = match arg.kind {
                ArgKind::Text | ArgKind::Date => Control::Text,
                ArgKind::Number => Control::Numeric,
                ArgKind::Bool => Control::Toggle,
            };
            control(
                ctx,
                &format!("{id_base}-{name}"),
                name,
                &placement::humanise(name),
                kind,
                false,
                FieldValue::Empty,
                *signal,
                None,
            )
        })
        .collect();
    let controls = controls.into_iter().collect_view();
    let heads: Vec<AnyView> = columns
        .iter()
        .map(|column| {
            let source = match &column.source {
                ColumnSource::Field { field } => format!(" · {}", placement::humanise(field)),
                ColumnSource::Count => " · n".to_owned(),
                ColumnSource::Sum { field } => {
                    format!(" · Σ {}", placement::humanise(field))
                }
            };
            let label = format!("{}{source}", placement::humanise(&column.name));
            view! { <span class=class::COLUMN>{label}</span> }.into_any()
        })
        .collect();
    let heads = heads.into_iter().collect_view();
    let scope = {
        let entity = entity_owned.clone();
        let filter = filter.clone();
        move || {
            let snapshot = ctx.data.get();
            match data::read(&snapshot, &entity, filter.as_ref(), &[], 0, None) {
                Reading::Rows(rows) => format!("{} ligne(s)", rows.total),
                Reading::Empty => text::ZERO.to_owned(),
                Reading::Loading => text::LOADING.to_owned(),
                Reading::Failed(reason) => reason.to_owned(),
                Reading::Denied(reason) => reason.to_owned(),
                Reading::Unknown => text::UNKNOWN_ENTITY.to_owned(),
            }
        }
    };
    let trigger = match run {
        Run::Manual => {
            let collect = values.clone();
            view! {
                <button class=class::BUTTON on:click=move |_| {
                    let args: Values = collect
                        .iter()
                        .map(|(name, signal)| (name.clone(), signal.get_untracked()))
                        .collect();
                    ctx.on_intent.run(Intent::Run {
                        entity: entity_owned.clone(),
                        args,
                    });
                }>{text::RUN}</button>
            }
            .into_any()
        }
        Run::OnChange => view! { <p class=class::HINT>{text::RUN_AUTO}</p> }.into_any(),
    };
    view! {
        <div class=class::PROGRAM>
            <div class=class::PROGRAM_ARGS>{controls}</div>
            {trigger}
            <div class=class::COLUMNS>{heads}</div>
            <p class=class::HINT>{text::PROGRAM_RESULT}" · "{scope}</p>
        </div>
    }
    .into_any()
}

#[cfg(test)]
mod tests {
    //! The decisions this file takes *without* a DOM: which control a declared type
    //! becomes, what a click means, how a step's state reads as a tone. The views
    //! themselves need a browser; these do not.

    use super::*;
    use serde_json::json;
    use this_descriptor::views::HostSurface;

    fn view_desc(look: Option<this_descriptor::views::Look>) -> ViewDesc {
        ViewDesc {
            live: None,
            name: "invoices".into(),
            surface: Surface::Main,
            title: None,
            replaces: None,
            look,
            root: Node::Stack { children: vec![] },
        }
    }

    fn composer_module() -> ModuleDescriptor {
        ModuleDescriptor::from_json(
            &json!({
                "name": "atelier", "version": "1.0.0",
                "entities": [{"name": "note", "fields": [{"name": "titre", "ty": "string"}]}],
                "actions": [
                    {"kind": "agent_tool", "name": "resumer", "op": "send_message", "text": "Résume."},
                    {"kind": "create", "name": "noter", "entity": "note", "fields": ["titre"]},
                    {"kind": "agent_tool", "name": "stop", "op": "stop_run"}
                ],
                "views": [{"name": "saisie", "surface": "chat", "root": {"type": "stack", "children": [
                    {"type": "leaf", "brick": {"type": "text", "entity": "note", "field": "titre"}},
                    {"type": "leaf", "brick": {"type": "composer", "placeholder": "Écris…",
                        "actions": ["resumer", "noter"], "attachments": true}}
                ]}}]
            })
            .to_string(),
        )
        .expect("module")
    }

    #[test]
    fn a_composer_is_found_in_its_tree_and_read_as_a_spec() {
        let m = composer_module();
        let spec = ComposerSpec::find(&m.views[0].root).expect("a composer is declared");
        assert_eq!(
            spec,
            ComposerSpec {
                placeholder: Some("Écris…".into()),
                actions: vec!["resumer".into(), "noter".into()],
                attachments: true,
            }
        );
        let other = Node::Stack { children: vec![] };
        assert_eq!(
            ComposerSpec::find(&other),
            None,
            "no composer, the host keeps its own"
        );
    }

    #[test]
    fn a_tree_without_its_composer_keeps_the_rest_and_a_bare_composer_leaves_nothing() {
        let m = composer_module();
        let rest = without_composer(&m.views[0].root).expect("a text is left");
        let Node::Stack { children } = rest else {
            panic!("still a stack")
        };
        assert_eq!(children.len(), 1);
        let bare = Node::Leaf {
            brick: Box::new(Brick::Composer {
                placeholder: None,
                actions: vec![],
                attachments: false,
            }),
        };
        assert_eq!(without_composer(&bare), None);
        let only = Node::Stack {
            children: vec![bare],
        };
        assert_eq!(
            without_composer(&only),
            None,
            "an emptied container draws nothing"
        );
    }

    #[test]
    fn settings_lines_emit_navigate_with_its_module_and_a_secret_by_name() {
        let mut m = ModuleDescriptor::new("atelier", "1.0.0");
        m.actions.push(
            serde_json::from_str(
                r#"{"kind":"navigate","name":"ouvrir","view":"providers","module":"reglages"}"#,
            )
            .unwrap(),
        );
        m.actions.push(
            serde_json::from_str(r#"{"kind":"request_secret","name":"cle","secret":"api-key"}"#)
                .unwrap(),
        );
        assert_eq!(
            settings_intent(&m, "ouvrir"),
            Intent::Navigate {
                view: "providers".into(),
                action: Some("ouvrir".into()),
                module: Some("reglages".into()),
            }
        );
        let secret = settings_intent(&m, "cle");
        assert_eq!(
            secret,
            Intent::Secret {
                action: "cle".into(),
                secret: "api-key".into()
            }
        );
        // The intent carries a NAME: there is no field in which a value could travel.
        assert_eq!(secret.kind(), "secret");
        assert_eq!(secret.entity(), None);
    }

    /// Une surface de l'hôte : le module l'offre dans son menu, l'intention dit LAQUELLE et
    /// sous quel nom. L'hôte seul sait où elle vit — l'intention ne porte donc aucune entité.
    #[test]
    fn a_host_action_emits_the_surface_it_offers() {
        let mut m = ModuleDescriptor::new("atelier", "1.0.0");
        m.actions.push(
            serde_json::from_str(r#"{"kind":"host","name":"modules","surface":"modules"}"#)
                .unwrap(),
        );
        m.actions.push(
            serde_json::from_str(r#"{"kind":"host","name":"reglages","surface":"settings"}"#)
                .unwrap(),
        );
        assert_eq!(
            settings_intent(&m, "modules"),
            Intent::Host {
                action: "modules".into(),
                surface: HostSurface::Modules,
            }
        );
        let settings = settings_intent(&m, "reglages");
        assert_eq!(settings.kind(), "host");
        assert_eq!(settings.entity(), None);
        assert_eq!(settings.action(), Some("reglages"));
        assert!(matches!(
            settings,
            Intent::Host {
                surface: HostSurface::Settings,
                ..
            }
        ));
    }

    #[test]
    fn a_composer_action_emits_the_intent_a_button_would_without_a_row() {
        let m = composer_module();
        assert_eq!(
            composer_intent(&m, "resumer"),
            Intent::Tool {
                action: "resumer".into(),
                op: this_descriptor::views::AgentOp::SendMessage,
                entity: None,
                text: Some("Résume.".into()),
            }
        );
        assert!(matches!(
            composer_intent(&m, "noter"),
            Intent::Create { ref entity, .. } if entity == "note"
        ));
        assert!(matches!(
            composer_intent(&m, "stop"),
            Intent::Tool {
                op: this_descriptor::views::AgentOp::StopRun,
                ..
            }
        ));
    }

    #[test]
    fn a_declared_field_type_becomes_the_right_control() {
        assert_eq!(control_of_kind(Some(FieldKind::Bool)), Control::Toggle);
        assert_eq!(control_of_kind(Some(FieldKind::Int)), Control::Numeric);
        assert_eq!(control_of_kind(Some(FieldKind::Decimal)), Control::Numeric);
        assert_eq!(
            control_of_kind(Some(FieldKind::Enum(vec!["draft".into()]))),
            Control::Choice(vec!["draft".into()])
        );
        assert_eq!(
            control_of_kind(Some(FieldKind::Ref("customer".into()))),
            Control::Reference("customer".into())
        );
        assert_eq!(control_of_kind(Some(FieldKind::Json)), Control::Multiline);
        assert_eq!(control_of_kind(Some(FieldKind::Text)), Control::Text);
        // A field the descriptor does not declare (or a type this build predates) is still
        // editable rather than dropped.
        assert_eq!(control_of_kind(None), Control::Text);
    }

    #[test]
    fn a_brick_input_kind_wins_over_the_field_type() {
        assert_eq!(control_of_input(Input::Text, None), Control::Text);
        assert_eq!(control_of_input(Input::Multiline, None), Control::Multiline);
        assert_eq!(control_of_input(Input::File, None), Control::File);
        assert_eq!(control_of_input(Input::Toggle, None), Control::Toggle);
        assert_eq!(
            control_of_input(Input::Reference, Some("customer")),
            Control::Reference("customer".into())
        );
        // Reference without a target: `views::issues` refuses it, the renderer keeps the
        // field usable as text and says the declaration is incomplete.
        assert_eq!(control_of_input(Input::Reference, None), Control::Text);
        assert_eq!(control_of_input(Input::Reference, Some("")), Control::Text);
    }

    #[test]
    fn every_click_kind_becomes_one_intent() {
        assert_eq!(
            click_intent(&Click::Select, "invoice", "42"),
            Intent::Select {
                entity: "invoice".into(),
                id: "42".into()
            }
        );
        assert_eq!(
            click_intent(&Click::OpenDetail, "invoice", "42"),
            Intent::Open {
                entity: "invoice".into(),
                id: "42".into()
            }
        );
        assert_eq!(
            click_intent(
                &Click::Navigate {
                    view: "detail".into()
                },
                "invoice",
                "42"
            ),
            Intent::Navigate {
                view: "detail".into(),
                action: None,
                module: None
            }
        );
    }

    #[test]
    fn a_row_value_becomes_a_form_value() {
        let row = json!({"flag": true, "off": false, "count": 3, "text": "abc", "none": null});
        assert_eq!(value_from(Some(&row), "flag"), FieldValue::Flag(true));
        assert_eq!(value_from(Some(&row), "off"), FieldValue::Flag(false));
        assert_eq!(
            value_from(Some(&row), "count"),
            FieldValue::Text("3".into())
        );
        assert_eq!(
            value_from(Some(&row), "text"),
            FieldValue::Text("abc".into())
        );
        assert_eq!(value_from(Some(&row), "none"), FieldValue::Empty);
        assert_eq!(value_from(Some(&row), "absent"), FieldValue::Empty);
        assert_eq!(value_from(None, "text"), FieldValue::Empty);
        assert_eq!(text_of(&FieldValue::Flag(true)), text::YES);
        assert_eq!(text_of(&FieldValue::Flag(false)), text::NO);
        assert_eq!(text_of(&FieldValue::Empty), "");
        assert_eq!(text_of(&FieldValue::File("a.pdf".into())), "a.pdf");
    }

    #[test]
    fn a_tool_step_state_reads_as_a_tone_and_a_word() {
        assert_eq!(step_tone("succeeded"), DesignTone::Ok);
        assert_eq!(step_tone("FAILED"), DesignTone::Danger);
        assert_eq!(step_tone("running"), DesignTone::Info);
        assert_eq!(step_tone("pending"), DesignTone::Warn);
        assert_eq!(step_tone("quelque chose"), DesignTone::Muted);
    }

    #[test]
    fn every_device_capability_has_a_readable_label() {
        for capability in [
            DeviceCapability::Camera,
            DeviceCapability::Microphone,
            DeviceCapability::Location,
            DeviceCapability::Screen,
            DeviceCapability::DeviceMotion,
        ] {
            assert!(!capability_label(capability).is_empty());
        }
    }

    #[test]
    fn an_unknown_brick_is_described_not_swallowed() {
        let described = describe_brick(&Brick::Banner {
            tone: Tone::Info,
            text: "x".into(),
        });
        assert!(described.contains("Banner"), "{described}");
    }

    #[test]
    fn the_declared_look_becomes_classes() {
        let plain = view_classes(&view_desc(None));
        assert!(plain.contains(class::ACCENT_NEUTRAL));
        assert!(plain.contains(class::LAYOUT_STACK));
        assert!(plain.contains(class::TRANSITION_NONE));

        let chosen = view_classes(&view_desc(Some(this_descriptor::views::Look {
            accent: this_descriptor::views::Accent::Danger,
            layout: this_descriptor::views::Layout::Grid,
            transition: this_descriptor::views::Transition::Fade,
        })));
        assert!(chosen.contains(class::ACCENT_DANGER));
        assert!(chosen.contains(class::LAYOUT_GRID));
        assert!(chosen.contains(class::TRANSITION_FADE));
    }

    #[test]
    fn a_row_identifier_is_what_a_link_needs() {
        let snapshot = Snapshot::new().with(
            "invoice",
            Feed::Ready(vec![json!({"id": "7", "number": "F-7"})]),
        );
        assert_eq!(
            data::target_id(&snapshot, "invoice", None),
            Some("7".to_owned())
        );
        assert_eq!(data::target_id(&snapshot, "customer", None), None);
    }

    /// Une feuille enveloppée dans un `when` : l'état d'ouverture EST la condition, portée
    /// par une ligne de l'entité — la feuille n'est dessinée que si la condition tient.
    #[test]
    fn a_popup_draws_only_while_its_condition_holds() {
        let filter = FilterExpr::Cmp {
            field: "ouverte".into(),
            op: this_descriptor::FilterOp::Eq,
            value: json!("oui"),
        };
        let ouverte = Snapshot::new().with(
            "popup",
            Feed::Ready(vec![json!({"id": "1", "ouverte": "oui"})]),
        );
        assert_eq!(
            data::condition(&ouverte, "popup", &filter),
            data::Condition::Holds
        );
        let fermee = Snapshot::new().with(
            "popup",
            Feed::Ready(vec![json!({"id": "1", "ouverte": "non"})]),
        );
        assert_eq!(
            data::condition(&fermee, "popup", &filter),
            data::Condition::Fails
        );
        let absente = Snapshot::new().with("popup", Feed::Ready(vec![]));
        assert_eq!(
            data::condition(&absente, "popup", &filter),
            data::Condition::Absent
        );
    }

    /// Refermer une feuille émet un `Dismiss` UNE fois ; une feuille déjà fermée n'émet rien.
    #[test]
    fn closing_a_popup_emits_dismiss_only_while_open() {
        assert_eq!(
            dismiss_intent("detail", true),
            Some(Intent::Dismiss {
                view: "detail".into()
            })
        );
        assert_eq!(dismiss_intent("detail", false), None);
    }

    /// `Escape` est la touche qui referme une feuille (le fond, lui, est un clic DOM).
    #[test]
    fn escape_is_the_key_that_closes_a_popup() {
        assert!(is_escape("Escape"));
        assert!(!is_escape("Enter"));
        assert!(!is_escape(""));
    }
}

#[cfg(test)]
mod tests_action_row {
    use super::*;

    /// Le bouton qui atteint le code d'un module doit dire LAQUELLE des lignes.
    #[test]
    fn une_operation_qui_vise_une_entite_a_besoin_de_la_ligne() {
        let vise_une_entite = Action::Invoke {
            label: None,
            name: "terminer".into(),
            operation: "terminer".into(),
            entity: Some("tache".into()),
        };
        assert!(action_needs_row(Some(&vise_une_entite)));

        let operation_de_module = Action::Invoke {
            label: None,
            name: "recalculer".into(),
            operation: "recalculer".into(),
            entity: None,
        };
        assert!(
            !action_needs_row(Some(&operation_de_module)),
            "une opération de module ne doit pas être désactivée sur une entité vide"
        );
    }

    #[test]
    fn les_ecritures_ont_besoin_de_la_ligne_et_la_creation_non() {
        let update = Action::Update {
            label: None,
            name: "ranger".into(),
            entity: "tache".into(),
            fields: vec!["fait".into()],
        };
        let delete = Action::Delete {
            label: None,
            name: "jeter".into(),
            entity: "tache".into(),
        };
        let create = Action::Create {
            label: None,
            name: "ajouter".into(),
            entity: "tache".into(),
            fields: vec!["titre".into()],
        };
        assert!(action_needs_row(Some(&update)));
        assert!(action_needs_row(Some(&delete)));
        assert!(!action_needs_row(Some(&create)));
        assert!(!action_needs_row(None));
    }

    /// Le chaînon manquant : l'intention d'une action de ligne porte l'identifiant de la
    /// ligne ET le nom de l'action — sinon le module reçoit un appel qu'il ne peut pas situer.
    #[test]
    fn une_action_de_ligne_porte_l_identifiant_de_la_ligne() {
        let autoriser = Action::Invoke {
            label: None,
            name: "autoriser".into(),
            operation: "autoriser".into(),
            entity: Some("autorisation".into()),
        };
        let intent = intent_of(
            Some(autoriser),
            "autoriser",
            Some("autorisation"),
            Some("cap-camera".into()),
            Vec::new(),
        );
        assert_eq!(
            intent,
            Intent::Invoke {
                action: "autoriser".into(),
                operation: "autoriser".into(),
                entity: Some("autorisation".into()),
                id: Some("cap-camera".into()),
                values: vec![],
            }
        );

        let retirer = Action::Delete {
            label: None,
            name: "retirer".into(),
            entity: "autorisation".into(),
        };
        assert_eq!(
            intent_of(
                Some(retirer),
                "retirer",
                Some("autorisation"),
                Some("cap-camera".into()),
                Vec::new(),
            ),
            Intent::Delete {
                action: "retirer".into(),
                entity: "autorisation".into(),
                id: Some("cap-camera".into()),
            }
        );
    }
}

#[cfg(test)]
mod tests_fil {
    //! Le corps du fil, mesuré sans DOM : ce qu'un journal d'outil devient, ce qu'une fenêtre
    //! de fil montre, ce qu'un avis annonce, ce qu'une carte de question propose.
    use super::*;
    use serde_json::json;
    use std::sync::Arc;

    // ---- le champ d'une entité d'un module requis --------------------------------------

    fn module_requerant(entites: &[&str], requis: &[&str]) -> ModuleDescriptor {
        let entites: Vec<_> = entites
            .iter()
            .map(|n| json!({"name": n, "fields": [{"name": "contenu", "ty": "text"}]}))
            .collect();
        let requis: Vec<_> = requis
            .iter()
            .map(|n| json!({"name": n, "version": "*"}))
            .collect();
        ModuleDescriptor::from_json(
            &json!({"name": if requis.is_empty() { "socle" } else { "atelier" },
                "version": "1.0.0", "requires": requis, "entities": entites})
            .to_string(),
        )
        .expect("module")
    }

    /// LE défaut de l'appareil : une bulle dont l'entité appartient à un module REQUIS lisait
    /// le champ « id » et affichait l'uuid. Avec le descripteur complété, elle lit `contenu`.
    #[test]
    fn le_champ_d_affichage_d_une_entite_d_un_module_requis_est_trouve() {
        let socle = module_requerant(&["message"], &[]);
        let atelier = module_requerant(&[], &["socle"]);
        let voulus = ["content", "text", "body", "message", "contenu"];

        let brut = StoredValue::new(atelier.clone());
        assert_eq!(display_field(brut, "message", &voulus), "id", "la cause");

        let complet = StoredValue::new(atelier.with_required_entities(&[&socle, &atelier]));
        assert_eq!(display_field(complet, "message", &voulus), "contenu");
    }

    // ---- les étapes d'outil d'une bulle --------------------------------------------------

    /// MESURÉ : le journal d'une bulle de l'Atelier est une LISTE d'objets rangée dans le
    /// champ `journal` d'une ligne. La brique `tool_step` lit UNE ligne d'une entité, donc
    /// elle ne peut pas atteindre l'étape d'une bulle : c'est la bulle qui les dessine.
    #[test]
    fn un_journal_de_bulle_devient_des_etapes_lisibles() {
        let journal = json!([
            { "genre": "dit", "texte": "je regarde" },
            { "id": "t1", "genre": "outil", "outil": "lire", "ms": 42,
              "refus": false, "echec": false, "en_cours": false, "resume": "3 lignes" },
            { "id": "t2", "genre": "outil", "outil": "ecrire", "ms": 7,
              "refus": true, "echec": false, "en_cours": false, "resume": "refusé" },
            { "id": "t3", "genre": "outil", "outil": "compiler", "ms": 0,
              "refus": false, "echec": true, "en_cours": false, "resume": "erreur" },
            { "id": "t4", "genre": "outil", "outil": "attendre", "ms": 0,
              "refus": false, "echec": false, "en_cours": true, "resume": "" },
            { "genre": "avis", "texte": "contexte coupé" }
        ]);
        let entrees = step_entries(&journal);
        assert_eq!(entrees.len(), 6, "{entrees:?}");
        assert_eq!(entrees[0], StepEntry::Said("je regarde".into()));
        assert_eq!(
            entrees[1],
            StepEntry::Tool {
                name: "lire".into(),
                state: "fait".into(),
                resume: "3 lignes".into(),
                ms: 42
            }
        );
        // Les trois états qui comptent se lisent sans le texte : refusé, échoué, en cours.
        for (entree, attendu) in [
            (&entrees[2], "refusé"),
            (&entrees[3], "échec"),
            (&entrees[4], "en cours"),
        ] {
            let StepEntry::Tool { state, .. } = entree else {
                panic!("une étape d'outil attendue, pas {entree:?}");
            };
            assert_eq!(state, attendu);
        }
        assert_eq!(entrees[5], StepEntry::Noted("contexte coupé".into()));
    }

    #[test]
    fn un_journal_range_en_texte_json_se_lit_comme_un_journal() {
        let brut = json!(r#"[{"outil":"lire","etat":"fait","resume":"ok","ms":3}]"#);
        assert_eq!(
            step_entries(&brut),
            vec![StepEntry::Tool {
                name: "lire".into(),
                state: "fait".into(),
                resume: "ok".into(),
                ms: 3
            }]
        );
    }

    /// Une bulle qui n'a pas de journal n'en dessine pas, et un champ qui n'est pas du JSON
    /// ne devient pas une étape inventée : le rendu d'un journal n'est pas un endroit où
    /// deviner.
    #[test]
    fn un_journal_absent_ou_illisible_ne_dessine_rien() {
        for vide in [
            json!(""),
            json!("pas du json"),
            json!("{}"),
            json!([]),
            json!(null),
        ] {
            assert!(
                step_entries(&vide).is_empty(),
                "{vide} a produit des étapes"
            );
        }
        // Un objet sans nom d'outil n'est pas une étape.
        assert!(step_entries(&json!([{"etat": "fait"}])).is_empty());
    }

    /// L'ordre du journal est l'ordre affiché : les phrases (`dit`) et les appels d'outil
    /// s'intercalent dans leur ordre chronologique, pas les outils groupés à la fin.
    #[test]
    fn un_journal_garde_l_ordre_des_phrases_et_des_outils() {
        let journal = json!([
            { "genre": "dit", "texte": "d'abord je regarde" },
            { "id": "t1", "genre": "outil", "outil": "lire", "ms": 42,
              "refus": false, "echec": false, "en_cours": false, "resume": "3 lignes" },
            { "genre": "dit", "texte": "puis j'écris" },
            { "id": "t2", "genre": "outil", "outil": "ecrire", "ms": 7,
              "refus": false, "echec": false, "en_cours": false, "resume": "écrit" }
        ]);
        let noms: Vec<String> = step_entries(&journal)
            .iter()
            .map(|e| match e {
                StepEntry::Said(texte) => format!("dit:{texte}"),
                StepEntry::Tool { name, .. } => format!("outil:{name}"),
                StepEntry::Noted(texte) => format!("avis:{texte}"),
            })
            .collect();
        assert_eq!(
            noms,
            vec![
                "dit:d'abord je regarde",
                "outil:lire",
                "dit:puis j'écris",
                "outil:ecrire"
            ]
        );
    }

    /// Quand le journal porte les phrases, la prose n'est pas rendue une seconde fois :
    /// le champ `contenu` en est la copie fondue, et le doublerait en repoussant les
    /// outils après tout le texte.
    #[test]
    fn la_prose_n_est_pas_doublee_quand_le_journal_porte_les_phrases() {
        let dit_outil = vec![
            StepEntry::Said("je regarde".into()),
            StepEntry::Tool {
                name: "lire".into(),
                state: "fait".into(),
                resume: "3 lignes".into(),
                ms: 42,
            },
            StepEntry::Said("voici".into()),
        ];
        assert_eq!(
            prose_de_bulle("je regarde\n\nvoici", &dit_outil),
            None,
            "le texte est déjà dans le journal"
        );

        // Sans phrase dans le journal, la prose est le contenu de la ligne.
        let outils_seuls = vec![StepEntry::Tool {
            name: "lire".into(),
            state: "fait".into(),
            resume: String::new(),
            ms: 0,
        }];
        assert_eq!(
            prose_de_bulle("un texte", &outils_seuls),
            Some("un texte".to_string())
        );
        // Un texte blanc ne se rend jamais.
        assert_eq!(prose_de_bulle("   ", &[]), None);
    }

    // ---- le regroupement des outils d'une bulle ------------------------------------------

    /// Trois outils qui se suivent forment UN groupe d'une ligne, qui les nomme tous.
    #[test]
    fn trois_outils_consecutifs_forment_un_groupe_qui_les_nomme() {
        let outils = vec![
            StepEntry::Tool {
                name: "architecture".into(),
                state: "fait".into(),
                resume: "…".into(),
                ms: 67,
            },
            StepEntry::Tool {
                name: "doc_forge".into(),
                state: "fait".into(),
                resume: "…".into(),
                ms: 1,
            },
            StepEntry::Tool {
                name: "read_module".into(),
                state: "fait".into(),
                resume: "…".into(),
                ms: 90,
            },
        ];
        let segments = segmente(&outils);
        assert_eq!(segments.len(), 1, "{segments:?}");
        let Segment::Tools(groupe) = &segments[0] else {
            panic!("un groupe attendu, pas {segments:?}");
        };
        assert_eq!(groupe.len(), 3);
        let label = tool_group_label(groupe);
        assert_eq!(
            label, "3 outils · architecture, doc_forge, read_module · 158 ms",
            "la ligne nomme les trois outils et la durée totale"
        );
    }

    /// Un texte de l'agent entre deux outils coupe le groupe : quatre outils, deux groupes,
    /// et le texte garde sa place chronologique.
    #[test]
    fn un_texte_entre_deux_outils_coupe_le_groupe() {
        let entries = vec![
            StepEntry::Tool {
                name: "a".into(),
                state: "fait".into(),
                resume: String::new(),
                ms: 1,
            },
            StepEntry::Tool {
                name: "b".into(),
                state: "fait".into(),
                resume: String::new(),
                ms: 1,
            },
            StepEntry::Said("je regarde".into()),
            StepEntry::Tool {
                name: "c".into(),
                state: "fait".into(),
                resume: String::new(),
                ms: 1,
            },
            StepEntry::Tool {
                name: "d".into(),
                state: "fait".into(),
                resume: String::new(),
                ms: 1,
            },
        ];
        let segments = segmente(&entries);
        assert_eq!(
            segments,
            vec![
                Segment::Tools(vec![
                    StepEntry::Tool {
                        name: "a".into(),
                        state: "fait".into(),
                        resume: String::new(),
                        ms: 1,
                    },
                    StepEntry::Tool {
                        name: "b".into(),
                        state: "fait".into(),
                        resume: String::new(),
                        ms: 1,
                    },
                ]),
                Segment::Said("je regarde".into()),
                Segment::Tools(vec![
                    StepEntry::Tool {
                        name: "c".into(),
                        state: "fait".into(),
                        resume: String::new(),
                        ms: 1,
                    },
                    StepEntry::Tool {
                        name: "d".into(),
                        state: "fait".into(),
                        resume: String::new(),
                        ms: 1,
                    },
                ]),
            ],
            "le texte coupe le groupe, les outils qui se suivent se regroupent"
        );
    }

    /// Le résultat brut d'un outil ne fait pas partie de la ligne de groupe : replié, il ne
    /// s'ouvre que sur demande.
    #[test]
    fn le_resultat_brut_ne_est_pas_deverse_dans_la_ligne_de_groupe() {
        let outils = vec![StepEntry::Tool {
            name: "read_module".into(),
            state: "fait".into(),
            resume: "{\"compile\":{},\"enabled\":true}".into(),
            ms: 90,
        }];
        let label = tool_group_label(&outils);
        assert!(
            !label.contains("compile"),
            "le brut n'est pas dans la ligne : {label}"
        );
        assert!(label.contains("read_module"), "{label}");
    }

    // ---- la fenêtre du fil ---------------------------------------------------------------

    /// La règle qui compte : un fil se lit par le BAS, et élargir la fenêtre ne perd rien.
    #[test]
    fn une_fenetre_de_fil_montre_les_derniers_et_grandit_vers_le_haut() {
        // 5 messages, fenêtre de 2 : les 2 derniers, 3 cachés en haut.
        assert_eq!(thread_window(5, 2), (3, 2));
        // On élargit de 2 : les 4 derniers, dont les 2 déjà affichés. Rien n'est perdu.
        assert_eq!(thread_window(5, 4), (1, 4));
        // Plus large que le fil : tout, rien de caché.
        assert_eq!(thread_window(5, 40), (0, 5));
        assert_eq!(thread_hidden(5, 40), 0);
        // Une fenêtre nulle ou un fil vide ne cachent rien.
        assert_eq!(thread_hidden(0, 40), 0);
        assert_eq!(thread_window(5, 0), (5, 0));
    }

    /// Mesure de la propriété qui compte, sur toutes les tailles : la fenêtre élargie
    /// CONTIENT la fenêtre étroite, et la fin du fil est toujours visible.
    #[test]
    fn elargir_la_fenetre_garde_ce_qui_etait_deja_affiche() {
        for total in 0..12usize {
            for pas in 1..=4u32 {
                let (mut premier, _) = thread_window(total, pas as usize);
                let mut vu = Vec::new();
                for fenetre in 1..=6u32 {
                    let (debut, _) = thread_window(total, (pas * fenetre) as usize);
                    assert!(
                        debut <= premier,
                        "élargir a caché un message (total {total}, pas {pas})"
                    );
                    premier = debut;
                    vu.push(debut);
                }
                let fin = thread_window(total, u32::MAX as usize);
                assert_eq!(fin, (0, total), "une fenêtre immense montre tout");
            }
        }
    }

    // ---- ce qu'un avis annonce ------------------------------------------------------------

    #[test]
    fn un_avis_ne_se_dessine_que_s_il_a_quelque_chose_a_dire() {
        assert_eq!(annonce(None), None);
        assert_eq!(annonce(Some(&json!(null))), None);
        assert_eq!(annonce(Some(&json!(""))), None);
        assert_eq!(annonce(Some(&json!("   "))), None);
        assert_eq!(
            annonce(Some(&json!("Le travail est plus long que prévu."))),
            Some("Le travail est plus long que prévu.".into())
        );
        // Un champ qui n'est pas du texte s'annonce quand même : 3 tentatives, c'est un fait.
        assert_eq!(annonce(Some(&json!(3))), Some("3".into()));
    }

    // ---- une carte de question ------------------------------------------------------------

    #[test]
    fn les_choix_d_une_question_se_lisent_en_liste_ou_en_texte() {
        assert_eq!(
            choice_list(&json!(["Budget", "Sport"])),
            vec!["Budget".to_string(), "Sport".to_string()]
        );
        assert_eq!(
            choice_list(&json!(r#"["Budget","Sport"]"#)),
            vec!["Budget".to_string(), "Sport".to_string()]
        );
        // Un choix unique écrit comme un texte ne disparaît pas : il vaut UN choix.
        assert_eq!(choice_list(&json!("Budget")), vec!["Budget".to_string()]);
        assert!(choice_list(&json!("")).is_empty());
        assert!(choice_list(&json!(null)).is_empty());
    }

    /// La carte de question d'un fil : son texte, ses choix, la réponse déjà donnée.
    #[test]
    fn une_carte_de_question_se_lit_dans_les_champs_que_le_module_nomme() {
        let module = ModuleDescriptor::from_json(
            &json!({
                "name": "atelier", "version": "1.0.0",
                "entities": [{
                    "name": "demande", "plural": "demandes",
                    "fields": [
                        {"name": "texte", "ty": "string"},
                        {"name": "choix", "ty": {"list": "string"}},
                        {"name": "reponse", "ty": "string", "optional": true}
                    ]
                }],
                "views": []
            })
            .to_string(),
        )
        .expect("le module se lit");
        let brique = Brick::Question {
            entity: "demande".into(),
            filter: None,
            text: "texte".into(),
            options: "choix".into(),
            answer: Some("reponse".into()),
        };
        let mut module = module;
        module.normalize();
        if let Some(entite) = brique.entity() {
            assert_eq!(entite, "demande");
        }

        let snapshot = Snapshot::new().with(
            "demande",
            Feed::Ready(vec![json!({
                "id": "question-1", "texte": "Sur quoi ?", "choix": ["Budget", "Sport"],
                "reponse": ""
            })]),
        );
        let lecture = crate::views::state::brick_reading(&brique, &snapshot)
            .expect("une carte de question lit une entité");
        let rows = lecture.rows().expect("une question posée");
        assert_eq!(rows.rows.len(), 1);
        assert_eq!(
            data::display(data::value_of(rows.rows[0], "texte")),
            "Sur quoi ?"
        );
        assert!(annonce(data::value_of(rows.rows[0], "reponse")).is_none());
        assert_eq!(
            choice_list(data::value_of(rows.rows[0], "choix").expect("les choix")),
            vec!["Budget".to_string(), "Sport".to_string()]
        );
        assert_eq!(
            data::value_of(rows.rows[0], "id").and_then(Value::as_str),
            Some("question-1"),
            "l'identifiant du guichet est ce que la réponse doit nommer"
        );
    }

    /// Le contrat refuse une carte de question dont les champs n'existent pas : une vue
    /// fausse se dit à la validation, pas à l'écran.
    #[test]
    fn une_carte_de_question_sur_un_champ_inconnu_est_refusee() {
        let module = ModuleDescriptor::from_json(
            &json!({
                "name": "atelier", "version": "1.0.0",
                "entities": [{"name": "demande", "plural": "demandes",
                    "fields": [{"name": "texte", "ty": "string"}]}],
                "views": [{ "name": "fil", "surface": "chat", "root": {"type": "leaf",
                    "brick": {"type": "question", "entity": "demande", "text": "texte",
                              "options": "choix"}}}]
            })
            .to_string(),
        )
        .expect("le module se lit");
        let refus = module
            .check(
                &this_descriptor::CustomRegistry::default(),
                this_descriptor::CheckProfile::Compiled,
            )
            .expect_err("un champ lu qui n'existe pas est refusé");
        let codes: Vec<&str> = refus.iter().map(|i| i.code.0.as_str()).collect();
        assert!(
            codes
                .iter()
                .any(|code| code.ends_with("view_field_unknown")),
            "{codes:?}"
        );
    }

    // ---- le fil par ligne ---------------------------------------------------------------

    /// Les champs d'une entité `message` : `contenu` porte le texte, `role` porte l'auteur.
    fn champs_message() -> ChampsBulle {
        ChampsBulle {
            content: "contenu".into(),
            author: None,
            role: Some("role".into()),
            state: None,
            steps: None,
        }
    }

    /// Une bulle ne lit QUE sa ligne : les champs extraits viennent de la ligne dont on
    /// donne l'`id`, jamais d'une autre, et une ligne absente ne dessine rien.
    #[test]
    fn une_bulle_ne_lit_que_sa_ligne() {
        let snapshot = Snapshot::new().with(
            "message",
            Feed::Ready(vec![
                json!({"id": "m1", "contenu": "premier", "role": "utilisateur"}),
                json!({"id": "m2", "contenu": "deuxième", "role": "module"}),
            ]),
        );
        let champs = champs_message();
        let bulle = ligne_en_bulle(&snapshot, "message", "m2", &champs, false).expect("m2 existe");
        assert_eq!(bulle.text, "deuxième");
        assert_eq!(bulle.role, "module");
        // Une ligne absente : rien, pas la première ligne du feed.
        assert!(ligne_en_bulle(&snapshot, "message", "absent", &champs, false).is_none());
    }

    /// Un fil se lit ligne par ligne : une ligne qui change ne redessine qu'ELLE. C'est la
    /// propriété que le `Memo` par `id` de `bubble_ligne` garantit, mesurée sur le vrai
    /// moteur réactif (sans DOM) : trois lignes, on change la deuxième, seul son rendu
    /// s'exécute.
    #[test]
    fn une_ligne_qui_change_ne_redessine_que_sa_bulle() {
        let owner = Owner::new();
        owner.set();

        let champs = champs_message();
        let lignes = vec![
            json!({"id": "m1", "contenu": "premier", "role": "utilisateur"}),
            json!({"id": "m2", "contenu": "deuxième", "role": "module"}),
            json!({"id": "m3", "contenu": "troisième", "role": "module"}),
        ];
        let data = RwSignal::new(Snapshot::new().with("message", Feed::Ready(lignes.clone())));

        // Ce que `bubble_ligne` fait : un `Memo` par ligne (la lecture de la bulle), et une
        // fermeture de rendu qui compte chacune de ses exécutions — c'est elle qui reflète
        // le travail du DOM (redessiner la bulle).
        let rendus: Vec<(String, Arc<AtomicUsize>, Memo<Option<RowView>>)> = ["m1", "m2", "m3"]
            .iter()
            .map(|id| {
                let id = id.to_string();
                let ligne = Memo::new({
                    let id = id.clone();
                    let champs = champs.clone();
                    move |_| {
                        let snapshot = data.get();
                        ligne_en_bulle(&snapshot, "message", &id, &champs, false)
                    }
                });
                let compteur = Arc::new(AtomicUsize::new(0));
                let rendu = Memo::new({
                    let compteur = Arc::clone(&compteur);
                    move |_| {
                        compteur.fetch_add(1, Ordering::SeqCst);
                        ligne.get()
                    }
                });
                (id, compteur, rendu)
            })
            .collect();

        // Premier rendu : chaque bulle se dessine exactement une fois. `get_untracked` : le
        // harnais de test lit, il n'est pas lui-même un abonné réactif.
        for (_, compteur, rendu) in &rendus {
            let _ = rendu.get_untracked();
            assert_eq!(
                compteur.load(Ordering::SeqCst),
                1,
                "le premier rendu dessine chaque bulle une fois"
            );
        }

        // On modifie UN message : m2 change, m1 et m3 restent identiques.
        let mut lignes2 = lignes.clone();
        lignes2[1] = json!({"id": "m2", "contenu": "deuxième (modifié)", "role": "module"});
        data.set(Snapshot::new().with("message", Feed::Ready(lignes2)));

        // Le fil relit chaque bulle : seul m2 doit avoir changé, donc seul son rendu se
        // réexécute.
        for (id, compteur, rendu) in &rendus {
            let _ = rendu.get_untracked();
            let n = compteur.load(Ordering::SeqCst);
            if id == "m2" {
                assert_eq!(n, 2, "m2 a changé, son rendu se réexécute");
            } else {
                assert_eq!(n, 1, "{id} n'a pas changé, son rendu ne se réexécute pas");
            }
        }

        // La ligne lue est bien la nouvelle (lecture sans suivi : c'est un constat, pas un
        // rendu).
        assert_eq!(
            data.get_untracked()
                .ligne("message", "m2")
                .and_then(|row| row.get("contenu")),
            Some(&json!("deuxième (modifié)"))
        );
    }
}
