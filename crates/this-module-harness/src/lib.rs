//! Compile a module, store it, read it back — the isolated core of a harness.
//!
//! ## What it composes, and what it refuses to copy
//!
//! | Source | What it brings |
//! |---|---|
//! | `this-module-lang` | a language that is total by construction: no loop, no state, no implicit I/O |
//! | `this-module-lang::assemble` | WAT → wasm, in Rust, no `wabt.js` |
//! | `this-vfs` | write/read with the contract's etag and preconditions |
//! | `this-vfs::layout` | the **same** paths and `manifest.json` the server uses |
//!
//! The last line is the point: an artifact produced here is stored where a server
//! expects it, with a manifest that says what it should contain. Without the
//! manifest the file would exist and nothing would say what it is — no
//! verification, and the format's "root of trust" would mean nothing.
//!
//! ## What it deliberately does not do
//!
//! - **No engine of its own.** Running a module needs an engine (`wasmi` on a host,
//!   `WebAssembly` in a page); that belongs behind the `Executor` trait, so this
//!   crate stays compilable for both. The one call the harness describes itself is
//!   the **view render call** ([`render`]): bounded in fuel and memory, refused
//!   before execution when the module does not hold the contract, and validated
//!   against `this-descriptor` before anything is drawn.
//! - **No DOM, no tokio.** Nothing here knows whether it runs in a page or on a
//!   server; only [`Vfs`] does, and it is injected.
//!
//! ```no_run
//! # async fn demo<V: this_vfs::Vfs>(vfs: &V) -> Result<(), Box<dyn std::error::Error>> {
//! use this_module_harness::Harness;
//!
//! let harness = Harness::new(vfs);
//! harness.compile("shop", "0.3.0", "fn run(a: Int) -> Int { a + 1 }").await?;
//!
//! // A harness keeps the name and the version — not the bytes.
//! let program = harness.load("shop", "0.3.0").await?;
//! # Ok(()) }
//! ```

pub mod call;
pub mod drafter;
pub mod executor;
pub mod module_draft;
pub mod render;
pub mod serving;

use chrono::Utc;
use executor::{Executor, ExecutorError, Outcome};
use render::{Limits, Rendered, ViewRenderer};
use std::collections::BTreeMap;
use this_module_lang::assemble::compile_to_wasm;
use this_module_lang::eval::Value;
use this_vfs::layout::YANKED_DIR;
use this_vfs::layout::{MANIFEST_FILE, MANIFEST_FORMAT, Manifest};
use this_vfs::{Precondition, Vfs, VfsEntry, VfsError, VfsPath, etag_of};

/// The binary, inside the version directory. Defined in `this_vfs::layout`, re-exported here.
pub use this_vfs::layout::PROGRAM_FILE;
/// The source, next to it: what produced the artifact, kept so it can be read back.
pub const SOURCE_FILE: &str = "source.txt";

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("compilation : {0}")]
    Compile(String),
    #[error("assemblage : {0}")]
    Assemble(String),
    #[error("vfs : {0}")]
    Vfs(#[from] VfsError),
    #[error("manifeste illisible : {0}")]
    Manifest(String),
    #[error("intégrité : l'etag de `{path}` ne correspond pas au manifeste")]
    Integrity { path: String },
    #[error("exécution : {0}")]
    Execution(#[from] ExecutorError),
    #[error("le modèle n'a pas répondu : {0}")]
    Model(String),
    #[error("la réponse du modèle ne contient aucun programme : {0}")]
    NoProgram(String),
    #[error("le programme proposé ne compile pas ({message})")]
    /// `program` et non `source` : `thiserror` réserve `source` à la cause d'une erreur.
    NotCompiling { program: String, message: String },
    #[error("le descripteur proposé n'est pas un JSON valide : {0}")]
    InvalidDescriptor(String),
    #[error("le module proposé ne chargerait pas : {issues:?}")]
    Rejected { issues: Vec<String> },

    #[error(
        "un module stocké ne se remplace pas : `{path}` existe déjà (compilez une nouvelle version)"
    )]
    AlreadyStored { path: String },
    #[error("ce n'est pas un module wasm ({bytes} octets) : {message}")]
    NotWasm { bytes: usize, message: String },
    #[error("la requête de rendu n'est pas du JSON : {0}")]
    InvalidRequest(String),
    #[error("le module a rendu un arbre hors contrat : {0}")]
    InvalidViews(String),
    #[error("l'arbre rendu contredit le descripteur du module : {issues:?}")]
    ViewsRejected { issues: Vec<String> },
}

