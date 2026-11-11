//! What a user configures about one provider, and the checks that keep it honest.

use std::fmt;

use serde::{Deserialize, Serialize};
use this_llm::{CredentialRef, Preset};

/// Why a configuration is refused. Each variant names one rule, in French, for the UI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfigError {
    /// The identifier is not a lowercase slug (`a-z`, `0-9`, `-`, 1 to 48 characters).
    Id(String),
    /// The label is empty or longer than 80 characters.
    Label,
    /// The base URL is not a plain `http(s)://host[/path]`.
    BaseUrl(&'static str),
    /// The base URL carries `user:password@`. Credentials are references, not URL parts.
    CredentialInUrl,
    /// A remote provider over plain `http`: its key would travel in clear.
    InsecureRemote,
    /// A remote provider with no credential reference.
    MissingCredential,
    /// The model name is empty, too long, or contains whitespace.
    Model,
    /// A context window of zero.
    ContextWindow,
    /// A maximum output of zero.
    MaxOutput,
    /// A timeout outside `1..=3600` seconds.
    Timeout,
}

impl fmt::Display for ConfigError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Id(id) => write!(
                formatter,
                "identifiant `{id}` : minuscules, chiffres et tirets, 1 à 48 caractères"
            ),
            Self::Label => formatter.write_str("libellé vide ou trop long (80 caractères max)"),
            Self::BaseUrl(reason) => write!(formatter, "adresse invalide : {reason}"),
            Self::CredentialInUrl => formatter.write_str(
                "l'adresse contient un identifiant (`user:mot-de-passe@`) ; utilise une référence de credential",
            ),
            Self::InsecureRemote => formatter
                .write_str("un fournisseur distant doit être en https (sauf localhost)"),
            Self::MissingCredential => {
                formatter.write_str("un fournisseur distant exige une référence de credential")
            }
            Self::Model => formatter.write_str("nom de modèle vide, trop long ou avec des espaces"),
            Self::ContextWindow => formatter.write_str("la fenêtre de contexte doit être > 0"),
            Self::MaxOutput => formatter.write_str("le maximum de sortie doit être > 0"),
            Self::Timeout => formatter.write_str("le délai doit être entre 1 et 3600 secondes"),
        }
    }
}

impl std::error::Error for ConfigError {}

/// A provider identifier: a lowercase slug, stable across renames of the label.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct ProviderId(String);

