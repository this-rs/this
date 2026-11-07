//! What a this-rs app needs to know about a model, and how it asks one.
//!
//! ## What belongs here, and what does not
//!
//! A provider is a stable fact: a base URL, a model, a way to name a credential.
//! A transport is not: `reqwest` on a server, `fetch` in a page, a command on a
//! phone. So this crate **describes and does not transport**. Every app writes its
//! own transport against [`Completion`] — one contract, three implementations,
//! instead of three apps inventing three endpoints.
//!
//! ## The rule that is not negotiable
//!
//! A credential is a **reference** ([`CredentialRef`]): `env:VAR`, `vault:name`, or
//! `none`. Never a value. A provider description travels — it is logged, shown in a
//! settings screen, sent to a UI — and a key written in it would leak everywhere it
//! goes. [`CredentialRef::parse`] refuses anything that is not a reference, so the
//! mistake is caught where it is made.
//!
//! ## Local first
//!
//! A local model needs no key and no outbound network. [`Preset`] carries the base
//! URL each one listens on, so no app has to know that Ollama answers on
//! `localhost:11434/v1` — and replacing the model stays a change of preset.

use std::fmt;

mod image;

pub use image::{ImagePart, ImageSource};

/// How a request reaches a model.
///
/// Asynchronous, and `Send` only off `wasm32`: a browser completion goes through a
/// `!Send` future, while a server's handlers require `Send`. The trait says the
/// truth per target rather than imposing one side's constraint on the other.
#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
pub trait Completion {
    /// `system` describes the task; `request` is what the caller asked for.
    async fn complete(&self, system: &str, request: &str) -> Result<String, String>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CredentialError {
    /// Something that looks like a secret value, not a way to find one.
    RawValue,
    /// A reference with an empty name.
    Empty,
}

impl fmt::Display for CredentialError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::RawValue => formatter.write_str(
                "un credential est une RÉFÉRENCE (`env:VAR`, `vault:nom`, `none`), jamais une valeur",
            ),
            Self::Empty => formatter.write_str("référence de credential vide"),
        }
    }
}

impl std::error::Error for CredentialError {}

/// Where to find a credential — never the credential.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CredentialRef {
    /// No credential: a local model needs none.
    None,
    /// An environment variable of the host.
    Env(String),
    /// A name in the host's vault.
    Vault(String),
}

impl CredentialRef {
    /// Read a reference. Anything that is not one is refused.
    ///
    /// This is the guard: `parse("sk-abc123")` fails, so a key cannot travel inside
    /// a provider description by accident.
    pub fn parse(text: &str) -> Result<Self, CredentialError> {
        let text = text.trim();
        if text == "none" {
            return Ok(Self::None);
        }
        if let Some(name) = text.strip_prefix("env:") {
            return if name.is_empty() {
                Err(CredentialError::Empty)
            } else {
                Ok(Self::Env(name.to_string()))
            };
        }
        if let Some(name) = text.strip_prefix("vault:") {
            return if name.is_empty() {
                Err(CredentialError::Empty)
            } else {
                Ok(Self::Vault(name.to_string()))
            };
        }
        Err(CredentialError::RawValue)
    }

    /// A printable form. Safe to log: it names the place, not the secret.
    pub fn describe(&self) -> String {
        match self {
            Self::None => "none".to_string(),
            Self::Env(name) => format!("env:{name}"),
            Self::Vault(name) => format!("vault:{name}"),
        }
    }
}

/// A model endpoint most apps meet by name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Preset {
    /// Ollama, the usual local runner.
    Ollama,
    /// LM Studio.
    LmStudio,
    /// `llama.cpp`'s server.
    LlamaCpp,
    /// Any OpenAI-compatible endpoint the app points at.
    OpenAiCompatible,
    /// DeepSeek.
    DeepSeek,
    /// OpenAI.
    OpenAi,
    /// vLLM's OpenAI server, on its usual port.
    Vllm,
    /// OpenRouter: one key, many hosted models.
    OpenRouter,
}

