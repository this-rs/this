//! The providers of a this-rs app: configure them, load their models, budget them,
//! and fit a prompt to what they can hold.
//!
//! ## The shape of the module
//!
//! - [`ProviderConfig`] — one provider, as a user configures it: a preset, a base URL,
//!   a model (or "the one the engine announces"), a credential **reference**, a
//!   context window, a price, a [`Budget`].
//! - [`Hub`] — the registry: the list of providers, the active one, the spending
//!   ledger. It is generic over where the configuration lives ([`ProviderStore`]),
//!   where secrets come from ([`SecretResolver`]) and what time it is ([`Clock`]).
//! - [`ProviderClient`] (feature `http`) — talks to one provider: lists its models,
//!   fits and budgets a request, sends it, settles the real usage. It implements
//!   [`this_llm::Completion`], so an app's harness does not change.
//!
//! The chat wire format is OpenAI-compatible (`/models`, `/chat/completions`), which
//! covers Ollama, LM Studio, llama.cpp, vLLM, OpenRouter, DeepSeek and OpenAI.
//!
//! ## What does not travel
//!
//! - A credential is a **reference** ([`this_llm::CredentialRef`]), never a value. A
//!   URL that carries `user:password@` is refused by [`ProviderConfig::validate`].
//! - A [`Secret`] has no `Display` and a redacted `Debug`; it is resolved when a client
//!   is built, held by that client, and never written to a store or a log.
//!
//! ## Budgets are reserved, then settled
//!
//! A request reserves its worst case (estimated input + `max_output_tokens`, priced)
//! before it leaves, and gives back the difference once the engine answers. Two calls
//! in flight cannot both pass a limit that only one of them fits.
//!
//! ## Estimates are conservative and say so
//!
//! Tokens are estimated at three characters each ([`estimate_tokens`]), which
//! overestimates for plain English and is closer to the truth for French, code and
//! JSON. The estimate only decides what is *refused* before sending; the usage recorded
//! afterwards is the one the engine reports, when it reports one.

mod budget;
mod config;
mod error;
mod hub;
pub mod images;
mod model;
mod secret;
mod store;

#[cfg(feature = "http")]
mod client;

pub use budget::{BudgetError, Clock, Ledger, SystemClock, Usage};
pub use config::{Budget, ConfigError, Pricing, ProviderConfig, ProviderId};
pub use error::{ErrorKind, ProviderError};
pub use hub::Hub;
pub use model::{ContextFit, ModelInfo, ParseError, estimate_tokens, fit, parse_models};
pub use secret::{Secret, SecretError, SecretResolver, Secrets};
pub use store::{JsonFileStore, MemoryStore, ProviderStore, Snapshot, StoreError};

#[cfg(all(feature = "http", not(target_arch = "wasm32")))]
pub use client::Turn;
#[cfg(feature = "http")]
pub use client::{ChatRequest, ChatResponse, Finish, ProviderClient, TurnReply, TurnRequest};

pub use this_llm::{Completion, CredentialRef, ImagePart, ImageSource, Preset};
