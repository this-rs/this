//! Trust anchor and admission rules of a module version. Pure: no I/O.
//!
//! Policy, fail-closed:
//! - a version **with code** (it contains [`PROGRAM_FILE`]) is admitted only with a
//!   signature by a key of the trust anchor. No anchor declared means no version
//!   with code is admitted at all;
//! - a version **without code** is admitted without signature. If it does carry a
//!   signature and an anchor is declared, that signature must verify: a present but
//!   wrong signature is never legitimate;
//! - a version is **frozen**: a `(name, version)` already stored keeps its `files`
//!   (the etag of each file). A new put of the same version is accepted only if the
//!   `files` are identical, which makes re-putting idempotent and any change of
//!   content under an existing tag a refusal.
//!
//! What a signature covers is defined by [`Manifest::signing_bytes`] (see `layout`).
//!
//! Wiring into a store (`VfsModuleStore` in `this-rs`) is not done here: it reads
//! the existing manifest, computes `has_code` from the files, and calls [`check_put`].

use crate::layout::{Manifest, SignatureBlock};
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};
use std::collections::BTreeMap;

/// Ed25519 public keys a host trusts, by key identifier.
#[derive(Debug, Clone)]
pub struct TrustAnchor {
    keys: BTreeMap<String, VerifyingKey>,
}

impl TrustAnchor {
    /// Builds an anchor from `(key_id, 32-byte public key)` pairs.
    pub fn new<'a>(
        keys: impl IntoIterator<Item = (&'a str, [u8; 32])>,
    ) -> Result<Self, TrustError> {
        let mut map = BTreeMap::new();
        for (id, bytes) in keys {
            let key = VerifyingKey::from_bytes(&bytes)
                .map_err(|_| TrustError::InvalidAnchorKey(id.to_string()))?;
            if map.insert(id.to_string(), key).is_some() {
                return Err(TrustError::DuplicateKeyId(id.to_string()));
            }
        }
        Ok(Self { keys: map })
    }

    /// The trusted key with this identifier, if any.
    pub fn key(&self, key_id: &str) -> Option<&VerifyingKey> {
        self.keys.get(key_id)
    }
}

/// Signs a manifest with `key`, recording `key_id`. Any signature already on the
/// manifest is ignored: the result covers the current fields.
pub fn sign_manifest(manifest: &Manifest, key_id: &str, key: &SigningKey) -> SignatureBlock {
    let sig = key.sign(&manifest.signing_bytes());
    SignatureBlock {
        key_id: key_id.to_string(),
        signature: STANDARD.encode(sig.to_bytes()),
    }
}

/// Checks that `manifest` carries a signature that a key of `anchor` makes over its
/// current fields.
pub fn verify_manifest(manifest: &Manifest, anchor: &TrustAnchor) -> Result<(), TrustError> {
    let block = manifest.signature.as_ref().ok_or(TrustError::Unsigned)?;
    let key = anchor
        .key(&block.key_id)
        .ok_or_else(|| TrustError::UnknownKey(block.key_id.clone()))?;
    let bytes = STANDARD
        .decode(&block.signature)
        .map_err(|_| TrustError::MalformedSignature)?;
    let sig = Signature::from_slice(&bytes).map_err(|_| TrustError::MalformedSignature)?;
    key.verify_strict(&manifest.signing_bytes(), &sig)
        .map_err(|_| TrustError::BadSignature)
}

