//! Appairage par le réseau : mène deux appareils qui ne se connaissent pas
//! jusqu'à une [`Admission`], **sans rien écrire** avant les deux confirmations.
//!
//! # Déroulé (façon Bluetooth SSP « Numeric Comparison »)
//!
//! Sur une connexion QUIC négociée en [`ALPN_APPAIRAGE`], un seul flux
//! bidirectionnel, des messages cadrés en petit JSON (plafond [`PLAFOND`]) :
//!
//! ```text
//! I (client)                                  R (serveur)
//!   tire nI ; engage h = SHA256(D ‖ nI)  ──▶
//!                                        ◀──  tire nR ; nonce nR   (après l'engagement)
//!   revele nI                            ──▶  vérifie h, sinon abandon
//!   code = sas(moi, pair, nI ‖ nR)            code = sas(moi, pair, nI ‖ nR)
//!   confirme | refuse                    ◀─▶  confirme | refuse
//! ```
//!
//! `D` est [`ENGAGEMENT_DOMAIN`]. L'engagement fige nI avant que R ne choisisse
//! nR, et R ne révèle rien qui dépende de nI : aucun des deux ne peut orienter le
//! code. Les deux clés du SAS sont **celles du canal** (la mienne, et celle du
//! certificat TLS du pair, dont la poignée de main a prouvé la détention) ; aucune
//! clé n'est jamais lue dans un message. Un homme du milieu tient deux canaux, donc
//! deux paires de clés différentes, donc (sauf 1 chance sur 10⁶) deux codes
//! différents : l'humain qui compare les codes le démasque.
//!
//! L'[`Admission`] n'est rendue que si la décision locale ET celle du pair sont
//! « oui », dans le délai. Le transport n'écrit jamais au registre : l'appelant
//! inscrit l'admission ([`Admission::inscrire`]).

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use ed25519_dalek::VerifyingKey;
use rustls::crypto::CryptoProvider;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use this_vfs::layout::{AnchorKeyRole, AnchorKeyState};
use tokio::sync::mpsc;
use tokio::time::{Instant, timeout_at};

use super::{ALPN_APPAIRAGE, CadrageError, Identite, TransportError, alpn_negocie, cadrage, tls};
use crate::envelope::{from_hex, to_hex};
use crate::{Admission, DeviceId, Peers, Rencontre, SasAttendu};

/// Préfixe de domaine de l'engagement : `SHA256(ENGAGEMENT_DOMAIN ‖ nI)`.
pub const ENGAGEMENT_DOMAIN: &[u8] = b"this-mesh/pair/commit/v1";

/// Délai global par défaut d'un appairage, de la connexion à la décision du pair.
pub const DELAI_APPAIRAGE: Duration = Duration::from_secs(120);

/// Nombre de demandes entrantes en attente de décision : au-delà, fermées.
pub const CAPACITE_DEMANDES: usize = 4;

/// Plafond d'un message d'appairage, vérifié avant allocation.
pub(super) const PLAFOND: usize = 1024;

/// Taille de chaque demi-nonce (nI, nR).
const TAILLE_NONCE: usize = 32;

/// Codes de fermeture QUIC de l'appairage (lisibles dans une trace réseau).
const CODE_APPAIRE: u32 = 0;
const CODE_REFUS: u32 = 10;
const CODE_ABANDON: u32 = 11;
const CODE_PROTOCOLE: u32 = 12;
const CODE_REVOQUEE: u32 = 13;
const CODE_FILE_PLEINE: u32 = 14;

/// Un message de l'appairage sur le fil : `{"t":"engage","h":…}`, etc.
#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "t", rename_all = "lowercase")]
pub(super) enum Echange {
    /// I → R : engagement sur nI (hex du SHA-256).
    Engage { h: String },
    /// R → I : nR en hex, envoyé seulement après l'engagement.
    Nonce { n: String },
    /// I → R : nI en hex, à comparer à l'engagement.
    Revele { n: String },
    /// Décision humaine : oui.
    Confirme,
    /// Décision humaine : non.
    Refuse,
}

/// L'engagement sur un demi-nonce : `SHA256(ENGAGEMENT_DOMAIN ‖ n)`.
pub(super) fn engagement(n: &[u8]) -> [u8; 32] {
    let mut h = Sha256::new();
    h.update(ENGAGEMENT_DOMAIN);
    h.update(n);
    h.finalize().into()
}

