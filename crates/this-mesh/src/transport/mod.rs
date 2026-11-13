//! Transport QUIC du maillage : cadrage, serveur, client, coupure à la révocation.
//!
//! Sous la feature `transport` (éteinte par défaut) : la crate reste pure et
//! sans I/O sans elle. Le fil est un canal QUIC par message, en TLS 1.3 mutuel,
//! certificat Ed25519 auto-signé épinglé par la clé d'appareil (voir
//! [`tls`]). Chaque message est cadré (longueur 4 octets big-endian puis octets),
//! et la longueur annoncée est refusée avant lecture si elle dépasse le plafond.

mod cadrage;
mod tls;

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use ed25519_dalek::VerifyingKey;
use serde::{Deserialize, Serialize};
use this_vfs::layout::AnchorKeyState;
use tokio::sync::{Mutex, mpsc};

use crate::{DeviceId, Gate, Limits, Message, Peers, Raison, Verdict};

pub use cadrage::CadrageError;
pub use tls::{Epingle, Identite};

/// Protocole d'application négocié en ALPN : empêche la confusion de protocole.
pub const ALPN: &[u8] = b"this-mesh/1";

/// Plafond du verdict renvoyé au client (petit objet JSON, jamais un message).
const ACK_CEILING: usize = 4096;

#[derive(Debug, thiserror::Error)]
pub enum TransportError {
    #[error("épinglage : {0}")]
    Epinglage(String),
    #[error("cadrage : {0}")]
    Cadrage(#[from] CadrageError),
    #[error("connexion : {0}")]
    Connexion(String),
    #[error("identité : {0}")]
    Identite(String),
    #[error("refus : {0}")]
    Refuse(String),
    #[error("fin de flux")]
    FinDeFlux,
}

/// Le flux des messages reçus par un serveur : un `(expéditeur, verdict)` par
/// message admis, refusé, ignoré ou déjà-vu.
pub struct Flux {
    rx: mpsc::UnboundedReceiver<(DeviceId, Verdict)>,
    local: SocketAddr,
    endpoint: quinn::Endpoint,
}

impl Flux {
    pub fn local_addr(&self) -> SocketAddr {
        self.local
    }

