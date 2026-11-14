//! Serveur HTTP minimal d'Atrium : expose le registre et la présence (logique pure).
//! Toute entrée invalide répond 400, jamais de panique.
//!
//! Durci pour un exposé sur Internet :
//! - écoute sur une IP choisie (`--bind`, défaut `127.0.0.1`) ;
//! - limite de débit par IP source (jeton, 30/min) ;
//! - bornes mémoire (comptes, appareils par compte) ;
//! - `/annuaire` authentifié (signature d'appelant) et restreint au même compte.

use ed25519_dalek::{Signature, VerifyingKey};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Instant;
use this_atrium::Etat as EtatAppareil;
use this_atrium::presence::Presence;
use this_atrium::{AccountId, DeviceId, Registre};
use tiny_http::{Header, Method, Response, Server};

/// Domaine des octets signés pour une requête authentifiée (`/annuaire`, futur `/relais`).
const DOMAINE_REQUETE: &[u8] = b"this-atrium/v1/req\n";

/// Fenêtre d'horloge acceptée pour l'horodatage d'un appelant, en millisecondes.
const FENETRE_HORLOGE_MS: u64 = 60_000;

/// Fenêtre de rejeu des nonces, en millisecondes (purgés au-delà).
const FENETRE_NONCE_MS: u64 = 60_000;

/// Limite de débit par IP : capacité 30 jetons, recharge 30/min (0,5/s).
const CAPACITE_JETONS: f64 = 30.0;
const RECHARGE_PAR_SEC: f64 = 0.5;
const MAX_IPS: usize = 10_000;

/// Borne de nonces mémorisés (anti-amplification de la carte de nonces).
const MAX_NONCES: usize = 100_000;

/// Bornes mémoire.
const MAX_COMPTES: usize = 100_000;
const MAX_APPAREILS: usize = 64;

/// Une réponse HTTP : `(statut, corps)`.
type Reponse = (u16, String);

/// La réponse 401 standard : ne porte jamais d'adresse.
fn non_authentifie() -> Reponse {
    (401, json!({ "erreur": "non authentifié" }).to_string())
}

/// Limite de débit par IP source : seau à jetons.
struct Limite {
    jetons: HashMap<String, (f64, Instant)>,
}

impl Limite {
    fn nouveau() -> Self {
        Self {
            jetons: HashMap::new(),
        }
    }

    /// Vrai si l'IP a encore un jeton (un jeton est consommé) ; faux sinon.
    fn autorise(&mut self, ip: &str) -> bool {
        let maintenant = Instant::now();
        // Borne la table : au-delà de ~MAX_IPS entrées, on retire la plus ancienne.
        if self.jetons.len() >= MAX_IPS
            && !self.jetons.contains_key(ip)
            && let Some(plus_vieux) = self
                .jetons
                .iter()
                .min_by_key(|(_, (_, dernier))| *dernier)
                .map(|(k, _)| k.clone())
        {
            self.jetons.remove(&plus_vieux);
        }
        let entree = self
            .jetons
            .entry(ip.to_string())
            .or_insert((CAPACITE_JETONS, maintenant));
        let ecoule = maintenant.duration_since(entree.1).as_secs_f64();
        entree.0 = (entree.0 + ecoule * RECHARGE_PAR_SEC).min(CAPACITE_JETONS);
        entree.1 = maintenant;
        if entree.0 >= 1.0 {
            entree.0 -= 1.0;
            true
        } else {
            false
        }
    }
}

/// Fenêtre de nonces déjà vus (anti-rejeu), bornée et purgée.
struct Nonces {
    vus: HashMap<String, u64>,
}

impl Nonces {
    fn nouveau() -> Self {
        Self {
            vus: HashMap::new(),
        }
    }

    /// Vrai si le nonce a déjà été vu dans la fenêtre ; sinon il est mémorisé.
    fn deja_vu(&mut self, nonce: &str, maintenant_ms: u64) -> bool {
        self.vus
            .retain(|_, t| maintenant_ms.saturating_sub(*t) <= FENETRE_NONCE_MS);
        if self.vus.contains_key(nonce) {
            return true;
        }
        if self.vus.len() >= MAX_NONCES
            && let Some(plus_vieux) = self
                .vus
                .iter()
                .min_by_key(|(_, t)| **t)
                .map(|(k, _)| k.clone())
        {
            self.vus.remove(&plus_vieux);
        }
        self.vus.insert(nonce.to_string(), maintenant_ms);
        false
    }
}

