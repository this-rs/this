//! Transport QUIC du maillage : cadrage, serveur, client, coupure à la révocation.
//!
//! Sous la feature `transport` (éteinte par défaut) : la crate reste pure et
//! sans I/O sans elle. Le fil est un canal QUIC par message, en TLS 1.3 mutuel,
//! certificat Ed25519 auto-signé épinglé par la clé d'appareil (voir
//! [`tls`]). Chaque message est cadré (longueur 4 octets big-endian puis octets),
//! et la longueur annoncée est refusée avant lecture si elle dépasse le plafond.
//!
//! L'appairage par le réseau ([`appairer_vers`], [`ecouter_avec_appairage`]) passe
//! par un ALPN dédié ([`ALPN_APPAIRAGE`]) sur le même port : une connexion
//! d'appairage ne touche jamais la porte ni le flux des messages.
//!
//! Un pair peut **répondre** : [`ecouter_complet`] prend un [`Repondeur`], appelé
//! pour chaque message admis par la porte (et seulement ceux-là) ; sa réponse
//! voyage dans l'accusé, que [`appeler`] rend au client.

mod appairage;
mod cadrage;
mod tls;

use std::future::Future;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use ed25519_dalek::VerifyingKey;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use this_vfs::layout::{AnchorKeyRole, AnchorKeyState};
use tokio::sync::{Mutex, Notify, mpsc};

use crate::{DeviceId, Gate, Limits, Message, Peers, Raison, Surveillance, Verdict};

pub use appairage::{
    AppairageEnCours, CAPACITE_DEMANDES, DELAI_APPAIRAGE, ENGAGEMENT_DOMAIN, appairer_vers,
};
pub use cadrage::CadrageError;
pub use tls::{Epingle, Identite};

/// Protocole d'application négocié en ALPN : empêche la confusion de protocole.
pub const ALPN: &[u8] = b"this-mesh/1";

/// ALPN de l'appairage : distinct de [`ALPN`], négocié sur le même port. Le
/// serveur aiguille sur l'ALPN négocié ; seul le client d'appairage l'offre.
pub const ALPN_APPAIRAGE: &[u8] = b"this-mesh-pair/1";

/// Plafond du verdict renvoyé au client (petit objet JSON, jamais un message).
/// C'est le plafond de lecture d'[`envoyer`] : une réponse de service n'y tient
/// pas, il faut [`appeler`].
const ACK_CEILING: usize = 4096;

/// Délai accordé au [`Repondeur`] côté serveur (un moteur lent doit pouvoir
/// répondre). Passé ce délai, l'accusé part **sans** réponse.
pub const DELAI_REPONSE: Duration = Duration::from_secs(300);

/// Taille maximale d'une réponse de service, en octets de JSON sérialisé. Au-delà,
/// la réponse est remplacée par `{"erreur":"reponse_trop_grosse"}`.
pub const PLAFOND_REPONSE: usize = 4 * 1024 * 1024;

/// Marge de l'accusé autour de la réponse (`{"v":"admis","reponse":…}`).
const MARGE_ACCUSE: usize = 4096;

/// Ce qui répond aux messages admis : reçoit l'expéditeur authentifié et le
/// message admis, rend éventuellement une valeur à mettre dans l'accusé.
///
/// N'est appelé que pour un verdict [`Verdict::Admis`] : jamais pour un refusé,
/// un ignoré ni un déjà-vu (une opération n'est pas rejouée).
pub type Repondeur = Arc<
    dyn Fn(DeviceId, Message) -> Pin<Box<dyn Future<Output = Option<Value>> + Send>> + Send + Sync,
>;

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
    ecouter_interne(adresse, identite, peers, limits, None, None).await
}

