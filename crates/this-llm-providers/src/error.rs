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
    /// The engine answered with an error status.
    Status {
        code: u16,
        excerpt: String,
    },
    /// The engine answered with something that is not the expected shape.
    Parse(String),
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
            Self::Status { code, excerpt } => {
                write!(formatter, "le moteur a répondu {code} : {excerpt}")
            }
            Self::Parse(reason) => write!(formatter, "réponse inattendue : {reason}"),
        }
    }
}

impl std::error::Error for ProviderError {}

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
