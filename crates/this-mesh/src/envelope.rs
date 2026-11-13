use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use serde::{Deserialize, Serialize};
use serde_json::{Value, value::RawValue};
use std::fmt;
use uuid::Uuid;

/// Version de l'enveloppe.
pub const MESH_VERSION: u32 = 1;

/// Préfixe de domaine des octets signés : une signature du maillage ne vaut rien
/// ailleurs (manifestes, etc.).
pub const SIGNING_DOMAIN: &[u8] = b"this-mesh/v1\n";

/// Identifiant d'appareil : `device-` suivi de 8 hexadécimaux minuscules.
///
/// Ce n'est qu'une étiquette : la confiance vient de la clé enregistrée sous cet
/// identifiant dans [`crate::Peers`], jamais de sa forme.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct DeviceId(String);

impl DeviceId {
    /// Valide la forme `device-<8 hex minuscules>`.
    pub fn parse(s: &str) -> Option<Self> {
        let hex = s.strip_prefix("device-")?;
        (hex.len() == 8 && hex.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')))
            .then(|| Self(s.to_owned()))
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// L'identifiant dérivé d'une clé publique : `device-` suivi des 4 premiers
    /// octets de la clé en hexadécimal minuscule. C'est la dérivation que
    /// l'application applique déjà (`id_maillage`) ; elle sert au transport à
    /// nommer un pair qu'il ne connaît encore que par sa clé (appairage). Reste
    /// une étiquette : la confiance vient de la clé enregistrée, pas de ce nom.
    pub fn de_cle(cle: &VerifyingKey) -> Self {
        let mut id = String::from("device-");
        for octet in &cle.as_bytes()[..4] {
            id.push_str(&format!("{octet:02x}"));
        }
        Self(id)
    }
}

impl fmt::Display for DeviceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Types de messages : **énuméré**, aucun type libre.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MessageType {
    AppairageSas,
    GrantDemande,
    GrantReponse,
    EspaceOp,
    EspaceSync,
    AgentPropose,
    AgentRepond,
    /// Appel d'un service du pair : le pair répond dans l'accusé (voir
    /// `transport::appeler`).
    ServiceAppel,
    /// Réponse d'un service, quand elle voyage comme un message à part entière.
    ServiceReponse,
}

impl MessageType {
    pub const ALL: [MessageType; 9] = [
        Self::AppairageSas,
        Self::GrantDemande,
        Self::GrantReponse,
        Self::EspaceOp,
        Self::EspaceSync,
        Self::AgentPropose,
        Self::AgentRepond,
        Self::ServiceAppel,
        Self::ServiceReponse,
    ];
    /// Nom sur le fil.
    pub fn wire(self) -> &'static str {
        match self {
            Self::AppairageSas => "appairage.sas",
            Self::GrantDemande => "grant.demande",
            Self::GrantReponse => "grant.reponse",
            Self::EspaceOp => "espace.op",
            Self::EspaceSync => "espace.sync",
            Self::AgentPropose => "agent.propose",
            Self::AgentRepond => "agent.repond",
            Self::ServiceAppel => "service.appel",
            Self::ServiceReponse => "service.reponse",
        }
    }
    /// `None` pour un type inconnu.
    pub fn from_wire(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|t| t.wire() == s)
    }
}

/// Échec de signature ou de canonicalisation.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum SignError {
    /// La charge contient un nombre non entier : sa forme canonique n'est pas unique.
    #[error("charge non canonique : nombre à virgule (utiliser des entiers ou des chaînes)")]
    NonCanonical,
}

/// Un message du maillage, toujours la même structure.
///
/// `charge` est une valeur JSON ; sa forme **canonique** (clés triées) est ce qui
/// est signé, donc l'ordre des champs ou la mise en forme sur le fil n'ont aucun effet.
#[derive(Debug, Clone, PartialEq)]
pub struct Message {
    pub v: u32,
    pub id: Uuid,
    pub de: DeviceId,
    pub a: DeviceId,
    pub kind: MessageType,
    /// Compteur logique propre à l'appareil émetteur — pas un horodatage.
    pub horloge: u64,
    pub charge: Value,
    /// Ed25519, 64 octets en hexadécimal.
    pub signature: String,
}

/// Forme du fil : le type est une chaîne (un type inconnu doit se lire sans échec)
/// et la charge reste du JSON brut, non interprété.
#[derive(Deserialize)]
pub(crate) struct Wire<'a> {
    pub v: u32,
    pub id: Uuid,
    pub de: DeviceId,
    pub a: DeviceId,
    #[serde(rename = "type")]
    pub kind: String,
    pub horloge: u64,
    #[serde(borrow)]
    pub charge: &'a RawValue,
    pub signature: String,
}

#[derive(Serialize)]
struct WireOut<'a> {
    v: u32,
    id: &'a Uuid,
    de: &'a DeviceId,
    a: &'a DeviceId,
    #[serde(rename = "type")]
    kind: &'static str,
    horloge: u64,
    charge: &'a Value,
    signature: &'a str,
}