impl ProviderId {
    pub fn new(text: &str) -> Result<Self, ConfigError> {
        let valid = !text.is_empty()
            && text.len() <= 48
            && !text.starts_with('-')
            && text
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
        if valid {
            Ok(Self(text.to_string()))
        } else {
            Err(ConfigError::Id(text.to_string()))
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for ProviderId {
    type Error = ConfigError;

    fn try_from(text: String) -> Result<Self, Self::Error> {
        Self::new(&text)
    }
}

impl From<ProviderId> for String {
    fn from(id: ProviderId) -> String {
        id.0
    }
}

impl fmt::Display for ProviderId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// A price, in **micro-dollars per million tokens** (integers: no float drift in a ledger).
/// `$0.27 / Mtok` is `270_000`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Pricing {
    pub input_micros_per_mtok: u64,
    pub output_micros_per_mtok: u64,
}

impl Pricing {
    /// The cost of a call, rounded up to the next micro-dollar.
    pub fn cost_micros(&self, input_tokens: u64, output_tokens: u64) -> u64 {
        let total = u128::from(input_tokens) * u128::from(self.input_micros_per_mtok)
            + u128::from(output_tokens) * u128::from(self.output_micros_per_mtok);
        u64::try_from(total.div_ceil(1_000_000)).unwrap_or(u64::MAX)
    }
}

/// How much a provider may be asked for, per request and per day.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Budget {
    /// Output cap per request, used when the caller does not ask for one.
    pub max_output_tokens: u32,
    /// Input + output tokens allowed per UTC day. `None`: no limit.
    pub max_tokens_per_day: Option<u64>,
    /// Priced spend allowed per UTC day, in micro-dollars. `None`: no limit.
    pub max_cost_micros_per_day: Option<u64>,
}

impl Default for Budget {
    fn default() -> Self {
        Self {
            // 16384, not 4096: an agent writes a whole module in one tool call, and a
            // local engine serves 128k-262k windows. The cap still shrinks to the room the
            // window leaves after the input (see `ProviderClient::prepare`).
            max_output_tokens: 16_384,
            max_tokens_per_day: None,
            max_cost_micros_per_day: None,
        }
    }
}

/// One provider, as configured. Holds references, never secrets.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderConfig {
    pub id: ProviderId,
    pub label: String,
    pub preset: Preset,
    pub base_url: String,
    /// `None`: the first model the engine announces (or the first of `models`).
    #[serde(default)]
    pub model: Option<String>,
    /// Les modèles que la personne a cochés dans la liste annoncée par le moteur.
    ///
    /// Vide (le cas de toutes les configurations existantes) : rien n'est filtré, le moteur
    /// décide. Renseigné : c'est une liste de travail, pas une permission — elle ordonne
    /// ce qu'un écran propose, et `model` absent veut dire « le premier coché ».
    /// Absent du JSON quand vide, pour que les configurations déjà rangées ne bougent pas.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub models: Vec<String>,
    pub credential: CredentialRef,
    /// Overrides the context window the engine announces (or when it announces none).
    #[serde(default)]
    pub context_window: Option<u32>,
    /// `None`: free or unknown; usage is counted in tokens only.
    #[serde(default)]
    pub pricing: Option<Pricing>,
    #[serde(default)]
    pub budget: Budget,
    /// Asks a local reasoning model to think. Off by default: latency beats meditation.
    #[serde(default)]
    pub thinking: bool,
    /// The SILENCE limit: how long a stream that has begun may say nothing between two
    /// pieces. A reasoning model may think for minutes between two chunks.
    #[serde(default = "default_timeout_secs")]
    pub timeout_secs: u64,
    /// The OPENING limit: how long to wait for the headers of a streamed answer. A healthy
    /// engine opens its stream within seconds; one that accepts the request and never opens
    /// (a model listed in a catalogue but not served to this account) would otherwise keep
    /// the turn mute for the whole `timeout_secs`. `None`: 30 s, or `timeout_secs` when that
    /// is shorter. Absent from the JSON when unset, so stored configurations do not change.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub open_timeout_secs: Option<u64>,
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    /// Whether the model reads images. `None`: unknown, images are sent and the engine
    /// answers for itself (an engine that refuses them answers an HTTP error, which is
    /// shown). `Some(false)`: a request with images is refused before it leaves, with the
    /// reason. Absent from the JSON when unknown, so existing configurations are unchanged.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vision: Option<bool>,
}

/// The opening limit when the configuration sets none.
const DEFAULT_OPEN_TIMEOUT_SECS: u64 = 30;

fn default_timeout_secs() -> u64 {
    120
}

fn default_enabled() -> bool {
    true
}

impl ProviderConfig {
    /// A provider with the preset's own address and the defaults of [`Budget`].
    pub fn new(
        id: ProviderId,
        label: impl Into<String>,
        preset: Preset,
        credential: CredentialRef,
    ) -> Self {
        Self {
            id,
            label: label.into(),
            preset,
            base_url: preset.base_url().to_string(),
            model: None,
            models: Vec::new(),
            credential,
            context_window: None,
            pricing: None,
            budget: Budget::default(),
            thinking: false,
            timeout_secs: default_timeout_secs(),
            open_timeout_secs: None,
            enabled: true,
            vision: None,
        }
    }

    /// The base URL without its trailing slash, ready to append `/models`.
    pub fn normalized_base_url(&self) -> &str {
        self.base_url.trim_end_matches('/')
    }

    /// Checks every rule. A configuration that passes can be stored and used.
    pub fn validate(&self) -> Result<(), ConfigError> {
        let label = self.label.trim();
        if label.is_empty() || label.chars().count() > 80 {
            return Err(ConfigError::Label);
        }

        let url = self.base_url.as_str();
        if url.trim() != url {
            return Err(ConfigError::BaseUrl("espaces autour de l'adresse"));
        }
        let (secure, rest) = if let Some(rest) = url.strip_prefix("https://") {
            (true, rest)
        } else if let Some(rest) = url.strip_prefix("http://") {
            (false, rest)
        } else {
            return Err(ConfigError::BaseUrl(
                "doit commencer par http:// ou https://",
            ));
        };
        let authority = rest.split('/').next().unwrap_or_default();
        if authority.is_empty() {
            return Err(ConfigError::BaseUrl("hôte manquant"));
        }
        if authority.contains('@') {
            return Err(ConfigError::CredentialInUrl);
        }
        if url.contains(['?', '#', ' ']) {
            return Err(ConfigError::BaseUrl("ni requête, ni fragment, ni espace"));
        }

        if !self.preset.is_local() {
            let loopback = authority.starts_with("localhost") || authority.starts_with("127.0.0.1");
            if !secure && !loopback {
                return Err(ConfigError::InsecureRemote);
            }
            if self.credential == CredentialRef::None {
                return Err(ConfigError::MissingCredential);
            }
        }

        if let Some(model) = &self.model
            && (model.trim().is_empty()
                || model.len() > 200
                || model.chars().any(char::is_whitespace))
        {
            return Err(ConfigError::Model);
        }
        if self.context_window == Some(0) {
            return Err(ConfigError::ContextWindow);
        }
        if self.budget.max_output_tokens == 0 {
            return Err(ConfigError::MaxOutput);
        }
        if self.timeout_secs == 0 || self.timeout_secs > 3600 {
            return Err(ConfigError::Timeout);
        }
        if self
            .open_timeout_secs
            .is_some_and(|secs| secs == 0 || secs > 3600)
        {
            return Err(ConfigError::Timeout);
        }
        Ok(())
    }

