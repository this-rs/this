//! A long answer is not a dead answer.
//!
//! Measured on 07/11/2026: the Atelier's turns stopped by themselves. The engine (llama-server,
//! 262144 tokens per slot) was far from full; the client carried `reqwest`'s TOTAL timeout
//! (`timeout_secs` = 120), which also bounds the streamed body. A local engine writing 4096
//! tokens at 53 tok/s needs 77 s for the output alone, plus the prompt: past 120 s the turn was
//! cut while the engine was still talking.
//!
//! These tests pin the contract: `timeout_secs` is the longest SILENCE tolerated, never the
//! length of a turn.

#![cfg(feature = "http")]

use std::time::{Duration, Instant};

use serde_json::json;
use this_llm::{CredentialRef, Preset};
use this_llm_providers::{
    Clock, Hub, MemoryStore, ProviderConfig, ProviderId, Secrets, TurnRequest,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

struct FixedClock;

impl Clock for FixedClock {
    fn day(&self) -> u64 {
        20_000
    }
}

/// How the stand-in engine behaves on `POST /v1/chat/completions`.
#[derive(Clone, Copy)]
enum Pace {
    /// `pieces` chunks, one every `gap`: alive all along, long in total.
    Steady { pieces: usize, gap: Duration },
    /// Headers, then nothing for `mute`, then one piece.
    Mute { mute: Duration },
}

async fn read_request(socket: &mut TcpStream) -> String {
    let mut raw = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        let read = socket.read(&mut chunk).await.unwrap_or(0);
        if read == 0 {
            break;
        }
        raw.extend_from_slice(&chunk[..read]);
        let text = String::from_utf8_lossy(&raw);
        if let Some(end) = text.find("\r\n\r\n") {
            let length = text[..end]
                .to_ascii_lowercase()
                .lines()
                .find_map(|line| line.strip_prefix("content-length:").map(str::to_string))
                .and_then(|value| value.trim().parse::<usize>().ok())
                .unwrap_or(0);
            if raw.len() >= end + 4 + length {
                break;
            }
        }
    }
    String::from_utf8_lossy(&raw).to_string()
}

fn event(content: &str, finish: Option<&str>) -> String {
    let body = json!({"choices": [{"index": 0, "delta": {"content": content}, "finish_reason": finish}]});
    format!("data: {body}\n\n")
}

async fn engine(listener: TcpListener, pace: Pace) {
    loop {
        let Ok((mut socket, _)) = listener.accept().await else {
            return;
        };
        tokio::spawn(async move {
            let request = read_request(&mut socket).await;
            if request.starts_with("GET /v1/models") {
                let body = json!({"data": [{"id": "qwen-test", "meta": {"n_ctx": 262144}}]})
                    .to_string();
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = socket.write_all(response.as_bytes()).await;
                let _ = socket.shutdown().await;
                return;
            }
            // A streamed answer with no length: the body ends when the socket closes.
            let head = "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n";
            if socket.write_all(head.as_bytes()).await.is_err() {
                return;
            }
            match pace {
                Pace::Steady { pieces, gap } => {
                    for index in 0..pieces {
                        tokio::time::sleep(gap).await;
                        if socket.write_all(event(&format!("m{index} "), None).as_bytes()).await.is_err() {
                            return;
                        }
                    }
                }
                Pace::Mute { mute } => {
                    tokio::time::sleep(mute).await;
                    let _ = socket.write_all(event("tard ", None).as_bytes()).await;
                }
            }
            let _ = socket.write_all(event("", Some("stop")).as_bytes()).await;
            let _ = socket.write_all(b"data: [DONE]\n\n").await;
            let _ = socket.shutdown().await;
        });
    }
}

async fn client_with(pace: Pace, timeout_secs: u64) -> this_llm_providers::ProviderClient {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("port libre");
    let address = listener.local_addr().expect("adresse");
    tokio::spawn(engine(listener, pace));
    let mut config = ProviderConfig::new(
        ProviderId::new("local").unwrap(),
        "Moteur lent",
        Preset::OpenAiCompatible,
        CredentialRef::None,
    );
    config.base_url = format!("http://{address}/v1");
    config.timeout_secs = timeout_secs;
    let hub = Hub::open(MemoryStore::default(), Secrets::env_only(), FixedClock).unwrap();
    hub.upsert(config.clone()).unwrap();
    hub.set_active(Some(&config.id)).unwrap();
    hub.client(&config.id).expect("client")
}

fn conversation() -> serde_json::Value {
    json!([{ "role": "user", "content": "Écris longtemps." }])
}

#[tokio::test]
async fn a_stream_longer_than_the_timeout_but_never_silent_runs_to_its_end() {
    // 8 pieces, 400 ms apart: 3.2 s in total, never more than 0.4 s of silence.
    let client = client_with(
        Pace::Steady {
            pieces: 8,
            gap: Duration::from_millis(400),
        },
        1,
    )
    .await;
    let started = Instant::now();
    let mut turn = client
        .open_turn(TurnRequest::new(conversation()))
        .await
        .expect("le tour s'ouvre");
    let mut text = String::new();
    while let Some(piece) = turn.next().await.expect("aucune coupure tant que le moteur parle") {
        text.push_str(&piece);
    }
    assert!(
        started.elapsed() > Duration::from_secs(2),
        "le tour a bien duré plus que le délai de 1 s ({:?})",
        started.elapsed()
    );
    assert_eq!(text, "m0 m1 m2 m3 m4 m5 m6 m7 ", "toute la réponse, rien de coupé");
}

#[tokio::test]
async fn a_mute_stream_is_still_cut_after_the_tolerated_silence() {
    // Headers at once, then 3 s of silence: longer than the 1 s tolerated.
    let client = client_with(
        Pace::Mute {
            mute: Duration::from_secs(3),
        },
        1,
    )
    .await;
    let started = Instant::now();
    let mut turn = client
        .open_turn(TurnRequest::new(conversation()))
        .await
        .expect("les en-têtes arrivent tout de suite");
    let mut outcome = Ok(());
    loop {
        match turn.next().await {
            Ok(Some(_)) => continue,
            Ok(None) => break,
            Err(error) => {
                outcome = Err(error);
                break;
            }
        }
    }
    assert!(outcome.is_err(), "un moteur muet reste coupé");
    assert!(
        started.elapsed() < Duration::from_millis(2_800),
        "coupé à la fin du silence toléré, pas à la fin du mutisme ({:?})",
        started.elapsed()
    );
}