impl Message {
    /// Construit et signe un message.
    pub fn signed(
        key: &SigningKey,
        id: Uuid,
        de: DeviceId,
        a: DeviceId,
        kind: MessageType,
        horloge: u64,
        charge: Value,
    ) -> Result<Self, SignError> {
        let mut m = Message {
            v: MESH_VERSION,
            id,
            de,
            a,
            kind,
            horloge,
            charge,
            signature: String::new(),
        };
        let sig = key.sign(&m.signing_bytes()?);
        m.signature = to_hex(&sig.to_bytes());
        Ok(m)
    }

    /// Les octets signés : [`SIGNING_DOMAIN`] suivi du JSON canonique (compact, clés
    /// triées par ordre d'octets, à tous les niveaux) de l'objet
    /// `{a, charge, de, horloge, id, type, v}` — tout sauf `signature`, `de` et `a` compris.
    pub fn signing_bytes(&self) -> Result<Vec<u8>, SignError> {
        signing_bytes(
            self.v,
            &self.id,
            &self.de,
            &self.a,
            self.kind.wire(),
            self.horloge,
            &self.charge,
        )
    }

    /// Vérifie la signature contre une clé publique.
    pub fn verify(&self, key: &VerifyingKey) -> bool {
        verify(
            key,
            &self.signing_bytes().unwrap_or_default(),
            &self.signature,
        )
    }

    /// Sérialise pour le fil (JSON, charge en forme canonique).
    pub fn to_bytes(&self) -> Vec<u8> {
        serde_json::to_vec(&WireOut {
            v: self.v,
            id: &self.id,
            de: &self.de,
            a: &self.a,
            kind: self.kind.wire(),
            horloge: self.horloge,
            charge: &self.charge,
            signature: &self.signature,
        })
        .expect("Message serializes")
    }
}

pub(crate) fn signing_bytes(
    v: u32,
    id: &Uuid,
    de: &DeviceId,
    a: &DeviceId,
    kind: &str,
    horloge: u64,
    charge: &Value,
) -> Result<Vec<u8>, SignError> {
    let mut out = SIGNING_DOMAIN.to_vec();
    // Clés dans l'ordre d'octets : a, charge, de, horloge, id, type, v.
    out.extend_from_slice(b"{\"a\":");
    write_str(&mut out, a.as_str());
    out.extend_from_slice(b",\"charge\":");
    canonical(&mut out, charge)?;
    out.extend_from_slice(b",\"de\":");
    write_str(&mut out, de.as_str());
    out.extend_from_slice(
        format!(
            ",\"horloge\":{horloge},\"id\":\"{}\",\"type\":",
            id.hyphenated()
        )
        .as_bytes(),
    );
    write_str(&mut out, kind);
    out.extend_from_slice(format!(",\"v\":{v}}}").as_bytes());
    Ok(out)
}

pub(crate) fn verify(key: &VerifyingKey, bytes: &[u8], sig_hex: &str) -> bool {
    let Some(raw) = from_hex(sig_hex) else {
        return false;
    };
    let Ok(arr) = <[u8; 64]>::try_from(raw.as_slice()) else {
        return false;
    };
    key.verify(bytes, &Signature::from_bytes(&arr)).is_ok()
}

fn write_str(out: &mut Vec<u8>, s: &str) {
    serde_json::to_writer(&mut *out, s).expect("string serializes");
}

fn canonical(out: &mut Vec<u8>, v: &Value) -> Result<(), SignError> {
    match v {
        Value::Null => out.extend_from_slice(b"null"),
        Value::Bool(b) => out.extend_from_slice(if *b { b"true" } else { b"false" }),
        Value::Number(n) => {
            if n.is_f64() {
                return Err(SignError::NonCanonical);
            }
            out.extend_from_slice(n.to_string().as_bytes());
        }
        Value::String(s) => write_str(out, s),
        Value::Array(a) => {
            out.push(b'[');
            for (i, x) in a.iter().enumerate() {
                if i > 0 {
                    out.push(b',');
                }
                canonical(out, x)?;
            }
            out.push(b']');
        }
        Value::Object(m) => {
            // Ne dépend pas de la feature `preserve_order` de serde_json.
            let mut keys: Vec<&String> = m.keys().collect();
            keys.sort();
            out.push(b'{');
            for (i, k) in keys.into_iter().enumerate() {
                if i > 0 {
                    out.push(b',');
                }
                write_str(out, k);
                out.push(b':');
                canonical(out, &m[k])?;
            }
            out.push(b'}');
        }
    }
    Ok(())
}

pub(crate) fn to_hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

pub(crate) fn from_hex(s: &str) -> Option<Vec<u8>> {
    if !s.len().is_multiple_of(2) || !s.is_ascii() {
        return None;
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).ok())
        .collect()
}
