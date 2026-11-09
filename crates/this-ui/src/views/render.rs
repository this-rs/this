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
    Node, ProgramArg, RelationMode, Run, Signal as IndicatorSignal, Surface, Tab, Tone, ViewDesc,
};
use this_descriptor::{FilterExpr, ModuleDescriptor, SortSpec};

use crate::design::{self, Tone as DesignTone};
use crate::model::Kind as FieldKind;
use crate::views::data::{self, Feed, Reading, Snapshot};
use crate::views::intent::{FieldValue, Intent, LinkOp, Values};
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
    let classes = view_classes(view);
    let label = title.clone();
    let root = view.root.clone();
    let gate_root = view.root.clone();
    let state = Memo::new(move |_| state::view_state(&gate_root, &ctx.data.get()));
    let tree = render_node(ctx, &root);
    view! {
        <section class=classes aria-label=label>
            <h2 class=class::VIEW_TITLE>{title}</h2>
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
    }
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
    view! {
        <section class=class::MODAL role="dialog" aria-modal="false" aria-label=label>
            <div class=class::MODAL_HEAD>
                <h3>{heading}</h3>
                <button class=class::ICON_BUTTON aria-label=text::CLOSE title=text::CLOSE
                    on:click=move |_| {
                        let name = ctx.view.get_value();
                        ctx.on_intent.run(Intent::Dismiss { view: name });
                    }>
                    <span aria-hidden="true">"×"</span>
                </button>
            </div>
            <div class=class::MODAL_BODY>{items}</div>
        </section>
    }
    .into_any()
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
                } if target == entity => Some((name.clone(), target.clone(), fields.clone())),
                _ => None,
            })
        })
        .flatten();
    let button = create.map(|(name, target, fields)| {
        let label = placement::humanise(&name);
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
fn tile_for(ctx: Ctx, entity: &str, reading: &Reading<'_>, when_empty: &str) -> AnyView {
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
        Brick::Button {
            entity,
            filter,
            action,
        } => button(ctx, entity, filter.as_ref(), action),
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
        Brick::Message { entity, filter } => message(ctx, entity, filter.as_ref()),
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

fn button(ctx: Ctx, entity: &str, filter: Option<&FilterExpr>, action: &str) -> AnyView {
    let label = placement::humanise(action);
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
fn intent_of(
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
        }) => Intent::Update {
            action: name,
            entity,
            fields,
            id,
            values,
        },
        Some(Action::Delete { name, entity }) => Intent::Delete {
            action: name,
            entity,
            id,
        },
        Some(Action::Navigate { name, view }) => Intent::Navigate {
            view,
            action: Some(name),
        },
        Some(Action::AgentTool {
            name,
            op,
            entity,
            text,
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
        }) => Intent::Capability {
            action: name,
            capability,
            grant,
        },
        Some(Action::Invoke {
            name,
            operation,
            entity,
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
        Node::Leaf { .. } => return Some(node.clone()),
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
    };
    let empty = match &kept {
        Node::Stack { children }
        | Node::Row { children }
        | Node::Drawer { children, .. }
        | Node::Modal { children, .. } => children.is_empty(),
        Node::Tabs { tabs } => tabs.iter().all(|tab| tab.children.is_empty()),
        Node::Leaf { .. } => false,
    };
    (!empty).then_some(kept)
}

/// The intent a composer action emits: the same as a `button`'s, with no row (a composer
/// reads none — the contract refuses the actions that would need one).
pub fn composer_intent(module: &ModuleDescriptor, action: &str) -> Intent {
    let declared = module.actions.iter().find(|a| a.name() == action).cloned();
    intent_of(declared, action, None, None, Vec::new())
}

/// The module's actions, as buttons for the host to place around its field. Each click
/// emits the intent the action means — the host runs it by the same path as any view.
pub fn composer_actions(ctx: Ctx, actions: &[String]) -> AnyView {
    let buttons = actions
        .iter()
        .map(|action| {
            let name = action.clone();
            let label = placement::humanise(action);
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
                        let phase = match field.as_deref() {
                            Some(field) => {
                                data::display(first.and_then(|row| data::value_of(row, field)))
                            }
                            None => text::DASH.to_owned(),
                        };
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
) -> AnyView {
    let entity = entity.to_owned();
    let filter = filter.cloned();
    let label = label.to_owned();
    let subtitle = subtitle.map(str::to_owned);
    let empty = empty.unwrap_or(text::NOTHING).to_owned();
    let sort: Vec<SortSpec> = sort.to_vec();
    let filterable: Vec<String> = filterable.to_vec();
    let on_click = on_click.cloned();
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
                            match &on_click {
                                None => view! {
                                    <li class=class::LIST_ROW>
                                        <span class=class::LIST_LABEL>{title}</span>
                                        {sub}
                                    </li>
                                }
                                .into_any(),
                                Some(click) => {
                                    let id = data::display(data::value_of(row, "id"));
                                    let intent = click_intent(click, &entity, &id);
                                    view! {
                                        <li>
                                            <button class=class::LIST_ROW
                                                on:click=move |_| ctx.on_intent.run(intent.clone())>
                                                <span class=class::LIST_LABEL>{title}</span>
                                                {sub}
                                            </button>
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
        .map(placement::humanise)
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

/// Une bulle par ligne lue : son auteur et son contenu. Filtrée sur une ligne, c'est UN
/// message ; filtrée sur la conversation ouverte (`{"$context": "conversation"}`), c'est le
/// fil, dans l'ordre des lignes. C'est la brique qui permet à un module de redessiner chaque
/// message — le contenu reste celui de la ligne, la forme est celle du contrat.
fn message(ctx: Ctx, entity: &str, filter: Option<&FilterExpr>) -> AnyView {
    let entity = entity.to_owned();
    let filter = filter.cloned();
    let content = display_field(
        ctx.module,
        &entity,
        &["content", "text", "body", "message", "contenu"],
    );
    let author = display_field(ctx.module, &entity, &["role", "author", "sender", "from"]);
    let log = move || {
        let snapshot = ctx.data.get();
        match data::read(&snapshot, &entity, filter.as_ref(), &[], 0, None) {
            Reading::Rows(rows) => rows
                .rows
                .into_iter()
                .map(|row| {
                    let text = data::display(data::value_of(row, &content));
                    let who = data::value_of(row, &author).map(|value| data::display(Some(value)));
                    view! {
                        <div class=class::MESSAGE>
                            {who.map(|who| {
                                view! { <span class=class::MESSAGE_META>{who}</span> }.into_any()
                            })}
                            <span>{text}</span>
                        </div>
                    }
                    .into_any()
                })
                .collect_view()
                .into_any(),
            other => tile_for(ctx, &entity, &other, text::NOTHING),
        }
    };
    view! { <div class=class::CHAT>{log}</div> }.into_any()
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

    fn view_desc(look: Option<this_descriptor::views::Look>) -> ViewDesc {
        ViewDesc {
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
                action: None
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
}

#[cfg(test)]
mod tests_action_row {
    use super::*;

    /// Le bouton qui atteint le code d'un module doit dire LAQUELLE des lignes.
    #[test]
    fn une_operation_qui_vise_une_entite_a_besoin_de_la_ligne() {
        let vise_une_entite = Action::Invoke {
            name: "terminer".into(),
            operation: "terminer".into(),
            entity: Some("tache".into()),
        };
        assert!(action_needs_row(Some(&vise_une_entite)));

        let operation_de_module = Action::Invoke {
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
            name: "ranger".into(),
            entity: "tache".into(),
            fields: vec!["fait".into()],
        };
        let delete = Action::Delete {
            name: "jeter".into(),
            entity: "tache".into(),
        };
        let create = Action::Create {
            name: "ajouter".into(),
            entity: "tache".into(),
            fields: vec!["titre".into()],
        };
        assert!(action_needs_row(Some(&update)));
        assert!(action_needs_row(Some(&delete)));
        assert!(!action_needs_row(Some(&create)));
        assert!(!action_needs_row(None));
    }
}
