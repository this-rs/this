//! The images of a conversation, from the neutral form a host hands over to the dialect
//! the engine reads.
//!
//! ## The neutral form
//!
//! A [`TurnRequest`](crate::TurnRequest) carries the `messages` array as JSON. A user
//! message may carry, next to its `content`, an `images` array of
//! [`ImagePart`](this_llm::ImagePart) (`{ mime, source: { base64 | url | ref }, name? }`).
//! A message without images has no `images` key and goes out exactly as it came in.
//!
//! ## The three steps, and who owns each
//!
//! 1. [`keep_recent_images`] (host policy): a thread re-sends every image at every
//!    turn; beyond a ceiling the oldest are replaced by a written mention.
//! 2. [`resolve_refs`] (host): a `ref` source is a name only the host can read (a blob
//!    digest). The host turns it into `base64`, or says why it cannot: the image is then
//!    replaced by a written mention in the message — the turn goes on, and the model is
//!    told that an image is missing.
//! 3. [`render_openai`] (this crate, when the body is built): `content` becomes the
//!    OpenAI parts array, **only** for messages that carry images.
//!
//! Nothing here drops an image silently: an image is sent, or replaced by a sentence that
//! names it, or the whole request is refused with a readable reason.

use serde_json::{Value, json};
use this_llm::{ImagePart, ImageSource};

use crate::error::ProviderError;

/// The key of the neutral images array in a user message.
pub const IMAGES_KEY: &str = "images";

/// What an image is assumed to cost in the context window, in tokens.
///
/// A heuristic, not a measure: engines count images differently (a fixed tile budget,
/// a count that grows with the resolution). It errs on the expensive side, like
/// [`estimate_tokens`](crate::estimate_tokens), because the estimate only decides what is
/// refused before sending. Counting the base64 text instead would refuse any photo:
/// 1 MiB of image is 1.4 M characters, far more than any window.
pub const IMAGE_TOKENS_ESTIMATE: u32 = 1_600;

fn images_of(message: &Value) -> Option<&Vec<Value>> {
    message.get(IMAGES_KEY).and_then(Value::as_array)
}

fn label_of(raw: &Value) -> String {
    raw.get("name")
        .and_then(Value::as_str)
        .unwrap_or("image")
        .to_string()
}

/// Appends a sentence to a message's text content.
fn mention(message: &mut Value, sentence: &str) {
    let text = match message.get("content").and_then(Value::as_str) {
        Some(content) if !content.is_empty() => format!("{content}\n{sentence}"),
        _ => sentence.to_string(),
    };
    message["content"] = Value::String(text);
}

/// Removes an `images` array left empty: the message is then text-only again, byte for
/// byte.
fn tidy(message: &mut Value) {
    if images_of(message).is_some_and(Vec::is_empty)
        && let Some(object) = message.as_object_mut()
    {
        object.remove(IMAGES_KEY);
    }
}

/// How many images the conversation carries.
pub fn count_images(messages: &Value) -> usize {
    messages
        .as_array()
        .map(|all| all.iter().filter_map(images_of).map(Vec::len).sum())
        .unwrap_or(0)
}

/// Keeps the `max` most recent images of the conversation; each older one is replaced by
/// a sentence in its own message. Returns how many were set aside.
pub fn keep_recent_images(messages: &mut Value, max: usize) -> usize {
    let Some(all) = messages.as_array_mut() else {
        return 0;
    };
    let mut kept = 0usize;
    let mut set_aside = 0usize;
    for message in all.iter_mut().rev() {
        let Some(images) = images_of(message).cloned() else {
            continue;
        };
        // Within one message too, the last images are the most recent.
        let room = max.saturating_sub(kept);
        let cut = images.len().saturating_sub(room);
        kept += images.len() - cut;
        if cut == 0 {
            continue;
        }
        for raw in &images[..cut] {
            mention(
                message,
                &format!(
                    "[image non renvoyée : {} — au-delà de {max} images par requête, seules les plus récentes partent]",
                    label_of(raw)
                ),
            );
        }
        set_aside += cut;
        message[IMAGES_KEY] = Value::Array(images[cut..].to_vec());
        tidy(message);
    }
    set_aside
}

