//! Identité TLS et épinglage : le certificat auto-signé est dérivé de la clé
//! Ed25519 de l'appareil, et le vérificateur n'accepte QUE le certificat attendu.
//!
//! # Pourquoi c'est sûr ici
//!
//! Pas de PKI, pas de magasin de racines, pas de « accepter n'importe quoi ».
//! Les deux côtés épingle l'**empreinte connue par appairage** : la clé publique
//! Ed25519 de l'autre appareil, la même valeur qui a été comparée par le code
//! court (SAS) lors de l'appairage, et la même que le registre
//! ([`crate::Peers`]) retient. Un homme du milieu ne peut pas présenter un
//! certificat dont la clé publique est celle de l'appareil attendu, car il ne
//! détient pas la clé privée correspondante : le serveur comme le client
//! signent la poignée de main TLS 1.3 avec cette clé, et rustls vérifie cette
//! signature après que le vérificateur a épinglé la clé.

use std::sync::Arc;

use ed25519_dalek::{SigningKey, VerifyingKey};
use rcgen::{CertificateParams, DnType, KeyPair, PKCS_ED25519};
use rustls::client::danger::ServerCertVerifier;
use rustls::crypto::{CryptoProvider, verify_tls13_signature};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use rustls::server::danger::ClientCertVerifier;
use rustls::{ClientConfig, DigitallySignedStruct, Error, ServerConfig, SignatureScheme};

use crate::DeviceId;
use crate::transport::TransportError;

/// L'identité TLS d'un appareil : son certificat auto-signé et sa clé privée,
/// tous deux dérivés de la clé Ed25519 qui signe aussi les messages.
pub struct Identite {
    id: DeviceId,
    cle_publique: [u8; 32],
    cert: CertificateDer<'static>,
    key: PrivateKeyDer<'static>,
    provider: Arc<CryptoProvider>,
}

// `Debug` manuel : la clé privée ne doit jamais apparaître dans une trace
// (`PrivateKeyDer` s'affiche en hexadécimal). On n'expose que ce qui est public.
impl std::fmt::Debug for Identite {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Identite")
            .field("id", &self.id)
            .field("cle_publique", &self.cle_publique)
            .finish_non_exhaustive()
    }
}

impl Identite {
    /// Construit l'identité à partir de la clé d'appareil et de son identifiant.
    ///
    /// Le certificat est auto-signé : sa clé publique **est** la clé Ed25519 de
    /// l'appareil (SPKI Ed25519), son `CommonName` est l'identifiant. Aucun secret
    /// supplémentaire n'est généré, aucune racine n'est introduite.
    pub fn de_cle(key: &SigningKey, id: &DeviceId) -> Result<Self, TransportError> {
        let seed = key.to_bytes();
        let provider = rustls::crypto::ring::default_provider();
        let key_der = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(pkcs8_ed25519(&seed)));
        let key_pair = KeyPair::from_der_and_sign_algo(&key_der, &PKCS_ED25519)
            .map_err(|e| TransportError::Identite(e.to_string()))?;

        let mut params =
            CertificateParams::new(vec![]).map_err(|e| TransportError::Identite(e.to_string()))?;
        params
            .distinguished_name
            .push(DnType::CommonName, id.as_str());
        let cert = params
            .self_signed(&key_pair)
            .map_err(|e| TransportError::Identite(e.to_string()))?;

        Ok(Self {
            id: id.clone(),
            cle_publique: key.verifying_key().to_bytes(),
            cert: cert.der().clone(),
            key: key_der,
            provider: Arc::new(provider),
        })
    }

    pub fn id(&self) -> &DeviceId {
        &self.id
    }

    /// La clé publique Ed25519 portée par le certificat (32 octets).
    pub fn cle_publique(&self) -> [u8; 32] {
        self.cle_publique
    }

    /// Empreinte (SHA-256 de la clé publique) pour les journaux, jamais comparée
    /// pour décider — la décision se fait sur la clé complète.
    pub fn empreinte(&self) -> [u8; 32] {
        use sha2::{Digest, Sha256};
        Sha256::digest(self.cle_publique).into()
    }

    pub(crate) fn cert(&self) -> CertificateDer<'static> {
        self.cert.clone()
    }

    pub(crate) fn key(&self) -> PrivateKeyDer<'static> {
        self.key.clone_key()
    }

    pub(crate) fn provider(&self) -> Arc<CryptoProvider> {
        self.provider.clone()
    }
}

/// Ce qu'un client attend du serveur : sa clé publique Ed25519, épinglée.
///
/// C'est l'empreinte connue par appairage (comparée par le SAS) ; on n'épingle
/// ni une autorité ni un nom, seulement cette clé.
#[derive(Debug, Clone)]
pub struct Epingle {
    attendu: [u8; 32],
}

