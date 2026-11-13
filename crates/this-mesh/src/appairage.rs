//! Appairage : le code court (SAS) et la machine d'états qui mène la clé d'un pair
//! jusqu'au registre, **sans aucune écriture avant les deux confirmations**.
//!
//! La dérivation du code ([`sas`]) est pure et symétrique ; la machine d'états
//! ([`Appairage`]) ne produit une [`Admission`] que lorsque la confirmation locale
//! ET la confirmation du pair sont acquises. Une confirmation seule ne prouve rien :
//! c'est précisément ce qu'un homme du milieu ne peut pas produire.

use crate::{DeviceId, PeerError, Peers};
use chrono::Utc;
use ed25519_dalek::VerifyingKey;
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use this_vfs::layout::AnchorKeyRole;

type HmacSha256 = Hmac<Sha256>;

/// Préfixe de domaine de la dérivation du SAS : il sert de clé HMAC, donc un code
/// dérivé du maillage ne vaut pour rien d'autre (jamais confondu avec une signature).
pub const SAS_DOMAIN: &[u8] = b"this-mesh/sas/v1";

/// Nombre de chiffres du code court.
pub const SAS_DIGITS: u32 = 6;

/// Dérive le code court à 6 chiffres d'un appairage, des deux clés publiques et d'un
/// nonce partagé : HMAC-SHA256 (clé [`SAS_DOMAIN`]) sur `min(clés) ‖ max(clés) ‖ nonce`,
/// digest réduit modulo 10⁶ et zéro-paddé. Les clés sont triées avant l'entrée, donc
/// l'ordre n'importe pas ; le nonce rend deux appairages simultanés distincts.
pub fn sas(a: &VerifyingKey, b: &VerifyingKey, nonce: &[u8]) -> String {
    // Ordre canonique : les deux côtés ont chacun les deux clés, éventuellement dans
    // l'ordre inverse ; le tri garantit le même flux d'octets des deux côtés.
    let mut keys = [a.as_bytes(), b.as_bytes()];
    keys.sort_unstable();

    let mut mac = HmacSha256::new_from_slice(SAS_DOMAIN).expect("domaine non vide");
    mac.update(&keys[0][..]);
    mac.update(&keys[1][..]);
    mac.update(nonce);
    let digest = mac.finalize().into_bytes();
    // 8 premiers octets en entier non signé big-endian, réduit modulo 10^SAS_DIGITS :
    // le biais de ce modulo est de l'ordre de 2⁻⁵⁰, sans effet sur un code à comparer.
    let n = u64::from_be_bytes(digest[..8].try_into().expect("digest SHA-256 = 32 octets"));
    let modulus = 10u64.pow(SAS_DIGITS);
    format!("{:0width$}", n % modulus, width = SAS_DIGITS as usize)
}

/// État « rencontre » : l'autre clé est connue, rien n'est encore dérivé ni écrit.
///
/// `moi` est ma clé publique (elle sert à dériver le SAS), `pair` l'identifiant et la
/// clé de l'autre, `nonce` le nonce partagé de cet appairage.
#[derive(Debug, Clone)]
pub struct Rencontre {
    moi: VerifyingKey,
    pair_id: DeviceId,
    pair_key: VerifyingKey,
    nonce: Vec<u8>,
}

impl Rencontre {
    /// Nouvelle rencontre. Aucun effet de bord : ni code figé, ni registre touché.
    pub fn nouvelle(
        moi: VerifyingKey,
        pair_id: DeviceId,
        pair_key: VerifyingKey,
        nonce: Vec<u8>,
    ) -> Self {
        Self {
            moi,
            pair_id,
            pair_key,
            nonce,
        }
    }

    /// Le code court dérivé, sans quitter l'état (pour l'affichage).
    pub fn code(&self) -> String {
        sas(&self.moi, &self.pair_key, &self.nonce)
    }

    /// Passe à « sas attendu » : le code est figé, on attend les deux confirmations.
    pub fn afficher(self) -> SasAttendu {
        let code = sas(&self.moi, &self.pair_key, &self.nonce);
        SasAttendu {
            pair_id: self.pair_id,
            pair_key: self.pair_key,
            code,
            local: false,
            pair: false,
        }
    }
}

/// État « sas attendu » : le code est affiché, aucune confirmation n'est acquise.
///
/// Tant que les deux confirmations ne sont pas acquises, [`SasAttendu::appairer`]
/// rend `None` : rien ne peut être écrit au registre.
#[derive(Debug, Clone)]
pub struct SasAttendu {
    pair_id: DeviceId,
    pair_key: VerifyingKey,
    code: String,
    local: bool,
    pair: bool,
}

impl SasAttendu {
    /// Le code court à comparer des deux côtés.
    pub fn code(&self) -> &str {
        &self.code
    }

