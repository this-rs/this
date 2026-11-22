//! Un secret ne part jamais en clair vers une machine qui n'est pas la nôtre. Une configuration
//! interdite venue d'un store (prérempli, personnalisé, ancienne version) reste VISIBLE — pour être
//! corrigée dans l'interface — mais `Hub::client` la refuse AVANT que le secret soit résolu, donc a
//! fortiori avant tout envoi ; `Hub::upsert` la refuse aussi. Toute autre faute arrête l'ouverture.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use this_llm_providers::{
    Clock, CredentialRef, Hub, MemoryStore, Preset, ProviderConfig, ProviderError, ProviderId,
    ProviderStore, Secret, SecretError, SecretResolver, Snapshot, StoreError,
};

struct Jour;
impl Clock for Jour {
    fn day(&self) -> u64 {
        20_000
    }
}

/// Un résolveur qui COMPTE ses appels : toute résolution d'un secret est visible.
#[derive(Clone, Default)]
struct Espion(Arc<AtomicUsize>);

impl SecretResolver for Espion {
    fn resolve(&self, credential: &CredentialRef) -> Result<Option<Secret>, SecretError> {
        self.0.fetch_add(1, Ordering::SeqCst);
        match credential {
            CredentialRef::None => Ok(None),
            _ => Ok(Some(Secret::new("sk-secret-a-ne-jamais-envoyer"))),
        }
    }
}

fn id(nom: &str) -> ProviderId {
    ProviderId::new(nom).unwrap()
}

/// Une configuration INTERDITE telle qu'un store générique peut la contenir : on la fabrique en
/// contournant `Hub::upsert` (qui, lui, valide).
fn interdite(preset: Preset, credential: CredentialRef, url: &str) -> ProviderConfig {
    let mut config = ProviderConfig::new(id("p"), "P", preset, credential);
    config.base_url = url.into();
    config
}

const CAS: [(Preset, &str); 3] = [
    (Preset::OpenAiCompatible, "http://api.example.com/v1"),
    (Preset::Ollama, "http://localhost.exemple.test:11434/v1"),
    (Preset::OpenAi, "http://127.0.0.1.evil.com/v1"),
];

#[test]
fn un_store_prerempli_avec_une_config_interdite_s_ouvre_la_montre_et_refuse_le_client_sans_lire_le_secret()
 {
    for (preset, url) in CAS {
        for credential in [
            CredentialRef::Env("K".into()),
            CredentialRef::Vault("k".into()),
            CredentialRef::Session("s".into()),
        ] {
            let espion = Espion::default();
            let store = MemoryStore::new(Snapshot {
                active: Some(id("p")),
                providers: vec![interdite(preset, credential.clone(), url)],
            });
            let hub = Hub::open(store, espion.clone(), Jour).unwrap_or_else(|e| {
                panic!("{preset:?} + {credential:?} + {url} : ouvert, visible : {e:?}")
            });
            assert_eq!(hub.providers().len(), 1, "visible pour être corrigée");
            let Err(erreur) = hub.client(&id("p")) else {
                panic!("{preset:?} + {credential:?} + {url} : aucun client ne doit être créé");
            };
            assert!(
                matches!(
                    erreur,
                    ProviderError::Config(this_llm_providers::ConfigError::InsecureRemote)
                ),
                "{erreur:?}"
            );
            assert_eq!(
                espion.0.load(Ordering::SeqCst),
                0,
                "le secret n'a jamais été résolu"
            );
        }
    }
}

#[test]
fn toute_autre_faute_d_un_store_arrete_l_ouverture() {
    let mut cassee = interdite(Preset::Ollama, CredentialRef::None, "http://127.0.0.1:1/v1");
    cassee.timeout_secs = 0; // pas une faute de transport : refus bruyant à l'ouverture
    let store = MemoryStore::new(Snapshot {
        active: None,
        providers: vec![cassee],
    });
    assert!(matches!(
        Hub::open(store, Espion::default(), Jour),
        Err(ProviderError::Store(StoreError::Format(_)))
    ));
}

#[test]
fn upsert_refuse_une_configuration_interdite_sans_lire_le_secret() {
    for (preset, url) in CAS {
        let espion = Espion::default();
        let hub = Hub::open(MemoryStore::default(), espion.clone(), Jour).unwrap();
        let config = interdite(preset, CredentialRef::Env("K".into()), url);
        assert!(hub.upsert(config).is_err(), "{preset:?} + {url}");
        assert!(hub.providers().is_empty(), "rien n'est enregistré");
        assert_eq!(espion.0.load(Ordering::SeqCst), 0, "aucun secret résolu");
    }
}

#[test]
fn le_lan_sans_secret_s_ouvre_toujours() {
    let store = MemoryStore::new(Snapshot {
        active: None,
        providers: vec![interdite(
            Preset::Ollama,
            CredentialRef::None,
            "http://192.168.1.20:11434/v1",
        )],
    });
    assert!(Hub::open(store, Espion::default(), Jour).is_ok());
}

/// Un `ProviderStore` PERSONNALISÉ (base, API, fichier d'une autre version…) qui ne valide rien :
/// `JsonFileStore` valide à la lecture, mais ce contrat-là n'impose rien à un store tiers.
struct StoreSansValidation(Snapshot);

impl ProviderStore for StoreSansValidation {
    fn load(&self) -> Result<Snapshot, StoreError> {
        Ok(self.0.clone())
    }
    fn save(&self, _: &Snapshot) -> Result<(), StoreError> {
        Ok(())
    }
}

#[test]
fn un_store_personnalise_qui_ne_valide_rien_est_rattrape_par_le_hub() {
    for (preset, url) in CAS {
        let espion = Espion::default();
        let store = StoreSansValidation(Snapshot {
            active: None,
            providers: vec![interdite(preset, CredentialRef::Vault("k".into()), url)],
        });
        let hub =
            Hub::open(store, espion.clone(), Jour).expect("ouvert : la faute est de transport");
        assert!(hub.client(&id("p")).is_err(), "{preset:?} + {url}");
        assert_eq!(espion.0.load(Ordering::SeqCst), 0, "aucun secret résolu");
    }
}
