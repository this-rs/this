//! An image a person joins to a message, described by its SOURCE, not by a transport.
//!
//! Three sources, because three places hold an image:
//!
//! - [`ImageSource::Base64`]: the bytes themselves, already encoded. What a provider
//!   finally sends.
//! - [`ImageSource::Url`]: an address the engine fetches itself (`https://…`, or a
//!   `data:` URL written by someone else).
//! - [`ImageSource::Ref`]: an opaque name only the HOST can resolve (a content digest, a
//!   file id). It exists so that the bytes travel once: a page that keeps a conversation
//!   holds the reference, and the host that builds the HTTP body reads the bytes at that
//!   moment. Without it, every turn of a tool loop would carry every image of the thread
//!   through the page, again. A reference that reaches a provider unresolved is refused
//!   there, never sent and never dropped.

use std::fmt;

/// One image of a message.
#[derive(Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ImagePart {
    /// `image/png`, `image/jpeg`… The engine needs it to read a base64 payload.
    pub mime: String,
    pub source: ImageSource,
    /// What a person called it (a file name). Lets a host say WHICH image is missing,
    /// instead of "an image".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

/// Where an image's bytes are.
#[derive(Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ImageSource {
    /// The bytes, base64-encoded (standard alphabet, padded).
    Base64(String),
    /// An address the engine reads itself.
    Url(String),
    /// A name only the host resolves (for instance a sha256 digest in a blob store).
    Ref(String),
}

impl ImagePart {
    pub fn new(mime: impl Into<String>, source: ImageSource) -> Self {
        Self {
            mime: mime.into(),
            source,
            name: None,
        }
    }

    pub fn named(mut self, name: impl Into<String>) -> Self {
        self.name = Some(name.into());
        self
    }

    /// The name to show, or a neutral word when there is none.
    pub fn label(&self) -> &str {
        self.name.as_deref().unwrap_or("image")
    }
}

// A base64 payload is megabytes long: a `{:?}` of a message must not print it.
impl fmt::Debug for ImageSource {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Base64(data) => write!(formatter, "Base64(<{} bytes>)", data.len()),
            Self::Url(url) if url.starts_with("data:") => {
                write!(formatter, "Url(<data: {} bytes>)", url.len())
            }
            Self::Url(url) => write!(formatter, "Url({url:?})"),
            Self::Ref(name) => write!(formatter, "Ref({name:?})"),
        }
    }
}

impl fmt::Debug for ImagePart {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ImagePart")
            .field("mime", &self.mime)
            .field("source", &self.source)
            .field("name", &self.name)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_payload_is_never_printed_whole() {
        let part = ImagePart::new("image/png", ImageSource::Base64("A".repeat(10_000)));
        let printed = format!("{part:?}");
        assert!(printed.len() < 200, "{printed}");
        assert!(printed.contains("10000 bytes"));
    }
}