/// L'état partagé du serveur.
struct EtatServeur {
    registre: Registre,
    presence: Presence,
    limites: Limite,
    nonces: Nonces,
}

impl EtatServeur {
    fn nouveau() -> Self {
        Self {
            registre: Registre::nouveau(),
            presence: Presence::default(),
            limites: Limite::nouveau(),
            nonces: Nonces::nouveau(),
        }
    }
}

type Etat = Arc<Mutex<EtatServeur>>;

fn erreur(msg: impl std::fmt::Display) -> Reponse {
    (400, json!({ "erreur": msg.to_string() }).to_string())
}

fn cle_de(v: &Value, champ: &str) -> Result<VerifyingKey, String> {
    let s = v
        .get(champ)
        .and_then(Value::as_str)
        .ok_or(format!("champ « {champ} » manquant"))?;
    let octets: [u8; 32] = hex::decode(s)
        .map_err(|_| format!("« {champ} » n'est pas de l'hexadécimal"))?
        .try_into()
        .map_err(|_| format!("« {champ} » doit faire 32 octets"))?;
    VerifyingKey::from_bytes(&octets).map_err(|_| format!("« {champ} » n'est pas une clé valide"))
}

fn signature_de(v: &Value) -> Result<Vec<u8>, String> {
    let s = v
        .get("signature")
        .and_then(Value::as_str)
        .ok_or("champ « signature » manquant")?;
    hex::decode(s).map_err(|_| "« signature » n'est pas de l'hexadécimal".to_string())
}

fn texte(v: &Value, champ: &str) -> Result<String, String> {
    v.get(champ)
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or(format!("champ « {champ} » manquant"))
}

fn entier(v: &Value, champ: &str) -> Result<u64, String> {
    v.get(champ)
        .and_then(Value::as_u64)
        .ok_or(format!("champ « {champ} » manquant ou invalide"))
}

fn requete_param<'a>(requete: &'a str, nom: &str) -> Option<&'a str> {
    requete
        .split('&')
        .find_map(|p| p.strip_prefix(nom)?.strip_prefix('='))
}

fn corps_json(corps: &str) -> Result<Value, String> {
    serde_json::from_str(corps).map_err(|e| format!("JSON invalide : {e}"))
}

/// Horloge en millisecondes depuis l'époque Unix (0 si l'horloge est en défaut).
fn maintenant_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// La matière signée d'une requête authentifiée :
/// `DOMAINE_REQUETE || METHODE || "\n" || url_complete || "\n" || ts_decimal || "\n" || nonce`.
fn message_requete(methode: &str, url: &str, ts_decimal: &str, nonce_octets: &[u8]) -> Vec<u8> {
    let mut m = Vec::with_capacity(
        DOMAINE_REQUETE.len()
            + methode.len()
            + url.len()
            + ts_decimal.len()
            + nonce_octets.len()
            + 3,
    );
    m.extend_from_slice(DOMAINE_REQUETE);
    m.extend_from_slice(methode.as_bytes());
    m.extend_from_slice(b"\n");
    m.extend_from_slice(url.as_bytes());
    m.extend_from_slice(b"\n");
    m.extend_from_slice(ts_decimal.as_bytes());
    m.extend_from_slice(b"\n");
    m.extend_from_slice(nonce_octets);
    m
}

fn en_tete<'a>(en_tetes: &'a [(String, String)], nom: &str) -> Option<&'a str> {
    en_tetes
        .iter()
        .find(|(k, _)| k.as_str().eq_ignore_ascii_case(nom))
        .map(|(_, v)| v.as_str())
}

