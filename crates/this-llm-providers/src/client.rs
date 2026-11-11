//! One provider, reached over the OpenAI-compatible wire: list its models, fit and
//! budget a conversation, send it (whole or streamed), settle what it really consumed.

use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use reqwest::RequestBuilder;
use serde_json::{Value, json};
use this_llm::Completion;

use crate::budget::{Clock, Ledger, Usage};
use crate::config::{Pricing, ProviderConfig, ProviderId};
use crate::error::ProviderError;
use crate::images::{
    IMAGE_TOKENS_ESTIMATE, count_images, error_message, refuses_images, render_openai,
    without_images,
};
use crate::model::{ContextFit, ModelInfo, estimate_tokens, fit, parse_models};
#[cfg(not(target_arch = "wasm32"))]
use crate::probe::{MAX_PROBE_CONCURRENCY, ProbeOptions, ProbeVerdict, redact, verdict_of_status};

/// Room the tool question gives a model to think and then call: a reasoning model needs
/// well over 64 tokens before its call (measured: 130 for `nemotron-3-nano-omni`).
const PROBE_TOOLS_MAX_TOKENS: u32 = 512;
/// A model that answered without calling the tool is asked this many times in all.
const PROBE_TOOLS_ATTEMPTS: u32 = 2;
use crate::secret::Secret;

/// What the chat template adds around a conversation, estimated once per call.
const TEMPLATE_OVERHEAD_TOKENS: u32 = 8;

/// Ceiling on the connection phase alone (DNS, TCP, TLS): a reachable engine accepts its
/// socket within seconds, even when it then reads a long prompt for minutes.
#[cfg(not(target_arch = "wasm32"))]
const CONNECT_TIMEOUT_SECS: u64 = 15;

/// One question for a model, as two messages.
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

/// The answer to a [`ChatRequest`], with what it cost and whether it was cut short.
#[derive(Debug, Clone, PartialEq)]
pub struct ChatResponse {
    pub text: String,
    /// The model the engine ran (resolved, when the configuration left it open).
    pub model: String,
    pub usage: Usage,
    /// The output hit its cap: the text ends mid-thought and must not be trusted as whole.
    pub truncated: bool,
}

/// A conversation in the engine's own message format, with optional tools.
#[derive(Debug, Clone, PartialEq)]
pub struct TurnRequest {
    /// The `messages` array, as the chat endpoint takes it.
    pub messages: Value,
    /// The `tools` array, when the model may call tools.
    pub tools: Option<Value>,
    /// Overrides the provider's `budget.max_output_tokens` for this turn.
    pub max_output_tokens: Option<u32>,
    pub temperature: f32,
}

impl TurnRequest {
    pub fn new(messages: Value) -> Self {
        Self {
            messages,
            tools: None,
            max_output_tokens: None,
            temperature: 0.0,
        }
    }

    pub fn with_tools(mut self, tools: Value) -> Self {
        self.tools = Some(tools);
        self
    }
}

/// Why the engine stopped.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Finish {
    Stop,
    ToolCalls,
    /// The output hit its cap.
    Length,
    /// Any other reason the engine names, kept verbatim.
    Other(String),
}

impl Finish {
    fn from_reason(reason: &str) -> Self {
        match reason {
            "stop" => Self::Stop,
            "tool_calls" | "function_call" => Self::ToolCalls,
            "length" => Self::Length,
            other => Self::Other(other.to_string()),
        }
    }

    pub fn as_str(&self) -> &str {
        match self {
            Self::Stop => "stop",
            Self::ToolCalls => "tool_calls",
            Self::Length => "length",
            Self::Other(reason) => reason,
        }
    }
}

