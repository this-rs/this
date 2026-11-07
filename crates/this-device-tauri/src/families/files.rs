//! Files the person picks (`Capability::FilesPick`): the wire types of the native side and the
//! pure checks applied to them. No Tauri, no native code: it runs on the host.
//!
//! The native side (`ios/Sources/files.swift`) answers the two commands `filesPick` and
//! `filesRead`. Every answer is one JSON object with a `kind`:
//!
//! ```text
//! {"kind":"files","files":[{"id":"..","name":"..","size":12,"bookmark":"<base64>"}]}
//! {"kind":"file","name":"..","size":12,"stale":false,"bookmark":null,"data":"<base64>"}
//! {"kind":"error","code":"too_large"}
//! ```
//!
//! ## Security: a bookmark is a durable grant of access
//!
//! A [`Bookmark`] lets its holder read one file again, after a restart. It is sensitive data:
//! its `Debug` is redacted, and it must never be logged or displayed. This module stores none:
//! the kernel (Obrain) keeps the bookmark it is given, and hands it back to `filesRead`.

use std::fmt;

use serde::{Deserialize, Serialize};

/// The fewest files a pick may ask for.
pub const PICK_MIN: u32 = 1;
/// The most files a pick may ask for. The native side applies the same bound.
pub const PICK_MAX: u32 = 5;
/// The largest file a read returns: 10 MiB, measured on the decoded bytes.
pub const READ_MAX_BYTES: u64 = 10 * 1024 * 1024;

/// The pick limit, kept in `PICK_MIN..=PICK_MAX`. Clamped, not refused, as for scan durations.
pub fn clamp_pick(max: u32) -> u32 {
    max.clamp(PICK_MIN, PICK_MAX)
}

/// Why a files request failed. Names are the codes the native side sends.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FilesError {
    /// The person has not granted this module the capability.
    NotAuthorized,
    /// The bridge does not support this on this system (picker needs iOS 14).
    NotSupported,
    /// The picker cannot be shown now (another sheet is open, no window).
    Unavailable,
    /// The bookmark is not valid base64, does not resolve, or names a directory.
    InvalidBookmark,
    /// The file is larger than [`READ_MAX_BYTES`], or than [`WRITE_MAX_BYTES`] for a write.
    TooLarge,
    /// A write whose name could leave the app's directory (a separator, `..`, a control char).
    InvalidName,
    /// The payload is not base64.
    BadBase64,
    /// Something went wrong on the native side.
    Internal,
}

impl fmt::Display for FilesError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let text = match self {
            FilesError::NotAuthorized => "not authorized",
            FilesError::NotSupported => "not supported",
            FilesError::Unavailable => "unavailable",
            FilesError::InvalidBookmark => "invalid bookmark",
            FilesError::TooLarge => "too large",
            FilesError::InvalidName => "invalid name",
            FilesError::BadBase64 => "bad base64",
            FilesError::Internal => "internal error",
        };
        f.write_str(text)
    }
}

/// Base64 text, validated on the way in. Its decoded length is computed without decoding.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct Base64(String);

impl Base64 {
    /// Accepts padded standard base64 (`A-Z a-z 0-9 + /`, `=` padding, length a multiple of 4).
    /// The empty string is valid base64 (an empty file).
    pub fn new(text: String) -> Result<Self, FilesError> {
        if is_base64(&text) {
            Ok(Self(text))
        } else {
            Err(FilesError::InvalidBookmark)
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The number of bytes the text decodes to.
    pub fn decoded_len(&self) -> u64 {
        decoded_len(&self.0).unwrap_or(0)
    }
}

impl fmt::Debug for Base64 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Base64({} chars)", self.0.len())
    }
}

impl TryFrom<String> for Base64 {
    type Error = FilesError;

    fn try_from(text: String) -> Result<Self, FilesError> {
        Self::new(text)
    }
}

impl From<Base64> for String {
    fn from(value: Base64) -> String {
        value.0
    }
}

/// A security-scoped bookmark, base64. Sensitive: see the module documentation.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct Bookmark(Base64);

