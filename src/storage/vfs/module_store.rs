//! `VfsModuleStore`: the `ModuleStore` over any `Vfs` (memory, local fs, obrain).
//!
//! # Layout (under `root`, default `/modules`)
//!
//! ```text
//! <name>/<version>/descriptor.json      normalized descriptor, no texts
//! <name>/<version>/i18n/<locale>.json   one MessageBundle per locale
//! <name>/<version>/assets/**            artifacts (wasm, schemas, ...)
//! <name>/<version>/manifest.json        COMMIT MARKER, written last (IfNoneMatch):
//!                                       { files: relative path -> etag }
//! <name>/_yanked/<version>              rollback marker (outside the immutable tree)
//! ```
//!
//! # Guarantees
//!
//! - **Fail-closed put**: `check(Stored)`, bundle checks (module, syntax, key namespace,
//!   locale declared and unique case-insensitively), asset path checks, and
//!   `check_compat` against EVERY stored version in semver order (yanked included: a
//!   yanked version's tags stay published; a backport is checked against the versions
//!   above it) run BEFORE anything is written.
//! - **Immutable versions**: a committed version is never rewritten. Putting the same
//!   content again is idempotent (returns the manifest's meta); different content is
//!   refused with `this.vfs.precondition_failed` (`Aborted`, 409).
//! - **Atomic visibility**: a version without `manifest.json` does not exist for
//!   `get`/`list`/`bundle` (a crashed put leaves invisible orphans, deleted by the next
//!   put of that version before it writes).
//! - **Integrity**: every read compares the file's etag with the manifest; a tampered or
//!   half-overwritten file is refused (`this.internal`), never served.
//! - **Rollback without mutation**: `yank` hides a version from `get(name, None)` (the
//!   previous highest version becomes the active one) while `get(name, Some(v))` still
//!   serves it; `unyank` restores it. Nothing in the version tree is touched.
//! - `get` re-runs `check(Stored)` with the CURRENT registry: a custom validator
//!   unregistered since the upload makes the module unloadable instead of silently
//!   unchecked.
//!
//! Authorization (`this.module.load`) is the caller's job (RFC trust model).

// `ContractError` is the error of the `ModuleStore` contract (the trait returns it); same
// allowance as this-descriptor.
#![allow(clippy::result_large_err)]

use super::{ModuleStore, Precondition, Vfs, VfsEntry, VfsError, VfsMeta, VfsPath};
use crate::descriptor::check::issues_to_error;
use crate::descriptor::compat::check_compat;
use crate::descriptor::framework::codes;
use crate::descriptor::{
    CheckProfile, ContractError, CustomRegistry, DescriptorIssue, ErrorCode, Locale, MessageBundle,
    ModuleDescriptor,
};
use crate::server::module_runtime::ModuleSource;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

const DESCRIPTOR_FILE: &str = "descriptor.json";
const MANIFEST_FILE: &str = "manifest.json";
const YANKED_DIR: &str = "_yanked";
const MANIFEST_FORMAT: u32 = 1;

/// Commit record of one version.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Manifest {
    format: u32,
    name: String,
    version: String,
    /// Path relative to the version directory -> etag.
    files: BTreeMap<String, String>,
    created: DateTime<Utc>,
    /// Authentication of this record. **The manifest is the root of trust**: it
    /// carries the etag of every file, so signing it authenticates them all. The
    /// field is always part of the format (a store must stay readable by a host
    /// built without the feature); it is only *enforced* by a host that declares
    /// a [`TrustAnchor`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    signature: Option<SignatureBlock>,
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

/// The keys a host accepts for stored modules.
///
/// Fail-closed by construction: a store that has an anchor refuses a manifest
/// that carries no signature, and one signed by an unknown key. A store with no
/// anchor behaves as before — integrity against the manifest, not
/// authentication of it.
#[cfg(feature = "module-signing")]
#[derive(Debug, Clone, Default)]
pub struct TrustAnchor {
    keys: BTreeMap<String, ed25519_dalek::VerifyingKey>,
}

#[cfg(feature = "module-signing")]
impl TrustAnchor {
    pub fn new() -> Self {
        Self::default()
    }

    /// Trust a key given as a SubjectPublicKeyInfo PEM (the format
    /// `openssl genpkey`/`this sign` produce).
    pub fn insert_pem(
        &mut self,
        key_id: impl Into<String>,
        pem: &str,
    ) -> Result<(), ContractError> {
        use ed25519_dalek::pkcs8::DecodePublicKey;
        let key = ed25519_dalek::VerifyingKey::from_public_key_pem(pem).map_err(|e| {
            ContractError::framework(codes::MODULE_UNTRUSTED_KEY)
                .with_param("reason", e.to_string())
        })?;
        self.keys.insert(key_id.into(), key);
        Ok(())
    }

    /// Trust an already-parsed key (tests, or a host that reads keys from its own
    /// configuration store).
    pub fn insert(&mut self, key_id: impl Into<String>, key: ed25519_dalek::VerifyingKey) {
        self.keys.insert(key_id.into(), key);
    }

    /// Is this anchor empty? An empty anchor is a misconfiguration, not a policy:
    /// the store refuses everything rather than accepting everything.
    pub fn is_empty(&self) -> bool {
        self.keys.is_empty()
    }
}

/// One stored version of a module.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredVersion {
    pub version: String,
    pub yanked: bool,
    pub created: DateTime<Utc>,
}

/// A stored module as the runtime loads it: descriptor plus its texts.
#[derive(Debug, Clone, PartialEq)]
pub struct StoredModule {
    pub descriptor: ModuleDescriptor,
    pub bundles: Vec<MessageBundle>,
}

/// Semver ordering key without naming the `semver` crate (this-rs only depends on it
/// through this-descriptor). Non-semver strings sort first (`None`).
fn semver_key(v: &str) -> impl Ord + use<> {
    ModuleDescriptor::new("_", v).semver()
}

fn invalid(issues: Vec<DescriptorIssue>) -> ContractError {
    issues_to_error(&issues)
}

fn issue(reason: &str, path: impl Into<String>, message: impl Into<String>) -> DescriptorIssue {
    DescriptorIssue {
        code: ErrorCode::new(format!("this.descriptor.{reason}")),
        path: path.into(),
        message: message.into(),
    }
}

/// Compatibility of `next` with ONE stored version `other`, in semver order: a lower
/// version must evolve into `next`, `next` must evolve into a higher one (backport). So a
/// backport `1.0.1` put after `2.0.0` is accepted when it lacks fields added in `2.0.0`,
/// and refused when it binds a tag `2.0.0` gives to another field.
fn compat_with(other: &ModuleDescriptor, next: &ModuleDescriptor) -> Vec<DescriptorIssue> {
    if semver_key(&other.version) < semver_key(&next.version) {
        check_compat(other, next)
    } else {
        check_compat(next, other)
    }
}

