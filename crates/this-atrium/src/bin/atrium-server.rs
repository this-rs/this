//! Serveur HTTP minimal d'Atrium : expose le registre et la présence (logique pure).
//! Toute entrée invalide répond 400, jamais de panique.
//!
//! Durci pour un exposé sur Internet :
//! - écoute sur une IP choisie (`--bind`, défaut `127.0.0.1`) ;
//! - limite de débit par IP source (jeton, 30/min) ;
//! - bornes mémoire (comptes, appareils par compte, boîtes du relais) ;
//! - `/annuaire` authentifié (signature d'appelant), restreint au même compte ou à un
//!   compte ayant reçu une autorisation signée (`POST/DELETE /autoriser`, `GET /grants`) ;
//! - `/relais` : store-and-forward **opaque** — le serveur ne lit jamais les octets qu'il
//!   transporte, il les compte (quota horaire par compte émetteur) et les met en boîte.

use ed25519_dalek::{Signature, VerifyingKey};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::Instant;
use this_atrium::Etat as EtatAppareil;
use this_atrium::autorisations::{Autorisation, Autorisations, Portee};
use this_atrium::presence::Presence;
use this_atrium::relais::{Quota, Relais};
use this_atrium::{AccountId, DeviceId, Registre};
use tiny_http::{Header, Method, Response, Server};

/// Domaine des octets signés pour une requête authentifiée (`/annuaire`, `/relais`).
const DOMAINE_REQUETE: &[u8] = b"this-atrium/v1/req\n";

/// Domaine des octets signés d'un dépôt au relais : `DOMAINE_RELAIS || de \n a \n
/// sha256(octets) \n maintenant_ms`, signé par la clé de l'appareil `de`. À domaine séparé,
/// cette signature ne vaut ni pour un enregistrement d'appareil, ni pour une requête.
const DOMAINE_RELAIS: &[u8] = b"this-atrium/v1/relais\n";

/// Capacité d'autorisation qui ouvre le relais : le destinataire l'accorde à l'émetteur.
const CAPACITE_RELAIS: &str = "relais";

/// Quota du relais : 10 Mo par heure et par compte émetteur (fenêtre glissante).
const QUOTA_RELAIS: Quota = Quota {
    octets_par_heure: 10_000_000,
};

/// Borne d'une boîte : au plus 50 messages…
const MAX_BOITE_MESSAGES: usize = 50;
/// …et au plus 1 Mio en attente, par appareil destinataire.
const MAX_BOITE_OCTETS: usize = 1_048_576;

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
const MAX_AUTORISATIONS: usize = 10_000;

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
    autorisations: Autorisations,
    limites: Limite,
    nonces: Nonces,
    /// Les compteurs de quota du relais, par compte émetteur.
    relais: Relais,
    /// Les boîtes du relais, par appareil destinataire : les octets en attente, tels quels.
    /// Le serveur ne les interprète jamais — il ne fait que les garder.
    boites: BTreeMap<DeviceId, VecDeque<Vec<u8>>>,
}

