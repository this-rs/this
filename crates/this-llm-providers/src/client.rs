//! One provider, reached over the OpenAI-compatible wire: list its models, fit and
//! budget a request, send it, settle what it really consumed.

use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use reqwest::RequestBuilder;
use serde_json::{Value, json};
use this_llm::Completion;

use crate::budget::{Clock, Ledger, Usage};
use crate::config::{ProviderConfig, ProviderId};
use crate::error::ProviderError;
use crate::model::{ContextFit, ModelInfo, estimate_tokens, fit, parse_models};
use crate::secret::Secret;

/// What the chat template adds around the two messages, estimated once per call.
const MESSAGE_OVERHEAD_TOKENS: u32 = 8;

/// One question for a model.
#[derive(Debug, Clone, PartialEq)]
pub struct ChatRequest {
    pub system: String,
    pub user: String,
    /// Overrides the provider's `budget.max_output_tokens` for this call.
    pub max_output_tokens: Option<u32>,
    pub temperature: f32,
}

impl ChatRequest {
    pub fn new(system: impl Into<String>, user: impl Into<String>) -> Self {
        Self {
            system: system.into(),
            user: user.into(),
            max_output_tokens: None,
            temperature: 0.0,
        }
    }
}

/// The answer, with what it cost and whether it was cut short.
#[derive(Debug, Clone, PartialEq)]
pub struct ChatResponse {
    pub text: String,
    /// The model the engine ran (resolved, when the configuration left it open).
    pub model: String,
    pub usage: Usage,
    /// The output hit its cap: the text ends mid-thought and must not be trusted as whole.
    pub truncated: bool,
}

