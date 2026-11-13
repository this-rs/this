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
//!   locale declared and unique case-insensitively), asset checks (valid path, no
//!   case-insensitive duplicate, non-empty, under [`MAX_ASSET_BYTES`]), and
//!   `check_compat` against EVERY stored version in semver order (yanked included: a
//!   yanked version's tags stay published; a backport is checked against the versions
//!   above it) run BEFORE anything is written.
//! - **Fail-closed code**: a version that carries executable code (`program.wasm` or any
//!   `.wasm` asset) is admitted only under a declared trust anchor. A store with no
//!   anchor refuses such a version with `this.module.unsigned`.
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
// La disposition sur le VFS et le format du manifeste sont partagés avec le client :
// ils viennent de `this-vfs`, pas d'ici. C'est ce qui rend un module écrit par une
// page lisible (et vérifiable) par un serveur qui n'a jamais vu ce client.
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
#[cfg(feature = "module-signing")]
use this_vfs::layout::SignatureBlock;
// The anchor's key entries are shared with the client (`this-vfs`) and the app:
// re-export the record shape so callers import it from one path.
#[cfg(feature = "module-signing")]
pub use this_vfs::layout::{AnchorKey, AnchorKeyRole, AnchorKeyState};
use this_vfs::layout::{
    DESCRIPTOR_FILE, MANIFEST_FILE, MANIFEST_FORMAT, MAX_ASSET_BYTES, Manifest, YANKED_DIR,
};
// Le format de paquet (export/import) vit aussi dans `this-vfs` : un paquet écrit
// par une page est lisible (et vérifiable) par un hôte qui n'a jamais vu ce client.
#[cfg(feature = "module-signing")]
use this_vfs::layout::{ModulePackage, PACKAGE_FORMAT, package_path_allowed};

/// One key a host admits, with its lifecycle state.
///
/// Verification consults `state` alone (`Active` admits; `Revoked` and absent
/// refuse with the same stable verdict). `role` and `added_at` are provenance
/// only: the anchor says « this code comes from a source we admit », never
/// « this code may do this or that ».
#[cfg(feature = "module-signing")]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrustedKey {
    pub key: ed25519_dalek::VerifyingKey,
    pub state: AnchorKeyState,
    pub added_at: DateTime<Utc>,
    pub role: AnchorKeyRole,
}

/// The keys a host accepts for stored modules.
///
/// Fail-closed by construction: a store that has an anchor refuses a manifest
/// that carries no signature, and one signed by an unknown or revoked key. A
/// store with no anchor behaves as before — integrity against the manifest, not
/// authentication of it.
///
/// An anchor holds a *set* of keys: the host's own, plus one per paired peer.
/// Revocation is a live mutation, not a cache — calling [`TrustAnchor::revoke`]
/// affects the very next verification, no restart.
#[cfg(feature = "module-signing")]
#[derive(Debug, Clone, Default)]
pub struct TrustAnchor {
    keys: BTreeMap<String, TrustedKey>,
}

#[cfg(feature = "module-signing")]
impl TrustAnchor {
    pub fn new() -> Self {
        Self::default()
    }

    /// Trust a key given as a SubjectPublicKeyInfo PEM (the format
    /// `openssl genpkey`/`this sign` produce). Added `active`, role `ownership`.
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
        self.keys.insert(
            key_id.into(),
            TrustedKey {
                key,
                state: AnchorKeyState::Active,
                added_at: Utc::now(),
                role: AnchorKeyRole::Ownership,
            },
        );
        Ok(())
    }

    /// Trust an already-parsed key (tests, or a host that reads keys from its own
    /// configuration store). Backward-compatible shorthand for the host's own
    /// key: added `active`, role `ownership`.
    pub fn insert(&mut self, key_id: impl Into<String>, key: ed25519_dalek::VerifyingKey) {
        self.keys.insert(
            key_id.into(),
            TrustedKey {
                key,
                state: AnchorKeyState::Active,
                added_at: Utc::now(),
                role: AnchorKeyRole::Ownership,
            },
        );
    }

    /// Trust a paired peer's key: added `active`, role `shared`.
    pub fn insert_peer(&mut self, key_id: impl Into<String>, key: ed25519_dalek::VerifyingKey) {
        self.keys.insert(
            key_id.into(),
            TrustedKey {
                key,
                state: AnchorKeyState::Active,
                added_at: Utc::now(),
                role: AnchorKeyRole::Shared,
            },
        );
    }

    /// Full control over an entry: its state, its role, and the moment it was
    /// added. Used to reload a persisted anchor without losing `added_at`.
    pub fn insert_trusted(
        &mut self,
        key_id: impl Into<String>,
        key: ed25519_dalek::VerifyingKey,
        state: AnchorKeyState,
        role: AnchorKeyRole,
        added_at: DateTime<Utc>,
    ) {
        self.keys.insert(
            key_id.into(),
            TrustedKey {
                key,
                state,
                added_at,
                role,
            },
        );
    }

    /// Revoke a key. It stays *known* (so a host can tell « revoked » from
    /// « never seen ») but no longer authenticates. Applies to the next
    /// verification: the anchor is a live field, not an immutable cache.
    ///
    /// Returns `false` when the key is absent.
    pub fn revoke(&mut self, key_id: &str) -> bool {
        match self.keys.get_mut(key_id) {
            Some(entry) => {
                entry.state = AnchorKeyState::Revoked;
                true
            }
            None => false,
        }
    }

    /// The entry for a key id, whatever its state.
    pub fn get(&self, key_id: &str) -> Option<&TrustedKey> {
        self.keys.get(key_id)
    }

    /// The anchor's keys as their serializable records, for persistence or to
    /// share the trust set with a peer.
    pub fn list(&self) -> Vec<AnchorKey> {
        self.keys
            .iter()
            .map(|(id, entry)| AnchorKey {
                key_id: id.clone(),
                public_key: hex_key(&entry.key),
                state: entry.state,
                added_at: entry.added_at,
                role: entry.role,
            })
            .collect()
    }

    /// Is this anchor empty? An empty anchor is a misconfiguration, not a policy:
    /// the store refuses everything rather than accepting everything.
    pub fn is_empty(&self) -> bool {
        self.keys.is_empty()
    }
}