    /// Confirmation **locale** : l'humain confirme que le code affiché des deux côtés
    /// est le même. Seule, elle ne suffit pas.
    pub fn confirmer_local(&mut self) {
        self.local = true;
    }

    /// Confirmation **du pair** : l'autre côté a confirmé le même code (reçue par le
    /// transport, hors de ce crate). Seule, elle ne suffit pas non plus.
    pub fn confirmer_pair(&mut self) {
        self.pair = true;
    }

    pub fn local_confirme(&self) -> bool {
        self.local
    }

    pub fn pair_confirme(&self) -> bool {
        self.pair
    }

    /// Les deux confirmations sont-elles acquises ?
    pub fn est_complet(&self) -> bool {
        self.local && self.pair
    }

    /// Termine l'appairage : `Some` seulement si les deux confirmations sont acquises,
    /// sinon `None` — l'état reste en attente et rien n'est écrit.
    pub fn appairer(&self) -> Option<Admission> {
        if !self.est_complet() {
            return None;
        }
        Some(Admission {
            id: self.pair_id.clone(),
            key: self.pair_key,
            role: AnchorKeyRole::Shared,
        })
    }
}

/// Le lien entre deux appareils appairés, choisi **des deux côtés**.
///
/// - [`Lien::Moi`] : deux appareils de la même personne (rôle `Ownership`) ;
/// - [`Lien::Autre`] : l'appareil de quelqu'un d'autre (rôle `Shared`).
///
/// Sur le fil : `"moi"` / `"autre"`. Le lien final ne vaut `Moi` que si les DEUX
/// côtés ont dit `moi` ([`Lien::combiner`]) : un côté seul ne peut jamais
/// s'octroyer la propriété. Par défaut (champ absent, pair plus ancien) : `Autre`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Lien {
    /// Mes deux appareils.
    Moi,
    /// L'appareil de quelqu'un d'autre.
    #[default]
    Autre,
}

impl Lien {
    /// Le lien final d'un appairage : `Moi` si et seulement si les deux côtés ont
    /// dit `Moi`, sinon `Autre`. Symétrique.
    pub fn combiner(self, autre: Lien) -> Lien {
        if self == Lien::Moi && autre == Lien::Moi {
            Lien::Moi
        } else {
            Lien::Autre
        }
    }

    /// Le rôle inscrit au registre : `Moi` → `Ownership`, `Autre` → `Shared`.
    pub fn role(self) -> AnchorKeyRole {
        match self {
            Lien::Moi => AnchorKeyRole::Ownership,
            Lien::Autre => AnchorKeyRole::Shared,
        }
    }
}

/// L'inscription produite par un appairage complet, **avant toute écriture**.
///
/// Cette valeur ne touche pas le registre ; [`Admission::inscrire`] est le seul point
/// qui l'écrit, et elle n'existe qu'une fois les deux confirmations acquises.
///
/// Le lien ([`Lien`]) n'est pas un champ à part : il est porté par `role`, qui en
/// découle exactement (`Moi` ⇔ `Ownership`) — voir [`Admission::lien`]. Ainsi un
/// appelant qui construit `Admission { id, key, role }` reste valide, et le lien
/// ne peut jamais contredire le rôle inscrit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Admission {
    pub id: DeviceId,
    pub key: VerifyingKey,
    pub role: AnchorKeyRole,
}

impl Admission {
    /// Le lien de cet appairage, lu sur le rôle : `Ownership` → [`Lien::Moi`],
    /// tout autre rôle → [`Lien::Autre`].
    pub fn lien(&self) -> Lien {
        match self.role {
            AnchorKeyRole::Ownership => Lien::Moi,
            _ => Lien::Autre,
        }
    }

    /// Fixe le lien : le rôle en découle ([`Lien::role`]).
    pub fn avec_lien(mut self, lien: Lien) -> Self {
        self.role = lien.role();
        self
    }

    /// Écrit le pair au registre, `Active`, au moment présent. Le registre est un
    /// handle partagé ([`Peers::admit`] prend `&self`) : l'inscription est vue
    /// aussitôt par tous ses clones (listener, porte).
    ///
    /// **Préférer [`Admission::inscrire_verifie`]**, qui rend le refus. Celle-ci
    /// n'écrase jamais une autre clé : si `id` porte déjà une clé différente
    /// (collision d'identifiant), elle n'écrit **rien** et le refus n'est visible
    /// que dans [`Peers::ecrasements_refuses`] — l'appelant croirait à tort que
    /// le pair est inscrit.
    pub fn inscrire(&self, peers: &Peers) {
        peers.admit(&self.id, self.key, self.role, Utc::now());
    }

