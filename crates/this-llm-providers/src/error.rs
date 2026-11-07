//! Every way a provider call or a configuration change can be refused, in one type.

use std::fmt;

use crate::budget::BudgetError;
use crate::config::ConfigError;
use crate::model::ParseError;
use crate::secret::SecretError;
use crate::store::StoreError;

#[derive(Debug)]
pub enum ProviderError {
    Config(ConfigError),
    Store(StoreError),
    Secret(SecretError),
    Budget(BudgetError),
    /// No provider with this identifier.
    NotFound(String),
    /// The provider exists but is switched off.
    Disabled(String),
    /// A remote provider resolved to no key.
    MissingSecret(String),
    /// The prompt and the reserved output do not fit the window.
    Context {
        needed: u32,
        window: u32,
    },
    /// The engine announces no model and none is configured.
    NoModel(String),
    /// The engine could not be reached.
    Transport(String),
    /// The engine did not answer in time.
    Timeout(String),
    /// A stream stopped without `[DONE]`, without a finish reason and without a tool call:
    /// what arrived is a fragment, not an answer.
    StreamCut,
    /// The engine answered with an error status.
    Status {
        code: u16,
        excerpt: String,
    },
    /// The engine answered with something that is not the expected shape.
    Parse(String),
    /// Images were joined that this request cannot carry (a provider declared without
    /// vision, a reference the host did not resolve, a malformed part). Refused before
    /// sending: an image is never dropped in silence.
    Images(String),
}

impl fmt::Display for ProviderError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Config(error) => write!(formatter, "{error}"),
            Self::Store(error) => write!(formatter, "{error}"),
            Self::Secret(error) => write!(formatter, "{error}"),
            Self::Budget(error) => write!(formatter, "{error}"),
            Self::NotFound(id) => write!(formatter, "aucun provider `{id}`"),
            Self::Disabled(id) => write!(formatter, "le provider `{id}` est désactivé"),
            Self::MissingSecret(id) => write!(
                formatter,
                "le provider `{id}` n'a pas de clé résolue ; vérifie sa référence de credential"
            ),
            Self::Context { needed, window } => write!(
                formatter,
                "la requête demande {needed} tokens (entrée + sortie réservée), la fenêtre en fait {window}"
            ),
            Self::NoModel(id) => write!(
                formatter,
                "`{id}` n'annonce aucun modèle et aucun n'est configuré"
            ),
            Self::Transport(reason) => write!(formatter, "moteur injoignable : {reason}"),
            Self::Timeout(reason) => {
                write!(formatter, "le moteur n'a pas répondu à temps : {reason}")
            }
            Self::StreamCut => formatter.write_str(
                "flux coupé avant la fin : ni [DONE] ni finish_reason, et aucun appel d'outil",
            ),
            Self::Status { code, excerpt } => {
                write!(formatter, "le moteur a répondu {code} : {excerpt}")
            }
            Self::Parse(reason) => write!(formatter, "réponse inattendue : {reason}"),
            Self::Images(reason) => write!(formatter, "images refusées : {reason}"),
        }
    }
}

impl std::error::Error for ProviderError {}

/// The family of a failure. A host switches on it; a message is for reading.
///
/// The names are stable and match the transport failure codes of a harness
/// (`unreachable`, `http`, `truncated`, `unreadable`, `timeout`, `provider`), so an app can
/// hand them over without translating.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorKind {
    /// The engine did not answer at all (DNS, refused connection).
    Unreachable,
    /// The engine answered with an error status.
    Http(u16),
    /// The stream stopped without saying it was done.
    Truncated,
    /// The answer does not read.
    Unreadable,
    /// The engine did not answer in time.
    Timeout,
    /// The provider's own rules refused the call: a budget, a context window, a missing
    /// key, an invalid configuration. Nothing was sent to the engine.
    Provider,
}

impl ErrorKind {
    /// The stable code, as a harness names it.
    pub fn code(self) -> &'static str {
        match self {
            Self::Unreachable => "unreachable",
            Self::Http(_) => "http",
            Self::Truncated => "truncated",
            Self::Unreadable => "unreadable",
            Self::Timeout => "timeout",
            Self::Provider => "provider",
        }
    }

    /// The HTTP status, when there is one.
    pub fn status(self) -> Option<u16> {
        match self {
            Self::Http(status) => Some(status),
            _ => None,
        }
    }
}

impl ProviderError {
    /// The family of this failure.
    pub fn kind(&self) -> ErrorKind {
        match self {
            Self::Transport(_) => ErrorKind::Unreachable,
            Self::Timeout(_) => ErrorKind::Timeout,
            Self::Status { code, .. } => ErrorKind::Http(*code),
            Self::StreamCut => ErrorKind::Truncated,
            Self::Parse(_) => ErrorKind::Unreadable,
            Self::Config(_)
            | Self::Store(_)
            | Self::Secret(_)
            | Self::Budget(_)
            | Self::NotFound(_)
            | Self::Disabled(_)
            | Self::MissingSecret(_)
            | Self::Context { .. }
            | Self::NoModel(_)
            | Self::Images(_) => ErrorKind::Provider,
        }
    }
}

impl From<ConfigError> for ProviderError {
    fn from(error: ConfigError) -> Self {
        Self::Config(error)
    }
}

impl From<StoreError> for ProviderError {
    fn from(error: StoreError) -> Self {
        Self::Store(error)
    }
}

impl From<SecretError> for ProviderError {
    fn from(error: SecretError) -> Self {
        Self::Secret(error)
    }
}

impl From<BudgetError> for ProviderError {
    fn from(error: BudgetError) -> Self {
        Self::Budget(error)
    }
}

impl From<ParseError> for ProviderError {
    fn from(error: ParseError) -> Self {
        Self::Parse(error.to_string())
    }
}