/// A client for one provider. Holds its key; never shows it.
pub struct ProviderClient {
    config: ProviderConfig,
    secret: Option<Secret>,
    http: reqwest::Client,
    ledger: Arc<Mutex<Ledger>>,
    clock: Arc<dyn Clock>,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

fn transport(error: reqwest::Error) -> ProviderError {
    ProviderError::Transport(error.to_string())
}

fn excerpt(text: &str) -> String {
    text.chars().take(300).collect()
}

/// Reads an answer: a non-2xx status becomes an error with a short excerpt of the body.
async fn read_json(response: reqwest::Response) -> Result<Value, ProviderError> {
    let status = response.status();
    let text = response.text().await.map_err(transport)?;
    if !status.is_success() {
        return Err(ProviderError::Status {
            code: status.as_u16(),
            excerpt: excerpt(&text),
        });
    }
    serde_json::from_str(&text).map_err(|error| {
        ProviderError::Parse(format!("JSON invalide : {error} ({})", excerpt(&text)))
    })
}

impl ProviderClient {
    pub(crate) fn new(
        config: ProviderConfig,
        secret: Option<Secret>,
        ledger: Arc<Mutex<Ledger>>,
        clock: Arc<dyn Clock>,
    ) -> Self {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(config.timeout_secs))
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());
        Self {
            config,
            secret,
            http,
            ledger,
            clock,
        }
    }

    pub fn config(&self) -> &ProviderConfig {
        &self.config
    }

    pub fn id(&self) -> &ProviderId {
        &self.config.id
    }

    /// For a settings screen: label, host and model. Never the key, never the full URL.
    pub fn describe(&self) -> String {
        let host = self
            .config
            .base_url
            .trim_start_matches("https://")
            .trim_start_matches("http://")
            .split('/')
            .next()
            .unwrap_or_default();
        let model = self.config.model.as_deref().unwrap_or("(modèle annoncé)");
        format!("{} · {host} · {model}", self.config.label)
    }

    fn url(&self, path: &str) -> String {
        format!("{}/{path}", self.config.normalized_base_url())
    }

    fn authorize(&self, request: RequestBuilder) -> RequestBuilder {
        match &self.secret {
            Some(secret) => request.bearer_auth(secret.expose()),
            None => request,
        }
    }

    /// The models the engine announces. Fails when the engine does not answer.
    pub async fn list_models(&self) -> Result<Vec<ModelInfo>, ProviderError> {
        let response = self
            .authorize(self.http.get(self.url("models")))
            .send()
            .await
            .map_err(transport)?;
        let body = read_json(response).await?;
        Ok(parse_models(&body)?)
    }

    /// The model to ask and the window it has, resolved from the configuration and the
    /// engine's announcement. An engine without `/models` is still usable when a model is
    /// configured: the window then comes from the configuration, or stays unknown.
    async fn resolve_model(&self) -> Result<(String, Option<u32>), ProviderError> {
        let (model, announced) = match (&self.config.model, self.list_models().await) {
            (Some(name), Ok(models)) => {
                let window = models
                    .iter()
                    .find(|model| &model.id == name)
                    .and_then(|model| model.context_window);
                (name.clone(), window)
            }
            (Some(name), Err(_)) => (name.clone(), None),
            (None, Ok(models)) => {
                let first = models
                    .into_iter()
                    .next()
                    .ok_or_else(|| ProviderError::NoModel(self.config.id.to_string()))?;
                (first.id, first.context_window)
            }
            (None, Err(error)) => return Err(error),
        };
        Ok((model, self.config.context_window.or(announced)))
    }

    /// Asks one question: fits it to the window, reserves its worst case against the
    /// budget, sends it, and settles the usage the engine reports.
    pub async fn chat(&self, request: ChatRequest) -> Result<ChatResponse, ProviderError> {
        let (model, window) = self.resolve_model().await?;
        let input = estimate_tokens(&request.system)
            .saturating_add(estimate_tokens(&request.user))
            .saturating_add(MESSAGE_OVERHEAD_TOKENS);

        // An explicit output cap is kept or refused. The default cap is only a ceiling:
        // it shrinks to the room the window leaves after the input, so a small window
        // still answers. A cut-off answer is flagged by `truncated`, never hidden.
        let max_output = match (request.max_output_tokens, window) {
            (Some(explicit), _) => explicit,
            (None, Some(window)) => self
                .config
                .budget
                .max_output_tokens
                .min(window.saturating_sub(input)),
            (None, None) => self.config.budget.max_output_tokens,
        };
        if let Some(window) = window {
            if input >= window {
                return Err(ProviderError::Context {
                    needed: input,
                    window,
                });
            }
            if let ContextFit::TooLong { needed, window } = fit(input, max_output, window) {
                return Err(ProviderError::Context { needed, window });
            }
        }

        let estimate = Usage {
            input_tokens: u64::from(input),
            output_tokens: u64::from(max_output),
            cost_micros: self.config.pricing.map_or(0, |price| {
                price.cost_micros(u64::from(input), u64::from(max_output))
            }),
        };
        let day = self.clock.day();
        // Reserved before the await, and the guard is dropped at the end of the statement.
        lock(&self.ledger).reserve(&self.config, day, estimate)?;

        match self.send(&model, &request, max_output, estimate).await {
            Ok(mut response) => {
                response.model = model;
                lock(&self.ledger).settle(&self.config.id, day, estimate, response.usage);
                Ok(response)
            }
            Err(error) => {
                lock(&self.ledger).release(&self.config.id, day, estimate);
                Err(error)
            }
        }
    }

    async fn send(
        &self,
        model: &str,
        request: &ChatRequest,
        max_output: u32,
        fallback: Usage,
    ) -> Result<ChatResponse, ProviderError> {
        let mut body = json!({
            "model": model,
            "messages": [
                { "role": "system", "content": request.system },
                { "role": "user", "content": request.user },
            ],
            "temperature": request.temperature,
            "max_tokens": max_output,
        });
        if self.config.preset.is_local() {
            // Understood by vLLM and llama.cpp for Qwen-style models; ignored elsewhere.
            // Not sent to hosted providers, which refuse unknown fields.
            body["chat_template_kwargs"] = json!({ "enable_thinking": self.config.thinking });
        }

        let response = self
            .authorize(self.http.post(self.url("chat/completions")).json(&body))
            .send()
            .await
            .map_err(transport)?;
        let value = read_json(response).await?;

        let text = value
            .pointer("/choices/0/message/content")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                ProviderError::Parse(format!(
                    "réponse sans contenu : {}",
                    excerpt(&value.to_string())
                ))
            })?
            .to_string();
        let truncated = value
            .pointer("/choices/0/finish_reason")
            .and_then(Value::as_str)
            == Some("length");

        let usage = match value.get("usage") {
            Some(reported) => {
                let input = reported
                    .get("prompt_tokens")
                    .and_then(Value::as_u64)
                    .unwrap_or(fallback.input_tokens);
                let output = reported
                    .get("completion_tokens")
                    .and_then(Value::as_u64)
                    .unwrap_or(0);
                Usage {
                    input_tokens: input,
                    output_tokens: output,
                    cost_micros: self
                        .config
                        .pricing
                        .map_or(0, |price| price.cost_micros(input, output)),
                }
            }
            // An engine that reports no usage is charged what was reserved: the safe side.
            None => fallback,
        };

        Ok(ChatResponse {
            text,
            model: model.to_string(),
            usage,
            truncated,
        })
    }
}

#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
impl Completion for ProviderClient {
    async fn complete(&self, system: &str, request: &str) -> Result<String, String> {
        self.chat(ChatRequest::new(system, request))
            .await
            .map(|response| response.text)
            .map_err(|error| error.to_string())
    }
}
