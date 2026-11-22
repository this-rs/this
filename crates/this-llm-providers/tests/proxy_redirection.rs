//! Même vers une IP loopback validée, un Bearer pourrait encore quitter la machine par deux chemins
//! que la validation ne voit pas : un proxy d'environnement (`HTTP_PROXY`, `ALL_PROXY`) et une
//! redirection (302 vers un autre hôte). Ce test les ferme tous les deux, en réel : trois écouteurs
//! locaux, un processus enfant (ce binaire, ré-invoqué) qui porte les variables de proxy, et la
//! preuve que la cible reçoit le Bearer tandis que le proxy et la cible de redirection ne reçoivent
//! RIEN. Aucune requête ne sort de 127.0.0.1.
#![cfg(all(feature = "http", not(target_arch = "wasm32")))]

use std::io::{Read, Write};
use std::net::TcpListener;
use std::process::Command;
use std::time::{Duration, Instant};

use this_llm_providers::{
    ChatRequest, Clock, CredentialRef, Hub, MemoryStore, Preset, ProviderConfig, ProviderId,
    Secrets,
};

struct Jour;
impl Clock for Jour {
    fn day(&self) -> u64 {
        20_000
    }
}

const CLE: &str = "test-only-token";

/// L'ENFANT : un vrai client, un vrai `chat`, avec HTTP_PROXY posé par le parent. Son résultat
/// importe peu (la cible répond 302, non suivie → erreur attendue) ; ce qui compte est OÙ la
/// requête est partie, et le parent le mesure.
#[test]
#[ignore = "lancé seulement par le test parent, dans un processus enfant"]
fn enfant_proxy() {
    let cible = std::env::var("THIS_PROXY_TEST_CIBLE").expect("cible posée par le parent");
    let mut config = ProviderConfig::new(
        ProviderId::new("local").unwrap(),
        "Local",
        Preset::OpenAiCompatible,
        CredentialRef::Vault("k".into()),
    );
    config.base_url = cible;
    let hub = Hub::open(
        MemoryStore::default(),
        Secrets::with_vault(|_| Some(CLE.to_string())),
        Jour,
    )
    .unwrap();
    hub.upsert(config.clone()).unwrap();
    let client = hub.client(&config.id).unwrap();
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let _ = rt.block_on(client.chat(ChatRequest::new("sys", "ping")));
}

#[test]
fn un_bearer_en_http_loopback_ne_passe_ni_par_le_proxy_ni_par_une_redirection() {
    let cible = TcpListener::bind("127.0.0.1:0").unwrap();
    let proxy = TcpListener::bind("127.0.0.1:0").unwrap();
    let redirection = TcpListener::bind("127.0.0.1:0").unwrap();
    for l in [&cible, &proxy, &redirection] {
        l.set_nonblocking(true).unwrap();
    }
    let url_cible = format!("http://{}/v1", cible.local_addr().unwrap());
    let url_proxy = format!("http://{}", proxy.local_addr().unwrap());
    let url_redirection = format!("http://{}/vole", redirection.local_addr().unwrap());

    let repondeur = std::thread::spawn(move || {
        let limite = Instant::now() + Duration::from_secs(10);
        let mut socket = loop {
            match cible.accept() {
                Ok((s, _)) => break s,
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock && Instant::now() < limite => {
                    std::thread::sleep(Duration::from_millis(10))
                }
                Err(e) => panic!("la cible loopback n'a pas reçu la requête : {e}"),
            }
        };
        socket
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        let mut tampon = [0; 8192];
        let n = socket.read(&mut tampon).unwrap();
        let requete = String::from_utf8_lossy(&tampon[..n]).to_lowercase();
        assert!(
            requete.contains(&format!("authorization: bearer {CLE}")),
            "le Bearer doit atteindre la cible loopback : {requete}"
        );
        write!(
            socket,
            "HTTP/1.1 302 Found\r\nLocation: {url_redirection}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
        )
        .unwrap();
    });

    let sortie = Command::new(std::env::current_exe().unwrap())
        .args(["--ignored", "--exact", "enfant_proxy", "--nocapture"])
        .env("THIS_PROXY_TEST_CIBLE", &url_cible)
        .env("HTTP_PROXY", &url_proxy)
        .env("http_proxy", &url_proxy)
        .env("ALL_PROXY", &url_proxy)
        .env("all_proxy", &url_proxy)
        .env("NO_PROXY", "")
        .env("no_proxy", "")
        .output()
        .unwrap();
    repondeur.join().unwrap();
    assert!(
        sortie.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&sortie.stdout),
        String::from_utf8_lossy(&sortie.stderr)
    );
    assert!(
        matches!(proxy.accept(), Err(e) if e.kind() == std::io::ErrorKind::WouldBlock),
        "le Bearer est passé par le proxy"
    );
    assert!(
        matches!(redirection.accept(), Err(e) if e.kind() == std::io::ErrorKind::WouldBlock),
        "la redirection a été suivie : le Bearer a quitté la cible"
    );
}