/// What a compilation produced, and where it landed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Artifact {
    pub name: String,
    pub version: String,
    pub etag: String,
    pub bytes: usize,
}

/// Compiles modules into a `Vfs`, and reads them back.
pub struct Harness<'a, V: Vfs> {
    vfs: &'a V,
}

impl<'a, V: Vfs> Harness<'a, V> {
    pub fn new(vfs: &'a V) -> Self {
        Self { vfs }
    }

    /// Compile `source` and store it as `name`/`version`.
    ///
    /// Writes three files under `/modules/{name}/{version}/`: the binary, the
    /// source, and a manifest carrying both etags.
    pub async fn compile(
        &self,
        name: &str,
        version: &str,
        source: &str,
    ) -> Result<Artifact, Error> {
        let wasm = compile_to_wasm(source).map_err(Error::Assemble)?;
        self.write_version(name, version, &wasm, Some(source)).await
    }

    /// Store a binary that was assembled elsewhere — a page that compiled it, another
    /// host that shipped it, or a fixture in a test.
    ///
    /// The bytes are admitted by the same door as [`Harness::compile`]: checked to be a
    /// wasm module, never written over an existing version, and the manifest that
    /// declares them is written **before** them. No source is stored (there is none to
    /// store), so [`Harness::source`] will not answer for such a version — the manifest
    /// declares `program.wasm` alone.
    pub async fn store_program(
        &self,
        name: &str,
        version: &str,
        wasm: &[u8],
    ) -> Result<Artifact, Error> {
        self.write_version(name, version, wasm, None).await
    }

    /// The single door through which code enters the store.
    ///
    /// Fail-closed, in this order:
    ///
    /// 1. the bytes must be a wasm module (fully validated when an engine is available);
    /// 2. an existing version is never replaced — `IfNoneMatch` on every file;
    /// 3. the **manifest comes first**, and the binary last: an interruption leaves at
    ///    worst a record that declares code which is not there (a version a reader
    ///    refuses), never code that no record declares.
    ///
    /// What this does not do, and cannot do here: *authenticate* the record. A version
    /// with code is admitted by a store only when its manifest carries a signature from
    /// the trust anchor (`this_vfs::layout`), and the harness holds no key. So the write
    /// is the trust boundary, and the signature belongs to the caller — `this.module.load`
    /// in the RFC trust model. Anything that writes the layout's paths itself (a fixture,
    /// a page, another host) is outside this door and outside its guarantees.
    async fn write_version(
        &self,
        name: &str,
        version: &str,
        wasm: &[u8],
        source: Option<&str>,
    ) -> Result<Artifact, Error> {
        check_program(wasm)?;
        let dir = version_dir(name, version)?;

        let mut files = BTreeMap::new();
        files.insert(PROGRAM_FILE.to_string(), etag_of(wasm));
        if let Some(source) = source {
            files.insert(SOURCE_FILE.to_string(), etag_of(source.as_bytes()));
        }
        let manifest = Manifest {
            format: MANIFEST_FORMAT,
            name: name.to_string(),
            version: version.to_string(),
            files,
            created: Utc::now(),
            signature: None,
        };
        let encoded =
            serde_json::to_vec_pretty(&manifest).map_err(|e| Error::Manifest(e.to_string()))?;

        // Le manifeste d'abord : une version n'entre dans le store qu'avec son acte.
        self.create(&dir.join(MANIFEST_FILE)?, &encoded).await?;
        if let Some(source) = source {
            self.create(&dir.join(SOURCE_FILE)?, source.as_bytes())
                .await?;
        }
        // Le code ensuite, en dernier.
        self.create(&dir.join(PROGRAM_FILE)?, wasm).await?;

        Ok(Artifact {
            name: name.to_string(),
            version: version.to_string(),
            etag: etag_of(wasm),
            bytes: wasm.len(),
        })
    }

