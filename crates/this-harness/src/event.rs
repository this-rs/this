//! Ce que le harnais raconte pendant qu'il travaille.
//!
//! Un harnais qui ne rend que le texte final oblige l'interface à deviner :
//! elle ne sait pas si le modèle réfléchit, s'il attend un outil, ou s'il a
//! fini. Ces événements sont la seule chose que l'interface doit savoir lire —
//! et ils suffisent : chaque bulle, chaque étape, chaque état affiché en
//! dérive.
//!
//! Les noms suivent Project Orchestrator (`stream_delta`, `tool_use`,
//! `tool_result`, `result`, `pending_queue`) : deux harnais qui nomment
//! différemment la même chose se traduisent une fois de trop.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use this_agent::Stop;

/// Un message écrit par l'utilisateur pendant qu'un tour tourne.
///
/// Il est **tenu**, pas perdu : il part à la frontière du tour suivant. C'est
/// la moitié de la bidirectionnalité ; l'autre moitié est `interrupt`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Pending {
    pub id: u64,
    pub text: String,
    /// Date de mise en file, en millisecondes epoch.
    pub at_ms: f64,
    /// Les images jointes : elles attendent avec le texte, et partent avec lui.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub images: Vec<crate::wire::ImagePart>,
}

/// Ce qui sort du harnais, dans l'ordre où ça arrive.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Event {
    /// Un message tenu vient d'entrer dans la conversation.
    User { text: String },
    /// La file d'attente, en entier, à chaque changement.
    ///
    /// Toujours la liste complète, jamais un delta : une interface qui reçoit
    /// des diffs doit les rejouer, et se trompe dès qu'un message est déplacé.
    Pending {
        messages: Vec<crate::event::Pending>,
    },
    /// Une consigne interne du harnais (relance après un refus, après un
    /// silence). Ce n'est pas l'utilisateur qui parle : l'interface la rend
    /// discrètement, ou pas du tout.
    Hint { text: String },
    /// Un tour commence.
    Turn { index: u32 },
    /// Un morceau de texte du modèle. S'accumule dans le tour courant.
    Delta { text: String },
    /// Le modèle demande un outil.
    ToolUse {
        id: String,
        tool: String,
        input: Value,
    },
    /// L'outil a répondu.
    ToolResult {
        id: String,
        tool: String,
        text: String,
        /// L'outil a refusé — ce n'est pas la même chose qu'une erreur.
        refused: bool,
        /// L'outil a échoué (il a planté, ou n'a pas pu faire ce qu'on lui demandait). Distinct
        /// d'un refus : une décision se contourne, une erreur se corrige.
        #[serde(default)]
        failed: bool,
        /// L'état n'a pas bougé : une action qui ne change rien n'est pas un
        /// progrès, et la boucle doit le voir.
        unchanged: bool,
        ms: u64,
    },
    /// Le texte définitif du tour (il remplace les deltas accumulés).
    Spoke { text: String },
    /// Le run s'est arrêté, et il dit pourquoi.
    Stopped {
        stop: Stop,
        /// Le dernier texte que le modèle a écrit.
        text: String,
        turns: u32,
        ms: u64,
        /// Code de panne du transport, quand l'arrêt vient d'une panne.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        code: Option<String>,
    },
}

impl Event {
    /// Un nom stable, pour les journaux et les tests.
    pub fn name(&self) -> &'static str {
        match self {
            Self::User { .. } => "user",
            Self::Pending { .. } => "pending",
            Self::Hint { .. } => "hint",
            Self::Turn { .. } => "turn",
            Self::Delta { .. } => "delta",
            Self::ToolUse { .. } => "tool_use",
            Self::ToolResult { .. } => "tool_result",
            Self::Spoke { .. } => "spoke",
            Self::Stopped { .. } => "stopped",
        }
    }
}

/// L'état du harnais, sérialisable : ce qu'il faut pour reprendre après une
/// fermeture d'app.
///
/// La file d'attente en fait partie : un message tapé juste avant que
/// l'application soit suspendue ne doit pas disparaître avec la page.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Snapshot {
    #[serde(default = "one")]
    pub version: u32,
    #[serde(default)]
    pub transcript: Vec<crate::wire::Message>,
    #[serde(default)]
    pub pending: Vec<Pending>,
}

fn one() -> u32 {
    1
}

impl Snapshot {
    pub fn empty() -> Self {
        Self::default()
    }
}

/// Ce qu'un appel d'outil a produit, vu par le harnais.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Outcome {
    /// Ce que le modèle lira.
    pub text: String,
    /// Le motif, si l'outil a refusé.
    pub refused: Option<String>,
    /// La cause, si l'outil a échoué.
    pub failed: Option<String>,
    /// Empreinte de l'état avant l'appel — vide quand l'outil ne touche rien.
    pub before: String,
    /// Empreinte de l'état après l'appel.
    pub after: String,
}

impl Outcome {
    pub fn ok(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            ..Self::default()
        }
    }
    pub fn refused(text: impl Into<String>, motif: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            refused: Some(motif.into()),
            ..Self::default()
        }
    }
    /// L'outil a échoué : le texte est ce que lira le modèle, `cause` ce que le harnais relance.
    pub fn failed(text: impl Into<String>, cause: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            failed: Some(cause.into()),
            ..Self::default()
        }
    }
    /// Un outil qui écrit observe son propre effet : c'est la seule mesure
    /// honnête de « ça a marché ».
    pub fn observed(mut self, before: impl Into<String>, after: impl Into<String>) -> Self {
        self.before = before.into();
        self.after = after.into();
        self
    }
    /// Vrai si l'appel a été observé **et** n'a rien changé.
    pub fn unchanged(&self) -> bool {
        !self.before.is_empty() && self.before == self.after
    }
}