impl Epingle {
    pub fn cle(cle: &VerifyingKey) -> Self {
        Self {
            attendu: cle.to_bytes(),
        }
    }

    pub fn attendu(&self) -> [u8; 32] {
        self.attendu
    }
}

/// Construit la configuration TLS du **serveur** : présente le certificat de
/// `identite`, exige un certificat client et n'accepte que des clés Ed25519
/// bien formées. La décision « clé connue / révoquée » revient à la
/// [`crate::Gate`] (source unique), pas au vérificateur TLS.
pub(crate) fn config_serveur(identite: &Identite) -> Result<ServerConfig, TransportError> {
    let mut cfg = ServerConfig::builder_with_provider(identite.provider())
        .with_protocol_versions(&[&rustls::version::TLS13])
        .map_err(|e| TransportError::Identite(e.to_string()))?
        .with_client_cert_verifier(Arc::new(VerifierClient {
            provider: identite.provider(),
        }))
        .with_single_cert(vec![identite.cert()], identite.key())
        .map_err(|e| TransportError::Identite(e.to_string()))?;
    cfg.alpn_protocols = vec![crate::transport::ALPN.to_vec()];
    Ok(cfg)
}

/// Construit la configuration TLS du **client** : épingle le serveur sur
/// `epingle` et présente le certificat de `identite`.
pub(crate) fn config_client(
    identite: &Identite,
    epingle: &Epingle,
) -> Result<ClientConfig, TransportError> {
    let mut cfg = ClientConfig::builder_with_provider(identite.provider())
        .with_protocol_versions(&[&rustls::version::TLS13])
        .map_err(|e| TransportError::Identite(e.to_string()))?
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(VerifierServeur {
            attendu: epingle.attendu,
            provider: identite.provider(),
        }))
        .with_client_auth_cert(vec![identite.cert()], identite.key())
        .map_err(|e| TransportError::Identite(e.to_string()))?;
    cfg.alpn_protocols = vec![crate::transport::ALPN.to_vec()];
    Ok(cfg)
}

/// Vérificateur côté client : n'accepte que le certificat dont la clé publique
/// Ed25519 est exactement `attendu`.
#[derive(Debug)]
struct VerifierServeur {
    attendu: [u8; 32],
    provider: Arc<CryptoProvider>,
}