    /// Le prochain verdict, ou `None` quand le serveur est fermé.
    pub async fn prochain(&mut self) -> Option<(DeviceId, Verdict)> {
        self.rx.recv().await
    }
}

impl Drop for Flux {
    fn drop(&mut self) {
        // Ferme l'endpoint : la boucle d'acceptation se termine et les
        // connexions en cours sont coupées — aucune tâche ne traîne.
        self.endpoint
            .close(quinn::VarInt::from_u32(0), b"serveur ferme");
    }
}

/// Écoute sur `adresse`, présente `identite`, n'admet que des clients dont la
/// clé est dans `peers`, et rend le flux des verdicts de [`Gate::admettre`].
///
/// La décision « clé connue / révoquée » revient à la porte (source unique) ;
/// le contrôle de révocation est aussi fait à l'acceptation de chaque connexion.
pub async fn ecouter(
    adresse: SocketAddr,
    identite: &Identite,
    peers: Peers,
    limits: Limits,
) -> Result<Flux, TransportError> {
    let ceiling = limits.ceiling();
    let gate = Arc::new(Mutex::new(Gate::new(peers, limits)));

    let server_config = tls::quic_server(tls::config_serveur(identite)?)?;
    let endpoint = quinn::Endpoint::server(server_config, adresse)
        .map_err(|e| TransportError::Connexion(e.to_string()))?;
    let local = endpoint
        .local_addr()
        .map_err(|e| TransportError::Connexion(e.to_string()))?;

    let (tx, rx) = mpsc::unbounded_channel();
    let accept_endpoint = endpoint.clone();
    let accept_gate = gate.clone();
    let accept_tx = tx.clone();
    tokio::spawn(async move {
        while let Some(incoming) = accept_endpoint.accept().await {
            let gate = accept_gate.clone();
            let tx = accept_tx.clone();
            tokio::spawn(async move {
                if let Err(e) = gerer_connexion(incoming, gate, tx, ceiling).await {
                    // La raison d'un refus est déjà rendue au flux (et au client) ;
                    // ici on ne garde que la trace de la fermeture.
                    let _ = e;
                }
            });
        }
    });

    Ok(Flux {
        rx,
        local,
        endpoint,
    })
}

/// Envoie `message` au serveur `adresse`, en présentant `identite` (le client)
/// et en épinglant le serveur sur `epingle`. Rend le verdict du serveur.
pub async fn envoyer(
    adresse: SocketAddr,
    identite: &Identite,
    epingle: &Epingle,
    message: &Message,
) -> Result<Verdict, TransportError> {
    let mut endpoint = quinn::Endpoint::client("0.0.0.0:0".parse().expect("adresse valide"))
        .map_err(|e| TransportError::Connexion(e.to_string()))?;
    endpoint.set_default_client_config(tls::quic_client(tls::config_client(identite, epingle)?)?);

    let connection = endpoint
        .connect(adresse, "this-mesh.local")
        .map_err(|e| TransportError::Connexion(e.to_string()))?
        .await
        .map_err(|e| TransportError::Connexion(e.to_string()))?;

    let (mut send, mut recv) = connection
        .open_bi()
        .await
        .map_err(|e| TransportError::Connexion(e.to_string()))?;
    cadrage::ecrire(&mut send, &message.to_bytes())
        .await
        .map_err(CadrageError::from)?;
    send.finish()
        .map_err(|e| TransportError::Connexion(e.to_string()))?;

    let ack = cadrage::lire(&mut recv, ACK_CEILING).await?;
    let ack: Ack = serde_json::from_slice(&ack)
        .map_err(|e| TransportError::Connexion(format!("verdict illisible : {e}")))?;
    verdict_du_ack(&ack, message)
}

/// Gère une connexion : authentifie le pair par sa clé épinglée, contrôle la
/// révocation à l'acceptation, puis admet chaque message cadré.
async fn gerer_connexion(
    incoming: quinn::Incoming,
    gate: Arc<Mutex<Gate>>,
    tx: mpsc::UnboundedSender<(DeviceId, Verdict)>,
    ceiling: usize,
) -> Result<(), TransportError> {
    let connection = incoming
        .await
        .map_err(|e| TransportError::Connexion(e.to_string()))?;

    // 1. Identité authentifiée par le canal : la clé publique du certificat client.
    let cle = cle_du_pair(&connection)?;
    let vk = VerifyingKey::from_bytes(&cle).map_err(|e| TransportError::Identite(e.to_string()))?;

    // 2. L'identifiant connu pour cette clé, et la coupure à la révocation à
    // l'acceptation : une clé révoquée n'établit rien. Une clé **inconnue**
    // n'est pas refusée ici : la porte la refuse au premier message avec
    // `CleInconnue` (ce qui garde intacte la protection contre le rejeu).
    let expediteur_connu = {
        let gate = gate.lock().await;
        match gate.peers().id_de_cle(&vk) {
            Some(id) => {
                if gate.peers().state(&id) != Some(AnchorKeyState::Active) {
                    connection.close(quinn::VarInt::from_u32(1), b"cle revoquee");
                    return Err(TransportError::Refuse(format!("clé révoquée : {id}")));
                }
                Some(id)
            }
            None => None,
        }
    };

    // 3. Boucle des messages : cadrage, porte, verdict rendu tel quel.
    loop {
        let (mut send, mut recv) = match connection.accept_bi().await {
            Ok(stream) => stream,
            // Fermeture propre ou erreur : fin de la connexion.
            Err(_) => break,
        };

        let octets = match cadrage::lire(&mut recv, ceiling).await {
            Ok(o) => o,
            Err(CadrageError::TropGros { .. }) => {
                // Un pair annonce une longueur au-delà du plafond : on ferme sans
                // avoir alloué (le refus est antérieur à la lecture du corps).
                connection.close(quinn::VarInt::from_u32(2), b"cadrage hors plafond");
                return Ok(());
            }
            Err(e) => {
                connection.close(quinn::VarInt::from_u32(3), b"cadrage invalide");
                return Err(e.into());
            }
        };

        let (expediteur, verdict) = match &expediteur_connu {
            Some(id) => {
                let mut gate = gate.lock().await;
                let verdict = gate.admettre(id, &octets);
                (id.clone(), verdict)
            }
            None => {
                let de = de_annonce(&octets);
                let id = DeviceId::parse(&de)
                    .unwrap_or_else(|| DeviceId::parse("device-00000000").expect("valide"));
                (id, Verdict::Refuse(Raison::CleInconnue(de)))
            }
        };

        let ack = ack_du_verdict(&verdict);
        let ack_bytes = serde_json::to_vec(&ack).expect("verdict serialise");
        let refuse = matches!(verdict, Verdict::Refuse(_));
        if cadrage::ecrire(&mut send, &ack_bytes).await.is_ok() {
            let _ = send.finish();
        }
        let _ = tx.send((expediteur, verdict));
        if refuse {
            // Refuse ferme la connexion. L'acquittement (et la raison) est déjà
            // écrit et le flux fini ; on attend (borné) que le pair ferme pour
            // que l'acquittement parte avant la fermeture — fermer tout de suite
            // le perdrait. Un pair qui ne ferme pas est coupé après le délai.
            let _ = tokio::time::timeout(Duration::from_secs(1), connection.closed()).await;
            connection.close(quinn::VarInt::from_u32(4), b"refuse");
            return Ok(());
        }
    }

    Ok(())
}

/// L'`de` annoncé dans l'enveloppe, sans décoder la charge : sert de raison à
/// un refus `CleInconnue` (jamais de décision). En échec, une étiquette neutre.
fn de_annonce(octets: &[u8]) -> String {
    #[derive(Deserialize)]
    struct Entete<'a> {
        #[serde(borrow)]
        de: &'a str,
    }
    serde_json::from_slice::<Entete<'_>>(octets)
        .map(|e| e.de.to_owned())
        .unwrap_or_default()
}