    /// Écrit le pair au registre comme [`Admission::inscrire`], mais rend
    /// [`PeerError::IdentifiantPris`] si `id` porte déjà une **autre** clé
    /// (active ou révoquée) ; le registre reste alors intact. Réinscrire la même
    /// clé (ré-appairage) réussit et met à jour le rôle.
    pub fn inscrire_verifie(&self, peers: &Peers) -> Result<(), PeerError> {
        peers.try_admit(&self.id, self.key, self.role, Utc::now())
    }
}

/// La machine d'états d'un appairage, **épuisable** : un `match` sur les trois
/// variantes couvre tout.
///
/// ```text
/// Rencontre ──afficher──▶ SasAttendu ──confirmer_local + confirmer_pair──▶ Appaire
/// ```
/// Un abandon à n'importe quel état ne laisse rien : seul [`Appairage::Appaire`]
/// porte l'[`Admission`], et même elle n'écrit le registre que via [`Admission::inscrire`].
#[derive(Debug, Clone)]
pub enum Appairage {
    /// L'autre clé est connue, le code court n'est pas encore dérivé.
    Rencontre(Rencontre),
    /// Le code court s'affiche ; on attend les deux confirmations.
    SasAttendu(SasAttendu),
    /// Appairage complet, prêt à être inscrit au registre.
    Appaire(Admission),
}

impl Appairage {
    /// Démarre une rencontre. Rien n'est écrit au registre.
    pub fn rencontre(
        moi: VerifyingKey,
        pair_id: DeviceId,
        pair_key: VerifyingKey,
        nonce: Vec<u8>,
    ) -> Self {
        Self::Rencontre(Rencontre::nouvelle(moi, pair_id, pair_key, nonce))
    }

    /// `Rencontre` → `SasAttendu` ; tout autre état reste inchangé.
    pub fn afficher(self) -> Self {
        match self {
            Self::Rencontre(r) => Self::SasAttendu(r.afficher()),
            other => other,
        }
    }

    /// Confirmation locale (no-op hors de l'état `SasAttendu`).
    pub fn confirmer_local(&mut self) {
        if let Self::SasAttendu(s) = self {
            s.confirmer_local();
        }
    }

    /// Confirmation du pair (no-op hors de l'état `SasAttendu`).
    pub fn confirmer_pair(&mut self) {
        if let Self::SasAttendu(s) = self {
            s.confirmer_pair();
        }
    }