    /// Run the **view render call** of a stored module, and validate what comes back.
    ///
    /// The contract — the three exports, the memory round trip, what is refused before
    /// execution — is in [`render`]. Two bounds hold on the host side of this call:
    ///
    /// - the response is re-checked against [`Limits::response_bytes`] and parsed into
    ///   [`this_descriptor::views::ViewDesc`] (closed vocabulary, `deny_unknown_fields`);
    /// - a tree outside the contract becomes [`Error::InvalidViews`]: it is **refused,
    ///   never drawn**.
    ///
    /// [`Harness::render_against`] adds the semantic pass (declared entities, declared
    /// actions, known fields) using `this-descriptor`'s own checks.
    pub async fn render<E: ViewRenderer>(
        &self,
        executor: &E,
        name: &str,
        version: &str,
        request: &serde_json::Value,
        limits: &Limits,
    ) -> Result<Rendered, Error> {
        let program = self.load(name, version).await?;
        let request =
            serde_json::to_vec(request).map_err(|e| Error::InvalidRequest(e.to_string()))?;
        let call = executor.render(&program, &request, limits).await?;
        let views = render::views_of(&call.response, limits.response_bytes)
            .map_err(|e| Error::InvalidViews(e.to_string()))?;
        Ok(Rendered {
            views,
            fuel_used: call.fuel_used,
            bytes: call.response.len(),
        })
    }

    /// [`Harness::render`], then the semantic pass: the rendered views are confronted
    /// with `descriptor` by `this_descriptor::views::issues` — the same check that
    /// guards a stored module. A tree that names an entity, an action or a field the
    /// module does not declare is refused with the list of what contradicts what.
    pub async fn render_against<E: ViewRenderer>(
        &self,
        executor: &E,
        name: &str,
        version: &str,
        request: &serde_json::Value,
        limits: &Limits,
        descriptor: &this_descriptor::ModuleDescriptor,
    ) -> Result<Rendered, Error> {
        let rendered = self
            .render(executor, name, version, request, limits)
            .await?;
        let issues = render::issues_against(descriptor, &rendered.views);
        if !issues.is_empty() {
            return Err(Error::ViewsRejected { issues });
        }
        Ok(rendered)
    }

    /// Read the stored binary back. The bytes are checked against the manifest's
    /// etag: a store that returns something else is a store you cannot trust.
    pub async fn load(&self, name: &str, version: &str) -> Result<Vec<u8>, Error> {
        let dir = version_dir(name, version)?;
        let encoded = self.vfs.read(&dir.join(MANIFEST_FILE)?).await?;
        let manifest: Manifest =
            serde_json::from_slice(&encoded).map_err(|e| Error::Manifest(e.to_string()))?;

        let wasm = self.vfs.read(&dir.join(PROGRAM_FILE)?).await?;
        match manifest.files.get(PROGRAM_FILE) {
            Some(expected) if expected == &etag_of(&wasm) => Ok(wasm),
            Some(_) => Err(Error::Integrity {
                path: PROGRAM_FILE.to_string(),
            }),
            None => Err(Error::Manifest(format!(
                "le manifeste de {name}/{version} ne déclare pas `{PROGRAM_FILE}`"
            ))),
        }
    }

    /// Load `name`/`version` **from the store** and run it.
    ///
    /// This is the gesture that makes a module testable rather than viewable: the
    /// artifact is read back from the `Vfs` — not from a buffer still in hand —
    /// then handed to whichever engine the caller chose.
    pub async fn run<E: Executor>(
        &self,
        executor: &E,
        name: &str,
        version: &str,
        args: &[Value],
        now: i64,
    ) -> Result<Outcome, Error> {
        let program = self.load(name, version).await?;
        Ok(executor.run(&program, args, now).await?)
    }

