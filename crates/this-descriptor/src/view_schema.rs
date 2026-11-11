//! # The view contract, described for an agent
//!
//! [`view_schema`] returns a serializable description of everything a module can declare in
//! `views` and `actions`: the containers ([`Node`](crate::views::Node)), the bricks
//! ([`Brick`](crate::views::Brick)), the actions ([`Action`](crate::views::Action)), the closed
//! enumerations, the default of every optional field, a minimal and a complete example of each
//! variant, the validation rules (with how to fix each), and a coverage table that answers
//! "which type draws a list / a form / a drawer...".
//!
//! An agent reads this document, then writes a view without guessing. The description is
//! written by hand (no new dependency, no derive) and kept honest by tests: they ask `serde`
//! itself which variants and fields exist and compare, so a variant added to the contract
//! without a schema entry (or the reverse) fails the build.
//!
//! ```
//! let schema = this_descriptor::view_schema::view_schema();
//! let brick = schema.brick("list").unwrap();
//! assert!(brick.fields.iter().any(|f| f.name == "entity" && f.required));
//! println!("{}", serde_json::to_string_pretty(&brick.example).unwrap());
//! ```
//!
//! The parsers ([`parse_view`], [`parse_brick`], [`parse_node`], [`parse_action`]) turn a serde
//! failure into a [`ViewParseError`] that names the valid choices and suggests the closest one.

use crate::views::{Action, Brick, Node, ViewDesc};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// Version of the schema document itself. Bumped when its SHAPE changes, not its content.
pub const VIEW_SCHEMA_VERSION: u32 = 1;

/// The complete description of the view contract.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ViewSchema {
    /// [`VIEW_SCHEMA_VERSION`].
    pub version: u32,
    /// How to read `FieldSchema::ty`.
    pub type_grammar: Vec<String>,
    /// Where `views` and `actions` live in a module descriptor.
    pub module_keys: Vec<FieldSchema>,
    /// A `ViewDesc`: name, surface, title, replaces, look, root.
    pub view: ObjectSchema,
    /// Containers: the `type` of a `Node`.
    pub nodes: Vec<VariantSchema>,
    /// Bricks: the `type` of a `Brick` (what a `leaf` draws).
    pub bricks: Vec<VariantSchema>,
    /// Actions: the `kind` of an `Action`.
    pub actions: Vec<VariantSchema>,
    /// Row clicks: the `kind` of a `Click`.
    pub clicks: Vec<VariantSchema>,
    /// Computed-column sources: the `op` of a `ColumnSource`.
    pub column_sources: Vec<VariantSchema>,
    /// Filter expressions (`filter` on a brick).
    pub filter: Vec<VariantSchema>,
    /// Plain structures nested in the above (`Tab`, `Look`, `FormField`...).
    pub objects: Vec<ObjectSchema>,
    /// Closed enumerations.
    pub enums: Vec<EnumSchema>,
    /// The bricks a `header` surface accepts.
    pub header_bricks: Vec<String>,
    /// Maximum container nesting depth.
    pub max_depth: usize,
    /// Every validation rule `ModuleDescriptor::check` applies to views and actions.
    pub rules: Vec<RuleSchema>,
    /// Which type covers which interface need.
    pub coverage: Vec<CoverageEntry>,
    /// A small module fragment (`entities`, `actions`, `views`) that passes `check`.
    pub example_module: Value,
}

/// One field of a structure or variant.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FieldSchema {
    pub name: String,
    /// See `ViewSchema::type_grammar`.
    pub ty: String,
    /// `true`: absent = refused at deserialization.
    pub required: bool,
    /// The value taken when absent (`None` for a required field, or an optional one with no
    /// value, e.g. `Option`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<Value>,
    pub summary: String,
    /// What the value must designate for `check` to accept it: `entity`, `field`, `action`,
    /// `view`, `link`, or `action:device_capability`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refers_to: Option<String>,
}

/// A variant of a tagged enumeration (`Node`, `Brick`, `Action`...).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct VariantSchema {
    /// The value of the tag (`list`, `create`...).
    pub name: String,
    pub summary: String,
    pub fields: Vec<FieldSchema>,
    /// The smallest valid JSON: required fields only.
    pub example: Value,
    /// Every field set to a non-default value.
    pub full_example: Value,
    /// Bricks only: accepted on a `header` surface.
    pub header_ok: bool,
}

/// A plain structure (`ViewDesc`, `Tab`, `Look`...).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ObjectSchema {
    pub name: String,
    pub summary: String,
    pub fields: Vec<FieldSchema>,
    pub example: Value,
    pub full_example: Value,
}

/// A closed enumeration of plain strings.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EnumSchema {
    pub name: String,
    pub summary: String,
    pub values: Vec<EnumValue>,
    /// The value taken when the field is absent, if the field has a default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EnumValue {
    pub name: String,
    pub summary: String,
}

/// A validation rule. The reported code is `this.descriptor.<code>`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RuleSchema {
    pub code: String,
    pub summary: String,
    pub fix: String,
}

/// One row of the coverage audit: an interface need and the type that serves it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CoverageEntry {
    pub need: String,
    /// `brick:list`, `node:tabs`, `brick:field(input=toggle)`... Empty = not covered.
    pub covered_by: Vec<String>,
    pub note: String,
}

impl ViewSchema {
    pub fn brick(&self, name: &str) -> Option<&VariantSchema> {
        self.bricks.iter().find(|v| v.name == name)
    }
    pub fn node(&self, name: &str) -> Option<&VariantSchema> {
        self.nodes.iter().find(|v| v.name == name)
    }
    pub fn action(&self, name: &str) -> Option<&VariantSchema> {
        self.actions.iter().find(|v| v.name == name)
    }
    pub fn rule(&self, code: &str) -> Option<&RuleSchema> {
        self.rules.iter().find(|r| r.code == code)
    }

    /// The schema as pretty JSON, ready to hand to an agent.
    pub fn to_json_pretty(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_default()
    }
}

// ---------------------------------------------------------------------------------------
// builders
// ---------------------------------------------------------------------------------------

fn req(name: &str, ty: &str, summary: &str) -> FieldSchema {
    FieldSchema {
        name: name.into(),
        ty: ty.into(),
        required: true,
        default: None,
        summary: summary.into(),
        refers_to: None,
    }
}

fn opt(name: &str, ty: &str, summary: &str) -> FieldSchema {
    FieldSchema {
        required: false,
        ..req(name, ty, summary)
    }
}

impl FieldSchema {
    fn refers(mut self, to: &str) -> Self {
        self.refers_to = Some(to.into());
        self
    }
    fn default_value(mut self, v: Value) -> Self {
        self.default = Some(v);
        self
    }
}

fn entity_f() -> FieldSchema {
    req("entity", "string", "The entity the brick reads.").refers("entity")
}

fn filter_f() -> FieldSchema {
    opt("filter", "filter", "Which rows to read. Absent = all rows.")
}

fn variant(
    name: &str,
    summary: &str,
    fields: Vec<FieldSchema>,
    example: Value,
    full_example: Value,
) -> VariantSchema {
    VariantSchema {
        name: name.into(),
        summary: summary.into(),
        fields,
        example,
        full_example,
        header_ok: true,
    }
}

fn brick_v(
    name: &str,
    summary: &str,
    fields: Vec<FieldSchema>,
    example: Value,
    full_example: Value,
    header_ok: bool,
) -> VariantSchema {
    VariantSchema {
        header_ok,
        ..variant(name, summary, fields, example, full_example)
    }
}

fn object(
    name: &str,
    summary: &str,
    fields: Vec<FieldSchema>,
    example: Value,
    full_example: Value,
) -> ObjectSchema {
    ObjectSchema {
        name: name.into(),
        summary: summary.into(),
        fields,
        example,
        full_example,
    }
}

fn enum_s(name: &str, summary: &str, default: Option<&str>, values: &[(&str, &str)]) -> EnumSchema {
    EnumSchema {
        name: name.into(),
        summary: summary.into(),
        values: values
            .iter()
            .map(|(n, s)| EnumValue {
                name: (*n).into(),
                summary: (*s).into(),
            })
            .collect(),
        default: default.map(Into::into),
    }
}

fn rule(code: &str, summary: &str, fix: &str) -> RuleSchema {
    RuleSchema {
        code: code.into(),
        summary: summary.into(),
        fix: fix.into(),
    }
}

fn cover(need: &str, by: &[&str], note: &str) -> CoverageEntry {
    CoverageEntry {
        need: need.into(),
        covered_by: by.iter().map(|s| (*s).into()).collect(),
        note: note.into(),
    }
}

/// A sample filter, used by the complete examples.
fn sample_filter() -> Value {
    json!({"cmp": {"field": "titre", "op": "starts_with", "value": "A"}})
}

// ---------------------------------------------------------------------------------------
// the schema
// ---------------------------------------------------------------------------------------

/// Builds the description of the view contract. Cheap, pure, deterministic.
pub fn view_schema() -> ViewSchema {
    ViewSchema {
        version: VIEW_SCHEMA_VERSION,
        type_grammar: vec![
            "string, bool, u32: JSON scalars".into(),
            "filter: a filter expression, see `filter`".into(),
            "context: `{\"$context\": \"<key>\"}` as a `cmp` value: a value the host provides \
             when the view is drawn (the open conversation, the selected row…), not one \
             written in the module. The host documents its keys; an unknown key is `null` \
             and matches no row."
                .into(),
            "node: a container or leaf, see `nodes`".into(),
            "brick: a brick, see `bricks`".into(),
            "enum:<Name>: one of the strings of `enums[Name]`".into(),
            "object:<Name>: a structure of `objects[Name]`".into(),
            "click: an object tagged by `kind`, see `clicks`".into(),
            "list<T>: a JSON array of T".into(),
        ],
        module_keys: vec![
            opt(
                "views",
                "list<object:ViewDesc>",
                "The views the module draws. Names are unique.",
            ),
            opt(
                "actions",
                "list<action>",
                "The actions bricks may call. Names are unique; see `actions`.",
            ),
        ],
        view: view_object(),
        nodes: nodes(),
        bricks: bricks(),
        actions: actions(),
        clicks: clicks(),
        column_sources: column_sources(),
        filter: filter_variants(),
        objects: objects(),
        enums: enums(),
        header_bricks: [
            "text",
            "caption",
            "banner",
            "indicator",
            "metrics",
            "button",
        ]
        .iter()
        .map(|s| (*s).to_string())
        .collect(),
        max_depth: crate::views::MAX_VIEW_DEPTH,
        rules: rules(),
        coverage: coverage(),
        example_module: example_module(),
    }
}

fn view_object() -> ObjectSchema {
    object(
        "ViewDesc",
        "A view: a name, a surface, a tree. The host draws it; the module never draws.",
        vec![
            req("name", "string", "Identifier, `^[a-z][a-z0-9_]*$`, unique in the module."),
            opt("surface", "enum:Surface", "Where the view is drawn.")
                .default_value(json!("main")),
            opt("title", "string", "Section title, when the host shows one."),
            opt(
                "replaces",
                "string",
                "Name of another view of this module that this one replaces (the host shows only the replacement).",
            )
            .refers("view"),
            opt("look", "object:Look", "Declared appearance: closed choices, no CSS."),
            req("root", "node", "The tree."),
        ],
        json!({"name": "invoices", "root": {"type": "stack", "children": []}}),
        json!({
            "name": "invoices", "surface": "panel", "title": "Invoices", "replaces": "old_invoices",
            "look": {"accent": "primary", "layout": "grid", "transition": "fade"},
            "root": {"type": "stack", "children": []}
        }),
    )
}