    /// Transition terminale : `Appaire` seulement si les deux confirmations sont
    /// acquises ; sinon l'état reste `SasAttendu`, rien n'est écrit.
    pub fn appairer(self) -> Self {
        match self {
            Self::SasAttendu(s) => match s.appairer() {
                Some(a) => Self::Appaire(a),
                None => Self::SasAttendu(s),
            },
            other => other,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{DeviceId, Peers};
    use ed25519_dalek::SigningKey;
    use this_vfs::layout::{AnchorKeyRole, AnchorKeyState};

    fn key(n: u8) -> VerifyingKey {
        SigningKey::from_bytes(&[n; 32]).verifying_key()
    }
    fn dev(s: &str) -> DeviceId {
        DeviceId::parse(s).unwrap()
    }

    #[test]
    fn a_sas_symetrique_ordre_inverse_identique() {
        // Deux calculs indépendants, clés données dans l'ordre inverse : même code.
        let code_ab = sas(&key(1), &key(2), b"nonce-partage");
        let code_ba = sas(&key(2), &key(1), b"nonce-partage");
        assert_eq!(code_ab, code_ba);
    }

    #[test]
    fn b_sas_nonce_different_donne_code_different() {
        assert_ne!(
            sas(&key(1), &key(2), b"nonce-1"),
            sas(&key(1), &key(2), b"nonce-2")
        );
    }

    #[test]
    fn c_sas_cle_substituee_donne_code_different() {
        assert_ne!(
            sas(&key(1), &key(2), b"nonce"),
            sas(&key(1), &key(3), b"nonce")
        );
    }

    #[test]
    fn sas_a_exactement_six_chiffres_et_deterministe() {
        let code = sas(&key(1), &key(2), b"nonce");
        assert_eq!(code.len(), 6, "{code}");
        assert!(code.bytes().all(|b| b.is_ascii_digit()), "{code}");
        assert_eq!(code, sas(&key(1), &key(2), b"nonce"));
    }

    #[test]
    fn d_abandon_avant_confirmation_du_pair_ne_touche_pas_le_registre() {
        let peers = Peers::new();
        let id = dev("device-aaaaaaaa");
        let mut app = Appairage::rencontre(key(1), id, key(2), b"nonce".to_vec());
        app = app.afficher();
        app.confirmer_local();
        // Abandon : pas de confirmation du pair → rien n'a jamais été écrit.
        assert!(peers.is_empty());
        // Même la transition terminale ne produit rien sans le pair.
        let app = app.appairer();
        assert!(matches!(app, Appairage::SasAttendu(_)));
        assert!(peers.is_empty());
    }

    #[test]
    fn e_confirmation_locale_seule_ne_suffit_pas() {
        let peers = Peers::new();
        let id = dev("device-aaaaaaaa");
        let mut sas_attendu =
            Rencontre::nouvelle(key(1), id.clone(), key(2), b"nonce".to_vec()).afficher();
        sas_attendu.confirmer_local();
        // L'état reste en attente, la clé n'est pas enregistrée.
        assert!(sas_attendu.local_confirme());
        assert!(!sas_attendu.pair_confirme());
        assert!(!sas_attendu.est_complet());
        assert_eq!(sas_attendu.appairer(), None);
        assert!(peers.is_empty());
    }

    #[test]
    fn f_deux_confirmations_inscrivent_le_pair() {
        let peers = Peers::new();
        let id = dev("device-aaaaaaaa");
        let mut sas_attendu =
            Rencontre::nouvelle(key(1), id.clone(), key(2), b"nonce".to_vec()).afficher();
        assert_eq!(sas_attendu.code().len(), 6);
        sas_attendu.confirmer_local();
        sas_attendu.confirmer_pair();
        assert!(sas_attendu.est_complet());
        let admission = sas_attendu.appairer().unwrap();
        assert_eq!(admission.id, id);
        assert_eq!(admission.role, AnchorKeyRole::Shared);
        admission.inscrire(&peers);
        assert_eq!(peers.len(), 1);
        assert_eq!(peers.state(&id), Some(AnchorKeyState::Active));
        assert_eq!(peers.role(&id), Some(AnchorKeyRole::Shared));
    }

    // Collision d'identifiant : une autre clé sous l'identifiant déjà inscrit.
    #[test]
    fn inscrire_n_ecrase_jamais_une_autre_cle() {
        let peers = Peers::new();
        let id = dev("device-aaaaaaaa");
        let lea = Admission {
            id: id.clone(),
            key: key(2),
            role: AnchorKeyRole::Ownership,
        };
        lea.inscrire_verifie(&peers).unwrap();
        let intrus = Admission {
            id: id.clone(),
            key: key(3),
            role: AnchorKeyRole::Shared,
        };
        assert_eq!(
            intrus.inscrire_verifie(&peers),
            Err(PeerError::IdentifiantPris(id.to_string()))
        );
        intrus.inscrire(&peers);
        assert_eq!(peers.id_de_cle(&key(2)), Some(id.clone()));
        assert_eq!(peers.id_de_cle(&key(3)), None);
        assert_eq!(peers.role(&id), Some(AnchorKeyRole::Ownership));
        assert_eq!(peers.ecrasements_refuses(), vec![id.clone()]);

        // La même clé, autre rôle : passe (ré-appairage).
        let reappairage = lea.avec_lien(Lien::Autre);
        reappairage.inscrire_verifie(&peers).unwrap();
        assert_eq!(peers.role(&id), Some(AnchorKeyRole::Shared));
    }

    #[test]
    fn lien_moi_seulement_si_les_deux_disent_moi() {
        assert_eq!(Lien::Moi.combiner(Lien::Moi), Lien::Moi);
        assert_eq!(Lien::Moi.combiner(Lien::Autre), Lien::Autre);
        assert_eq!(Lien::Autre.combiner(Lien::Moi), Lien::Autre);
        assert_eq!(Lien::Autre.combiner(Lien::Autre), Lien::Autre);
        assert_eq!(Lien::default(), Lien::Autre);
        assert_eq!(Lien::Moi.role(), AnchorKeyRole::Ownership);
        assert_eq!(Lien::Autre.role(), AnchorKeyRole::Shared);
        assert_eq!(serde_json::to_string(&Lien::Moi).unwrap(), r#""moi""#);
        assert_eq!(serde_json::to_string(&Lien::Autre).unwrap(), r#""autre""#);
        let a = Admission {
            id: dev("device-aaaaaaaa"),
            key: key(2),
            role: AnchorKeyRole::Shared,
        };
        assert_eq!(a.lien(), Lien::Autre);
        let a = a.avec_lien(Lien::Moi);
        assert_eq!((a.lien(), a.role), (Lien::Moi, AnchorKeyRole::Ownership));
    }

    #[test]
    fn g_etats_sont_epuisables_par_match() {
        // Le compilateur exige que les trois variantes soient couvertes.
        let app = Appairage::rencontre(key(1), dev("device-aaaaaaaa"), key(2), b"n".to_vec());
        let label = match &app {
            Appairage::Rencontre(_) => "rencontre",
            Appairage::SasAttendu(_) => "sas",
            Appairage::Appaire(_) => "appaire",
        };
        assert_eq!(label, "rencontre");
    }
}