/// Authentifie l'appelant d'une requête (`/annuaire`, futur `/relais`) et rend son compte.
///
/// Vérifie, dans l'ordre : les quatre en-têtes requis, l'horloge (`|now - ts| <= 60 s`),
/// le nonce (jamais vu dans la fenêtre), l'appareil appelant **Actif** dans le registre, et
/// la signature Ed25519 de la matière `message_requete`. Tout échec → 401, jamais d'adresse.
fn verifier_appelant(
    etat: &mut EtatServeur,
    methode: &str,
    url: &str,
    en_tetes: &[(String, String)],
) -> Result<AccountId, Reponse> {
    let appareil = en_tete(en_tetes, "X-Atrium-Device").ok_or_else(non_authentifie)?;
    let ts_texte = en_tete(en_tetes, "X-Atrium-Ts").ok_or_else(non_authentifie)?;
    let nonce_texte = en_tete(en_tetes, "X-Atrium-Nonce").ok_or_else(non_authentifie)?;
    let sig_texte = en_tete(en_tetes, "X-Atrium-Sig").ok_or_else(non_authentifie)?;

    let ts: u64 = ts_texte.parse().map_err(|_| non_authentifie())?;
    let nonce_octets = hex::decode(nonce_texte).map_err(|_| non_authentifie())?;
    let sig_octets = hex::decode(sig_texte).map_err(|_| non_authentifie())?;
    if sig_octets.len() != 64 {
        return Err(non_authentifie());
    }

    let maintenant = maintenant_ms();
    if maintenant.abs_diff(ts) > FENETRE_HORLOGE_MS {
        return Err(non_authentifie());
    }
    if etat.nonces.deja_vu(nonce_texte, maintenant) {
        return Err(non_authentifie());
    }

    let appareil_id: DeviceId = serde_json::from_value(Value::String(appareil.to_string()))
        .map_err(|_| non_authentifie())?;

    let mut cle_appelant: Option<VerifyingKey> = None;
    let mut compte_appelant: Option<AccountId> = None;
    for (cid, compte) in &etat.registre.comptes {
        if let Some(ap) = compte.appareils.get(&appareil_id)
            && ap.etat == EtatAppareil::Actif
        {
            cle_appelant = Some(ap.cle);
            compte_appelant = Some(cid.clone());
            break;
        }
    }
    let cle = cle_appelant.ok_or_else(non_authentifie)?;
    let compte = compte_appelant.ok_or_else(non_authentifie)?;

    let message = message_requete(methode, url, &ts.to_string(), &nonce_octets);
    let signature = Signature::from_slice(&sig_octets).map_err(|_| non_authentifie())?;
    cle.verify_strict(&message, &signature)
        .map_err(|_| non_authentifie())?;

    Ok(compte)
}

