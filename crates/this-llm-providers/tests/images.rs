//! Images, checked on the body that really leaves on the socket: a stand-in engine keeps
//! every `POST /v1/chat/completions` it receives, and the tests read them back.

#![cfg(feature = "http")]

use std::sync::{Arc, Mutex};

use serde_json::{Value, json};
use this_llm::{CredentialRef, Preset};
use this_llm_providers::{
    Clock, Hub, MemoryStore, ProviderConfig, ProviderError, ProviderId, Secrets, TurnRequest,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

struct FixedClock;

impl Clock for FixedClock {
    fn day(&self) -> u64 {
        20_000
    }
}

type Posted = Arc<Mutex<Vec<Value>>>;

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

async fn engine(listener: TcpListener, posted: Posted) {
    loop {
        let Ok((mut socket, _)) = listener.accept().await else {
            return;
        };
        let posted = posted.clone();
        tokio::spawn(async move {
            let mut raw = Vec::new();
            let mut chunk = [0u8; 65536];
            let mut body_at = None;
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
                    body_at = Some(end + 4);
                    if raw.len() >= end + 4 + length {
                        break;
                    }
                }
            }
            let answer = if raw.starts_with(b"GET /v1/models") {
                json!({"data": [{"id": "vision-test", "max_model_len": 8192}]})
            } else {
                if let Some(at) = body_at
                    && let Ok(body) = serde_json::from_slice::<Value>(&raw[at..])
                {
                    posted.lock().unwrap().push(body);
                }
                json!({
                    "choices": [{"message": {"role": "assistant", "content": "vu"}, "finish_reason": "stop"}],
                    "usage": {"prompt_tokens": 10, "completion_tokens": 1}
                })
            }
            .to_string();
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{answer}",
                answer.len()
            );
            let _ = socket.write_all(response.as_bytes()).await;
            let _ = socket.shutdown().await;
        });
    }
}

async fn start(vision: Option<bool>) -> (Hub, ProviderId, Posted) {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("port libre");
    let address = listener.local_addr().expect("adresse locale");
    let posted: Posted = Arc::default();
    tokio::spawn(engine(listener, posted.clone()));
    let id = ProviderId::new("local").unwrap();
    let mut config = ProviderConfig::new(
        id.clone(),
        "Moteur de test",
        Preset::OpenAiCompatible,
        CredentialRef::None,
    );
    config.base_url = format!("http://{address}/v1");
    config.vision = vision;
    let hub = Hub::open(MemoryStore::default(), Secrets::env_only(), FixedClock).unwrap();
    hub.upsert(config).unwrap();
    hub.set_active(Some(&id)).unwrap();
    (hub, id, posted)
}

fn with_image(source: Value) -> Value {
    json!([
        { "role": "system", "content": "brief" },
        { "role": "user", "content": "que vois-tu ?", "images": [
            { "mime": "image/png", "source": source, "name": "photo.png" }
        ] },
    ])
}

#[tokio::test]
async fn a_text_only_conversation_leaves_exactly_as_it_was_given() {
    let (hub, id, posted) = start(None).await;
    let messages = json!([
        { "role": "system", "content": "brief" },
        { "role": "user", "content": "bonjour" },
    ]);
    let reply = hub
        .client(&id)
        .unwrap()
        .send_turn(TurnRequest::new(messages.clone()))
        .await
        .expect("tour");
    assert_eq!(reply.content, "vu");
    let posted = posted.lock().unwrap();
    assert_eq!(posted.len(), 1);
    // Byte for byte: the posted array is the given one, re-serialized identically.
    assert_eq!(posted[0]["messages"].to_string(), messages.to_string());
}

#[tokio::test]
async fn a_base64_image_leaves_as_an_openai_image_url_part() {
    let (hub, id, posted) = start(None).await;
    // About 1 MiB of payload against an 8192-token window: the flat image estimate keeps
    // it from being refused as "too long", which counting characters would do.
    let payload = "A".repeat(1_400_000);
    hub.client(&id)
        .unwrap()
        .send_turn(TurnRequest::new(with_image(json!({ "base64": payload }))))
        .await
        .expect("tour avec image");
    let posted = posted.lock().unwrap();
    assert_eq!(posted.len(), 1);
    let user = &posted[0]["messages"][1];
    assert_eq!(user["role"], "user");
    assert!(user.get("images").is_none(), "la forme neutre ne part pas");
    let parts = user["content"].as_array().expect("content en parties");
    assert_eq!(parts[0], json!({ "type": "text", "text": "que vois-tu ?" }));
    assert_eq!(parts[1]["type"], "image_url");
    let url = parts[1]["image_url"]["url"].as_str().unwrap();
    assert_eq!(url, format!("data:image/png;base64,{payload}"));
    assert_eq!(
        posted[0]["messages"][0],
        json!({ "role": "system", "content": "brief" })
    );
}

#[tokio::test]
async fn a_provider_without_vision_refuses_and_nothing_is_posted() {
    let (hub, id, posted) = start(Some(false)).await;
    let error = hub
        .client(&id)
        .unwrap()
        .send_turn(TurnRequest::new(with_image(json!({ "base64": "AAAA" }))))
        .await
        .unwrap_err();
    assert!(matches!(error, ProviderError::Images(_)), "{error}");
    assert!(error.to_string().contains("sans vision"), "{error}");
    assert!(posted.lock().unwrap().is_empty());
}

#[tokio::test]
async fn an_unresolved_reference_refuses_and_nothing_is_posted() {
    let (hub, id, posted) = start(None).await;
    let error = hub
        .client(&id)
        .unwrap()
        .open_turn(TurnRequest::new(with_image(
            json!({ "ref": "ab".repeat(32) }),
        )))
        .await
        .err()
        .expect("refus");
    assert!(error.to_string().contains("non résolue"), "{error}");
    assert!(posted.lock().unwrap().is_empty());
}
