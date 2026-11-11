//! `/models` is asked at every turn, only to learn the window of the model — and it was the
//! one request of the turn bounded by nothing but the SILENCE delay (`timeout_secs`, 120 s).
//! Measured on 11/11/2026: a saturated local relay answered `/models` in tens of seconds, and
//! the turn was refused "at the opening" before the opening had even been tried.
//!
//! Two rules: the catalogue call is bounded by the OPENING delay, and a configured model whose
//! window is configured too asks the engine nothing before the turn.

#![cfg(feature = "http")]

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use serde_json::json;
use this_llm::{CredentialRef, Preset};
use this_llm_providers::{
    Hub, MemoryStore, ProviderClient, ProviderConfig, ProviderId, Secrets, TurnRequest,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

const KEY: &str = "nvapi-TESTKEY-do-not-leak-0123456789";

/// An engine whose catalogue HANGS (`/models` never answers) while the chat opens at once.
/// Counts the `/models` requests it receives.
async fn engine(listener: TcpListener, models_seen: Arc<AtomicUsize>) {
    loop {
        let Ok((mut socket, _)) = listener.accept().await else {
            return;
        };
        let models_seen = models_seen.clone();
        tokio::spawn(async move {
            let mut raw = vec![0u8; 8192];
            let read = socket.read(&mut raw).await.unwrap_or(0);
            let request = String::from_utf8_lossy(&raw[..read]).to_string();
            if request.starts_with("GET /v1/models") {
                models_seen.fetch_add(1, Ordering::SeqCst);
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

async fn client(window: Option<u32>) -> (ProviderClient, Arc<AtomicUsize>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}/v1", listener.local_addr().unwrap());
    let models_seen = Arc::new(AtomicUsize::new(0));
    tokio::spawn(engine(listener, models_seen.clone()));
    let id = ProviderId::new("local").unwrap();
    let mut config = ProviderConfig::new(
        id.clone(),
        "Catalogue muet",
        Preset::OpenAiCompatible,
        CredentialRef::Vault("k".to_string()),
    );
    config.base_url = base;
    config.model = Some("qwen-test".to_string());
    config.context_window = window;
    // The silence delay stays long: it must NOT be what bounds the catalogue.
    config.timeout_secs = 120;
    config.open_timeout_secs = Some(1);
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
    (hub.client(&id).expect("client"), models_seen)
}

fn request() -> TurnRequest {
    TurnRequest::new(json!([{ "role": "user", "content": "bonjour" }]))
}

/// A hanging catalogue costs the OPENING delay, not the silence delay: the turn then opens
/// on the configured model, with an unknown window.
#[tokio::test]
async fn un_catalogue_qui_pend_ne_retient_le_tour_que_le_delai_d_ouverture() {
    let (client, models_seen) = client(None).await;
    let started = Instant::now();
    let turn = client.open_turn(request()).await.expect("the chat opens");
    let elapsed = started.elapsed();
    assert_eq!(turn.model(), "qwen-test");
    assert_eq!(
        models_seen.load(Ordering::SeqCst),
        1,
        "the catalogue was asked once"
    );
    assert!(
        elapsed >= Duration::from_millis(900) && elapsed < Duration::from_secs(5),
        "bounded by the opening delay (1 s), not the silence delay (120 s): {elapsed:?}"
    );
}

/// A configured model with a configured window asks the engine nothing before the turn.
#[tokio::test]
async fn un_modele_et_une_fenetre_configures_n_appellent_pas_le_catalogue() {
    let (client, models_seen) = client(Some(32_768)).await;
    let started = Instant::now();
    let turn = client.open_turn(request()).await.expect("the chat opens");
    assert_eq!(turn.model(), "qwen-test");
    assert_eq!(
        models_seen.load(Ordering::SeqCst),
        0,
        "no `/models` request at all"
    );
    assert!(
        started.elapsed() < Duration::from_millis(900),
        "no wait: {:?}",
        started.elapsed()
    );
}