/// Comme [`ecouter`], et accepte en plus les demandes d'appairage (ALPN
/// [`ALPN_APPAIRAGE`]) sur le même port.
///
/// Chaque demande dont l'engagement-révélation a abouti remonte sur le récepteur
/// comme un [`AppairageEnCours`] (code affiché, décision locale attendue). Le
/// récepteur est borné à [`CAPACITE_DEMANDES`] : une demande qui ne tient pas est
/// fermée aussitôt, jamais mise en file. `role_par_defaut` n'est plus
/// qu'**indicatif** (gardé pour la compatibilité) : le rôle de l'[`crate::Admission`]
/// rendue découle du [`crate::Lien`] choisi des deux côtés
/// ([`AppairageEnCours::decider_avec_lien`]). `delai` borne chaque appairage de
/// bout en bout.
///
/// Une clé **révoquée** qui demande l'appairage est fermée d'office, sans remonter.
/// Une clé **active** peut se ré-appairer : l'admission reprend son identifiant et
/// l'inscription met à jour son rôle.
pub async fn ecouter_avec_appairage(
    adresse: SocketAddr,
    identite: &Identite,
    peers: Peers,
    limits: Limits,
    role_par_defaut: AnchorKeyRole,
    delai: Duration,
) -> Result<(Flux, mpsc::Receiver<AppairageEnCours>), TransportError> {
    let (tx, rx) = mpsc::channel(CAPACITE_DEMANDES);
    let repondeur = appairage::RepondeurAppairage::nouveau(
        identite,
        peers.clone(),
        role_par_defaut,
        delai,
        tx,
    )?;
    let flux = ecouter_interne(
        adresse,
        identite,
        peers,
        limits,
        Some(Arc::new(repondeur)),
        None,
    )
    .await?;
    Ok((flux, rx))
}

/// Comme [`ecouter_avec_appairage`], et chaque message **admis** par la porte est
/// passé à `repondeur` ; sa réponse (`Some`) part dans l'accusé, lu par [`appeler`].
///
/// - Seuls les messages admis appellent le répondeur : jamais un refusé, un
///   ignoré ni un déjà-vu (un rejeu n'exécute pas deux fois une opération).
/// - Le verdict part dans le [`Flux`] comme avec [`ecouter`], **avant** l'appel au
///   répondeur : un répondeur lent ne retarde pas le flux.
/// - Aucun verrou de la porte n'est tenu pendant l'attente du répondeur.
/// - Le répondeur a [`DELAI_REPONSE`] ; passé ce délai, l'accusé part sans réponse.
///   Une réponse de plus de [`PLAFOND_REPONSE`] octets devient
///   `{"erreur":"reponse_trop_grosse"}`.
/// - Une révocation de l'expéditeur pendant l'attente coupe la session sans accusé.
/// - Les messages d'une même connexion sont traités un à un : un appel lent
///   retarde les suivants **de cette connexion** (pas ceux des autres pairs).
pub async fn ecouter_complet(
    adresse: SocketAddr,
    identite: &Identite,
    peers: Peers,
    limits: Limits,
    role_par_defaut: AnchorKeyRole,
    delai: Duration,
    repondeur: Repondeur,
) -> Result<(Flux, mpsc::Receiver<AppairageEnCours>), TransportError> {
    ecouter_complet_avec_delai(
        adresse,
        identite,
        peers,
        limits,
        role_par_defaut,
        delai,
        repondeur,
        DELAI_REPONSE,
    )
    .await
}

/// [`ecouter_complet`] avec un délai de réponse choisi (tests).
#[allow(clippy::too_many_arguments)]
pub(crate) async fn ecouter_complet_avec_delai(
    adresse: SocketAddr,
    identite: &Identite,
    peers: Peers,
    limits: Limits,
    role_par_defaut: AnchorKeyRole,
    delai: Duration,
    repondeur: Repondeur,
    delai_reponse: Duration,
) -> Result<(Flux, mpsc::Receiver<AppairageEnCours>), TransportError> {
    let (tx, rx) = mpsc::channel(CAPACITE_DEMANDES);
    let appairage = appairage::RepondeurAppairage::nouveau(
        identite,
        peers.clone(),
        role_par_defaut,
        delai,
        tx,
    )?;
    let flux = ecouter_interne(
        adresse,
        identite,
        peers,
        limits,
        Some(Arc::new(appairage)),
        Some(Service {
            repondeur,
            delai: delai_reponse,
        }),
    )
    .await?;
    Ok((flux, rx))
}

/// Le répondeur de service d'un serveur et son délai.
#[derive(Clone)]
struct Service {
    repondeur: Repondeur,
    delai: Duration,
}