/// La clé publique Ed25519 authentifiée par le canal pour ce pair.
fn cle_du_pair(connection: &quinn::Connection) -> Result<[u8; 32], TransportError> {
    let cert = connection
        .peer_identity()
        .and_then(|id| {
            id.downcast::<Vec<rustls::pki_types::CertificateDer<'static>>>()
                .ok()
        })
        .and_then(|mut v| {
            if v.is_empty() {
                None
            } else {
                Some(v.remove(0))
            }
        })
        .ok_or_else(|| TransportError::Identite("identité du pair absente".into()))?;
    tls::cle_ed25519_du_cert(cert.as_ref()).map_err(|e| TransportError::Identite(e.to_string()))
}

/// Verdict du serveur, sur le fil : un petit objet JSON.
#[derive(Debug, Serialize, Deserialize)]
struct Ack {
    v: String,
    #[serde(default)]
    id: Option<String>,
    #[serde(default, rename = "type")]
    type_: Option<String>,
    #[serde(default)]
    raison: Option<String>,
}

fn ack_du_verdict(v: &Verdict) -> Ack {
    match v {
        Verdict::Admis(_) => Ack {
            v: "admis".into(),
            id: None,
            type_: None,
            raison: None,
        },
        Verdict::DejaVu(id) => Ack {
            v: "deja-vu".into(),
            id: Some(id.to_string()),
            type_: None,
            raison: None,
        },
        Verdict::Ignore(t) => Ack {
            v: "ignore".into(),
            id: None,
            type_: Some(t.clone()),
            raison: None,
        },
        Verdict::Refuse(r) => Ack {
            v: "refuse".into(),
            id: None,
            type_: None,
            raison: Some(r.to_string()),
        },
    }
}