fn nodes() -> Vec<VariantSchema> {
    let leaf = json!({"type": "leaf", "brick": {"type": "metrics", "entity": "invoice"}});
    vec![
        variant(
            "stack",
            "Children stacked vertically.",
            vec![req(
                "children",
                "list<node>",
                "The children, top to bottom.",
            )],
            json!({"type": "stack", "children": []}),
            json!({"type": "stack", "children": [leaf.clone()]}),
        ),
        variant(
            "row",
            "Children side by side.",
            vec![req(
                "children",
                "list<node>",
                "The children, left to right.",
            )],
            json!({"type": "row", "children": []}),
            json!({"type": "row", "children": [leaf.clone()]}),
        ),
        variant(
            "tabs",
            "One tab per entry, each with its own subtree. At least one tab; labels unique.",
            vec![req("tabs", "list<object:Tab>", "The tabs, in order.")],
            json!({"type": "tabs", "tabs": [{"label": "All", "children": []}]}),
            json!({"type": "tabs", "tabs": [
                {"label": "All", "children": [leaf.clone()]},
                {"label": "Paid", "children": []}
            ]}),
        ),
        variant(
            "drawer",
            "A fold-away section under a label.",
            vec![
                req("label", "string", "The label that opens it."),
                req("children", "list<node>", "What it holds."),
            ],
            json!({"type": "drawer", "label": "Details", "children": []}),
            json!({"type": "drawer", "label": "Details", "children": [leaf.clone()]}),
        ),
        variant(
            "modal",
            "A sheet opened over the content, with a title.",
            vec![
                req("title", "string", "The sheet title."),
                req("children", "list<node>", "What it holds."),
            ],
            json!({"type": "modal", "title": "Confirm", "children": []}),
            json!({"type": "modal", "title": "Confirm", "children": [leaf.clone()]}),
        ),
        variant(
            "when",
            "\"Show `child` only if\": a visibility condition on ONE node — a brick (a leaf) or a \
             whole group. The host evaluates it against what it already knows, BEFORE drawing: \
             `child` is drawn if and only if at least one row of `entity` satisfies `filter`. \
             False => not drawn. Data absent (entity not read yet, or no row at all) => not \
             drawn: a condition that cannot be evaluated never shows a button at random. A \
             failed or refused read => not drawn, and the refusal is named in the host's log. \
             A hidden node does not count in the view's verdict, so a tree whose every \
             conditional node is hidden draws empty, without error.",
            vec![
                req("entity", "string", "The entity the condition reads.").refers("entity"),
                req(
                    "filter",
                    "filter",
                    "The condition, as a list filter: known fields only, `$context` allowed.",
                ),
                req("child", "node", "The node shown when the condition holds."),
            ],
            json!({"type": "when", "entity": "invoice",
                   "filter": {"cmp": {"field": "titre", "op": "eq", "value": "A"}},
                   "child": {"type": "stack", "children": []}}),
            json!({"type": "when", "entity": "invoice",
                   "filter": {"cmp": {"field": "titre", "op": "ne", "value": null}},
                   "child": {"type": "leaf", "brick": {"type": "metrics", "entity": "invoice"}}}),
        ),
        variant(
            "html",
            "A web page the MODULE ships: one file of its VFS folder, mounted by the host in a \
             sandboxed iframe (never `allow-same-origin`). The unlimited door of the interface: \
             when you write HTML/CSS/JS you leave the contract — no imposed theme, no verified \
             contrast, no 44 px touch target; the host cannot measure the inside. Bridge: the \
             host posts `{type:\"this:data\", rows, theme, locale}` (only the entities the module \
             declares), the page posts `{type:\"this:intent\", …}`, validated like any intent \
             (create/update/delete/invoke/navigate); an unknown intent is logged and ignored. \
             Refused: empty path, path outside the module (absolute elsewhere, `..`), extension \
             other than .html/.htm (an .svg is refused), and placement in a `header`.",
            vec![
                req(
                    "path",
                    "string",
                    "File in the module folder: `ui/index.html` or `/modules/<name>/<version>/ui/index.html`.",
                ),
                opt(
                    "height",
                    "integer",
                    "Height in px. Absent = intrinsic height; the host caps it at 2000.",
                ),
            ],
            json!({"type": "html", "path": "ui/index.html"}),
            json!({"type": "html", "path": "ui/index.html", "height": 320}),
        ),
        variant(
            "leaf",
            "A leaf: draws one brick.",
            vec![req("brick", "brick", "The brick drawn.")],
            json!({"type": "leaf", "brick": {"type": "metrics", "entity": "invoice"}}),
            leaf,
        ),
    ]
}

fn bricks() -> Vec<VariantSchema> {
    let f = sample_filter();
    vec![
        brick_v(
            "text",
            "One value: `field` of the first row of the filter.",
            vec![
                entity_f(),
                filter_f(),
                req("field", "string", "The field shown.").refers("field"),
            ],
            json!({"type": "text", "entity": "invoice", "field": "titre"}),
            json!({"type": "text", "entity": "invoice", "filter": f, "field": "titre"}),
            true,
        ),
        brick_v(
            "caption",
            "A secondary line: `field` of the first row, small, muted, no frame, no background, one line (ellipsis). No action.",
            vec![
                entity_f(),
                filter_f(),
                req("field", "string", "The field shown.").refers("field"),
            ],
            json!({"type": "caption", "entity": "invoice", "field": "titre"}),
            json!({"type": "caption", "entity": "invoice", "filter": f, "field": "titre"}),
            true,
        ),
        brick_v(
            "button",
            "A button that triggers an action of the module.",
            vec![
                entity_f(),
                filter_f(),
                req("action", "string", "The action called.").refers("action"),
                opt(
                    "label",
                    "string",
                    "The button's text, as interface text (\"Pin this key\"): no `_`, not empty. \
                     Wins over the action's own `label`, which wins over the action name \
                     humanised by the host.",
                ),
            ],
            json!({"type": "button", "entity": "invoice", "action": "new"}),
            json!({"type": "button", "entity": "invoice", "filter": f, "action": "new",
                   "label": "New invoice"}),
            true,
        ),
        brick_v(
            "banner",
            "An alert banner. Reads no row.",
            vec![
                req("tone", "enum:Tone", "The tone."),
                req("text", "string", "A message key or a plain text."),
            ],
            json!({"type": "banner", "tone": "info", "text": "alerts.offline"}),
            json!({"type": "banner", "tone": "warning", "text": "alerts.offline"}),
            true,
        ),
        brick_v(
            "indicator",
            "A signal: connection, activity (phase and duration), counter or badge. \
             `activity` and `badge` need `entity` and `field`; `counter` needs `entity`; \
             `connection` needs nothing.",
            vec![
                req("signal", "enum:Signal", "Which signal."),
                opt(
                    "entity",
                    "string",
                    "The entity read (all signals but `connection`).",
                )
                .refers("entity"),
                filter_f(),
                opt("field", "string", "Activity: the phase. Badge: the value.").refers("field"),
                opt("duration", "string", "Activity: the elapsed duration.").refers("field"),
            ],
            json!({"type": "indicator", "signal": "connection"}),
            json!({"type": "indicator", "signal": "activity", "entity": "invoice",
                   "filter": f, "field": "titre", "duration": "amount"}),
            true,
        ),
        brick_v(
            "list",
            "The rows of an entity: label, subtitle, sort, filter, pagination, click, and per-row actions.",
            vec![
                entity_f(),
                filter_f(),
                req("label", "string", "The field shown as the row label.").refers("field"),
                opt("subtitle", "string", "A second line.").refers("field"),
                opt(
                    "empty",
                    "string",
                    "Message (key or text) when there is no row.",
                ),
                opt("sort", "list<object:SortSpec>", "Default sort, in order."),
                opt(
                    "filterable",
                    "list<string>",
                    "Fields the user may filter on.",
                )
                .refers("field"),
                opt(
                    "page_size",
                    "u32",
                    "Rows per page, 1..=100. Absent = no pagination.",
                ),
                opt(
                    "on_click",
                    "click",
                    "What a row click does. Absent = nothing.",
                ),
                opt(
                    "row_actions",
                    "list<string>",
                    "Actions of this module, drawn as a button in each row and executed with the row's id; each once. An update, delete or entity invoke — not a create, navigate, request_secret, agent_tool, device_capability, host or module-level invoke. A click on one does not open the row.",
                )
                .refers("action"),
            ],
            json!({"type": "list", "entity": "invoice", "label": "titre"}),
            json!({"type": "list", "entity": "invoice", "filter": f, "label": "titre",
                   "subtitle": "amount", "empty": "invoices.none",
                   "sort": [{"field": "amount", "dir": "desc"}], "filterable": ["titre"],
                   "page_size": 25, "on_click": {"kind": "open_detail"},
                   "row_actions": ["edit"]}),
            false,
        ),
        brick_v(
            "card",
            "Several fields of one row (the `detail` genre).",
            vec![
                entity_f(),
                filter_f(),
                req("fields", "list<string>", "The fields shown, in order.").refers("field"),
            ],
            json!({"type": "card", "entity": "invoice", "fields": ["titre"]}),
            json!({"type": "card", "entity": "invoice", "filter": f, "fields": ["titre", "amount"]}),
            false,
        ),
        brick_v(
            "form",
            "A form: its fields in order, each with a label and a read-only flag.",
            vec![
                entity_f(),
                filter_f(),
                req(
                    "fields",
                    "list<object:FormField>",
                    "The fields, in order; no duplicate.",
                ),
                opt(
                    "submit",
                    "string",
                    "A `create` or `update` action of the SAME entity.",
                )
                .refers("action"),
            ],
            json!({"type": "form", "entity": "invoice", "fields": [{"field": "titre"}]}),
            json!({"type": "form", "entity": "invoice", "filter": f,
                   "fields": [{"field": "titre", "label": "Title"},
                              {"field": "amount", "read_only": true}],
                   "submit": "new"}),
            false,
        ),
        brick_v(
            "field",
            "One editable field. `input` picks the control; `target` names the entity of a reference.",
            vec![
                entity_f(),
                filter_f(),
                req("field", "string", "The field edited.").refers("field"),
                opt("input", "enum:Input", "The control.").default_value(json!("text")),
                opt(
                    "target",
                    "string",
                    "The entity a `reference` points to. Only with `reference`.",
                )
                .refers("entity"),
            ],
            json!({"type": "field", "entity": "invoice", "field": "titre"}),
            json!({"type": "field", "entity": "invoice", "filter": f, "field": "titre",
                   "input": "reference", "target": "conversation"}),
            false,
        ),
        brick_v(
            "relation",
            "A relation between entities, shown or editable, through a declared link.",
            vec![
                entity_f(),
                filter_f(),
                req(
                    "link",
                    "string",
                    "The `link_type` of a link whose source is `entity`.",
                )
                .refers("link"),
                req("mode", "enum:RelationMode", "Read or edit."),
            ],
            json!({"type": "relation", "entity": "invoice", "link": "invoice_conversation", "mode": "show"}),
            json!({"type": "relation", "entity": "invoice", "filter": f,
                   "link": "invoice_conversation", "mode": "edit"}),
            false,
        ),
        brick_v(
            "chat",
            "A conversation: its messages in order, and the input.",
            vec![entity_f(), filter_f()],
            json!({"type": "chat", "entity": "conversation"}),
            json!({"type": "chat", "entity": "conversation", "filter": f}),
            false,
        ),
        brick_v(
            "message",
            "A message bubble per row read: its author, its content, and the tool steps the row \
             carries. Filtered on one row, it is one message; filtered on a conversation \
             (`{\"$context\": \"conversation\"}`), it is the thread — without the input, \
             which `chat` has. The content is markdown when `markdown` is true, plain text \
             otherwise; tool steps are read from a field holding a list of \
             `{outil, etat, resume, ms}` objects (a row that has none draws none). A thread is \
             read from the bottom: `page_size` draws the LAST rows and offers a button that \
             widens the window.",
            vec![
                entity_f(),
                filter_f(),
                opt(
                    "markdown",
                    "bool",
                    "The content of a bubble is markdown, rendered by the host. Absent: plain \
                     text — what is read is what is written.",
                )
                .default_value(json!(false)),
                opt(
                    "page_size",
                    "u32",
                    "Thread window, 1..=100. Absent = the whole thread. Draws the LAST rows and \
                     a button that widens the window by as much.",
                ),
            ],
            json!({"type": "message", "entity": "conversation"}),
            json!({"type": "message", "entity": "conversation",
                   "filter": {"cmp": {"field": "titre", "op": "eq", "value": {"$context": "conversation"}}},
                   "markdown": true, "page_size": 40}),
            false,
        ),
        brick_v(
            "notice",
            "A notice read from one row: a tone and a field. For what a host must announce \
             about a turn — why it stopped, a failure, the context size — and cannot write in \
             advance. An EMPTY field announces nothing and the brick is not drawn; a FAILED \
             reading is always drawn, in the error tone. `action` puts a button inside the \
             notice, so that the announcement and its gesture appear and disappear together.",
            vec![
                req("tone", "enum:Tone", "The tone."),
                entity_f(),
                filter_f(),
                req(
                    "field",
                    "string",
                    "The field read: its text is what the notice says.",
                )
                .refers("field"),
                opt(
                    "action",
                    "string",
                    "An action of this module, drawn as a button inside the notice.",
                )
                .refers("action"),
                opt(
                    "label",
                    "string",
                    "The text of that button (interface text: no `_`, not empty). Same \
                     precedence as a button's `label`. Needs `action`: without a button there \
                     is nothing to label.",
                ),
            ],
            json!({"type": "notice", "tone": "info", "entity": "invoice", "field": "titre"}),
            json!({"type": "notice", "tone": "warning", "entity": "invoice", "filter": f,
                   "field": "titre", "action": "go", "label": "Keep going"}),
            false,
        ),
        brick_v(
            "question",
            "The card of a question the agent asks and waits on, in the middle of the thread: \
             its text, its choices, the answer already given. It reads the entity this brick \
             names, and the HOST fills it: pending questions live in the running turn, not in \
             any table. The host provides one row per question under that entity, in the \
             fields named here — `id` carries the gateway's identifier, by which the answer \
             reaches the agent. A click emits an answer intent; the host answers the gateway. \
             No action of the module is called: answering the agent is the person's gesture.",
            vec![
                entity_f(),
                filter_f(),
                req(
                    "text",
                    "string",
                    "The field holding the question, in one sentence.",
                )
                .refers("field"),
                req(
                    "options",
                    "string",
                    "The field holding the possible answers (a list of texts).",
                )
                .refers("field"),
                opt(
                    "answer",
                    "string",
                    "The field holding the answer already given. Absent = the card shows none.",
                )
                .refers("field"),
            ],
            json!({"type": "question", "entity": "demande", "text": "texte",
                   "options": "choix"}),
            json!({"type": "question", "entity": "demande",
                   "filter": {"cmp": {"field": "texte", "op": "starts_with", "value": "A"}},
                   "text": "texte", "options": "choix", "answer": "reponse"}),
            false,
        ),
        brick_v(
            "tool_step",
            "A tool step inside a message thread: its state is a field.",
            vec![
                entity_f(),
                filter_f(),
                req("field", "string", "The state field.").refers("field"),
            ],
            json!({"type": "tool_step", "entity": "conversation", "field": "titre"}),
            json!({"type": "tool_step", "entity": "conversation", "filter": f, "field": "titre"}),
            false,
        ),
        brick_v(
            "queue",
            "A queue: the items to process, in `sort` order.",
            vec![
                entity_f(),
                filter_f(),
                req("label", "string", "The field shown for each item.").refers("field"),
                opt("sort", "list<object:SortSpec>", "The processing order."),
            ],
            json!({"type": "queue", "entity": "invoice", "label": "titre"}),
            json!({"type": "queue", "entity": "invoice", "filter": f, "label": "titre",
                   "sort": [{"field": "amount", "dir": "asc"}]}),
            false,
        ),
        brick_v(
            "code_block",
            "A code block: the text of a field, with a fixed language.",
            vec![
                entity_f(),
                filter_f(),
                req("field", "string", "The field holding the code.").refers("field"),
                opt("language", "string", "The language, for highlighting."),
            ],
            json!({"type": "code_block", "entity": "invoice", "field": "titre"}),
            json!({"type": "code_block", "entity": "invoice", "filter": f, "field": "titre", "language": "rust"}),
            false,
        ),
        brick_v(
            "result",
            "The result of a run: a field, shown as is.",
            vec![
                entity_f(),
                filter_f(),
                req("field", "string", "The field shown.").refers("field"),
            ],
            json!({"type": "result", "entity": "invoice", "field": "titre"}),
            json!({"type": "result", "entity": "invoice", "filter": f, "field": "titre"}),
            false,
        ),
        brick_v(
            "capability_request",
            "A device-capability request card; the action carries the decision. Reads no row.",
            vec![
                req(
                    "capability",
                    "enum:DeviceCapability",
                    "The capability asked.",
                ),
                req(
                    "action",
                    "string",
                    "A `device_capability` action of the SAME capability.",
                )
                .refers("action:device_capability"),
            ],
            json!({"type": "capability_request", "capability": "camera", "action": "cam_ok"}),
            json!({"type": "capability_request", "capability": "camera", "action": "cam_ok"}),
            false,
        ),
        brick_v(
            "composer",
            "The thread's composer: what the input zone OFFERS, declared by the module. Reads no \
             row. The host keeps the real field and its keyboard, sending, stopping a run, \
             drag-and-drop and the attachment queue; the module says the invitation, whether \
             attachments are accepted, and which of its actions sit around the field.",
            vec![
                opt(
                    "placeholder",
                    "string",
                    "The field's invitation. Absent = the host's; empty or blank = refused.",
                ),
                opt(
                    "actions",
                    "list<string>",
                    "Actions of this module, drawn as buttons around the field, in order; each \
                     once. Not an update, a delete, an invoke on an entity or a \
                     device_capability: a composer has no row and no pending question.",
                )
                .refers("action"),
                opt(
                    "attachments",
                    "bool",
                    "Whether the composer accepts attachments (the `+` button, drop, paste).",
                )
                .default_value(json!(false)),
            ],
            json!({"type": "composer"}),
            json!({"type": "composer", "placeholder": "Écris ta question…",
                   "actions": ["say", "new"], "attachments": true}),
            false,
        ),
        brick_v(
            "settings",
            "A settings screen: titled sections, and in each section lines that trigger an \
             action of the module (open a sub-screen, call an operation, ask the host for a \
             secret). Reads no row. Bricks read entities and settings are not entities; no \
             other brick carries a section title or an action's label. A secret has no value \
             here: its line only asks the host to collect it.",
            vec![req(
                "sections",
                "list<object:SettingsSection>",
                "The sections, in order; at least one.",
            )],
            json!({"type": "settings", "sections": [
                {"title": "Engine", "items": [{"label": "Open", "action": "go"}]}
            ]}),
            json!({"type": "settings", "sections": [
                {"title": "Engine", "hint": "Where answers come from",
                 "items": [{"label": "Providers", "hint": "Pick one", "action": "go"}]}
            ]}),
            false,
        ),
        brick_v(
            "secret",
            "A secret input: the person types, the value goes straight to the host's vault. \
             The brick carries only the secret's NAME (and how to show it) — no value ever \
             crosses the module or its view. The field is masked (`type=\"password\"`) and \
             emptied after the value is sent. Reads no row, references no action.",
            vec![
                req(
                    "name",
                    "string",
                    "The secret's name: letters, digits, `_`, `-` or `.`, no space.",
                ),
                opt("label", "string", "The label shown. Absent = the name, humanised."),
                opt("hint", "string", "A sentence under the field."),
            ],
            json!({"type": "secret", "name": "provider-api-key"}),
            json!({"type": "secret", "name": "provider-api-key",
                   "label": "Clé API", "hint": "Ne quitte jamais l'appareil"}),
            false,
        ),
        brick_v(
            "metrics",
            "Numbers: without `field`, the row count; with it, the sum of the field.",
            vec![
                entity_f(),
                filter_f(),
                opt("field", "string", "The field summed.").refers("field"),
            ],
            json!({"type": "metrics", "entity": "invoice"}),
            json!({"type": "metrics", "entity": "invoice", "filter": f, "field": "amount"}),
            true,
        ),
        brick_v(
            "program",
            "A program: typed arguments, a closed run mode, computed columns.",
            vec![
                entity_f(),
                filter_f(),
                opt(
                    "args",
                    "list<object:ProgramArg>",
                    "Typed arguments; names unique.",
                ),
                req("run", "enum:Run", "When it runs."),
                opt(
                    "columns",
                    "list<object:Column>",
                    "Computed columns; names unique.",
                ),
            ],
            json!({"type": "program", "entity": "invoice", "run": "manual"}),
            json!({"type": "program", "entity": "invoice", "filter": f,
                   "args": [{"name": "since", "kind": "date"}], "run": "on_change",
                   "columns": [{"name": "total", "source": {"op": "sum", "field": "amount"}}]}),
            false,
        ),
    ]
}