/// A finished turn: the assistant message in OpenAI shape, and what it cost.
#[derive(Debug, Clone, PartialEq)]
pub struct TurnReply {
    pub content: String,
    /// Tool calls as the engine wrote them (`id`, `type`, `function{name,arguments}`).
    pub tool_calls: Vec<Value>,
    pub finish: Finish,
    pub model: String,
    pub usage: Usage,
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

/// The message of a transport error, with its cause. reqwest's own message stops at
/// "error sending request for url (…)", which hides *why*: a refused connection, an unknown
/// host, a local-network permission. The chain is what a user needs to act on.
fn cause_chain(error: &dyn std::error::Error) -> String {
    let mut message = error.to_string();
    let mut source = error.source();
    while let Some(cause) = source {
        let text = cause.to_string();
        if !message.contains(&text) {
            message.push_str(" — ");
            message.push_str(&text);
        }
        source = cause.source();
    }
    message
}

fn transport(error: reqwest::Error) -> ProviderError {
    let message = cause_chain(&error);
    if error.is_timeout() {
        ProviderError::Timeout(message)
    } else {
        ProviderError::Transport(message)
    }
}

fn excerpt(text: &str) -> String {
    text.chars().take(300).collect()
}

/// The error for a non-2xx answer. A request that carried images and a body that says the
/// engine reads none become [`ProviderError::ImagesUnsupported`]; anything else stays a
/// status with a short excerpt of the body.
fn status_error(code: u16, text: &str, images: usize) -> ProviderError {
    if images > 0 && refuses_images(text) {
        return ProviderError::ImagesUnsupported {
            code,
            detail: excerpt(&error_message(text)),
        };
    }
    ProviderError::Status {
        code,
        excerpt: excerpt(text),
    }
}

/// Reads an answer: a non-2xx status becomes an error (see [`status_error`]). `images` is
/// how many images the request carried.
async fn read_json(response: reqwest::Response, images: usize) -> Result<Value, ProviderError> {
    let status = response.status();
    let text = response.text().await.map_err(transport)?;
    if !status.is_success() {
        return Err(status_error(status.as_u16(), &text, images));
    }
    serde_json::from_str(&text).map_err(|error| {
        ProviderError::Parse(format!("JSON invalide : {error} ({})", excerpt(&text)))
    })
}

/// The usage an engine reports, priced; or the reserved estimate when it reports none.
fn usage_of(value: &Value, fallback: Usage, pricing: Option<Pricing>) -> Usage {
    let Some(reported) = value.get("usage").filter(|usage| !usage.is_null()) else {
        // An engine that reports no usage is charged what was reserved: the safe side.
        return fallback;
    };
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
        cost_micros: pricing.map_or(0, |price| price.cost_micros(input, output)),
    }
}

/// A reservation on the ledger. Settled by [`Reservation::settle`]; anything else that
/// drops it (an error, an abandoned stream) gives the reserved amount back.
struct Reservation {
    ledger: Arc<Mutex<Ledger>>,
    id: ProviderId,
    day: u64,
    reserved: Usage,
    settled: bool,
}

impl Reservation {
    fn settle(mut self, actual: Usage) {
        lock(&self.ledger).settle(&self.id, self.day, self.reserved, actual);
        self.settled = true;
    }
}

impl Drop for Reservation {
    fn drop(&mut self) {
        if !self.settled {
            lock(&self.ledger).release(&self.id, self.day, self.reserved);
        }
    }
}

impl ProviderClient {
    pub(crate) fn new(
        config: ProviderConfig,
        secret: Option<Secret>,
        ledger: Arc<Mutex<Ledger>>,
        clock: Arc<dyn Clock>,
    ) -> Self {
        // On a native target the client carries the configured timeout. In a browser
        // the page's own network stack decides, and reqwest offers no timeout there.
        //
        // `timeout_secs` is a SILENCE limit, never a total deadline. `ClientBuilder::timeout`
        // bounds the whole exchange, streamed body included: a local engine that reads a long
        // prompt then writes 4096 tokens (measured 07/11/2026: 53 tok/s, so 77 s for the
        // output alone) was cut at 120 s while it was still talking. A read timeout resets on
        // every chunk: a live stream runs as long as it needs, a mute one stops after
        // `timeout_secs`.
        let builder = reqwest::Client::builder();
        #[cfg(not(target_arch = "wasm32"))]
        let builder = {
            let silence = std::time::Duration::from_secs(config.timeout_secs);
            builder
                .connect_timeout(silence.min(std::time::Duration::from_secs(CONNECT_TIMEOUT_SECS)))
                .read_timeout(silence)
        };
        let http = builder.build().unwrap_or_else(|_| reqwest::Client::new());
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
        let body = read_json(response, 0).await?;
        Ok(parse_models(&body)?)
    }

