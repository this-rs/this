//! The whole path, against a real socket: a stand-in OpenAI-compatible engine answers
//! `/v1/models` and `/v1/chat/completions` (whole or streamed), and the hub drives it.
//!
//! The engine picks its answer from a marker in the user message: `SCENARIO:tools`,
//! `SCENARIO:length`, `SCENARIO:cut`. Anything else gets a plain two-piece answer.

#![cfg(feature = "http")]

use std::sync::Arc;

use serde_json::{Value, json};
use this_llm::{Completion, CredentialRef, Preset};
use this_llm_providers::{
    Budget, ChatRequest, Clock, ErrorKind, Finish, Hub, MemoryStore, ProviderConfig, ProviderError,
    ProviderId, Secrets, TurnRequest,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

struct FixedClock(u64);

impl Clock for FixedClock {
    fn day(&self) -> u64 {
        self.0
    }
}

/// Answers the routes it knows. Reads a whole request (headers, then the body announced
/// by `Content-Length`) before answering, so the client never sees a reset.
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
            let (status, content_type, body) = route(&request);
            let response = format!(
                "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
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

/// Server-sent events: one `data:` line per chunk, then `[DONE]`.
fn sse(events: &[Value]) -> String {
    let mut body: String = events
        .iter()
        .map(|event| format!("data: {event}\n\n"))
        .collect();
    body.push_str("data: [DONE]\n\n");
    body
}

fn delta(content: &str, finish: Option<&str>) -> Value {
    json!({"choices": [{"index": 0, "delta": {"content": content}, "finish_reason": finish}]})
}

fn usage_chunk() -> Value {
    json!({"choices": [], "usage": {"prompt_tokens": 10, "completion_tokens": 5}})
}

fn route(request: &str) -> (&'static str, &'static str, String) {
    if request.starts_with("GET /v1/models") {
        return (
            "200 OK",
            "application/json",
            json!({"data": [{"id": "qwen-test", "max_model_len": 4096}]}).to_string(),
        );
    }
    if request.starts_with("POST /v1/chat/completions") {
        if !request.contains("\"model\":\"qwen-test\"") {
            return (
                "400 Bad Request",
                "application/json",
                json!({"error": "modèle inconnu"}).to_string(),
            );
        }
        let streamed = request.contains("\"stream\":true");
        let tools = request.contains("SCENARIO:tools");
        let scenario = if request.contains("SCENARIO:length") {
            "length"
        } else if request.contains("SCENARIO:cut") {
            "cut"
        } else {
            "plain"
        };

        if streamed {
            let events = match (tools, scenario) {
                (true, _) => vec![
                    json!({"choices": [{"index": 0, "delta": {"tool_calls": [
                        {"index": 0, "id": "call_1", "function": {"name": "lire", "arguments": "{\"pa"}}
                    ]}, "finish_reason": null}]}),
                    json!({"choices": [{"index": 0, "delta": {"tool_calls": [
                        {"index": 0, "function": {"arguments": "th\":\"a\"}"}}
                    ]}, "finish_reason": null}]}),
                    json!({"choices": [{"index": 0, "delta": {}, "finish_reason": "tool_calls"}]}),
                    usage_chunk(),
                ],
                (false, "cut") => vec![delta("Bon", None)],
                (false, "length") => vec![delta("Bon", None), delta("jour", Some("length"))],
                (false, _) => vec![
                    delta("Bon", None),
                    delta("jour", Some("stop")),
                    usage_chunk(),
                ],
            };
            // A cut stream is sent without its `[DONE]`: the engine simply stops.
            let body = if scenario == "cut" && !tools {
                events
                    .iter()
                    .map(|event| format!("data: {event}\n\n"))
                    .collect()
            } else {
                sse(&events)
            };
            return ("200 OK", "text/event-stream", body);
        }

        let message = if tools {
            json!({"role": "assistant", "content": null, "tool_calls": [
                {"id": "call_1", "type": "function",
                 "function": {"name": "lire", "arguments": "{\"path\":\"a\"}"}}
            ]})
        } else {
            json!({"role": "assistant", "content": "bonjour"})
        };
        let finish = if tools { "tool_calls" } else { "stop" };
        return (
            "200 OK",
            "application/json",
            json!({
                "choices": [{"message": message, "finish_reason": finish}],
                "usage": {"prompt_tokens": 10, "completion_tokens": 5}
            })
            .to_string(),
        );
    }
    (
        "404 Not Found",
        "application/json",
        json!({"error": "route"}).to_string(),
    )
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

fn local() -> ProviderId {
    ProviderId::new("local").unwrap()
}

/// A budget whose daily limit fits one call (about 56 estimated tokens) but not two once
/// the first has settled at its real 15 tokens: 15 + 56 > 60.
fn one_call_a_day() -> Budget {
    Budget {
        max_output_tokens: 20,
        max_tokens_per_day: Some(60),
        max_cost_micros_per_day: None,
    }
}

fn conversation(user: &str) -> Value {
    json!([
        { "role": "system", "content": "Tu es bref." },
        { "role": "user", "content": user },
    ])
}

#[tokio::test]
async fn the_engine_model_is_discovered_and_the_real_usage_is_recorded() {
    let base = start_engine().await;
    let hub = hub_with(provider(&base, Budget::default()), 20_000);
    let client = hub.client(&local()).expect("client");

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
    assert_eq!(hub.usage_today(&local()).tokens(), 15);
}

#[tokio::test]
async fn a_request_larger_than_the_window_is_refused_before_it_is_sent() {
    let base = start_engine().await;
    let hub = hub_with(provider(&base, Budget::default()), 20_000);
    let client = hub.client(&local()).expect("client");

    let mut request = ChatRequest::new("sys", "x".repeat(30_000));
    request.max_output_tokens = Some(64);
    match client.chat(request).await {
        Err(ProviderError::Context { window, .. }) => assert_eq!(window, 4096),
        other => panic!("attendu un refus de contexte, obtenu {other:?}"),
    }
    assert_eq!(
        hub.usage_today(&local()).tokens(),
        0,
        "rien réservé, rien dépensé"
    );
}

#[tokio::test]
async fn the_daily_budget_stops_the_second_call_and_the_first_is_kept() {
    let base = start_engine().await;
    let hub = hub_with(provider(&base, one_call_a_day()), 20_000);
    let client = hub.client(&local()).expect("client");

    client
        .chat(ChatRequest::new("Tu es bref.", "Dis bonjour."))
        .await
        .expect("premier appel dans le budget");
    match client
        .chat(ChatRequest::new("Tu es bref.", "Dis bonjour."))
        .await
    {
        Err(ProviderError::Budget(_)) => {}
        other => panic!("attendu un refus de budget, obtenu {other:?}"),
    }
    assert_eq!(hub.usage_today(&local()).tokens(), 15);
}

#[tokio::test]
async fn the_completion_contract_is_served_by_the_same_client() {
    let base = start_engine().await;
    let hub = hub_with(provider(&base, Budget::default()), 20_000);
    let client = hub.client(&local()).expect("client");

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
    let client = hub.client(&local()).expect("client");

    match client.chat(ChatRequest::new("sys", "hi")).await {
        Err(ProviderError::Status { code, .. }) => assert_eq!(code, 400),
        other => panic!("attendu un statut 400, obtenu {other:?}"),
    }
    assert_eq!(hub.usage_today(&local()).tokens(), 0);
}

#[tokio::test]
async fn concurrent_calls_cannot_both_pass_a_limit_only_one_fits() {
    let base = start_engine().await;
    let hub = Arc::new(hub_with(provider(&base, one_call_a_day()), 20_000));
    let client = Arc::new(hub.client(&local()).expect("client"));

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

#[tokio::test]
async fn a_streamed_turn_yields_its_pieces_and_settles_the_reported_usage() {
    let base = start_engine().await;
    let hub = hub_with(provider(&base, Budget::default()), 20_000);
    let client = hub.client(&local()).expect("client");

    let mut turn = client
        .open_turn(TurnRequest::new(conversation("Dis bonjour.")))
        .await
        .expect("tour ouvert");
    let mut pieces = Vec::new();
    while let Some(piece) = turn.next().await.expect("morceau") {
        pieces.push(piece);
    }
    assert_eq!(pieces, vec!["Bon".to_string(), "jour".to_string()]);
    assert_eq!(turn.received(), "Bonjour");

    let reply = turn.complete().expect("tour terminé");
    assert_eq!(reply.content, "Bonjour");
    assert_eq!(reply.finish, Finish::Stop);
    assert_eq!(reply.usage.input_tokens, 10, "l'usage annoncé par le flux");
    assert_eq!(hub.usage_today(&local()).tokens(), 15);
}

#[tokio::test]
async fn streamed_tool_calls_are_joined_from_their_fragments() {
    let base = start_engine().await;
    let hub = hub_with(provider(&base, Budget::default()), 20_000);
    let client = hub.client(&local()).expect("client");

    let mut turn = client
        .open_turn(TurnRequest::new(conversation("SCENARIO:tools")).with_tools(json!([])))
        .await
        .expect("tour ouvert");
    while turn.next().await.expect("morceau").is_some() {}
    let reply = turn.complete().expect("tour terminé");

    assert_eq!(reply.finish, Finish::ToolCalls);
    assert_eq!(reply.tool_calls.len(), 1);
    assert_eq!(reply.tool_calls[0]["id"], "call_1");
    assert_eq!(reply.tool_calls[0]["function"]["name"], "lire");
    assert_eq!(
        reply.tool_calls[0]["function"]["arguments"],
        "{\"path\":\"a\"}"
    );
}

#[tokio::test]
async fn a_streamed_turn_cut_by_the_output_cap_says_so() {
    let base = start_engine().await;
    let hub = hub_with(provider(&base, Budget::default()), 20_000);
    let client = hub.client(&local()).expect("client");

    let mut turn = client
        .open_turn(TurnRequest::new(conversation("SCENARIO:length")))
        .await
        .expect("tour ouvert");
    while turn.next().await.expect("morceau").is_some() {}
    let reply = turn.complete().expect("tour terminé");
    assert_eq!(reply.finish, Finish::Length);
}

#[tokio::test]
async fn a_stream_that_ends_without_done_is_an_error_not_an_answer() {
    let base = start_engine().await;
    let hub = hub_with(provider(&base, Budget::default()), 20_000);
    let client = hub.client(&local()).expect("client");

    let mut turn = client
        .open_turn(TurnRequest::new(conversation("SCENARIO:cut")))
        .await
        .expect("tour ouvert");
    let mut failure = None;
    loop {
        match turn.next().await {
            Ok(Some(_)) => {}
            Ok(None) => break,
            Err(error) => {
                failure = Some(error);
                break;
            }
        }
    }
    assert!(
        matches!(failure, Some(ProviderError::StreamCut)),
        "un flux coupé n'est pas une réponse : {failure:?}"
    );
    assert_eq!(turn.received(), "Bon", "ce qui est arrivé reste lisible");
}

#[tokio::test]
async fn dropping_a_streamed_turn_gives_its_reservation_back() {
    let base = start_engine().await;
    let hub = hub_with(provider(&base, one_call_a_day()), 20_000);
    let client = hub.client(&local()).expect("client");

    let turn = client
        .open_turn(TurnRequest::new(conversation("Dis bonjour.")))
        .await
        .expect("tour ouvert");
    assert!(
        hub.usage_today(&local()).tokens() > 0,
        "réservé pendant le tour"
    );
    drop(turn);
    assert_eq!(
        hub.usage_today(&local()).tokens(),
        0,
        "annulé : budget rendu"
    );

    // And the budget is usable again.
    client
        .chat(ChatRequest::new("Tu es bref.", "Dis bonjour."))
        .await
        .expect("le budget a été rendu");
}

#[tokio::test]
async fn a_whole_turn_with_tools_returns_the_assistant_message() {
    let base = start_engine().await;
    let hub = hub_with(provider(&base, Budget::default()), 20_000);
    let client = hub.client(&local()).expect("client");

    let reply = client
        .send_turn(TurnRequest::new(conversation("SCENARIO:tools")).with_tools(json!([])))
        .await
        .expect("réponse");
    assert_eq!(reply.finish, Finish::ToolCalls);
    assert_eq!(reply.tool_calls[0]["function"]["name"], "lire");
    assert_eq!(reply.model, "qwen-test");
}

/// Chaque panne arrive avec sa famille : l'hôte commute dessus au lieu de lire un message.
#[tokio::test]
async fn every_failure_arrives_with_its_family() {
    let base = start_engine().await;

    // Un statut d'erreur du moteur : Http, avec le statut.
    let mut config = provider(&base, Budget::default());
    config.model = Some("modele-absent".into());
    let hub = hub_with(config, 20_000);
    let client = hub.client(&local()).expect("client");
    let error = client
        .chat(ChatRequest::new("sys", "hi"))
        .await
        .expect_err("400");
    assert_eq!(error.kind(), ErrorKind::Http(400));
    assert_eq!(error.kind().code(), "http");
    assert_eq!(error.kind().status(), Some(400));

    // Une requête trop grosse pour la fenêtre : décision du provider, rien n'est parti.
    let hub = hub_with(provider(&base, Budget::default()), 20_000);
    let client = hub.client(&local()).expect("client");
    let mut grosse = ChatRequest::new("sys", "x".repeat(30_000));
    grosse.max_output_tokens = Some(64);
    let error = client.chat(grosse).await.expect_err("contexte");
    assert_eq!(error.kind(), ErrorKind::Provider);

    // Un budget épuisé : même famille.
    let hub = hub_with(provider(&base, one_call_a_day()), 20_000);
    let client = hub.client(&local()).expect("client");
    client
        .chat(ChatRequest::new("Tu es bref.", "Dis bonjour."))
        .await
        .expect("premier");
    let error = client
        .chat(ChatRequest::new("Tu es bref.", "Dis bonjour."))
        .await
        .expect_err("budget");
    assert_eq!(error.kind(), ErrorKind::Provider);

    // Un flux coupé : Truncated.
    let hub = hub_with(provider(&base, Budget::default()), 20_000);
    let client = hub.client(&local()).expect("client");
    let mut turn = client
        .open_turn(TurnRequest::new(conversation("SCENARIO:cut")))
        .await
        .expect("ouvert");
    let error = loop {
        match turn.next().await {
            Ok(Some(_)) => {}
            Ok(None) => panic!("un flux coupé ne finit pas proprement"),
            Err(error) => break error,
        }
    };
    assert_eq!(error.kind(), ErrorKind::Truncated);

    // Un moteur qui n'écoute pas : Unreachable.
    let closed = {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("port");
        let address = listener.local_addr().expect("adresse");
        drop(listener);
        format!("http://{address}/v1")
    };
    let mut config = provider(&closed, Budget::default());
    config.model = Some("qwen-test".into());
    let hub = hub_with(config, 20_000);
    let client = hub.client(&local()).expect("client");
    let error = client
        .chat(ChatRequest::new("sys", "hi"))
        .await
        .expect_err("injoignable");
    assert_eq!(error.kind(), ErrorKind::Unreachable);
}

/// The message of a transport failure names its cause: a refused connection says so, instead
/// of the generic "error sending request" that reqwest prints on its own.
#[tokio::test]
async fn a_transport_failure_says_why_it_failed() {
    let closed = {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("port");
        let address = listener.local_addr().expect("adresse");
        drop(listener);
        format!("http://{address}/v1")
    };
    let mut config = provider(&closed, Budget::default());
    config.model = Some("qwen-test".into());
    let hub = hub_with(config, 20_000);
    let client = hub.client(&local()).expect("client");
    let error = client
        .chat(ChatRequest::new("sys", "hi"))
        .await
        .expect_err("injoignable");
    let message = error.to_string();
    assert!(
        message.to_ascii_lowercase().contains("refused") || message.contains("refusé"),
        "la cause doit être dite, pas seulement « error sending request » : {message}"
    );
}