fn actions() -> Vec<VariantSchema> {
    let name = || {
        req(
            "name",
            "string",
            "Identifier, `^[a-z][a-z0-9_]*$`, unique among the module's actions.",
        )
    };
    let label = || {
        opt(
            "label",
            "string",
            "What a button for this action reads: interface text (\"Pin this key\"), not an \
             identifier — no `_`, not empty. A brick's own `label` wins over it; absent, the \
             host shows the action name humanised.",
        )
    };
    vec![
        variant(
            "create",
            "Creates a row of `entity` with the fields `fields`.",
            vec![
                name(),
                label(),
                req("entity", "string", "The entity created.").refers("entity"),
                req(
                    "fields",
                    "list<string>",
                    "Declared fields written (never base fields); at least one; no duplicate.",
                )
                .refers("field"),
            ],
            json!({"kind": "create", "name": "new", "entity": "invoice", "fields": ["titre"]}),
            json!({"kind": "create", "name": "new", "label": "Do it", "entity": "invoice", "fields": ["titre", "amount"]}),
        ),
        variant(
            "update",
            "Modifies the fields `fields` of a row of `entity`.",
            vec![
                name(),
                label(),
                req("entity", "string", "The entity modified.").refers("entity"),
                req(
                    "fields",
                    "list<string>",
                    "Declared fields written (never base fields); at least one; no duplicate.",
                )
                .refers("field"),
            ],
            json!({"kind": "update", "name": "edit", "entity": "invoice", "fields": ["amount"]}),
            json!({"kind": "update", "name": "edit", "label": "Do it", "entity": "invoice", "fields": ["amount", "titre"]}),
        ),
        variant(
            "delete",
            "Deletes a row of `entity`.",
            vec![
                name(),
                label(),
                req("entity", "string", "The entity deleted.").refers("entity"),
            ],
            json!({"kind": "delete", "name": "drop", "entity": "invoice"}),
            json!({"kind": "delete", "name": "drop", "label": "Do it", "entity": "invoice"}),
        ),
        variant(
            "navigate",
            "Opens a view of this module, or (with `module`) a view of another module.",
            vec![
                name(),
                label(),
                req(
                    "view",
                    "string",
                    "The view opened. Without `module` it must be a view of this module; with \
                     `module` it is NOT checked here (the other module's descriptor is not at \
                     hand): the host resolves it at run time and refuses with a message.",
                ),
                opt(
                    "module",
                    "string",
                    "Open a view of this other module. It must be listed in this module's \
                     `requires[]`.",
                ),
            ],
            json!({"kind": "navigate", "name": "go", "view": "invoices"}),
            json!({"kind": "navigate", "name": "go", "label": "Do it", "view": "providers", "module": "reglages"}),
        ),
        variant(
            "request_secret",
            "Asks the host to collect the secret `secret` (an API key, a token). The module \
             declares that a secret exists and what it is called; typing it, storing it and \
             reading it stay the host's. No value ever passes through the module or its view.",
            vec![
                name(),
                label(),
                req(
                    "secret",
                    "string",
                    "The secret's name: letters, digits, `_`, `-` or `.`, no space.",
                ),
            ],
            json!({"kind": "request_secret", "name": "cle_api", "secret": "provider-api-key"}),
            json!({"kind": "request_secret", "name": "cle_api", "label": "Do it", "secret": "provider-api-key"}),
        ),
        variant(
            "agent_tool",
            "An agent operation: send a message, stop a run. Bound to an entity if needed.",
            vec![
                name(),
                label(),
                req("op", "enum:AgentOp", "The operation."),
                opt("entity", "string", "The entity it applies to.").refers("entity"),
                opt(
                    "text",
                    "string",
                    "What to ask the agent. Required for send_message: the host never writes the request itself.",
                ),
            ],
            json!({"kind": "agent_tool", "name": "halt", "op": "stop_run"}),
            json!({"kind": "agent_tool", "name": "demander", "label": "Do it", "op": "send_message", "entity": "conversation", "text": "Quels articles manquent ?"}),
        ),
        variant(
            "invoke",
            "Runs a named operation of the module: the only way a view reaches its code.",
            vec![
                name(),
                label(),
                req(
                    "operation",
                    "string",
                    "The operation declared in `operations`.",
                )
                .refers("operation"),
                opt("entity", "string", "The entity the operation targets.").refers("entity"),
            ],
            json!({"kind": "invoke", "name": "terminer", "operation": "terminer"}),
            json!({"kind": "invoke", "name": "terminer", "label": "Do it", "operation": "terminer", "entity": "invoice"}),
        ),
        variant(
            "device_capability",
            "Allow or deny a device capability.",
            vec![
                name(),
                label(),
                req(
                    "capability",
                    "enum:DeviceCapability",
                    "The capability decided.",
                ),
                req("grant", "enum:Grant", "The decision."),
            ],
            json!({"kind": "device_capability", "name": "cam_ok", "capability": "camera", "grant": "allow"}),
            json!({"kind": "device_capability", "name": "mic_no", "label": "Do it", "capability": "microphone", "grant": "deny"}),
        ),
        variant(
            "host",
            "Asks the host to open one of ITS surfaces: the installed-modules registry, the \
             model chooser, the app's settings screen. The module does not own them — it \
             offers them, in its own menu, under the label of its choice. No entity, no field: \
             these are the app's own state.",
            vec![
                name(),
                label(),
                req(
                    "surface",
                    "enum:HostSurface",
                    "Which of the host's surfaces to open.",
                ),
            ],
            json!({"kind": "host", "name": "modules", "surface": "modules"}),
            json!({"kind": "host", "name": "reglages", "label": "Do it", "surface": "settings"}),
        ),
    ]
}