async fn ecouter_interne(
    adresse: SocketAddr,
    identite: &Identite,
    peers: Peers,
    limits: Limits,
    repondeur: Option<Arc<appairage::RepondeurAppairage>>,
    service: Option<Service>,
) -> Result<Flux, TransportError> {
    let ceiling = limits.ceiling();
    let gate = Arc::new(Mutex::new(Gate::new(peers, limits)));

    let server_config = tls::quic_server(tls::config_serveur(identite, repondeur.is_some())?)?;
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
            let repondeur = repondeur.clone();
            let service = service.clone();
            tokio::spawn(async move {
                let Ok(connection) = incoming.await else {
                    return;
                };
                // Aiguillage par l'ALPN négocié, jamais par un message : une
                // connexion d'appairage ne voit ni la porte ni le flux.
                if alpn_negocie(&connection).as_deref() == Some(ALPN_APPAIRAGE) {
                    match repondeur {
                        Some(r) => {
                            let _ = appairage::repondre(connection, &r).await;
                        }
                        None => connection.close(quinn::VarInt::from_u32(6), b"appairage ferme"),
                    }
                    return;
                }
                if let Err(e) = gerer_connexion(connection, gate, tx, ceiling, service).await {
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
    let (ack, _endpoint) = echanger(adresse, identite, epingle, message, ACK_CEILING).await?;
    verdict_du_ack(&ack, message)
}

/// Comme [`envoyer`], et rend aussi la réponse du pair (le `reponse` de l'accusé,
/// voir [`ecouter_complet`]). L'accusé est lu jusqu'à [`PLAFOND_REPONSE`] plus une
/// marge. `delai` borne toute l'opération (connexion, envoi, attente de la
/// réponse) : au-delà, `Err(TransportError::Connexion("délai dépassé"))`.
///
/// `None` en réponse : le pair n'a rien répondu, n'a pas de répondeur, ou son
/// répondeur a dépassé son délai. Un déjà-vu ne porte jamais de réponse.
pub async fn appeler(
    adresse: SocketAddr,
    identite: &Identite,
    epingle: &Epingle,
    message: &Message,
    delai: Duration,
) -> Result<(Verdict, Option<Value>), TransportError> {
    let plafond = PLAFOND_REPONSE + MARGE_ACCUSE;
    let (ack, _endpoint) = tokio::time::timeout(
        delai,
        echanger(adresse, identite, epingle, message, plafond),
    )
    .await
    .map_err(|_| TransportError::Connexion("délai dépassé".into()))??;
    let verdict = verdict_du_ack(&ack, message)?;
    Ok((verdict, ack.reponse))
}

/// Une connexion, un message, un accusé lu sous `plafond`. L'endpoint est rendu
/// pour vivre jusqu'à la lecture du verdict.
async fn echanger(
    adresse: SocketAddr,
    identite: &Identite,
    epingle: &Epingle,
    message: &Message,
    plafond: usize,
) -> Result<(Ack, quinn::Endpoint), TransportError> {
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

    let ack = cadrage::lire(&mut recv, plafond).await?;
    let ack: Ack = serde_json::from_slice(&ack)
        .map_err(|e| TransportError::Connexion(format!("verdict illisible : {e}")))?;
    Ok((ack, endpoint))
}

/// Gère une connexion : authentifie le pair par sa clé épinglée, contrôle la
/// révocation à l'acceptation, puis admet chaque message cadré.
async fn gerer_connexion(
    connection: quinn::Connection,
    gate: Arc<Mutex<Gate>>,
    tx: mpsc::UnboundedSender<(DeviceId, Verdict)>,
    ceiling: usize,
    service: Option<Service>,
) -> Result<(), TransportError> {
    // 1. Identité authentifiée par le canal : la clé publique du certificat client.
    let cle = cle_du_pair(&connection)?;
    let vk = VerifyingKey::from_bytes(&cle).map_err(|e| TransportError::Identite(e.to_string()))?;

    // 2. Coupure ciblée : `coupure` est réveillée si, et seulement si, la clé
    // de CETTE session est révoquée. Une clé inconnue n'est pas refusée ici : la
    // porte la refuse au message avec `CleInconnue` (la mémoire anti-rejeu reste
    // intacte), et la session la reconnaît dès qu'elle est admise.
    let coupure = Arc::new(Notify::new());
    let mut lien = match resoudre(&gate, &vk, &coupure).await {
        Ok(lien) => lien,
        Err(id) => {
            connection.close(quinn::VarInt::from_u32(1), b"cle revoquee");
            return Err(TransportError::Refuse(format!("clé révoquée : {id}")));
        }
    };

    // 3. Boucle des messages : attente du flux (interruptible par la coupure),
    // cadrage, porte, verdict rendu tel quel.
    loop {
        let flux = tokio::select! {
            biased;
            () = coupure.notified() => {
                // Révocation de cette clé : seule cette session tombe.
                connection.close(quinn::VarInt::from_u32(1), b"cle revoquee");
                return Ok(());
            }
            flux = connection.accept_bi() => flux,
        };
        let (mut send, mut recv) = match flux {
            Ok(stream) => stream,
            // Fermeture propre ou erreur : fin de la connexion.
            Err(_) => break,
        };

        // Clé pas encore admise à l'ouverture de la session : on la relit ici.
        if lien.is_none() {
            lien = match resoudre(&gate, &vk, &coupure).await {
                Ok(lien) => lien,
                Err(id) => {
                    connection.close(quinn::VarInt::from_u32(1), b"cle revoquee");
                    return Err(TransportError::Refuse(format!("clé révoquée : {id}")));
                }
            };
        }

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

        // Le verrou de la porte ne vit que dans ce bloc : il est relâché avant
        // tout appel au répondeur.
        let (expediteur, verdict) = match &lien {
            Some((id, _)) => {
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

        let mut ack = ack_du_verdict(&verdict);
        let refuse = matches!(verdict, Verdict::Refuse(_));
        // Le message admis à passer au répondeur (seulement `Admis`).
        let appel = match (&service, &verdict) {
            (Some(service), Verdict::Admis(m)) => Some((service.clone(), m.clone())),
            _ => None,
        };
        let _ = tx.send((expediteur.clone(), verdict));
        if let Some((service, message)) = appel {
            let attente =
                tokio::time::timeout(service.delai, (service.repondeur)(expediteur, message));
            ack.reponse = tokio::select! {
                biased;
                () = coupure.notified() => {
                    // Révoqué pendant l'appel : la session tombe, sans accusé.
                    connection.close(quinn::VarInt::from_u32(1), b"cle revoquee");
                    return Ok(());
                }
                reponse = attente => reponse.ok().flatten().map(plafonner_reponse),
            };
        }
        let ack_bytes = serde_json::to_vec(&ack).expect("verdict serialise");
        if cadrage::ecrire(&mut send, &ack_bytes).await.is_ok() {
            let _ = send.finish();
        }
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

/// Une réponse plus grosse que [`PLAFOND_REPONSE`] (JSON sérialisé) est remplacée
/// par `{"erreur":"reponse_trop_grosse"}`.
fn plafonner_reponse(reponse: Value) -> Value {
    match serde_json::to_vec(&reponse) {
        Ok(octets) if octets.len() <= PLAFOND_REPONSE => reponse,
        _ => json!({"erreur": "reponse_trop_grosse"}),
    }
}

/// Résout l'identifiant de la clé du canal et arme la coupure de sa session.
///
/// - `Ok(None)` : clé inconnue (la porte refusera les messages).
/// - `Ok(Some((id, surveillance)))` : clé active ; tant que `surveillance` vit,
///   une révocation de `id` réveille `coupure`.
/// - `Err(id)` : clé connue mais révoquée.
///
/// L'abonnement est pris **avant** le contrôle d'état : une révocation survenue
/// entre les deux ne peut pas passer inaperçue.
async fn resoudre(
    gate: &Mutex<Gate>,
    vk: &VerifyingKey,
    coupure: &Arc<Notify>,
) -> Result<Option<(DeviceId, Surveillance)>, DeviceId> {
    let gate = gate.lock().await;
    let peers = gate.peers();
    let Some(id) = peers.id_de_cle(vk) else {
        return Ok(None);
    };
    let reveil = coupure.clone();
    let surveillance = peers.surveiller_revocation(&id, move || reveil.notify_one());
    if peers.state(&id) != Some(AnchorKeyState::Active) {
        return Err(id);
    }
    Ok(Some((id, surveillance)))
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

/// L'ALPN négocié par la poignée de main de `connection`, s'il y en a un.
fn alpn_negocie(connection: &quinn::Connection) -> Option<Vec<u8>> {
    connection
        .handshake_data()?
        .downcast::<quinn::crypto::rustls::HandshakeData>()
        .ok()?
        .protocol
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
    /// La réponse du répondeur de service, pour un message admis.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    reponse: Option<Value>,
}

fn ack_du_verdict(v: &Verdict) -> Ack {
    match v {
        Verdict::Admis(_) => Ack {
            v: "admis".into(),
            id: None,
            type_: None,
            raison: None,
            reponse: None,
        },
        Verdict::DejaVu(id) => Ack {
            v: "deja-vu".into(),
            id: Some(id.to_string()),
            type_: None,
            raison: None,
            reponse: None,
        },
        Verdict::Ignore(t) => Ack {
            v: "ignore".into(),
            id: None,
            type_: Some(t.clone()),
            raison: None,
            reponse: None,
        },
        Verdict::Refuse(r) => Ack {
            v: "refuse".into(),
            id: None,
            type_: None,
            raison: Some(r.to_string()),
            reponse: None,
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
        let peers = Peers::new();
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
        let peers = registre_alice();
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
    // ---- Registre partagé et coupure ciblée (même listener, jamais redémarré) ----

    fn serveur_id() -> Identite {
        identite(3, "device-cccccccc")
    }
    fn epingle_serveur() -> Epingle {
        epingle(3)
    }
    const ALICE: &str = "device-aaaaaaaa";
    const BOB: &str = "device-bbbbbbbb";

    /// Registre à deux pairs : alice (clé [1]) et bob (clé [2]), tous deux Active.
    fn registre_ab() -> Peers {
        let peers = Peers::new();
        peers.admit(
            &dev(ALICE),
            cle(1).verifying_key(),
            AnchorKeyRole::Shared,
            Utc::now(),
        );
        peers.admit(
            &dev(BOB),
            cle(2).verifying_key(),
            AnchorKeyRole::Shared,
            Utc::now(),
        );
        peers
    }

    /// Session cliente gardée ouverte : l'endpoint doit vivre avec la connexion.
    async fn session(
        adresse: SocketAddr,
        id: &Identite,
        epingle: &Epingle,
    ) -> (quinn::Endpoint, quinn::Connection) {
        let mut endpoint = quinn::Endpoint::client("0.0.0.0:0".parse().unwrap()).unwrap();
        endpoint.set_default_client_config(
            tls::quic_client(tls::config_client(id, epingle).unwrap()).unwrap(),
        );
        let connection = endpoint
            .connect(adresse, "this-mesh.local")
            .unwrap()
            .await
            .unwrap();
        (endpoint, connection)
    }

    /// Un message sur une session déjà ouverte ; rend le `v` du verdict.
    async fn envoyer_sur(connection: &quinn::Connection, m: &Message) -> String {
        let (mut send, mut recv) = connection.open_bi().await.unwrap();
        cadrage::ecrire(&mut send, &m.to_bytes()).await.unwrap();
        send.finish().unwrap();
        let octets = cadrage::lire(&mut recv, ACK_CEILING).await.unwrap();
        serde_json::from_slice::<Ack>(&octets).unwrap().v
    }

    // (a) Un pair ajouté PENDANT que le listener tourne est admis, sans redémarrer.
    #[tokio::test]
    async fn a_pair_ajoute_pendant_l_ecoute_est_admis_sans_redemarrer() {
        let peers = registre_alice();
        let serveur = ecouter(
            "127.0.0.1:0".parse().unwrap(),
            &serveur_id(),
            peers.clone(),
            Limits::default(),
        )
        .await
        .unwrap();
        let adresse = serveur.local_addr();

        // Bob n'est pas encore admis : la porte le refuse.
        let avant = message(&cle(2), BOB, ALICE, 20);
        let refus = envoyer(adresse, &identite(2, BOB), &epingle_serveur(), &avant).await;
        assert!(matches!(refus, Err(TransportError::Refuse(_))));

        // Admission par un autre clone du même registre, listener en marche.
        peers.admit(
            &dev(BOB),
            cle(2).verifying_key(),
            AnchorKeyRole::Shared,
            Utc::now(),
        );
        let m = message(&cle(2), BOB, ALICE, 21);
        let verdict = envoyer(adresse, &identite(2, BOB), &epingle_serveur(), &m).await;
        assert_eq!(verdict.unwrap(), Verdict::Admis(m));
        assert_eq!(
            serveur.local_addr(),
            adresse,
            "même listener, jamais rouvert"
        );
    }

    // (b) Révoquer A coupe SA session, laisse celle de B ouverte, et B reste admis.
    #[tokio::test]
    async fn b_revoquer_a_coupe_sa_session_et_laisse_celle_de_b() {
        let peers = registre_ab();
        let serveur = ecouter(
            "127.0.0.1:0".parse().unwrap(),
            &serveur_id(),
            peers.clone(),
            Limits::default(),
        )
        .await
        .unwrap();
        let adresse = serveur.local_addr();
        let (_ea, conn_a) = session(adresse, &identite(1, ALICE), &epingle_serveur()).await;
        let (_eb, conn_b) = session(adresse, &identite(2, BOB), &epingle_serveur()).await;

        assert_eq!(
            envoyer_sur(&conn_a, &message(&cle(1), ALICE, BOB, 30)).await,
            "admis"
        );
        assert_eq!(
            envoyer_sur(&conn_b, &message(&cle(2), BOB, ALICE, 31)).await,
            "admis"
        );

        assert!(peers.revoke(&dev(ALICE)));

        // Session de A : fermée par le serveur. Session de B : toujours ouverte.
        assert!(
            avec_delai(DELAI, conn_a.closed()).await.is_some(),
            "la session de A aurait dû tomber"
        );
        assert!(
            conn_b.close_reason().is_none(),
            "la session de B aurait dû rester ouverte"
        );
        assert_eq!(
            envoyer_sur(&conn_b, &message(&cle(2), BOB, ALICE, 32)).await,
            "admis"
        );

        // A, revenu par une nouvelle connexion, est refusé.
        let revenu = message(&cle(1), ALICE, BOB, 33);
        assert!(
            envoyer(adresse, &identite(1, ALICE), &epingle_serveur(), &revenu)
                .await
                .is_err()
        );
        assert_eq!(serveur.local_addr(), adresse);
    }

    // (c) Après la révocation de A, la mémoire anti-rejeu de B est intacte.
    #[tokio::test]
    async fn c_apres_revocation_la_memoire_anti_rejeu_est_intacte() {
        let peers = registre_ab();
        let serveur = ecouter(
            "127.0.0.1:0".parse().unwrap(),
            &serveur_id(),
            peers.clone(),
            Limits::default(),
        )
        .await
        .unwrap();
        let (_eb, conn_b) =
            session(serveur.local_addr(), &identite(2, BOB), &epingle_serveur()).await;
        let m = message(&cle(2), BOB, ALICE, 40);
        assert_eq!(envoyer_sur(&conn_b, &m).await, "admis");

        assert!(peers.revoke(&dev(ALICE)));

        // Le même `id`, rejoué par B après la révocation de A : acquitté, pas rejoué.
        assert_eq!(envoyer_sur(&conn_b, &m).await, "deja-vu");
    }

    // (e) Une clé jamais admise reste refusée, même sur une session ouverte.
    #[tokio::test]
    async fn e_cle_inconnue_reste_refusee_sur_une_session_ouverte() {
        let peers = registre_ab();
        let serveur = ecouter(
            "127.0.0.1:0".parse().unwrap(),
            &serveur_id(),
            peers,
            Limits::default(),
        )
        .await
        .unwrap();
        let (_ex, conn_x) = session(
            serveur.local_addr(),
            &identite(9, "device-99999999"),
            &epingle_serveur(),
        )
        .await;
        let mx = message(&cle(9), "device-99999999", ALICE, 50);
        assert_eq!(envoyer_sur(&conn_x, &mx).await, "refuse");
    }

    // ---- Appel de service : un pair répond dans l'accusé ----

    use std::sync::atomic::{AtomicUsize, Ordering};

    /// Un répondeur qui compte ses appels et rend `f(expéditeur, message)`.
    fn compteur_repondeur(
        compteur: Arc<AtomicUsize>,
        f: fn(&DeviceId, &Message) -> Option<Value>,
    ) -> Repondeur {
        Arc::new(move |de, m| {
            compteur.fetch_add(1, Ordering::SeqCst);
            let r = f(&de, &m);
            Box::pin(async move { r })
        })
    }

    /// Un écho : `{"de", "charge"}`.
    fn echo(de: &DeviceId, m: &Message) -> Option<Value> {
        Some(json!({"de": de.as_str(), "charge": m.charge}))
    }

    /// Serveur `ecouter_complet` (clé [3]) avec alice et bob admis.
    async fn serveur_complet(repondeur: Repondeur) -> Flux {
        ecouter_complet(
            "127.0.0.1:0".parse().unwrap(),
            &serveur_id(),
            registre_ab(),
            Limits::default(),
            AnchorKeyRole::Shared,
            DELAI,
            repondeur,
        )
        .await
        .unwrap()
        .0
    }

    fn appel(k: &SigningKey, de: &str, id: u128) -> Message {
        Message::signed(
            k,
            Uuid::from_u128(id),
            dev(de),
            dev("device-cccccccc"),
            MessageType::ServiceAppel,
            1,
            json!({"service":"echo","x":1}),
        )
        .unwrap()
    }

    #[test]
    fn les_types_de_service_ont_leur_nom_sur_le_fil() {
        assert_eq!(MessageType::ServiceAppel.wire(), "service.appel");
        assert_eq!(MessageType::ServiceReponse.wire(), "service.reponse");
        assert_eq!(
            MessageType::from_wire("service.appel"),
            Some(MessageType::ServiceAppel)
        );
        assert_eq!(
            MessageType::from_wire("service.reponse"),
            Some(MessageType::ServiceReponse)
        );
        assert!(MessageType::ALL.contains(&MessageType::ServiceAppel));
        assert!(MessageType::ALL.contains(&MessageType::ServiceReponse));
        // L'accusé sans réponse garde exactement sa forme d'avant.
        let ack = serde_json::to_string(&ack_du_verdict(&Verdict::DejaVu(Uuid::nil()))).unwrap();
        assert!(!ack.contains("reponse"), "{ack}");
    }

    // Un `service.appel` admis reçoit la réponse du répondeur ; le verdict part
    // aussi dans le flux.
    #[tokio::test]
    async fn s1_service_appel_admis_recoit_la_reponse() {
        let compteur = Arc::new(AtomicUsize::new(0));
        let mut serveur = serveur_complet(compteur_repondeur(compteur.clone(), echo)).await;
        let m = appel(&cle(1), ALICE, 60);
        let (verdict, reponse) = appeler(
            serveur.local_addr(),
            &identite(1, ALICE),
            &epingle_serveur(),
            &m,
            DELAI,
        )
        .await
        .unwrap();
        assert_eq!(verdict, Verdict::Admis(m.clone()));
        assert_eq!(
            reponse,
            Some(json!({"de": ALICE, "charge": {"service":"echo","x":1}}))
        );
        assert_eq!(compteur.load(Ordering::SeqCst), 1);
        let (de, recu) = avec_delai(DELAI, serveur.prochain())
            .await
            .flatten()
            .unwrap();
        assert_eq!(de, dev(ALICE));
        assert_eq!(recu, Verdict::Admis(m));
    }

    // Une clé inconnue : refusée par la porte, le répondeur n'est jamais appelé,
    // aucune réponse.
    #[tokio::test]
    async fn s2_cle_inconnue_n_appelle_jamais_le_repondeur() {
        let compteur = Arc::new(AtomicUsize::new(0));
        let mut serveur = serveur_complet(compteur_repondeur(compteur.clone(), echo)).await;
        let m = appel(&cle(9), "device-99999999", 61);
        let resultat = appeler(
            serveur.local_addr(),
            &identite(9, "device-99999999"),
            &epingle_serveur(),
            &m,
            DELAI,
        )
        .await;
        assert!(
            matches!(resultat, Err(TransportError::Refuse(_))),
            "{resultat:?}"
        );
        let (_, recu) = avec_delai(DELAI, serveur.prochain())
            .await
            .flatten()
            .unwrap();
        assert_eq!(
            recu,
            Verdict::Refuse(Raison::CleInconnue("device-99999999".into()))
        );
        assert_eq!(compteur.load(Ordering::SeqCst), 0);
    }

    // Un rejeu (même id) : déjà-vu, sans réponse, et le répondeur n'est appelé
    // qu'une fois.
    #[tokio::test]
    async fn s3_rejeu_n_appelle_pas_deux_fois_le_repondeur() {
        let compteur = Arc::new(AtomicUsize::new(0));
        let serveur = serveur_complet(compteur_repondeur(compteur.clone(), echo)).await;
        let m = appel(&cle(1), ALICE, 62);
        let cible = (serveur.local_addr(), identite(1, ALICE), epingle_serveur());
        let (v1, r1) = appeler(cible.0, &cible.1, &cible.2, &m, DELAI)
            .await
            .unwrap();
        assert_eq!(v1, Verdict::Admis(m.clone()));
        assert!(r1.is_some());
        let (v2, r2) = appeler(cible.0, &cible.1, &cible.2, &m, DELAI)
            .await
            .unwrap();
        assert_eq!(v2, Verdict::DejaVu(Uuid::from_u128(62)));
        assert_eq!(r2, None);
        assert_eq!(compteur.load(Ordering::SeqCst), 1);
    }

    // Une réponse au-delà de `PLAFOND_REPONSE` est remplacée par une erreur.
    #[tokio::test]
    async fn s4_reponse_trop_grosse_est_remplacee() {
        let compteur = Arc::new(AtomicUsize::new(0));
        let serveur = serveur_complet(compteur_repondeur(compteur.clone(), |_, _| {
            Some(json!("x".repeat(PLAFOND_REPONSE)))
        }))
        .await;
        let m = appel(&cle(1), ALICE, 63);
        let (verdict, reponse) = appeler(
            serveur.local_addr(),
            &identite(1, ALICE),
            &epingle_serveur(),
            &m,
            DELAI,
        )
        .await
        .unwrap();
        assert_eq!(verdict, Verdict::Admis(m));
        assert_eq!(reponse, Some(json!({"erreur": "reponse_trop_grosse"})));
        // Juste sous le plafond : la réponse passe intacte (`appeler` la lit).
        assert_eq!(
            plafonner_reponse(json!("x".repeat(PLAFOND_REPONSE - 2))),
            json!("x".repeat(PLAFOND_REPONSE - 2))
        );
    }

    // Un répondeur qui dépasse son délai : l'accusé part sans réponse, en temps borné.
    #[tokio::test]
    async fn s5_repondeur_en_retard_l_accuse_arrive_sans_reponse() {
        let lent: Repondeur = Arc::new(|_, _| {
            Box::pin(async {
                tokio::time::sleep(Duration::from_secs(30)).await;
                Some(json!("trop tard"))
            })
        });
        let (serveur, _rx) = ecouter_complet_avec_delai(
            "127.0.0.1:0".parse().unwrap(),
            &serveur_id(),
            registre_ab(),
            Limits::default(),
            AnchorKeyRole::Shared,
            DELAI,
            lent,
            Duration::from_millis(200),
        )
        .await
        .unwrap();
        let m = appel(&cle(1), ALICE, 64);
        let debut = std::time::Instant::now();
        let (verdict, reponse) = appeler(
            serveur.local_addr(),
            &identite(1, ALICE),
            &epingle_serveur(),
            &m,
            DELAI,
        )
        .await
        .unwrap();
        assert_eq!(verdict, Verdict::Admis(m));
        assert_eq!(reponse, None);
        assert!(
            debut.elapsed() < Duration::from_secs(3),
            "{:?}",
            debut.elapsed()
        );
    }

    // `appeler` borne toute l'opération : un pair qui ne répond pas à temps donne
    // `Connexion("délai dépassé")`.
    #[tokio::test]
    async fn s6_appeler_rend_delai_depasse() {
        let lent: Repondeur = Arc::new(|_, _| {
            Box::pin(async {
                tokio::time::sleep(Duration::from_secs(30)).await;
                None
            })
        });
        let serveur = serveur_complet(lent).await;
        let m = appel(&cle(1), ALICE, 65);
        let resultat = appeler(
            serveur.local_addr(),
            &identite(1, ALICE),
            &epingle_serveur(),
            &m,
            Duration::from_millis(500),
        )
        .await;
        match resultat {
            Err(TransportError::Connexion(raison)) => assert_eq!(raison, "délai dépassé"),
            autre => panic!("attendu délai dépassé, reçu {autre:?}"),
        }
    }

    // `envoyer` vers `ecouter_complet` marche toujours : verdict rendu, réponse
    // ignorée, flux alimenté.
    #[tokio::test]
    async fn s7_envoyer_vers_ecouter_complet_marche_toujours() {
        let compteur = Arc::new(AtomicUsize::new(0));
        let mut serveur = serveur_complet(compteur_repondeur(compteur.clone(), echo)).await;
        let m = message(&cle(2), BOB, ALICE, 66);
        let verdict = envoyer(
            serveur.local_addr(),
            &identite(2, BOB),
            &epingle_serveur(),
            &m,
        )
        .await
        .unwrap();
        assert_eq!(verdict, Verdict::Admis(m.clone()));
        assert_eq!(compteur.load(Ordering::SeqCst), 1);
        let (de, recu) = avec_delai(DELAI, serveur.prochain())
            .await
            .flatten()
            .unwrap();
        assert_eq!(de, dev(BOB));
        assert_eq!(recu, Verdict::Admis(m));
    }
}