/// Turns every `ref` source into bytes, through the host's `resolve`: it receives the part
/// and its reference, and returns the resolved part (a `base64` source, and the mime the
/// host read from the bytes themselves) or the reason it cannot.
///
/// An image the host cannot read is replaced by `[image indisponible : <name> — <reason>]`
/// in its message: the turn is not refused for a file that vanished, and the model is not
/// left to believe it saw everything. Returns how many images became unavailable.
pub fn resolve_refs(
    messages: &mut Value,
    mut resolve: impl FnMut(&ImagePart, &str) -> Result<ImagePart, String>,
) -> usize {
    let Some(all) = messages.as_array_mut() else {
        return 0;
    };
    let mut missing = 0usize;
    for message in all.iter_mut() {
        let Some(images) = images_of(message).cloned() else {
            continue;
        };
        let mut out = Vec::with_capacity(images.len());
        for raw in images {
            // A part that does not read is kept as is: the renderer refuses it, loudly.
            let Ok(part) = serde_json::from_value::<ImagePart>(raw.clone()) else {
                out.push(raw);
                continue;
            };
            let ImageSource::Ref(reference) = &part.source else {
                out.push(raw);
                continue;
            };
            match resolve(&part, &reference.clone()) {
                Ok(resolved) => out.push(serde_json::to_value(&resolved).unwrap_or(raw)),
                Err(reason) => {
                    missing += 1;
                    mention(
                        message,
                        &format!("[image indisponible : {} — {reason}]", part.label()),
                    );
                }
            }
        }
        message[IMAGES_KEY] = Value::Array(out);
        tidy(message);
    }
    missing
}

/// Removes every image of the conversation; each one is replaced by `sentence` in its own
/// message. Returns how many were removed.
///
/// For an engine that does not read images: the turn goes on as text, and the model is
/// told, image by image, that something was joined that it cannot see.
pub fn strip_images(messages: &mut Value, sentence: &str) -> usize {
    let Some(all) = messages.as_array_mut() else {
        return 0;
    };
    let mut removed = 0usize;
    for message in all.iter_mut() {
        let Some(count) = images_of(message).map(Vec::len) else {
            continue;
        };
        for _ in 0..count {
            mention(message, sentence);
        }
        removed += count;
        if let Some(object) = message.as_object_mut() {
            object.remove(IMAGES_KEY);
        }
    }
    removed
}

/// What engines say when a request carries images and the model behind them reads none.
///
/// Each pattern is one seen verbatim, not guessed; matched without regard to case:
/// - llama.cpp `llama-server` without `--mmproj`: `image input is not supported - hint: if
///   this is unexpected, you may need to provide the mmproj` (HTTP 500), and on the
///   `multimodal_data` path `Multimodal data provided, but model does not support
///   multimodal requests.` Both strings read in `libllama-server-impl` of the Homebrew
///   build 0.0.9910.
/// - OpenAI, for a model without image input: `Invalid content type. image_url is only
///   supported by certain models.` (HTTP 400, reported on the OpenAI community forum).
pub const IMAGE_REFUSALS: [&str; 3] = [
    "image input is not supported",
    "does not support multimodal requests",
    "image_url is only supported by certain models",
];

/// Whether an error body says that the engine does not read images.
///
/// Only meaningful for a request that carried images: the caller checks that first, so a
/// text-only request that fails stays a plain status error.
pub fn refuses_images(body: &str) -> bool {
    let body = body.to_lowercase();
    IMAGE_REFUSALS.iter().any(|pattern| body.contains(pattern))
}

