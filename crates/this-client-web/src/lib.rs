//! # Client gRPC-Web en wasm
//!
//! Un navigateur ne parle pas gRPC (HTTP/2 binaire) : il parle **gRPC-Web**, qui est du
//! gRPC emballé dans HTTP/1.1. Ce module fait le minimum honnête :
//!
//! - encodage d'un message `ListRequest { string entity = 1 }` ;
//! - framing gRPC-Web : `[compression: 1][longueur: 4 big-endian][message]` ;
//! - décodage d'un `ListReply { repeated string records_json = 1 }`, en sautant le cadre
//!   de trailers (bit `0x80`).
//!
//! Ce n'est pas une bibliothèque gRPC générale : c'est le strict nécessaire pour les deux
//! messages du service `Data`, et c'est ce qui permet de vérifier que le transport marche
//! avant de généraliser.

use gloo_net::http::Request;
use this_app::{Change, ChangeOp};
use wasm_bindgen::JsCast;
use wasm_bindgen_futures::spawn_local;

/// Encode un varint (comme le fait protobuf pour les longueurs).
fn varint(mut value: usize, out: &mut Vec<u8>) {
    loop {
        let byte = (value & 0x7f) as u8;
        value >>= 7;
        if value == 0 {
            out.push(byte);
            return;
        }
        out.push(byte | 0x80);
    }
}

/// Lit un varint et rend (valeur, octets consommés).
fn read_varint(bytes: &[u8], at: usize) -> Option<(usize, usize)> {
    let mut value = 0usize;
    let mut shift = 0;
    let mut read = 0;
    loop {
        let byte = *bytes.get(at + read)?;
        value |= ((byte & 0x7f) as usize) << shift;
        read += 1;
        if byte & 0x80 == 0 {
            return Some((value, read));
        }
        shift += 7;
        if shift > 28 {
            return None;
        }
    }
}

/// Emballe un message dans le framing gRPC-Web (non compressé).
pub fn frame(message: &[u8]) -> Vec<u8> {
    let mut framed = Vec::with_capacity(message.len() + 5);
    framed.push(0x00);
    framed.extend_from_slice(&(message.len() as u32).to_be_bytes());
    framed.extend_from_slice(message);
    framed
}

/// Extrait les messages d'une réponse gRPC-Web ; ignore le cadre de trailers.
pub fn unframe(bytes: &[u8]) -> Vec<&[u8]> {
    let mut messages = Vec::new();
    let mut at = 0;
    while at + 5 <= bytes.len() {
        let flags = bytes[at];
        let len = u32::from_be_bytes([bytes[at + 1], bytes[at + 2], bytes[at + 3], bytes[at + 4]])
            as usize;
        let start = at + 5;
        let end = start + len;
        if end > bytes.len() {
            break;
        }
        // 0x80 = cadre de trailers : ce ne sont pas des données
        if flags & 0x80 == 0 {
            messages.push(&bytes[start..end]);
        }
        at = end;
    }
    messages
}

/// `ListReply` → la liste des enregistrements JSON (champ 1 répété).
#[allow(dead_code)]
/// Construit un message protobuf champ par champ.
#[derive(Debug, Default)]
pub struct Message {
    bytes: Vec<u8>,
}

impl Message {
    pub fn new() -> Self {
        Self::default()
    }

    /// Champ `string` (wire type LEN). Les champs vides sont **omis**, comme le fait protobuf.
    pub fn string(mut self, field: u32, value: &str) -> Self {
        if !value.is_empty() {
            self.bytes.push(((field << 3) | 2) as u8);
            varint(value.len(), &mut self.bytes);
            self.bytes.extend_from_slice(value.as_bytes());
        }
        self
    }

    pub fn finish(self) -> Vec<u8> {
        self.bytes
    }
}

