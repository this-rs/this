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
//! - **No execution.** Running a module needs an engine (`wasmi` on a host,
//!   `WebAssembly` in a page); that belongs behind the `Executor` trait, so this
//!   crate stays compilable for both.
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

pub mod executor;

use chrono::Utc;
use std::collections::BTreeMap;
use this_module_lang::assemble::compile_to_wasm;
use this_vfs::layout::{MANIFEST_FILE, MANIFEST_FORMAT, Manifest};
use this_vfs::{Precondition, Vfs, VfsError, VfsPath, etag_of};

/// The binary, inside the version directory.
pub const PROGRAM_FILE: &str = "program.wasm";
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
        let dir = version_dir(name, version)?;

        self.write(&dir.join(PROGRAM_FILE)?, &wasm).await?;
        self.write(&dir.join(SOURCE_FILE)?, source.as_bytes())
            .await?;

        let mut files = BTreeMap::new();
        files.insert(PROGRAM_FILE.to_string(), etag_of(&wasm));
        files.insert(SOURCE_FILE.to_string(), etag_of(source.as_bytes()));

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
        self.write(&dir.join(MANIFEST_FILE)?, &encoded).await?;

        Ok(Artifact {
            name: name.to_string(),
            version: version.to_string(),
            etag: etag_of(&wasm),
            bytes: wasm.len(),
        })
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

    /// The source that produced a stored artifact.
    pub async fn source(&self, name: &str, version: &str) -> Result<String, Error> {
        let dir = version_dir(name, version)?;
        let bytes = self.vfs.read(&dir.join(SOURCE_FILE)?).await?;
        String::from_utf8(bytes).map_err(|e| Error::Manifest(e.to_string()))
    }

    async fn write(&self, path: &VfsPath, bytes: &[u8]) -> Result<(), Error> {
        self.vfs.write(path, bytes, Precondition::None).await?;
        Ok(())
    }
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