impl ServerCertVerifier for VerifierServeur {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &rustls::pki_types::ServerName<'_>,
        _ocsp_response: &[u8],
        _now: rustls::pki_types::UnixTime,
    ) -> Result<rustls::client::danger::ServerCertVerified, Error> {
        let cle = cle_ed25519_du_cert(end_entity.as_ref())
            .map_err(|e| Error::General(format!("épinglage : {e}")))?;
        if cle == self.attendu {
            Ok(rustls::client::danger::ServerCertVerified::assertion())
        } else {
            Err(Error::General(
                "épinglage : la clé du certificat ne correspond pas à l'empreinte attendue".into(),
            ))
        }
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, Error> {
        Err(Error::General("TLS 1.2 est désactivé".into()))
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, Error> {
        verify_tls13_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.provider
            .signature_verification_algorithms
            .supported_schemes()
    }
}

/// Vérificateur côté serveur : exige un certificat client, en extrait la clé
/// Ed25519 (preuve de possession : la poignée de main est signée avec la clé
/// privée correspondante). Le registre décide ensuite qui est admis.
#[derive(Debug)]
struct VerifierClient {
    provider: Arc<CryptoProvider>,
}

impl ClientCertVerifier for VerifierClient {
    fn offer_client_auth(&self) -> bool {
        true
    }

    fn client_auth_mandatory(&self) -> bool {
        true
    }

    fn root_hint_subjects(&self) -> &[rustls::DistinguishedName] {
        &[]
    }

    fn verify_client_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _now: rustls::pki_types::UnixTime,
    ) -> Result<rustls::server::danger::ClientCertVerified, Error> {
        cle_ed25519_du_cert(end_entity.as_ref())
            .map_err(|e| Error::General(format!("certificat client : {e}")))?;
        Ok(rustls::server::danger::ClientCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, Error> {
        Err(Error::General("TLS 1.2 est désactivé".into()))
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, Error> {
        verify_tls13_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.provider
            .signature_verification_algorithms
            .supported_schemes()
    }
}

/// Construit la configuration QUIC du client à partir de la configuration TLS.
pub(crate) fn quic_client(config: ClientConfig) -> Result<quinn::ClientConfig, TransportError> {
    let crypto = quinn::crypto::rustls::QuicClientConfig::try_from(config)
        .map_err(|e| TransportError::Identite(e.to_string()))?;
    Ok(quinn::ClientConfig::new(Arc::new(crypto)))
}

/// Construit la configuration QUIC du serveur à partir de la configuration TLS.
pub(crate) fn quic_server(config: ServerConfig) -> Result<quinn::ServerConfig, TransportError> {
    let crypto = quinn::crypto::rustls::QuicServerConfig::try_from(config)
        .map_err(|e| TransportError::Identite(e.to_string()))?;
    Ok(quinn::ServerConfig::with_crypto(Arc::new(crypto)))
}

/// PKCS#8 v1 d'une clé privée Ed25519 : `SEQUENCE { INTEGER 0, SEQUENCE { OID
/// 1.3.101.112 }, OCTET STRING { OCTET STRING { 32 octets } } }`. Forme
/// canonique, 48 octets, la seule que `rcgen` (via ring) sait relire pour la
/// même graine que `ed25519-dalek`.
fn pkcs8_ed25519(seed: &[u8; 32]) -> Vec<u8> {
    let mut v = Vec::with_capacity(48);
    v.extend_from_slice(&[
        0x30, 0x2e, // SEQUENCE, 46 octets
        0x02, 0x01, 0x00, // INTEGER 0 (version)
        0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, // SEQUENCE { OID 1.3.101.112 }
        0x04, 0x22, // OCTET STRING, 34 octets
        0x04, 0x20, // OCTET STRING, 32 octets (la graine)
    ]);
    v.extend_from_slice(seed);
    v
}

/// Extrait la clé publique Ed25519 (32 octets) d'un certificat X.509 DER, en
/// vérifiant que l'algorithme de la SPKI est bien Ed25519 (OID 1.3.101.112).
pub(crate) fn cle_ed25519_du_cert(cert_der: &[u8]) -> Result<[u8; 32], &'static str> {
    let spki = spki_du_cert(cert_der)?;
    cle_ed25519_du_spki(spki)
}

/// Localise la SPKI (`SubjectPublicKeyInfo`) d'un certificat X.509 : le seul
/// enfant `SEQUENCE` du `tbsCertificate` qui contient un enfant direct
/// `BIT STRING`.
fn spki_du_cert(cert_der: &[u8]) -> Result<&[u8], &'static str> {
    // Certificate ::= SEQUENCE { tbsCertificate, signatureAlgorithm, signatureValue }
    let (tag, cont, _fin, _) = tlv(cert_der, 0).ok_or("certificat : DER illisible")?;
    if tag != 0x30 {
        return Err("certificat : attendu une SEQUENCE");
    }
    let (tbs_tag, tbs_start, tbs_end, _) =
        tlv(cert_der, cont).ok_or("certificat : tbs illisible")?;
    if tbs_tag != 0x30 {
        return Err("certificat : tbs attendu en SEQUENCE");
    }
    let mut pos = tbs_start;
    while pos < tbs_end {
        let (child_tag, child_start, child_end, next) =
            tlv(cert_der, pos).ok_or("certificat : champ illisible")?;
        if child_tag == 0x30 && a_enfant_direct(cert_der, child_start, child_end, 0x03) {
            return Ok(&cert_der[pos..next]);
        }
        pos = next;
    }
    Err("certificat : SPKI introuvable")
}

/// Extrait la clé Ed25519 d'une SPKI DER : `SEQUENCE { algorithm, BIT STRING }`.
fn cle_ed25519_du_spki(spki: &[u8]) -> Result<[u8; 32], &'static str> {
    let (tag, cont, _fin, _next) = tlv(spki, 0).ok_or("SPKI illisible")?;
    if tag != 0x30 {
        return Err("SPKI : attendu une SEQUENCE");
    }
    // 1. Algorithme : SEQUENCE dont l'OID est Ed25519.
    let (alg_tag, alg_start, alg_end, after_alg) =
        tlv(spki, cont).ok_or("SPKI : algorithme illisible")?;
    if alg_tag != 0x30 || !a_oid_ed25519(spki, alg_start, alg_end) {
        return Err("SPKI : algorithme attendu Ed25519 (1.3.101.112)");
    }
    // 2. Clé : BIT STRING (1 octet de bits inutilisés, puis 32 octets).
    let (bit_tag, bit_start, bit_end, _) = tlv(spki, after_alg).ok_or("SPKI : clé illisible")?;
    if bit_tag != 0x03 {
        return Err("SPKI : clé attendue en BIT STRING");
    }
    let inutilises = *spki.get(bit_start).ok_or("SPKI : BIT STRING vide")?;
    if inutilises != 0 {
        return Err("SPKI : bits inutilisés non nuls");
    }
    let cle = spki
        .get(bit_start + 1..bit_end)
        .ok_or("SPKI : clé tronquée")?;
    if cle.len() != 32 {
        return Err("SPKI : clé Ed25519 attendue sur 32 octets");
    }
    Ok(<[u8; 32]>::try_from(cle).expect("32 octets"))
}