/// Locale tag usable as a file name on every backend: BCP 47 characters only.
fn locale_ok(l: &Locale) -> bool {
    let s = l.as_str();
    !s.is_empty()
        && s.len() <= 35
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

fn immutable(path: &VfsPath) -> ContractError {
    ContractError::framework(codes::VFS_PRECONDITION_FAILED)
        .with_param("path", path.as_str())
        .with_param("reason", "version is immutable")
}

fn integrity(path: &VfsPath, what: &str) -> ContractError {
    ContractError::framework(codes::INTERNAL)
        .with_param("path", path.as_str())
        .with_param("reason", format!("module store integrity: {what}"))
}

fn json<T: Serialize>(v: &T) -> Result<Vec<u8>, ContractError> {
    serde_json::to_vec_pretty(v)
        .map_err(|e| ContractError::framework(codes::INTERNAL).with_param("reason", e.to_string()))
}

/// The exact bytes a signature covers: the manifest re-serialized **without** its
/// signature block.
///
/// Deterministic because the fields are a fixed-order struct and `files` is a
/// `BTreeMap`: the same manifest always produces the same bytes, on the signing
/// side and on the verifying side, without a bespoke canonicalization format.
#[cfg(feature = "module-signing")]
fn canonical_manifest_bytes(m: &Manifest) -> Result<Vec<u8>, ContractError> {
    let mut bare = m.clone();
    bare.signature = None;
    serde_json::to_vec(&bare).map_err(|e| {
        ContractError::framework(codes::MODULE_BAD_SIGNATURE).with_param("reason", e.to_string())
    })
}

/// `ModuleStore` over a `Vfs`.
pub struct VfsModuleStore {
    vfs: Arc<dyn Vfs>,
    registry: Arc<CustomRegistry>,
    allow_public: bool,
    root: VfsPath,
    /// Keys accepted for stored modules. `None` = no authentication of the
    /// manifest (integrity only), the historical behaviour.
    #[cfg(feature = "module-signing")]
    trust: Option<TrustAnchor>,
    /// Publisher side: the key this store signs committed manifests with.
    #[cfg(feature = "module-signing")]
    signing: Option<(String, ed25519_dalek::SigningKey)>,
    /// Serializes put/remove/yank in this process (check-then-commit sequences).
    write_lock: tokio::sync::Mutex<()>,
}

impl std::fmt::Debug for VfsModuleStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VfsModuleStore")
            .field("root", &self.root)
            .field("allow_public", &self.allow_public)
            .field("caps", &self.vfs.caps())
            .finish()
    }
}

impl VfsModuleStore {
    /// Store under `/modules`, default registry, `public` refused.
    pub fn new(vfs: Arc<dyn Vfs>) -> Self {
        Self {
            vfs,
            registry: Arc::new(CustomRegistry::default()),
            allow_public: false,
            root: VfsPath::parse("/modules").expect("static path"),
            #[cfg(feature = "module-signing")]
            trust: None,
            #[cfg(feature = "module-signing")]
            signing: None,
            write_lock: tokio::sync::Mutex::new(()),
        }
    }

    /// Require every stored manifest to be signed by one of these keys.
    ///
    /// This is the trust anchor the whole hot-load story was missing: without it
    /// a stored module is only *integrity*-checked (its files match the etags of
    /// a manifest anyone with write access can rewrite). With it, the manifest
    /// must carry a signature by a known key, so an operator who can write to the
    /// store still cannot invent a module.
    ///
    /// An **empty** anchor refuses everything: it is a misconfiguration, and
    /// treating it as "no policy" would silently disable the protection.
    #[cfg(feature = "module-signing")]
    pub fn with_trust_anchor(mut self, anchor: TrustAnchor) -> Self {
        self.trust = Some(anchor);
        self
    }

    /// Sign every manifest this store commits.
    ///
    /// Publisher side: a host does not need it, a build step or a CLI that
    /// publishes modules does.
    #[cfg(feature = "module-signing")]
    pub fn with_signing_key(
        mut self,
        key_id: impl Into<String>,
        key: ed25519_dalek::SigningKey,
    ) -> Self {
        self.signing = Some((key_id.into(), key));
        self
    }

    /// Registry of the host's custom validators / transforms / types (the one `check`
    /// must see, at put AND at get).
    pub fn with_registry(mut self, registry: Arc<CustomRegistry>) -> Self {
        self.registry = registry;
        self
    }

    /// Host decision: may a stored module declare `public` policies?
    pub fn allow_public(mut self, allow: bool) -> Self {
        self.allow_public = allow;
        self
    }

    pub fn with_root(mut self, root: VfsPath) -> Self {
        self.root = root;
        self
    }

    pub fn vfs(&self) -> &Arc<dyn Vfs> {
        &self.vfs
    }

    fn profile(&self) -> CheckProfile {
        CheckProfile::Stored {
            allow_public: self.allow_public,
        }
    }

    fn module_dir(&self, name: &str) -> Result<VfsPath, ContractError> {
        Ok(self.root.join(name)?)
    }

    fn version_dir(&self, name: &str, version: &str) -> Result<VfsPath, ContractError> {
        Ok(self.module_dir(name)?.join(version)?)
    }

    fn yank_marker(&self, name: &str, version: &str) -> Result<VfsPath, ContractError> {
        Ok(self.module_dir(name)?.join(YANKED_DIR)?.join(version)?)
    }

    fn rel(dir: &VfsPath, rel: &str) -> Result<VfsPath, ContractError> {
        Ok(VfsPath::parse(&format!(
            "{}/{rel}",
            dir.as_str().trim_end_matches('/')
        ))?)
    }

    async fn manifest(&self, name: &str, version: &str) -> Result<Option<Manifest>, ContractError> {
        let path = Self::rel(&self.version_dir(name, version)?, MANIFEST_FILE)?;
        let bytes = match self.vfs.read(&path).await {
            Ok(b) => b,
            Err(VfsError::NotFound(_)) => return Ok(None),
            Err(e) => return Err(e.into()),
        };
        let m: Manifest =
            serde_json::from_slice(&bytes).map_err(|e| integrity(&path, &e.to_string()))?;
        if m.name != name || m.version != version {
            return Err(ContractError::framework(codes::MODULE_IDENTITY_MISMATCH)
                .with_param("path", path.as_str())
                .with_param("expected", format!("{name}@{version}"))
                .with_param("found", format!("{}@{}", m.name, m.version)));
        }
        // Authentication before anything is read out of this manifest: the etags
        // it carries become trustworthy only once the record itself is.
        #[cfg(feature = "module-signing")]
        self.verify_manifest(&path, &m)?;
        Ok(Some(m))
    }

    /// Refuse a manifest that is not signed by a trusted key, when the host
    /// declared an anchor. No anchor = no authentication (historical behaviour).
    #[cfg(feature = "module-signing")]
    fn verify_manifest(&self, path: &VfsPath, m: &Manifest) -> Result<(), ContractError> {
        let Some(anchor) = self.trust.as_ref() else {
            return Ok(());
        };
        let Some(sig) = m.signature.as_ref() else {
            return Err(ContractError::framework(codes::MODULE_UNSIGNED)
                .with_param("path", path.as_str())
                .with_param("module", format!("{}@{}", m.name, m.version)));
        };
        let Some(key) = anchor.keys.get(&sig.key_id) else {
            return Err(ContractError::framework(codes::MODULE_UNTRUSTED_KEY)
                .with_param("path", path.as_str())
                .with_param("key_id", sig.key_id.clone()));
        };
        let bytes = canonical_manifest_bytes(m)?;
        use base64::Engine as _;
        let raw = base64::engine::general_purpose::STANDARD
            .decode(&sig.signature)
            .map_err(|e| {
                ContractError::framework(codes::MODULE_BAD_SIGNATURE)
                    .with_param("path", path.as_str())
                    .with_param("reason", e.to_string())
            })?;
        let signature = ed25519_dalek::Signature::from_slice(&raw).map_err(|e| {
            ContractError::framework(codes::MODULE_BAD_SIGNATURE)
                .with_param("path", path.as_str())
                .with_param("reason", e.to_string())
        })?;
        // `verify_strict` rather than `verify`: it rejects malleable signatures
        // and small-order keys, which a permissive verifier would accept.
        key.verify_strict(&bytes, &signature).map_err(|_| {
            ContractError::framework(codes::MODULE_BAD_SIGNATURE)
                .with_param("path", path.as_str())
                .with_param("key_id", sig.key_id.clone())
        })
    }