/// Un appairage dont le code est affiché, en attente de la décision locale.
///
/// Lâché sans [`AppairageEnCours::decider`], il ferme sa connexion : le pair voit
/// un abandon et rien n'est écrit d'aucun côté.
pub struct AppairageEnCours {
    sas: SasAttendu,
    pair_id: DeviceId,
    pair_cle: VerifyingKey,
    role: AnchorKeyRole,
    connexion: quinn::Connection,
    send: quinn::SendStream,
    recv: quinn::RecvStream,
    echeance: Instant,
    /// Côté répondeur : le registre, pour revérifier la révocation à la décision.
    peers: Option<Peers>,
    /// Côté initiateur : l'endpoint client doit vivre avec la connexion.
    _endpoint: Option<quinn::Endpoint>,
    termine: bool,
}

impl std::fmt::Debug for AppairageEnCours {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AppairageEnCours")
            .field("code", &self.sas.code())
            .field("pair", &self.pair_id)
            .field("role", &self.role)
            .finish_non_exhaustive()
    }
}

impl AppairageEnCours {
    /// Le code court à comparer avec celui qu'affiche le pair.
    pub fn code(&self) -> &str {
        self.sas.code()
    }

    /// L'identifiant du pair : celui que le registre lui connaît déjà (ré-appairage
    /// côté répondeur), sinon dérivé de sa clé TLS ([`DeviceId::de_cle`]).
    pub fn pair(&self) -> &DeviceId {
        &self.pair_id
    }

    /// La clé publique du pair, extraite de son certificat TLS (jamais d'un message).
    pub fn cle_pair(&self) -> VerifyingKey {
        self.pair_cle
    }

    /// Envoie la décision locale, attend celle du pair. `Ok(Admission)` seulement
    /// si les deux disent oui avant l'échéance ; un refus (local ou du pair), un
    /// abandon ou le délai dépassé rendent une erreur, et rien n'est écrit.
    pub async fn decider(mut self, accepte: bool) -> Result<Admission, TransportError> {
        let echeance = self.echeance;
        let mot = if accepte {
            Echange::Confirme
        } else {
            Echange::Refuse
        };
        let envoi = ecrire(&mut self.send, &mot, echeance).await;
        let _ = self.send.finish();

        if !accepte {
            // On laisse partir le refus (attente bornée de l'acquittement) avant
            // de fermer : le pair lit « refuse » plutôt qu'une coupure.
            let borne = echeance.min(Instant::now() + Duration::from_secs(2));
            let _ = timeout_at(borne, self.send.stopped()).await;
            self.fermer(CODE_REFUS, b"refus");
            return Err(TransportError::Refuse("appairage refusé localement".into()));
        }
        if let Err(e) = envoi {
            self.fermer(CODE_ABANDON, b"abandon");
            return Err(abandon_du_pair(e));
        }

        match lire(&mut self.recv, echeance).await {
            Ok(Echange::Confirme) => {}
            Ok(Echange::Refuse) => {
                self.fermer(CODE_REFUS, b"refus");
                return Err(TransportError::Refuse(
                    "le pair a refusé l'appairage".into(),
                ));
            }
            Ok(autre) => {
                self.fermer(CODE_PROTOCOLE, b"protocole");
                return Err(TransportError::Connexion(format!(
                    "appairage : décision attendue, reçu {autre:?}"
                )));
            }
            Err(e) => {
                self.fermer(CODE_ABANDON, b"abandon");
                return Err(abandon_du_pair(e));
            }
        }

        // Notre confirmation doit être reçue avant la fermeture : attente bornée
        // de son acquittement (le pair a peut-être déjà fermé, c'est sans effet).
        let _ = timeout_at(echeance, self.send.stopped()).await;

        // Côté répondeur : une révocation survenue pendant l'appairage l'emporte.
        if let Some(peers) = &self.peers
            && let Some(id) = peers.id_de_cle(&self.pair_cle)
            && peers.state(&id) == Some(AnchorKeyState::Revoked)
        {
            self.fermer(CODE_REVOQUEE, b"cle revoquee");
            return Err(TransportError::Refuse(format!("clé révoquée : {id}")));
        }

        self.sas.confirmer_local();
        self.sas.confirmer_pair();
        let mut admission = self
            .sas
            .appairer()
            .ok_or_else(|| TransportError::Refuse("appairage incomplet".into()))?;
        admission.role = self.role;
        self.fermer(CODE_APPAIRE, b"appaire");
        Ok(admission)
    }

    fn fermer(&mut self, code: u32, raison: &[u8]) {
        self.termine = true;
        self.connexion.close(quinn::VarInt::from_u32(code), raison);
    }
}

impl Drop for AppairageEnCours {
    fn drop(&mut self) {
        // Lâché sans décision : le pair voit un abandon, rien n'est écrit.
        if !self.termine {
            self.connexion
                .close(quinn::VarInt::from_u32(CODE_ABANDON), b"abandon");
        }
    }
}

