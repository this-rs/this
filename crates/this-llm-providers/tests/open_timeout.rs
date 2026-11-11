//! The OPENING delay of a streamed answer is not the SILENCE delay of a stream that has begun.
//!
//! A model that a catalogue lists but an account cannot call accepts the request and never
//! opens its answer. With only `timeout_secs` (120 s) the turn stayed mute for two minutes
//! and ended on reqwest's raw "operation timed out". `open_timeout_secs` (30 s by default)
//! bounds the wait for the headers; a stream that has begun keeps the silence limit.

#![cfg(feature = "http")]

use std::time::{Duration, Instant};

use serde_json::json;
use this_llm::{CredentialRef, Preset};
use this_llm_providers::{
    ErrorKind, Hub, MemoryStore, ProviderConfig, ProviderId, Secrets, TurnRequest,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

const KEY: &str = "nvapi-TESTKEY-do-not-leak-0123456789";

/// Accepts every connection, reads the request, and answers `/models`, then either says nothing to the chat (`hang`) or opens a
/// short stream.
async fn engine(listener: TcpListener, hang: bool) {
    loop {
        let Ok((mut socket, _)) = listener.accept().await else {
            return;
        };
        tokio::spawn(async move {
            let mut raw = vec![0u8; 8192];
            let read = socket.read(&mut raw).await.unwrap_or(0);
            let request = String::from_utf8_lossy(&raw[..read]).to_string();
            if request.starts_with("GET /v1/models") {
                let body = json!({"data": [{"id": "qwen-test"}]}).to_string();
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = socket.write_all(response.as_bytes()).await;
                let _ = socket.shutdown().await;
                return;
            }
            if hang {
                tokio::time::sleep(Duration::from_secs(60)).await;
                return;
            }
            let body = format!(
                "data: {}\n\ndata: [DONE]\n\n",
                json!({"choices": [{"index": 0, "delta": {"content": "Bonjour"}, "finish_reason": "stop"}]})
            );
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = socket.write_all(response.as_bytes()).await;
            let _ = socket.shutdown().await;
        });
    }
}

async fn client(hang: bool, open: Option<u64>, silence: u64) -> this_llm_providers::ProviderClient {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}/v1", listener.local_addr().unwrap());
    tokio::spawn(engine(listener, hang));
    let id = ProviderId::new("local").unwrap();
    let mut config = ProviderConfig::new(
        id.clone(),
        "Moteur muet",
        Preset::OpenAiCompatible,
        CredentialRef::Vault("k".to_string()),
    );
    config.base_url = base;
    config.model = Some("qwen-test".to_string());
    config.timeout_secs = silence;
    config.open_timeout_secs = open;
    struct Day;
    impl this_llm_providers::Clock for Day {
        fn day(&self) -> u64 {
            20_000
        }
    }
    let hub = Hub::open(
        MemoryStore::default(),
        Secrets::with_vault(|_| Some(KEY.to_string())),
        Day,
    )
    .unwrap();
    hub.upsert(config).unwrap();
    hub.client(&id).expect("client")
}

fn request() -> TurnRequest {
    TurnRequest::new(json!([{ "role": "user", "content": "bonjour" }]))
}

#[tokio::test]
async fn an_engine_that_never_opens_fails_at_the_open_timeout_not_the_silence_one() {
    let client = client(true, Some(1), 60).await;
    let started = Instant::now();
    let error = match client.open_turn(request()).await {
        Err(error) => error,
        Ok(_) => panic!("l'ouverture aurait dû échouer"),
    };
    let waited = started.elapsed();
    assert!(
        waited >= Duration::from_millis(900) && waited < Duration::from_secs(5),
        "{waited:?}"
    );
    assert_eq!(error.kind(), ErrorKind::Timeout);
    let text = error.to_string();
    assert!(text.contains("aucune réponse à l'ouverture"), "{text}");
    assert!(
        text.contains("Moteur muet") && text.contains("qwen-test") && text.contains("1 s"),
        "{text}"
    );
    assert!(!text.contains("operation timed out"), "{text}");
    assert!(!text.contains(KEY), "{text}");
}

#[tokio::test]
async fn without_an_open_timeout_the_shorter_of_thirty_seconds_and_the_silence_limit_applies() {
    // No `open_timeout_secs`, a silence limit of 2 s: the opening is bounded at 2 s, not 30.
    let client = client(true, None, 2).await;
    let started = Instant::now();
    let error = match client.open_turn(request()).await {
        Err(error) => error,
        Ok(_) => panic!("l'ouverture aurait dû échouer"),
    };
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "{:?}",
        started.elapsed()
    );
    assert!(
        error
            .to_string()
            .contains("aucune réponse à l'ouverture en 2 s"),
        "{error}"
    );
}

#[tokio::test]
async fn a_healthy_stream_opens_well_inside_the_open_timeout() {
    let client = client(false, Some(1), 60).await;
    let mut turn = client.open_turn(request()).await.expect("ouvert");
    let mut text = String::new();
    while let Some(piece) = turn.next().await.expect("morceau") {
        text.push_str(&piece);
    }
    assert_eq!(text, "Bonjour");
}
