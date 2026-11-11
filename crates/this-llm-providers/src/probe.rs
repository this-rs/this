//! What one real request says about a model: does it answer, in how long, does it call
//! tools, and when it does not, why.
//!
//! A provider's catalogue (`GET /models`) lists what the engine KNOWS, not what THIS account
//! may call. Measured against NVIDIA NIM (11/2026): of 84 announced models, 60 answer
//! `404 Function '…': Not found for account '…'`, 9 accept the request and never answer, 11
//! answer. A settings screen that offers the whole catalogue lets a person tick models that
//! can never work, and each turn on one stays mute until the silence limit. A probe sends ONE
//! minimal message and turns whatever happens into a [`ProbeVerdict`]: it never fails.
//!
//! The verdict types and the text clean-up are plain data (usable from a wasm front-end that
//! only shows them); the probing itself is [`crate::ProviderClient::probe`] (native targets).

// The probing itself is native only; a wasm build keeps the types and leaves its helpers idle.
#![cfg_attr(target_arch = "wasm32", allow(dead_code))]

use std::time::Duration;

use serde::{Deserialize, Serialize};
#[cfg(feature = "http")]
use serde_json::Value;

/// How long a probe waits for an answer before calling the model mute.
pub const DEFAULT_PROBE_TIMEOUT: Duration = Duration::from_secs(10);

/// The most probes in flight at once (a catalogue has dozens of models; an engine rate-limits).
pub const MAX_PROBE_CONCURRENCY: usize = 6;

#[cfg(feature = "http")]
/// Longest detail kept from an engine's answer.
const DETAIL_LIMIT: usize = 200;

/// Why an engine refused a probe: the remedy differs for each.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Refusal {
    /// 401 / 403: the key is refused (missing, wrong, revoked, or not allowed this model).
    Key,
    /// 429: too many requests; trying again later may work.
    RateLimit,
    /// 5xx: the engine itself failed.
    Server,
    /// Any other status (400, 422, …): the request was refused for its content.
    Other,
}

/// The verdict of a probe. Serialized with a `verdict` tag, in snake_case.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "verdict", rename_all = "snake_case")]
pub enum ProbeVerdict {
    /// HTTP 200 in `ms` milliseconds. `tools` is `Some` only when tools were asked about:
    /// whether the model called one when offered one.
    Answers {
        ms: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tools: Option<bool>,
    },
    /// HTTP 404: the model is in the catalogue, not in this account. `detail` is what the
    /// engine said, with the account identifier masked.
    NotInAccount { detail: String },
    /// The request was accepted and nothing came back within the probe delay.
    Mute { after_ms: u64 },
    /// The engine answered an error status other than 404.
    Refused {
        status: u16,
        reason: Refusal,
        detail: String,
    },
    /// The engine could not be reached (refused connection, DNS, TLS).
    Unreachable { detail: String },
}

impl ProbeVerdict {
    /// Whether the model answered: the only verdict that makes a model worth ticking.
    pub fn answers(&self) -> bool {
        matches!(self, Self::Answers { .. })
    }
}

/// How to probe.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProbeOptions {
    /// Also ask a second question with a tool on offer.
    pub with_tools: bool,
    /// How long to wait for each request before calling the model mute.
    pub timeout: Duration,
}

impl Default for ProbeOptions {
    fn default() -> Self {
        Self {
            with_tools: false,
            timeout: DEFAULT_PROBE_TIMEOUT,
        }
    }
}

impl ProbeOptions {
    pub fn with_tools(mut self, with_tools: bool) -> Self {
        self.with_tools = with_tools;
        self
    }
}

#[cfg(feature = "http")]
/// The reason for a refused status.
pub(crate) fn refusal_of(status: u16) -> Refusal {
    match status {
        401 | 403 => Refusal::Key,
        429 => Refusal::RateLimit,
        500..=599 => Refusal::Server,
        _ => Refusal::Other,
    }
}

#[cfg(feature = "http")]
/// The verdict for a non-200 answer.
pub(crate) fn verdict_of_status(status: u16, body: &str, secret: Option<&str>) -> ProbeVerdict {
    let detail = clean_detail(body, secret);
    if status == 404 {
        return ProbeVerdict::NotInAccount { detail };
    }
    ProbeVerdict::Refused {
        status,
        reason: refusal_of(status),
        detail,
    }
}

