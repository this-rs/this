//! Un secret ne part jamais en clair vers une machine qui n'est pas la nôtre : ni à l'ouverture d'un
//! store prérempli, ni à la création d'un client. La configuration interdite est refusée AVANT que le
//! secret soit résolu, donc a fortiori avant tout envoi.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use this_llm_providers::{
    Clock, CredentialRef, Hub, MemoryStore, Preset, ProviderConfig, ProviderError, ProviderId,
    Secret, SecretError, SecretResolver, Snapshot, StoreError,
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
fn un_store_prerempli_avec_un_secret_en_clair_vers_un_autre_hote_n_ouvre_pas() {
    for (preset, url) in CAS {
        for credential in [
            CredentialRef::Env("K".into()),
            CredentialRef::Vault("k".into()),
            CredentialRef::Session("s".into()),
        ] {
            let espion = Espion::default();
            let store = MemoryStore::new(Snapshot {
                active: None,
                providers: vec![interdite(preset, credential.clone(), url)],
            });
            let Err(erreur) = Hub::open(store, espion.clone(), Jour) else {
                panic!("{preset:?} + {credential:?} + {url} : le hub ne doit pas s'ouvrir");
            };
            assert!(
                matches!(erreur, ProviderError::Store(StoreError::Format(_))),
                "{erreur:?}"
            );
            assert_eq!(
                espion.0.load(Ordering::SeqCst),
                0,
                "aucun secret résolu à l'ouverture"
            );
        }
    }
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