/// Décode un pourcentage (`grpc-message` est encodé ainsi, espaces et accents compris).
fn percent_decode(raw: &str) -> String {
    let bytes = raw.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut at = 0;
    while at < bytes.len() {
        if bytes[at] == b'%' && at + 2 < bytes.len() {
            let hex = std::str::from_utf8(&bytes[at + 1..at + 3]).unwrap_or("");
            if let Ok(byte) = u8::from_str_radix(hex, 16) {
                out.push(byte);
                at += 3;
                continue;
            }
        }
        out.push(bytes[at]);
        at += 1;
    }
    String::from_utf8_lossy(&out).to_string()
}

/// Extrait les trailers d'une réponse gRPC-Web (cadre marqué `0x80`).
fn trailers(bytes: &[u8]) -> String {
    let mut at = 0;
    while at + 5 <= bytes.len() {
        let flags = bytes[at];
        let len = u32::from_be_bytes([bytes[at + 1], bytes[at + 2], bytes[at + 3], bytes[at + 4]])
            as usize;
        let start = at + 5;
        let end = start + len;
        if end > bytes.len() {
            break;
        }
        if flags & 0x80 != 0 {
            return String::from_utf8_lossy(&bytes[start..end]).to_string();
        }
        at = end;
    }
    String::new()
}

/// Lit `grpc-status` / `grpc-message` dans un bloc d'en-têtes de trailers.
fn parse_trailers(text: &str) -> (String, Option<String>) {
    let mut message = None;
    let mut status = "0".to_string();
    for line in text.split("\r\n") {
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        match name.trim().to_ascii_lowercase().as_str() {
            "grpc-status" => status = value.trim().to_string(),
            "grpc-message" => message = Some(percent_decode(value.trim())),
            _ => {}
        }
    }
    (status, message)
}

/// Traduit un statut gRPC non nul en message du domaine.
fn refusal_from_parts(status: &str, message: Option<String>) -> Option<String> {
    if status == "0" || status.is_empty() {
        return None;
    }
    // le domaine a renvoyé le même JSON que la surface REST : on en garde le message
    let message = message.unwrap_or_else(|| format!("gRPC status {status}"));
    Some(
        serde_json::from_str::<serde_json::Value>(&message)
            .ok()
            .and_then(|value| value["message"].as_str().map(str::to_string))
            .unwrap_or(message),
    )
}

/// Appelle une méthode unaire et rend la charge utile du `JsonReply`.
pub async fn call(path: &str, request: Vec<u8>) -> Result<String, String> {
    let bytes = exchange(path, request).await?;
    let messages = unframe(&bytes);
    let Some(first) = messages.first() else {
        return Ok(String::new());
    };
    let fields = decode_strings(first);
    Ok(fields.into_iter().next().unwrap_or_default())
}

/// Appelle `List` et rend les enregistrements JSON.
pub async fn call_list(path: &str, entity: &str) -> Result<Vec<String>, String> {
    let bytes = exchange(path, Message::new().string(1, entity).finish()).await?;
    let messages = unframe(&bytes);
    let Some(first) = messages.first() else {
        return Ok(Vec::new());
    };
    Ok(decode_strings(first))
}

/// Envoie une trame gRPC-Web et vérifie les trailers.
async fn exchange(path: &str, message: Vec<u8>) -> Result<Vec<u8>, String> {
    let response = Request::post(path)
        .header("content-type", "application/grpc-web+proto")
        .header("x-grpc-web", "1")
        .body(frame(&message))
        .map_err(|e| e.to_string())?
        .send()
        .await
        .map_err(|e| e.to_string())?;

    let http_status = response.status();
    if !(200..300).contains(&http_status) {
        return Err(format!("gRPC-Web HTTP {http_status}"));
    }

    // Un refus arrive de **deux** façons, et il faut lire les deux :
    // - « trailers-only » : `grpc-status` dans les en-têtes HTTP, corps vide (c'est ce que
    //   fait tonic-web pour une erreur immédiate) ;
    // - `grpc-status` dans un cadre de trailers, à la fin du corps.
    let header_status = response
        .headers()
        .get("grpc-status")
        .unwrap_or_else(|| "0".to_string());
    let header_message = response
        .headers()
        .get("grpc-message")
        .map(|raw| percent_decode(&raw));

    let bytes = response.binary().await.map_err(|e| e.to_string())?;

    if let Some(refusal) = refusal_from_parts(&header_status, header_message) {
        return Err(refusal);
    }
    let (body_status, body_message) = parse_trailers(&trailers(&bytes));
    if let Some(refusal) = refusal_from_parts(&body_status, body_message) {
        return Err(refusal);
    }
    Ok(bytes)
}