    /// Every version stored for `name`, oldest first.
    ///
    /// Sorting is numeric per component, so `0.10.0` comes after `0.9.0` — a plain
    /// string sort would put it before, and a harness that reads "the latest"
    /// would silently run an older module.
    ///
    /// The reserved directory of the layout is skipped: a yanked version is not a
    /// version you can ask for.
    pub async fn versions(&self, name: &str) -> Result<Vec<String>, Error> {
        let dir = VfsPath::parse(&format!("/modules/{name}")).map_err(Error::Vfs)?;
        let prefix = format!("/modules/{name}/");
        let mut versions: Vec<String> = self
            .vfs
            .list(&dir)
            .await?
            .into_iter()
            .filter_map(|entry| match entry {
                VfsEntry::Dir { path } => path
                    .as_str()
                    .strip_prefix(&prefix)
                    .map(|rest| rest.to_string()),
                VfsEntry::File { .. } => None,
            })
            .filter(|rest| !rest.is_empty() && !rest.contains('/') && rest != YANKED_DIR)
            .collect();
        versions.sort_by(|a, b| compare_versions(a, b));
        Ok(versions)
    }

    /// The highest stored version of `name`, if any.
    pub async fn latest(&self, name: &str) -> Result<Option<String>, Error> {
        Ok(self.versions(name).await?.pop())
    }

    /// The source that produced a stored artifact.
    pub async fn source(&self, name: &str, version: &str) -> Result<String, Error> {
        let dir = version_dir(name, version)?;
        let bytes = self.vfs.read(&dir.join(SOURCE_FILE)?).await?;
        String::from_utf8(bytes).map_err(|e| Error::Manifest(e.to_string()))
    }

    /// Writes only where nothing exists: a stored artifact is not replaced silently.
    async fn create(&self, path: &VfsPath, bytes: &[u8]) -> Result<(), Error> {
        match self.vfs.write(path, bytes, Precondition::IfNoneMatch).await {
            Ok(_) => Ok(()),
            Err(VfsError::PreconditionFailed { .. }) => Err(Error::AlreadyStored {
                path: path.as_str().to_string(),
            }),
            Err(other) => Err(Error::Vfs(other)),
        }
    }
}

/// Le contrôle que le harness fait sur tout code qui entre : une en-tête wasm, et — quand
/// un moteur est disponible — la validation complète du module, sans l'exécuter. Un store
/// qui ne retient que des bytes validés n'est pas un store qui exécutera n'importe quoi.
fn check_program(wasm: &[u8]) -> Result<(), Error> {
    let not_wasm = |message: String| Error::NotWasm {
        bytes: wasm.len(),
        message,
    };
    if wasm.len() < 8 {
        return Err(not_wasm("- de 8 octets".to_string()));
    }
    if &wasm[..4] != b"\0asm" {
        return Err(not_wasm(
            "l'en-tête ne commence pas par `\\0asm`".to_string(),
        ));
    }
    if wasm[4..8] != [1, 0, 0, 0] {
        return Err(not_wasm(format!(
            "version de binaire {:?}, seule la 1 est admise",
            &wasm[4..8]
        )));
    }
    #[cfg(feature = "wasmi")]
    {
        let engine = wasmi::Engine::default();
        wasmi::Module::new(&engine, wasm).map_err(|e| not_wasm(e.to_string()))?;
    }
    Ok(())
}

/// Compare deux versions composant par composant, numériquement quand c'est
/// possible. `0.10.0 > 0.9.0`, ce qu'un tri de chaînes ne donne pas.
fn compare_versions(a: &str, b: &str) -> std::cmp::Ordering {
    let num = |s: &str, i: usize| s.split('.').nth(i).and_then(|p| p.parse::<u64>().ok());
    for i in 0..4 {
        match (num(a, i), num(b, i)) {
            (Some(x), Some(y)) if x != y => return x.cmp(&y),
            _ => continue,
        }
    }
    a.cmp(b)
}