fn clicks() -> Vec<VariantSchema> {
    vec![
        variant(
            "select",
            "Selects the row without leaving the view.",
            vec![],
            json!({"kind": "select"}),
            json!({"kind": "select"}),
        ),
        variant(
            "open_detail",
            "Opens the row's detail.",
            vec![],
            json!({"kind": "open_detail"}),
            json!({"kind": "open_detail"}),
        ),
        variant(
            "navigate",
            "Opens another view of this module.",
            vec![req("view", "string", "The view opened.").refers("view")],
            json!({"kind": "navigate", "view": "invoices"}),
            json!({"kind": "navigate", "view": "invoices"}),
        ),
    ]
}

fn column_sources() -> Vec<VariantSchema> {
    vec![
        variant(
            "field",
            "The value of a field.",
            vec![req("field", "string", "The field read.").refers("field")],
            json!({"op": "field", "field": "titre"}),
            json!({"op": "field", "field": "titre"}),
        ),
        variant(
            "count",
            "The number of rows.",
            vec![],
            json!({"op": "count"}),
            json!({"op": "count"}),
        ),
        variant(
            "sum",
            "The sum of a field.",
            vec![req("field", "string", "The field summed.").refers("field")],
            json!({"op": "sum", "field": "amount"}),
            json!({"op": "sum", "field": "amount"}),
        ),
    ]
}

/// The filter grammar. Externally tagged: `{"and": [...]}`, `{"cmp": {...}}`.
fn filter_variants() -> Vec<VariantSchema> {
    let cmp = || json!({"cmp": {"field": "amount", "op": "gt", "value": 0}});
    vec![
        variant(
            "and",
            "All sub-filters hold. Value: a list of filters.",
            vec![req(
                "and",
                "list<filter>",
                "The sub-filters (the JSON key is the variant name).",
            )],
            json!({"and": [cmp()]}),
            json!({"and": [cmp(), cmp()]}),
        ),
        variant(
            "or",
            "At least one sub-filter holds. Value: a list of filters.",
            vec![req("or", "list<filter>", "The sub-filters.")],
            json!({"or": [cmp()]}),
            json!({"or": [cmp(), cmp()]}),
        ),
        variant(
            "not",
            "Negation. Value: one filter.",
            vec![req("not", "filter", "The negated filter.")],
            json!({"not": cmp()}),
            json!({"not": cmp()}),
        ),
        variant(
            "cmp",
            "A comparison. `field` is a path (`amount`, or `target.name` through a link); `op` is one of \
             eq, ne, gt, gte, lt, lte, in, contains, starts_with, is_null. `value` may be a \
             context reference, `{\"$context\": \"<key>\"}`: the host fills it in when the view \
             is drawn (see `context` in the type grammar).",
            vec![
                req("field", "string", "The field compared.").refers("field"),
                req("op", "enum:FilterOp", "The operator."),
                req(
                    "value",
                    "json",
                    "The value compared (any JSON), or `{\"$context\": \"<key>\"}` for a value the \
                     host provides at render time.",
                ),
            ],
            json!({"cmp": {"field": "amount", "op": "gt", "value": 0}}),
            json!({"cmp": {"field": "conversation", "op": "eq", "value": {"$context": "conversation"}}}),
        ),
    ]
}

fn objects() -> Vec<ObjectSchema> {
    vec![
        object(
            "Tab",
            "A tab of a `tabs` container.",
            vec![
                req("label", "string", "Non-empty, unique in its container."),
                req("children", "list<node>", "The tab's subtree."),
            ],
            json!({"label": "All", "children": []}),
            json!({"label": "All", "children": [{"type": "stack", "children": []}]}),
        ),
        object(
            "SettingsSection",
            "A titled section of a `settings` brick.",
            vec![
                req("title", "string", "Non-blank."),
                opt("hint", "string", "A sentence under the title."),
                req(
                    "items",
                    "list<object:SettingsItem>",
                    "The lines; at least one.",
                ),
            ],
            json!({"title": "Engine", "items": [{"label": "Open", "action": "go"}]}),
            json!({"title": "Engine", "hint": "Where answers come from",
                   "items": [{"label": "Open", "action": "go"}]}),
        ),
        object(
            "SettingsItem",
            "A line of a settings section: a label, a hint, the action it triggers.",
            vec![
                req("label", "string", "Non-blank."),
                opt("hint", "string", "A sentence under the label."),
                req(
                    "action",
                    "string",
                    "An action of this module that needs no row: navigate, request_secret, \
                     agent_tool, device_capability, or a module-level invoke.",
                )
                .refers("action"),
            ],
            json!({"label": "Open", "action": "go"}),
            json!({"label": "Open", "hint": "Pick one", "action": "go"}),
        ),
        object(
            "Look",
            "Declared appearance. Closed choices; the host styles them.",
            vec![
                opt("accent", "enum:Accent", "Accent colour.").default_value(json!("neutral")),
                opt(
                    "layout",
                    "enum:Layout",
                    "Layout of the root container's children.",
                )
                .default_value(json!("stack")),
                opt("transition", "enum:Transition", "Transition on opening.")
                    .default_value(json!("none")),
            ],
            json!({}),
            json!({"accent": "primary", "layout": "grid", "transition": "slide"}),
        ),
        object(
            "FormField",
            "A field of a `form`; its position is the declared order.",
            vec![
                req("field", "string", "The entity field.").refers("field"),
                opt("label", "string", "Label shown instead of the field name."),
                opt("read_only", "bool", "Display only.").default_value(json!(false)),
            ],
            json!({"field": "titre"}),
            json!({"field": "titre", "label": "Title", "read_only": true}),
        ),
        object(
            "SortSpec",
            "One sort criterion.",
            vec![
                req("field", "string", "The field sorted on.").refers("field"),
                opt("dir", "enum:SortDir", "Direction.").default_value(json!("asc")),
            ],
            json!({"field": "amount"}),
            json!({"field": "amount", "dir": "desc"}),
        ),
        object(
            "ProgramArg",
            "A typed argument of a program.",
            vec![
                req(
                    "name",
                    "string",
                    "Identifier, unique among the program's args.",
                ),
                req("kind", "enum:ArgKind", "The argument type."),
            ],
            json!({"name": "since", "kind": "date"}),
            json!({"name": "since", "kind": "date"}),
        ),
        object(
            "Column",
            "A computed column of a program.",
            vec![
                req(
                    "name",
                    "string",
                    "Identifier, unique among the program's columns.",
                ),
                req(
                    "source",
                    "object:ColumnSource",
                    "How the value is computed; see `column_sources`.",
                ),
            ],
            json!({"name": "n", "source": {"op": "count"}}),
            json!({"name": "total", "source": {"op": "sum", "field": "amount"}}),
        ),
    ]
}

fn enums() -> Vec<EnumSchema> {
    vec![
        enum_s(
            "Surface",
            "Where a view is drawn.",
            Some("main"),
            &[
                ("sidebar", "The side menu, when there is one."),
                ("main", "The main screen."),
                ("panel", "A panel beside or over the main content."),
                ("header", "A thin bar: only small bricks."),
                (
                    "chat",
                    "The conversation thread: the same bricks, in the chat.",
                ),
                (
                    "widget",
                    "The home-screen widget: drawn by another host, same bricks.",
                ),
                ("lock_widget", "The lock-screen widget (iOS 16+)."),
                (
                    "activity",
                    "A Live Activity: a state the app pushes, not a screen.",
                ),
                ("island", "The Dynamic Island layouts of a Live Activity."),
            ],
        ),
        enum_s(
            "Accent",
            "Accent colour.",
            Some("neutral"),
            &[
                ("neutral", ""),
                ("primary", ""),
                ("success", ""),
                ("warning", ""),
                ("danger", ""),
                ("info", ""),
            ],
        ),
        enum_s(
            "Layout",
            "Layout of the root container's children.",
            Some("stack"),
            &[("stack", ""), ("grid", ""), ("compact", "")],
        ),
        enum_s(
            "Transition",
            "Transition on opening.",
            Some("none"),
            &[("none", ""), ("fade", ""), ("slide", "")],
        ),
        enum_s(
            "Tone",
            "Tone of a banner.",
            None,
            &[
                ("info", ""),
                ("success", ""),
                ("warning", ""),
                ("error", ""),
            ],
        ),
        enum_s(
            "Signal",
            "What an indicator shows.",
            None,
            &[
                ("connection", "Connection state; no entity."),
                (
                    "activity",
                    "Phase and elapsed duration; entity + field (+ duration).",
                ),
                ("counter", "A count of rows; entity."),
                ("badge", "A value; entity + field."),
            ],
        ),
        enum_s(
            "Input",
            "The control of an editable field.",
            Some("text"),
            &[
                ("text", "Single-line text."),
                ("multiline", "Multi-line text."),
                ("number", "A number; the field must be numeric."),
                ("file", "A file."),
                (
                    "reference",
                    "A reference to another entity, with a picker; needs `target`.",
                ),
                ("toggle", "On/off; the field must be a bool."),
            ],
        ),
        enum_s(
            "RelationMode",
            "Read or edit a relation.",
            None,
            &[("show", ""), ("edit", "")],
        ),
        enum_s(
            "ArgKind",
            "Type of a program argument.",
            None,
            &[("text", ""), ("number", ""), ("bool", ""), ("date", "")],
        ),
        enum_s(
            "Run",
            "When a program runs.",
            None,
            &[
                ("manual", "On demand, by a button or a tool."),
                ("on_change", "On every change of the rows read."),
            ],
        ),
        enum_s(
            "AgentOp",
            "Exposed agent operations.",
            None,
            &[("send_message", ""), ("stop_run", "")],
        ),
        enum_s(
            "Grant",
            "Decision on a capability.",
            None,
            &[("allow", ""), ("deny", "")],
        ),
        enum_s(
            "HostSurface",
            "A surface the host keeps and a module may offer in its menu.",
            None,
            &[
                (
                    "modules",
                    "The installed-modules registry: enable, remove, open.",
                ),
                ("model", "The chooser of the current model."),
                ("settings", "The app's settings screen."),
            ],
        ),
        enum_s(
            "DeviceCapability",
            "Device capabilities.",
            None,
            &[
                ("camera", ""),
                ("microphone", ""),
                ("location", ""),
                ("screen", ""),
                ("device_motion", ""),
                // Les vingt et une capacités du pont d'appareil, pour qu'une vue de module
                // puisse demander autre chose que cinq accès (mesuré le 08/11/2026).
                ("contacts", ""),
                ("calendar", ""),
                ("health", ""),
                ("bluetooth", ""),
                ("gyroscope", ""),
                ("motion_activity", ""),
                ("nfc", ""),
                ("location_always", ""),
                ("speech", ""),
                ("face_id", ""),
                ("photos", ""),
                ("photos_pick", ""),
                ("files_pick", ""),
                ("clipboard", ""),
                ("share", ""),
                ("notifications", ""),
                ("live_activity", ""),
                ("local_network", ""),
            ],
        ),
        enum_s(
            "SortDir",
            "Sort direction.",
            Some("asc"),
            &[("asc", ""), ("desc", "")],
        ),
        enum_s(
            "FilterOp",
            "Comparison operator of a filter.",
            None,
            &[
                ("eq", ""),
                ("ne", ""),
                ("gt", ""),
                ("gte", ""),
                ("lt", ""),
                ("lte", ""),
                ("in", ""),
                ("contains", ""),
                ("starts_with", ""),
                ("is_null", ""),
            ],
        ),
    ]
}