/// Les champs `string` d'un message, dans l'ordre.
fn decode_strings(message: &[u8]) -> Vec<String> {
    let mut out = Vec::new();
    let mut at = 0;
    while at < message.len() {
        let Some((tag, tag_len)) = read_varint(message, at) else {
            break;
        };
        at += tag_len;
        let wire = tag & 0x07;
        match wire {
            2 => {
                let Some((len, len_len)) = read_varint(message, at) else {
                    break;
                };
                at += len_len;
                let end = at + len;
                if end > message.len() {
                    break;
                }
                if let Ok(text) = std::str::from_utf8(&message[at..end]) {
                    out.push(text.to_string());
                }
                at = end;
            }
            0 => {
                let Some((_, n)) = read_varint(message, at) else {
                    break;
                };
                at += n;
            }
            1 => at += 8,
            5 => at += 4,
            _ => break,
        }
    }
    out
}

/// Appelle `shop.data.v1.Data/List` en gRPC-Web et rend les enregistrements décodés.
pub async fn list(path: &str, entity: &str) -> Result<Vec<serde_json::Value>, String> {
    call_list(path, entity)
        .await?
        .iter()
        .map(|raw| serde_json::from_str(raw).map_err(|e| e.to_string()))
        .collect()
}

// ---- Flux : gRPC-Web server streaming ------------------------------------------------

/// Décode un `Change` (entity, id, op, record_json).
fn decode_change(message: &[u8]) -> Option<Change> {
    let mut entity = String::new();
    let mut id = String::new();
    let mut op = 0usize;
    let mut record = None;
    let mut at = 0;
    while at < message.len() {
        let (tag, tag_len) = read_varint(message, at)?;
        at += tag_len;
        let field = tag >> 3;
        match tag & 0x07 {
            2 => {
                let (len, len_len) = read_varint(message, at)?;
                at += len_len;
                let end = at + len;
                if end > message.len() {
                    return None;
                }
                let text = std::str::from_utf8(&message[at..end]).ok()?.to_string();
                match field {
                    1 => entity = text,
                    2 => id = text,
                    4 => record = Some(text),
                    _ => {}
                }
                at = end;
            }
            0 => {
                let (value, n) = read_varint(message, at)?;
                at += n;
                if field == 3 {
                    op = value;
                }
            }
            _ => return None,
        }
    }
    let op = match op {
        1 => ChangeOp::Created,
        2 => ChangeOp::Updated,
        3 => ChangeOp::Deleted,
        // un op inconnu vaut mieux traité comme une mise à jour que perdu
        _ => ChangeOp::Updated,
    };
    Some(Change {
        entity,
        id,
        op,
        record: record.and_then(|raw| serde_json::from_str(&raw).ok()),
    })
}

thread_local! {
    /// Génération du flux courant. Un nouvel abonnement incrémente la génération : l'ancien
    /// flux s'arrête à son prochain fragment, au lieu de doubler les événements.
    static GENERATION: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
    /// Lecteur du flux courant : un flux vivant ne produit pas de fragment, sa lecture reste
    /// en attente. Pour le libérer, il faut l'annuler au moment de l'abonnement suivant.
    static LIVE_READER: std::cell::RefCell<Option<web_sys::ReadableStreamDefaultReader>> =
        const { std::cell::RefCell::new(None) };
}