impl Bookmark {
    /// A bookmark is never empty: an empty one designates no file.
    pub fn new(text: String) -> Result<Self, FilesError> {
        let base64 = Base64::new(text)?;
        if base64.as_str().is_empty() {
            return Err(FilesError::InvalidBookmark);
        }
        Ok(Self(base64))
    }

    /// The base64 text, to hand back to the native side. Never log it.
    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }
}

impl fmt::Debug for Bookmark {
    /// Redacted: a bookmark must not reach a log or a panic message.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Bookmark(<redacted>)")
    }
}

impl TryFrom<String> for Bookmark {
    type Error = FilesError;

    fn try_from(text: String) -> Result<Self, FilesError> {
        Self::new(text)
    }
}

impl From<Bookmark> for String {
    fn from(value: Bookmark) -> String {
        String::from(value.0)
    }
}

/// One file the person picked.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct PickedFile {
    /// A fresh identifier for this pick.
    pub id: String,
    /// The file name, as the system shows it.
    pub name: String,
    /// The size in bytes, as the system reported it at pick time.
    pub size: u64,
    /// The bookmark to read the file again.
    pub bookmark: Bookmark,
}

/// The answer of `filesPick`: `kind` is `files`, `files` may be empty (cancelled).
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PickReply {
    Files { files: Vec<PickedFile> },
    Error { code: FilesError },
}

/// The answer of `filesRead`.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ReadReply {
    File {
        name: String,
        /// The number of bytes in `data`, once decoded.
        size: u64,
        /// True when the system flagged the bookmark as stale. The file was still read.
        stale: bool,
        /// A fresh bookmark, when the stored one was stale and could be renewed: the caller
        /// must replace the stored bookmark with it.
        bookmark: Option<Bookmark>,
        /// The file content, base64.
        data: Base64,
    },
    Error {
        code: FilesError,
    },
}

/// Checks that a file of `size` bytes may be read: at most [`READ_MAX_BYTES`].
pub fn check_size(size: u64) -> Result<(), FilesError> {
    if size > READ_MAX_BYTES {
        Err(FilesError::TooLarge)
    } else {
        Ok(())
    }
}

/// Checks a read answer: the size is within the bound, and `data` decodes to exactly `size`
/// bytes. A file whose content does not match its declared size is refused, not trusted.
pub fn check_read(size: u64, data: &Base64) -> Result<(), FilesError> {
    check_size(size)?;
    if data.decoded_len() == size {
        Ok(())
    } else {
        Err(FilesError::Internal)
    }
}

/// Whether `text` is padded standard base64. Pure: no decoding, no allocation of the bytes.
/// Le plus gros fichier qu'une écriture accepte, décodé. Au-delà, l'appelant découpe : le pont
/// n'a pas de raison de porter un film.
pub const WRITE_MAX_BYTES: u64 = 8 * 1024 * 1024;

/// La requête d'écriture : un nom de fichier, et le contenu en base64.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WriteRequest {
    pub name: String,
    pub data_base64: String,
}

/// Contrôle une écriture : un nom qui ne peut pas sortir du dossier, et une charge bornée.
///
/// Le nom est refusé s'il contient un séparateur de chemin, un `..` ou un caractère de contrôle :
/// une écriture ne doit pas pouvoir viser `/var/mobile/...` parce que l'appelant l'a demandé.
pub fn check_write(request: &WriteRequest) -> Result<u64, FilesError> {
    let name = request.name.trim();
    if name.is_empty()
        || name.contains('/')
        || name.contains('\\')
        || name.contains("..")
        || name.chars().any(|c| c.is_control())
    {
        return Err(FilesError::InvalidName);
    }
    let bytes = decoded_len(&request.data_base64).ok_or(FilesError::BadBase64)?;
    if bytes > WRITE_MAX_BYTES {
        return Err(FilesError::TooLarge);
    }
    Ok(bytes)
}

pub fn is_base64(text: &str) -> bool {
    decoded_len(text).is_some()
}