fn verdict_du_ack(ack: &Ack, message: &Message) -> Result<Verdict, TransportError> {
    match ack.v.as_str() {
        "admis" => Ok(Verdict::Admis(message.clone())),
        "deja-vu" => {
            let id = ack
                .id
                .as_deref()
                .ok_or_else(|| TransportError::Connexion("deja-vu sans id".into()))?;
            let id =
                uuid::Uuid::parse_str(id).map_err(|e| TransportError::Connexion(e.to_string()))?;
            Ok(Verdict::DejaVu(id))
        }
        "ignore" => Ok(Verdict::Ignore(ack.type_.clone().unwrap_or_default())),
        "refuse" => Err(TransportError::Refuse(
            ack.raison.clone().unwrap_or_default(),
        )),
        autre => Err(TransportError::Connexion(format!(
            "verdict inconnu : {autre}"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{MessageType, Peers, Raison};
    use chrono::Utc;
    use ed25519_dalek::SigningKey;
    use serde_json::json;
    use std::time::Duration;
    use this_vfs::layout::AnchorKeyRole;
    use uuid::Uuid;

    const DELAI: Duration = Duration::from_secs(5);

    /// Attente bornée : un test ne doit jamais pendre sur un flux silencieux.
    async fn avec_delai<T>(d: Duration, f: impl std::future::Future<Output = T>) -> Option<T> {
        tokio::time::timeout(d, f).await.ok()
    }

    fn dev(s: &str) -> DeviceId {
        DeviceId::parse(s).unwrap()
    }
    fn cle(n: u8) -> SigningKey {
        SigningKey::from_bytes(&[n; 32])
    }
    fn identite(n: u8, id: &str) -> Identite {
        Identite::de_cle(&cle(n), &dev(id)).unwrap()
    }
    fn epingle(n: u8) -> Epingle {
        Epingle::cle(&cle(n).verifying_key())
    }

    /// Registre avec `alice` Active (rôle Shared), clé `[1;32]`.
    fn registre_alice() -> Peers {
        let mut peers = Peers::new();
        peers.admit(
            &dev("device-aaaaaaaa"),
            cle(1).verifying_key(),
            AnchorKeyRole::Shared,
            Utc::now(),
        );
        peers
    }

    fn message(k: &SigningKey, de: &str, a: &str, id: u128) -> Message {
        Message::signed(
            k,
            Uuid::from_u128(id),
            dev(de),
            dev(a),
            MessageType::EspaceOp,
            1,
            json!({"liste":"courses","op":"add"}),
        )
        .unwrap()
    }

    // (a) Deux endpoints échangent un message signé : la porte rend `Admis` et
    // le message relu est identique.
    #[tokio::test]
    async fn a_echange_signe_est_admis_et_relu_a_l_identique() {
        // Alice écoute (clé [1]), Bob envoie (clé [2], enregistrée elle aussi).
        let mut serveur = ecouter(
            "127.0.0.1:0".parse().unwrap(),
            &identite(1, "device-aaaaaaaa"),
            registre_alice(),
            Limits::default(),
        )
        .await
        .unwrap();
        let adresse = serveur.local_addr();

        // Bob doit être connu du registre pour que le serveur accepte sa clé.
        // On réutilise la clé d'alice pour simplifier : le message est signé par alice.
        let m = message(&cle(1), "device-aaaaaaaa", "device-bbbbbbbb", 1);
        let verdict = envoyer(adresse, &identite(1, "device-aaaaaaaa"), &epingle(1), &m).await;

        let (expediteur, recu) = avec_delai(DELAI, serveur.prochain())
            .await
            .flatten()
            .expect("aucun verdict reçu");
        assert_eq!(expediteur, dev("device-aaaaaaaa"));
        match &recu {
            Verdict::Admis(relu) => assert_eq!(relu, &m),
            other => panic!("attendu Admis, reçu {other:?}"),
        }
        // Côté client, le verdict rendu est bien `Admis`.
        assert_eq!(verdict.unwrap(), Verdict::Admis(m));
    }

    // (b) Une clé épinglée qui ne correspond pas → la connexion échoue, le
    // message n'est pas transmis.
    #[tokio::test]
    async fn b_epinglage_incorrect_fait_echouer_la_connexion() {
        let mut serveur = ecouter(
            "127.0.0.1:0".parse().unwrap(),
            &identite(1, "device-aaaaaaaa"),
            registre_alice(),
            Limits::default(),
        )
        .await
        .unwrap();
        let adresse = serveur.local_addr();

        // Le client épingle une AUTRE clé que celle du serveur.
        let m = message(&cle(1), "device-aaaaaaaa", "device-bbbbbbbb", 2);
        let resultat = envoyer(adresse, &identite(1, "device-aaaaaaaa"), &epingle(9), &m).await;
        assert!(resultat.is_err(), "la connexion aurait dû échouer");
        // Aucun message n'est reçu côté serveur.
        assert!(
            avec_delai(DELAI, serveur.prochain())
                .await
                .flatten()
                .is_none()
        );
    }

    // (c) Longueur annoncée au-delà du plafond → refus immédiat, aucune
    // allocation (couvert aussi au niveau cadrage par `cadrage::tests`).
    #[tokio::test]
    async fn c_longueur_au_dela_du_plafond_ferme_sans_admettre() {
        let mut serveur = ecouter(
            "127.0.0.1:0".parse().unwrap(),
            &identite(1, "device-aaaaaaaa"),
            registre_alice(),
            Limits::default(),
        )
        .await
        .unwrap();
        let adresse = serveur.local_addr();

        // Client brut (identité valide) qui annonce u32::MAX octets.
        let mut endpoint = quinn::Endpoint::client("0.0.0.0:0".parse().unwrap()).unwrap();
        endpoint.set_default_client_config(
            tls::quic_client(
                tls::config_client(&identite(1, "device-aaaaaaaa"), &epingle(1)).unwrap(),
            )
            .unwrap(),
        );
        let connection = endpoint
            .connect(adresse, "this-mesh.local")
            .expect("connexion lancée")
            .await
            .expect("poignée de main valide");
        let (mut send, _recv) = connection.open_bi().await.unwrap();
        send.write_all(&u32::MAX.to_be_bytes()).await.unwrap();
        send.finish().unwrap();

        // Aucun verdict admis : la connexion est fermée sans lecture du corps.
        assert!(
            avec_delai(DELAI, serveur.prochain())
                .await
                .flatten()
                .is_none()
        );
    }

    // (d) Un message signé par une clé absente du registre → Refuse(CleInconnue)
    // et la connexion se ferme.
    #[tokio::test]
    async fn d_cle_inconnue_est_refusee_et_ferme_la_connexion() {
        let mut serveur = ecouter(
            "127.0.0.1:0".parse().unwrap(),
            &identite(1, "device-aaaaaaaa"),
            registre_alice(),
            Limits::default(),
        )
        .await
        .unwrap();
        let adresse = serveur.local_addr();

        // Bob (clé [2]) n'est PAS dans le registre : la porte doit refuser.
        let m = message(&cle(2), "device-bbbbbbbb", "device-aaaaaaaa", 3);
        let resultat = envoyer(adresse, &identite(2, "device-bbbbbbbb"), &epingle(1), &m).await;
        assert!(matches!(resultat, Err(TransportError::Refuse(_))));

        let (expediteur, recu) = avec_delai(DELAI, serveur.prochain())
            .await
            .flatten()
            .expect("aucun verdict reçu");
        assert_eq!(expediteur, dev("device-bbbbbbbb"));
        assert_eq!(
            recu,
            Verdict::Refuse(Raison::CleInconnue("device-bbbbbbbb".into()))
        );
        // La connexion est fermée : plus rien n'arrive.
        assert!(
            avec_delai(DELAI, serveur.prochain())
                .await
                .flatten()
                .is_none()
        );
    }

    // (e) Un message rejoué (même id) → DejaVu.
    #[tokio::test]
    async fn e_rejeu_est_deja_vu() {
        let mut serveur = ecouter(
            "127.0.0.1:0".parse().unwrap(),
            &identite(1, "device-aaaaaaaa"),
            registre_alice(),
            Limits::default(),
        )
        .await
        .unwrap();
        let adresse = serveur.local_addr();

        let m = message(&cle(1), "device-aaaaaaaa", "device-bbbbbbbb", 7);
        let premier = envoyer(adresse, &identite(1, "device-aaaaaaaa"), &epingle(1), &m).await;
        assert_eq!(premier.unwrap(), Verdict::Admis(m.clone()));
        let (_e, recu) = avec_delai(DELAI, serveur.prochain())
            .await
            .flatten()
            .unwrap();
        assert!(matches!(recu, Verdict::Admis(_)));

        // Rejeu : même id, même signature.
        let second = envoyer(adresse, &identite(1, "device-aaaaaaaa"), &epingle(1), &m).await;
        assert_eq!(second.unwrap(), Verdict::DejaVu(Uuid::from_u128(7)));
        let (_e, recu) = avec_delai(DELAI, serveur.prochain())
            .await
            .flatten()
            .unwrap();
        assert_eq!(recu, Verdict::DejaVu(Uuid::from_u128(7)));
    }

    // Révocation à l'acceptation : une clé révoquée n'établit pas de connexion.
    #[tokio::test]
    async fn une_cle_revoquee_n_etablit_pas_de_connexion() {
        let mut peers = registre_alice();
        // Alice est connue puis révoquée.
        peers.revoke(&dev("device-aaaaaaaa"));
        let mut serveur = ecouter(
            "127.0.0.1:0".parse().unwrap(),
            &identite(1, "device-aaaaaaaa"),
            peers,
            Limits::default(),
        )
        .await
        .unwrap();
        let adresse = serveur.local_addr();

        let m = message(&cle(1), "device-aaaaaaaa", "device-bbbbbbbb", 8);
        let resultat = envoyer(adresse, &identite(1, "device-aaaaaaaa"), &epingle(1), &m).await;
        // La poignée de main aboutit (la clé est extraite), mais la connexion
        // est fermée à l'acceptation avant tout message : l'envoi échoue.
        assert!(resultat.is_err());
        assert!(
            avec_delai(DELAI, serveur.prochain())
                .await
                .flatten()
                .is_none()
        );
    }
}