fn rules() -> Vec<RuleSchema> {
    vec![
        rule(
            "invalid_name",
            "A view, action, argument, column or tab name is not valid.",
            "Use `^[a-z][a-z0-9_]*$` (tab labels and form fields: non-empty).",
        ),
        rule(
            "duplicate_view",
            "Two views share a name.",
            "Rename one; view names are unique in the module.",
        ),
        rule(
            "action_agent_text_missing",
            "A `send_message` action does not say what it asks.",
            "Add `text`: the host never writes the request in the module's place.",
        ),
        rule(
            "action_agent_text_unused",
            "A `stop_run` action carries a `text` that would never be sent.",
            "Remove `text`; only `send_message` sends something.",
        ),
        rule(
            "duplicate_action",
            "Two actions share a name.",
            "Rename one; action names are unique in the module.",
        ),
        rule(
            "duplicate_operation",
            "Two operations share a name.",
            "Rename one; operation names are unique in the module.",
        ),
        rule(
            "operation_entity_unknown",
            "An operation names an entity the module does not declare.",
            "Declare the entity in `entities` or fix the name.",
        ),
        rule(
            "action_operation_unknown",
            "An invoke action names an operation the module does not declare.",
            "Declare the operation in `operations` or fix the name.",
        ),
        rule(
            "action_entity_unknown",
            "An action names an entity the module does not declare.",
            "Declare the entity in `entities` or fix the name.",
        ),
        rule(
            "action_field_unknown",
            "A create/update action writes a field that is not declared (base fields are never writable).",
            "Write only fields declared on the entity.",
        ),
        rule(
            "action_fields_empty",
            "A create/update action writes no field.",
            "List at least one field.",
        ),
        rule(
            "duplicate_action_field",
            "A create/update action lists a field twice.",
            "List each field once.",
        ),
        rule(
            "view_target_unknown",
            "A navigate action, a click or `replaces` names a view that does not exist.",
            "Declare the view in `views` or fix the name.",
        ),
        rule(
            "view_entity_unknown",
            "A brick (or a reference `target`) names an entity the module does not declare.",
            "Declare the entity or fix the name.",
        ),
        rule(
            "view_entity_required",
            "An indicator other than `connection` has no `entity`.",
            "Add `entity`, or use the `connection` signal.",
        ),
        rule(
            "view_field_required",
            "An `activity` or `badge` indicator has no `field`.",
            "Add `field` (the phase, or the value).",
        ),
        rule(
            "view_field_unknown",
            "A brick reads a field the entity does not have (declared or base).",
            "Declare the field on the entity or fix the name.",
        ),
        rule(
            "view_action_unknown",
            "A brick calls an action the module does not declare.",
            "Add the action to `actions` or fix the name.",
        ),
        rule(
            "view_action_kind_mismatch",
            "A form submits something other than a create/update of its own entity, or a capability request points at an action that is not the matching `device_capability`.",
            "Point `submit` at a create/update action on the form's entity; point a capability request at a `device_capability` action of the same capability.",
        ),
        rule(
            "view_relation_unknown",
            "A relation names a link that does not start at its entity.",
            "Declare the link in `links` with this entity as source, or fix `link`.",
        ),
        rule(
            "field_input_invalid",
            "A `reference` input has no `target`, another input has one, or a `toggle`/`number` input sits on a field of the wrong type.",
            "Give a reference its `target`; use `toggle` on bool fields and `number` on numeric ones.",
        ),
        rule(
            "surface_brick_refused",
            "A brick that does not fit a `header` is on a header surface.",
            "Use text, caption, banner, indicator, metrics or button, or move the view to main/sidebar/panel.",
        ),
        rule(
            "composer_placeholder_empty",
            "A composer declares an empty or blank `placeholder`.",
            "Write a short invitation, or remove `placeholder` to keep the host's.",
        ),
        rule(
            "duplicate_composer_action",
            "A composer lists an action twice.",
            "List each composer action once.",
        ),
        rule(
            "composer_action_refused",
            "A composer names an action that needs a row (update, delete, invoke on an entity) or decides a device question (device_capability).",
            "Use a create, navigate, agent_tool or module-level invoke action; put row actions on a `button` of a view that reads the row.",
        ),
        rule(
            "navigate_module_not_required",
            "A navigate action opens a view of another module that this module does not list in `requires[]`.",
            "Declare the module in `requires[]`, or fix its name. The view itself is not checked here: the host resolves it at run time and refuses with a message.",
        ),
        rule(
            "navigate_module_is_self",
            "A navigate action names this very module in `module`.",
            "Drop `module` to open one of this module's own views.",
        ),
        rule(
            "action_secret_invalid",
            "A request_secret action names a secret with an empty name, a space or a character outside letters, digits, `_`, `-`, `.`.",
            "Use a short name such as `provider-api-key`.",
        ),
        rule(
            "settings_empty",
            "A settings brick has no section, or a section has no item.",
            "Add the section or the item, or remove the empty one.",
        ),
        rule(
            "settings_text_empty",
            "A settings section has a blank title, or an item a blank label.",
            "Write the title or the label.",
        ),
        rule(
            "settings_action_refused",
            "A settings line names an action that needs a row (create, update, delete, invoke on an entity).",
            "Use navigate, request_secret, agent_tool, device_capability or a module-level invoke.",
        ),
        rule(
            "invalid_page_size",
            "A list `page_size` is 0 or above 100.",
            "Use 1..=100, or omit it for no pagination.",
        ),
        rule(
            "program_name_duplicate",
            "A program declares two args, or two columns, with the same name.",
            "Rename one.",
        ),
        rule(
            "duplicate_form_field",
            "A form lists a field twice.",
            "List each field once.",
        ),
        rule(
            "duplicate_tab",
            "Two tabs of one container share a label.",
            "Give each tab its own label.",
        ),
        rule(
            "tabs_empty",
            "A `tabs` container has no tab.",
            "Add a tab, or use `stack`.",
        ),
        rule(
            "label_empty",
            "A `label` (on an action, a button or a notice) is empty or only spaces.",
            "Write the text the person should read, or drop `label` to get the action name humanised.",
        ),
        rule(
            "label_not_text",
            "A `label` is not interface text: it holds a `_` (an identifier) or a control character.",
            "Write plain text with accents and spaces, e.g. \"Épingler cette clé\".",
        ),
        rule(
            "notice_label_without_action",
            "A notice has a `label` but no `action`: there is no button to label.",
            "Add the `action`, or drop the `label`.",
        ),
        rule(
            "view_too_deep",
            "Containers are nested deeper than the maximum depth.",
            "Flatten the tree or split it in several views.",
        ),
        rule(
            "html_path_empty",
            "An `html` node has an empty path.",
            "Name the file the module ships, e.g. `ui/index.html`.",
        ),
        rule(
            "html_path_outside_module",
            "An `html` path is absolute outside the module folder, or climbs out with `..`.",
            "Use a path relative to the module, e.g. `ui/index.html`.",
        ),
        rule(
            "html_extension_refused",
            "An `html` path does not end in .html or .htm.",
            "Serve an .html page (scripts and styles go inline or in that page).",
        ),
        rule(
            "html_svg_refused",
            "An `html` path is an .svg: its own scripts and external references escape a <meta> CSP.",
            "Serve the SVG as an image, not as an `html` page.",
        ),
        rule(
            "html_in_header",
            "An `html` node sits in a `header` view: a thin bar cannot host an iframe.",
            "Move the view to `main`, `sidebar` or `panel`.",
        ),
        rule(
            "view_replaces_self",
            "A view replaces itself.",
            "Point `replaces` at another view, or remove it.",
        ),
        rule(
            "view_replaced_twice",
            "Two views replace the same one.",
            "Keep one replacement per view.",
        ),
        rule(
            "view_replaces_cycle",
            "Replacements form a cycle (a -> b -> a).",
            "Break the chain: the host would show no view of the cycle.",
        ),
        rule(
            "secret_name_empty",
            "A secret brick declares an empty secret name.",
            "Name the secret the host writes, e.g. `provider-api-key`.",
        ),
        rule(
            "secret_name_invalid",
            "A secret brick declares a secret name with a space or a character outside letters, digits, `_`, `-`, `.`.",
            "Use a short name such as `provider-api-key`.",
        ),
        rule(
            "row_action_refused",
            "A list row action names an action that does not aim at the row (create, navigate, request_secret, agent_tool, device_capability, host, module-level invoke).",
            "Use an update, delete or entity invoke action.",
        ),
        rule(
            "duplicate_row_action",
            "A list declares the same row action twice.",
            "List each row action once.",
        ),
    ]
}

fn coverage() -> Vec<CoverageEntry> {
    vec![
        cover(
            "list",
            &["brick:list"],
            "Rows with label, subtitle, sort, filter, pagination, click.",
        ),
        cover("detail", &["brick:card"], "Several fields of one row."),
        cover(
            "form",
            &["brick:form"],
            "Ordered fields, labels, read-only, submit action.",
        ),
        cover(
            "chat",
            &["brick:chat", "brick:message"],
            "Thread and input (`chat`); a bubble per row read, filtered on the open conversation \
             through `{\"$context\": \"conversation\"}` (`message`); `tool_step` draws a tool step.",
        ),
        cover(
            "metrics",
            &["brick:metrics", "brick:indicator(signal=counter)"],
            "Row count, or sum of a field.",
        ),
        cover("tabs", &["node:tabs"], "One subtree per tab."),
        cover(
            "html",
            &["node:html"],
            "A page the module ships, in a sandboxed iframe: the unlimited door, outside the contract.",
        ),
        cover("sheet", &["node:modal"], "A titled sheet over the content."),
        cover(
            "drawer",
            &["node:drawer"],
            "A fold-away section under a label.",
        ),
        cover("button", &["brick:button"], "Triggers an action."),
        cover(
            "label",
            &["brick:button", "brick:notice", "action:create"],
            "A button reads the `label` of the brick, else the `label` of the action (every \
             action kind has one), else the action name humanised. Row actions and composer \
             actions take the action's label. Interface text: no `_`, not empty.",
        ),
        cover(
            "conditional",
            &["node:when"],
            "Show a node (a brick or a whole group) only if a row of an entity satisfies a \
             filter. Absent data or a failed read hides it; never shows it at random.",
        ),
        cover(
            "question",
            &["brick:question"],
            "The card of a question the agent asks and waits on: its text, its choices, the \
             answer already given. The host fills the row; a click answers the agent's gateway.",
        ),
        cover(
            "composer",
            &["brick:composer"],
            "What the thread's input offers: invitation, attachments, the module's actions \
             around the field. The field, sending and stopping stay the host's.",
        ),
        cover(
            "settings",
            &["brick:settings", "action:request_secret"],
            "Titled sections of lines that open a sub-screen (also of another required \
             module), call an operation or ask the host for a secret. Secrets and the vault \
             stay the host's.",
        ),
        cover(
            "secret input",
            &["brick:secret"],
            "The person types; the value goes straight to the host's vault, never through \
             the module.",
        ),
        cover(
            "row action",
            &["brick:list"],
            "A button per row, executing an action with the row's id.",
        ),
        cover("text", &["brick:text"], "One field of the first row."),
        cover(
            "secondary line",
            &["brick:caption"],
            "One field of the first row, small and muted, without frame.",
        ),
        cover(
            "banner",
            &["brick:banner", "brick:notice"],
            "`banner`: a closed tone and a declared text. `notice`: a closed tone and a FIELD — \
             for what the host must announce about a turn; an empty field draws nothing.",
        ),
        cover(
            "indicator",
            &["brick:indicator"],
            "connection, activity, counter, badge.",
        ),
        cover(
            "text field",
            &["brick:field(input=text)", "brick:field(input=multiline)"],
            "",
        ),
        cover(
            "number field",
            &["brick:field(input=number)"],
            "Added with this audit: `Input` had no numeric control.",
        ),
        cover(
            "toggle field",
            &["brick:field(input=toggle)"],
            "The field must be a bool.",
        ),
        cover(
            "file field",
            &["brick:field(input=file)"],
            "The control only: where the bytes go is the host's business.",
        ),
        cover(
            "reference field",
            &["brick:field(input=reference)"],
            "Needs `target`.",
        ),
        cover(
            "relation",
            &["brick:relation"],
            "Through a declared link; show or edit.",
        ),
        cover("tool step", &["brick:tool_step"], "State is a field."),
        cover("queue", &["brick:queue"], "Items in `sort` order."),
        cover("code block", &["brick:code_block"], "Fixed language."),
        cover("result", &["brick:result"], "A field shown as is."),
        cover(
            "capability request",
            &["brick:capability_request", "action:device_capability"],
            "The action carries the decision.",
        ),
        cover(
            "program",
            &["brick:program"],
            "Typed args, closed run mode, computed columns.",
        ),
        cover(
            "multi-column table",
            &[],
            "NOT covered: `list` shows a label and a subtitle; use `card` per row or `program` columns. A table brick would be a new variant.",
        ),
        cover(
            "date / select field",
            &[],
            "NOT covered: no `date` or enum-select control in `Input`; `text` stands in.",
        ),
        cover("image / media", &[], "NOT covered: no media brick."),
    ]
}

fn example_module() -> Value {
    json!({
        "name": "billing",
        "version": "1.0.0",
        "entities": [{"name": "invoice", "fields": [
            {"name": "titre", "ty": "string"}, {"name": "amount", "ty": "f64"}]}],
        "actions": [
            {"kind": "create", "name": "new", "entity": "invoice", "fields": ["titre", "amount"]}
        ],
        "views": [{
            "name": "invoices", "surface": "main", "title": "Invoices",
            "root": {"type": "stack", "children": [
                {"type": "leaf", "brick": {"type": "list", "entity": "invoice", "label": "titre",
                                           "subtitle": "amount", "on_click": {"kind": "open_detail"}}},
                {"type": "leaf", "brick": {"type": "button", "entity": "invoice", "action": "new"}}
            ]}
        }]
    })
}

// ---------------------------------------------------------------------------------------
// parsing with actionable errors
// ---------------------------------------------------------------------------------------

/// A view-contract parse failure that tells the author what to do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ViewParseError {
    /// What serde said (path-free, with line and column).
    pub message: String,
    /// What to do about it: the valid choices, the closest one, where to read more.
    pub hint: String,
}

impl std::fmt::Display for ViewParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} — {}", self.message, self.hint)
    }
}

impl std::error::Error for ViewParseError {}