    /// Read a committed file and verify it against the manifest.
    async fn read_verified(
        &self,
        dir: &VfsPath,
        m: &Manifest,
        rel: &str,
    ) -> Result<Option<Vec<u8>>, ContractError> {
        let Some(want) = m.files.get(rel) else {
            return Ok(None);
        };
        let path = Self::rel(dir, rel)?;
        let bytes = match self.vfs.read(&path).await {
            Ok(b) => b,
            Err(VfsError::NotFound(_)) => return Err(integrity(&path, "file missing")),
            Err(e) => return Err(e.into()),
        };
        if &super::etag_of(&bytes) != want {
            return Err(integrity(&path, "etag differs from the manifest"));
        }
        Ok(Some(bytes))
    }

    /// Every stored (committed) version of `name`, ascending semver.
    pub async fn versions(&self, name: &str) -> Result<Vec<StoredVersion>, ContractError> {
        let dir = self.module_dir(name)?;
        let mut out = Vec::new();
        let yanked: BTreeSet<String> = self
            .vfs
            .list(&dir.join(YANKED_DIR)?)
            .await?
            .into_iter()
            .filter_map(|e| match e {
                VfsEntry::File { path, .. } => path.file_name().map(str::to_string),
                VfsEntry::Dir { .. } => None,
            })
            .collect();
        for e in self.vfs.list(&dir).await? {
            let VfsEntry::Dir { path } = e else { continue };
            let Some(v) = path.file_name() else { continue };
            if ModuleDescriptor::new("_", v).semver().is_none() {
                continue; // `_yanked` and anything that is not a version
            }
            if let Some(m) = self.manifest(name, v).await? {
                out.push(StoredVersion {
                    version: v.to_string(),
                    yanked: yanked.contains(v),
                    created: m.created,
                });
            }
        }
        out.sort_by_key(|s| semver_key(&s.version));
        Ok(out)
    }

    /// Version served by `get(name, None)`: highest non-yanked.
    pub async fn active_version(&self, name: &str) -> Result<Option<String>, ContractError> {
        Ok(self
            .versions(name)
            .await?
            .into_iter()
            .rev()
            .find(|v| !v.yanked)
            .map(|v| v.version))
    }

    /// Put a version with its artifacts (`assets/<rel>`: wasm, schemas...). Same
    /// guarantees as `put`; the assets are part of the immutable version.
    pub async fn put_with_assets(
        &self,
        descriptor: &ModuleDescriptor,
        bundles: &[MessageBundle],
        assets: &[(&str, &[u8])],
    ) -> Result<VfsMeta, ContractError> {
        // 1. Checks, nothing written yet.
        let mut desc = descriptor.clone();
        desc.normalize();
        desc.check(&self.registry, self.profile())
            .map_err(invalid)?;
        let mut issues = Vec::new();
        let mut locales = BTreeSet::new();
        let ns = format!("{}.", desc.name);
        // `load_stored` only reads the declared locales and the default one: a bundle for
        // any other locale would be stored and never served. Exact match: `load_stored` asks
        // for the declared spelling.
        let declared: BTreeSet<String> = desc
            .i18n
            .locales
            .iter()
            .chain(std::iter::once(&desc.i18n.default_locale))
            .map(|l| l.as_str().to_string())
            .collect();
        for (i, b) in bundles.iter().enumerate() {
            let p = format!("bundles[{i}]");
            if b.module != desc.name {
                issues.push(issue(
                    "bundle_module",
                    &p,
                    format!("bundle of `{}` in module `{}`", b.module, desc.name),
                ));
            }
            if let Err(mut e) = ModuleDescriptor::check_bundle_syntax(b.syntax) {
                e.path = p.clone();
                issues.push(e);
            }
            // Case-insensitive: `fr-CA` and `fr-ca` are one file on a case-insensitive
            // file system (macOS, Windows), the second write would replace the first.
            let folded = b.locale.as_str().to_ascii_lowercase();
            if !locales.insert(folded) {
                issues.push(issue("duplicate_locale", &p, b.locale.to_string()));
            }
            if !locale_ok(&b.locale) {
                issues.push(issue("invalid_locale", &p, b.locale.to_string()));
            } else if !declared.contains(b.locale.as_str()) {
                issues.push(issue(
                    "locale_undeclared",
                    &p,
                    format!(
                        "bundle `{}` is neither in i18n.locales nor the default locale",
                        b.locale
                    ),
                ));
            }
            for key in b.messages.keys() {
                if !key.0.starts_with(&ns) {
                    issues.push(issue(
                        "key_namespace",
                        format!("{p}.messages"),
                        format!("message key `{}` must start with `{ns}`", key.0),
                    ));
                }
            }
        }
        // Asset paths: valid, unique case-insensitively (one file on macOS / Windows), and
        // no asset below another (`a` and `a/b`): the Vfs would refuse the second write
        // AFTER the first files are written.
        let mut asset_names: BTreeSet<String> = BTreeSet::new();
        for (rel, _) in assets {
            if VfsPath::parse(&format!("/assets/{rel}")).is_err()
                || !asset_names.insert(rel.to_ascii_lowercase())
            {
                issues.push(issue("invalid_asset", "assets", rel.to_string()));
            }
        }
        for a in &asset_names {
            let as_dir = format!("{a}/");
            if asset_names.iter().any(|b| b.starts_with(&as_dir)) {
                issues.push(issue(
                    "invalid_asset",
                    "assets",
                    format!("`{a}` is both a file and a directory"),
                ));
            }
        }
        if !issues.is_empty() {
            return Err(invalid(issues));
        }

        // 2. Files of the version (relative path -> bytes), deterministic.
        let mut files: BTreeMap<String, Vec<u8>> = BTreeMap::new();
        files.insert(DESCRIPTOR_FILE.into(), json(&desc)?);
        for b in bundles {
            files.insert(format!("i18n/{}.json", b.locale), json(b)?);
        }
        for (rel, bytes) in assets {
            files.insert(format!("assets/{rel}"), bytes.to_vec());
        }
        let etags: BTreeMap<String, String> = files
            .iter()
            .map(|(k, v)| (k.clone(), super::etag_of(v)))
            .collect();

        let _g = self.write_lock.lock().await;
        let dir = self.version_dir(&desc.name, &desc.version)?;
        let manifest_path = Self::rel(&dir, MANIFEST_FILE)?;

        // 3. Immutability: same content = idempotent, other content = refused.
        if let Some(m) = self.manifest(&desc.name, &desc.version).await? {
            if m.files == etags {
                return self
                    .vfs
                    .stat(&manifest_path)
                    .await?
                    .ok_or_else(|| integrity(&manifest_path, "manifest vanished"));
            }
            return Err(immutable(&dir));
        }

        // 4. Compatibility against EVERY stored version (yanked included: a yanked
        //    version's tags stay published), in semver order (`compat_with`): a removed
        //    version's reservations are still carried by the versions after it, and a
        //    backport is checked against the versions above it.
        let mut issues = Vec::new();
        for other in self.versions(&desc.name).await? {
            let other = self.get_checked(&desc.name, &other.version, false).await?;
            issues.extend(compat_with(&other, &desc));
        }
        if !issues.is_empty() {
            issues.sort_by(|a, b| (&a.path, &a.message).cmp(&(&b.path, &b.message)));
            issues.dedup();
            return Err(invalid(issues));
        }

        // 5. Orphans of a crashed put of this version (no manifest, so invisible) are
        //    removed first: their shape could block this put (`assets/a` left as a file
        //    where this put writes `assets/a/b`).
        for f in self.files_below(&dir).await? {
            match self.vfs.delete(&f, Precondition::None).await {
                Ok(()) | Err(VfsError::NotFound(_)) => {}
                Err(e) => return Err(e.into()),
            }
        }

        // 6. Files, then the commit marker.
        for (rel, bytes) in &files {
            self.vfs
                .write(&Self::rel(&dir, rel)?, bytes, Precondition::None)
                .await?;
        }
        let mut manifest = Manifest {
            format: MANIFEST_FORMAT,
            name: desc.name.clone(),
            version: desc.version.clone(),
            files: etags,
            created: Utc::now(),
            signature: None,
        };
        // Publisher side: sign the record we are about to commit, so a host with
        // an anchor can authenticate it. Signing here (rather than in a separate
        // tool) keeps the signed bytes and the stored bytes the same by
        // construction — the classic way to ship a signature that verifies
        // nowhere.
        #[cfg(feature = "module-signing")]
        if let Some((key_id, key)) = self.signing.as_ref() {
            use base64::Engine as _;
            use ed25519_dalek::Signer as _;

            let signature = key.sign(&canonical_manifest_bytes(&manifest)?);
            manifest.signature = Some(SignatureBlock {
                key_id: key_id.clone(),
                signature: base64::engine::general_purpose::STANDARD.encode(signature.to_bytes()),
            });
        }
        match self
            .vfs
            .write(&manifest_path, &json(&manifest)?, Precondition::IfNoneMatch)
            .await
        {
            Ok(meta) => Ok(meta),
            // Another process committed this version between our check and now.
            Err(VfsError::PreconditionFailed { .. }) => Err(immutable(&dir)),
            Err(e) => Err(e.into()),
        }
    }