/// Côté initiateur : se connecte à `adresse` en ALPN d'appairage, mène
/// l'engagement-révélation et rend l'appairage dont le code est affiché.
///
/// `role` est le rôle que j'attribue au pair (porté par l'[`Admission`]) ;
/// `delai` borne tout l'appairage, décision du pair comprise.
pub async fn appairer_vers(
    adresse: SocketAddr,
    identite: &Identite,
    role: AnchorKeyRole,
    delai: Duration,
) -> Result<AppairageEnCours, TransportError> {
    let echeance = Instant::now() + delai;
    let locale: SocketAddr = if adresse.is_ipv6() {
        "[::]:0".parse().expect("adresse valide")
    } else {
        "0.0.0.0:0".parse().expect("adresse valide")
    };
    let mut endpoint =
        quinn::Endpoint::client(locale).map_err(|e| TransportError::Connexion(e.to_string()))?;
    endpoint.set_default_client_config(tls::quic_client(tls::config_client_appairage(identite)?)?);

    let connexion = timeout_at(echeance, async {
        endpoint
            .connect(adresse, "this-mesh.local")
            .map_err(|e| TransportError::Connexion(e.to_string()))?
            .await
            .map_err(|e| TransportError::Connexion(e.to_string()))
    })
    .await
    .map_err(|_| delai_depasse())??;

    // Défense en profondeur : rustls l'impose déjà (seul ALPN offert).
    if alpn_negocie(&connexion).as_deref() != Some(ALPN_APPAIRAGE) {
        connexion.close(quinn::VarInt::from_u32(CODE_PROTOCOLE), b"alpn");
        return Err(TransportError::Connexion(
            "appairage : ALPN d'appairage non négocié".into(),
        ));
    }

    let moi = cle_locale(identite)?;
    let pair_cle = cle_verifiante(&connexion)?;
    if pair_cle == moi {
        connexion.close(quinn::VarInt::from_u32(CODE_PROTOCOLE), b"soi-meme");
        return Err(TransportError::Refuse("appairage avec soi-même".into()));
    }

    let (mut send, mut recv) = timeout_at(echeance, connexion.open_bi())
        .await
        .map_err(|_| delai_depasse())?
        .map_err(|e| TransportError::Connexion(e.to_string()))?;

    // 1. Engagement sur nI.
    let n_i = tls::aleatoire_32(&identite.provider())?;
    let h = to_hex(&engagement(&n_i));
    ecrire(&mut send, &Echange::Engage { h }, echeance).await?;

    // 2. nR, choisi par R sans connaître nI.
    let n_r = match lire(&mut recv, echeance).await? {
        Echange::Nonce { n } => nonce_de_hex(&n)?,
        autre => {
            connexion.close(quinn::VarInt::from_u32(CODE_PROTOCOLE), b"protocole");
            return Err(TransportError::Connexion(format!(
                "appairage : nonce attendu, reçu {autre:?}"
            )));
        }
    };

    // 3. Révélation de nI.
    ecrire(&mut send, &Echange::Revele { n: to_hex(&n_i) }, echeance).await?;

    let pair_id = DeviceId::de_cle(&pair_cle);
    Ok(en_cours(
        moi,
        pair_id,
        pair_cle,
        nonce_sas(&n_i, &n_r),
        role,
        Canal {
            connexion,
            send,
            recv,
        },
        echeance,
        None,
        Some(endpoint),
    ))
}

/// Ce que le serveur sait pour répondre aux demandes d'appairage.
pub(super) struct Repondeur {
    moi: VerifyingKey,
    provider: Arc<CryptoProvider>,
    peers: Peers,
    role: AnchorKeyRole,
    delai: Duration,
    tx: mpsc::Sender<AppairageEnCours>,
}

impl Repondeur {
    pub(super) fn nouveau(
        identite: &Identite,
        peers: Peers,
        role: AnchorKeyRole,
        delai: Duration,
        tx: mpsc::Sender<AppairageEnCours>,
    ) -> Result<Self, TransportError> {
        Ok(Self {
            moi: cle_locale(identite)?,
            provider: identite.provider(),
            peers,
            role,
            delai,
            tx,
        })
    }
}

