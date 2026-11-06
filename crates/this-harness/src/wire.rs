//! Ce qui traverse le fil : les messages, les appels d'outils, la réponse.
//!
//! Rien ici ne connaît de protocole. `Message` est la forme que le harnais
//! manipule ; c'est le [`Transport`](crate::Transport) de l'application qui
//! la traduit dans le dialecte de son moteur (OpenAI, CLI, autre). Un harnais
//! qui parlerait directement l'API d'un fournisseur serait un harnais qui
//! change de fournisseur en même temps que de version.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Un message de la conversation, tel que le modèle le lira.
///
/// La variante `Tool` porte l'identifiant de l'appel qu'elle répond : sans lui,
/// un moteur refuse la conversation, et deux outils appelés dans le même tour
/// deviennent indiscernables.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "role", rename_all = "snake_case")]
pub enum Message {
    System {
        content: String,
    },
    User {
        content: String,
    },
    Assistant {
        #[serde(default)]
        content: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        calls: Vec<ToolCall>,
    },
    Tool {
        id: String,
        content: String,
    },
}

impl Message {
    pub fn system(content: impl Into<String>) -> Self {
        Self::System {
            content: content.into(),
        }
    }
    pub fn user(content: impl Into<String>) -> Self {
        Self::User {
            content: content.into(),
        }
    }
    pub fn assistant(content: impl Into<String>) -> Self {
        Self::Assistant {
            content: content.into(),
            calls: Vec::new(),
        }
    }
    pub fn tool(id: impl Into<String>, content: impl Into<String>) -> Self {
        Self::Tool {
            id: id.into(),
            content: content.into(),
        }
    }
    /// Le texte visible d'un message, quel qu'il soit.
    pub fn text(&self) -> &str {
        match self {
            Self::System { content } | Self::User { content } | Self::Tool { content, .. } => {
                content
            }
            Self::Assistant { content, .. } => content,
        }
    }
    pub fn is_user(&self) -> bool {
        matches!(self, Self::User { .. })
    }
}

/// Un outil demandé par le modèle.
#[derive(Clone, Debug, PartialEq, Default, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    /// Déjà analysé : un argument mal formé est un refus, pas une panique au
    /// fond de l'exécution.
    #[serde(default)]
    pub arguments: Value,
}

/// Un outil tel qu'on le présente au modèle.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ToolSpec {
    pub name: String,
    pub description: String,
    /// Un JSON Schema (`{"type":"object","properties":{...}}`).
    #[serde(default)]
    pub parameters: Value,
}

/// Une demande de complétion : tout ce qu'il faut pour un tour, et rien de plus.
///
/// Les champs sont privés **exprès**. Un transport qui ne lit que `messages`
/// oublie la consigne système — c'est exactement le bug qui a laissé l'Atelier
/// sans brief pendant une version entière : le modèle recevait la conversation
/// sans savoir quels outils il avait, ni qu'il devait agir au lieu de décrire.
/// Ici, le seul accès au fil est [`Request::thread`], qui met la consigne en
/// tête. L'oubli n'est plus possible : il n'y a pas d'autre porte.
#[derive(Clone, Debug, PartialEq)]
pub struct Request {
    system: String,
    messages: Vec<Message>,
    tools: Vec<ToolSpec>,
}

impl Request {
    pub(crate) fn new(system: String, messages: Vec<Message>, tools: Vec<ToolSpec>) -> Self {
        Self {
            system,
            messages,
            tools,
        }
    }

    /// Ajoute le contexte éphémère d'un tour : un `Message::system` juste avant
    /// le dernier message utilisateur — là où le modèle le lira comme « ce qui
    /// vaut pour cette demande ». Sans message utilisateur, il va en fin de fil.
    /// `None` ou un texte vide : la requête est rendue intacte.
    pub(crate) fn with_context(mut self, context: Option<String>) -> Self {
        let Some(context) = context.filter(|c| !c.trim().is_empty()) else {
            return self;
        };
        let at = self
            .messages
            .iter()
            .rposition(Message::is_user)
            .unwrap_or(self.messages.len());
        self.messages.insert(at, Message::system(context));
        self
    }

    /// Le fil tel que le modèle doit le lire : la consigne système, puis les
    /// messages. C'est la seule façon d'obtenir les messages.
    pub fn thread(&self) -> Vec<Message> {
        let mut all = Vec::with_capacity(self.messages.len() + 1);
        if !self.system.trim().is_empty() {
            all.push(Message::system(&self.system));
        }
        all.extend(self.messages.iter().cloned());
        all
    }

    pub fn system(&self) -> &str {
        &self.system
    }

    pub fn tools(&self) -> &[ToolSpec] {
        &self.tools
    }

    /// Rien à envoyer : pas même un message utilisateur.
    pub fn is_empty(&self) -> bool {
        self.messages.is_empty()
    }
}

/// Pourquoi le moteur a cessé de parler.
///
/// La distinction n'est pas décorative : `Length` veut dire « la réponse est
/// coupée, elle n'a pas conclu », et la traiter comme une fin fait croire à
/// l'utilisateur que le travail est terminé alors qu'il s'est arrêté au milieu
/// d'une phrase.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Finish {
    /// Le moteur n'a rien dit de sa fin — un flux coupé ressemble à ça.
    #[default]
    Open,
    /// Fin normale.
    Stop,
    /// Budget de tokens atteint : la réponse est tronquée.
    Length,
    /// Le modèle demande des outils et attend leurs résultats.
    Tools,
    /// Le transport a interrompu le flux (annulation).
    Aborted,
}

/// Ce que le moteur a répondu pour un tour.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Reply {
    pub text: String,
    pub calls: Vec<ToolCall>,
    pub finish: Finish,
}

/// La famille d'une panne de transport. Un code se commute, un message se lit.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Code {
    /// Le moteur n'a pas répondu du tout (DNS, refus de connexion).
    Unreachable,
    /// Le moteur a répondu, mais en erreur.
    Http,
    /// Le flux s'est arrêté sans dire qu'il était fini.
    Truncated,
    /// La réponse ne se lit pas.
    Unreadable,
    /// Le délai est dépassé.
    Timeout,
    /// L'appel a été annulé par l'utilisateur.
    Aborted,
    /// Un refus raconté par le fournisseur lui-même.
    Provider,
}

/// Une panne de transport, avec sa famille et sa phrase.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Failure {
    pub code: Code,
    pub message: String,
    /// Le statut HTTP, quand il y en a un.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<u16>,
}

impl Failure {
    pub fn new(code: Code, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            status: None,
        }
    }
    pub fn http(status: u16) -> Self {
        Self {
            code: Code::Http,
            message: format!("le moteur a répondu {status}"),
            status: Some(status),
        }
    }
    pub fn unreachable(message: impl Into<String>) -> Self {
        Self::new(Code::Unreachable, message)
    }
    pub fn truncated(said: &str) -> Self {
        Self::new(
            Code::Truncated,
            format!("le flux s'est arrêté sans fin (ni [DONE] ni finish_reason). Reçu :\n{said}"),
        )
    }
    pub fn aborted() -> Self {
        Self::new(Code::Aborted, "appel annulé")
    }
    /// Vrai quand l'utilisateur a demandé l'arrêt : ce n'est pas un échec.
    pub fn is_aborted(&self) -> bool {
        self.code == Code::Aborted
    }
}

impl std::fmt::Display for Failure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for Failure {}
