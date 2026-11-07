//! Where the configuration lives. A trait: a server keeps a file, a phone keeps the
//! webview's storage, a test keeps memory.

use std::fmt;
use std::path::PathBuf;
use std::sync::{Mutex, PoisonError};

use serde::{Deserialize, Serialize};

use crate::config::{ProviderConfig, ProviderId};

/// Everything the user configured: the providers and which one is active.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Snapshot {
    #[serde(default)]
    pub active: Option<ProviderId>,
    #[serde(default)]
    pub providers: Vec<ProviderConfig>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StoreError {
    /// The medium failed (disk, quota, permissions).
    Io(String),
    /// The stored data is not a valid configuration. Refused rather than repaired.
    Format(String),
}

impl fmt::Display for StoreError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(reason) => write!(formatter, "stockage des providers : {reason}"),
            Self::Format(reason) => write!(
                formatter,
                "configuration des providers illisible : {reason}"
            ),
        }
    }
}

impl std::error::Error for StoreError {}

/// Reads and writes the whole [`Snapshot`]. Small, so it is always written whole.
pub trait ProviderStore: Send + Sync {
    fn load(&self) -> Result<Snapshot, StoreError>;
    fn save(&self, snapshot: &Snapshot) -> Result<(), StoreError>;
}

/// A store in memory: tests, and hosts that persist elsewhere.
#[derive(Debug, Default)]
pub struct MemoryStore {
    inner: Mutex<Snapshot>,
}

impl MemoryStore {
    pub fn new(snapshot: Snapshot) -> Self {
        Self {
            inner: Mutex::new(snapshot),
        }
    }
}

impl ProviderStore for MemoryStore {
    fn load(&self) -> Result<Snapshot, StoreError> {
        Ok(self
            .inner
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone())
    }

    fn save(&self, snapshot: &Snapshot) -> Result<(), StoreError> {
        *self.inner.lock().unwrap_or_else(PoisonError::into_inner) = snapshot.clone();
        Ok(())
    }
}

/// A JSON file. A missing file is an empty configuration; a corrupt one is an error,
/// and it is left untouched. Writes go to a sibling file, then replace the original,
/// so a crash mid-write cannot leave half a configuration behind.
#[derive(Debug, Clone)]
pub struct JsonFileStore {
    path: PathBuf,
}

impl JsonFileStore {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    pub fn path(&self) -> &PathBuf {
        &self.path
    }
}

impl ProviderStore for JsonFileStore {
    fn load(&self) -> Result<Snapshot, StoreError> {
        let text = match std::fs::read_to_string(&self.path) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Snapshot::default());
            }
            Err(error) => return Err(StoreError::Io(error.to_string())),
        };
        let snapshot: Snapshot =
            serde_json::from_str(&text).map_err(|error| StoreError::Format(error.to_string()))?;
        for provider in &snapshot.providers {
            provider
                .validate()
                .map_err(|error| StoreError::Format(format!("`{}` : {error}", provider.id)))?;
        }
        Ok(snapshot)
    }

    fn save(&self, snapshot: &Snapshot) -> Result<(), StoreError> {
        let text = serde_json::to_string_pretty(snapshot)
            .map_err(|error| StoreError::Format(error.to_string()))?;
        if let Some(parent) = self.path.parent().filter(|p| !p.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent).map_err(|error| StoreError::Io(error.to_string()))?;
        }
        let temporary = self.path.with_extension("json.tmp");
        std::fs::write(&temporary, text).map_err(|error| StoreError::Io(error.to_string()))?;
        std::fs::rename(&temporary, &self.path).map_err(|error| StoreError::Io(error.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use this_llm::{CredentialRef, Preset};

    fn sample() -> Snapshot {
        let vllm = ProviderConfig::new(
            ProviderId::new("vllm").unwrap(),
            "vLLM",
            Preset::Vllm,
            CredentialRef::None,
        );
        Snapshot {
            active: Some(vllm.id.clone()),
            providers: vec![vllm],
        }
    }

    #[test]
    fn the_memory_store_keeps_what_it_was_given() {
        let store = MemoryStore::default();
        store.save(&sample()).unwrap();
        assert_eq!(store.load().unwrap(), sample());
    }

    #[test]
    fn the_file_store_round_trips_and_a_missing_file_is_empty() {
        let directory = std::env::temp_dir().join(format!(
            "this-llm-providers-{}-{}",
            std::process::id(),
            "roundtrip"
        ));
        let _ = std::fs::remove_dir_all(&directory);
        let store = JsonFileStore::new(directory.join("providers.json"));

        assert_eq!(store.load().unwrap(), Snapshot::default());
        store.save(&sample()).unwrap();
        assert_eq!(store.load().unwrap(), sample());

        let _ = std::fs::remove_dir_all(&directory);
    }

    #[test]
    fn a_corrupt_file_is_refused_not_repaired() {
        let directory = std::env::temp_dir().join(format!(
            "this-llm-providers-{}-{}",
            std::process::id(),
            "corrupt"
        ));
        std::fs::create_dir_all(&directory).unwrap();
        let path = directory.join("providers.json");
        std::fs::write(&path, r#"{"providers":[{"id":"Bad Id"}]}"#).unwrap();

        let store = JsonFileStore::new(&path);
        assert!(matches!(store.load(), Err(StoreError::Format(_))));
        assert!(path.exists(), "le fichier n'est pas touché");

        let _ = std::fs::remove_dir_all(&directory);
    }
}