    async fn get_checked(
        &self,
        name: &str,
        version: &str,
        check: bool,
    ) -> Result<ModuleDescriptor, ContractError> {
        let dir = self.version_dir(name, version)?;
        let m = self
            .manifest(name, version)
            .await?
            .ok_or_else(|| ContractError::from(VfsError::NotFound(dir.clone())))?;
        let bytes = self
            .read_verified(&dir, &m, DESCRIPTOR_FILE)
            .await?
            .ok_or_else(|| integrity(&dir, "manifest lists no descriptor"))?;
        let text =
            std::str::from_utf8(&bytes).map_err(|_| integrity(&dir, "descriptor is not UTF-8"))?;
        let desc = ModuleDescriptor::from_json(text)?;
        if desc.name != name || desc.version != version {
            return Err(ContractError::framework(codes::MODULE_IDENTITY_MISMATCH)
                .with_param("path", dir.as_str())
                .with_param("found", format!("{}@{}", desc.name, desc.version)));
        }
        if check {
            desc.check(&self.registry, self.profile())
                .map_err(invalid)?;
        }
        Ok(desc)
    }

    /// Hide `version` from `get(name, None)` (rollback to the previous version).
    pub async fn yank(&self, name: &str, version: &str) -> Result<(), ContractError> {
        let _g = self.write_lock.lock().await;
        if self.manifest(name, version).await?.is_none() {
            return Err(VfsError::NotFound(self.version_dir(name, version)?).into());
        }
        let marker = self.yank_marker(name, version)?;
        let body = json(&serde_json::json!({ "yanked_at": Utc::now() }))?;
        match self
            .vfs
            .write(&marker, &body, Precondition::IfNoneMatch)
            .await
        {
            Ok(_) | Err(VfsError::PreconditionFailed { .. }) => Ok(()), // idempotent
            Err(e) => Err(e.into()),
        }
    }

    /// Undo `yank`.
    pub async fn unyank(&self, name: &str, version: &str) -> Result<(), ContractError> {
        let _g = self.write_lock.lock().await;
        match self
            .vfs
            .delete(&self.yank_marker(name, version)?, Precondition::None)
            .await
        {
            Ok(()) | Err(VfsError::NotFound(_)) => Ok(()),
            Err(e) => Err(e.into()),
        }
    }

    /// Artifact `assets/<rel>` of a committed version, verified against the manifest.
    pub async fn asset(
        &self,
        name: &str,
        version: &str,
        rel: &str,
    ) -> Result<Option<Vec<u8>>, ContractError> {
        let dir = self.version_dir(name, version)?;
        let Some(m) = self.manifest(name, version).await? else {
            return Err(VfsError::NotFound(dir).into());
        };
        self.read_verified(&dir, &m, &format!("assets/{rel}")).await
    }

    async fn files_below(&self, dir: &VfsPath) -> Result<Vec<VfsPath>, ContractError> {
        let mut out = Vec::new();
        let mut stack = vec![dir.clone()];
        while let Some(d) = stack.pop() {
            for e in self.vfs.list(&d).await? {
                match e {
                    VfsEntry::File { path, .. } => out.push(path),
                    VfsEntry::Dir { path } => stack.push(path),
                }
            }
        }
        Ok(out)
    }
}

#[async_trait]
impl ModuleStore for VfsModuleStore {
    async fn put(
        &self,
        descriptor: &ModuleDescriptor,
        bundles: &[MessageBundle],
    ) -> Result<VfsMeta, ContractError> {
        self.put_with_assets(descriptor, bundles, &[]).await
    }

    async fn get(
        &self,
        name: &str,
        version: Option<&str>,
    ) -> Result<ModuleDescriptor, ContractError> {
        let version = match version {
            Some(v) => v.to_string(),
            None => match self.active_version(name).await? {
                Some(v) => v,
                None => return Err(VfsError::NotFound(self.module_dir(name)?).into()),
            },
        };
        self.get_checked(name, &version, true).await
    }

    async fn bundle(
        &self,
        name: &str,
        version: &str,
        locale: &str,
    ) -> Result<Option<MessageBundle>, ContractError> {
        let dir = self.version_dir(name, version)?;
        let Some(m) = self.manifest(name, version).await? else {
            return Err(VfsError::NotFound(dir).into());
        };
        let rel = format!("i18n/{locale}.json");
        if VfsPath::parse(&format!("/{rel}")).is_err() {
            return Ok(None);
        }
        let Some(bytes) = self.read_verified(&dir, &m, &rel).await? else {
            return Ok(None);
        };
        let path = Self::rel(&dir, &rel)?;
        let b: MessageBundle =
            serde_json::from_slice(&bytes).map_err(|e| integrity(&path, &e.to_string()))?;
        if b.module != name || b.locale.as_str() != locale {
            return Err(integrity(
                &path,
                "bundle module or locale differs from its path",
            ));
        }
        Ok(Some(b))
    }