    /// The model to ask and the window it has, resolved from the configuration and the
    /// engine's announcement. An engine without `/models` is still usable when a model is
    /// configured: the window then comes from the configuration, or stays unknown.
    async fn resolve_model(&self) -> Result<(String, Option<u32>), ProviderError> {
        let (model, announced) = match (&self.config.model, self.list_models().await) {
            (Some(name), Ok(models)) => {
                // An engine that serves ONE model (llama-server) answers whatever name it is
                // given: a configured name that differs from its file name still gets that
                // model's window, instead of no window (and no fitting of the output cap).
                let window = models
                    .iter()
                    .find(|model| &model.id == name)
                    .or(match models.as_slice() {
                        [only] => Some(only),
                        _ => None,
                    })
                    .and_then(|model| model.context_window);
                (name.clone(), window)
            }
            (Some(name), Err(_)) => (name.clone(), None),
            (None, Ok(models)) => {
                // La liste cochée est un ORDRE de préférence, pas un filtre : le premier
                // coché que le moteur annonce gagne, sinon le premier qu'il annonce. Un
                // modèle coché puis retiré du catalogue ne bloque donc rien.
                let choisi = self
                    .config
                    .models
                    .iter()
                    .find_map(|voulu| models.iter().find(|model| &model.id == voulu))
                    .or_else(|| models.first())
                    .ok_or_else(|| ProviderError::NoModel(self.config.id.to_string()))?;
                (choisi.id.clone(), choisi.context_window)
            }
            // Un moteur qui cache sa liste (`/models` en erreur) mais sert un modèle coché
            // reste utilisable : la fenêtre est inconnue, le plafond n'est pas rétréci.
            (None, Err(error)) => match self.config.models.first() {
                Some(premier) => (premier.clone(), None),
                None => return Err(error),
            },
        };
        Ok((model, self.config.context_window.or(announced)))
    }

    /// Fits, reserves, and says which model answers. The reservation is returned, not
    /// held: whoever sends the request owns it, and dropping it gives the budget back.
    ///
    /// Also renders the messages in the engine's dialect (images included): a request
    /// whose images cannot go is refused here, before any network call.
    async fn prepare(
        &self,
        request: &TurnRequest,
    ) -> Result<(String, u32, Reservation, Value), ProviderError> {
        let messages = render_openai(
            &request.messages,
            self.config.id.as_str(),
            self.config.vision,
        )?;
        let (model, window) = self.resolve_model().await?;

        // An image is counted at a flat estimate, not as the characters of its base64.
        let images = u32::try_from(count_images(&request.messages)).unwrap_or(u32::MAX);
        let mut payload = without_images(&request.messages).to_string();
        if let Some(tools) = &request.tools {
            payload.push_str(&tools.to_string());
        }
        let input = estimate_tokens(&payload)
            .saturating_add(TEMPLATE_OVERHEAD_TOKENS)
            .saturating_add(images.saturating_mul(IMAGE_TOKENS_ESTIMATE));

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
        // The guard is dropped at the end of the statement: no lock is held across an await.
        lock(&self.ledger).reserve(&self.config, day, estimate)?;
        let reservation = Reservation {
            ledger: Arc::clone(&self.ledger),
            id: self.config.id.clone(),
            day,
            reserved: estimate,
            settled: false,
        };
        Ok((model, max_output, reservation, messages))
    }

    /// The JSON body for one request. `messages` are the ones [`Self::prepare`] rendered.
    /// `stream` asks for server-sent events and the usage of the stream, which
    /// OpenAI-compatible engines send as a final chunk.
    fn body(
        &self,
        model: &str,
        messages: Value,
        request: &TurnRequest,
        max_output: u32,
        stream: bool,
    ) -> Value {
        let mut body = json!({
            "model": model,
            "messages": messages,
            "temperature": request.temperature,
            "max_tokens": max_output,
        });
        if let Some(tools) = &request.tools {
            body["tools"] = tools.clone();
            body["tool_choice"] = json!("auto");
        }
        if stream {
            body["stream"] = json!(true);
            body["stream_options"] = json!({ "include_usage": true });
        }
        if self.config.preset.is_local() {
            // Understood by vLLM and llama.cpp for Qwen-style models; ignored elsewhere.
            // Not sent to hosted providers, which refuse unknown fields.
            body["chat_template_kwargs"] = json!({ "enable_thinking": self.config.thinking });
        }
        body
    }