/// S'abonne au flux d'événements du module en gRPC-Web.
///
/// `tonic-web` répond à un RPC de streaming par un corps HTTP unique : on lit le
/// `ReadableStream` et on découpe les trames au fur et à mesure, sans attendre la fin —
/// qui n'arrive jamais sur un flux vivant.
pub fn subscribe(
    path: &str,
    request: Vec<u8>,
    on_change: impl Fn(Change) + 'static,
    on_error: impl Fn(String) + 'static,
) {
    let mine = GENERATION.with(|g| {
        let next = g.get() + 1;
        g.set(next);
        next
    });
    // l'ancien flux est fermé tout de suite, sans attendre un événement qui ne viendra peut-être pas
    LIVE_READER.with(|r| {
        if let Some(previous) = r.borrow_mut().take() {
            let _ = previous.cancel();
        }
    });
    let path = path.to_string();
    spawn_local(async move {
        if let Err(error) = stream(&path, request, mine, &on_change).await {
            // une erreur d'un flux remplacé n'est pas une panne : on ne la signale pas
            if GENERATION.with(|g| g.get()) == mine {
                on_error(error);
            }
        }
    });
}

async fn stream(
    path: &str,
    request: Vec<u8>,
    mine: u64,
    on_change: &impl Fn(Change),
) -> Result<(), String> {
    let response = Request::post(path)
        .header("content-type", "application/grpc-web+proto")
        .header("x-grpc-web", "1")
        .body(frame(&request))
        .map_err(|e| e.to_string())?
        .send()
        .await
        .map_err(|e| e.to_string())?;

    if !(200..300).contains(&response.status()) {
        return Err(format!("flux gRPC-Web : HTTP {}", response.status()));
    }
    let body = response
        .body()
        .ok_or("flux gRPC-Web : réponse sans corps")?;
    let reader = body
        .get_reader()
        .dyn_into::<web_sys::ReadableStreamDefaultReader>()
        .map_err(|_| "flux gRPC-Web : lecteur indisponible".to_string())?;
    LIVE_READER.with(|r| *r.borrow_mut() = Some(reader.clone()));

    let mut buffer: Vec<u8> = Vec::new();
    loop {
        if GENERATION.with(|g| g.get()) != mine {
            // un abonnement plus récent a pris la suite : on libère ce flux et on s'arrête
            let _ = reader.cancel();
            return Ok(());
        }
        let chunk = wasm_bindgen_futures::JsFuture::from(reader.read())
            .await
            .map_err(|e| format!("flux gRPC-Web : lecture interrompue : {e:?}"))?;
        let done = js_sys::Reflect::get(&chunk, &wasm_bindgen::JsValue::from_str("done"))
            .ok()
            .and_then(|value| value.as_bool())
            .unwrap_or(false);

        if let Ok(value) = js_sys::Reflect::get(&chunk, &wasm_bindgen::JsValue::from_str("value")) {
            if !value.is_undefined() {
                buffer.extend_from_slice(&js_sys::Uint8Array::new(&value).to_vec());
            }
        }

        // on consomme toutes les trames complètes déjà reçues
        while buffer.len() >= 5 {
            let flags = buffer[0];
            let len = u32::from_be_bytes([buffer[1], buffer[2], buffer[3], buffer[4]]) as usize;
            if buffer.len() < 5 + len {
                break;
            }
            let payload = buffer[5..5 + len].to_vec();
            buffer.drain(..5 + len);

            if flags & 0x80 != 0 {
                // cadre de trailers : fin de flux ou erreur
                let (status, message) = parse_trailers(&String::from_utf8_lossy(&payload));
                if let Some(refusal) = refusal_from_parts(&status, message) {
                    return Err(refusal);
                }
                continue;
            }
            if let Some(change) = decode_change(&payload) {
                on_change(change);
            }
        }

        if done {
            return Ok(());
        }
    }
}