/// Cœur testable : (méthode, chemin+requête, corps, IP, en-têtes) → (statut, corps).
fn repondre(
    etat: &Etat,
    methode: &str,
    url: &str,
    corps: &str,
    ip: &str,
    en_tetes: &[(String, String)],
) -> Reponse {
    let (chemin, requete) = url.split_once('?').unwrap_or((url, ""));
    let Ok(mut g) = etat.lock() else {
        return (500, json!({ "erreur": "état indisponible" }).to_string());
    };

    if methode == "GET" && chemin == "/sante" {
        return (200, "ok".into());
    }
    if !g.limites.autorise(ip) {
        return (429, json!({ "erreur": "trop de requêtes" }).to_string());
    }

    match (methode, chemin) {
        ("POST", "/compte") => {
            let cle = match corps_json(corps).and_then(|v| cle_de(&v, "cle")) {
                Ok(c) => c,
                Err(e) => return erreur(e),
            };
            let id = AccountId::de_cle(&cle);
            if !g.registre.deja_inscrit(&id) && g.registre.comptes.len() >= MAX_COMPTES {
                return (503, json!({ "erreur": "capacité atteinte" }).to_string());
            }
            match g.registre.inscrire(cle).map_err(|e| e.to_string()) {
                Ok(c) => (200, json!({ "compte": c.as_str() }).to_string()),
                Err(e) => erreur(e),
            }
        }
        ("POST", "/appareil") => {
            let (compte, cle, signature) = match corps_json(corps).and_then(|v| {
                let compte: AccountId = serde_json::from_value(Value::String(texte(&v, "compte")?))
                    .map_err(|e| e.to_string())?;
                Ok((compte, cle_de(&v, "cle")?, signature_de(&v)?))
            }) {
                Ok(t) => t,
                Err(e) => return erreur(e),
            };
            let id = DeviceId::de_cle(&cle);
            let (deja_present, nb) = match g.registre.comptes.get(&compte) {
                Some(c) => (c.appareils.contains_key(&id), c.appareils.len()),
                None => (false, 0),
            };
            if !deja_present && nb >= MAX_APPAREILS {
                return (503, json!({ "erreur": "capacité atteinte" }).to_string());
            }
            match g
                .registre
                .enregistrer(&compte, cle, &signature)
                .map_err(|e| e.to_string())
            {
                Ok(d) => (200, json!({ "appareil": d.as_str() }).to_string()),
                Err(e) => erreur(e),
            }
        }
        ("POST", "/presence") => {
            let (compte, appareil, adresse, duree_ms, ms, signature) = match corps_json(corps)
                .and_then(|v| {
                    let compte: AccountId =
                        serde_json::from_value(Value::String(texte(&v, "compte")?))
                            .map_err(|e| e.to_string())?;
                    let appareil: DeviceId =
                        serde_json::from_value(Value::String(texte(&v, "appareil")?))
                            .map_err(|e| e.to_string())?;
                    Ok((
                        compte,
                        appareil,
                        texte(&v, "adresse")?,
                        entier(&v, "duree_ms")?,
                        entier(&v, "maintenant_ms")?,
                        signature_de(&v)?,
                    ))
                }) {
                Ok(t) => t,
                Err(e) => return erreur(e),
            };
            let inner = &mut *g;
            match inner
                .presence
                .publier(
                    &inner.registre,
                    &compte,
                    &appareil,
                    &adresse,
                    duree_ms,
                    ms,
                    &signature,
                )
                .map_err(|e| e.to_string())
            {
                Ok(()) => (200, "{}".into()),
                Err(e) => erreur(e),
            }
        }
        ("GET", "/annuaire") => {
            let compte = match verifier_appelant(&mut g, "GET", url, en_tetes) {
                Ok(c) => c,
                Err(r) => return r,
            };
            let Some(appareil) = requete_param(requete, "appareil") else {
                return erreur("paramètre « appareil » manquant");
            };
            let Ok(appareil_id) =
                serde_json::from_value::<DeviceId>(Value::String(appareil.to_string()))
            else {
                return erreur("appareil invalide");
            };
            let Some(ms) = requete_param(requete, "maintenant_ms").and_then(|s| s.parse().ok())
            else {
                return erreur("paramètre « maintenant_ms » manquant ou invalide");
            };
            let appartient = g
                .registre
                .comptes
                .get(&compte)
                .map(|c| c.appareils.contains_key(&appareil_id))
                .unwrap_or(false);
            if !appartient {
                return (403, json!({ "erreur": "hors de votre compte" }).to_string());
            }
            match g.presence.joindre(&appareil_id, ms) {
                Some(a) => (200, json!({ "adresse": a }).to_string()),
                None => (404, "{}".into()),
            }
        }
        _ => erreur("requête inconnue"),
    }
}

