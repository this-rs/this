//! The layout a module takes **on** a VFS, defined once.
//!
//! This is the real compatibility surface between two hosts. A trait's shape is
//! local ergonomics; what a client writes and a server reads has to agree on
//! paths and on a commit record. Defining that here — in a crate that compiles
//! for `wasm32` — is what lets a module compiled and stored in a page be read,
//! verified and served by a host that never saw the client.
//!
//! ```text
//! /modules/{name}/{version}/descriptor.json
//! /modules/{name}/{version}/manifest.json      ← the root of trust
//! /modules/{name}/_yanked/{version}
//! ```
//!
//! The manifest carries the etag of every file of the version, so signing it
//! authenticates them all. It is part of the format even for a host built
//! without signature enforcement: a store must stay readable everywhere.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// The descriptor, relative to a version directory.
pub const DESCRIPTOR_FILE: &str = "descriptor.json";
/// The commit record, relative to a version directory.
pub const MANIFEST_FILE: &str = "manifest.json";
/// Where a yanked version is parked, relative to the module directory.
pub const YANKED_DIR: &str = "_yanked";
/// Version of the commit record format itself.
pub const MANIFEST_FORMAT: u32 = 1;
/// The binary, relative to a version directory. Its presence makes a version a
/// version *with code*: such a version is admitted only with a valid signature
/// from a key of the trust anchor (see `trust::check_put`).
pub const PROGRAM_FILE: &str = "program.wasm";

/// Upper bound on the size of a single asset a module version may carry, in bytes.
///
/// Assets are small declarative artifacts (schemas, icons, i18n payloads), never
/// blobs; a module version is a set of such files plus, at most, one `program.wasm`.
/// The bound is shared between the writer (`this-vfs` / the harness) and the
/// store that admits a version, so a writer cannot produce a version the store
/// will refuse. Mirrors the 64 MiB attachment cap used elsewhere in the project.
pub const MAX_ASSET_BYTES: usize = 64 * 1024 * 1024;

/// Commit record of one version.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    /// Format of this record, not of the module.
    pub format: u32,
    pub name: String,
    pub version: String,
    /// Path relative to the version directory -> etag.
    pub files: BTreeMap<String, String>,
    pub created: DateTime<Utc>,
    /// Authentication of this record; only *enforced* by a host that declares a
    /// trust anchor, but always part of the format.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signature: Option<SignatureBlock>,
}

/// Ed25519 signature over a manifest, with the identifier of the signing key.
///
/// The signed bytes are the manifest serialized **without** this block (see
/// [`Manifest::signing_bytes`]), so re-signing after a field change is always
/// detectable: the signature covers the etags, hence the files.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignatureBlock {
    /// Which trusted key signed it. Lets a host rotate keys without a flag day.
    pub key_id: String,
    /// Ed25519 signature, base64 (standard alphabet, padded).
    pub signature: String,
}

/// The exact value a signature covers: every field of [`Manifest`] except
/// `signature`.
///
/// This struct *is* the signing format. Signer and verifier both serialize it
/// with `serde_json::to_vec`, so the bytes are fixed by:
/// - the field set and the **declaration order** below (`format`, `name`,
///   `version`, `files`, `created`): reordering or renaming a field is a format
///   change and invalidates every existing signature;
/// - `files` being a `BTreeMap`: its keys come out sorted, whatever the order of
///   insertion;
/// - serde_json's compact output (no spaces, no trailing newline).
///
/// Do not add a field here without bumping [`MANIFEST_FORMAT`].
#[derive(Debug, Serialize)]
pub struct ManifestBody<'a> {
    pub format: u32,
    pub name: &'a str,
    pub version: &'a str,
    pub files: &'a BTreeMap<String, String>,
    pub created: &'a DateTime<Utc>,
}

/// Lifecycle state of one key of a trust anchor.
///
/// The anchor only ever answers « does this manifest come from a source we
/// admit ». It never carries what that source may do; [`AnchorKeyRole`] is the
/// only extra provenance it keeps, and verification consults the state alone.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AnchorKeyState {
    /// The key may authenticate a manifest.
    Active,
    /// The key is known but no longer admitted: same verdict as an unknown key,
    /// only distinguishable so a host can tell « revoked » from « never seen ».
    Revoked,
}

/// Provenance of one key of a trust anchor. Informational: verification never
/// reads it (see [`AnchorKeyState`]); it never authorizes behaviour.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AnchorKeyRole {
    /// The host's own key.
    Ownership,
    /// A paired peer's key.
    Shared,
}