fn version_dir(name: &str, version: &str) -> Result<VfsPath, Error> {
    VfsPath::parse(&format!("/modules/{name}/{version}")).map_err(Error::Vfs)
}

#[cfg(test)]
mod tests {
    use super::*;
    use this_vfs::InMemoryVfs;

    const PROGRAM: &str = "fn run(a: Int) -> Int { emit(\"bonjour\"); a + 1 }";

    #[tokio::test]
    async fn a_compiled_module_is_stored_where_the_layout_says() {
        let vfs = InMemoryVfs::default();
        let harness = Harness::new(&vfs);
        let artifact = harness.compile("shop", "0.3.0", PROGRAM).await.unwrap();

        assert!(artifact.bytes > 8, "un module wasm n'est pas vide");
        for file in [PROGRAM_FILE, SOURCE_FILE, MANIFEST_FILE] {
            let path = VfsPath::parse(&format!("/modules/shop/0.3.0/{file}")).unwrap();
            assert!(
                vfs.stat(&path).await.unwrap().is_some(),
                "{file} devrait exister"
            );
        }
    }

    #[tokio::test]
    async fn a_module_is_read_back_from_the_store_not_from_memory() {
        let vfs = InMemoryVfs::default();
        let harness = Harness::new(&vfs);
        let artifact = harness.compile("shop", "0.3.0", PROGRAM).await.unwrap();

        let program = harness.load("shop", "0.3.0").await.unwrap();
        assert_eq!(&program[..4], b"\0asm");
        assert_eq!(etag_of(&program), artifact.etag);

        let source = harness.source("shop", "0.3.0").await.unwrap();
        assert_eq!(
            source, PROGRAM,
            "la source rangée est celle qui a produit l'artefact"
        );
    }

    #[tokio::test]
    async fn a_corrupted_artifact_is_refused() {
        let vfs = InMemoryVfs::default();
        let harness = Harness::new(&vfs);
        harness.compile("shop", "0.3.0", PROGRAM).await.unwrap();

        // Quelqu'un écrit autre chose à la place du binaire, sans le manifeste.
        let program = VfsPath::parse("/modules/shop/0.3.0/program.wasm").unwrap();
        vfs.write(&program, &[0, 1, 2, 3], Precondition::None)
            .await
            .unwrap();

        match harness.load("shop", "0.3.0").await {
            Err(Error::Integrity { .. }) => {}
            other => panic!("un artefact corrompu doit être refusé, reçu {other:?}"),
        }
    }

    #[cfg(feature = "wasmi")]
    #[tokio::test]
    async fn a_stored_module_can_be_asked_things_after_forgetting_it() {
        use crate::executor::WasmiExecutor;
        use this_module_lang::eval::Value;

        let vfs = InMemoryVfs::default();
        let harness = Harness::new(&vfs);
        harness
            .compile(
                "shop",
                "0.3.0",
                "fn run(a: Int) -> Int { emit(\"commande\"); a + 1 }",
            )
            .await
            .unwrap();

        // On ne garde plus l'artefact : ni octets, ni source en mémoire.
        // Seuls le nom et la version subsistent.
        let outcome = harness
            .run(&WasmiExecutor, "shop", "0.3.0", &[Value::Int(41)], 0)
            .await
            .unwrap();

        assert_eq!(outcome.value, "42");
        assert_eq!(outcome.effects, vec!["emit(\"commande\")".to_string()]);
    }

    #[cfg(feature = "wasmi")]
    #[tokio::test]
    async fn an_absent_module_is_reported_rather_than_improvised() {
        use crate::executor::WasmiExecutor;

        let vfs = InMemoryVfs::default();
        let harness = Harness::new(&vfs);
        assert!(
            harness
                .run(&WasmiExecutor, "shop", "9.9.9", &[], 0)
                .await
                .is_err(),
            "un module absent ne doit pas être inventé"
        );
    }