fn servir(server: Server, etat: Etat, max: Option<usize>) {
    let json_h = Header::from_bytes("Content-Type", "application/json").expect("en-tête statique");
    for (n, mut req) in server.incoming_requests().enumerate() {
        let methode = match req.method() {
            Method::Get => "GET",
            Method::Post => "POST",
            _ => "AUTRE",
        };
        let url = req.url().to_string();
        let ip = req
            .remote_addr()
            .map(|a| a.ip().to_string())
            .unwrap_or_default();
        let en_tetes: Vec<(String, String)> = req
            .headers()
            .iter()
            .map(|h| {
                (
                    h.field.as_str().as_str().to_string(),
                    h.value.as_str().to_string(),
                )
            })
            .collect();
        let mut corps = String::new();
        if std::io::Read::read_to_string(req.as_reader(), &mut corps).is_err() {
            corps.clear();
        }
        let (statut, rep) = repondre(&etat, methode, &url, &corps, &ip, &en_tetes);
        eprintln!("{methode} {url} -> {statut}");
        let _ = req.respond(
            Response::from_string(rep)
                .with_status_code(statut)
                .with_header(json_h.clone()),
        );
        if max.is_some_and(|m| n + 1 >= m) {
            break;
        }
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let bind = args
        .iter()
        .position(|a| a == "--bind")
        .and_then(|i| args.get(i + 1))
        .cloned()
        .unwrap_or_else(|| "127.0.0.1".to_string());
    let port = args
        .iter()
        .position(|a| a == "--port")
        .and_then(|i| args.get(i + 1))
        .and_then(|p| p.parse::<u16>().ok())
        .unwrap_or(7443);
    let server = match Server::http(format!("{bind}:{port}")) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("impossible d'écouter sur {bind}:{port} : {e}");
            std::process::exit(1);
        }
    };
    eprintln!("atrium-server à l'écoute sur {bind}:{port}");
    servir(server, Arc::new(Mutex::new(EtatServeur::nouveau())), None);
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signer, SigningKey};
    use std::io::{Read, Write};

    fn etat_frais() -> Etat {
        Arc::new(Mutex::new(EtatServeur::nouveau()))
    }

    fn cle(n: u8) -> SigningKey {
        SigningKey::from_bytes(&[n; 32])
    }

    fn inscrire_compte(etat: &mut EtatServeur, cle_compte: &SigningKey) -> AccountId {
        etat.registre.inscrire(cle_compte.verifying_key()).unwrap()
    }

    fn enregistrer_appareil(
        etat: &mut EtatServeur,
        compte: &AccountId,
        cle_compte: &SigningKey,
        cle_appareil: &SigningKey,
    ) -> DeviceId {
        let mut message = Vec::new();
        message.extend_from_slice(this_atrium::DOMAINE_APPAREIL);
        message.extend_from_slice(&cle_appareil.verifying_key().to_bytes());
        let signature = cle_compte.sign(&message);
        etat.registre
            .enregistrer(compte, cle_appareil.verifying_key(), &signature.to_bytes())
            .unwrap()
    }

    fn signe_presence(
        cle_appareil: &SigningKey,
        adresse: &str,
        expire_ms: u64,
        appareil: &DeviceId,
    ) -> Vec<u8> {
        let mut message = Vec::new();
        message.extend_from_slice(this_atrium::presence::DOMAINE_PRESENCE);
        message.extend_from_slice(adresse.as_bytes());
        message.extend_from_slice(&expire_ms.to_be_bytes());
        message.extend_from_slice(appareil.as_str().as_bytes());
        cle_appareil.sign(&message).to_bytes().to_vec()
    }

    fn en_tetes_appelant(
        appareil: &DeviceId,
        ts: u64,
        nonce: &[u8],
        cle_appareil: &SigningKey,
        url: &str,
    ) -> Vec<(String, String)> {
        let message = message_requete("GET", url, &ts.to_string(), nonce);
        let signature = cle_appareil.sign(&message);
        vec![
            ("X-Atrium-Device".to_string(), appareil.as_str().to_string()),
            ("X-Atrium-Ts".to_string(), ts.to_string()),
            ("X-Atrium-Nonce".to_string(), hex::encode(nonce)),
            (
                "X-Atrium-Sig".to_string(),
                hex::encode(signature.to_bytes()),
            ),
        ]
    }

    #[test]
    fn sante_repond_ok() {
        let server = Server::http("127.0.0.1:47431").expect("port libre");
        let etat = etat_frais();
        let th = std::thread::spawn(move || servir(server, etat, Some(1)));
        let mut s = std::net::TcpStream::connect("127.0.0.1:47431").unwrap();
        s.set_read_timeout(Some(std::time::Duration::from_secs(5)))
            .unwrap();
        s.write_all(b"GET /sante HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n")
            .unwrap();
        let mut rep = String::new();
        let _ = s.read_to_string(&mut rep);
        th.join().unwrap();
        assert!(rep.contains("200"), "{rep}");
        assert!(rep.ends_with("ok"), "{rep}");
    }

    #[test]
    fn annuaire_sans_signature_401() {
        let etat = etat_frais();
        let (statut, _) = repondre(
            &etat,
            "GET",
            "/annuaire?appareil=device-x&maintenant_ms=0",
            "",
            "1.2.3.4",
            &[],
        );
        assert_eq!(statut, 401);
    }

    #[test]
    fn annuaire_meme_compte_ok() {
        let etat = etat_frais();
        let cle_compte = cle(7);
        let cle_a = cle(1);
        let cle_b = cle(2);

        let (a, b, maintenant) = {
            let mut g = etat.lock().unwrap();
            let compte = inscrire_compte(&mut g, &cle_compte);
            let a = enregistrer_appareil(&mut g, &compte, &cle_compte, &cle_a);
            let b = enregistrer_appareil(&mut g, &compte, &cle_compte, &cle_b);
            let maintenant = maintenant_ms();
            let duree = 60_000;
            let expire = maintenant + duree;
            let signature = signe_presence(&cle_b, "192.0.2.10:9000", expire, &b);
            let inner = &mut *g;
            inner
                .presence
                .publier(
                    &inner.registre,
                    &compte,
                    &b,
                    "192.0.2.10:9000",
                    duree,
                    maintenant,
                    &signature,
                )
                .unwrap();
            (a, b, maintenant)
        };

        let url = format!(
            "/annuaire?appareil={}&maintenant_ms={}",
            b.as_str(),
            maintenant
        );
        let ts = maintenant_ms();
        let en_tetes = en_tetes_appelant(&a, ts, &[9u8; 16], &cle_a, &url);
        let (statut, corps) = repondre(&etat, "GET", &url, "", "1.2.3.4", &en_tetes);
        assert_eq!(statut, 200, "corps : {corps}");
        assert!(corps.contains("192.0.2.10:9000"), "{corps}");
    }

    #[test]
    fn annuaire_autre_compte_403() {
        let etat = etat_frais();
        let cle_c1 = cle(7);
        let cle_c2 = cle(8);
        let cle_a1 = cle(1);
        let cle_a2 = cle(2);

        let (dev1, dev2) = {
            let mut g = etat.lock().unwrap();
            let compte1 = inscrire_compte(&mut g, &cle_c1);
            let compte2 = inscrire_compte(&mut g, &cle_c2);
            let dev1 = enregistrer_appareil(&mut g, &compte1, &cle_c1, &cle_a1);
            let dev2 = enregistrer_appareil(&mut g, &compte2, &cle_c2, &cle_a2);
            (dev1, dev2)
        };

        let url = format!("/annuaire?appareil={}&maintenant_ms=0", dev2.as_str());
        let ts = maintenant_ms();
        let en_tetes = en_tetes_appelant(&dev1, ts, &[1u8; 16], &cle_a1, &url);
        let (statut, _) = repondre(&etat, "GET", &url, "", "1.2.3.4", &en_tetes);
        assert_eq!(statut, 403);
    }

    #[test]
    fn signature_invalide_401() {
        let etat = etat_frais();
        let cle_c = cle(7);
        let cle_a = cle(1);

        let dev = {
            let mut g = etat.lock().unwrap();
            let compte = inscrire_compte(&mut g, &cle_c);
            enregistrer_appareil(&mut g, &compte, &cle_c, &cle_a)
        };

        let url = format!("/annuaire?appareil={}&maintenant_ms=0", dev.as_str());
        let ts = maintenant_ms();
        let mut en_tetes = en_tetes_appelant(&dev, ts, &[2u8; 16], &cle_a, &url);
        let sig = &mut en_tetes[3].1;
        let mut octets = hex::decode(sig.as_str()).unwrap();
        octets[0] ^= 0xff;
        *sig = hex::encode(octets);

        let (statut, _) = repondre(&etat, "GET", &url, "", "1.2.3.4", &en_tetes);
        assert_eq!(statut, 401);
    }

    #[test]
    fn limite_de_debit_429() {
        let etat = etat_frais();
        let mut limite_atteinte = false;
        for _ in 0..40 {
            let (statut, _) = repondre(&etat, "POST", "/compte", "{}", "9.9.9.9", &[]);
            if statut == 429 {
                limite_atteinte = true;
                break;
            }
            assert_eq!(statut, 400);
        }
        assert!(limite_atteinte, "la limite de débit n'a pas été atteinte");
    }

    #[test]
    fn borne_appareils_503() {
        let etat = etat_frais();
        let cle_compte = cle(7);
        let compte = {
            let mut g = etat.lock().unwrap();
            let compte = inscrire_compte(&mut g, &cle_compte);
            for i in 0..MAX_APPAREILS {
                let cle_appareil = SigningKey::from_bytes(&[i as u8; 32]);
                enregistrer_appareil(&mut g, &compte, &cle_compte, &cle_appareil);
            }
            compte
        };

        let cle_65 = SigningKey::from_bytes(&[200u8; 32]);
        let mut message = Vec::new();
        message.extend_from_slice(this_atrium::DOMAINE_APPAREIL);
        message.extend_from_slice(&cle_65.verifying_key().to_bytes());
        let signature = cle_compte.sign(&message);
        let corps = json!({
            "compte": compte.as_str(),
            "cle": hex::encode(cle_65.verifying_key().to_bytes()),
            "signature": hex::encode(signature.to_bytes()),
        })
        .to_string();

        let (statut, _) = repondre(&etat, "POST", "/appareil", &corps, "1.2.3.4", &[]);
        assert_eq!(statut, 503);
    }
}