    async fn list(&self) -> Result<Vec<(String, String)>, ContractError> {
        let mut out = Vec::new();
        for e in self.vfs.list(&self.root).await? {
            let VfsEntry::Dir { path } = e else { continue };
            let Some(name) = path.file_name() else {
                continue;
            };
            for v in self.versions(name).await? {
                out.push((name.to_string(), v.version));
            }
        }
        Ok(out)
    }

    async fn remove(&self, name: &str, version: &str) -> Result<(), ContractError> {
        let _g = self.write_lock.lock().await;
        let dir = self.version_dir(name, version)?;
        let manifest_path = Self::rel(&dir, MANIFEST_FILE)?;
        let Some(meta) = self.vfs.stat(&manifest_path).await? else {
            return Err(VfsError::NotFound(dir).into());
        };
        // Un-commit first: from here on the version is invisible, whatever happens next.
        self.vfs
            .delete(&manifest_path, Precondition::IfMatch(meta.etag))
            .await?;
        for f in self.files_below(&dir).await? {
            match self.vfs.delete(&f, Precondition::None).await {
                Ok(()) | Err(VfsError::NotFound(_)) => {}
                Err(e) => return Err(e.into()),
            }
        }
        match self
            .vfs
            .delete(&self.yank_marker(name, version)?, Precondition::None)
            .await
        {
            Ok(()) | Err(VfsError::NotFound(_)) => Ok(()),
            Err(e) => Err(e.into()),
        }
    }
}

/// Read a module from its source the way `ModuleLoader` consumes it: `Stored` sources
/// give the checked descriptor and the bundles of every declared locale (default locale
/// included); `Compiled` sources return `None` (their descriptor comes from the code).
pub async fn read_source(source: &ModuleSource) -> Result<Option<StoredModule>, ContractError> {
    let ModuleSource::Stored {
        store,
        name,
        version,
    } = source
    else {
        return Ok(None);
    };
    load_stored(store.as_ref(), name, version.as_deref())
        .await
        .map(Some)
}