/// The backticked names serde lists after `expected` in an error message.
pub(crate) fn expected_in(msg: &str) -> Vec<String> {
    let Some(i) = msg.find("expected") else {
        return Vec::new();
    };
    let rest = &msg[i..];
    let mut out = Vec::new();
    let mut parts = rest.split('`');
    parts.next(); // text before the first backtick
    while let Some(name) = parts.next() {
        out.push(name.to_string());
        parts.next(); // separator
    }
    out
}

/// The backticked name after `unknown variant` / `unknown field` / `missing field`.
fn subject_of(msg: &str, lead: &str) -> Option<String> {
    let i = msg.find(lead)?;
    let rest = &msg[i + lead.len()..];
    let start = rest.find('`')? + 1;
    let end = rest[start..].find('`')? + start;
    Some(rest[start..end].to_string())
}

fn hint_for(kind: &str, msg: &str) -> String {
    let see = "Read `view_schema()` for the fields and examples of each variant.";
    for (lead, what) in [("unknown variant", "value"), ("unknown field", "field")] {
        if let Some(bad) = subject_of(msg, lead) {
            let valid = expected_in(msg);
            let mut h = format!("`{bad}` is not a valid {what} of a {kind}");
            if !valid.is_empty() {
                h.push_str(&format!("; valid: {}", valid.join(", ")));
            }
            if let Some(c) = crate::views::closest(&bad, valid.iter().map(String::as_str)) {
                h.push_str(&format!(". Did you mean `{c}`?"));
            }
            return format!("{h}. {see}");
        }
    }
    if let Some(missing) = subject_of(msg, "missing field") {
        return format!("add the required field `{missing}` to this {kind}. {see}");
    }
    format!("fix the {kind} JSON. {see}")
}

fn parse<T: DeserializeOwned>(kind: &str, json: &str) -> Result<T, ViewParseError> {
    serde_json::from_str(json).map_err(|e| {
        let message = e.to_string();
        let hint = hint_for(kind, &message);
        ViewParseError { message, hint }
    })
}

/// Parses a [`ViewDesc`]; an unknown brick, field or value is refused with the valid choices.
pub fn parse_view(json: &str) -> Result<ViewDesc, ViewParseError> {
    parse("view", json)
}

/// Parses a [`Node`] (container or leaf).
pub fn parse_node(json: &str) -> Result<Node, ViewParseError> {
    parse("node", json)
}

/// Parses a [`Brick`].
pub fn parse_brick(json: &str) -> Result<Brick, ViewParseError> {
    parse("brick", json)
}

