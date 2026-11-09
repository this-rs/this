//! # The intent: what a click asks the host to do, never what it does
//!
//! `this-ui` draws the view contract and emits [`Intent`] values. It performs no call of its
//! own: no HTTP, no Tauri command, no storage write. The same rendering therefore serves a
//! server-backed app, a Tauri shell and a test, which differ only by who answers the intent.
//!
//! Every [`Action`](this_descriptor::views::Action) a module declares maps to exactly one
//! intent kind. A brick may also ask for something the action vocabulary does not cover
//! (running a program, sending a chat message, adding a link): it then emits a named intent
//! that the host must answer itself.

use this_descriptor::views::{AgentOp, DeviceCapability, Grant, HostSurface};

/// A value read from a control, on its way to the host.
#[derive(Clone, Debug, PartialEq)]
pub enum FieldValue {
    /// A single-line or multi-line text.
    Text(String),
    /// A toggle.
    Flag(bool),
    /// The name of a file the person chose. The bytes do not cross the boundary: they stay
    /// in the `<input type="file">` the host owns, under the field name carried here.
    File(String),
    /// Nothing was entered.
    Empty,
}

impl FieldValue {
    /// A text value.
    pub fn text(value: impl Into<String>) -> Self {
        FieldValue::Text(value.into())
    }

    /// The text, when there is one.
    pub fn as_text(&self) -> Option<&str> {
        match self {
            FieldValue::Text(text) | FieldValue::File(text) => Some(text),
            FieldValue::Flag(_) | FieldValue::Empty => None,
        }
    }

    /// Nobody entered anything.
    pub fn is_empty(&self) -> bool {
        match self {
            FieldValue::Text(text) | FieldValue::File(text) => text.is_empty(),
            FieldValue::Flag(_) => false,
            FieldValue::Empty => true,
        }
    }
}

/// The values of a form, in the declaration order of its fields.
pub type Values = Vec<(String, FieldValue)>;

/// Which way a relation edit goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LinkOp {
    Add,
    Remove,
}

/// What the surface asks the host to do. The host decides, executes and reports back.
#[derive(Clone, Debug, PartialEq)]
pub enum Intent {
    /// A declared `create` action: create a row of `entity` with `fields`.
    Create {
        action: String,
        entity: String,
        fields: Vec<String>,
        values: Values,
    },
    /// A declared `update` action on the row `id` (known when the brick targets one row).
    Update {
        action: String,
        entity: String,
        fields: Vec<String>,
        id: Option<String>,
        values: Values,
    },
    /// A declared `delete` action.
    Delete {
        action: String,
        entity: String,
        id: Option<String>,
    },
    /// Show another view of the module. `action` is `Some` when a declared `navigate`
    /// action was clicked, `None` when a list row carried `click: navigate`.
    ///
    /// `module` is `Some` when the action names ANOTHER module's view: the host resolves it
    /// at run time (the contract could not check it) and refuses with a message when that
    /// module is not installed or has no such view.
    Navigate {
        view: String,
        action: Option<String>,
        module: Option<String>,
    },
    /// A declared `request_secret` action: the host collects the secret `secret`. It carries
    /// a NAME and never a value — nothing typed in the host's secure input comes back here.
    Secret { action: String, secret: String },
    /// A declared agent tool: send a message, stop a run.
    Tool {
        action: String,
        op: AgentOp,
        entity: Option<String>,
        /// Ce que le module demande à l'agent. `None` pour un arrêt.
        text: Option<String>,
    },
    /// A declared device capability decision.
    Capability {
        action: String,
        capability: DeviceCapability,
        grant: Grant,
    },
    /// A list row was selected, without leaving the view.
    Select { entity: String, id: String },
    /// A list row asked for its detail.
    Open { entity: String, id: String },
    /// A single field was committed (blur or enter) — the host may keep it as a draft.
    Edit {
        entity: String,
        id: Option<String>,
        field: String,
        value: FieldValue,
    },
    /// The chat composer was submitted. No action of the contract covers it: the host maps
    /// it onto the module's own send-message action.
    Send { entity: String, text: String },
    /// The person answered a question the agent asked and is waiting on.
    ///
    /// No action of the contract covers it: the question is not a row of the module, it is
    /// the agent's gateway (`id`), and the answer is what the person typed or chose. The
    /// host hands it back to that gateway; the module is not called at all.
    Answer {
        /// The entity the card reads, for the host's logs.
        entity: String,
        /// The gateway's identifier — the `id` of the row the card drew.
        id: String,
        /// What the person answered.
        choice: String,
    },
    /// A relation was edited.
    Link {
        entity: String,
        id: Option<String>,
        link: String,
        target: Option<String>,
        op: LinkOp,
    },
    /// A program was asked to run, with its typed arguments.
    Run { entity: String, args: Values },
    /// The host must fetch (or retry) the rows of `entity`.
    Reload { entity: String },
    /// The person asks for access to `entity`: the host shows its permission flow.
    RequestAccess { entity: String },
    /// The person dismissed a modal.
    Dismiss { view: String },
    /// Une surface que l'hôte garde : le registre des modules, le choix du modèle, l'écran
    /// de réglages. Le module l'OFFRE (un bouton de son menu) ; l'hôte seul sait l'ouvrir.
    Host {
        action: String,
        surface: HostSurface,
    },
    /// Une opération nommée du module (`operations` du descripteur), ou une action que
    /// cette surface n'a pas su classer.
    ///
    /// C'est le SEUL intent qui atteint le code d'un module compilé. Il porte donc de quoi
    /// l'appeler : le nom de l'opération, la ligne visée quand le bouton est sur une ligne,
    /// et les valeurs d'un formulaire éventuel. Sans `id`, un bouton « terminer » posé sur
    /// une ligne ne pourrait pas dire laquelle — mesuré : l'opération refusait alors faute
    /// d'identifiant, et l'interface semblait morte.
    Invoke {
        action: String,
        /// Nom de l'opération déclarée ; à défaut, celui de l'action.
        operation: String,
        entity: Option<String>,
        /// Ligne visée, quand la brique appartient à une ligne.
        id: Option<String>,
        values: Values,
    },
}