/// Admission of a put of `new`, given the stored version of the same
/// `(name, version)` if any.
///
/// - `has_code`: the version contains [`PROGRAM_FILE`];
/// - `anchor`: the trust anchor the host declares, `None` if it declares none.
pub fn check_put(
    existing: Option<&Manifest>,
    new: &Manifest,
    has_code: bool,
    anchor: Option<&TrustAnchor>,
) -> Result<(), TrustError> {
    if has_code {
        let anchor = anchor.ok_or(TrustError::NoTrustAnchor)?;
        verify_manifest(new, anchor)?;
    } else if let (Some(anchor), Some(_)) = (anchor, new.signature.as_ref()) {
        verify_manifest(new, anchor)?;
    }

    if let Some(old) = existing {
        if old.name != new.name || old.version != new.version {
            return Err(TrustError::Mismatch);
        }
        if old.files != new.files {
            return Err(TrustError::VersionFrozen {
                name: new.name.clone(),
                version: new.version.clone(),
            });
        }
    }
    Ok(())
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum TrustError {
    #[error("trust anchor: invalid Ed25519 public key for key id `{0}`")]
    InvalidAnchorKey(String),
    #[error("trust anchor: key id `{0}` declared twice")]
    DuplicateKeyId(String),
    #[error("module with code refused: no trust anchor declared")]
    NoTrustAnchor,
    #[error("module with code refused: manifest is not signed")]
    Unsigned,
    #[error("signature by unknown key `{0}`")]
    UnknownKey(String),
    #[error("signature is not valid base64 Ed25519")]
    MalformedSignature,
    #[error("signature does not verify over the manifest")]
    BadSignature,
    #[error("existing version record is for another name or version")]
    Mismatch,
    #[error("version {name}/{version} is frozen: its files differ from the stored ones")]
    VersionFrozen { name: String, version: String },
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::{MANIFEST_FORMAT, PROGRAM_FILE};
    use chrono::{DateTime, Utc};

    fn manifest(version: &str, files: &[(&str, &str)]) -> Manifest {
        Manifest {
            format: MANIFEST_FORMAT,
            name: "shop".into(),
            version: version.into(),
            files: files
                .iter()
                .map(|(p, e)| (p.to_string(), e.to_string()))
                .collect(),
            created: DateTime::parse_from_rfc3339("2026-01-01T00:00:00Z")
                .unwrap()
                .with_timezone(&Utc),
            signature: None,
        }
    }

    fn key_a() -> SigningKey {
        SigningKey::from_bytes(&[7u8; 32])
    }

    fn key_b() -> SigningKey {
        SigningKey::from_bytes(&[9u8; 32])
    }

    fn anchor_a() -> TrustAnchor {
        TrustAnchor::new([("k1", key_a().verifying_key().to_bytes())]).unwrap()
    }

    fn with_code() -> Manifest {
        manifest("0.3.0", &[("descriptor.json", "e1"), (PROGRAM_FILE, "e2")])
    }

    fn signed(m: &Manifest, key: &SigningKey) -> Manifest {
        let mut m = m.clone();
        m.signature = Some(sign_manifest(&m, "k1", key));
        m
    }

    #[test]
    fn code_without_signature_is_refused() {
        let anchor = anchor_a();
        assert_eq!(
            check_put(None, &with_code(), true, Some(&anchor)),
            Err(TrustError::Unsigned)
        );
    }

    #[test]
    fn signature_of_another_key_is_refused() {
        let anchor = anchor_a();
        // Same key id as the anchor's key, but signed with key B: must not verify.
        let forged = signed(&with_code(), &key_b());
        assert_eq!(
            check_put(None, &forged, true, Some(&anchor)),
            Err(TrustError::BadSignature)
        );
        // Signed under an identifier the anchor does not know.
        let mut unknown = signed(&with_code(), &key_a());
        unknown.signature.as_mut().unwrap().key_id = "k2".into();
        assert_eq!(
            check_put(None, &unknown, true, Some(&anchor)),
            Err(TrustError::UnknownKey("k2".into()))
        );
    }

    #[test]
    fn valid_signature_is_accepted() {
        let anchor = anchor_a();
        let ok = signed(&with_code(), &key_a());
        assert_eq!(check_put(None, &ok, true, Some(&anchor)), Ok(()));
    }

    #[test]
    fn tampered_files_invalidate_the_signature() {
        let anchor = anchor_a();
        let mut m = signed(&with_code(), &key_a());
        m.files.insert(PROGRAM_FILE.into(), "evil".into());
        assert_eq!(verify_manifest(&m, &anchor), Err(TrustError::BadSignature));
    }

    #[test]
    fn code_without_any_anchor_is_refused_even_if_signed() {
        let ok = signed(&with_code(), &key_a());
        assert_eq!(
            check_put(None, &ok, true, None),
            Err(TrustError::NoTrustAnchor)
        );
    }

    #[test]
    fn module_without_code_is_accepted_unsigned() {
        let m = manifest("0.3.0", &[("descriptor.json", "e1")]);
        assert_eq!(check_put(None, &m, false, None), Ok(()));
        assert_eq!(check_put(None, &m, false, Some(&anchor_a())), Ok(()));
    }

    #[test]
    fn same_version_with_other_files_is_refused() {
        let stored = manifest("1.0.0", &[("descriptor.json", "e1")]);
        let changed = manifest("1.0.0", &[("descriptor.json", "e9")]);
        assert_eq!(
            check_put(Some(&stored), &changed, false, None),
            Err(TrustError::VersionFrozen {
                name: "shop".into(),
                version: "1.0.0".into(),
            })
        );
    }

    #[test]
    fn same_version_with_same_files_is_accepted() {
        let stored = manifest("1.0.0", &[("descriptor.json", "e1")]);
        let again = manifest("1.0.0", &[("descriptor.json", "e1")]);
        assert_eq!(check_put(Some(&stored), &again, false, None), Ok(()));
    }

    #[test]
    fn duplicate_key_id_in_anchor_is_refused() {
        let k = key_a().verifying_key().to_bytes();
        assert_eq!(
            TrustAnchor::new([("k1", k), ("k1", k)]).unwrap_err(),
            TrustError::DuplicateKeyId("k1".into())
        );
    }
}
