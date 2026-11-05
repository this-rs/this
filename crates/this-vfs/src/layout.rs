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

impl Manifest {
    /// A manifest for a version, with the current format and timestamp.
    pub fn new(
        name: impl Into<String>,
        version: impl Into<String>,
        files: BTreeMap<String, String>,
    ) -> Self {
        Self {
            format: MANIFEST_FORMAT,
            name: name.into(),
            version: version.into(),
            files,
            created: Utc::now(),
            signature: None,
        }
    }
}

/// Ed25519 signature over a manifest, with the identifier of the signing key.
///
/// The signed bytes are the manifest serialized **without** this block, so
/// re-signing after a field change is always detectable: the signature covers
/// the etags, hence the files.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignatureBlock {
    /// Which trusted key signed it. Lets a host rotate keys without a flag day.
    pub key_id: String,
    /// Ed25519 signature, base64 (standard alphabet, padded).
    pub signature: String,
}
