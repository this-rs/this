//! The registry: which providers exist, which one is active, and what they have spent.

use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use crate::budget::{Clock, Ledger, Usage};
use crate::config::{ProviderConfig, ProviderId};
use crate::error::ProviderError;
use crate::secret::SecretResolver;
use crate::store::{ProviderStore, Snapshot};

/// The providers of one app. Every mutation is validated, then written through the store
/// before it is reported as done.
pub struct Hub {
    store: Box<dyn ProviderStore>,
    /// Only a client resolves keys: without the `http` feature nothing reads it.
    #[cfg_attr(not(feature = "http"), allow(dead_code))]
    secrets: Arc<dyn SecretResolver>,
    clock: Arc<dyn Clock>,
    ledger: Arc<Mutex<Ledger>>,
    state: Mutex<Snapshot>,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Hub {
    /// Loads the configuration from `store`. A corrupt store fails here, loudly.
    pub fn open(
        store: impl ProviderStore + 'static,
        secrets: impl SecretResolver + 'static,
        clock: impl Clock + 'static,
    ) -> Result<Self, ProviderError> {
        let snapshot = store.load()?;
        Ok(Self {
            store: Box::new(store),
            secrets: Arc::new(secrets),
            clock: Arc::new(clock),
            ledger: Arc::new(Mutex::new(Ledger::default())),
            state: Mutex::new(snapshot),
        })
    }

    /// Every configured provider, enabled or not, in the order they were added.
    pub fn providers(&self) -> Vec<ProviderConfig> {
        lock(&self.state).providers.clone()
    }

    pub fn get(&self, id: &ProviderId) -> Option<ProviderConfig> {
        lock(&self.state)
            .providers
            .iter()
            .find(|provider| &provider.id == id)
            .cloned()
    }

    /// The active provider, if one is set and still enabled.
    pub fn active(&self) -> Option<ProviderConfig> {
        let state = lock(&self.state);
        let id = state.active.as_ref()?;
        state
            .providers
            .iter()
            .find(|provider| &provider.id == id && provider.enabled)
            .cloned()
    }

    /// Adds a provider, or replaces the one with the same identifier.
    pub fn upsert(&self, config: ProviderConfig) -> Result<(), ProviderError> {
        config.validate()?;
        let mut state = lock(&self.state);
        match state
            .providers
            .iter_mut()
            .find(|provider| provider.id == config.id)
        {
            Some(slot) => *slot = config,
            None => state.providers.push(config),
        }
        Self::drop_stale_active(&mut state);
        self.store.save(&state)?;
        Ok(())
    }

    /// Removes a provider. If it was active, nothing is active afterwards.
    pub fn remove(&self, id: &ProviderId) -> Result<(), ProviderError> {
        let mut state = lock(&self.state);
        let before = state.providers.len();
        state.providers.retain(|provider| &provider.id != id);
        if state.providers.len() == before {
            return Err(ProviderError::NotFound(id.to_string()));
        }
        Self::drop_stale_active(&mut state);
        self.store.save(&state)?;
        Ok(())
    }

    /// Chooses the active provider, or none. Only an existing, enabled provider can be chosen.
    pub fn set_active(&self, id: Option<&ProviderId>) -> Result<(), ProviderError> {
        let mut state = lock(&self.state);
        match id {
            None => state.active = None,
            Some(id) => {
                let provider = state
                    .providers
                    .iter()
                    .find(|provider| &provider.id == id)
                    .ok_or_else(|| ProviderError::NotFound(id.to_string()))?;
                if !provider.enabled {
                    return Err(ProviderError::Disabled(id.to_string()));
                }
                state.active = Some(id.clone());
            }
        }
        self.store.save(&state)?;
        Ok(())
    }

    /// What `id` has consumed today, reservations included.
    pub fn usage_today(&self, id: &ProviderId) -> Usage {
        lock(&self.ledger).spent(id, self.clock.day())
    }

    /// A client for `id`: its key is resolved now, held by the client, and nowhere else.
    #[cfg(feature = "http")]
    pub fn client(&self, id: &ProviderId) -> Result<crate::ProviderClient, ProviderError> {
        let config = self
            .get(id)
            .ok_or_else(|| ProviderError::NotFound(id.to_string()))?;
        if !config.enabled {
            return Err(ProviderError::Disabled(id.to_string()));
        }
        let secret = self.secrets.resolve(&config.credential)?;
        if !config.preset.is_local() && secret.is_none() {
            return Err(ProviderError::MissingSecret(id.to_string()));
        }
        Ok(crate::ProviderClient::new(
            config,
            secret,
            Arc::clone(&self.ledger),
            Arc::clone(&self.clock),
        ))
    }

    /// The active provider loses its place when it is removed or disabled.
    fn drop_stale_active(state: &mut Snapshot) {
        let keeps = state.active.as_ref().is_some_and(|id| {
            state
                .providers
                .iter()
                .any(|provider| &provider.id == id && provider.enabled)
        });
        if !keeps {
            state.active = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::budget::SystemClock;
    use crate::secret::Secrets;
    use crate::store::MemoryStore;
    use this_llm::{CredentialRef, Preset};

    fn hub() -> Hub {
        Hub::open(MemoryStore::default(), Secrets::env_only(), SystemClock).unwrap()
    }

    fn vllm(id: &str) -> ProviderConfig {
        ProviderConfig::new(
            ProviderId::new(id).unwrap(),
            id,
            Preset::Vllm,
            CredentialRef::None,
        )
    }

    #[test]
    fn a_provider_is_added_replaced_and_removed() {
        let hub = hub();
        hub.upsert(vllm("a")).unwrap();
        let mut changed = vllm("a");
        changed.label = "renamed".into();
        hub.upsert(changed).unwrap();
        assert_eq!(hub.providers().len(), 1);
        assert_eq!(hub.providers()[0].label, "renamed");

        hub.remove(&ProviderId::new("a").unwrap()).unwrap();
        assert!(hub.providers().is_empty());
        assert!(matches!(
            hub.remove(&ProviderId::new("a").unwrap()),
            Err(ProviderError::NotFound(_))
        ));
    }

    #[test]
    fn an_invalid_provider_is_refused_before_it_is_stored() {
        let hub = hub();
        let mut bad = vllm("a");
        bad.base_url = "ftp://nope".into();
        assert!(matches!(hub.upsert(bad), Err(ProviderError::Config(_))));
        assert!(hub.providers().is_empty());
    }

    #[test]
    fn the_active_provider_is_one_that_exists_and_is_enabled() {
        let hub = hub();
        let id = ProviderId::new("a").unwrap();
        hub.upsert(vllm("a")).unwrap();
        hub.set_active(Some(&id)).unwrap();
        assert_eq!(hub.active().map(|p| p.id), Some(id.clone()));

        let mut off = vllm("a");
        off.enabled = false;
        hub.upsert(off).unwrap();
        assert!(hub.active().is_none(), "désactivé : plus actif");
        assert!(matches!(
            hub.set_active(Some(&id)),
            Err(ProviderError::Disabled(_))
        ));
    }

    #[test]
    fn removing_the_active_provider_leaves_none_active() {
        let hub = hub();
        let id = ProviderId::new("a").unwrap();
        hub.upsert(vllm("a")).unwrap();
        hub.set_active(Some(&id)).unwrap();
        hub.remove(&id).unwrap();
        assert!(hub.active().is_none());
        assert_eq!(hub.get(&id), None);
    }

    #[cfg(feature = "http")]
    #[test]
    fn a_remote_provider_gets_no_client_without_its_key() {
        let hub = hub();
        let remote = ProviderConfig::new(
            ProviderId::new("ds").unwrap(),
            "DeepSeek",
            Preset::DeepSeek,
            CredentialRef::Env("THIS_LLM_PROVIDERS_TEST_UNSET_KEY".into()),
        );
        hub.upsert(remote).unwrap();
        let error = hub
            .client(&ProviderId::new("ds").unwrap())
            .err()
            .expect("pas de client sans clé");
        assert!(matches!(error, ProviderError::Secret(_)));
    }
}