    /// How long to wait for the headers of a streamed answer.
    pub fn open_timeout(&self) -> std::time::Duration {
        let secs = self
            .open_timeout_secs
            .unwrap_or_else(|| DEFAULT_OPEN_TIMEOUT_SECS.min(self.timeout_secs));
        std::time::Duration::from_secs(secs)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(text: &str) -> ProviderId {
        ProviderId::new(text).expect("identifiant valide")
    }

    #[test]
    fn an_unknown_vision_is_absent_from_the_json_and_read_back_as_unknown() {
        let config = ProviderConfig::new(id("local"), "Local", Preset::Ollama, CredentialRef::None);
        let written = serde_json::to_value(&config).unwrap();
        assert!(written.get("vision").is_none(), "{written}");
        let back: ProviderConfig = serde_json::from_value(written).unwrap();
        assert_eq!(back.vision, None);
        let mut blind = config;
        blind.vision = Some(false);
        assert_eq!(serde_json::to_value(&blind).unwrap()["vision"], false);
    }

    #[test]
    fn an_unset_open_timeout_leaves_a_stored_configuration_byte_for_byte() {
        let config = ProviderConfig::new(id("local"), "Local", Preset::Ollama, CredentialRef::None);
        let written = serde_json::to_string(&config).unwrap();
        assert!(!written.contains("open_timeout"), "{written}");
        // A configuration stored before the field existed reads back and writes back unchanged.
        let back: ProviderConfig = serde_json::from_str(&written).unwrap();
        assert_eq!(back.open_timeout_secs, None);
        assert_eq!(serde_json::to_string(&back).unwrap(), written);
        let mut set = config;
        set.open_timeout_secs = Some(12);
        assert_eq!(serde_json::to_value(&set).unwrap()["open_timeout_secs"], 12);
    }

    #[test]
    fn the_open_timeout_is_thirty_seconds_or_the_silence_limit_when_shorter() {
        let mut config =
            ProviderConfig::new(id("local"), "Local", Preset::Ollama, CredentialRef::None);
        assert_eq!(config.open_timeout().as_secs(), 30);
        config.timeout_secs = 10;
        assert_eq!(config.open_timeout().as_secs(), 10);
        config.open_timeout_secs = Some(45);
        assert_eq!(config.open_timeout().as_secs(), 45);
        config.open_timeout_secs = Some(0);
        assert_eq!(config.validate(), Err(ConfigError::Timeout));
    }

    #[test]
    fn identifiers_are_slugs() {
        assert!(ProviderId::new("vllm-local").is_ok());
        assert!(ProviderId::new("deepseek2").is_ok());
        for bad in [
            "",
            "-x",
            "Upper",
            "with space",
            "under_score",
            &"a".repeat(49),
        ] {
            assert!(ProviderId::new(bad).is_err(), "`{bad}` doit être refusé");
        }
    }

    #[test]
    fn nvidia_nim_comes_with_its_address_and_no_guessed_model() {
        let config = ProviderConfig::new(
            id("nim"),
            "NVIDIA NIM",
            Preset::NvidiaNim,
            CredentialRef::Vault("nim-key".to_string()),
        );
        assert_eq!(config.base_url, "https://integrate.api.nvidia.com/v1");
        assert_eq!(config.model, None, "aucun nom deviné : la liste décide");
        assert_eq!(
            config.validate(),
            Ok(()),
            "hébergé, avec sa référence de clé"
        );
    }

    #[test]
    fn une_liste_de_modeles_vide_ne_change_pas_le_json_ecrit() {
        let config = ProviderConfig::new(
            id("nim"),
            "NVIDIA NIM",
            Preset::NvidiaNim,
            CredentialRef::Vault("nim-key".to_string()),
        );
        let written = serde_json::to_value(&config).unwrap();
        assert!(
            written.get("models").is_none(),
            "les configurations déjà rangées ne doivent pas changer de forme : {written}"
        );
        let back: ProviderConfig = serde_json::from_value(written).unwrap();
        assert!(back.models.is_empty());
    }

    #[test]
    fn les_modeles_coches_sont_ranges_et_relus_dans_l_ordre() {
        let mut config = ProviderConfig::new(
            id("nim"),
            "NVIDIA NIM",
            Preset::NvidiaNim,
            CredentialRef::Vault("nim-key".to_string()),
        );
        config.models = vec![
            "meta/llama-3.3-70b-instruct".to_string(),
            "nvidia/nemotron-4-340b".to_string(),
        ];
        let written = serde_json::to_value(&config).unwrap();
        assert_eq!(written["models"][1], "nvidia/nemotron-4-340b");
        let back: ProviderConfig = serde_json::from_value(written).unwrap();
        assert_eq!(back.models, config.models);
    }

    #[test]
    fn a_local_preset_validates_as_it_comes() {
        let config = ProviderConfig::new(id("vllm"), "vLLM", Preset::Vllm, CredentialRef::None);
        assert_eq!(config.validate(), Ok(()));
        assert_eq!(config.base_url, "http://localhost:8000/v1");
    }

    #[test]
    fn a_remote_provider_needs_a_reference() {
        let config = ProviderConfig::new(
            id("deepseek"),
            "DeepSeek",
            Preset::DeepSeek,
            CredentialRef::None,
        );
        assert_eq!(config.validate(), Err(ConfigError::MissingCredential));

        let config = ProviderConfig::new(
            id("deepseek"),
            "DeepSeek",
            Preset::DeepSeek,
            CredentialRef::Env("DEEPSEEK_API_KEY".into()),
        );
        assert_eq!(config.validate(), Ok(()));
    }

    #[test]
    fn a_key_inside_the_url_is_refused() {
        let mut config = ProviderConfig::new(
            id("gateway"),
            "Gateway",
            Preset::OpenAiCompatible,
            CredentialRef::None,
        );
        config.base_url = "https://user:sk-live-123@gateway.example.com/v1".into();
        assert_eq!(config.validate(), Err(ConfigError::CredentialInUrl));
    }

    #[test]
    fn a_remote_provider_over_plain_http_is_refused_unless_loopback() {
        let mut config = ProviderConfig::new(
            id("proxy"),
            "Proxy",
            Preset::OpenAi,
            CredentialRef::Vault("openai".into()),
        );
        config.base_url = "http://api.example.com/v1".into();
        assert_eq!(config.validate(), Err(ConfigError::InsecureRemote));

        config.base_url = "http://127.0.0.1:9000/v1".into();
        assert_eq!(config.validate(), Ok(()));
    }

    #[test]
    fn budgets_and_windows_must_be_positive() {
        let mut config = ProviderConfig::new(id("vllm"), "vLLM", Preset::Vllm, CredentialRef::None);
        config.context_window = Some(0);
        assert_eq!(config.validate(), Err(ConfigError::ContextWindow));
        config.context_window = None;
        config.budget.max_output_tokens = 0;
        assert_eq!(config.validate(), Err(ConfigError::MaxOutput));
    }

    #[test]
    fn a_price_is_exact_in_micro_dollars() {
        // $0.27 per million input tokens: one million input tokens cost 270 000 micro-dollars.
        let price = Pricing {
            input_micros_per_mtok: 270_000,
            output_micros_per_mtok: 1_100_000,
        };
        assert_eq!(price.cost_micros(1_000_000, 0), 270_000);
        assert_eq!(price.cost_micros(0, 1_000_000), 1_100_000);
        // Rounded up: a single token is never free.
        assert_eq!(price.cost_micros(1, 0), 1);
    }

    #[test]
    fn a_config_round_trips_through_json_without_a_secret_field() {
        let config = ProviderConfig::new(
            id("deepseek"),
            "DeepSeek",
            Preset::DeepSeek,
            CredentialRef::Env("DEEPSEEK_API_KEY".into()),
        );
        let json = serde_json::to_string(&config).expect("sérialisable");
        assert!(json.contains("DEEPSEEK_API_KEY"));
        let back: ProviderConfig = serde_json::from_str(&json).expect("relisible");
        assert_eq!(back, config);
    }
}
