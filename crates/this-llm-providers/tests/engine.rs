//! The whole path, against a real socket: a stand-in OpenAI-compatible engine answers
//! `/v1/models` and `/v1/chat/completions`, and the hub drives it.

#![cfg(feature = "http")]

use std::sync::Arc;

use serde_json::json;
use this_llm::{Completion, CredentialRef, Preset};
use this_llm_providers::{
    Budget, ChatRequest, Clock, Hub, MemoryStore, ProviderConfig, ProviderError, ProviderId,
    Secrets,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

struct FixedClock(u64);

impl Clock for FixedClock {
    fn day(&self) -> u64 {
        self.0
    }
}

/// Answers the two routes it knows. Reads a whole request (headers, then the body
/// announced by `Content-Length`) before answering, so the client never sees a reset.
async fn engine(listener: TcpListener) {
    loop {
        let Ok((mut socket, _)) = listener.accept().await else {
            return;
        };
        tokio::spawn(async move {
            let mut raw = Vec::new();
            let mut chunk = [0u8; 4096];
            loop {
                let read = socket.read(&mut chunk).await.unwrap_or(0);
                if read == 0 {
                    break;
                }
                raw.extend_from_slice(&chunk[..read]);
                if let Some(end) = find(&raw, b"\r\n\r\n") {
                    let head = String::from_utf8_lossy(&raw[..end]).to_ascii_lowercase();
                    let length = head
                        .lines()
                        .find_map(|line| line.strip_prefix("content-length:"))
                        .and_then(|value| value.trim().parse::<usize>().ok())
                        .unwrap_or(0);
                    if raw.len() >= end + 4 + length {
                        break;
                    }
                }
            }
            let request = String::from_utf8_lossy(&raw).to_string();
            let (status, body) = route(&request);
            let response = format!(
                "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = socket.write_all(response.as_bytes()).await;
            let _ = socket.shutdown().await;
        });
    }
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

fn route(request: &str) -> (&'static str, String) {
    if request.starts_with("GET /v1/models") {
        return (
            "200 OK",
            json!({"data": [{"id": "qwen-test", "max_model_len": 4096}]}).to_string(),
        );
    }
    if request.starts_with("POST /v1/chat/completions") {
        if !request.contains("\"model\":\"qwen-test\"") {
            return (
                "400 Bad Request",
                json!({"error": "modèle inconnu"}).to_string(),
            );
        }
        return (
            "200 OK",
            json!({
                "choices": [{"message": {"content": "bonjour"}, "finish_reason": "stop"}],
                "usage": {"prompt_tokens": 10, "completion_tokens": 5}
            })
            .to_string(),
        );
    }
    ("404 Not Found", json!({"error": "route"}).to_string())
}

/// Starts the engine and returns the base URL a provider should point at.
async fn start_engine() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("port libre");
    let address = listener.local_addr().expect("adresse locale");
    tokio::spawn(engine(listener));
    format!("http://{address}/v1")
}

fn provider(base_url: &str, budget: Budget) -> ProviderConfig {
    let mut config = ProviderConfig::new(
        ProviderId::new("local").unwrap(),
        "Moteur de test",
        Preset::OpenAiCompatible,
        CredentialRef::None,
    );
    config.base_url = base_url.to_string();
    config.budget = budget;
    config
}

fn hub_with(config: ProviderConfig, day: u64) -> Hub {
    let hub = Hub::open(MemoryStore::default(), Secrets::env_only(), FixedClock(day)).unwrap();
    hub.upsert(config.clone()).unwrap();
    hub.set_active(Some(&config.id)).unwrap();
    hub
}

#[tokio::test]
async fn the_engine_model_is_discovered_and_the_real_usage_is_recorded() {
    let base = start_engine().await;
    let hub = hub_with(provider(&base, Budget::default()), 20_000);
    let client = hub
        .client(&ProviderId::new("local").unwrap())
        .expect("client");

    let response = client
        .chat(ChatRequest::new("Tu es bref.", "Dis bonjour."))
        .await
        .expect("réponse");

    assert_eq!(response.text, "bonjour");
    assert_eq!(
        response.model, "qwen-test",
        "le modèle est celui que le moteur annonce"
    );
    assert!(!response.truncated);
    assert_eq!(
        response.usage.input_tokens, 10,
        "l'usage réel, pas l'estimation"
    );
    assert_eq!(
        hub.usage_today(&ProviderId::new("local").unwrap()).tokens(),
        15
    );
}

#[tokio::test]
async fn a_request_larger_than_the_window_is_refused_before_it_is_sent() {
    let base = start_engine().await;
    let hub = hub_with(provider(&base, Budget::default()), 20_000);
    let client = hub
        .client(&ProviderId::new("local").unwrap())
        .expect("client");

    let mut request = ChatRequest::new("sys", "x".repeat(30_000));
    request.max_output_tokens = Some(64);
    match client.chat(request).await {
        Err(ProviderError::Context { window, .. }) => assert_eq!(window, 4096),
        other => panic!("attendu un refus de contexte, obtenu {other:?}"),
    }
    assert_eq!(
        hub.usage_today(&ProviderId::new("local").unwrap()).tokens(),
        0,
        "rien réservé, rien dépensé"
    );
}

#[tokio::test]
async fn the_daily_budget_stops_the_second_call_and_the_first_is_kept() {
    let base = start_engine().await;
    // Each call reserves 30 estimated tokens (1 + 1 + 8 overhead + 20 output). The first
    // settles at 15 real ones; the second would bring the day to 15 + 30 = 45, past 30.
    let budget = Budget {
        max_output_tokens: 20,
        max_tokens_per_day: Some(30),
        max_cost_micros_per_day: None,
    };
    let hub = hub_with(provider(&base, budget), 20_000);
    let client = hub
        .client(&ProviderId::new("local").unwrap())
        .expect("client");

    client
        .chat(ChatRequest::new("sys", "hi"))
        .await
        .expect("premier appel dans le budget");
    match client.chat(ChatRequest::new("sys", "hi")).await {
        Err(ProviderError::Budget(_)) => {}
        other => panic!("attendu un refus de budget, obtenu {other:?}"),
    }
    assert_eq!(
        hub.usage_today(&ProviderId::new("local").unwrap()).tokens(),
        15
    );
}

#[tokio::test]
async fn the_completion_contract_is_served_by_the_same_client() {
    let base = start_engine().await;
    let hub = hub_with(provider(&base, Budget::default()), 20_000);
    let client = hub
        .client(&ProviderId::new("local").unwrap())
        .expect("client");

    let text = Completion::complete(&client, "sys", "bonjour")
        .await
        .expect("texte");
    assert_eq!(text, "bonjour");
}

#[tokio::test]
async fn an_engine_error_is_reported_and_its_reservation_given_back() {
    let base = start_engine().await;
    let mut config = provider(&base, Budget::default());
    config.model = Some("modele-absent".into());
    let hub = hub_with(config, 20_000);
    let client = hub
        .client(&ProviderId::new("local").unwrap())
        .expect("client");

    match client.chat(ChatRequest::new("sys", "hi")).await {
        Err(ProviderError::Status { code, .. }) => assert_eq!(code, 400),
        other => panic!("attendu un statut 400, obtenu {other:?}"),
    }
    assert_eq!(
        hub.usage_today(&ProviderId::new("local").unwrap()).tokens(),
        0
    );
}

#[tokio::test]
async fn concurrent_calls_cannot_both_pass_a_limit_only_one_fits() {
    let base = start_engine().await;
    let budget = Budget {
        max_output_tokens: 20,
        max_tokens_per_day: Some(30),
        max_cost_micros_per_day: None,
    };
    let hub = Arc::new(hub_with(provider(&base, budget), 20_000));
    let client = Arc::new(
        hub.client(&ProviderId::new("local").unwrap())
            .expect("client"),
    );

    let first = {
        let client = Arc::clone(&client);
        tokio::spawn(async move { client.chat(ChatRequest::new("sys", "hi")).await })
    };
    let second = {
        let client = Arc::clone(&client);
        tokio::spawn(async move { client.chat(ChatRequest::new("sys", "hi")).await })
    };
    let outcomes = [first.await.unwrap(), second.await.unwrap()];
    let accepted = outcomes.iter().filter(|outcome| outcome.is_ok()).count();
    assert_eq!(
        accepted, 1,
        "une seule réservation tient dans le budget : {outcomes:?}"
    );
}