/// Parses an [`Action`].
pub fn parse_action(json: &str) -> Result<Action, ViewParseError> {
    parse("action", json)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::check::CheckProfile;
    use crate::custom::CustomRegistry;
    use crate::entity::EntityDesc;
    use crate::field::{FieldDesc, FieldType};
    use crate::link::LinkDesc;
    use crate::module::ModuleDescriptor;
    use crate::views::{
        Accent, AgentOp, ArgKind, Click, ColumnSource, DeviceCapability, Grant, HostSurface, Input,
        Layout, Look, RelationMode, Run, Signal, Surface, Tone, Transition,
    };
    use std::collections::BTreeSet;

    // ---------- helpers ----------

    /// What serde itself says about `v` as a `T`.
    fn err_of<T: DeserializeOwned>(v: Value) -> String {
        match serde_json::from_value::<T>(v) {
            Ok(_) => String::new(),
            Err(e) => e.to_string(),
        }
    }

    /// Deserializes `v` as the type named `ty`, and serializes it back.
    fn round_trip(ty: &str, v: &Value) -> Result<Value, String> {
        fn go<T: DeserializeOwned + Serialize>(v: &Value) -> Result<Value, String> {
            let t: T = serde_json::from_value(v.clone()).map_err(|e| e.to_string())?;
            serde_json::to_value(&t).map_err(|e| e.to_string())
        }
        match ty {
            "ViewDesc" => go::<ViewDesc>(v),
            "node" => go::<Node>(v),
            "brick" => go::<Brick>(v),
            "action" => go::<Action>(v),
            "click" => go::<Click>(v),
            "column_source" => go::<ColumnSource>(v),
            "filter" => go::<crate::query::FilterExpr>(v),
            "Tab" => go::<crate::views::Tab>(v),
            "SettingsSection" => go::<crate::views::SettingsSection>(v),
            "SettingsItem" => go::<crate::views::SettingsItem>(v),
            "Look" => go::<Look>(v),
            "FormField" => go::<crate::views::FormField>(v),
            "SortSpec" => go::<crate::query::SortSpec>(v),
            "ProgramArg" => go::<crate::views::ProgramArg>(v),
            "Column" => go::<crate::views::Column>(v),
            other => Err(format!("test bug: no type for `{other}`")),
        }
    }

    /// The tag key of each tagged family.
    const FAMILIES: [(&str, &str); 5] = [
        ("node", "type"),
        ("brick", "type"),
        ("action", "kind"),
        ("click", "kind"),
        ("column_source", "op"),
    ];

    fn variants_of<'a>(s: &'a ViewSchema, family: &str) -> &'a [VariantSchema] {
        match family {
            "node" => &s.nodes,
            "brick" => &s.bricks,
            "action" => &s.actions,
            "click" => &s.clicks,
            "column_source" => &s.column_sources,
            "filter" => &s.filter,
            other => panic!("test bug: family `{other}`"),
        }
    }

    fn set(v: impl IntoIterator<Item = String>) -> BTreeSet<String> {
        v.into_iter().collect()
    }

    /// A module every example of the schema can be checked against.
    fn fixture() -> ModuleDescriptor {
        let mut m = ModuleDescriptor::new("billing", "1.0.0");
        let mut invoice = EntityDesc::new("invoice");
        invoice.fields = vec![
            FieldDesc::new("titre", FieldType::String),
            FieldDesc::new("amount", FieldType::F64),
            FieldDesc::new("paid", FieldType::Bool),
        ];
        let mut conversation = EntityDesc::new("conversation");
        conversation.fields = vec![FieldDesc::new("titre", FieldType::String)];
        // L'entité d'une carte de question : l'hôte la remplit, le module la déclare.
        let mut demande = EntityDesc::new("demande");
        demande.fields = vec![
            FieldDesc::new("texte", FieldType::String),
            FieldDesc::new("choix", FieldType::Json),
            FieldDesc::new("reponse", FieldType::String),
        ];
        m.entities.push(invoice);
        m.entities.push(conversation);
        m.entities.push(demande);
        m.links.push(LinkDesc::new(
            "invoice_conversation",
            "invoice",
            "conversation",
        ));
        for a in [
            r#"{"kind":"create","name":"new","entity":"invoice","fields":["titre","amount"]}"#,
            r#"{"kind":"update","name":"edit","entity":"invoice","fields":["amount"]}"#,
            r#"{"kind":"delete","name":"drop","entity":"invoice"}"#,
            r#"{"kind":"navigate","name":"go","view":"invoices"}"#,
            r#"{"kind":"navigate","name":"go_other","view":"providers","module":"reglages"}"#,
            r#"{"kind":"request_secret","name":"cle_api","secret":"provider-api-key"}"#,
            r#"{"kind":"agent_tool","name":"halt","op":"stop_run"}"#,
            r#"{"kind":"agent_tool","name":"say","op":"send_message","entity":"conversation","text":"Quels articles manquent ?"}"#,
            r#"{"kind":"device_capability","name":"cam_ok","capability":"camera","grant":"allow"}"#,
            r#"{"kind":"device_capability","name":"mic_no","capability":"microphone","grant":"deny"}"#,
        ] {
            m.actions.push(parse_action(a).expect("fixture action"));
        }
        m.requires.push(crate::ModuleReq {
            name: "reglages".into(),
            version: "*".into(),
            optional: false,
        });
        m.views.push(
            parse_view(r#"{"name":"invoices","root":{"type":"stack","children":[]}}"#)
                .expect("fixture view"),
        );
        m.operations
            .push(crate::OperationDesc::new("terminer").entity("invoice"));
        m
    }

    fn view_with_root(name: &str, surface: Surface, root: Value) -> ViewDesc {
        let v = json!({"name": name, "surface": surface, "root": root});
        parse_view(&v.to_string()).expect("view")
    }

    fn check_codes(m: &ModuleDescriptor) -> Vec<String> {
        m.check(&CustomRegistry::default(), CheckProfile::Compiled)
            .err()
            .unwrap_or_default()
            .into_iter()
            .map(|i| i.code.0)
            .collect()
    }

    // ---------- the guard: schema == contract ----------

    #[test]
    fn schema_names_exactly_the_variants_serde_knows_for_every_tagged_family() {
        let s = view_schema();
        for (family, tag) in FAMILIES {
            let probe = json!({ tag: "__nope__" });
            let msg = match family {
                "node" => err_of::<Node>(probe),
                "brick" => err_of::<Brick>(probe),
                "action" => err_of::<Action>(probe),
                "click" => err_of::<Click>(probe),
                _ => err_of::<ColumnSource>(probe),
            };
            let real = set(expected_in(&msg));
            let described = set(variants_of(&s, family).iter().map(|v| v.name.clone()));
            assert!(
                !real.is_empty(),
                "{family}: serde listed no variant in `{msg}`"
            );
            assert_eq!(
                described, real,
                "{family}: the schema and the contract diverge (left = schema, right = serde)"
            );
        }
        // Filter expressions are externally tagged.
        let msg = err_of::<crate::query::FilterExpr>(json!({"__nope__": 1}));
        assert_eq!(
            set(variants_of(&s, "filter").iter().map(|v| v.name.clone())),
            set(expected_in(&msg)),
            "filter: the schema and the contract diverge"
        );
    }

    #[test]
    fn schema_names_exactly_the_values_serde_knows_for_every_enumeration() {
        let s = view_schema();
        let real: Vec<(&str, String)> = vec![
            ("Surface", err_of::<Surface>(json!("__nope__"))),
            ("Accent", err_of::<Accent>(json!("__nope__"))),
            ("Layout", err_of::<Layout>(json!("__nope__"))),
            ("Transition", err_of::<Transition>(json!("__nope__"))),
            ("Tone", err_of::<Tone>(json!("__nope__"))),
            ("Signal", err_of::<Signal>(json!("__nope__"))),
            ("Input", err_of::<Input>(json!("__nope__"))),
            ("RelationMode", err_of::<RelationMode>(json!("__nope__"))),
            ("ArgKind", err_of::<ArgKind>(json!("__nope__"))),
            ("Run", err_of::<Run>(json!("__nope__"))),
            ("AgentOp", err_of::<AgentOp>(json!("__nope__"))),
            ("Grant", err_of::<Grant>(json!("__nope__"))),
            ("HostSurface", err_of::<HostSurface>(json!("__nope__"))),
            (
                "DeviceCapability",
                err_of::<DeviceCapability>(json!("__nope__")),
            ),
            (
                "SortDir",
                err_of::<crate::query::SortDir>(json!("__nope__")),
            ),
            (
                "FilterOp",
                err_of::<crate::query::FilterOp>(json!("__nope__")),
            ),
        ];
        assert_eq!(
            set(real.iter().map(|(n, _)| (*n).to_string())),
            set(s.enums.iter().map(|e| e.name.clone())),
            "an enumeration is missing from, or phantom in, the schema"
        );
        for (name, msg) in real {
            let e = s.enums.iter().find(|e| e.name == name).expect("enum");
            assert_eq!(
                set(e.values.iter().map(|v| v.name.clone())),
                set(expected_in(&msg)),
                "enum {name}: the schema and the contract diverge"
            );
        }
    }

    #[test]
    fn schema_names_exactly_the_fields_serde_knows_for_every_variant_and_object() {
        let s = view_schema();
        for (family, tag) in FAMILIES {
            for v in variants_of(&s, family) {
                let probe = json!({ tag: v.name, "__nope__": 1 });
                let msg = match family {
                    "node" => err_of::<Node>(probe),
                    "brick" => err_of::<Brick>(probe),
                    "action" => err_of::<Action>(probe),
                    "click" => err_of::<Click>(probe),
                    _ => err_of::<ColumnSource>(probe),
                };
                if v.fields.is_empty() {
                    // serde ignores extra keys on a UNIT variant of an internally tagged
                    // enum even with `deny_unknown_fields` (select, open_detail, count).
                    continue;
                }
                assert!(
                    msg.contains("unknown field"),
                    "{family}/{}: an unknown field must be refused, got `{msg}`",
                    v.name
                );
                let real: BTreeSet<String> = set(expected_in(&msg))
                    .into_iter()
                    .filter(|f| f != tag)
                    .collect();
                let described = set(v.fields.iter().map(|f| f.name.clone()));
                assert_eq!(described, real, "{family}/{}: fields diverge", v.name);
            }
        }
        // Objects (SortSpec is shared with the query API and does not deny unknown fields).
        for o in &s.objects {
            if o.name == "SortSpec" {
                continue;
            }
            let probe = json!({"__nope__": 1});
            let msg = match o.name.as_str() {
                "Tab" => err_of::<crate::views::Tab>(probe),
                "SettingsSection" => err_of::<crate::views::SettingsSection>(probe),
                "SettingsItem" => err_of::<crate::views::SettingsItem>(probe),
                "Look" => err_of::<Look>(probe),
                "FormField" => err_of::<crate::views::FormField>(probe),
                "ProgramArg" => err_of::<crate::views::ProgramArg>(probe),
                "Column" => err_of::<crate::views::Column>(probe),
                other => panic!("test bug: object `{other}`"),
            };
            assert_eq!(
                set(o.fields.iter().map(|f| f.name.clone())),
                set(expected_in(&msg)),
                "object {}: fields diverge",
                o.name
            );
        }
        let msg = err_of::<ViewDesc>(json!({"__nope__": 1}));
        assert_eq!(
            set(s.view.fields.iter().map(|f| f.name.clone())),
            set(expected_in(&msg)),
            "ViewDesc: fields diverge"
        );
    }

    /// `(family, name, tagged, minimal, complete, fields)`.
    type Example<'a> = (
        String,
        String,
        bool,
        &'a Value,
        &'a Value,
        &'a [FieldSchema],
    );

    /// Every (family, schema entry) pair carrying examples.
    fn all_examples(s: &ViewSchema) -> Vec<Example<'_>> {
        let mut out = Vec::new();
        for family in [
            "node",
            "brick",
            "action",
            "click",
            "column_source",
            "filter",
        ] {
            for v in variants_of(s, family) {
                out.push((
                    family.to_string(),
                    v.name.clone(),
                    family != "filter",
                    &v.example,
                    &v.full_example,
                    v.fields.as_slice(),
                ));
            }
        }
        for o in &s.objects {
            out.push((
                o.name.clone(),
                o.name.clone(),
                false,
                &o.example,
                &o.full_example,
                o.fields.as_slice(),
            ));
        }
        out.push((
            "ViewDesc".into(),
            "ViewDesc".into(),
            false,
            &s.view.example,
            &s.view.full_example,
            s.view.fields.as_slice(),
        ));
        out
    }

    #[test]
    fn every_example_deserializes_and_the_complete_example_round_trips_unchanged() {
        let s = view_schema();
        for (family, name, _, min, full, fields) in all_examples(&s) {
            let m = round_trip(&family, min)
                .unwrap_or_else(|e| panic!("{family}/{name}: example refused: {e}"));
            assert_eq!(
                round_trip(&family, &m).as_ref(),
                Ok(&m),
                "{family}/{name}: minimal example is not stable"
            );
            let f = round_trip(&family, full)
                .unwrap_or_else(|e| panic!("{family}/{name}: full example refused: {e}"));
            assert_eq!(
                &f, full,
                "{family}/{name}: the complete example loses or alters a field"
            );
            // The complete example sets every described field (filters: single key).
            if family != "filter" {
                let keys: BTreeSet<String> = full
                    .as_object()
                    .map(|o| o.keys().cloned().collect())
                    .unwrap_or_default();
                let tagged = ["node", "brick", "action", "click", "column_source"]
                    .contains(&family.as_str());
                let described: BTreeSet<String> = fields
                    .iter()
                    .map(|f| f.name.clone())
                    .chain(tagged.then(|| match family.as_str() {
                        "action" | "click" => "kind".to_string(),
                        "column_source" => "op".to_string(),
                        _ => "type".to_string(),
                    }))
                    .collect();
                assert_eq!(keys, described, "{family}/{name}: full example key set");
            }
        }
    }

    #[test]
    fn required_flags_are_true_to_serde() {
        let s = view_schema();
        for (family, name, tagged, min, full, fields) in all_examples(&s) {
            if family == "filter" {
                continue;
            }
            let _ = tagged;
            for f in fields {
                if f.required {
                    let mut v = full.clone();
                    if let Some(o) = v.as_object_mut() {
                        o.remove(&f.name);
                    }
                    assert!(
                        round_trip(&family, &v).is_err(),
                        "{family}/{name}: `{}` is declared required but its absence is accepted",
                        f.name
                    );
                    assert!(
                        min.get(&f.name).is_some(),
                        "{family}/{name}: the minimal example omits required `{}`",
                        f.name
                    );
                } else {
                    let mut v = full.clone();
                    if let Some(o) = v.as_object_mut() {
                        o.remove(&f.name);
                    }
                    assert!(
                        round_trip(&family, &v).is_ok(),
                        "{family}/{name}: `{}` is declared optional but its absence is refused",
                        f.name
                    );
                    assert!(
                        min.get(&f.name).is_none(),
                        "{family}/{name}: the minimal example sets optional `{}`",
                        f.name
                    );
                }
            }
        }
    }

    #[test]
    fn declared_defaults_are_the_values_taken_when_absent() {
        let s = view_schema();
        let mut checked = 0;
        for (family, name, _, min, _, fields) in all_examples(&s) {
            for f in fields {
                let Some(d) = &f.default else { continue };
                let mut with = min.clone();
                if let Some(o) = with.as_object_mut() {
                    o.insert(f.name.clone(), d.clone());
                }
                let a = round_trip(&family, min).expect("min");
                let b = round_trip(&family, &with).expect("with default");
                // Parsed values are equal iff re-serialization is (skip_serializing_if hides
                // the default on both sides).
                assert_eq!(
                    a, b,
                    "{family}/{name}: `{}` default {d} is not what absence means",
                    f.name
                );
                checked += 1;
            }
        }
        assert!(checked >= 6, "the defaults were not exercised ({checked})");
        // An enumeration's own default agrees with the field default that names it.
        for (_, _, _, _, _, fields) in all_examples(&s) {
            for f in fields {
                let Some(en) = f.ty.strip_prefix("enum:") else {
                    continue;
                };
                if let Some(d) = &f.default {
                    let e = s.enums.iter().find(|e| e.name == en).expect("enum exists");
                    assert_eq!(
                        e.default.as_deref().map(Value::from),
                        Some(d.clone()),
                        "enum {en}: default differs from field `{}`",
                        f.name
                    );
                }
            }
        }
    }

    #[test]
    fn every_enum_value_round_trips_as_its_string() {
        fn rt<T: DeserializeOwned + Serialize>(v: &Value) -> Result<Value, String> {
            let t: T = serde_json::from_value(v.clone()).map_err(|e| e.to_string())?;
            serde_json::to_value(&t).map_err(|e| e.to_string())
        }
        let s = view_schema();
        let mut n = 0;
        for e in &s.enums {
            for val in &e.values {
                let v = json!(val.name);
                let back = match e.name.as_str() {
                    "Surface" => rt::<Surface>(&v),
                    "Accent" => rt::<Accent>(&v),
                    "Layout" => rt::<Layout>(&v),
                    "Transition" => rt::<Transition>(&v),
                    "Tone" => rt::<Tone>(&v),
                    "Signal" => rt::<Signal>(&v),
                    "Input" => rt::<Input>(&v),
                    "RelationMode" => rt::<RelationMode>(&v),
                    "ArgKind" => rt::<ArgKind>(&v),
                    "Run" => rt::<Run>(&v),
                    "AgentOp" => rt::<AgentOp>(&v),
                    "Grant" => rt::<Grant>(&v),
                    "HostSurface" => rt::<HostSurface>(&v),
                    "DeviceCapability" => rt::<DeviceCapability>(&v),
                    "SortDir" => rt::<crate::query::SortDir>(&v),
                    "FilterOp" => rt::<crate::query::FilterOp>(&v),
                    other => panic!("test bug: enum `{other}`"),
                };
                assert_eq!(back, Ok(v), "enum {}::{}", e.name, val.name);
                n += 1;
            }
        }
        let total: usize = s.enums.iter().map(|e| e.values.len()).sum();
        assert_eq!(n, total, "every enum value must be exercised");
        assert!(n >= 55, "too few enum values exercised ({n})");
    }

    // ---------- the schema agrees with the checker ----------

    #[test]
    fn every_brick_example_passes_check_and_the_header_flag_is_true() {
        let s = view_schema();
        for b in &s.bricks {
            for (label, ex) in [("example", &b.example), ("full_example", &b.full_example)] {
                let mut m = fixture();
                m.views.push(view_with_root(
                    "probe",
                    Surface::Main,
                    json!({"type": "leaf", "brick": ex}),
                ));
                assert_eq!(
                    check_codes(&m),
                    Vec::<String>::new(),
                    "brick {} {label} must pass check",
                    b.name
                );
            }
            let mut m = fixture();
            m.views.push(view_with_root(
                "probe",
                Surface::Header,
                json!({"type": "leaf", "brick": b.example}),
            ));
            let refused =
                check_codes(&m).contains(&"this.descriptor.surface_brick_refused".to_string());
            assert_eq!(
                refused, !b.header_ok,
                "brick {}: `header_ok` disagrees with check",
                b.name
            );
        }
        assert_eq!(
            set(s
                .bricks
                .iter()
                .filter(|b| b.header_ok)
                .map(|b| b.name.clone())),
            set(s.header_bricks.iter().cloned()),
            "header_bricks and the per-brick flags diverge"
        );
    }

    #[test]
    fn every_node_and_action_example_passes_check() {
        let s = view_schema();
        for n in &s.nodes {
            for ex in [&n.example, &n.full_example] {
                let mut m = fixture();
                m.views
                    .push(view_with_root("probe", Surface::Main, ex.clone()));
                assert_eq!(check_codes(&m), Vec::<String>::new(), "node {}", n.name);
            }
        }
        for a in &s.actions {
            for ex in [&a.example, &a.full_example] {
                let mut m = fixture();
                m.actions.clear();
                m.actions
                    .push(parse_action(&ex.to_string()).expect("action"));
                assert_eq!(check_codes(&m), Vec::<String>::new(), "action {}", a.name);
            }
        }
    }

    #[test]
    fn the_example_module_is_a_valid_module() {
        let s = view_schema();
        let m = ModuleDescriptor::from_json(&s.example_module.to_string())
            .unwrap_or_else(|e| panic!("example module refused: {e:?}"));
        assert_eq!(check_codes(&m), Vec::<String>::new());
        assert_eq!(m.views.len(), 1);
        assert_eq!(m.actions.len(), 1);
    }

    /// One mutation of the fixture per rule: the code it must provoke.
    fn rule_cases() -> Vec<(&'static str, ModuleDescriptor)> {
        fn leaf(brick: &str) -> Value {
            json!({"type": "leaf", "brick": serde_json::from_str::<Value>(brick).expect("json")})
        }
        fn with_view(root: Value) -> ModuleDescriptor {
            let mut m = fixture();
            m.views.push(view_with_root("probe", Surface::Main, root));
            m
        }
        fn header_view(root: Value) -> ModuleDescriptor {
            let mut m = fixture();
            m.views.push(view_with_root("probe", Surface::Header, root));
            m
        }
        fn with_header(brick: &str) -> ModuleDescriptor {
            let mut m = fixture();
            m.views
                .push(view_with_root("probe", Surface::Header, leaf(brick)));
            m
        }
        fn with_action(a: &str) -> ModuleDescriptor {
            let mut m = fixture();
            m.actions.push(parse_action(a).expect("action"));
            m
        }
        fn with_operations(ops: &[(&str, &str)]) -> ModuleDescriptor {
            let mut m = fixture();
            for (name, entity) in ops {
                m.operations
                    .push(crate::OperationDesc::new(*name).entity(*entity));
            }
            m
        }
        fn replacing(pairs: &[(&str, &str)]) -> ModuleDescriptor {
            let mut m = fixture();
            for (name, target) in pairs {
                let mut v =
                    view_with_root(name, Surface::Main, json!({"type":"stack","children":[]}));
                v.replaces = Some((*target).to_string());
                m.views.push(v);
            }
            m
        }
        let mut dup_view = fixture();
        dup_view.views.push(dup_view.views[0].clone());
        let mut deep = json!({"type": "stack", "children": []});
        for _ in 0..crate::views::MAX_VIEW_DEPTH + 1 {
            deep = json!({"type": "stack", "children": [deep]});
        }
        let mut self_replace = fixture();
        self_replace.views[0].replaces = Some("invoices".into());
        let mut twice = replacing(&[("a", "invoices"), ("b", "invoices")]);
        twice.views[0].replaces = None;
        vec![
            (
                "invalid_name",
                with_action(r#"{"kind":"delete","name":"Drop All","entity":"invoice"}"#),
            ),
            ("duplicate_view", dup_view),
            (
                "duplicate_action",
                with_action(r#"{"kind":"delete","name":"new","entity":"invoice"}"#),
            ),
            (
                "action_entity_unknown",
                with_action(r#"{"kind":"delete","name":"x","entity":"ghost"}"#),
            ),
            (
                "action_field_unknown",
                with_action(r#"{"kind":"update","name":"x","entity":"invoice","fields":["id"]}"#),
            ),
            (
                "action_fields_empty",
                with_action(r#"{"kind":"create","name":"x","entity":"invoice","fields":[]}"#),
            ),
            (
                "duplicate_action_field",
                with_action(
                    r#"{"kind":"create","name":"x","entity":"invoice","fields":["titre","titre"]}"#,
                ),
            ),
            (
                "view_target_unknown",
                with_action(r#"{"kind":"navigate","name":"x","view":"ghost"}"#),
            ),
            (
                "navigate_module_not_required",
                with_action(r#"{"kind":"navigate","name":"x","view":"v","module":"ghost"}"#),
            ),
            (
                "navigate_module_is_self",
                with_action(r#"{"kind":"navigate","name":"x","view":"v","module":"billing"}"#),
            ),
            (
                "action_secret_invalid",
                with_action(r#"{"kind":"request_secret","name":"x","secret":"a b"}"#),
            ),
            (
                "settings_empty",
                with_view(leaf(r#"{"type":"settings","sections":[]}"#)),
            ),
            (
                "settings_text_empty",
                with_view(leaf(
                    r#"{"type":"settings","sections":[{"title":" ","items":[{"label":"a","action":"go"}]}]}"#,
                )),
            ),
            (
                "settings_action_refused",
                with_view(leaf(
                    r#"{"type":"settings","sections":[{"title":"t","items":[{"label":"a","action":"drop"}]}]}"#,
                )),
            ),
            (
                "view_entity_unknown",
                with_view(leaf(r#"{"type":"list","entity":"ghost","label":"titre"}"#)),
            ),
            (
                "duplicate_operation",
                with_operations(&[("terminer", "invoice"), ("terminer", "invoice")]),
            ),
            (
                "operation_entity_unknown",
                with_operations(&[("envoyer", "ghost")]),
            ),
            (
                "action_operation_unknown",
                with_action(
                    r#"{"kind":"invoke","name":"x","operation":"ghost","entity":"invoice"}"#,
                ),
            ),
            (
                "view_entity_required",
                with_view(leaf(r#"{"type":"indicator","signal":"counter"}"#)),
            ),
            (
                "view_field_required",
                with_view(leaf(
                    r#"{"type":"indicator","signal":"badge","entity":"invoice"}"#,
                )),
            ),
            (
                "view_field_unknown",
                with_view(leaf(r#"{"type":"list","entity":"invoice","label":"nom"}"#)),
            ),
            (
                "view_action_unknown",
                with_view(leaf(
                    r#"{"type":"button","entity":"invoice","action":"ghost"}"#,
                )),
            ),
            (
                "view_action_kind_mismatch",
                with_view(leaf(
                    r#"{"type":"capability_request","capability":"camera","action":"mic_no"}"#,
                )),
            ),
            (
                "view_relation_unknown",
                with_view(leaf(
                    r#"{"type":"relation","entity":"conversation","link":"invoice_conversation","mode":"show"}"#,
                )),
            ),
            (
                "action_agent_text_missing",
                with_action(r#"{"kind":"agent_tool","name":"demander","op":"send_message"}"#),
            ),
            (
                "action_agent_text_unused",
                with_action(
                    r#"{"kind":"agent_tool","name":"halt","op":"stop_run","text":"arrete"}"#,
                ),
            ),
            (
                "field_input_invalid",
                with_view(leaf(
                    r#"{"type":"field","entity":"invoice","field":"titre","input":"reference"}"#,
                )),
            ),
            (
                "surface_brick_refused",
                with_header(r#"{"type":"chat","entity":"conversation"}"#),
            ),
            (
                "composer_placeholder_empty",
                with_view(leaf(r#"{"type":"composer","placeholder":"   "}"#)),
            ),
            (
                "duplicate_composer_action",
                with_view(leaf(r#"{"type":"composer","actions":["say","say"]}"#)),
            ),
            (
                "composer_action_refused",
                with_view(leaf(r#"{"type":"composer","actions":["edit"]}"#)),
            ),
            (
                "invalid_page_size",
                with_view(leaf(
                    r#"{"type":"list","entity":"invoice","label":"titre","page_size":101}"#,
                )),
            ),
            (
                "row_action_refused",
                with_view(leaf(
                    r#"{"type":"list","entity":"invoice","label":"titre","row_actions":["new"]}"#,
                )),
            ),
            (
                "duplicate_row_action",
                with_view(leaf(
                    r#"{"type":"list","entity":"invoice","label":"titre","row_actions":["edit","edit"]}"#,
                )),
            ),
            (
                "secret_name_empty",
                with_view(leaf(r#"{"type":"secret","name":""}"#)),
            ),
            (
                "secret_name_invalid",
                with_view(leaf(r#"{"type":"secret","name":"a b"}"#)),
            ),
            (
                "program_name_duplicate",
                with_view(leaf(
                    r#"{"type":"program","entity":"invoice","run":"manual","args":[{"name":"a","kind":"text"},{"name":"a","kind":"bool"}]}"#,
                )),
            ),
            (
                "duplicate_form_field",
                with_view(leaf(
                    r#"{"type":"form","entity":"invoice","fields":[{"field":"titre"},{"field":"titre"}]}"#,
                )),
            ),
            (
                "duplicate_tab",
                with_view(
                    json!({"type":"tabs","tabs":[{"label":"A","children":[]},{"label":"A","children":[]}]}),
                ),
            ),
            ("tabs_empty", with_view(json!({"type":"tabs","tabs":[]}))),
            (
                "label_empty",
                with_action(
                    r#"{"kind":"delete","name":"retirer","entity":"invoice","label":"  "}"#,
                ),
            ),
            (
                "label_not_text",
                with_view(leaf(
                    r#"{"type":"button","entity":"invoice","action":"new","label":"epingler_cle"}"#,
                )),
            ),
            (
                "notice_label_without_action",
                with_view(leaf(
                    r#"{"type":"notice","tone":"info","entity":"invoice","field":"titre","label":"Go"}"#,
                )),
            ),
            (
                "html_path_empty",
                with_view(json!({"type":"html","path":""})),
            ),
            (
                "html_path_outside_module",
                with_view(json!({"type":"html","path":"../../etc/passwd.html"})),
            ),
            (
                "html_extension_refused",
                with_view(json!({"type":"html","path":"ui/app.js"})),
            ),
            (
                "html_svg_refused",
                with_view(json!({"type":"html","path":"ui/logo.svg"})),
            ),
            (
                "html_in_header",
                header_view(json!({"type":"html","path":"ui/index.html"})),
            ),
            ("view_too_deep", with_view(deep)),
            ("view_replaces_self", self_replace),
            ("view_replaced_twice", twice),
            ("view_replaces_cycle", replacing(&[("a", "b"), ("b", "a")])),
        ]
    }

    #[test]
    fn every_documented_rule_is_provoked_by_one_case_and_no_other_code_is_emitted() {
        let s = view_schema();
        let cases = rule_cases();
        let mut provoked = BTreeSet::new();
        for (code, m) in &cases {
            let full = format!("this.descriptor.{code}");
            let got = check_codes(m);
            assert!(
                got.contains(&full),
                "case `{code}` must provoke it, got {got:?}"
            );
            for g in &got {
                let short = g.trim_start_matches("this.descriptor.");
                assert!(
                    s.rule(short).is_some(),
                    "check emitted `{g}`, which the schema does not document"
                );
            }
            provoked.insert((*code).to_string());
        }
        assert_eq!(
            provoked,
            set(s.rules.iter().map(|r| r.code.clone())),
            "a documented rule has no provoking case, or a case has no documented rule"
        );
        assert_eq!(provoked.len(), cases.len(), "two cases for one rule");
        for r in &s.rules {
            assert!(
                !r.summary.is_empty() && !r.fix.is_empty(),
                "rule {}",
                r.code
            );
        }
    }

    #[test]
    fn a_reason_literal_added_to_the_checker_must_be_documented() {
        // Cheap tripwire on the source: any string literal that looks like a view/action
        // reason must be a documented rule.
        let src = include_str!("views.rs");
        let body = src.split("#[cfg(test)]").next().unwrap_or(src);
        let s = view_schema();
        let prefixes = [
            "view_",
            "action_",
            "duplicate_",
            "invalid_",
            "field_",
            "program_",
            "surface_",
            "tabs_",
            "composer_",
            "settings_",
            "navigate_",
            "label_",
            "notice_",
        ];
        let mut seen = 0;
        for lit in body.split('"').skip(1).step_by(2) {
            let reason_like = lit.chars().all(|c| c.is_ascii_lowercase() || c == '_')
                && prefixes.iter().any(|p| lit.starts_with(p))
                && !lit.ends_with('_');
            if reason_like {
                seen += 1;
                assert!(
                    s.rule(lit).is_some(),
                    "`{lit}` is emitted but not documented"
                );
            }
        }
        assert!(seen >= 20, "the tripwire saw only {seen} reasons");
    }

    // ---------- coverage audit ----------

    #[test]
    fn coverage_points_at_types_that_exist_and_the_audited_needs_are_listed() {
        let s = view_schema();
        for c in &s.coverage {
            for by in &c.covered_by {
                let (kind, rest) = by.split_once(':').expect("kind:name");
                let name = rest.split('(').next().unwrap_or(rest);
                let found = match kind {
                    "brick" => s.brick(name).is_some(),
                    "node" => s.node(name).is_some(),
                    "action" => s.action(name).is_some(),
                    other => panic!("unknown coverage kind `{other}`"),
                };
                assert!(found, "coverage `{}` points at missing `{by}`", c.need);
            }
        }
        for need in [
            "list",
            "detail",
            "form",
            "chat",
            "metrics",
            "tabs",
            "sheet",
            "drawer",
            "button",
            "text",
            "secondary line",
            "banner",
            "indicator",
            "text field",
            "number field",
            "toggle field",
            "file field",
            "reference field",
            "relation",
            "tool step",
            "queue",
            "code block",
            "result",
            "capability request",
            "program",
        ] {
            let c = s.coverage.iter().find(|c| c.need == need);
            assert!(
                c.is_some_and(|c| !c.covered_by.is_empty()),
                "need `{need}` must be covered"
            );
        }
        assert!(
            s.coverage.iter().any(|c| c.covered_by.is_empty()),
            "known gaps must be listed, not hidden"
        );
    }

    // ---------- the document itself ----------

    #[test]
    fn the_schema_serializes_deterministically_and_round_trips() {
        let a = view_schema().to_json_pretty();
        let b = view_schema().to_json_pretty();
        assert_eq!(a, b, "two builds must be byte-identical");
        let back: ViewSchema = serde_json::from_str(&a).expect("schema parses back");
        assert_eq!(back, view_schema());
        assert_eq!(back.version, VIEW_SCHEMA_VERSION);
        assert!(a.contains("\"capability_request\"") && a.contains("\"tool_step\""));
    }

    // ---------- actionable parse errors ----------

    #[test]
    fn an_unknown_brick_lists_the_valid_ones_and_suggests_the_closest() {
        let e = parse_brick(r#"{"type":"lst","entity":"invoice","label":"titre"}"#)
            .expect_err("unknown brick");
        assert!(e.hint.contains("`lst`"), "{e}");
        assert!(e.hint.contains("Did you mean `list`?"), "{e}");
        assert!(
            e.hint.contains("capability_request"),
            "valid names listed: {e}"
        );
        assert!(e.hint.contains("view_schema()"), "{e}");
    }

    #[test]
    fn a_missing_and_an_unknown_field_are_named() {
        let e = parse_brick(r#"{"type":"list","entity":"invoice"}"#).expect_err("missing label");
        assert!(e.hint.contains("`label`"), "{e}");
        let e = parse_brick(r#"{"type":"text","entity":"invoice","field":"a","colour":"red"}"#)
            .expect_err("unknown field");
        assert!(
            e.hint.contains("`colour`") && e.hint.contains("valid:"),
            "{e}"
        );
        let e = parse_action(r#"{"kind":"delet","name":"d","entity":"invoice"}"#)
            .expect_err("unknown action");
        assert!(e.hint.contains("Did you mean `delete`?"), "{e}");
        let e = parse_node(r#"{"type":"grid","children":[]}"#).expect_err("unknown node");
        assert!(e.hint.contains("stack"), "{e}");
    }

    #[test]
    fn a_valid_view_parses_and_the_look_defaults_are_the_documented_ones() {
        let v = parse_view(r#"{"name":"x","root":{"type":"stack","children":[]}}"#)
            .expect("valid view");
        assert_eq!(v.surface, Surface::Main);
        assert_eq!(Look::default().accent, Accent::Neutral);
        assert_eq!(Look::default().layout, Layout::Stack);
        assert_eq!(Look::default().transition, Transition::None);
    }
}
