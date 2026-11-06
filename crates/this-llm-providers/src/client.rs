//! One provider, reached over the OpenAI-compatible wire: list its models, fit and
//! budget a conversation, send it (whole or streamed), settle what it really consumed.

use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use reqwest::RequestBuilder;
use serde_json::{Value, json};
use this_llm::Completion;

use crate::budget::{Clock, Ledger, Usage};
use crate::config::{Pricing, ProviderConfig, ProviderId};
use crate::error::ProviderError;
use crate::model::{ContextFit, ModelInfo, estimate_tokens, fit, parse_models};
use crate::secret::Secret;

/// What the chat template adds around a conversation, estimated once per call.
const TEMPLATE_OVERHEAD_TOKENS: u32 = 8;

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
        let builder = reqwest::Client::builder();
        #[cfg(not(target_arch = "wasm32"))]
        let builder = builder.timeout(std::time::Duration::from_secs(config.timeout_secs));
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

    /// Fits, reserves, and says which model answers. The reservation is returned, not
    /// held: whoever sends the request owns it, and dropping it gives the budget back.
    async fn prepare(
        &self,
        request: &TurnRequest,
    ) -> Result<(String, u32, Reservation), ProviderError> {
        let (model, window) = self.resolve_model().await?;

        let mut payload = request.messages.to_string();
        if let Some(tools) = &request.tools {
            payload.push_str(&tools.to_string());
        }
        let input = estimate_tokens(&payload).saturating_add(TEMPLATE_OVERHEAD_TOKENS);

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
        Ok((model, max_output, reservation))
    }

    /// The JSON body for one request. `stream` asks for server-sent events and the usage
    /// of the stream, which OpenAI-compatible engines send as a final chunk.
    fn body(&self, model: &str, request: &TurnRequest, max_output: u32, stream: bool) -> Value {
        let mut body = json!({
            "model": model,
            "messages": request.messages,
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
        let (model, max_output, reservation) = self.prepare(&request).await?;
        let body = self.body(&model, &request, max_output, false);
        // Any error below drops the reservation, which gives the budget back.
        let response = self
            .authorize(self.http.post(self.url("chat/completions")).json(&body))
            .send()
            .await
            .map_err(transport)?;
        let value = read_json(response).await?;

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
        let (model, max_output, reservation) = self.prepare(&request).await?;
        let body = self.body(&model, &request, max_output, true);
        let response = self
            .authorize(self.http.post(self.url("chat/completions")).json(&body))
            .send()
            .await
            .map_err(transport)?;
        if !response.status().is_success() {
            let code = response.status().as_u16();
            let text = response.text().await.unwrap_or_default();
            return Err(ProviderError::Status {
                code,
                excerpt: excerpt(&text),
            });
        }
        Ok(Turn {
            response,
            pending: Vec::new(),
            content: String::new(),
            calls: Vec::new(),
            finish: None,
            done_seen: false,
            ended: false,
            reported: None,
            model,
            pricing: self.config.pricing,
            reservation,
        })
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
    reported: Option<Value>,
    model: String,
    pricing: Option<Pricing>,
    reservation: Reservation,
}

#[cfg(not(target_arch = "wasm32"))]
impl Turn {
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
    /// A stream that stops without `[DONE]`, without a finish reason and without a tool
    /// call is an error: its text is a fragment, not an answer.
    pub async fn next(&mut self) -> Result<Option<String>, ProviderError> {
        loop {
            if let Some(piece) = self.take_piece() {
                return Ok(Some(piece));
            }
            if self.ended {
                if self.finish.is_none() && !self.done_seen && self.calls.is_empty() {
                    return Err(ProviderError::Parse(
                        "flux coupé avant la fin : ni [DONE] ni finish_reason".to_string(),
                    ));
                }
                return Ok(None);
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

    /// Reads the complete lines already buffered, and returns the first text piece.
    fn take_piece(&mut self) -> Option<String> {
        while let Some(position) = self.pending.iter().position(|byte| *byte == b'\n') {
            let line: Vec<u8> = self.pending.drain(..=position).collect();
            let text = String::from_utf8_lossy(&line);
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
