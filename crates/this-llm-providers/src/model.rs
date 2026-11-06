//! What an engine says about its models, and whether a prompt fits them.

use std::fmt;

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// A model as an engine announces it. Every field but the id is optional: engines
/// disagree, and a missing window is "unknown", never "zero".
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelInfo {
    pub id: String,
    pub context_window: Option<u32>,
    pub max_output_tokens: Option<u32>,
}

/// The body of a `/models` answer does not describe models.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParseError {
    NoModelList,
}

impl fmt::Display for ParseError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("la réponse ne contient ni `data` ni `models`")
    }
}

impl std::error::Error for ParseError {}

/// Reads a `/models` answer. Accepts the OpenAI shape (`data[].id`, used by vLLM,
/// LM Studio, llama.cpp, OpenRouter) and the Ollama native shape (`models[].model`).
///
/// The context window is read from whichever key the engine uses: `context_length`
/// (LM Studio, OpenRouter), `max_model_len` (vLLM), `meta.n_ctx_train` (llama.cpp).
pub fn parse_models(body: &Value) -> Result<Vec<ModelInfo>, ParseError> {
    let items = body
        .get("data")
        .and_then(Value::as_array)
        .or_else(|| body.get("models").and_then(Value::as_array))
        .ok_or(ParseError::NoModelList)?;
    Ok(items.iter().filter_map(model_from).collect())
}

fn model_from(item: &Value) -> Option<ModelInfo> {
    let id = ["id", "model", "name"]
        .iter()
        .find_map(|key| item.get(*key).and_then(Value::as_str))?
        .trim();
    if id.is_empty() {
        return None;
    }
    let context_window = [
        "context_length",
        "max_model_len",
        "max_context_length",
        "context_window",
    ]
    .iter()
    .find_map(|key| item.get(*key).and_then(as_positive))
    .or_else(|| item.pointer("/meta/n_ctx_train").and_then(as_positive));
    let max_output_tokens = item
        .pointer("/top_provider/max_completion_tokens")
        .and_then(as_positive);
    Some(ModelInfo {
        id: id.to_string(),
        context_window,
        max_output_tokens,
    })
}

fn as_positive(value: &Value) -> Option<u32> {
    value
        .as_u64()
        .and_then(|n| u32::try_from(n).ok())
        .filter(|n| *n > 0)
}

/// An estimate of the tokens in `text`: three characters per token, rounded up.
///
/// Deliberately pessimistic. It decides what is refused *before* a call, so an
/// overestimate costs a refusal that could have passed; an underestimate costs an
/// engine error after the money is spent.
pub fn estimate_tokens(text: &str) -> u32 {
    let chars = text.chars().count();
    u32::try_from(chars.div_ceil(3)).unwrap_or(u32::MAX)
}

/// Whether a request fits a context window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContextFit {
    /// Fits, with `headroom` tokens still free.
    Fits { headroom: u32 },
    /// Does not fit: `needed` tokens (input + reserved output) for a `window`.
    TooLong { needed: u32, window: u32 },
}

/// The window must hold the input **and** the output the request reserves.
pub fn fit(input_tokens: u32, max_output_tokens: u32, window: u32) -> ContextFit {
    let needed = input_tokens.saturating_add(max_output_tokens);
    if needed <= window {
        ContextFit::Fits {
            headroom: window - needed,
        }
    } else {
        ContextFit::TooLong { needed, window }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn the_openai_shape_is_read_with_its_window() {
        let body = json!({"data": [
            {"id": "qwen3-27b", "max_model_len": 32768},
            {"id": "openrouter/x", "context_length": 128000,
             "top_provider": {"max_completion_tokens": 8192}},
            {"id": "bare-model"},
        ]});
        let models = parse_models(&body).expect("liste lisible");
        assert_eq!(models.len(), 3);
        assert_eq!(models[0].context_window, Some(32768));
        assert_eq!(models[1].context_window, Some(128000));
        assert_eq!(models[1].max_output_tokens, Some(8192));
        assert_eq!(models[2].context_window, None, "inconnue, pas zéro");
    }

    #[test]
    fn the_ollama_shape_and_llama_cpp_window_are_read() {
        let body = json!({"models": [{"model": "llama3.2"}]});
        let models = parse_models(&body).expect("liste lisible");
        assert_eq!(models[0].id, "llama3.2");

        let body = json!({"data": [{"id": "gguf", "meta": {"n_ctx_train": 4096}}]});
        assert_eq!(parse_models(&body).unwrap()[0].context_window, Some(4096));
    }

    #[test]
    fn an_answer_without_a_list_is_refused() {
        assert_eq!(
            parse_models(&json!({"error": "nope"})),
            Err(ParseError::NoModelList)
        );
    }

    #[test]
    fn a_zero_window_is_unknown_not_zero() {
        let body = json!({"data": [{"id": "m", "context_length": 0}]});
        assert_eq!(parse_models(&body).unwrap()[0].context_window, None);
    }

    #[test]
    fn the_estimate_is_pessimistic_and_rounds_up() {
        assert_eq!(estimate_tokens(""), 0);
        assert_eq!(estimate_tokens("abc"), 1);
        assert_eq!(estimate_tokens("abcd"), 2);
        // Accents count as characters, not bytes: "é" is one character.
        assert_eq!(estimate_tokens("été"), 1);
    }

    #[test]
    fn a_request_must_hold_its_output_too() {
        assert_eq!(fit(100, 100, 300), ContextFit::Fits { headroom: 100 });
        assert_eq!(
            fit(250, 100, 300),
            ContextFit::TooLong {
                needed: 350,
                window: 300
            }
        );
        assert_eq!(
            fit(u32::MAX, 1, 300),
            ContextFit::TooLong {
                needed: u32::MAX,
                window: 300
            }
        );
    }
}