/// L'enfant `SEQUENCE` d'algorithme contient-il l'OID Ed25519 (1.3.101.112) ?
fn a_oid_ed25519(der: &[u8], alg_start: usize, alg_end: usize) -> bool {
    let mut pos = alg_start;
    while pos < alg_end {
        let Some((tag, cont, _fin, next)) = tlv(der, pos) else {
            return false;
        };
        if tag == 0x06 && der.get(cont..next) == Some(&[0x2b, 0x65, 0x70][..]) {
            return true;
        }
        pos = next;
    }
    false
}

/// L'élément `SEQUENCE` [`start`, `end`) contient-il un enfant direct de tag
/// `tag_recherche` ?
fn a_enfant_direct(der: &[u8], start: usize, end: usize, tag_recherche: u8) -> bool {
    let mut pos = start;
    while pos < end {
        let Some((tag, _, _, next)) = tlv(der, pos) else {
            return false;
        };
        if tag == tag_recherche {
            return true;
        }
        pos = next;
    }
    false
}

/// Lit un élément TLV DER à `pos`. Rend `(tag, début_contenu, fin_contenu,
/// prochain_élément)`. Forme indéfinie refusée, longueurs > 4 octets refusées.
fn tlv(der: &[u8], pos: usize) -> Option<(u8, usize, usize, usize)> {
    let tag = *der.get(pos)?;
    let (len, len_octets) = longueur(der, pos + 1)?;
    let debut = pos + 1 + len_octets;
    let fin = debut.checked_add(len)?;
    if fin > der.len() {
        return None;
    }
    Some((tag, debut, fin, fin))
}

fn longueur(der: &[u8], pos: usize) -> Option<(usize, usize)> {
    let premier = *der.get(pos)?;
    if premier < 0x80 {
        Some((premier as usize, 1))
    } else {
        let n = (premier & 0x7f) as usize;
        if n == 0 || n > 4 {
            return None;
        }
        let mut len = 0usize;
        for i in 0..n {
            len = (len << 8) | (*der.get(pos + 1 + i)? as usize);
        }
        Some((len, 1 + n))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::SigningKey;

    fn dev(s: &str) -> DeviceId {
        DeviceId::parse(s).unwrap()
    }

    #[test]
    fn pkcs8_ed25519_a_la_bonne_forme() {
        let seed = [7u8; 32];
        let der = pkcs8_ed25519(&seed);
        assert_eq!(der.len(), 48);
        // En-tête canonique.
        assert_eq!(
            &der[..16],
            &[
                0x30, 0x2e, 0x02, 0x01, 0x00, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x04, 0x22,
                0x04, 0x20
            ]
        );
        assert_eq!(&der[16..], &seed);
    }

    #[test]
    fn le_certificat_porte_la_cle_de_l_appareil() {
        let cle = SigningKey::from_bytes(&[9u8; 32]);
        let id = dev("device-aaaaaaaa");
        let identite = Identite::de_cle(&cle, &id).unwrap();
        // La clé publique du certificat est exactement celle de l'appareil.
        let extraite = cle_ed25519_du_cert(identite.cert().as_ref()).unwrap();
        assert_eq!(extraite, cle.verifying_key().to_bytes());
        assert_eq!(identite.cle_publique(), extraite);
    }

    #[test]
    fn une_autre_cle_est_refusee_par_le_pin() {
        let cle = SigningKey::from_bytes(&[1u8; 32]);
        let autre = SigningKey::from_bytes(&[2u8; 32]);
        let id = dev("device-aaaaaaaa");
        let identite = Identite::de_cle(&cle, &id).unwrap();
        let extraite = cle_ed25519_du_cert(identite.cert().as_ref()).unwrap();
        assert_ne!(extraite, autre.verifying_key().to_bytes());
    }

    #[test]
    fn l_extraction_rejette_un_algorithme_non_ed25519() {
        // SPKI d'une clé inconnue (OID différent) : doit être refusée.
        // OID RSA (1.2.840.113549.1.1.1) au lieu d'Ed25519.
        let spki = [
            0x30, 0x0b, // SEQUENCE 11
            0x30, 0x09, 0x06, 0x07, 0x2a, 0x86, 0x48, 0xce, 0x3d, 0x02, 0x01, // algo P-256
            0x03, 0x00, // BIT STRING vide
        ];
        assert!(cle_ed25519_du_spki(&spki).is_err());
    }
}
