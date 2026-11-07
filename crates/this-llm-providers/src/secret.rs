//! Where a credential's value is read, at the moment a client needs it.

use std::fmt;

use this_llm::CredentialRef;

/// A secret value. No `Display`, no `Serialize`, and a `Debug` that prints nothing of
/// the value. It is held by a client for as long as the client lives, and nowhere else.
#[derive(Clone, PartialEq, Eq)]
pub struct Secret(String);

impl Secret {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// The value, for the one place that must put it on the wire.
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("Secret(***)")
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SecretError {
    /// The reference resolves to nothing: the variable or the vault entry is absent.
    Missing(String),
    /// The reference cannot be resolved here (no vault is plugged in on this host).
    Unavailable(String),
}

impl fmt::Display for SecretError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Missing(reference) => write!(
                formatter,
                "credential introuvable : {reference} (absent ou vide)"
            ),
            Self::Unavailable(reference) => {
                write!(formatter, "credential non résolvable ici : {reference}")
            }
        }
    }
}

impl std::error::Error for SecretError {}

/// Turns a reference into a value. Implemented by the host, which owns the vault.
pub trait SecretResolver: Send + Sync {
    /// `Ok(None)` for [`CredentialRef::None`]; the value otherwise, or why not.
    fn resolve(&self, credential: &CredentialRef) -> Result<Option<Secret>, SecretError>;
}

type VaultLookup = Box<dyn Fn(&str) -> Option<String> + Send + Sync>;

/// The stock resolver: environment variables, and a vault if the host plugs one in.
pub struct Secrets {
    vault: Option<VaultLookup>,
}

impl Secrets {
    /// Resolves `env:` references only. A `vault:` reference is reported unavailable,
    /// never silently ignored.
    pub fn env_only() -> Self {
        Self { vault: None }
    }

    /// Also resolves `vault:` references through `lookup`.
    pub fn with_vault(lookup: impl Fn(&str) -> Option<String> + Send + Sync + 'static) -> Self {
        Self {
            vault: Some(Box::new(lookup)),
        }
    }
}

impl SecretResolver for Secrets {
    fn resolve(&self, credential: &CredentialRef) -> Result<Option<Secret>, SecretError> {
        match credential {
            CredentialRef::None => Ok(None),
            CredentialRef::Env(name) => std::env::var(name)
                .ok()
                .filter(|value| !value.is_empty())
                .map(|value| Some(Secret::new(value)))
                .ok_or_else(|| SecretError::Missing(format!("env:{name}"))),
            CredentialRef::Vault(name) => match &self.vault {
                None => Err(SecretError::Unavailable(format!("vault:{name}"))),
                Some(lookup) => lookup(name)
                    .filter(|value| !value.is_empty())
                    .map(|value| Some(Secret::new(value)))
                    .ok_or_else(|| SecretError::Missing(format!("vault:{name}"))),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_secret_never_prints_its_value() {
        let secret = Secret::new("sk-very-secret");
        let shown = format!("{secret:?}");
        assert_eq!(shown, "Secret(***)");
        assert!(!shown.contains("sk-"));
    }

    #[test]
    fn no_credential_resolves_to_no_secret() {
        assert_eq!(Secrets::env_only().resolve(&CredentialRef::None), Ok(None));
    }

    #[test]
    fn a_vault_reference_without_a_vault_is_reported_not_ignored() {
        assert_eq!(
            Secrets::env_only().resolve(&CredentialRef::Vault("openai".into())),
            Err(SecretError::Unavailable("vault:openai".into()))
        );
    }

    #[test]
    fn a_plugged_vault_is_consulted_and_an_empty_entry_is_missing() {
        let secrets = Secrets::with_vault(|name| match name {
            "openai" => Some("sk-test".to_string()),
            "empty" => Some(String::new()),
            _ => None,
        });
        let found = secrets
            .resolve(&CredentialRef::Vault("openai".into()))
            .unwrap()
            .unwrap();
        assert_eq!(found.expose(), "sk-test");
        assert_eq!(
            secrets.resolve(&CredentialRef::Vault("empty".into())),
            Err(SecretError::Missing("vault:empty".into()))
        );
    }

    #[test]
    fn a_missing_environment_variable_is_named_by_its_reference() {
        assert_eq!(
            Secrets::env_only().resolve(&CredentialRef::Env("THIS_VAR_IS_NOT_SET_42".into())),
            Err(SecretError::Missing("env:THIS_VAR_IS_NOT_SET_42".into()))
        );
    }
}