    #[tokio::test]
    async fn versions_are_ordered_numerically_not_alphabetically() {
        let vfs = InMemoryVfs::default();
        let harness = Harness::new(&vfs);
        for version in ["0.9.0", "0.10.0", "0.2.0"] {
            harness.compile("shop", version, PROGRAM).await.unwrap();
        }

        assert_eq!(
            harness.versions("shop").await.unwrap(),
            vec!["0.2.0", "0.9.0", "0.10.0"],
            "0.10.0 vient après 0.9.0 : un tri de chaînes le placerait avant"
        );
        assert_eq!(
            harness.latest("shop").await.unwrap(),
            Some("0.10.0".to_string())
        );
    }

    #[tokio::test]
    async fn a_yanked_directory_is_not_a_version() {
        let vfs = InMemoryVfs::default();
        let harness = Harness::new(&vfs);
        harness.compile("shop", "0.1.0", PROGRAM).await.unwrap();

        // Le dossier réservé de la disposition, créé comme le ferait un retrait.
        let yanked = VfsPath::parse("/modules/shop/_yanked/0.1.0.json").unwrap();
        vfs.write(&yanked, b"{}", Precondition::None).await.unwrap();

        assert_eq!(harness.versions("shop").await.unwrap(), vec!["0.1.0"]);
    }

    #[tokio::test]
    async fn a_stored_version_is_not_replaced_silently() {
        let vfs = InMemoryVfs::default();
        let harness = Harness::new(&vfs);
        harness.compile("shop", "0.3.0", PROGRAM).await.unwrap();

        match harness
            .compile("shop", "0.3.0", "fn run() -> Int { 99 }")
            .await
        {
            Err(Error::AlreadyStored { .. }) => {}
            other => panic!("un module stocké ne se remplace pas, reçu {other:?}"),
        }
        assert_eq!(
            harness.source("shop", "0.3.0").await.unwrap(),
            PROGRAM,
            "le code d'origine est toujours celui qu'on relit"
        );
    }

    #[tokio::test]
    async fn something_that_is_not_wasm_does_not_enter_the_store() {
        let vfs = InMemoryVfs::default();
        let harness = Harness::new(&vfs);
        match harness.store_program("shop", "0.4.0", b"pas du wasm").await {
            Err(Error::NotWasm { .. }) => {}
            other => panic!("un binaire non wasm doit être refusé, reçu {other:?}"),
        }
        let dir = VfsPath::parse("/modules/shop/0.4.0").unwrap();
        assert!(
            vfs.list(&dir).await.unwrap().is_empty(),
            "un refus ne doit rien laisser dans le store"
        );
    }

    #[tokio::test]
    async fn two_versions_are_two_programs_not_one() {
        let vfs = InMemoryVfs::default();
        let harness = Harness::new(&vfs);
        harness
            .compile("shop", "0.1.0", "fn run() -> Int { 1 }")
            .await
            .unwrap();
        harness
            .compile("shop", "0.2.0", "fn run() -> Int { 2 }")
            .await
            .unwrap();

        assert_eq!(
            harness.source("shop", "0.1.0").await.unwrap(),
            "fn run() -> Int { 1 }"
        );
        assert_eq!(
            harness.source("shop", "0.2.0").await.unwrap(),
            "fn run() -> Int { 2 }"
        );
    }

    #[tokio::test]
    async fn a_rejected_program_writes_nothing() {
        let vfs = InMemoryVfs::default();
        let harness = Harness::new(&vfs);
        assert!(
            harness
                .compile("shop", "0.3.1", "fn run() -> Int { 1 + }")
                .await
                .is_err()
        );

        let dir = VfsPath::parse("/modules/shop/0.3.1").unwrap();
        assert!(
            vfs.list(&dir).await.unwrap().is_empty(),
            "un programme refusé ne doit rien laisser dans le store"
        );
    }
}