/// Côté répondeur : mène une connexion d'appairage déjà négociée jusqu'au code,
/// puis la remonte à l'appelant. Ne touche ni la porte ni le flux des messages.
pub(super) async fn repondre(
    connexion: quinn::Connection,
    r: &Repondeur,
) -> Result<(), TransportError> {
    let echeance = Instant::now() + r.delai;

    let pair_cle = cle_verifiante(&connexion)?;
    if pair_cle == r.moi {
        connexion.close(quinn::VarInt::from_u32(CODE_PROTOCOLE), b"soi-meme");
        return Err(TransportError::Refuse("appairage avec soi-même".into()));
    }
    // Clé révoquée : refusée d'office, jamais remontée. Clé active : ré-appairage
    // sous le même identifiant (l'inscription mettra à jour le rôle).
    let pair_id = match r.peers.id_de_cle(&pair_cle) {
        Some(id) if r.peers.state(&id) != Some(AnchorKeyState::Active) => {
            connexion.close(quinn::VarInt::from_u32(CODE_REVOQUEE), b"cle revoquee");
            return Err(TransportError::Refuse(format!("clé révoquée : {id}")));
        }
        Some(id) => id,
        None => DeviceId::de_cle(&pair_cle),
    };

    let (mut send, mut recv) = match timeout_at(echeance, connexion.accept_bi()).await {
        Ok(Ok(flux)) => flux,
        Ok(Err(e)) => return Err(TransportError::Connexion(e.to_string())),
        Err(_) => {
            connexion.close(quinn::VarInt::from_u32(CODE_ABANDON), b"delai");
            return Err(delai_depasse());
        }
    };

    let resultat = async {
        // 1. Engagement de I, avant tout nonce de notre part.
        let h = match lire(&mut recv, echeance).await? {
            Echange::Engage { h } => nonce_de_hex(&h)?,
            autre => {
                return Err(TransportError::Connexion(format!(
                    "appairage : engagement attendu, reçu {autre:?}"
                )));
            }
        };
        // 2. Notre nonce.
        let n_r = tls::aleatoire_32(&r.provider)?;
        ecrire(&mut send, &Echange::Nonce { n: to_hex(&n_r) }, echeance).await?;
        // 3. Révélation : doit correspondre à l'engagement.
        let n_i = match lire(&mut recv, echeance).await? {
            Echange::Revele { n } => nonce_de_hex(&n)?,
            autre => {
                return Err(TransportError::Connexion(format!(
                    "appairage : révélation attendue, reçu {autre:?}"
                )));
            }
        };
        if engagement(&n_i) != h {
            return Err(TransportError::Refuse(
                "appairage : la révélation ne correspond pas à l'engagement".into(),
            ));
        }
        Ok((n_i, n_r))
    }
    .await;

    let (n_i, n_r) = match resultat {
        Ok(n) => n,
        Err(e) => {
            connexion.close(quinn::VarInt::from_u32(CODE_PROTOCOLE), b"abandon");
            return Err(e);
        }
    };

    let demande = en_cours(
        r.moi,
        pair_id,
        pair_cle,
        nonce_sas(&n_i, &n_r),
        r.role,
        Canal {
            connexion,
            send,
            recv,
        },
        echeance,
        Some(r.peers.clone()),
        None,
    );
    // File bornée : une demande qui ne tient pas est fermée (par son `Drop`).
    match r.tx.try_send(demande) {
        Ok(()) => Ok(()),
        Err(e) => {
            let mut demande = e.into_inner();
            demande.fermer(CODE_FILE_PLEINE, b"file pleine");
            Err(TransportError::Refuse(
                "appairage : trop de demandes en attente".into(),
            ))
        }
    }
}

/// La connexion d'appairage et son flux unique.
struct Canal {
    connexion: quinn::Connection,
    send: quinn::SendStream,
    recv: quinn::RecvStream,
}

#[allow(clippy::too_many_arguments)]
fn en_cours(
    moi: VerifyingKey,
    pair_id: DeviceId,
    pair_cle: VerifyingKey,
    nonce: Vec<u8>,
    role: AnchorKeyRole,
    canal: Canal,
    echeance: Instant,
    peers: Option<Peers>,
    endpoint: Option<quinn::Endpoint>,
) -> AppairageEnCours {
    // Les fonctions pures de l'appairage dérivent le code : même calcul des deux côtés.
    let sas = Rencontre::nouvelle(moi, pair_id.clone(), pair_cle, nonce).afficher();
    AppairageEnCours {
        sas,
        pair_id,
        pair_cle,
        role,
        connexion: canal.connexion,
        send: canal.send,
        recv: canal.recv,
        echeance,
        peers,
        _endpoint: endpoint,
        termine: false,
    }
}

/// Le nonce du SAS : `nI ‖ nR`, dans cet ordre des deux côtés.
fn nonce_sas(n_i: &[u8; TAILLE_NONCE], n_r: &[u8; TAILLE_NONCE]) -> Vec<u8> {
    let mut nonce = Vec::with_capacity(2 * TAILLE_NONCE);
    nonce.extend_from_slice(n_i);
    nonce.extend_from_slice(n_r);
    nonce
}

fn cle_locale(identite: &Identite) -> Result<VerifyingKey, TransportError> {
    VerifyingKey::from_bytes(&identite.cle_publique())
        .map_err(|e| TransportError::Identite(e.to_string()))
}

fn cle_verifiante(connexion: &quinn::Connection) -> Result<VerifyingKey, TransportError> {
    let cle = super::cle_du_pair(connexion)?;
    VerifyingKey::from_bytes(&cle).map_err(|e| TransportError::Identite(e.to_string()))
}