#[cfg(feature = "http")]
/// What an engine said, reduced to one short line that is safe to show: the message of its
/// JSON error when it has one (`detail`, `error.message`, `error`, `message`), the key
/// masked wherever it appears, the account identifier masked, truncated.
pub(crate) fn clean_detail(body: &str, secret: Option<&str>) -> String {
    let message = serde_json::from_str::<Value>(body)
        .ok()
        .and_then(|value| {
            ["/detail", "/error/message", "/error", "/message", "/title"]
                .iter()
                .find_map(|path| {
                    value
                        .pointer(path)
                        .and_then(Value::as_str)
                        .map(str::to_string)
                })
        })
        .unwrap_or_else(|| body.to_string());
    redact(&message, secret)
}

#[cfg(feature = "http")]
/// Masks the key and the account identifier in `text`, collapses whitespace, truncates.
pub(crate) fn redact(text: &str, secret: Option<&str>) -> String {
    let mut text = text.to_string();
    if let Some(secret) = secret.filter(|secret| !secret.is_empty()) {
        text = text.replace(secret, "…");
    }
    let text = mask_account(&text);
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() > DETAIL_LIMIT {
        let mut cut: String = flat.chars().take(DETAIL_LIMIT).collect();
        cut.push('…');
        cut
    } else {
        flat
    }
}

#[cfg(feature = "http")]
/// `Not found for account 'abc123'` becomes `Not found for account '…'`: the identifier of
/// the account is not for a screenshot. Every quoted value after the word `account` is masked.
fn mask_account(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find("account") {
        let after = at + "account".len();
        out.push_str(&rest[..after]);
        rest = &rest[after..];
        let trimmed = rest.trim_start();
        let gap = rest.len() - trimmed.len();
        let masked = trimmed
            .chars()
            .next()
            .filter(|c| matches!(c, '\'' | '"' | '`'))
            .and_then(|quote| Some((quote, trimmed[1..].find(quote)?)));
        if let Some((quote, end)) = masked {
            out.push_str(&rest[..gap]);
            out.push(quote);
            out.push('…');
            out.push(quote);
            rest = &trimmed[1 + end + 1..];
        }
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(feature = "http")]
    #[test]
    fn the_nvidia_404_keeps_its_words_and_loses_the_account() {
        let body = r#"{"status":404,"title":"Not Found","detail":"Function 'abc-123': Not found for account 'SECRETACCT9'"}"#;
        match verdict_of_status(404, body, None) {
            ProbeVerdict::NotInAccount { detail } => {
                assert!(detail.contains("Not found for account"), "{detail}");
                assert!(!detail.contains("SECRETACCT9"), "{detail}");
            }
            other => panic!("{other:?}"),
        }
    }

    #[cfg(feature = "http")]
    #[test]
    fn the_key_never_survives_in_a_detail() {
        let body = r#"{"error":{"message":"bad key nvapi-SECRET-KEY-1234 for you"}}"#;
        let detail = clean_detail(body, Some("nvapi-SECRET-KEY-1234"));
        assert!(!detail.contains("SECRET"), "{detail}");
    }

    #[cfg(feature = "http")]
    #[test]
    fn a_long_detail_is_cut() {
        let detail = redact(&"x".repeat(1000), None);
        assert!(detail.chars().count() <= DETAIL_LIMIT + 1);
    }

    #[cfg(feature = "http")]
    #[test]
    fn statuses_say_key_rate_or_server() {
        assert_eq!(refusal_of(401), Refusal::Key);
        assert_eq!(refusal_of(403), Refusal::Key);
        assert_eq!(refusal_of(429), Refusal::RateLimit);
        assert_eq!(refusal_of(503), Refusal::Server);
        assert_eq!(refusal_of(422), Refusal::Other);
    }

    #[test]
    fn verdicts_round_trip_through_json() {
        let all = [
            ProbeVerdict::Answers {
                ms: 400,
                tools: Some(true),
            },
            ProbeVerdict::Answers {
                ms: 400,
                tools: None,
            },
            ProbeVerdict::NotInAccount { detail: "d".into() },
            ProbeVerdict::Mute { after_ms: 10_000 },
            ProbeVerdict::Refused {
                status: 429,
                reason: Refusal::RateLimit,
                detail: "d".into(),
            },
            ProbeVerdict::Unreachable { detail: "d".into() },
        ];
        for verdict in all {
            let json = serde_json::to_string(&verdict).unwrap();
            assert_eq!(
                serde_json::from_str::<ProbeVerdict>(&json).unwrap(),
                verdict,
                "{json}"
            );
        }
        let json = serde_json::to_value(ProbeVerdict::Mute { after_ms: 5 }).unwrap();
        assert_eq!(json["verdict"], "mute");
    }
}