/// One key of a trust anchor, in the shape it is persisted or shared between two
/// hosts. The verification side lives in `this` (it holds a parsed
/// `VerifyingKey`); this is its serializable twin — the public key as hex.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AnchorKey {
    pub key_id: String,
    /// Ed25519 public key, 32 bytes, lower-case hex.
    pub public_key: String,
    pub state: AnchorKeyState,
    pub added_at: DateTime<Utc>,
    pub role: AnchorKeyRole,
}

impl Manifest {
    /// The bytes covered by the signature: the canonical [`ManifestBody`] of this
    /// record. Any `signature` it carries is ignored.
    pub fn signing_bytes(&self) -> Vec<u8> {
        // Serializing this struct cannot fail: no map has non-string keys and no
        // field has a custom serializer that might return an error.
        serde_json::to_vec(&ManifestBody {
            format: self.format,
            name: &self.name,
            version: &self.version,
            files: &self.files,
            created: &self.created,
        })
        .expect("ManifestBody serializes to JSON")
    }
}

/// Version of the portable package format itself.
pub const PACKAGE_FORMAT: u32 = 1;

/// A portable, signed package of ONE module version: the competency a host can
/// share without sharing data.
///
/// ```text
/// ModulePackage {
///   format: u32,
///   manifest: Manifest,                 // signed by the author, files = etags below
///   files: BTreeMap<String, String>,    // path under the version dir -> base64 content
/// }
/// ```
///
/// A package carries exactly `descriptor.json` and `assets/**`, plus the signed
/// manifest that authenticates them. Nothing else — no `i18n/**`, no `rows/**`, no
/// line. That is what lets a competency cross a device boundary without carrying
/// a single byte of user data: data lives in the graph, never in `/modules`.
///
/// Chosen container: one self-contained JSON document. It needs no new dependency
/// (serde + serde_json are already here), its byte form is deterministic (sorted
/// `BTreeMap`, compact JSON, base64 of the stored files), and it is verifiable end
/// to end: the manifest signs the etags, the etags bind the shipped bytes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModulePackage {
    pub format: u32,
    pub manifest: Manifest,
    /// Path relative to the version directory -> content, base64 (standard
    /// alphabet, padded). Sorted because it is a `BTreeMap`: two exports of the
    /// same content are byte-identical.
    pub files: BTreeMap<String, String>,
}

/// Is `rel` a path a package may carry? Exactly `descriptor.json` or `assets/**`.
///
/// This is the only gate between a package and the store: a path that fails it —
/// `i18n/x.json`, `rows/x`, a stray `manifest.json`, anything — is refused at
/// import, naming the path, before a single byte is written.
pub fn package_path_allowed(rel: &str) -> bool {
    rel == DESCRIPTOR_FILE || rel.starts_with("assets/")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn anchor_key_round_trips_through_json() {
        let key = AnchorKey {
            key_id: "device-0102030405060708".into(),
            public_key: "ab".repeat(32),
            state: AnchorKeyState::Active,
            added_at: DateTime::parse_from_rfc3339("2026-01-01T00:00:00Z")
                .unwrap()
                .with_timezone(&Utc),
            role: AnchorKeyRole::Ownership,
        };
        let json = serde_json::to_string(&key).unwrap();
        let back: AnchorKey = serde_json::from_str(&json).unwrap();
        assert_eq!(key, back);
        assert_eq!(back.state, AnchorKeyState::Active);
        assert_eq!(back.role, AnchorKeyRole::Ownership);
    }

    #[test]
    fn package_path_allowlist_is_exactly_descriptor_and_assets() {
        for ok in ["descriptor.json", "assets/program.wasm", "assets/a/b.json"] {
            assert!(package_path_allowed(ok), "{ok}");
        }
        for bad in [
            "manifest.json",
            "i18n/fr.json",
            "rows/x",
            "rows/",
            "descriptor.json/extra",
            "asset/x",
            "",
        ] {
            assert!(!package_path_allowed(bad), "{bad}");
        }
    }

    #[test]
    fn signing_bytes_format_is_pinned() {
        let created = DateTime::parse_from_rfc3339("2026-01-01T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let mut files = BTreeMap::new();
        files.insert("program.wasm".to_string(), "e2".to_string());
        files.insert("descriptor.json".to_string(), "e1".to_string());
        let m = Manifest {
            format: MANIFEST_FORMAT,
            name: "shop".into(),
            version: "0.3.0".into(),
            files,
            created,
            signature: Some(SignatureBlock {
                key_id: "k".into(),
                signature: "ignored".into(),
            }),
        };
        assert_eq!(
            String::from_utf8(m.signing_bytes()).unwrap(),
            concat!(
                r#"{"format":1,"name":"shop","version":"0.3.0","files":"#,
                r#"{"descriptor.json":"e1","program.wasm":"e2"},"#,
                r#""created":"2026-01-01T00:00:00Z"}"#
            )
        );
    }
}