/// The engine's own sentence in an error body: `error.message` for an OpenAI-shaped body,
/// the body itself otherwise.
pub(crate) fn error_message(body: &str) -> String {
    serde_json::from_str::<Value>(body)
        .ok()
        .and_then(|value| {
            value
                .pointer("/error/message")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .unwrap_or_else(|| body.trim().to_string())
}

/// The conversation without its image payloads, for the token estimate.
pub(crate) fn without_images(messages: &Value) -> Value {
    let mut copy = messages.clone();
    if let Some(all) = copy.as_array_mut() {
        for message in all.iter_mut() {
            if let Some(object) = message.as_object_mut() {
                object.remove(IMAGES_KEY);
            }
        }
    }
    copy
}

/// The `messages` array in the OpenAI dialect.
///
/// A message without images is copied unchanged. A user message with images gets
/// `content: [{type:"text",text}, {type:"image_url",image_url:{url}}…]`, the text part
/// omitted when the text is empty. Refused, with the reason, when:
/// - `accepts_images` is `Some(false)` (the provider is declared without vision);
/// - an image is still a `ref` (the host did not resolve it);
/// - an image is malformed, not `image/*`, or attached to a non-user message.
pub fn render_openai(
    messages: &Value,
    provider: &str,
    accepts_images: Option<bool>,
) -> Result<Value, ProviderError> {
    let total = count_images(messages);
    if total == 0 {
        return Ok(messages.clone());
    }
    if accepts_images == Some(false) {
        return Err(ProviderError::Images(format!(
            "le provider `{provider}` est déclaré sans vision (`vision: false`) : {total} image(s) jointe(s) ne peuvent pas lui être envoyées. Choisis un provider qui lit les images, ou retire-les."
        )));
    }
    let Some(all) = messages.as_array() else {
        return Ok(messages.clone());
    };
    let mut out = Vec::with_capacity(all.len());
    for message in all {
        let Some(images) = images_of(message) else {
            out.push(message.clone());
            continue;
        };
        let role = message.get("role").and_then(Value::as_str).unwrap_or("");
        if role != "user" {
            return Err(ProviderError::Images(format!(
                "des images sont jointes à un message `{role}` : seul un message utilisateur en porte"
            )));
        }
        let mut parts = Vec::with_capacity(images.len() + 1);
        if let Some(text) = message.get("content").and_then(Value::as_str)
            && !text.is_empty()
        {
            parts.push(json!({ "type": "text", "text": text }));
        }
        for raw in images {
            let part: ImagePart = serde_json::from_value(raw.clone()).map_err(|error| {
                ProviderError::Images(format!("image `{}` mal formée : {error}", label_of(raw)))
            })?;
            if !part.mime.starts_with("image/") {
                return Err(ProviderError::Images(format!(
                    "`{}` n'est pas une image (type `{}`)",
                    part.label(),
                    part.mime
                )));
            }
            let url = match &part.source {
                ImageSource::Base64(data) => format!("data:{};base64,{data}", part.mime),
                ImageSource::Url(url) => url.clone(),
                ImageSource::Ref(reference) => {
                    return Err(ProviderError::Images(format!(
                        "image `{}` non résolue : la référence `{reference}` n'a pas été lue par l'hôte avant l'envoi",
                        part.label()
                    )));
                }
            };
            parts.push(json!({ "type": "image_url", "image_url": { "url": url } }));
        }
        let mut rendered = message.clone();
        if let Some(object) = rendered.as_object_mut() {
            object.remove(IMAGES_KEY);
        }
        rendered["content"] = Value::Array(parts);
        out.push(rendered);
    }
    Ok(Value::Array(out))
}

#[cfg(test)]
mod tests {
    use super::*;

    const SHA: &str = "abababababababababababababababababababababababababababababababab";

    fn thread_with(images: Value) -> Value {
        json!([
            { "role": "system", "content": "brief" },
            { "role": "user", "content": "regarde", "images": images },
        ])
    }

    #[test]
    fn a_text_only_conversation_is_rendered_unchanged_byte_for_byte() {
        let messages = json!([
            { "role": "system", "content": "brief" },
            { "role": "user", "content": "q" },
            { "role": "assistant", "content": null, "tool_calls": [] },
            { "role": "tool", "tool_call_id": "c1", "content": "ok" },
        ]);
        let rendered = render_openai(&messages, "p", Some(false)).unwrap();
        assert_eq!(rendered.to_string(), messages.to_string());
    }

    #[test]
    fn a_base64_image_becomes_an_openai_data_url_part() {
        let messages = thread_with(json!([
            { "mime": "image/png", "source": { "base64": "iVBORw0K" }, "name": "a.png" }
        ]));
        let rendered = render_openai(&messages, "p", None).unwrap();
        assert_eq!(
            rendered,
            json!([
                { "role": "system", "content": "brief" },
                { "role": "user", "content": [
                    { "type": "text", "text": "regarde" },
                    { "type": "image_url", "image_url": { "url": "data:image/png;base64,iVBORw0K" } },
                ] },
            ])
        );
    }

    #[test]
    fn an_url_image_is_passed_as_is_and_empty_text_has_no_text_part() {
        let messages = json!([
            { "role": "user", "content": "", "images": [
                { "mime": "image/jpeg", "source": { "url": "https://x/y.jpg" } }
            ] },
        ]);
        let rendered = render_openai(&messages, "p", Some(true)).unwrap();
        assert_eq!(
            rendered[0]["content"],
            json!([{ "type": "image_url", "image_url": { "url": "https://x/y.jpg" } }])
        );
    }

    #[test]
    fn a_provider_declared_without_vision_refuses_instead_of_dropping() {
        let messages = thread_with(json!([
            { "mime": "image/png", "source": { "base64": "AAAA" } }
        ]));
        let error = render_openai(&messages, "deepseek", Some(false)).unwrap_err();
        assert!(matches!(error, ProviderError::Images(_)));
        let said = error.to_string();
        assert!(
            said.contains("deepseek") && said.contains("sans vision"),
            "{said}"
        );
        assert_eq!(error.kind(), crate::ErrorKind::Provider);
    }

    #[test]
    fn an_unresolved_reference_is_refused_never_sent() {
        let messages = thread_with(json!([
            { "mime": "image/png", "source": { "ref": SHA }, "name": "a.png" }
        ]));
        let said = render_openai(&messages, "p", None).unwrap_err().to_string();
        assert!(
            said.contains("non résolue") && said.contains("a.png"),
            "{said}"
        );
    }

    #[test]
    fn a_non_image_or_a_non_user_image_is_refused() {
        let wrong_mime = thread_with(json!([
            { "mime": "application/pdf", "source": { "base64": "AAAA" } }
        ]));
        assert!(render_openai(&wrong_mime, "p", None).is_err());
        let on_assistant = json!([
            { "role": "assistant", "content": "x", "images": [
                { "mime": "image/png", "source": { "base64": "AAAA" } }
            ] },
        ]);
        assert!(render_openai(&on_assistant, "p", None).is_err());
    }

    #[test]
    fn resolving_turns_refs_into_base64_and_names_the_missing_ones() {
        let mut messages = thread_with(json!([
            { "mime": "image/png", "source": { "ref": SHA }, "name": "ici.png" },
            { "mime": "image/png", "source": { "ref": "0".repeat(64) }, "name": "perdue.png" },
            { "mime": "image/png", "source": { "base64": "BBBB" } },
        ]));
        let missing = resolve_refs(&mut messages, |part, reference| {
            assert_eq!(part.mime, "image/png");
            if reference == SHA {
                Ok(ImagePart {
                    source: ImageSource::Base64("AAAA".into()),
                    ..part.clone()
                })
            } else {
                Err("le fichier n'est plus sur l'appareil".into())
            }
        });
        assert_eq!(missing, 1);
        let user = &messages[1];
        assert_eq!(
            user["content"],
            "regarde\n[image indisponible : perdue.png — le fichier n'est plus sur l'appareil]"
        );
        assert_eq!(user["images"].as_array().unwrap().len(), 2);
        assert_eq!(user["images"][0]["source"], json!({ "base64": "AAAA" }));
        // The rest of the chain goes through: nothing is left to refuse.
        assert!(render_openai(&messages, "p", None).is_ok());
    }

    #[test]
    fn a_message_whose_only_image_vanished_is_text_only_again() {
        let mut messages = thread_with(json!([
            { "mime": "image/png", "source": { "ref": SHA } }
        ]));
        resolve_refs(&mut messages, |_, _| Err("absent".into()));
        assert_eq!(
            messages[1],
            json!({ "role": "user", "content": "regarde\n[image indisponible : image — absent]" })
        );
    }

    #[test]
    fn only_the_most_recent_images_are_kept_and_the_others_are_named() {
        let image =
            |name: &str| json!({ "mime": "image/png", "source": { "base64": "A" }, "name": name });
        let mut messages = json!([
            { "role": "user", "content": "un", "images": [image("1"), image("2")] },
            { "role": "assistant", "content": "ok" },
            { "role": "user", "content": "deux", "images": [image("3"), image("4")] },
        ]);
        assert_eq!(keep_recent_images(&mut messages, 3), 1);
        assert_eq!(count_images(&messages), 3);
        assert_eq!(messages[0]["images"], json!([image("2")]));
        assert!(
            messages[0]["content"]
                .as_str()
                .unwrap()
                .contains("image non renvoyée : 1")
        );
        assert_eq!(messages[2]["images"].as_array().unwrap().len(), 2);
        assert_eq!(messages[2]["content"], "deux");
    }

    #[test]
    fn the_estimate_ignores_the_payload() {
        let messages = thread_with(json!([
            { "mime": "image/png", "source": { "base64": "A".repeat(100_000) } }
        ]));
        assert!(without_images(&messages).to_string().len() < 200);
    }

    #[test]
    fn stripping_names_each_image_and_leaves_text_only_messages() {
        let image = json!({ "mime": "image/png", "source": { "base64": "A" } });
        let mut messages = json!([
            { "role": "system", "content": "brief" },
            { "role": "user", "content": "un", "images": [image.clone(), image.clone()] },
            { "role": "user", "content": "", "images": [image] },
        ]);
        assert_eq!(strip_images(&mut messages, "[sans image]"), 3);
        assert_eq!(count_images(&messages), 0);
        assert_eq!(messages[0], json!({ "role": "system", "content": "brief" }));
        assert_eq!(
            messages[1],
            json!({ "role": "user", "content": "un\n[sans image]\n[sans image]" })
        );
        assert_eq!(
            messages[2],
            json!({ "role": "user", "content": "[sans image]" })
        );
    }

    #[test]
    fn the_known_refusals_are_recognized_and_other_errors_are_not() {
        let llama = r#"{"error":{"code":500,"message":"image input is not supported - hint: if this is unexpected, you may need to provide the mmproj","type":"server_error"}}"#;
        assert!(refuses_images(llama));
        assert!(refuses_images(
            "Multimodal data provided, but model does not support multimodal requests."
        ));
        assert!(refuses_images(
            r#"{"error":{"message":"Invalid content type. image_url is only supported by certain models.","type":"invalid_request_error"}}"#
        ));
        assert!(!refuses_images(
            r#"{"error":{"code":500,"message":"failed to load model","type":"server_error"}}"#
        ));
        assert!(!refuses_images("audio input is not supported"));
        assert_eq!(
            error_message(llama),
            "image input is not supported - hint: if this is unexpected, you may need to provide the mmproj"
        );
        assert_eq!(error_message(" brut "), "brut");
    }
}