impl Intent {
    /// The entity the intent concerns, when it concerns one.
    pub fn entity(&self) -> Option<&str> {
        match self {
            Intent::Create { entity, .. }
            | Intent::Update { entity, .. }
            | Intent::Delete { entity, .. }
            | Intent::Select { entity, .. }
            | Intent::Open { entity, .. }
            | Intent::Edit { entity, .. }
            | Intent::Send { entity, .. }
            | Intent::Link { entity, .. }
            | Intent::Run { entity, .. }
            | Intent::Reload { entity }
            | Intent::Answer { entity, .. }
            | Intent::RequestAccess { entity } => Some(entity),
            Intent::Tool { entity, .. } | Intent::Invoke { entity, .. } => entity.as_deref(),
            Intent::Navigate { .. }
            | Intent::Capability { .. }
            | Intent::Secret { .. }
            | Intent::Host { .. }
            | Intent::Dismiss { .. } => None,
        }
    }

    /// The declared action the intent came from, when it came from one.
    pub fn action(&self) -> Option<&str> {
        match self {
            Intent::Create { action, .. }
            | Intent::Update { action, .. }
            | Intent::Delete { action, .. }
            | Intent::Tool { action, .. }
            | Intent::Capability { action, .. }
            | Intent::Secret { action, .. }
            | Intent::Host { action, .. }
            | Intent::Invoke { action, .. } => Some(action),
            Intent::Navigate { action, .. } => action.as_deref(),
            _ => None,
        }
    }

    /// Short name of the intent kind, for logs and diagnostics.
    pub fn kind(&self) -> &'static str {
        match self {
            Intent::Create { .. } => "create",
            Intent::Update { .. } => "update",
            Intent::Delete { .. } => "delete",
            Intent::Navigate { .. } => "navigate",
            Intent::Secret { .. } => "secret",
            Intent::Tool { .. } => "tool",
            Intent::Capability { .. } => "capability",
            Intent::Select { .. } => "select",
            Intent::Open { .. } => "open",
            Intent::Edit { .. } => "edit",
            Intent::Send { .. } => "send",
            Intent::Answer { .. } => "answer",
            Intent::Link { .. } => "link",
            Intent::Run { .. } => "run",
            Intent::Reload { .. } => "reload",
            Intent::RequestAccess { .. } => "request_access",
            Intent::Dismiss { .. } => "dismiss",
            Intent::Host { .. } => "host",
            Intent::Invoke { .. } => "invoke",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_text_value_reports_its_text_and_emptiness() {
        assert_eq!(FieldValue::text("abc").as_text(), Some("abc"));
        assert!(!FieldValue::text("abc").is_empty());
        assert!(FieldValue::text("").is_empty());
        assert!(FieldValue::Empty.is_empty());
        assert_eq!(FieldValue::Flag(true).as_text(), None);
        assert!(!FieldValue::Flag(false).is_empty());
    }

    #[test]
    fn an_intent_names_its_entity_and_its_action() {
        let create = Intent::Create {
            action: "create_invoice".into(),
            entity: "invoice".into(),
            fields: vec!["amount".into()],
            values: vec![],
        };
        assert_eq!(create.entity(), Some("invoice"));
        assert_eq!(create.action(), Some("create_invoice"));
        assert_eq!(create.kind(), "create");

        let navigation = Intent::Navigate {
            view: "detail".into(),
            action: None,
            module: None,
        };
        assert_eq!(navigation.entity(), None);
        assert_eq!(navigation.action(), None);

        let reload = Intent::Reload {
            entity: "invoice".into(),
        };
        assert_eq!(reload.action(), None);
        assert_eq!(reload.entity(), Some("invoice"));
        assert_eq!(reload.kind(), "reload");
    }
}