impl EtatServeur {
    fn nouveau() -> Self {
        Self {
            registre: Registre::nouveau(),
            presence: Presence::default(),
            autorisations: Autorisations::default(),
            limites: Limite::nouveau(),
            nonces: Nonces::nouveau(),
            relais: Relais::default(),
            boites: BTreeMap::new(),
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

/// Décode le corps de `/autoriser` en [`Autorisation`] (signature en 128 hexadécimaux).
fn autorisation_de(v: &Value) -> Result<Autorisation, String> {
    let compte = |champ: &str| -> Result<AccountId, String> {
        serde_json::from_value(Value::String(texte(v, champ)?)).map_err(|e| e.to_string())
    };
    let portee: Portee = serde_json::from_value(
        v.get("portee")
            .cloned()
            .ok_or("champ « portee » manquant")?,
    )
    .map_err(|e| format!("« portee » invalide : {e}"))?;
    let expire_ms = match v.get("expire_ms") {
        None | Some(Value::Null) => None,
        Some(e) => Some(e.as_u64().ok_or("« expire_ms » invalide")?),
    };
    Ok(Autorisation {
        de: compte("de")?,
        a: compte("a")?,
        capacite: texte(v, "capacite")?,
        portee,
        cree_ms: entier(v, "cree_ms")?,
        expire_ms,
        signature: signature_de(v)?,
    })
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

/// La matière signée d'un dépôt au relais :
/// `DOMAINE_RELAIS || de || "\n" || a || "\n" || sha256(octets) (32 octets bruts) ||
/// "\n" || maintenant_ms`.
///
/// Les octets relayés n'entrent dans la signature que **hachés** : le relais hache sans lire,
/// et rien de leur contenu ne peut être inféré de la matière signée.
fn message_relais(de: &DeviceId, a: &DeviceId, octets: &[u8], maintenant: u64) -> Vec<u8> {
    let mut m =
        Vec::with_capacity(DOMAINE_RELAIS.len() + de.as_str().len() + a.as_str().len() + 56);
    m.extend_from_slice(DOMAINE_RELAIS);
    m.extend_from_slice(de.as_str().as_bytes());
    m.push(b'\n');
    m.extend_from_slice(a.as_str().as_bytes());
    m.push(b'\n');
    m.extend_from_slice(&Sha256::digest(octets));
    m.push(b'\n');
    m.extend_from_slice(maintenant.to_string().as_bytes());
    m
}

fn en_tete<'a>(en_tetes: &'a [(String, String)], nom: &str) -> Option<&'a str> {
    en_tetes
        .iter()
        .find(|(k, _)| k.as_str().eq_ignore_ascii_case(nom))
        .map(|(_, v)| v.as_str())
}

/// Authentifie l'appelant d'une requête (`/annuaire`, `/relais`, `/grants`) et rend son compte.
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
            let compte_cible = g
                .registre
                .comptes
                .iter()
                .find(|(_, c)| c.appareils.contains_key(&appareil_id))
                .map(|(id, _)| id.clone());
            let permis = match compte_cible {
                Some(cible) => {
                    cible == compte
                        || g.autorisations.est_autorise(
                            &cible,
                            &compte,
                            "annuaire",
                            &appareil_id,
                            maintenant_ms(),
                        )
                }
                None => false,
            };
            if !permis {
                return (403, json!({ "erreur": "hors de votre compte" }).to_string());
            }
            match g.presence.joindre(&appareil_id, ms) {
                Some(a) => (200, json!({ "adresse": a }).to_string()),
                None => (404, "{}".into()),
            }
        }
        ("GET", "/appareils") => {
            let compte = match verifier_appelant(&mut g, "GET", url, en_tetes) {
                Ok(c) => c,
                Err(r) => return r,
            };
            let Some(cible) = requete_param(requete, "compte") else {
                return erreur("paramètre « compte » manquant");
            };
            let Ok(compte_cible) =
                serde_json::from_value::<AccountId>(Value::String(cible.to_string()))
            else {
                return erreur("compte invalide");
            };
            let Some(ms) = requete_param(requete, "maintenant_ms").and_then(|s| s.parse().ok())
            else {
                return erreur("paramètre « maintenant_ms » manquant ou invalide");
            };

            // Les appareils encore Actifs du compte cible (les révoqués sont exclus).
            let appareils = match g.registre.annuaire(&compte_cible) {
                Ok(a) => a,
                Err(_) => return erreur("compte inconnu"),
            };

            // Même compte, ou une autorisation « annuaire » accordée par le compte cible à
            // l'appelant. Une portée `tous` couvre n'importe lequel des appareils, donc elle
            // suffit pour la liste entière ; une portée `Appareil(x)` ne couvre que l'appareil
            // `x` du compte. On teste donc chaque appareil : `tous` (ou l'un de ses appareils)
            // suffit, ce qui évite de déléguer la décision à un appareil « représentatif ».
            let permis = compte_cible == compte
                || appareils.iter().any(|(id, _)| {
                    g.autorisations
                        .est_autorise(&compte_cible, &compte, "annuaire", id, ms)
                });
            if !permis {
                return (403, json!({ "erreur": "hors de votre compte" }).to_string());
            }

            let liste: Vec<Value> = appareils
                .iter()
                .map(|(id, _)| json!({ "id": id, "adresse": g.presence.joindre(id, ms) }))
                .collect();
            (200, json!({ "appareils": liste }).to_string())
        }
        ("POST", "/relais") => {
            let (de, a, octets, ms, signature) = match corps_json(corps).and_then(|v| {
                let de: DeviceId = serde_json::from_value(Value::String(texte(&v, "de")?))
                    .map_err(|e| e.to_string())?;
                let a: DeviceId = serde_json::from_value(Value::String(texte(&v, "a")?))
                    .map_err(|e| e.to_string())?;
                let octets = hex::decode(texte(&v, "octets_hex")?)
                    .map_err(|_| "« octets_hex » n'est pas de l'hexadécimal".to_string())?;
                Ok((
                    de,
                    a,
                    octets,
                    entier(&v, "maintenant_ms")?,
                    signature_de(&v)?,
                ))
            }) {
                Ok(t) => t,
                Err(e) => return erreur(e),
            };

            // Une boîte ne peut de toute façon pas tenir plus que sa borne : refus avant
            // tout travail cryptographique sur un corps démesuré.
            if octets.len() > MAX_BOITE_OCTETS {
                return (503, json!({ "erreur": "message trop gros" }).to_string());
            }

            let maintenant = maintenant_ms();
            if maintenant.abs_diff(ms) > FENETRE_HORLOGE_MS {
                return non_authentifie();
            }

            // L'émetteur doit être un appareil **Actif** du registre.
            let mut cle_emetteur: Option<VerifyingKey> = None;
            let mut compte_emetteur: Option<AccountId> = None;
            for (cid, compte) in &g.registre.comptes {
                if let Some(ap) = compte.appareils.get(&de)
                    && ap.etat == EtatAppareil::Actif
                {
                    cle_emetteur = Some(ap.cle);
                    compte_emetteur = Some(cid.clone());
                    break;
                }
            }
            let (Some(cle), Some(compte_de)) = (cle_emetteur, compte_emetteur) else {
                return non_authentifie();
            };

            let Ok(signature) = Signature::from_slice(&signature) else {
                return erreur("signature du relais invalide");
            };
            if cle
                .verify_strict(&message_relais(&de, &a, &octets, ms), &signature)
                .is_err()
            {
                return erreur("signature du relais invalide");
            }

            // Le destinataire doit exister ; son compte est celui qui peut autoriser.
            let compte_a = g
                .registre
                .comptes
                .iter()
                .find_map(|(cid, c)| c.appareils.contains_key(&a).then(|| cid.clone()));
            let Some(compte_a) = compte_a else {
                return (403, json!({ "erreur": "non autorisé" }).to_string());
            };

            // Même compte, ou le destinataire a accordé à l'émetteur la capacité « relais ».
            let permis = compte_a == compte_de
                || g.autorisations.est_autorise(
                    &compte_a,
                    &compte_de,
                    CAPACITE_RELAIS,
                    &a,
                    maintenant,
                );
            if !permis {
                return (403, json!({ "erreur": "non autorisé" }).to_string());
            }

            // Les bornes de la boîte d'abord : un refus ne doit rien coûter au quota.
            let deja: usize = g
                .boites
                .get(&a)
                .map(|b| b.iter().map(Vec::len).sum())
                .unwrap_or(0);
            let pleine = g
                .boites
                .get(&a)
                .is_some_and(|b| b.len() >= MAX_BOITE_MESSAGES)
                || deja.saturating_add(octets.len()) > MAX_BOITE_OCTETS;
            if pleine {
                return (
                    503,
                    json!({ "erreur": "boîte du destinataire pleine" }).to_string(),
                );
            }

            if g.relais
                .transmettre(&compte_de, maintenant, octets.len(), &QUOTA_RELAIS)
                .is_err()
            {
                return (
                    429,
                    json!({ "erreur": "quota de relais dépassé" }).to_string(),
                );
            }

            g.boites.entry(a).or_default().push_back(octets);
            (200, "{}".into())
        }
        ("GET", "/relais") => {
            if let Err(r) = verifier_appelant(&mut g, "GET", url, en_tetes) {
                return r;
            }
            // L'appareil authentifié est le destinataire : il ne lit que SA boîte.
            let Some(appareil) = en_tete(en_tetes, "X-Atrium-Device") else {
                return non_authentifie();
            };
            let Ok(appareil) =
                serde_json::from_value::<DeviceId>(Value::String(appareil.to_string()))
            else {
                return erreur("appareil invalide");
            };
            let messages: Vec<String> = g
                .boites
                .remove(&appareil)
                .map(|b| b.into_iter().map(hex::encode).collect())
                .unwrap_or_default();
            if messages.is_empty() {
                return (204, String::new());
            }
            (200, json!({ "messages": messages }).to_string())
        }
        ("POST", "/autoriser") | ("DELETE", "/autoriser") => {
            let x = match corps_json(corps).and_then(|v| autorisation_de(&v)) {
                Ok(x) => x,
                Err(e) => return erreur(e),
            };
            let Some(cle_de) = g.registre.comptes.get(&x.de).map(|c| c.cle) else {
                return (403, json!({ "erreur": "compte inconnu" }).to_string());
            };
            if methode == "POST" {
                let deja = g.autorisations.octroyees.iter().any(|o| {
                    o.de == x.de && o.a == x.a && o.capacite == x.capacite && o.portee == x.portee
                });
                if !deja && g.autorisations.octroyees.len() >= MAX_AUTORISATIONS {
                    return (503, json!({ "erreur": "capacité atteinte" }).to_string());
                }
                match g.autorisations.octroyer(x, &cle_de) {
                    Ok(()) => (200, "{}".into()),
                    Err(e) => erreur(e),
                }
            } else {
                match g.autorisations.retirer(&x, &cle_de) {
                    Ok(()) => (200, "{}".into()),
                    Err(e) => erreur(e),
                }
            }
        }
        ("GET", "/grants") => {
            let compte = match verifier_appelant(&mut g, "GET", url, en_tetes) {
                Ok(c) => c,
                Err(r) => return r,
            };
            let vue = |x: &Autorisation| {
                json!({
                    "de": x.de, "a": x.a, "capacite": x.capacite, "portee": x.portee,
                    "cree_ms": x.cree_ms, "expire_ms": x.expire_ms,
                    "signature": hex::encode(&x.signature),
                })
            };
            let octroyees: Vec<Value> = g
                .autorisations
                .octroyees
                .iter()
                .filter(|x| x.de == compte)
                .map(vue)
                .collect();
            let recues: Vec<Value> = g
                .autorisations
                .octroyees
                .iter()
                .filter(|x| x.a == compte)
                .map(vue)
                .collect();
            (
                200,
                json!({ "octroyees": octroyees, "recues": recues }).to_string(),
            )
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
            Method::Delete => "DELETE",
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

    fn corps_autorisation(
        de: &SigningKey,
        a: &AccountId,
        portee: Portee,
        corrompre: bool,
    ) -> String {
        let mut x = Autorisation {
            de: AccountId::de_cle(&de.verifying_key()),
            a: a.clone(),
            capacite: "annuaire".into(),
            portee,
            cree_ms: 1,
            expire_ms: None,
            signature: vec![],
        };
        let mut sig = de.sign(&x.octets_signes()).to_bytes().to_vec();
        if corrompre {
            sig[0] ^= 0xff;
        }
        x.signature = sig;
        json!({
            "de": x.de, "a": x.a, "capacite": x.capacite, "portee": x.portee,
            "cree_ms": x.cree_ms, "expire_ms": null,
            "signature": hex::encode(&x.signature),
        })
        .to_string()
    }

    /// Compte A (a1 publie une adresse) et compte B (b1). Rend (etat, a1, b1, cle_a, cle_b1, compte_b, ms).
    fn scene() -> (
        Etat,
        DeviceId,
        DeviceId,
        SigningKey,
        SigningKey,
        AccountId,
        u64,
    ) {
        let etat = etat_frais();
        let (cle_a, cle_b) = (cle(7), cle(8));
        let (cle_a1, cle_a2, cle_b1) = (cle(1), cle(2), cle(3));
        let ms = maintenant_ms();
        let (a1, b1, compte_b) = {
            let mut g = etat.lock().unwrap();
            let ca = inscrire_compte(&mut g, &cle_a);
            let cb = inscrire_compte(&mut g, &cle_b);
            let a1 = enregistrer_appareil(&mut g, &ca, &cle_a, &cle_a1);
            enregistrer_appareil(&mut g, &ca, &cle_a, &cle_a2);
            let b1 = enregistrer_appareil(&mut g, &cb, &cle_b, &cle_b1);
            let sig = signe_presence(&cle_a1, "192.0.2.10:9000", ms + 60_000, &a1);
            let inner = &mut *g;
            inner
                .presence
                .publier(
                    &inner.registre,
                    &ca,
                    &a1,
                    "192.0.2.10:9000",
                    60_000,
                    ms,
                    &sig,
                )
                .unwrap();
            (a1, b1, cb)
        };
        (etat, a1, b1, cle_a, cle_b1, compte_b, ms)
    }

    #[test]
    fn annuaire_apres_autorisation_200() {
        let (etat, a1, b1, cle_a, cle_b1, compte_b, ms) = scene();
        let corps = corps_autorisation(&cle_a, &compte_b, Portee::Tous, false);
        let (s, c) = repondre(&etat, "POST", "/autoriser", &corps, "1.2.3.4", &[]);
        assert_eq!(s, 200, "{c}");

        let url = format!("/annuaire?appareil={}&maintenant_ms={}", a1.as_str(), ms);
        let h = en_tetes_appelant(&b1, maintenant_ms(), &[4u8; 16], &cle_b1, &url);
        let (s, c) = repondre(&etat, "GET", &url, "", "1.2.3.4", &h);
        assert_eq!(s, 200, "{c}");
        assert!(c.contains("192.0.2.10:9000"), "{c}");

        // /grants : B voit l'autorisation reçue.
        let h = en_tetes_appelant(&b1, maintenant_ms(), &[5u8; 16], &cle_b1, "/grants");
        let (s, c) = repondre(&etat, "GET", "/grants", "", "1.2.3.4", &h);
        assert_eq!(s, 200, "{c}");
        let v: Value = serde_json::from_str(&c).unwrap();
        assert_eq!(v["recues"].as_array().unwrap().len(), 1);
        assert!(v["octroyees"].as_array().unwrap().is_empty());

        // DELETE retire le droit.
        let (s, _) = repondre(&etat, "DELETE", "/autoriser", &corps, "1.2.3.4", &[]);
        assert_eq!(s, 200);
        let h = en_tetes_appelant(&b1, maintenant_ms(), &[6u8; 16], &cle_b1, &url);
        let (s, _) = repondre(&etat, "GET", &url, "", "1.2.3.4", &h);
        assert_eq!(s, 403);
    }

    #[test]
    fn annuaire_sans_autorisation_403() {
        let (etat, a1, b1, _cle_a, cle_b1, _cb, ms) = scene();
        let url = format!("/annuaire?appareil={}&maintenant_ms={}", a1.as_str(), ms);
        let h = en_tetes_appelant(&b1, maintenant_ms(), &[4u8; 16], &cle_b1, &url);
        let (s, _) = repondre(&etat, "GET", &url, "", "1.2.3.4", &h);
        assert_eq!(s, 403);
    }

    #[test]
    fn autoriser_signature_fausse_400() {
        let (etat, _a1, _b1, cle_a, _cle_b1, compte_b, _ms) = scene();
        let corps = corps_autorisation(&cle_a, &compte_b, Portee::Tous, true);
        let (s, _) = repondre(&etat, "POST", "/autoriser", &corps, "1.2.3.4", &[]);
        assert_eq!(s, 400);
        assert!(etat.lock().unwrap().autorisations.octroyees.is_empty());
    }

    // ---- /appareils (lister les appareils d'un compte) ----

    /// Compte A (a1, a2) et compte B (b1), sans présence. Rend tout ce qu'il faut
    /// pour signer et interroger `/appareils`.
    struct SceneAppareils {
        etat: Etat,
        compte_a: AccountId,
        compte_b: AccountId,
        a1: DeviceId,
        a2: DeviceId,
        b1: DeviceId,
        cle_a: SigningKey,
        cle_a1: SigningKey,
        cle_b1: SigningKey,
    }

    fn scene_appareils() -> SceneAppareils {
        let etat = etat_frais();
        let (cle_a, cle_b) = (cle(7), cle(8));
        let (cle_a1, cle_a2, cle_b1) = (cle(1), cle(2), cle(3));
        let (compte_a, compte_b, a1, a2, b1) = {
            let mut g = etat.lock().unwrap();
            let ca = inscrire_compte(&mut g, &cle_a);
            let cb = inscrire_compte(&mut g, &cle_b);
            let a1 = enregistrer_appareil(&mut g, &ca, &cle_a, &cle_a1);
            let a2 = enregistrer_appareil(&mut g, &ca, &cle_a, &cle_a2);
            let b1 = enregistrer_appareil(&mut g, &cb, &cle_b, &cle_b1);
            (ca, cb, a1, a2, b1)
        };
        SceneAppareils {
            etat,
            compte_a,
            compte_b,
            a1,
            a2,
            b1,
            cle_a,
            cle_a1,
            cle_b1,
        }
    }

    #[test]
    fn appareils_meme_compte_liste() {
        let sc = scene_appareils();
        // a1 publie une présence : son adresse apparaît, l'autre appareil reste null.
        let ms = maintenant_ms();
        let duree = 60_000;
        let expire = ms + duree;
        {
            let mut g = sc.etat.lock().unwrap();
            let sig = signe_presence(&sc.cle_a1, "198.51.100.7:47821", expire, &sc.a1);
            let inner = &mut *g;
            inner
                .presence
                .publier(
                    &inner.registre,
                    &sc.compte_a,
                    &sc.a1,
                    "198.51.100.7:47821",
                    duree,
                    ms,
                    &sig,
                )
                .unwrap();
        }

        let url = format!(
            "/appareils?compte={}&maintenant_ms={}",
            sc.compte_a.as_str(),
            ms
        );
        let h = en_tetes_appelant(&sc.a1, maintenant_ms(), &[7u8; 16], &sc.cle_a1, &url);
        let (s, c) = repondre(&sc.etat, "GET", &url, "", "1.2.3.4", &h);
        assert_eq!(s, 200, "{c}");
        let v: Value = serde_json::from_str(&c).unwrap();
        let appareils = v["appareils"].as_array().unwrap();
        assert_eq!(appareils.len(), 2, "{c}");
        let mut ids = Vec::new();
        let mut adresses = Vec::new();
        for x in appareils {
            ids.push(x["id"].as_str().unwrap());
            adresses.push(x["adresse"].as_str());
        }
        assert!(ids.contains(&sc.a1.as_str()), "{c}");
        assert!(ids.contains(&sc.a2.as_str()), "{c}");
        assert!(adresses.contains(&Some("198.51.100.7:47821")), "{c}");
        assert!(adresses.contains(&None), "{c}");
    }

    #[test]
    fn appareils_autre_compte_403() {
        let sc = scene_appareils();
        let url = format!(
            "/appareils?compte={}&maintenant_ms={}",
            sc.compte_a.as_str(),
            maintenant_ms()
        );
        let h = en_tetes_appelant(&sc.b1, maintenant_ms(), &[8u8; 16], &sc.cle_b1, &url);
        let (s, _) = repondre(&sc.etat, "GET", &url, "", "1.2.3.4", &h);
        assert_eq!(s, 403);
    }

    #[test]
    fn appareils_apres_autorisation_200() {
        let sc = scene_appareils();
        let corps = corps_autorisation(&sc.cle_a, &sc.compte_b, Portee::Tous, false);
        let (s, c) = repondre(&sc.etat, "POST", "/autoriser", &corps, "1.2.3.4", &[]);
        assert_eq!(s, 200, "{c}");

        let url = format!(
            "/appareils?compte={}&maintenant_ms={}",
            sc.compte_a.as_str(),
            maintenant_ms()
        );
        let h = en_tetes_appelant(&sc.b1, maintenant_ms(), &[9u8; 16], &sc.cle_b1, &url);
        let (s, c) = repondre(&sc.etat, "GET", &url, "", "1.2.3.4", &h);
        assert_eq!(s, 200, "{c}");
        let v: Value = serde_json::from_str(&c).unwrap();
        let ids: Vec<&str> = v["appareils"]
            .as_array()
            .unwrap()
            .iter()
            .map(|x| x["id"].as_str().unwrap())
            .collect();
        assert_eq!(ids.len(), 2, "{c}");
        assert!(ids.contains(&sc.a1.as_str()), "{c}");
        assert!(ids.contains(&sc.a2.as_str()), "{c}");
    }

    // ---- Relais opaque ----

    /// Le décor du relais : compte A (a1, a2) et compte B (b1).
    struct SceneRelais {
        etat: Etat,
        a1: DeviceId,
        a2: DeviceId,
        b1: DeviceId,
        cle_a: SigningKey,
        cle_a1: SigningKey,
        cle_a2: SigningKey,
        cle_b1: SigningKey,
        compte_b: AccountId,
        ms: u64,
    }

    fn scene_relais() -> SceneRelais {
        let etat = etat_frais();
        let (cle_a, cle_b) = (cle(7), cle(8));
        let (cle_a1, cle_a2, cle_b1) = (cle(1), cle(2), cle(3));
        let (a1, a2, b1, compte_b) = {
            let mut g = etat.lock().unwrap();
            let compte_a = inscrire_compte(&mut g, &cle_a);
            let compte_b = inscrire_compte(&mut g, &cle_b);
            let a1 = enregistrer_appareil(&mut g, &compte_a, &cle_a, &cle_a1);
            let a2 = enregistrer_appareil(&mut g, &compte_a, &cle_a, &cle_a2);
            let b1 = enregistrer_appareil(&mut g, &compte_b, &cle_b, &cle_b1);
            (a1, a2, b1, compte_b)
        };
        SceneRelais {
            etat,
            a1,
            a2,
            b1,
            cle_a,
            cle_a1,
            cle_a2,
            cle_b1,
            compte_b,
            ms: maintenant_ms(),
        }
    }

    /// Le corps d'un dépôt au relais, signé par la clé de l'appareil émetteur.
    fn corps_relais(
        cle_emetteur: &SigningKey,
        de: &DeviceId,
        a: &DeviceId,
        octets: &[u8],
        ms: u64,
    ) -> String {
        let signature = cle_emetteur.sign(&message_relais(de, a, octets, ms));
        json!({
            "de": de.as_str(),
            "a": a.as_str(),
            "octets_hex": hex::encode(octets),
            "maintenant_ms": ms,
            "signature": hex::encode(signature.to_bytes()),
        })
        .to_string()
    }

    /// Une autorisation de capacité « relais », signée par le compte `de`.
    fn corps_autorisation_relais(de: &SigningKey, a: &AccountId) -> String {
        let mut x = Autorisation {
            de: AccountId::de_cle(&de.verifying_key()),
            a: a.clone(),
            capacite: CAPACITE_RELAIS.into(),
            portee: Portee::Tous,
            cree_ms: 1,
            expire_ms: None,
            signature: vec![],
        };
        x.signature = de.sign(&x.octets_signes()).to_bytes().to_vec();
        json!({
            "de": x.de, "a": x.a, "capacite": x.capacite, "portee": x.portee,
            "cree_ms": x.cree_ms, "expire_ms": null,
            "signature": hex::encode(&x.signature),
        })
        .to_string()
    }

    /// Lit la boîte de `appareil` (GET /relais signé) et rend les octets bruts reçus.
    fn read_relay(
        etat: &Etat,
        appareil: &DeviceId,
        cle: &SigningKey,
        ip: &str,
        nonce: u8,
    ) -> (u16, Vec<Vec<u8>>) {
        let h = en_tetes_appelant(appareil, maintenant_ms(), &[nonce; 16], cle, "/relais");
        let (statut, corps) = repondre(etat, "GET", "/relais", "", ip, &h);
        if statut != 200 {
            return (statut, Vec::new());
        }
        let v: Value = serde_json::from_str(&corps).unwrap();
        let messages = v["messages"]
            .as_array()
            .unwrap()
            .iter()
            .map(|m| hex::decode(m.as_str().unwrap()).unwrap())
            .collect();
        (statut, messages)
    }

    #[test]
    fn relais_meme_compte_octets_identiques() {
        let sc = scene_relais();
        let octets: Vec<u8> = (0..4_096usize).map(|i| (i % 251) as u8).collect();

        let corps = corps_relais(&sc.cle_a1, &sc.a1, &sc.a2, &octets, sc.ms);
        let (s, c) = repondre(&sc.etat, "POST", "/relais", &corps, "203.0.113.1", &[]);
        assert_eq!(s, 200, "{c}");

        // a2 lit SA boîte : les octets sont rendus tels quels, à l'octet près.
        let (s, messages) = read_relay(&sc.etat, &sc.a2, &sc.cle_a2, "203.0.113.2", 0x11);
        assert_eq!(s, 200);
        assert_eq!(messages, vec![octets]);

        // La boîte a été vidée : une seconde lecture ne rend rien.
        let (s, messages) = read_relay(&sc.etat, &sc.a2, &sc.cle_a2, "203.0.113.2", 0x12);
        assert_eq!(s, 204);
        assert!(messages.is_empty());
    }

    #[test]
    fn relais_sans_autorisation_403() {
        let sc = scene_relais();
        let octets = b"pour un autre compte".to_vec();
        let corps = corps_relais(&sc.cle_b1, &sc.b1, &sc.a2, &octets, sc.ms);
        let (s, _) = repondre(&sc.etat, "POST", "/relais", &corps, "203.0.113.3", &[]);
        assert_eq!(s, 403);

        // Rien n'a été mis en boîte.
        let (s, _) = read_relay(&sc.etat, &sc.a2, &sc.cle_a2, "203.0.113.4", 0x20);
        assert_eq!(s, 204);
    }

    #[test]
    fn relais_apres_autorisation_ok() {
        let sc = scene_relais();

        // A (le destinataire) accorde à B (l'émetteur) la capacité « relais ».
        let corps = corps_autorisation_relais(&sc.cle_a, &sc.compte_b);
        let (s, c) = repondre(&sc.etat, "POST", "/autoriser", &corps, "203.0.113.5", &[]);
        assert_eq!(s, 200, "{c}");

        let octets = b"de B vers A, autorise".to_vec();
        let corps = corps_relais(&sc.cle_b1, &sc.b1, &sc.a2, &octets, sc.ms);
        let (s, c) = repondre(&sc.etat, "POST", "/relais", &corps, "203.0.113.6", &[]);
        assert_eq!(s, 200, "{c}");

        let (s, messages) = read_relay(&sc.etat, &sc.a2, &sc.cle_a2, "203.0.113.7", 0x33);
        assert_eq!(s, 200);
        assert_eq!(messages, vec![octets]);
    }

    #[test]
    fn relais_quota_429() {
        let etat = etat_frais();
        let cle_compte = cle(7);
        let cle_emetteur = cle(1);
        let (emetteur, cibles) = {
            let mut g = etat.lock().unwrap();
            let compte = inscrire_compte(&mut g, &cle_compte);
            let emetteur = enregistrer_appareil(&mut g, &compte, &cle_compte, &cle_emetteur);
            // Une boîte ne tient qu'1 Mio : le quota de 10 Mo se mesure donc sur plusieurs
            // destinataires du même compte émetteur.
            let cibles: Vec<DeviceId> = (0..11u8)
                .map(|i| {
                    let cle_cible = SigningKey::from_bytes(&[100 + i; 32]);
                    enregistrer_appareil(&mut g, &compte, &cle_compte, &cle_cible)
                })
                .collect();
            (emetteur, cibles)
        };

        let octets = vec![0xA5u8; 1_000_000];
        let mut statuts = Vec::new();
        for (i, cible) in cibles.iter().enumerate() {
            let corps = corps_relais(&cle_emetteur, &emetteur, cible, &octets, maintenant_ms());
            let ip = format!("198.51.100.{i}");
            // Une IP par envoi : ce test porte sur le quota, pas sur la limite de débit.
            let (s, c) = repondre(&etat, "POST", "/relais", &corps, &ip, &[]);
            statuts.push((s, c));
        }
        for (i, (s, c)) in statuts.iter().enumerate().take(10) {
            assert_eq!(*s, 200, "envoi {i} : {c}");
        }
        assert_eq!(statuts[10].0, 429, "corps : {}", statuts[10].1);
    }

    #[test]
    fn relais_signature_fausse_400() {
        let sc = scene_relais();
        let octets = b"octets signes par un autre".to_vec();
        let mut corps: Value =
            serde_json::from_str(&corps_relais(&sc.cle_a1, &sc.a1, &sc.a2, &octets, sc.ms))
                .unwrap();
        let mut sig = hex::decode(corps["signature"].as_str().unwrap()).unwrap();
        sig[0] ^= 0xff;
        corps["signature"] = Value::String(hex::encode(sig));

        let (s, _) = repondre(
            &sc.etat,
            "POST",
            "/relais",
            &corps.to_string(),
            "203.0.113.8",
            &[],
        );
        assert_eq!(s, 400);

        // Rien n'a été mis en boîte, et le quota n'a pas bougé.
        let (s, _) = read_relay(&sc.etat, &sc.a2, &sc.cle_a2, "203.0.113.9", 0x40);
        assert_eq!(s, 204);
    }

    #[test]
    fn relais_horloge_hors_fenetre_401() {
        let sc = scene_relais();
        let octets = b"trop tard".to_vec();
        let corps = corps_relais(&sc.cle_a1, &sc.a1, &sc.a2, &octets, sc.ms - 61_000);
        let (s, _) = repondre(&sc.etat, "POST", "/relais", &corps, "203.0.113.10", &[]);
        assert_eq!(s, 401);
    }
}