/// The decoded length of padded standard base64, or `None` when `text` is not valid base64.
pub fn decoded_len(text: &str) -> Option<u64> {
    let bytes = text.as_bytes();
    if !bytes.len().is_multiple_of(4) {
        return None;
    }
    let padding = bytes.iter().rev().take_while(|&&b| b == b'=').count();
    if padding > 2 {
        return None;
    }
    let body = &bytes[..bytes.len() - padding];
    if !body
        .iter()
        .all(|&b| b.is_ascii_alphanumeric() || b == b'+' || b == b'/')
    {
        return None;
    }
    Some((bytes.len() / 4 * 3 - padding) as u64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn picked(bookmark: &str) -> String {
        format!(r#"{{"id":"a1","name":"rapport.pdf","size":12,"bookmark":"{bookmark}"}}"#)
    }

    #[test]
    fn a_pick_reply_decodes_into_the_contract() {
        let text = format!(r#"{{"kind":"files","files":[{}]}}"#, picked("Ym9va21hcms="));
        let reply: PickReply = serde_json::from_str(&text).unwrap();
        assert_eq!(
            reply,
            PickReply::Files {
                files: vec![PickedFile {
                    id: "a1".into(),
                    name: "rapport.pdf".into(),
                    size: 12,
                    bookmark: Bookmark::new("Ym9va21hcms=".into()).unwrap(),
                }]
            }
        );
    }

    #[test]
    fn a_cancelled_pick_is_an_empty_list_not_an_error() {
        let reply: PickReply = serde_json::from_str(r#"{"kind":"files","files":[]}"#).unwrap();
        assert_eq!(reply, PickReply::Files { files: vec![] });
    }

    #[test]
    fn a_file_read_decodes_with_a_null_fresh_bookmark() {
        let reply: ReadReply = serde_json::from_str(
            r#"{"kind":"file","name":"a.txt","size":3,"stale":false,"bookmark":null,"data":"YWJj"}"#,
        )
        .unwrap();
        match reply {
            ReadReply::File {
                name,
                size,
                stale,
                bookmark,
                data,
            } => {
                assert_eq!(name, "a.txt");
                assert_eq!(size, 3);
                assert!(!stale);
                assert_eq!(bookmark, None);
                assert_eq!(data.as_str(), "YWJj");
            }
            other => panic!("expected a file, got {other:?}"),
        }
    }

    #[test]
    fn a_stale_read_carries_the_fresh_bookmark() {
        let reply: ReadReply = serde_json::from_str(
            r#"{"kind":"file","name":"a.txt","size":0,"stale":true,"bookmark":"Ym9va21hcms=","data":""}"#,
        )
        .unwrap();
        match reply {
            ReadReply::File {
                stale, bookmark, ..
            } => {
                assert!(stale);
                assert_eq!(bookmark.unwrap().as_str(), "Ym9va21hcms=");
            }
            other => panic!("expected a file, got {other:?}"),
        }
    }

    #[test]
    fn every_files_error_code_the_native_side_sends_is_a_contract_code() {
        for (wire, code) in [
            ("not_authorized", FilesError::NotAuthorized),
            ("not_supported", FilesError::NotSupported),
            ("unavailable", FilesError::Unavailable),
            ("invalid_bookmark", FilesError::InvalidBookmark),
            ("too_large", FilesError::TooLarge),
            ("internal", FilesError::Internal),
        ] {
            let json = format!(r#"{{"kind":"error","code":"{wire}"}}"#);
            let reply: ReadReply = serde_json::from_str(&json).unwrap();
            assert_eq!(reply, ReadReply::Error { code }, "{wire}");
            let reply: PickReply = serde_json::from_str(&json).unwrap();
            assert_eq!(reply, PickReply::Error { code }, "{wire}");
        }
    }

    #[test]
    fn an_unknown_kind_is_refused_not_guessed() {
        assert!(serde_json::from_str::<ReadReply>(r#"{"kind":"folder","entries":[]}"#).is_err());
        assert!(serde_json::from_str::<PickReply>(r#"{"kind":"folder","entries":[]}"#).is_err());
    }

    #[test]
    fn a_bookmark_that_is_not_base64_is_refused_on_the_way_in() {
        for bad in ["", "not base64!", "Ym9v=bad", "Ym9va21", "A==="] {
            let text = picked(bad);
            assert!(
                serde_json::from_str::<PickReply>(&format!(
                    r#"{{"kind":"files","files":[{text}]}}"#
                ))
                .is_err(),
                "{bad:?} must be refused"
            );
        }
    }

    #[test]
    fn an_empty_bookmark_is_refused_even_though_empty_base64_is_valid() {
        assert_eq!(
            Bookmark::new(String::new()),
            Err(FilesError::InvalidBookmark)
        );
        assert!(Base64::new(String::new()).is_ok());
    }

    #[test]
    fn the_bookmark_is_redacted_in_debug_output() {
        let bookmark = Bookmark::new("c2VjcmV0LWJvb2ttYXJr".into()).unwrap();
        let shown = format!("{bookmark:?}");
        assert_eq!(shown, "Bookmark(<redacted>)");
        assert!(!shown.contains("c2VjcmV0"));
    }

    #[test]
    fn the_bookmark_serializes_back_to_its_base64_text() {
        let bookmark = Bookmark::new("Ym9va21hcms=".into()).unwrap();
        assert_eq!(
            serde_json::to_value(&bookmark).unwrap(),
            json!("Ym9va21hcms=")
        );
    }

    #[test]
    fn base64_accepts_padded_standard_text_and_nothing_else() {
        for good in ["", "YQ==", "YWI=", "YWJj", "AB+/"] {
            assert!(is_base64(good), "{good:?}");
        }
        for bad in [
            "Y", "YQ=", "YQ===", "====", "YQ==YQ==", "YW-j", "YW j", "YQ=A",
        ] {
            assert!(!is_base64(bad), "{bad:?}");
        }
    }

    #[test]
    fn decoded_length_counts_the_padding() {
        assert_eq!(decoded_len(""), Some(0));
        assert_eq!(decoded_len("YQ=="), Some(1));
        assert_eq!(decoded_len("YWI="), Some(2));
        assert_eq!(decoded_len("YWJj"), Some(3));
        assert_eq!(decoded_len("YWJjZA=="), Some(4));
        assert_eq!(decoded_len("YWJ"), None);
    }

    #[test]
    fn the_read_bound_is_ten_mebibytes() {
        assert_eq!(READ_MAX_BYTES, 10 * 1024 * 1024);
        assert!(check_size(READ_MAX_BYTES).is_ok());
        assert_eq!(check_size(READ_MAX_BYTES + 1), Err(FilesError::TooLarge));
    }

    #[test]
    fn a_read_is_checked_against_its_declared_size() {
        let data = Base64::new("YWJj".into()).unwrap();
        assert!(check_read(3, &data).is_ok());
        // Declared size and content disagree: refused, not trusted.
        assert_eq!(check_read(4, &data), Err(FilesError::Internal));
        assert_eq!(check_read(2, &data), Err(FilesError::Internal));
    }

    #[test]
    fn an_oversized_read_is_refused_before_its_content_is_looked_at() {
        let data = Base64::new(String::new()).unwrap();
        assert_eq!(
            check_read(READ_MAX_BYTES + 1, &data),
            Err(FilesError::TooLarge)
        );
    }

    #[test]
    fn the_pick_limit_is_clamped_to_one_to_five() {
        assert_eq!(clamp_pick(0), 1);
        assert_eq!(clamp_pick(1), 1);
        assert_eq!(clamp_pick(3), 3);
        assert_eq!(clamp_pick(5), 5);
        assert_eq!(clamp_pick(u32::MAX), PICK_MAX);
    }

    #[test]
    fn the_pick_argument_is_the_native_key() {
        assert_eq!(json!({ "max": clamp_pick(2) }), json!({ "max": 2 }));
    }
}