impl Preset {
    /// The base URLs a fresh install meets, so no app retypes them.
    pub fn base_url(self) -> &'static str {
        match self {
            Self::Ollama => "http://localhost:11434/v1",
            Self::LmStudio => "http://localhost:1234/v1",
            Self::LlamaCpp => "http://localhost:8080/v1",
            Self::Vllm => "http://localhost:8000/v1",
            Self::DeepSeek => "https://api.deepseek.com",
            Self::OpenAi => "https://api.openai.com/v1",
            Self::OpenRouter => "https://openrouter.ai/api/v1",
            Self::OpenAiCompatible => "http://localhost:8080/v1",
        }
    }

    /// A model that exists on a fresh install of this preset.
    ///
    /// Empty when the engine decides: a vLLM serves whatever it was started with,
    /// so the model is asked to the engine, never guessed here.
    pub fn default_model(self) -> &'static str {
        match self {
            Self::Ollama => "llama3.2",
            Self::LmStudio | Self::LlamaCpp | Self::OpenAiCompatible => "local-model",
            Self::Vllm => "",
            Self::DeepSeek => "deepseek-chat",
            Self::OpenAi => "gpt-4o-mini",
            Self::OpenRouter => "openai/gpt-4o-mini",
        }
    }

    /// True when the endpoint is expected on this machine.
    pub fn is_local(self) -> bool {
        matches!(
            self,
            Self::Ollama | Self::LmStudio | Self::LlamaCpp | Self::Vllm | Self::OpenAiCompatible
        )
    }
}

/// Everything needed to reach a model, minus the secret.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Provider {
    pub preset: Preset,
    pub base_url: String,
    pub model: String,
    pub credential: CredentialRef,
}

impl Provider {
    /// The provider a fresh install starts with: a local model, no key.
    pub fn local(preset: Preset) -> Self {
        Self {
            preset,
            base_url: preset.base_url().to_string(),
            model: preset.default_model().to_string(),
            credential: CredentialRef::None,
        }
    }

    /// A remote provider, whose credential is referenced rather than held.
    pub fn remote(preset: Preset, credential: CredentialRef) -> Self {
        Self {
            preset,
            base_url: preset.base_url().to_string(),
            model: preset.default_model().to_string(),
            credential,
        }
    }

    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        self.model = model.into();
        self
    }

    pub fn with_base_url(mut self, base_url: impl Into<String>) -> Self {
        self.base_url = base_url.into();
        self
    }

    /// True when the model is expected on this machine.
    pub fn is_local(&self) -> bool {
        self.preset.is_local()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_key_is_refused_where_a_reference_is_expected() {
        // La forme exacte d'une clé n'a pas d'importance : ce qui compte est qu'elle
        // ne soit pas une référence, et qu'on le dise.
        for raw in ["sk-abc123", "AIzaSyD…", "deadbeef"] {
            assert_eq!(
                CredentialRef::parse(raw),
                Err(CredentialError::RawValue),
                "`{raw}` est une valeur, pas une référence"
            );
        }
    }

    #[test]
    fn the_three_reference_forms_are_accepted() {
        assert_eq!(CredentialRef::parse("none"), Ok(CredentialRef::None));
        assert_eq!(
            CredentialRef::parse("env:DEEPSEEK_API_KEY"),
            Ok(CredentialRef::Env("DEEPSEEK_API_KEY".into()))
        );
        assert_eq!(
            CredentialRef::parse("vault:deepseek-api-key"),
            Ok(CredentialRef::Vault("deepseek-api-key".into()))
        );
    }

    #[test]
    fn a_reference_without_a_name_is_refused() {
        assert_eq!(CredentialRef::parse("env:"), Err(CredentialError::Empty));
        assert_eq!(CredentialRef::parse("vault:"), Err(CredentialError::Empty));
    }

    #[test]
    fn describing_a_reference_never_reveals_a_secret() {
        let described = CredentialRef::Vault("deepseek-api-key".into()).describe();
        assert_eq!(described, "vault:deepseek-api-key");
        assert!(!described.contains("sk-"));
    }

    #[test]
    fn a_fresh_install_targets_a_local_model_with_no_key() {
        let provider = Provider::local(Preset::Ollama);
        assert_eq!(provider.base_url, "http://localhost:11434/v1");
        assert_eq!(provider.credential, CredentialRef::None);
        assert!(provider.is_local());
    }

    #[test]
    fn every_local_preset_knows_its_own_port() {
        for preset in [Preset::Ollama, Preset::LmStudio, Preset::LlamaCpp] {
            let provider = Provider::local(preset);
            assert!(provider.base_url.starts_with("http://localhost:"));
            assert!(
                !provider.model.is_empty(),
                "{preset:?} doit nommer un modèle"
            );
            assert!(provider.is_local());
        }
    }

    #[test]
    fn a_remote_provider_is_not_local() {
        let provider = Provider::remote(
            Preset::DeepSeek,
            CredentialRef::Env("DEEPSEEK_API_KEY".into()),
        );
        assert!(!provider.is_local());
        assert_eq!(provider.base_url, "https://api.deepseek.com");
    }
}
