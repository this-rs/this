//! Stockage : un `DataService` du framework par entité — mémoire pour les tests, LMDB
//! pour le service. Les recherches passent par `list()` + filtre : la bêta compte des
//! dizaines de comptes, pas des millions (cf. README, « Fragile »).

use std::path::Path;
use std::sync::Arc;

use anyhow::Result;
use this::core::service::DataService;
use this::storage::{InMemoryDataService, LmdbDataService};

use crate::config::{ForgeConfig, StoreKind};
use crate::entities::{Account, Build, Invitation, Session};

/// Les quatre services de données de la forge.
#[derive(Clone)]
pub struct Store {
    pub accounts: Arc<dyn DataService<Account>>,
    pub invitations: Arc<dyn DataService<Invitation>>,
    pub sessions: Arc<dyn DataService<Session>>,
    pub builds: Arc<dyn DataService<Build>>,
}

impl Store {
    pub fn memory() -> Self {
        Self {
            accounts: Arc::new(InMemoryDataService::<Account>::new()),
            invitations: Arc::new(InMemoryDataService::<Invitation>::new()),
            sessions: Arc::new(InMemoryDataService::<Session>::new()),
            builds: Arc::new(InMemoryDataService::<Build>::new()),
        }
    }

    pub fn lmdb(dir: &Path) -> Result<Self> {
        let open = |n: &str| dir.join("db").join(n);
        for n in ["account", "invitation", "session", "build"] {
            std::fs::create_dir_all(open(n))?;
        }
        Ok(Self {
            accounts: Arc::new(LmdbDataService::<Account>::open(open("account"))?),
            invitations: Arc::new(LmdbDataService::<Invitation>::open(open("invitation"))?),
            sessions: Arc::new(LmdbDataService::<Session>::open(open("session"))?),
            builds: Arc::new(LmdbDataService::<Build>::open(open("build"))?),
        })
    }

    pub fn from_config(cfg: &ForgeConfig) -> Result<Self> {
        match cfg.store {
            StoreKind::Memory => Ok(Self::memory()),
            StoreKind::Lmdb => Self::lmdb(&cfg.data_dir),
        }
    }
}