fn nonce_de_hex(s: &str) -> Result<[u8; TAILLE_NONCE], TransportError> {
    from_hex(s)
        .and_then(|v| <[u8; TAILLE_NONCE]>::try_from(v.as_slice()).ok())
        .ok_or_else(|| TransportError::Connexion("appairage : valeur hexadécimale invalide".into()))
}

fn delai_depasse() -> TransportError {
    TransportError::Refuse("appairage : délai dépassé".into())
}

/// Une coupure pendant l'attente de la décision du pair est un abandon.
fn abandon_du_pair(e: TransportError) -> TransportError {
    match e {
        TransportError::Cadrage(CadrageError::Io(io)) => {
            TransportError::Refuse(format!("appairage abandonné par le pair : {io}"))
        }
        autre => autre,
    }
}

pub(super) async fn ecrire(
    send: &mut quinn::SendStream,
    m: &Echange,
    echeance: Instant,
) -> Result<(), TransportError> {
    let octets = serde_json::to_vec(m).expect("message d'appairage sérialisable");
    timeout_at(echeance, cadrage::ecrire(send, &octets))
        .await
        .map_err(|_| delai_depasse())?
        .map_err(|e| TransportError::Cadrage(CadrageError::Io(e)))
}

pub(super) async fn lire(
    recv: &mut quinn::RecvStream,
    echeance: Instant,
) -> Result<Echange, TransportError> {
    let octets = timeout_at(echeance, cadrage::lire(recv, PLAFOND))
        .await
        .map_err(|_| delai_depasse())??;
    serde_json::from_slice(&octets)
        .map_err(|e| TransportError::Connexion(format!("appairage : message illisible : {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transport::{Epingle, ecouter, ecouter_avec_appairage, envoyer};
    use crate::{Limits, Message, MessageType, Raison, Verdict};
    use chrono::Utc;
    use ed25519_dalek::SigningKey;
    use serde_json::json;
    use uuid::Uuid;

    const DELAI: Duration = Duration::from_secs(5);
    /// Attente « rien n'arrive » : assez longue pour qu'une demande eût remonté.
    const SILENCE: Duration = Duration::from_millis(600);

    fn cle(n: u8) -> SigningKey {
        SigningKey::from_bytes(&[n; 32])
    }
    fn vk(n: u8) -> VerifyingKey {
        cle(n).verifying_key()
    }
    fn id(n: u8) -> DeviceId {
        DeviceId::de_cle(&vk(n))
    }
    /// Identité dont l'identifiant est dérivé de la clé, comme dans l'application.
    fn identite(n: u8) -> Identite {
        Identite::de_cle(&cle(n), &id(n)).unwrap()
    }
    fn local() -> SocketAddr {
        "127.0.0.1:0".parse().unwrap()
    }
    fn message(n: u8, a: u8, numero: u128) -> Message {
        Message::signed(
            &cle(n),
            Uuid::from_u128(numero),
            id(n),
            id(a),
            MessageType::EspaceOp,
            1,
            json!({"liste":"courses","op":"add"}),
        )
        .unwrap()
    }

    /// R (clé [1]) écoute avec l'appairage, registre `peers`.
    async fn repondeur(
        peers: Peers,
        delai: Duration,
    ) -> (super::super::Flux, mpsc::Receiver<AppairageEnCours>) {
        ecouter_avec_appairage(
            local(),
            &identite(1),
            peers,
            Limits::default(),
            AnchorKeyRole::Shared,
            delai,
        )
        .await
        .unwrap()
    }

    /// I (clé [2]) lance l'appairage ; rend les deux côtés, codes affichés.
    async fn deux_cotes(
        adresse: SocketAddr,
        rx: &mut mpsc::Receiver<AppairageEnCours>,
        delai: Duration,
    ) -> (AppairageEnCours, AppairageEnCours) {
        let i = appairer_vers(adresse, &identite(2), AnchorKeyRole::Shared, delai)
            .await
            .expect("appairage lancé");
        let r = tokio::time::timeout(DELAI, rx.recv())
            .await
            .expect("aucune demande remontée")
            .expect("récepteur fermé");
        (i, r)
    }

    async fn aucune_demande(rx: &mut mpsc::Receiver<AppairageEnCours>) -> bool {
        tokio::time::timeout(SILENCE, rx.recv()).await.is_err()
    }

    // (1) Les deux disent oui : même code, chacun admet la clé de L'AUTRE, et
    // après inscription un envoi normal (`this-mesh/1`) de I vers R est admis.
    #[tokio::test]
    async fn p1_les_deux_disent_oui_puis_l_envoi_normal_est_admis() {
        let peers = Peers::new();
        let (mut flux, mut rx) = repondeur(peers.clone(), DELAI).await;
        let adresse = flux.local_addr();
        let (i, r) = deux_cotes(adresse, &mut rx, DELAI).await;

        assert_eq!(i.code(), r.code(), "les deux côtés affichent le même code");
        assert_eq!(i.code().len(), 6);
        assert_eq!(i.cle_pair(), vk(1));
        assert_eq!(r.cle_pair(), vk(2));
        assert_eq!(i.pair(), &id(1));
        assert_eq!(r.pair(), &id(2));
        assert!(peers.is_empty(), "rien d'écrit avant les décisions");

        let (ai, ar) = tokio::join!(i.decider(true), r.decider(true));
        let (ai, ar) = (ai.unwrap(), ar.unwrap());
        assert_eq!(ai.key, vk(1));
        assert_eq!(ai.id, id(1));
        assert_eq!(ar.key, vk(2));
        assert_eq!(ar.id, id(2));
        assert!(peers.is_empty(), "le transport n'écrit pas au registre");

        ar.inscrire(&peers);
        let m = message(2, 1, 100);
        let verdict = envoyer(adresse, &identite(2), &Epingle::cle(&ai.key), &m).await;
        assert_eq!(verdict.unwrap(), Verdict::Admis(m.clone()));
        let (de, recu) = tokio::time::timeout(DELAI, flux.prochain())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(de, id(2));
        assert_eq!(recu, Verdict::Admis(m));
    }

    async fn refus(i_accepte: bool, r_accepte: bool) {
        let peers = Peers::new();
        let (mut flux, mut rx) = repondeur(peers.clone(), DELAI).await;
        let adresse = flux.local_addr();
        let (i, r) = deux_cotes(adresse, &mut rx, DELAI).await;

        let (ai, ar) = tokio::join!(i.decider(i_accepte), r.decider(r_accepte));
        assert!(matches!(ai, Err(TransportError::Refuse(_))), "I : {ai:?}");
        assert!(matches!(ar, Err(TransportError::Refuse(_))), "R : {ar:?}");
        assert!(peers.is_empty());

        let m = message(2, 1, 200);
        let resultat = envoyer(adresse, &identite(2), &Epingle::cle(&vk(1)), &m).await;
        assert!(matches!(resultat, Err(TransportError::Refuse(_))));
        let (_, recu) = tokio::time::timeout(DELAI, flux.prochain())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            recu,
            Verdict::Refuse(Raison::CleInconnue(id(2).to_string()))
        );
    }

    // (2) R refuse : erreur des deux côtés, l'envoi reste `CleInconnue`.
    #[tokio::test]
    async fn p2_r_refuse_aucune_admission() {
        refus(true, false).await;
    }

    // (3) I refuse : symétrique.
    #[tokio::test]
    async fn p3_i_refuse_aucune_admission() {
        refus(false, true).await;
    }

    // (4) Un client qui triche (révèle autre chose que ce qu'il a engagé) : R
    // ferme la connexion et ne remonte aucune demande.
    #[tokio::test]
    async fn p4_revelation_ne_correspondant_pas_a_l_engagement_fait_abandonner_r() {
        let (flux, mut rx) = repondeur(Peers::new(), DELAI).await;
        let echeance = Instant::now() + DELAI;

        let mut endpoint = quinn::Endpoint::client("0.0.0.0:0".parse().unwrap()).unwrap();
        endpoint.set_default_client_config(
            tls::quic_client(tls::config_client_appairage(&identite(2)).unwrap()).unwrap(),
        );
        let connexion = endpoint
            .connect(flux.local_addr(), "this-mesh.local")
            .unwrap()
            .await
            .unwrap();
        let (mut send, mut recv) = connexion.open_bi().await.unwrap();
        let engage = [7u8; 32];
        let revele = [8u8; 32];
        ecrire(
            &mut send,
            &Echange::Engage {
                h: to_hex(&engagement(&engage)),
            },
            echeance,
        )
        .await
        .unwrap();
        assert!(matches!(
            lire(&mut recv, echeance).await.unwrap(),
            Echange::Nonce { .. }
        ));
        ecrire(&mut send, &Echange::Revele { n: to_hex(&revele) }, echeance)
            .await
            .unwrap();

        // R abandonne : la connexion est fermée avec le code de protocole.
        let raison = tokio::time::timeout(DELAI, connexion.closed())
            .await
            .expect("R aurait dû fermer");
        match raison {
            quinn::ConnectionError::ApplicationClosed(c) => {
                assert_eq!(c.error_code, quinn::VarInt::from_u32(CODE_PROTOCOLE))
            }
            autre => panic!("fermeture inattendue : {autre:?}"),
        }
        assert!(lire(&mut recv, echeance).await.is_err());
        assert!(aucune_demande(&mut rx).await);
    }

    // (5) Délai dépassé : un côté ne décide jamais, l'autre échoue en temps borné.
    #[tokio::test]
    async fn p5_delai_depasse_l_autre_cote_echoue_en_temps_borne() {
        let court = Duration::from_millis(300);
        let (flux, mut rx) = repondeur(Peers::new(), court).await;

        // R ne décide jamais : I échoue.
        let (i, r) = deux_cotes(flux.local_addr(), &mut rx, court).await;
        let debut = std::time::Instant::now();
        let ai = tokio::time::timeout(DELAI, i.decider(true))
            .await
            .expect("I aurait dû échouer avant 5 s");
        assert!(ai.is_err(), "{ai:?}");
        assert!(
            debut.elapsed() < Duration::from_secs(2),
            "{:?}",
            debut.elapsed()
        );
        drop(r);

        // I ne décide jamais : R échoue.
        let (i, r) = deux_cotes(flux.local_addr(), &mut rx, court).await;
        let debut = std::time::Instant::now();
        let ar = tokio::time::timeout(DELAI, r.decider(true))
            .await
            .expect("R aurait dû échouer avant 5 s");
        assert!(ar.is_err(), "{ar:?}");
        assert!(
            debut.elapsed() < Duration::from_secs(2),
            "{:?}",
            debut.elapsed()
        );
        drop(i);
    }

    // (6) Une clé révoquée qui demande l'appairage : aucune demande remontée.
    #[tokio::test]
    async fn p6_cle_revoquee_ne_remonte_aucune_demande() {
        let peers = Peers::new();
        peers.admit(&id(2), vk(2), AnchorKeyRole::Shared, Utc::now());
        assert!(peers.revoke(&id(2)));
        let (flux, mut rx) = repondeur(peers.clone(), DELAI).await;

        let resultat = appairer_vers(
            flux.local_addr(),
            &identite(2),
            AnchorKeyRole::Shared,
            DELAI,
        )
        .await;
        assert!(resultat.is_err(), "{resultat:?}");
        assert!(aucune_demande(&mut rx).await);
        assert_eq!(peers.state(&id(2)), Some(AnchorKeyState::Revoked));
    }

    // (7) Une connexion `this-mesh/1` d'une clé inconnue ne crée jamais de demande :
    // elle va à la porte, qui la refuse.
    #[tokio::test]
    async fn p7_connexion_normale_inconnue_ne_cree_pas_de_demande() {
        let (mut flux, mut rx) = repondeur(Peers::new(), DELAI).await;
        let m = message(2, 1, 700);
        let resultat = envoyer(flux.local_addr(), &identite(2), &Epingle::cle(&vk(1)), &m).await;
        assert!(matches!(resultat, Err(TransportError::Refuse(_))));
        let (_, recu) = tokio::time::timeout(DELAI, flux.prochain())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            recu,
            Verdict::Refuse(Raison::CleInconnue(id(2).to_string()))
        );
        assert!(aucune_demande(&mut rx).await);
    }

    // (8) Le vérificateur d'appairage ne sert pas à `this-mesh/1` : par
    // construction, `config_client_appairage` n'offre QUE l'ALPN d'appairage et
    // `VerifierAppairage` n'est construit nulle part ailleurs (voir sa doc). On
    // vérifie (a) qu'un client d'appairage n'aboutit pas sur un serveur qui ne
    // parle que `this-mesh/1`, sans rien produire dans le flux, et (b) que le
    // client normal garde son épinglage face à un serveur qui accepte l'appairage.
    #[tokio::test]
    async fn p8_le_verificateur_d_appairage_ne_sert_pas_a_this_mesh_1() {
        let mut normal = ecouter(local(), &identite(1), Peers::new(), Limits::default())
            .await
            .unwrap();
        let resultat = appairer_vers(
            normal.local_addr(),
            &identite(2),
            AnchorKeyRole::Shared,
            DELAI,
        )
        .await;
        assert!(resultat.is_err(), "{resultat:?}");
        assert!(
            tokio::time::timeout(SILENCE, normal.prochain())
                .await
                .is_err()
        );

        let (mut flux, mut rx) = repondeur(Peers::new(), DELAI).await;
        let m = message(2, 1, 800);
        let mauvaise = Epingle::cle(&vk(9));
        let resultat = envoyer(flux.local_addr(), &identite(2), &mauvaise, &m).await;
        assert!(resultat.is_err(), "l'épinglage aurait dû échouer");
        assert!(
            tokio::time::timeout(SILENCE, flux.prochain())
                .await
                .is_err()
        );
        assert!(aucune_demande(&mut rx).await);
    }

    // (9) Deux appairages successifs : nonces frais, codes différents.
    #[tokio::test]
    async fn p9_deux_appairages_successifs_ont_des_codes_differents() {
        let (flux, mut rx) = repondeur(Peers::new(), DELAI).await;
        let (i1, r1) = deux_cotes(flux.local_addr(), &mut rx, DELAI).await;
        let (i2, r2) = deux_cotes(flux.local_addr(), &mut rx, DELAI).await;
        assert_eq!(i1.code(), r1.code());
        assert_eq!(i2.code(), r2.code());
        // Probabilité de collision 10⁻⁶ : acceptable pour un test.
        assert_ne!(i1.code(), i2.code());
    }

    // Lâcher l'appairage sans décider : le pair voit un abandon, rien n'est écrit.
    #[tokio::test]
    async fn lacher_sans_decider_fait_echouer_le_pair() {
        let peers = Peers::new();
        let (flux, mut rx) = repondeur(peers.clone(), DELAI).await;
        let (i, r) = deux_cotes(flux.local_addr(), &mut rx, DELAI).await;
        drop(r);
        let ai = tokio::time::timeout(DELAI, i.decider(true)).await.unwrap();
        assert!(matches!(ai, Err(TransportError::Refuse(_))), "{ai:?}");
        assert!(peers.is_empty());
    }

    // File bornée : au-delà de `CAPACITE_DEMANDES` demandes non lues, la suivante
    // est fermée, pas mise en file.
    #[tokio::test]
    async fn une_demande_au_dela_de_la_capacite_est_fermee() {
        let (flux, mut rx) = repondeur(Peers::new(), DELAI).await;
        let mut gardes = Vec::new();
        for _ in 0..CAPACITE_DEMANDES {
            let i = appairer_vers(
                flux.local_addr(),
                &identite(2),
                AnchorKeyRole::Shared,
                DELAI,
            )
            .await
            .unwrap();
            gardes.push(i);
        }
        let surplus = appairer_vers(
            flux.local_addr(),
            &identite(2),
            AnchorKeyRole::Shared,
            DELAI,
        )
        .await
        .unwrap();
        let resultat = tokio::time::timeout(DELAI, surplus.decider(true))
            .await
            .unwrap();
        assert!(resultat.is_err(), "{resultat:?}");
        let mut remontees = 0;
        while let Ok(Some(_)) = tokio::time::timeout(SILENCE, rx.recv()).await {
            remontees += 1;
        }
        assert_eq!(remontees, CAPACITE_DEMANDES);
    }

    // Ré-appairage d'une clé active : même identifiant, rôle mis à jour à l'inscription.
    #[tokio::test]
    async fn re_appairage_d_une_cle_active_garde_l_identifiant() {
        let peers = Peers::new();
        let ancien = DeviceId::parse("device-aaaaaaaa").unwrap();
        peers.admit(&ancien, vk(2), AnchorKeyRole::Shared, Utc::now());
        let (flux, mut rx) = ecouter_avec_appairage(
            local(),
            &identite(1),
            peers.clone(),
            Limits::default(),
            AnchorKeyRole::Ownership,
            DELAI,
        )
        .await
        .unwrap();
        let (i, r) = deux_cotes(flux.local_addr(), &mut rx, DELAI).await;
        assert_eq!(r.pair(), &ancien);
        let (ai, ar) = tokio::join!(i.decider(true), r.decider(true));
        ai.unwrap();
        let ar = ar.unwrap();
        assert_eq!(ar.role, AnchorKeyRole::Ownership);
        ar.inscrire(&peers);
        assert_eq!(peers.len(), 1);
        assert_eq!(peers.role(&ancien), Some(AnchorKeyRole::Ownership));
    }

    #[test]
    fn les_messages_ont_la_forme_attendue_sur_le_fil() {
        let s = |m: &Echange| serde_json::to_string(m).unwrap();
        assert_eq!(s(&Echange::Confirme), r#"{"t":"confirme"}"#);
        assert_eq!(s(&Echange::Refuse), r#"{"t":"refuse"}"#);
        assert_eq!(
            s(&Echange::Engage { h: "ab".into() }),
            r#"{"t":"engage","h":"ab"}"#
        );
        assert_eq!(
            s(&Echange::Nonce { n: "cd".into() }),
            r#"{"t":"nonce","n":"cd"}"#
        );
        assert_eq!(
            s(&Echange::Revele { n: "ef".into() }),
            r#"{"t":"revele","n":"ef"}"#
        );
    }

    #[test]
    fn l_identifiant_derive_reprend_les_quatre_premiers_octets() {
        let k = vk(1);
        let attendu = format!(
            "device-{:02x}{:02x}{:02x}{:02x}",
            k.as_bytes()[0],
            k.as_bytes()[1],
            k.as_bytes()[2],
            k.as_bytes()[3]
        );
        assert_eq!(DeviceId::de_cle(&k).as_str(), attendu);
        assert!(DeviceId::parse(&attendu).is_some());
    }
}