/// Lower-case hex of an Ed25519 public key (32 bytes).
#[cfg(feature = "module-signing")]
fn hex_key(key: &ed25519_dalek::VerifyingKey) -> String {
    let mut s = String::with_capacity(64);
    for b in &key.to_bytes()[..] {
        s.push_str(&format!("{b:02x}"));
    }
    s
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

/// Whether an asset carries executable code: `program.wasm` and any other `.wasm`
/// file. Such a version is admitted only under a declared trust anchor (fail-closed).
fn is_code_asset(rel: &str) -> bool {
    rel.ends_with(".wasm")
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

/// Sign a manifest with a key, in place. Ed25519 as used here (RFC 8032) is
/// deterministic: signing the same bytes twice with the same key gives the same
/// signature, which is what makes an exported package reproducible byte for byte.
#[cfg(feature = "module-signing")]
fn signed_manifest(
    mut manifest: Manifest,
    key_id: &str,
    key: &ed25519_dalek::SigningKey,
) -> Manifest {
    use base64::Engine as _;
    use ed25519_dalek::Signer as _;
    let signature = key.sign(&manifest.signing_bytes());
    manifest.signature = Some(SignatureBlock {
        key_id: key_id.to_string(),
        signature: base64::engine::general_purpose::STANDARD.encode(signature.to_bytes()),
    });
    manifest
}

/// `ModuleStore` over a `Vfs`.
pub struct VfsModuleStore {
    vfs: Arc<dyn Vfs>,
    registry: Arc<CustomRegistry>,
    allow_public: bool,
    /// `true` : hôte à principal unique (app locale, CLI, runtime embarqué). Le module est
    /// écrit par celui qui l'exécute, donc l'exigence de politique explicite tombe — la
    /// validation structurelle, elle, reste entière.
    local: bool,
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
            local: false,
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

    /// Revoke a key of the trust anchor, live: the very next verification sees it
    /// as refused, no restart and no rebuild. Returns `false` when the store has
    /// no anchor or the key is absent.
    #[cfg(feature = "module-signing")]
    pub fn revoke_key(&mut self, key_id: &str) -> bool {
        self.trust
            .as_mut()
            .is_some_and(|anchor| anchor.revoke(key_id))
    }

    /// Registry of the host's custom validators / transforms / types (the one `check`
    /// must see, at put AND at get).
    pub fn with_registry(mut self, registry: Arc<CustomRegistry>) -> Self {
        self.registry = registry;
        self
    }

    /// Host decision: may a stored module declare `public` policies?
    /// Déclare le magasin « à principal unique » : un module déposé n'a personne à qui être
    /// accordé. C'est le régime d'une app locale, pas celui d'un serveur multi-locataire.
    pub fn local(mut self, local: bool) -> Self {
        self.local = local;
        self
    }

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
        if self.local {
            CheckProfile::Local {
                allow_public: self.allow_public,
            }
        } else {
            CheckProfile::Stored {
                allow_public: self.allow_public,
            }
        }
    }

    /// `true` when the host declared a trust anchor. Without the `module-signing`
    /// feature no anchor can exist, so code modules are always refused.
    fn anchor_declared(&self) -> bool {
        #[cfg(feature = "module-signing")]
        {
            self.trust.is_some()
        }
        #[cfg(not(feature = "module-signing"))]
        {
            false
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

    // `path` ne sert qu'a l'authentification (`module-signing`) : sans la feature, le
    // binding reste inutilise et `clippy -D warnings` refuse le fichier. Le taire ici
    // plutot que de renommer, pour que le code garde son sens quand la feature est active.
    #[cfg_attr(not(feature = "module-signing"), allow(unused_variables))]
    async fn manifest(&self, name: &str, version: &str) -> Result<Option<Manifest>, ContractError> {
        let Some((path, m)) = self.manifest_unverified(name, version).await? else {
            return Ok(None);
        };
        // Authentication before anything is read out of this manifest: the etags
        // it carries become trustworthy only once the record itself is.
        #[cfg(feature = "module-signing")]
        self.verify_manifest(&path, &m)?;
        Ok(Some(m))
    }

    /// Le manifeste tel qu'il est écrit, identité vérifiée mais signature NON authentifiée.
    ///
    /// Sert à l'INVENTAIRE (`versions`) : il doit **lister** une version héritée non signée
    /// pour que l'hôte sache qu'elle existe et puisse la remplacer, sans jamais la servir —
    /// `get` passe par [`Self::manifest`] et la refuse.
    ///
    /// Mesuré le 08/11/2026 dans l'Atelier : une seule version héritée non signée faisait
    /// échouer la liste ENTIÈRE, et comme `put` relit les versions du module pour vérifier
    /// la compatibilité, plus aucune version ne pouvait être rangée — le journal disait
    /// « modules non relus — échec : this.module.unsigned », puis « MANQUE — atelier
    /// installé en 0.7.0 n'a pas ses fichiers ».
    async fn manifest_unverified(
        &self,
        name: &str,
        version: &str,
    ) -> Result<Option<(VfsPath, Manifest)>, ContractError> {
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
        Ok(Some((path, m)))
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
        let Some(entry) = anchor.keys.get(&sig.key_id) else {
            return Err(ContractError::framework(codes::MODULE_UNTRUSTED_KEY)
                .with_param("path", path.as_str())
                .with_param("key_id", sig.key_id.clone())
                .with_param("state", "unknown"));
        };
        // A revoked key stays known but no longer admits anything: same stable
        // verdict as an unknown key, distinguished here so a host can tell the two
        // apart without a second code.
        if entry.state == AnchorKeyState::Revoked {
            return Err(ContractError::framework(codes::MODULE_UNTRUSTED_KEY)
                .with_param("path", path.as_str())
                .with_param("key_id", sig.key_id.clone())
                .with_param("state", "revoked"));
        }
        let key = &entry.key;
        let bytes = m.signing_bytes();
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
            // Une version rangée AVANT que l'hôte ne déclare une ancre n'a pas de
            // signature. `get` doit continuer à la refuser — rien de non signé n'est
            // jamais servi —, mais l'INVENTAIRE ne peut pas échouer pour autant : `put`
            // relit les versions du module pour vérifier la compatibilité, donc une seule
            // version héritée rendait le module IMPOSSIBLE À RÉÉCRIRE. Mesuré le
            // 08/11/2026 dans l'Atelier : « modules non relus — échec :
            // this.module.unsigned », puis « MANQUE — atelier installé en 0.7.0 n'a pas
            // ses fichiers ». On saute donc la version NON SIGNÉE, et rien d'autre : une
            // clé inconnue ou une signature invalide reste une erreur, parce que c'est
            // une altération, pas une migration.
            if let Some((_, m)) = self.manifest_unverified(name, v).await? {
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
        self.put_with_assets_with_required(descriptor, bundles, assets, &[])
            .await
    }

    /// [`put_with_assets`](Self::put_with_assets), where the descriptor is also checked
    /// against the entities of the modules it requires (transitively), looked up in `known`
    /// (see [`ModuleDescriptor::check_with_required`]). A module citing an entity of a
    /// module it does not require is still refused. With `known = &[]` this is exactly
    /// `put_with_assets`.
    pub async fn put_with_assets_with_required(
        &self,
        descriptor: &ModuleDescriptor,
        bundles: &[MessageBundle],
        assets: &[(&str, &[u8])],
        known: &[&ModuleDescriptor],
    ) -> Result<VfsMeta, ContractError> {
        // 1. Checks, nothing written yet.
        let mut desc = descriptor.clone();
        desc.normalize();
        desc.check_with_required(&self.registry, self.profile(), known)
            .map_err(invalid)?;
        // Fail-closed: a module that carries code (`program.wasm` or any `.wasm`
        // asset) is admitted only under a declared trust anchor. Without an anchor,
        // nothing can authenticate that code, so the version is refused before any
        // byte is written.
        if !self.anchor_declared() && assets.iter().any(|(rel, _)| is_code_asset(rel)) {
            return Err(ContractError::framework(codes::MODULE_UNSIGNED)
                .with_param("module", format!("{}@{}", desc.name, desc.version))
                .with_param("reason", "module with code, no trust anchor declared"));
        }
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
        for (rel, bytes) in assets {
            if VfsPath::parse(&format!("/assets/{rel}")).is_err()
                || !asset_names.insert(rel.to_ascii_lowercase())
            {
                issues.push(issue("invalid_asset", "assets", rel.to_string()));
            }
            if bytes.is_empty() {
                issues.push(issue("empty_asset", "assets", rel.to_string()));
            }
            if bytes.len() > MAX_ASSET_BYTES {
                issues.push(issue(
                    "asset_too_large",
                    "assets",
                    format!(
                        "`{rel}` is {} bytes, over the {MAX_ASSET_BYTES}-byte limit",
                        bytes.len()
                    ),
                ));
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
            let other = self.get_for_compat(&desc.name, &other.version).await?;
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
        // `mut` exists only for the signing block below: without `module-signing` this is a
        // plain binding, and `clippy -D warnings` on a build without that feature refuses it.
        #[cfg_attr(not(feature = "module-signing"), allow(unused_mut))]
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

            let signature = key.sign(&manifest.signing_bytes());
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
            // A module that requires another may name its entities: the same verdict as at
            // `put`, or a version stored with `put_with_assets_with_required` would be
            // refused on every read.
            let required = self.required_from_store(&desc).await;
            let known: Vec<&ModuleDescriptor> = required.iter().collect();
            desc.check_with_required(&self.registry, self.profile(), &known)
                .map_err(invalid)?;
        }
        Ok(desc)
    }

    /// The stored descriptors (active version, unchecked) of the modules `desc` requires,
    /// transitively. A missing or unreadable one is skipped: its entities stay unknown and
    /// the check refuses what cites them.
    async fn required_from_store(&self, desc: &ModuleDescriptor) -> Vec<ModuleDescriptor> {
        let mut seen: BTreeSet<String> = BTreeSet::new();
        seen.insert(desc.name.clone());
        let mut queue: Vec<String> = desc.requires.iter().map(|r| r.name.clone()).collect();
        let mut out = Vec::new();
        while let Some(name) = queue.pop() {
            if !seen.insert(name.clone()) {
                continue;
            }
            let Ok(Some(version)) = self.active_version(&name).await else {
                continue;
            };
            let Ok(dep) = Box::pin(self.get_checked(&name, &version, false)).await else {
                continue;
            };
            queue.extend(dep.requires.iter().map(|r| r.name.clone()));
            out.push(dep);
        }
        out
    }

    /// La même lecture que [`Self::get_checked`], mais **sans authentifier** le manifeste.
    ///
    /// Sert à la vérification de compatibilité d'un `put` : elle compare les étiquettes de
    /// versions déjà rangées, elle ne sert pas leur code. Exiger la signature là faisait
    /// échouer TOUT `put` sur un magasin qui garde une version héritée non signée — mesuré
    /// le 08/11/2026 dans l'Atelier : « atelier 0.7.0 NON rangé — échec :
    /// this.module.unsigned », alors que l'écriture était signée par la clé de l'appareil.
    /// `get` reste authentifié : rien de non signé n'est jamais servi.
    async fn get_for_compat(
        &self,
        name: &str,
        version: &str,
    ) -> Result<ModuleDescriptor, ContractError> {
        let dir = self.version_dir(name, version)?;
        let Some((_, m)) = self.manifest_unverified(name, version).await? else {
            return Err(ContractError::from(VfsError::NotFound(dir.clone())));
        };
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

    /// Export one stored version as a signed, portable [`ModulePackage`]: the
    /// competency a host can share without sharing data.
    ///
    /// The package carries exactly `descriptor.json` and `assets/**`, authenticated
    /// by a manifest signed with this store's signing key (the author). `i18n/**`
    /// and everything else is left out: a competency is what a module *does*, not
    /// its localized texts — and user data never lived in `/modules` to begin with.
    ///
    /// Deterministic: the same stored content exports to the same bytes. The order
    /// of the maps is a `BTreeMap`, the serialization is compact JSON, the `created`
    /// timestamp is the stored commit time (not `now()`), and the signature is
    /// RFC 8032 Ed25519 (no randomness). Two exports differ only if the content does.
    #[cfg(feature = "module-signing")]
    pub async fn export_module(&self, name: &str, version: &str) -> Result<Vec<u8>, ContractError> {
        let dir = self.version_dir(name, version)?;
        let Some(m) = self.manifest(name, version).await? else {
            return Err(VfsError::NotFound(dir).into());
        };
        let mut files: BTreeMap<String, String> = BTreeMap::new();
        let mut etags: BTreeMap<String, String> = BTreeMap::new();
        for rel in m.files.keys() {
            if !package_path_allowed(rel) {
                continue; // i18n stays local; the competency is descriptor + assets.
            }
            let bytes = self
                .read_verified(&dir, &m, rel)
                .await?
                .ok_or_else(|| integrity(&dir, "manifest lists a missing file"))?;
            use base64::Engine as _;
            files.insert(
                rel.clone(),
                base64::engine::general_purpose::STANDARD.encode(&bytes),
            );
            etags.insert(rel.clone(), super::etag_of(&bytes));
        }
        if !files.contains_key(DESCRIPTOR_FILE) {
            return Err(integrity(
                &dir,
                "descriptor.json is missing from the stored version",
            ));
        }
        let Some((key_id, key)) = self.signing.as_ref() else {
            return Err(ContractError::framework(codes::MODULE_UNSIGNED)
                .with_param("module", format!("{name}@{version}"))
                .with_param("reason", "export requires the author's signing key"));
        };
        // A fresh manifest over exactly the shipped files, signed by the author. The
        // stored `created` is preserved, not replaced by `now()`: it is the version's
        // commit time and the thing that keeps two exports of the same content equal.
        let manifest = signed_manifest(
            Manifest {
                format: MANIFEST_FORMAT,
                name: m.name.clone(),
                version: m.version.clone(),
                files: etags,
                created: m.created,
                signature: None,
            },
            key_id,
            key,
        );
        let pkg = ModulePackage {
            format: PACKAGE_FORMAT,
            manifest,
            files,
        };
        serde_json::to_vec(&pkg).map_err(|e| {
            ContractError::framework(codes::INTERNAL).with_param("reason", e.to_string())
        })
    }

    /// Import a package produced by [`export_module`](Self::export_module) (or any
    /// conforming writer), after authenticating it against the trust anchor.
    ///
    /// The order of the controls is the point: the path allowlist and the signature
    /// run BEFORE anything is written, so a package is either fully trusted or not
    /// imported at all — a byte of user data, an unknown or revoked key, or a file
    /// whose content does not hash to the signed etag all leave the store untouched.
    #[cfg(feature = "module-signing")]
    pub async fn import_module(&self, bytes: &[u8]) -> Result<VfsMeta, ContractError> {
        let pkg: ModulePackage = serde_json::from_slice(bytes).map_err(|e| {
            ContractError::framework(codes::MODULE_FORMAT_UNSUPPORTED)
                .with_param("reason", format!("not a module package: {e}"))
        })?;
        if pkg.format != PACKAGE_FORMAT {
            return Err(ContractError::framework(codes::MODULE_FORMAT_UNSUPPORTED)
                .with_param("format", pkg.format)
                .with_param("reason", "unknown package format"));
        }
        let name = pkg.manifest.name.clone();
        let version = pkg.manifest.version.clone();
        let dir = self.version_dir(&name, &version)?;
        let manifest_path = Self::rel(&dir, MANIFEST_FILE)?;

        // 1. Refuse user data: every shipped path must be descriptor.json or
        //    assets/**. Data (rows, lines) never lives in /modules — verify it here
        //    instead of assuming the separation held upstream.
        for rel in pkg.files.keys() {
            if !package_path_allowed(rel) {
                return Err(ContractError::framework(codes::MODULE_FORMAT_UNSUPPORTED)
                    .with_param("path", rel.clone())
                    .with_param("reason", "file outside descriptor.json and assets/**"));
            }
        }
        // 2. The manifest and the shipped files must describe exactly the same set:
        //    no dangling etag, no unlisted file, no stray path in the manifest.
        for rel in pkg.manifest.files.keys() {
            if !package_path_allowed(rel) {
                return Err(ContractError::framework(codes::MODULE_FORMAT_UNSUPPORTED)
                    .with_param("path", rel.clone())
                    .with_param(
                        "reason",
                        "manifest lists a file outside descriptor.json and assets/**",
                    ));
            }
            if !pkg.files.contains_key(rel) {
                return Err(ContractError::framework(codes::MODULE_FORMAT_UNSUPPORTED)
                    .with_param("path", rel.clone())
                    .with_param("reason", "manifest lists a file the package does not carry"));
            }
        }
        for rel in pkg.files.keys() {
            if !pkg.manifest.files.contains_key(rel) {
                return Err(ContractError::framework(codes::MODULE_FORMAT_UNSUPPORTED)
                    .with_param("path", rel.clone())
                    .with_param(
                        "reason",
                        "package carries a file the manifest does not list",
                    ));
            }
        }
        if !pkg.files.contains_key(DESCRIPTOR_FILE) {
            return Err(ContractError::framework(codes::MODULE_FORMAT_UNSUPPORTED)
                .with_param("path", DESCRIPTOR_FILE)
                .with_param("reason", "a package must carry descriptor.json"));
        }

        // 3. Authenticate the manifest against the anchor. This is the same verdict
        //    as `get`: unknown or revoked key -> `untrusted_key`, bad signature ->
        //    `bad_signature`, unsigned (anchor declared) -> `unsigned`.
        self.verify_manifest(&manifest_path, &pkg.manifest)?;

        // 4. Integrity: every shipped byte must hash to the signed etag.
        use base64::Engine as _;
        let mut written: BTreeMap<String, Vec<u8>> = BTreeMap::new();
        for (rel, b64) in &pkg.files {
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(b64)
                .map_err(|e| {
                    ContractError::framework(codes::MODULE_BAD_SIGNATURE)
                        .with_param("path", rel.clone())
                        .with_param("reason", format!("not base64: {e}"))
                })?;
            if pkg.manifest.files.get(rel) != Some(&super::etag_of(&bytes)) {
                return Err(integrity(
                    &Self::rel(&dir, rel)?,
                    "etag differs from the manifest",
                ));
            }
            written.insert(rel.clone(), bytes);
        }

        // 5. The descriptor must parse and identify the module. The receiver re-runs
        //    the full descriptor check at `get` (with its own registry and profile),
        //    so this is structural identity, not a policy verdict.
        let desc_bytes = written.get(DESCRIPTOR_FILE).expect("descriptor present");
        let desc_path = Self::rel(&dir, DESCRIPTOR_FILE)?;
        let desc_text = std::str::from_utf8(desc_bytes)
            .map_err(|_| integrity(&desc_path, "descriptor is not UTF-8"))?;
        let desc = ModuleDescriptor::from_json(desc_text)?;
        if desc.name != name || desc.version != version {
            return Err(ContractError::framework(codes::MODULE_IDENTITY_MISMATCH)
                .with_param("path", dir.as_str())
                .with_param("found", format!("{}@{}", desc.name, desc.version)));
        }
        // Fail-closed code, exactly like `put`: a package carrying executable code
        // into a store with no anchor is refused (nothing can authenticate it).
        if !self.anchor_declared() && written.keys().any(|rel| is_code_asset(rel)) {
            return Err(ContractError::framework(codes::MODULE_UNSIGNED)
                .with_param("module", format!("{name}@{version}"))
                .with_param("reason", "module with code, no trust anchor declared"));
        }

        // 6. Commit under the write lock: immutable versions, atomic visibility,
        //    manifest last (IfNoneMatch), exactly the discipline of `put`.
        let _g = self.write_lock.lock().await;
        if let Some(existing) = self.manifest(&name, &version).await? {
            if existing.files == pkg.manifest.files {
                return self
                    .vfs
                    .stat(&manifest_path)
                    .await?
                    .ok_or_else(|| integrity(&manifest_path, "manifest vanished"));
            }
            return Err(immutable(&dir));
        }
        for f in self.files_below(&dir).await? {
            match self.vfs.delete(&f, Precondition::None).await {
                Ok(()) | Err(VfsError::NotFound(_)) => {}
                Err(e) => return Err(e.into()),
            }
        }
        for (rel, content) in &written {
            self.vfs
                .write(&Self::rel(&dir, rel)?, content, Precondition::None)
                .await?;
        }
        match self
            .vfs
            .write(
                &manifest_path,
                &json(&pkg.manifest)?,
                Precondition::IfNoneMatch,
            )
            .await
        {
            Ok(meta) => Ok(meta),
            Err(VfsError::PreconditionFailed { .. }) => Err(immutable(&dir)),
            Err(e) => Err(e.into()),
        }
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

    async fn code(&self, name: &str, version: &str) -> Result<Option<Vec<u8>>, ContractError> {
        self.asset(name, version, this_vfs::layout::PROGRAM_FILE)
            .await
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
    async fn module_with_code_is_refused_without_a_trust_anchor() {
        let vfs = Arc::new(InMemoryVfs::new());
        // No `with_trust_anchor`: a store with no anchor refuses code, by default.
        let store = VfsModuleStore::new(vfs.clone());
        let m = v1("1.0.0");
        // `program.wasm` and any other `.wasm` asset both carry executable code.
        for asset in ["program.wasm", "lib.wasm"] {
            let err = store
                .put_with_assets(&m, &[bundle("fr", "Facture")], &[(asset, b"\0asm")])
                .await
                .unwrap_err();
            assert_eq!(err.code.0, codes::MODULE_UNSIGNED, "asset {asset}");
            assert!(
                store.get("billing", Some("1.0.0")).await.is_err(),
                "rien n'a été écrit pour {asset}"
            );
            assert_eq!(all_files(vfs.as_ref()).await, 0, "no file for {asset}");
        }
    }

    #[tokio::test]
    async fn declarative_module_is_accepted_without_a_trust_anchor() {
        let (_vfs, store) = mem();
        // Descriptor + i18n only.
        store
            .put(&v1("1.0.0"), &[bundle("fr", "Facture")])
            .await
            .unwrap();
        // A non-executable asset is not code and stays accepted.
        store
            .put_with_assets(&v1("1.1.0"), &[], &[("schema.json", b"{}")])
            .await
            .unwrap();
        assert_eq!(store.get("billing", None).await.unwrap().version, "1.1.0");
        assert_eq!(
            store
                .asset("billing", "1.1.0", "schema.json")
                .await
                .unwrap()
                .unwrap(),
            b"{}"
        );
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
        let asset = b"\0asm\x01\0\0\0";
        store
            .put_with_assets(
                &v1("1.0.0"),
                &[bundle("fr", "Facture")],
                &[("front.bin", asset)],
            )
            .await
            .unwrap();
        assert_eq!(
            store
                .asset("billing", "1.0.0", "front.bin")
                .await
                .unwrap()
                .unwrap(),
            asset
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
                &[("Front.txt", b"a"), ("front.txt", b"b")],
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
    async fn module_store_put_rejects_empty_or_oversized_assets() {
        let (vfs, store) = mem();
        // An empty asset is refused: an asset is content, and a content-less
        // artifact is a mistake (a dropped write, a truncated upload).
        let err = store
            .put_with_assets(&v1("1.0.0"), &[], &[("empty.bin", b"")])
            .await
            .unwrap_err();
        assert!(
            format!("{:?}", err.params).contains("empty_asset"),
            "{err:?}"
        );
        // An asset over `MAX_ASSET_BYTES` is refused before anything is written.
        let oversized = vec![0u8; MAX_ASSET_BYTES + 1];
        let err = store
            .put_with_assets(&v1("1.0.0"), &[], &[("big.bin", oversized.as_slice())])
            .await
            .unwrap_err();
        assert!(
            format!("{:?}", err.params).contains("asset_too_large"),
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
            .put_with_assets(&v1("1.0.0"), &[bundle("fr", "Facture")], &[("x.bin", b"x")])
            .await
            .unwrap();
        // Edited behind the store's back, on disk.
        std::fs::write(
            dir.path().join("modules/billing/1.0.0/assets/x.bin"),
            b"evil",
        )
        .unwrap();
        let err = store.asset("billing", "1.0.0", "x.bin").await.unwrap_err();
        assert_eq!(err.code.0, codes::INTERNAL);
        std::fs::remove_file(dir.path().join("modules/billing/1.0.0/descriptor.json")).unwrap();
        let err = store.get("billing", None).await.unwrap_err();
        assert_eq!(err.code.0, codes::INTERNAL);
        assert!(format!("{:?}", err.params).contains("file missing"));
    }

    // ---------- entities of the required modules ----------

    fn owner(name: &str, requires: &[&str], entity: &str) -> ModuleDescriptor {
        let mut m = ModuleDescriptor::new(name, "1.0.0");
        m.requires = requires.iter().map(|n| req(n)).collect();
        let mut e = EntityDesc::new(entity);
        e.fields = vec![FieldDesc::new("titre", FieldType::String).tag(16)];
        for op in Operation::CRUD {
            e.permissions.ops.insert(op, PolicyExpr::Authenticated);
        }
        m.entities.push(e);
        m
    }

    fn req(name: &str) -> crate::descriptor::ModuleReq {
        crate::descriptor::ModuleReq {
            name: name.into(),
            version: "*".into(),
            optional: false,
        }
    }

    fn lister(name: &str, requires: &[&str], entity: &str) -> ModuleDescriptor {
        let mut m = ModuleDescriptor::new(name, "1.0.0");
        m.requires = requires.iter().map(|n| req(n)).collect();
        m.views.push(
            serde_json::from_value(serde_json::json!({
                "name": "liste",
                "surface": "main",
                "root": {"type": "leaf", "brick": {"type": "list", "entity": entity, "label": "titre"}}
            }))
            .unwrap(),
        );
        m
    }

    #[tokio::test]
    async fn put_with_assets_without_known_refuses_a_required_entity_as_before() {
        let (_vfs, store) = mem();
        let menu = lister("menu", &["socle"], "conversation");
        let err = store.put_with_assets(&menu, &[], &[]).await.unwrap_err();
        assert_eq!(err.code.0, codes::DESCRIPTOR_INVALID);
        assert!(
            err.to_string().contains("view_entity_unknown")
                || format!("{err:?}").contains("view_entity_unknown"),
            "{err:?}"
        );
        // Same refusal through the new entry point with nothing known.
        let err = store
            .put_with_assets_with_required(&menu, &[], &[], &[])
            .await
            .unwrap_err();
        assert!(
            format!("{err:?}").contains("view_entity_unknown"),
            "{err:?}"
        );
        assert!(store.get("menu", None).await.is_err(), "nothing written");
    }

    #[tokio::test]
    async fn put_with_required_stores_a_module_that_cites_a_required_entity_and_get_reads_it() {
        let (_vfs, store) = mem();
        let socle = owner("socle", &[], "conversation");
        store.put(&socle, &[]).await.unwrap();
        let menu = lister("menu", &["socle"], "conversation");
        store
            .put_with_assets_with_required(&menu, &[], &[], &[&socle])
            .await
            .unwrap();
        // `get` re-checks: it resolves the required module from the store itself.
        assert_eq!(store.get("menu", None).await.unwrap().name, "menu");
    }

    #[tokio::test]
    async fn put_with_required_follows_requires_transitively() {
        let (_vfs, store) = mem();
        let socle = owner("socle", &[], "conversation");
        let milieu = lister("milieu", &["socle"], "conversation");
        store.put(&socle, &[]).await.unwrap();
        store
            .put_with_assets_with_required(&milieu, &[], &[], &[&socle])
            .await
            .unwrap();
        let menu = lister("menu", &["milieu"], "conversation");
        store
            .put_with_assets_with_required(&menu, &[], &[], &[&socle, &milieu])
            .await
            .unwrap();
        assert_eq!(store.get("menu", None).await.unwrap().name, "menu");
    }

    #[tokio::test]
    async fn put_with_required_still_refuses_the_entity_of_a_module_not_required() {
        let (_vfs, store) = mem();
        let socle = owner("socle", &[], "conversation");
        store.put(&socle, &[]).await.unwrap();
        // `menu` does not require `socle`, even though `socle` is known and stored.
        let menu = lister("menu", &[], "conversation");
        let err = store
            .put_with_assets_with_required(&menu, &[], &[], &[&socle])
            .await
            .unwrap_err();
        assert_eq!(err.code.0, codes::DESCRIPTOR_INVALID);
        assert!(
            format!("{err:?}").contains("view_entity_unknown"),
            "{err:?}"
        );
        assert!(store.get("menu", None).await.is_err(), "nothing written");
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

    /// The canonical bytes come from `this-vfs` (`Manifest::signing_bytes`), not from a
    /// bespoke canonicalization in the store. A signature produced by a third-party
    /// signer using only `this-vfs` must therefore verify in the store.
    #[tokio::test]
    async fn a_signature_produced_by_this_vfs_verifies_in_the_store() {
        use crate::storage::vfs::etag_of;
        use base64::Engine as _;
        use ed25519_dalek::Signer as _;

        let key = signing_key();
        let vfs = Arc::new(InMemoryVfs::new());
        let dir = VfsPath::parse("/modules/billing/0.0.1").unwrap();

        // The descriptor the manifest will point at, normalized as `put` stores it.
        let mut desc = descriptor();
        desc.normalize();
        let desc_bytes = serde_json::to_vec_pretty(&desc).unwrap();
        vfs.write(
            &VfsModuleStore::rel(&dir, DESCRIPTOR_FILE).unwrap(),
            &desc_bytes,
            Precondition::None,
        )
        .await
        .unwrap();

        // Sign the manifest's canonical bytes using `this-vfs` alone — no store helper.
        let mut files = BTreeMap::new();
        files.insert(DESCRIPTOR_FILE.to_string(), etag_of(&desc_bytes));
        let mut manifest = Manifest {
            format: MANIFEST_FORMAT,
            name: "billing".into(),
            version: "0.0.1".into(),
            files,
            created: Utc::now(),
            signature: None,
        };
        let signature = key.sign(&manifest.signing_bytes());
        manifest.signature = Some(SignatureBlock {
            key_id: "release-2026".into(),
            signature: base64::engine::general_purpose::STANDARD.encode(signature.to_bytes()),
        });
        vfs.write(
            &VfsModuleStore::rel(&dir, MANIFEST_FILE).unwrap(),
            &serde_json::to_vec_pretty(&manifest).unwrap(),
            Precondition::None,
        )
        .await
        .unwrap();

        let reader = VfsModuleStore::new(vfs).with_trust_anchor(anchor_trusting(&key));
        let loaded = reader
            .get("billing", None)
            .await
            .expect("signature produced by this-vfs must verify in the store");
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

    /// LE PIÈGE DE MIGRATION, mesuré le 08/11/2026 dans l'Atelier : `list()` sert à
    /// savoir ce qui est DÉJÀ rangé avant d'écrire. Si une seule version héritée n'est pas
    /// signée, la liste entière échoue — donc l'hôte ne peut plus rien ranger, et la
    /// nouvelle version d'un module système n'arrive jamais dans le VFS. Le journal de
    /// l'app le disait sans détour : « modules non relus — échec : this.module.unsigned »,
    /// suivi de « MANQUE — atelier installé en 0.7.0 n'a pas ses fichiers ».
    #[tokio::test]
    async fn a_legacy_unsigned_version_stays_visible_and_writable() {
        let vfs = Arc::new(InMemoryVfs::new());
        VfsModuleStore::new(vfs.clone())
            .put(&descriptor(), &[])
            .await
            .expect("put unsigned, avant l'ancre");

        let reader =
            VfsModuleStore::new(vfs.clone()).with_trust_anchor(anchor_trusting(&signing_key()));
        let versions = reader
            .list()
            .await
            .expect("l'inventaire doit survivre à une version héritée non signée");
        assert_eq!(versions.len(), 1, "elle reste VISIBLE : {versions:?}");
        assert_eq!(versions[0].0, "billing");
        // Ce qui NE change pas : la version non signée n'est jamais chargée.
        let err = reader
            .get("billing", None)
            .await
            .expect_err("get refuse toujours une version non signée");
        assert_eq!(err.code, codes::MODULE_UNSIGNED.into());

        // Et la conséquence qui comptait : le module redevient ÉCRIVABLE, signé par la
        // clé de l'appareil, malgré la version héritée à côté — c'est exactement ce que
        // fait l'hôte quand il range une nouvelle version d'un module système.
        let writer = VfsModuleStore::new(vfs).with_signing_key("device", signing_key());
        let mut nouvelle = descriptor();
        nouvelle.version = "0.0.2".into();
        writer
            .put(&nouvelle, &[])
            .await
            .expect("une nouvelle version signée doit pouvoir être rangée");
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

    /// A store signed by an arbitrary key (here, a peer), under a chosen key id.
    async fn store_signed_by(key_id: &str, key: SigningKey) -> (Arc<InMemoryVfs>, VfsModuleStore) {
        let vfs = Arc::new(InMemoryVfs::new());
        let writer = VfsModuleStore::new(vfs.clone()).with_signing_key(key_id, key);
        writer
            .put(&descriptor(), &[])
            .await
            .expect("put signed module");
        (vfs, writer)
    }

    /// The mission's core: a module signed by a paired peer must be admitted once
    /// that peer's key is in the anchor, `active`.
    #[tokio::test]
    async fn an_active_peer_key_is_admitted() {
        let peer = SigningKey::from_bytes(&[42u8; 32]);
        let peer_vk = peer.verifying_key();
        let (vfs, _) = store_signed_by("peer-42", peer).await;

        let mut anchor = TrustAnchor::new();
        anchor.insert_peer("peer-42", peer_vk);
        let entry = anchor.get("peer-42").expect("peer present");
        assert_eq!(entry.role, AnchorKeyRole::Shared);
        assert_eq!(entry.state, AnchorKeyState::Active);

        let reader = VfsModuleStore::new(vfs).with_trust_anchor(anchor);
        let loaded = reader
            .get("billing", None)
            .await
            .expect("active peer admitted");
        assert_eq!(loaded.name, "billing");
    }

    /// A revoked peer key stays known but no longer admits anything: the stable
    /// `untrusted_key` verdict, distinguished by `state = "revoked"`.
    #[tokio::test]
    async fn a_revoked_peer_key_is_refused_as_untrusted() {
        let peer = SigningKey::from_bytes(&[42u8; 32]);
        let peer_vk = peer.verifying_key();
        let (vfs, _) = store_signed_by("peer-42", peer).await;

        let mut anchor = TrustAnchor::new();
        anchor.insert_peer("peer-42", peer_vk);
        assert!(anchor.revoke("peer-42"));

        let reader = VfsModuleStore::new(vfs).with_trust_anchor(anchor);
        let err = reader
            .get("billing", None)
            .await
            .expect_err("revoked peer refused");
        assert_eq!(err.code, codes::MODULE_UNTRUSTED_KEY.into());
        assert_eq!(
            err.params.get("state").and_then(|v| v.as_str()),
            Some("revoked"),
            "revocation is distinguishable from an unknown key"
        );
    }

    /// An unknown key id — even one a peer actually signed with — is refused with
    /// the same stable verdict, marked `state = "unknown"`.
    #[tokio::test]
    async fn a_peer_signed_by_an_unknown_key_is_refused() {
        let peer = SigningKey::from_bytes(&[42u8; 32]);
        let (vfs, _) = store_signed_by("peer-42", peer).await;

        let mut anchor = TrustAnchor::new();
        anchor.insert_peer(
            "peer-other",
            SigningKey::from_bytes(&[43u8; 32]).verifying_key(),
        );
        let reader = VfsModuleStore::new(vfs).with_trust_anchor(anchor);

        let err = reader
            .get("billing", None)
            .await
            .expect_err("unknown peer refused");
        assert_eq!(err.code, codes::MODULE_UNTRUSTED_KEY.into());
        assert_eq!(
            err.params.get("state").and_then(|v| v.as_str()),
            Some("unknown")
        );
    }

    /// Revocation is a live mutation of the anchor, not a cache: the store that
    /// already admitted a peer refuses the same peer once `revoke_key` runs.
    #[tokio::test]
    async fn revocation_applies_without_restart_or_rebuild() {
        let peer = SigningKey::from_bytes(&[42u8; 32]);
        let peer_vk = peer.verifying_key();
        let (vfs, _) = store_signed_by("peer-42", peer).await;

        let mut anchor = TrustAnchor::new();
        anchor.insert_peer("peer-42", peer_vk);
        let mut reader = VfsModuleStore::new(vfs).with_trust_anchor(anchor);

        assert_eq!(
            reader.get("billing", None).await.expect("active peer").name,
            "billing"
        );

        assert!(reader.revoke_key("peer-42"));
        let err = reader
            .get("billing", None)
            .await
            .expect_err("revoked, same store, no restart");
        assert_eq!(err.code, codes::MODULE_UNTRUSTED_KEY.into());
    }

    /// The anchor's serializable list (state, role, added_at) is the shape a host
    /// persists and shares: `insert_trusted` → `list` preserves every field.
    #[test]
    fn anchor_key_list_round_trips_state_role_and_date() {
        let peer = SigningKey::from_bytes(&[42u8; 32]);
        let added = Utc::now();

        let mut anchor = TrustAnchor::new();
        anchor.insert_trusted(
            "peer-42",
            peer.verifying_key(),
            AnchorKeyState::Revoked,
            AnchorKeyRole::Shared,
            added,
        );

        let records = anchor.list();
        assert_eq!(records.len(), 1);
        let rec = &records[0];
        assert_eq!(rec.key_id, "peer-42");
        assert_eq!(rec.state, AnchorKeyState::Revoked);
        assert_eq!(rec.role, AnchorKeyRole::Shared);
        assert_eq!(rec.added_at, added);
        assert_eq!(rec.public_key.len(), 64, "32 octets en hex");
    }

    // ---------- le paquet portable : export, import, et ce qu'il refuse ----------

    /// (a) Ce que l'un exporte, un hôte vierge l'importe, et le module installé
    /// vérifie sous la clé de l'auteur (devenue clé de pair). L'export est
    /// déterministe : le même contenu deux fois → les mêmes octets.
    #[tokio::test]
    async fn export_then_import_installs_a_module_that_verifies() {
        let author = signing_key();
        let src_vfs = Arc::new(InMemoryVfs::new());
        // Un module à CODE exige une ancre chez celui qui l'écrit (fail-closed) ; la
        // clé de l'auteur y est la sienne. Chez le lecteur, elle deviendra un pair.
        let mut writer_anchor = TrustAnchor::new();
        writer_anchor.insert("author", author.verifying_key());
        let writer = VfsModuleStore::new(src_vfs)
            .with_signing_key("author", author.clone())
            .with_trust_anchor(writer_anchor);
        writer
            .put_with_assets(&descriptor(), &[], &[("program.wasm", b"\0asm\x01")])
            .await
            .expect("put competency with code");

        let pkg = writer
            .export_module("billing", "0.0.1")
            .await
            .expect("export");
        let pkg_again = writer
            .export_module("billing", "0.0.1")
            .await
            .expect("export again");
        assert_eq!(pkg, pkg_again, "l'export est déterministe");

        let dst_vfs = Arc::new(InMemoryVfs::new());
        let mut anchor = TrustAnchor::new();
        anchor.insert_peer("author", author.verifying_key());
        let reader = VfsModuleStore::new(dst_vfs).with_trust_anchor(anchor);
        reader
            .import_module(&pkg)
            .await
            .expect("import signed by an active peer");

        let loaded = reader.get("billing", None).await.expect("verified on get");
        assert_eq!(loaded.name, "billing");
        assert_eq!(
            reader
                .code("billing", "0.0.1")
                .await
                .expect("code asset")
                .expect("code present"),
            b"\0asm\x01"
        );
    }

    /// (b) Un paquet signé par une clé que l'ancre ne connaît pas est refusé,
    /// sans rien écrire, avec le même verdict que la vérification de manifeste.
    #[tokio::test]
    async fn import_signed_by_an_unknown_key_is_refused() {
        let peer = SigningKey::from_bytes(&[11u8; 32]);
        let src_vfs = Arc::new(InMemoryVfs::new());
        let writer = VfsModuleStore::new(src_vfs).with_signing_key("peer-11", peer);
        writer.put(&descriptor(), &[]).await.expect("put");
        let pkg = writer
            .export_module("billing", "0.0.1")
            .await
            .expect("export");

        let dst_vfs = Arc::new(InMemoryVfs::new());
        let mut anchor = TrustAnchor::new();
        anchor.insert_peer(
            "peer-other",
            SigningKey::from_bytes(&[12u8; 32]).verifying_key(),
        );
        let reader = VfsModuleStore::new(dst_vfs).with_trust_anchor(anchor);
        let err = reader
            .import_module(&pkg)
            .await
            .expect_err("unknown signer must be refused");
        assert_eq!(err.code, codes::MODULE_UNTRUSTED_KEY.into());
        assert_eq!(
            err.params.get("state").and_then(|v| v.as_str()),
            Some("unknown")
        );
        assert!(reader.list().await.unwrap().is_empty(), "nothing written");
    }

    /// (c) Un paquet signé par une clé révoquée est refusé, distingué d'une clé
    /// inconnue par `state = "revoked"`, et rien n'est écrit.
    #[tokio::test]
    async fn import_signed_by_a_revoked_key_is_refused() {
        let peer = SigningKey::from_bytes(&[13u8; 32]);
        let src_vfs = Arc::new(InMemoryVfs::new());
        let writer = VfsModuleStore::new(src_vfs).with_signing_key("peer-13", peer.clone());
        writer.put(&descriptor(), &[]).await.expect("put");
        let pkg = writer
            .export_module("billing", "0.0.1")
            .await
            .expect("export");

        let dst_vfs = Arc::new(InMemoryVfs::new());
        let mut anchor = TrustAnchor::new();
        anchor.insert_peer("peer-13", peer.verifying_key());
        assert!(anchor.revoke("peer-13"));
        let reader = VfsModuleStore::new(dst_vfs).with_trust_anchor(anchor);
        let err = reader
            .import_module(&pkg)
            .await
            .expect_err("revoked signer must be refused");
        assert_eq!(err.code, codes::MODULE_UNTRUSTED_KEY.into());
        assert_eq!(
            err.params.get("state").and_then(|v| v.as_str()),
            Some("revoked")
        );
        assert!(reader.list().await.unwrap().is_empty(), "nothing written");
    }

    /// (d) Même un paquet *correctement signé* qui embarque une donnée utilisateur
    /// est refusé, en nommant le chemin fautif — la signature ne blanchit pas la
    /// donnée, seule la liste blanche des chemins compte.
    #[tokio::test]
    async fn a_package_containing_user_data_is_refused_naming_the_path() {
        use base64::Engine as _;

        let author = signing_key();
        let src_vfs = Arc::new(InMemoryVfs::new());
        let writer = VfsModuleStore::new(src_vfs).with_signing_key("author", author.clone());
        writer.put(&descriptor(), &[]).await.expect("put");
        let pkg_bytes = writer
            .export_module("billing", "0.0.1")
            .await
            .expect("export");

        // Injecte une ligne utilisateur dans un vrai paquet et re-signe : la signature
        // est valide, seule la liste blanche des chemins peut refuser.
        let mut pkg: ModulePackage = serde_json::from_slice(&pkg_bytes).unwrap();
        let leak = br#"[{"user":"alice","note":"secret"}]"#;
        pkg.files.insert(
            "rows/leaks.json".to_string(),
            base64::engine::general_purpose::STANDARD.encode(leak),
        );
        pkg.manifest.files.insert(
            "rows/leaks.json".to_string(),
            crate::storage::vfs::etag_of(leak),
        );
        pkg.manifest = signed_manifest(pkg.manifest, "author", &author);
        let pkg_bytes = serde_json::to_vec(&pkg).unwrap();

        let dst_vfs = Arc::new(InMemoryVfs::new());
        let mut anchor = TrustAnchor::new();
        anchor.insert_peer("author", author.verifying_key());
        let reader = VfsModuleStore::new(dst_vfs).with_trust_anchor(anchor);
        let err = reader
            .import_module(&pkg_bytes)
            .await
            .expect_err("user data must be refused");
        assert_eq!(err.code, codes::MODULE_FORMAT_UNSUPPORTED.into());
        assert_eq!(
            err.params.get("path").and_then(|v| v.as_str()),
            Some("rows/leaks.json"),
            "le chemin fautif est nommé : {err:?}"
        );
        assert!(reader.list().await.unwrap().is_empty(), "nothing written");
    }
}