    /// Asks one question and waits for the whole answer.
    pub async fn chat(&self, request: ChatRequest) -> Result<ChatResponse, ProviderError> {
        let turn = TurnRequest {
            messages: json!([
                { "role": "system", "content": request.system },
                { "role": "user", "content": request.user },
            ]),
            tools: None,
            max_output_tokens: request.max_output_tokens,
            temperature: request.temperature,
        };
        let reply = self.send_turn(turn).await?;
        Ok(ChatResponse {
            text: reply.content,
            model: reply.model,
            usage: reply.usage,
            truncated: reply.finish == Finish::Length,
        })
    }

    /// Sends a conversation (and its tools) and waits for the whole assistant message.
    pub async fn send_turn(&self, request: TurnRequest) -> Result<TurnReply, ProviderError> {
        let (model, max_output, reservation, messages) = self.prepare(&request).await?;
        let body = self.body(&model, messages, &request, max_output, false);
        // Any error below drops the reservation, which gives the budget back.
        let response = self
            .authorize(self.http.post(self.url("chat/completions")).json(&body))
            .send()
            .await
            .map_err(transport)?;
        let value = read_json(response, count_images(&request.messages)).await?;

        let message = value.pointer("/choices/0/message");
        let content = message
            .and_then(|message| message.get("content"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let tool_calls = message
            .and_then(|message| message.get("tool_calls"))
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        if message.is_none() {
            return Err(ProviderError::Parse(format!(
                "réponse sans message : {}",
                excerpt(&value.to_string())
            )));
        }
        let finish = match value
            .pointer("/choices/0/finish_reason")
            .and_then(Value::as_str)
        {
            Some(reason) => Finish::from_reason(reason),
            None if !tool_calls.is_empty() => Finish::ToolCalls,
            None => Finish::Stop,
        };
        let usage = usage_of(&value, reservation.reserved, self.config.pricing);

        let reply = TurnReply {
            content,
            tool_calls,
            finish,
            model,
            usage,
        };
        reservation.settle(usage);
        Ok(reply)
    }

    /// Sends a conversation and returns a [`Turn`] that yields the text as it arrives.
    ///
    /// Dropping the turn before it ends closes the connection and gives the reserved
    /// budget back. The host owns the loop, the silence timeout and the cancellation.
    /// Native targets only: a browser answers in one piece, through [`Self::send_turn`].
    #[cfg(not(target_arch = "wasm32"))]
    pub async fn open_turn(&self, request: TurnRequest) -> Result<Turn, ProviderError> {
        let (model, max_output, reservation, messages) = self.prepare(&request).await?;
        let body = self.body(&model, messages, &request, max_output, true);
        // The headers of a streamed answer arrive within seconds on a healthy engine. A model
        // that is listed but not served accepts the request and never opens: bounded here, by
        // the OPENING delay, while `read_timeout` (the silence delay) keeps guarding a stream
        // that has begun.
        let sending = self
            .authorize(self.http.post(self.url("chat/completions")).json(&body))
            .send();
        let open = self.config.open_timeout();
        let never_opened = || {
            ProviderError::Timeout(format!(
                "{} · {model} : aucune réponse à l'ouverture en {} s",
                self.config.label,
                open.as_secs()
            ))
        };
        let response = match tokio::time::timeout(open, sending).await {
            Ok(Ok(response)) => response,
            // reqwest's own read timeout can win the race when the silence limit is the
            // shorter one: before any header, that is the same failure.
            Ok(Err(error)) if error.is_timeout() => return Err(never_opened()),
            Ok(Err(error)) => return Err(transport(error)),
            Err(_) => return Err(never_opened()),
        };
        if !response.status().is_success() {
            let code = response.status().as_u16();
            let text = response.text().await.unwrap_or_default();
            return Err(status_error(code, &text, count_images(&request.messages)));
        }
        Ok(Turn {
            response,
            pending: Vec::new(),
            content: String::new(),
            calls: Vec::new(),
            finish: None,
            done_seen: false,
            ended: false,
            alive: false,
            progress: false,
            last_was_progress: false,
            last_progress: std::time::Instant::now(),
            no_progress: Turn::NO_PROGRESS_LIMIT,
            reported: None,
            model,
            pricing: self.config.pricing,
            reservation,
        })
    }
}

/// A probe in flight (`Send`: a host runs the whole batch on a multi-threaded runtime), polled by [`ProviderClient::probe_all_with`].
#[cfg(not(target_arch = "wasm32"))]
type Probing<'a> = std::pin::Pin<Box<dyn std::future::Future<Output = ProbeVerdict> + Send + 'a>>;

#[cfg(not(target_arch = "wasm32"))]
impl ProviderClient {
    /// Sends ONE minimal message to `model` and says what became of it. Never fails: every
    /// outcome, a refused key and an unplugged engine included, is a [`ProbeVerdict`].
    ///
    /// Waits [`crate::DEFAULT_PROBE_TIMEOUT`] (10 s) for an answer. `with_tools` adds a second
    /// request, with a tool on offer, to learn whether the model calls tools.
    ///
    /// A probe does not go through the budget: it is not a turn, it reserves nothing and
    /// settles nothing, so asking about 84 models does not eat the day's ceiling. It is not
    /// free for the engine all the same (one request, or two with tools, per model): a host
    /// runs it on demand, never in the background. The verdict's text never holds the key.
    pub async fn probe(&self, model: &str, with_tools: bool) -> ProbeVerdict {
        self.probe_with(model, &ProbeOptions::default().with_tools(with_tools))
            .await
    }

    /// [`Self::probe`] with its options (the delay, notably).
    pub async fn probe_with(&self, model: &str, options: &ProbeOptions) -> ProbeVerdict {
        let plain = json!({
            "model": model,
            "messages": [{ "role": "user", "content": "Réponds seulement : ok" }],
            "max_tokens": 16,
        });
        let started = std::time::Instant::now();
        if let Err(verdict) = self.probe_request(&plain, options).await {
            return verdict;
        }
        let ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
        let tools = if options.with_tools {
            self.probe_tools(model, options).await
        } else {
            None
        };
        ProbeVerdict::Answers { ms, tools }
    }

    /// Whether the model calls the tool it is offered. `None` when the question could not be
    /// settled: the model did answer, so that is not a failure, but it says nothing about
    /// tools (mute, unreachable, or an answer cut by the limit before the model chose).
    ///
    /// Measured on NVIDIA NIM (11/11/2026): a reasoning model spends its first tokens thinking,
    /// so with `max_tokens` 64 it stops with `finish_reason: length` BEFORE the tool call
    /// (`nemotron-3-nano-omni`: 64 tokens used, no call; 256 tokens: call). A cut answer is not
    /// "no tools". And one sample can miss (`diffusiongemma` once answered in plain text), so a
    /// model that answered WITHOUT calling the tool is asked a second time before the verdict
    /// is `Some(false)`.
    async fn probe_tools(&self, model: &str, options: &ProbeOptions) -> Option<bool> {
        let body = json!({
            "model": model,
            "messages": [{ "role": "user",
                "content": "Lis le module « courses » avec ton outil." }],
            "max_tokens": PROBE_TOOLS_MAX_TOKENS,
            "tools": [{
                "type": "function",
                "function": {
                    "name": "lire_module",
                    "description": "Lit un module installé par son nom.",
                    "parameters": {
                        "type": "object",
                        "properties": { "nom": { "type": "string", "description": "nom du module" } },
                        "required": ["nom"],
                    },
                },
            }],
            "tool_choice": "auto",
        });
        for _attempt in 0..PROBE_TOOLS_ATTEMPTS {
            match self.probe_request(&body, options).await {
                Ok(value) => {
                    if value
                        .pointer("/choices/0/message/tool_calls")
                        .and_then(Value::as_array)
                        .is_some_and(|calls| !calls.is_empty())
                    {
                        return Some(true);
                    }
                    // Cut by the limit: the model had not finished choosing. Not an answer.
                    if value
                        .pointer("/choices/0/finish_reason")
                        .and_then(Value::as_str)
                        == Some("length")
                    {
                        return None;
                    }
                    // Answered in words, no call: ask once more before concluding.
                }
                // An engine that refuses the tools field (400) says the model has none.
                Err(ProbeVerdict::Refused {
                    reason: crate::probe::Refusal::Other,
                    ..
                }) => return Some(false),
                Err(_) => return None,
            }
        }
        Some(false)
    }

    /// One non-streamed request under the probe delay: the JSON on a 200, the verdict otherwise.
    async fn probe_request(
        &self,
        body: &Value,
        options: &ProbeOptions,
    ) -> Result<Value, ProbeVerdict> {
        let started = std::time::Instant::now();
        let after_ms = || u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
        let secret = self.secret.as_ref().map(Secret::expose);
        let unreachable = |error: &reqwest::Error| ProbeVerdict::Unreachable {
            detail: redact(&cause_chain(error), secret),
        };
        let sent = self
            .authorize(
                self.http
                    .post(self.url("chat/completions"))
                    .timeout(options.timeout)
                    .json(body),
            )
            .send()
            .await;
        let response = match sent {
            Ok(response) => response,
            Err(error) if error.is_timeout() => {
                return Err(ProbeVerdict::Mute {
                    after_ms: after_ms(),
                });
            }
            Err(error) => return Err(unreachable(&error)),
        };
        let status = response.status();
        let text = match response.text().await {
            Ok(text) => text,
            Err(error) if error.is_timeout() => {
                return Err(ProbeVerdict::Mute {
                    after_ms: after_ms(),
                });
            }
            Err(error) => return Err(unreachable(&error)),
        };
        if !status.is_success() {
            return Err(verdict_of_status(status.as_u16(), &text, secret));
        }
        // A 200 is an answer; a body that does not read is still one (`Null`).
        Ok(serde_json::from_str(&text).unwrap_or(Value::Null))
    }

    /// Probes every model, at most `concurrency` at a time (never more than
    /// [`MAX_PROBE_CONCURRENCY`], never fewer than one), and returns the verdicts in the
    /// order of `models`. Dropping the future abandons every probe still in flight.
    pub async fn probe_all(
        &self,
        models: &[String],
        with_tools: bool,
        concurrency: usize,
    ) -> Vec<ProbeVerdict> {
        self.probe_all_with(
            models,
            &ProbeOptions::default().with_tools(with_tools),
            concurrency,
        )
        .await
    }

    /// [`Self::probe_all`] with its options.
    pub async fn probe_all_with(
        &self,
        models: &[String],
        options: &ProbeOptions,
        concurrency: usize,
    ) -> Vec<ProbeVerdict> {
        use std::task::Poll;

        let limit = concurrency.clamp(1, MAX_PROBE_CONCURRENCY);
        let mut verdicts: Vec<Option<ProbeVerdict>> = vec![None; models.len()];
        let mut next = 0;
        // No spawn, no extra crate: the probes are polled together from this one future,
        // so dropping it drops them (and closes their sockets).
        let mut running: Vec<(usize, Probing<'_>)> = Vec::new();
        std::future::poll_fn(|cx| {
            loop {
                while running.len() < limit && next < models.len() {
                    let model = models[next].as_str();
                    running.push((next, Box::pin(self.probe_with(model, options))));
                    next += 1;
                }
                if running.is_empty() {
                    return Poll::Ready(());
                }
                let mut finished = false;
                let mut slot = 0;
                while slot < running.len() {
                    if let Poll::Ready(verdict) = running[slot].1.as_mut().poll(cx) {
                        let (index, _) = running.swap_remove(slot);
                        verdicts[index] = Some(verdict);
                        finished = true;
                    } else {
                        slot += 1;
                    }
                }
                if !finished {
                    return Poll::Pending;
                }
            }
        })
        .await;
        verdicts
            .into_iter()
            .map(|verdict| {
                verdict.unwrap_or(ProbeVerdict::Unreachable {
                    detail: String::new(),
                })
            })
            .collect()
    }
}

/// One streamed turn. Yields the text pieces as they arrive; [`Turn::complete`] settles
/// the budget once the stream has ended. Native targets only.
#[cfg(not(target_arch = "wasm32"))]
pub struct Turn {
    response: reqwest::Response,
    pending: Vec<u8>,
    content: String,
    calls: Vec<Value>,
    finish: Option<Finish>,
    done_seen: bool,
    ended: bool,
    /// A line arrived since the last piece was handed out, but it carried no text.
    alive: bool,
    /// A `data:` event that parses arrived since the last piece was handed out.
    progress: bool,
    /// What the last piece handed out by [`Turn::next`] stood for.
    last_was_progress: bool,
    /// When the last `data:` event that parses arrived (or the turn opened).
    last_progress: std::time::Instant,
    /// How long comments alone may keep the turn open.
    no_progress: std::time::Duration,
    reported: Option<Value>,
    model: String,
    pricing: Option<Pricing>,
    reservation: Reservation,
}

#[cfg(not(target_arch = "wasm32"))]
impl Turn {
    /// How long `:` comments ALONE may keep a turn open, with no `data:` event at all.
    ///
    /// A comment is a sign of life, not progress: an engine (or a proxy in front of it) that
    /// sends `: keep-alive` forever re-arms the HTTP read timeout forever, and a caller that
    /// guards silence around `next()` never fires either. Measured in the V20 review: one hour
    /// of keep-alive, never cut. Ten minutes because OpenRouter sends `: OPENROUTER
    /// PROCESSING` while a reasoning model thinks with its reasoning hidden, and the longest
    /// of those (o1-pro / o3-pro class) think for several minutes before the first token;
    /// ten minutes covers them with room, and stays under the 15 min run budget of a harness,
    /// so a live shell that only guards silence (180 s around `next()`) is still bounded.
    pub const NO_PROGRESS_LIMIT: std::time::Duration = std::time::Duration::from_secs(600);

    /// Sets how long `:` comments alone may keep this turn open ([`Self::NO_PROGRESS_LIMIT`]
    /// by default). Counted from the last `data:` event that parses, or from the opening.
    pub fn with_no_progress_limit(mut self, limit: std::time::Duration) -> Self {
        self.no_progress = limit;
        self
    }

    /// Whether the last piece handed out by [`Self::next`] came from a `data:` event (text,
    /// a tool call fragment, reasoning, usage, a role-only delta): PROGRESS. `false` after an
    /// empty piece that only stood for `:` comments: a sign of life, not progress. A caller
    /// guarding the stream re-arms its silence guard on both, its no-progress guard only on
    /// progress.
    pub fn last_was_progress(&self) -> bool {
        self.last_was_progress
    }

    /// The model the engine runs this turn on.
    pub fn model(&self) -> &str {
        &self.model
    }

    /// The text received so far, for showing what arrived before a failure.
    pub fn received(&self) -> &str {
        &self.content
    }

    /// The next piece of text, or `None` once the engine has ended the turn.
    ///
    /// An **empty** piece is a sign of life: the engine sent something that is not text —
    /// a tool call fragment, reasoning, a role-only or empty `delta`, usage, or a `:`
    /// keep-alive comment. It is handed out (at most once per network read, never more
    /// often than the text itself would be) so that a caller guarding the stream with its
    /// own silence timeout around `next()` sees the engine talk. A model writing a whole
    /// module in ONE tool call streams for minutes without a single `content` piece: if
    /// `next()` only returned text, that caller would cut it while the engine is talking.
    /// Callers that only want the text can append the empty piece or skip it.
    ///
    /// Keep-alive comments count too, but only as LIFE: they come from the engine side of
    /// the connection (OpenRouter sends `: OPENROUTER PROCESSING` while a reasoning model
    /// thinks), and the HTTP read timeout counts them as activity. They are not progress
    /// ([`Self::last_was_progress`] is `false`): past [`Self::NO_PROGRESS_LIMIT`] without a
    /// single `data:` event, `next()` fails with [`ProviderError::Timeout`] that says so. A
    /// stream of comments alone is therefore bounded even when the caller guards nothing
    /// but silence.
    ///
    /// A stream that stops without `[DONE]`, without a finish reason and without a tool
    /// call is an error: its text is a fragment, not an answer.
    pub async fn next(&mut self) -> Result<Option<String>, ProviderError> {
        loop {
            if let Some(piece) = self.take_piece() {
                self.alive = false;
                self.progress = false;
                self.last_was_progress = true;
                return Ok(Some(piece));
            }
            if self.ended {
                if self.finish.is_none() && !self.done_seen && self.calls.is_empty() {
                    return Err(ProviderError::StreamCut);
                }
                return Ok(None);
            }
            // Everything buffered is read and none of it was text: say the engine is alive
            // before waiting on the network again.
            if std::mem::take(&mut self.alive) {
                let progressed = std::mem::take(&mut self.progress);
                self.last_was_progress = progressed;
                // Comments only: alive, but has it said anything for too long?
                if !progressed && self.last_progress.elapsed() >= self.no_progress {
                    return Err(ProviderError::Timeout(format!(
                        "aucun progrès depuis {} s : le moteur n'envoie que des signes de vie \
                         (commentaires `:`), ni texte, ni appel d'outil, ni raisonnement",
                        self.no_progress.as_secs()
                    )));
                }
                return Ok(Some(String::new()));
            }
            match self.response.chunk().await.map_err(transport)? {
                Some(bytes) => self.pending.extend_from_slice(&bytes),
                None => {
                    // The last line may lack its newline: close it, then read what is left.
                    if !self.pending.is_empty() {
                        self.pending.push(b'\n');
                    }
                    self.ended = true;
                }
            }
        }
    }

    /// Settles the budget with the usage the stream reported, and returns the message.
    /// Fails when the turn has not ended yet.
    pub fn complete(self) -> Result<TurnReply, ProviderError> {
        if !self.ended {
            return Err(ProviderError::Parse("tour non terminé".to_string()));
        }
        let fallback = self.reservation.reserved;
        let usage = match &self.reported {
            Some(reported) => usage_of(&json!({ "usage": reported }), fallback, self.pricing),
            None => fallback,
        };
        let finish = match (self.finish.clone(), self.calls.is_empty()) {
            (Some(finish), _) => finish,
            (None, false) => Finish::ToolCalls,
            (None, true) => Finish::Stop,
        };
        let reply = TurnReply {
            content: self.content.clone(),
            tool_calls: self.calls.clone(),
            finish,
            model: self.model.clone(),
            usage,
        };
        self.reservation.settle(usage);
        Ok(reply)
    }

    /// Reads the complete lines already buffered, and returns the first text piece. Any
    /// other non-blank line (an event without text, a comment) marks the turn `alive`.
    fn take_piece(&mut self) -> Option<String> {
        while let Some(position) = self.pending.iter().position(|byte| *byte == b'\n') {
            let line: Vec<u8> = self.pending.drain(..=position).collect();
            let text = String::from_utf8_lossy(&line);
            if !text.trim().is_empty() {
                self.alive = true;
            }
            let Some(data) = text.trim().strip_prefix("data:") else {
                continue;
            };
            let data = data.trim();
            if data == "[DONE]" {
                self.done_seen = true;
                self.ended = true;
                continue;
            }
            if data.is_empty() {
                continue;
            }
            let Ok(value) = serde_json::from_str::<Value>(data) else {
                continue;
            };
            // An event that parses is progress, whatever it carries: text, a tool call
            // fragment, reasoning, usage. A `:` comment never reaches this line.
            self.progress = true;
            self.last_progress = std::time::Instant::now();
            if value.get("usage").is_some_and(|usage| !usage.is_null()) {
                self.reported = value.get("usage").cloned();
            }
            let choice = &value["choices"][0];
            if let Some(reason) = choice["finish_reason"].as_str() {
                self.finish = Some(Finish::from_reason(reason));
            }
            if let Some(parts) = choice["delta"]["tool_calls"].as_array() {
                self.join_tool_calls(parts);
            }
            if let Some(piece) = choice["delta"]["content"]
                .as_str()
                .filter(|text| !text.is_empty())
            {
                self.content.push_str(piece);
                return Some(piece.to_string());
            }
        }
        None
    }

    /// Tool calls arrive in fragments, indexed: the name and the arguments are joined here.
    fn join_tool_calls(&mut self, parts: &[Value]) {
        for part in parts {
            let index = part["index"].as_u64().unwrap_or(0) as usize;
            while self.calls.len() <= index {
                self.calls.push(json!({
                    "id": "", "type": "function",
                    "function": { "name": "", "arguments": "" }
                }));
            }
            let call = &mut self.calls[index];
            if let Some(id) = part["id"].as_str() {
                call["id"] = Value::String(id.to_string());
            }
            if let Some(name) = part["function"]["name"].as_str() {
                call["function"]["name"] = Value::String(name.to_string());
            }
            if let Some(fragment) = part["function"]["arguments"].as_str() {
                let mut joined = call["function"]["arguments"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string();
                joined.push_str(fragment);
                call["function"]["arguments"] = Value::String(joined);
            }
        }
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