/// Descriptor + bundles of a stored module (`version: None` = active version).
pub async fn load_stored(
    store: &dyn ModuleStore,
    name: &str,
    version: Option<&str>,
) -> Result<StoredModule, ContractError> {
    let descriptor = store.get(name, version).await?;
    let mut locales: BTreeSet<Locale> = descriptor.i18n.locales.iter().cloned().collect();
    locales.insert(descriptor.i18n.default_locale.clone());
    let mut bundles = Vec::new();
    for l in locales {
        if let Some(b) = store.bundle(name, &descriptor.version, l.as_str()).await? {
            bundles.push(b);
        }
    }
    Ok(StoredModule {
        descriptor,
        bundles,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::descriptor::field::{FieldDesc, FieldType};
    use crate::descriptor::{
        EntityDesc, ErrorKind, MessageKey, MessageSyntax, Operation, PolicyExpr,
    };
    use crate::storage::vfs::{InMemoryVfs, LocalFsVfs};

    fn module(version: &str, fields: Vec<FieldDesc>, reserved: Vec<u32>) -> ModuleDescriptor {
        let mut m = ModuleDescriptor::new("billing", version);
        let mut e = EntityDesc::new("invoice");
        e.fields = fields;
        e.reserved_tags = reserved;
        for op in Operation::CRUD {
            e.permissions.ops.insert(op, PolicyExpr::Authenticated);
        }
        m.entities.push(e);
        m
    }

    fn v1(version: &str) -> ModuleDescriptor {
        module(
            version,
            vec![
                FieldDesc::new("amount", FieldType::F64).tag(16),
                FieldDesc::new("note", FieldType::String).tag(17),
            ],
            vec![],
        )
    }

    fn bundle(locale: &str, text: &str) -> MessageBundle {
        let mut messages = BTreeMap::new();
        messages.insert(MessageKey("billing.invoice.title".into()), text.to_string());
        MessageBundle {
            module: "billing".into(),
            locale: Locale::new(locale),
            syntax: MessageSyntax::Simple,
            messages,
        }
    }

    fn mem() -> (Arc<InMemoryVfs>, VfsModuleStore) {
        let vfs = Arc::new(InMemoryVfs::new());
        (vfs.clone(), VfsModuleStore::new(vfs))
    }

    async fn all_files(vfs: &dyn Vfs) -> usize {
        let mut n = 0;
        let mut stack = vec![VfsPath::parse("/").unwrap()];
        while let Some(d) = stack.pop() {
            for e in vfs.list(&d).await.unwrap() {
                match e {
                    VfsEntry::File { .. } => n += 1,
                    VfsEntry::Dir { path } => stack.push(path),
                }
            }
        }
        n
    }

    #[tokio::test]
    async fn module_store_put_rejects_invalid_descriptor() {
        let (vfs, store) = mem();
        // No explicit policies: refused by check(Stored).
        let mut m = v1("1.0.0");
        m.entities[0].permissions.ops.clear();
        let err = store.put(&m, &[]).await.unwrap_err();
        assert_eq!(err.code.0, codes::DESCRIPTOR_INVALID);
        assert!(format!("{:?}", err.params).contains("policy_missing"));
        // `public` refused unless the host allows it.
        let mut p = v1("1.0.0");
        p.entities[0]
            .permissions
            .ops
            .insert(Operation::CRUD[0].clone(), PolicyExpr::Public);
        let err = store.put(&p, &[]).await.unwrap_err();
        assert!(format!("{:?}", err.params).contains("public_refused"));
        // Bundle of another module, key outside the namespace, duplicate locale.
        let mut foreign = bundle("fr", "Facture");
        foreign.module = "crm".into();
        let mut stray = bundle("en", "Invoice");
        stray
            .messages
            .insert(MessageKey("crm.x".into()), "x".into());
        let err = store
            .put(&v1("1.0.0"), &[foreign, stray, bundle("en", "dup")])
            .await
            .unwrap_err();
        let params = format!("{:?}", err.params);
        for reason in ["bundle_module", "key_namespace", "duplicate_locale"] {
            assert!(params.contains(reason), "{reason} in {params}");
        }
        // Nothing was written by any refused put.
        assert_eq!(all_files(vfs.as_ref()).await, 0);
        assert!(store.list().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn module_store_put_rejects_tag_reuse() {
        let (_vfs, store) = mem();
        store.put(&v1("1.0.0"), &[]).await.unwrap();
        // `note` removed and its tag 17 reused by `memo`.
        let reused = module(
            "1.1.0",
            vec![
                FieldDesc::new("amount", FieldType::F64).tag(16),
                FieldDesc::new("memo", FieldType::String).tag(17),
            ],
            vec![],
        );
        let err = store.put(&reused, &[]).await.unwrap_err();
        assert_eq!(err.code.0, codes::DESCRIPTOR_INVALID);
        assert!(format!("{:?}", err.params).contains("tag_reused"));
        // Removal with the tag reserved is accepted.
        let ok = module(
            "1.1.0",
            vec![FieldDesc::new("amount", FieldType::F64).tag(16)],
            vec![17],
        );
        store.put(&ok, &[]).await.unwrap();
        // Compat is checked against the highest version even when it is yanked.
        store.yank("billing", "1.1.0").await.unwrap();
        let reuse_again = module(
            "1.2.0",
            vec![
                FieldDesc::new("amount", FieldType::F64).tag(16),
                FieldDesc::new("memo", FieldType::String).tag(17),
            ],
            vec![],
        );
        let err = store.put(&reuse_again, &[]).await.unwrap_err();
        assert!(format!("{:?}", err.params).contains("tag_reused"));
        // Once 1.0.0 is removed, the reservation of 1.1.0 alone still protects tag 17
        // (check_compat reads prev.reserved_tags).
        store.unyank("billing", "1.1.0").await.unwrap();
        store.remove("billing", "1.0.0").await.unwrap();
        let err = store.put(&reuse_again, &[]).await.unwrap_err();
        assert!(format!("{:?}", err.params).contains("reserved by version 1.1.0"));
    }

    #[tokio::test]
    async fn module_store_get_highest_version() {
        let (_vfs, store) = mem();
        for v in ["1.0.0", "1.10.0", "1.2.0", "1.10.0-rc.1"] {
            store.put(&v1(v), &[]).await.unwrap();
        }
        // semver, not lexicographic: 1.10.0 > 1.10.0-rc.1 > 1.2.0 > 1.0.0.
        assert_eq!(store.get("billing", None).await.unwrap().version, "1.10.0");
        assert_eq!(
            store.get("billing", Some("1.2.0")).await.unwrap().version,
            "1.2.0"
        );
        let vs: Vec<String> = store
            .versions("billing")
            .await
            .unwrap()
            .into_iter()
            .map(|v| v.version)
            .collect();
        assert_eq!(vs, ["1.0.0", "1.2.0", "1.10.0-rc.1", "1.10.0"]);
        assert_eq!(store.list().await.unwrap().len(), 4);
        // Unknown module / version: this.vfs.not_found.
        let e = store.get("crm", None).await.unwrap_err();
        assert_eq!(e.code.0, codes::VFS_NOT_FOUND);
        let e = store.get("billing", Some("9.9.9")).await.unwrap_err();
        assert_eq!(e.code.0, codes::VFS_NOT_FOUND);
    }

    #[tokio::test]
    async fn module_store_bundles_per_locale() {
        let (_vfs, store) = mem();
        let mut m = v1("1.0.0");
        m.i18n.locales = vec![Locale::new("fr"), Locale::new("en")];
        store
            .put(&m, &[bundle("fr", "Facture"), bundle("en", "Invoice")])
            .await
            .unwrap();
        let fr = store
            .bundle("billing", "1.0.0", "fr")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            fr.get(&MessageKey("billing.invoice.title".into())),
            Some("Facture")
        );
        assert!(
            store
                .bundle("billing", "1.0.0", "de")
                .await
                .unwrap()
                .is_none()
        );
        assert!(
            store
                .bundle("billing", "1.0.0", "../x")
                .await
                .unwrap()
                .is_none()
        );
        // The descriptor carries no text.
        let d = store.get("billing", None).await.unwrap();
        assert!(!d.to_json_pretty().contains("Facture"));
    }

    #[tokio::test]
    async fn module_store_versions_are_immutable() {
        let (vfs, store) = mem();
        let m = v1("1.0.0");
        let first = store.put(&m, &[bundle("fr", "Facture")]).await.unwrap();
        // Same content again: idempotent.
        let again = store.put(&m, &[bundle("fr", "Facture")]).await.unwrap();
        assert_eq!(first.etag, again.etag);
        // Different content under the same version: 409 Aborted.
        let err = store.put(&m, &[bundle("fr", "Note")]).await.unwrap_err();
        assert_eq!(err.code.0, codes::VFS_PRECONDITION_FAILED);
        assert_eq!(err.kind, ErrorKind::Aborted);
        let fr = store
            .bundle("billing", "1.0.0", "fr")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(fr.messages.values().next().unwrap(), "Facture");
        // Tampering below the store is detected, never served.
        let p = VfsPath::parse("/modules/billing/1.0.0/i18n/fr.json").unwrap();
        vfs.write(
            &p,
            b"{\"module\":\"billing\",\"locale\":\"fr\",\"messages\":{}}",
            Precondition::None,
        )
        .await
        .unwrap();
        let err = store.bundle("billing", "1.0.0", "fr").await.unwrap_err();
        assert_eq!(err.code.0, codes::INTERNAL);
    }

    #[tokio::test]
    async fn module_store_uncommitted_version_is_invisible() {
        let (vfs, store) = mem();
        store.put(&v1("1.0.0"), &[]).await.unwrap();
        // A crashed put: files written, no manifest.
        let orphan = VfsPath::parse("/modules/billing/2.0.0/descriptor.json").unwrap();
        vfs.write(
            &orphan,
            v1("2.0.0").to_json_pretty().as_bytes(),
            Precondition::None,
        )
        .await
        .unwrap();
        assert_eq!(store.get("billing", None).await.unwrap().version, "1.0.0");
        assert_eq!(store.list().await.unwrap().len(), 1);
        // The next put of that version overwrites the orphan and commits.
        store.put(&v1("2.0.0"), &[]).await.unwrap();
        assert_eq!(store.get("billing", None).await.unwrap().version, "2.0.0");
    }

    #[tokio::test]
    async fn module_store_yank_rolls_back_and_unyank_restores() {
        let (_vfs, store) = mem();
        store.put(&v1("1.0.0"), &[]).await.unwrap();
        store.put(&v1("1.1.0"), &[]).await.unwrap();
        store.yank("billing", "1.1.0").await.unwrap();
        store.yank("billing", "1.1.0").await.unwrap(); // idempotent
        assert_eq!(store.get("billing", None).await.unwrap().version, "1.0.0");
        // Still served by exact version, still listed.
        assert!(store.get("billing", Some("1.1.0")).await.is_ok());
        assert!(store.versions("billing").await.unwrap()[1].yanked);
        store.unyank("billing", "1.1.0").await.unwrap();
        assert_eq!(store.get("billing", None).await.unwrap().version, "1.1.0");
        // Everything yanked: no active version.
        store.yank("billing", "1.0.0").await.unwrap();
        store.yank("billing", "1.1.0").await.unwrap();
        assert_eq!(
            store.get("billing", None).await.unwrap_err().code.0,
            codes::VFS_NOT_FOUND
        );
        assert!(store.yank("billing", "3.0.0").await.is_err());
    }

    #[tokio::test]
    async fn module_store_remove_and_assets() {
        let (vfs, store) = mem();
        let wasm = b"\0asm\x01\0\0\0";
        store
            .put_with_assets(
                &v1("1.0.0"),
                &[bundle("fr", "Facture")],
                &[("front.wasm", wasm)],
            )
            .await
            .unwrap();
        assert_eq!(
            store
                .asset("billing", "1.0.0", "front.wasm")
                .await
                .unwrap()
                .unwrap(),
            wasm
        );
        assert!(
            store
                .asset("billing", "1.0.0", "nope")
                .await
                .unwrap()
                .is_none()
        );
        assert!(
            store
                .put_with_assets(&v1("1.1.0"), &[], &[("../x", b"x")])
                .await
                .is_err()
        );
        store.yank("billing", "1.0.0").await.unwrap();
        store.remove("billing", "1.0.0").await.unwrap();
        assert!(store.list().await.unwrap().is_empty());
        assert_eq!(all_files(vfs.as_ref()).await, 0, "no file left behind");
        assert_eq!(
            store.remove("billing", "1.0.0").await.unwrap_err().code.0,
            codes::VFS_NOT_FOUND
        );
    }

    #[tokio::test]
    async fn module_store_get_rechecks_with_current_registry() {
        let (vfs, store) = mem();
        store.put(&v1("1.0.0"), &[]).await.unwrap();
        // A descriptor that a stricter host refuses: allow_public=false while the
        // stored one declared public with a permissive store.
        let lax = VfsModuleStore::new(vfs.clone()).allow_public(true);
        let mut p = v1("2.0.0");
        p.entities[0]
            .permissions
            .ops
            .insert(Operation::CRUD[0].clone(), PolicyExpr::Public);
        lax.put(&p, &[]).await.unwrap();
        assert_eq!(lax.get("billing", None).await.unwrap().version, "2.0.0");
        let err = store.get("billing", None).await.unwrap_err();
        assert_eq!(err.code.0, codes::DESCRIPTOR_INVALID);
    }

    #[tokio::test]
    async fn module_store_on_local_fs_survives_reopen() {
        let dir = tempfile::tempdir().unwrap();
        {
            let store = VfsModuleStore::new(Arc::new(LocalFsVfs::open(dir.path()).unwrap()));
            store
                .put(&v1("1.0.0"), &[bundle("fr", "Facture")])
                .await
                .unwrap();
            store.put(&v1("1.1.0"), &[]).await.unwrap();
        }
        let store = VfsModuleStore::new(Arc::new(LocalFsVfs::open(dir.path()).unwrap()));
        assert_eq!(store.get("billing", None).await.unwrap().version, "1.1.0");
        assert!(
            store
                .bundle("billing", "1.0.0", "fr")
                .await
                .unwrap()
                .is_some()
        );
        assert!(
            dir.path()
                .join("modules/billing/1.0.0/i18n/fr.json")
                .is_file()
        );
    }

    #[tokio::test]
    async fn read_source_loads_stored_module() {
        let (_vfs, store) = mem();
        let mut m = v1("1.0.0");
        m.i18n.locales = vec![Locale::new("en")];
        store
            .put(&m, &[bundle("fr", "Facture"), bundle("en", "Invoice")])
            .await
            .unwrap();
        let store: Arc<dyn ModuleStore> = Arc::new(store);
        let src = ModuleSource::Stored {
            store,
            name: "billing".into(),
            version: None,
        };
        let loaded = read_source(&src).await.unwrap().unwrap();
        assert_eq!(loaded.descriptor.name, "billing");
        // `en` declared + `fr` default locale.
        let mut locales: Vec<_> = loaded
            .bundles
            .iter()
            .map(|b| b.locale.to_string())
            .collect();
        locales.sort();
        assert_eq!(locales, ["en", "fr"]);
    }

    #[tokio::test]
    async fn module_store_backport_checked_against_higher_versions() {
        let (_vfs, store) = mem();
        store.put(&v1("1.0.0"), &[]).await.unwrap();
        // 2.0.0 adds `due` (18) and removes `note` (17, reserved).
        let v2 = module(
            "2.0.0",
            vec![
                FieldDesc::new("amount", FieldType::F64).tag(16),
                FieldDesc::new("due", FieldType::String).tag(18),
            ],
            vec![17],
        );
        store.put(&v2, &[]).await.unwrap();
        // Backport 1.0.1 = the 1.0 shape: lacks `due`, keeps `note`. Accepted (before
        // the review, `check_compat(2.0.0, 1.0.1)` refused it with tag_not_reserved).
        store.put(&v1("1.0.1"), &[]).await.unwrap();
        assert_eq!(store.get("billing", None).await.unwrap().version, "2.0.0");
        // Backport binding 18 to another field than 2.0.0 does: refused.
        let bad = module(
            "1.0.2",
            vec![
                FieldDesc::new("amount", FieldType::F64).tag(16),
                FieldDesc::new("note", FieldType::String).tag(17),
                FieldDesc::new("memo", FieldType::String).tag(18),
            ],
            vec![],
        );
        let err = store.put(&bad, &[]).await.unwrap_err();
        assert_eq!(err.code.0, codes::DESCRIPTOR_INVALID);
        assert!(
            format!("{:?}", err.params).contains("tag_reused"),
            "{err:?}"
        );
        // Backport dropping `note` without reserving 17: refused against 1.0.0.
        let unreserved = module(
            "1.0.3",
            vec![FieldDesc::new("amount", FieldType::F64).tag(16)],
            vec![],
        );
        let err = store.put(&unreserved, &[]).await.unwrap_err();
        assert!(
            format!("{:?}", err.params).contains("tag_not_reserved"),
            "{err:?}"
        );
        assert_eq!(store.versions("billing").await.unwrap().len(), 3);
    }

    #[tokio::test]
    async fn module_store_put_rejects_bad_locales_and_assets() {
        let (vfs, store) = mem();
        let mut m = v1("1.0.0");
        m.i18n.locales = vec![Locale::new("fr-CA"), Locale::new("fr-ca")];
        // Same file on a case-insensitive file system.
        let err = store
            .put(&m, &[bundle("fr-CA", "a"), bundle("fr-ca", "b")])
            .await
            .unwrap_err();
        assert!(format!("{:?}", err.params).contains("duplicate_locale"));
        // A locale nobody will ever load.
        let err = store
            .put(&v1("1.0.0"), &[bundle("de", "Rechnung")])
            .await
            .unwrap_err();
        assert!(format!("{:?}", err.params).contains("locale_undeclared"));
        // Not a BCP 47 tag (would create i18n/fr/CA.json).
        let mut m = v1("1.0.0");
        m.i18n.locales = vec![Locale::new("fr/CA")];
        let err = store.put(&m, &[bundle("fr/CA", "x")]).await.unwrap_err();
        assert!(format!("{:?}", err.params).contains("invalid_locale"));
        // Assets: case-insensitive duplicates, file-and-directory.
        let err = store
            .put_with_assets(
                &v1("1.0.0"),
                &[],
                &[("Front.wasm", b"a"), ("front.wasm", b"b")],
            )
            .await
            .unwrap_err();
        assert!(format!("{:?}", err.params).contains("invalid_asset"));
        let err = store
            .put_with_assets(&v1("1.0.0"), &[], &[("a", b"a"), ("a/b", b"b")])
            .await
            .unwrap_err();
        assert!(
            format!("{:?}", err.params).contains("both a file and a directory"),
            "{err:?}"
        );
        // Every refusal happened before any write.
        assert_eq!(all_files(vfs.as_ref()).await, 0);
    }

    #[tokio::test]
    async fn module_store_orphans_do_not_block_the_next_put() {
        let (vfs, store) = mem();
        // A crashed put left `assets/a` as a FILE, the retry writes `assets/a/b`.
        let orphan = VfsPath::parse("/modules/billing/1.0.0/assets/a").unwrap();
        vfs.write(&orphan, b"stale", Precondition::None)
            .await
            .unwrap();
        store
            .put_with_assets(&v1("1.0.0"), &[], &[("a/b", b"fresh")])
            .await
            .unwrap();
        assert_eq!(
            store
                .asset("billing", "1.0.0", "a/b")
                .await
                .unwrap()
                .unwrap(),
            b"fresh"
        );
        assert!(vfs.stat(&orphan).await.unwrap().is_none(), "orphan removed");
    }

    #[tokio::test]
    async fn module_store_on_local_fs_refuses_tampering() {
        let dir = tempfile::tempdir().unwrap();
        let store = VfsModuleStore::new(Arc::new(LocalFsVfs::open(dir.path()).unwrap()));
        store
            .put_with_assets(
                &v1("1.0.0"),
                &[bundle("fr", "Facture")],
                &[("x.wasm", b"x")],
            )
            .await
            .unwrap();
        // Edited behind the store's back, on disk.
        std::fs::write(
            dir.path().join("modules/billing/1.0.0/assets/x.wasm"),
            b"evil",
        )
        .unwrap();
        let err = store.asset("billing", "1.0.0", "x.wasm").await.unwrap_err();
        assert_eq!(err.code.0, codes::INTERNAL);
        std::fs::remove_file(dir.path().join("modules/billing/1.0.0/descriptor.json")).unwrap();
        let err = store.get("billing", None).await.unwrap_err();
        assert_eq!(err.code.0, codes::INTERNAL);
        assert!(format!("{:?}", err.params).contains("file missing"));
    }
}

#[cfg(all(test, feature = "module-signing"))]
mod signing_tests {
    //! Signature des modules stockés : l'ancre de confiance, et ce qu'elle refuse.

    use super::*;
    use crate::descriptor::field::{FieldDesc, FieldType};
    use crate::descriptor::{EntityDesc, Operation, PolicyExpr};
    use crate::storage::vfs::InMemoryVfs;
    use ed25519_dalek::SigningKey;

    fn descriptor() -> ModuleDescriptor {
        let mut m = ModuleDescriptor::new("billing", "0.0.1");
        let mut e = EntityDesc::new("invoice");
        e.fields = vec![FieldDesc::new("amount", FieldType::F64).tag(16)];
        for op in Operation::CRUD {
            e.permissions.ops.insert(op, PolicyExpr::Authenticated);
        }
        m.entities.push(e);
        m
    }

    fn signing_key() -> SigningKey {
        SigningKey::from_bytes(&[7u8; 32])
    }

    fn other_key() -> SigningKey {
        SigningKey::from_bytes(&[9u8; 32])
    }

    fn anchor_trusting(key: &SigningKey) -> TrustAnchor {
        let mut a = TrustAnchor::new();
        a.insert("release-2026", key.verifying_key());
        a
    }

    async fn store_with_signed_module() -> (Arc<InMemoryVfs>, VfsModuleStore) {
        let vfs = Arc::new(InMemoryVfs::new());
        let writer =
            VfsModuleStore::new(vfs.clone()).with_signing_key("release-2026", signing_key());
        writer
            .put(&descriptor(), &[])
            .await
            .expect("put signed module");
        (vfs, writer)
    }

    #[tokio::test]
    async fn a_signed_module_loads_under_its_anchor() {
        let (vfs, _) = store_with_signed_module().await;
        let reader = VfsModuleStore::new(vfs).with_trust_anchor(anchor_trusting(&signing_key()));

        let loaded = reader.get("billing", None).await.expect("verified");
        assert_eq!(loaded.name, "billing");
    }

    #[tokio::test]
    async fn an_unsigned_module_is_refused_once_an_anchor_is_declared() {
        // The whole point: an operator who can write to the store cannot invent a
        // module. Before the anchor, `put` alone was enough.
        let vfs = Arc::new(InMemoryVfs::new());
        VfsModuleStore::new(vfs.clone())
            .put(&descriptor(), &[])
            .await
            .expect("put unsigned");

        let reader = VfsModuleStore::new(vfs).with_trust_anchor(anchor_trusting(&signing_key()));
        let err = reader
            .get("billing", None)
            .await
            .expect_err("unsigned must be refused");
        assert_eq!(err.code, codes::MODULE_UNSIGNED.into());
    }

    /// The anchor does not know the *key id* at all: a signer it never heard of.
    #[tokio::test]
    async fn a_module_signed_by_an_unknown_key_id_is_refused() {
        let (vfs, _) = store_with_signed_module().await;
        let mut anchor = TrustAnchor::new();
        anchor.insert("someone-else", other_key().verifying_key());
        let reader = VfsModuleStore::new(vfs).with_trust_anchor(anchor);

        let err = reader
            .get("billing", None)
            .await
            .expect_err("unknown signer");
        assert_eq!(err.code, codes::MODULE_UNTRUSTED_KEY.into());
    }

    /// The anchor knows the key id but holds a different key: a forged signature
    /// claiming a trusted name.
    #[tokio::test]
    async fn a_forged_signature_under_a_trusted_key_id_is_refused() {
        let (vfs, _) = store_with_signed_module().await;
        let reader = VfsModuleStore::new(vfs).with_trust_anchor(anchor_trusting(&other_key()));

        let err = reader.get("billing", None).await.expect_err("forged");
        assert_eq!(err.code, codes::MODULE_BAD_SIGNATURE.into());
    }

    #[tokio::test]
    async fn a_manifest_rewritten_after_signing_is_refused() {
        // The attack the signature exists for: keep a valid signature, change what
        // it covers. Here, point an etag at another value.
        let (vfs, writer) = store_with_signed_module().await;
        let dir = writer.version_dir("billing", "0.0.1").expect("version dir");
        let path = VfsModuleStore::rel(&dir, MANIFEST_FILE).expect("manifest path");

        let bytes = vfs.read(&path).await.expect("read manifest");
        let mut m: Manifest = serde_json::from_slice(&bytes).expect("parse manifest");
        let victim = m.files.keys().next().cloned().expect("at least one file");
        m.files.insert(victim, "etag-bricole".to_string());
        vfs.write(&path, &serde_json::to_vec(&m).unwrap(), Precondition::None)
            .await
            .expect("rewrite manifest");

        let reader =
            VfsModuleStore::new(vfs.clone()).with_trust_anchor(anchor_trusting(&signing_key()));
        let err = reader
            .get("billing", None)
            .await
            .expect_err("tampered manifest");
        assert_eq!(err.code, codes::MODULE_BAD_SIGNATURE.into());
    }

    #[tokio::test]
    async fn without_an_anchor_the_historical_behaviour_is_kept() {
        let vfs = Arc::new(InMemoryVfs::new());
        VfsModuleStore::new(vfs.clone())
            .put(&descriptor(), &[])
            .await
            .expect("put unsigned");

        let reader = VfsModuleStore::new(vfs);
        assert_eq!(
            reader.get("billing", None).await.expect("no anchor").name,
            "billing",
            "a host that declares no anchor keeps integrity-only behaviour"
        );
    }
}
