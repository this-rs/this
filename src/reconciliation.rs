//! Contrat de réconciliation de l'autorité durable — composant RÉUTILISABLE.
//!
//! Ce module extrait, hors des tests, la chaîne d'autorité mesurée dans
//! `tests/h3_v2_chaine_autorite.rs` : un enregistrement original versionné et un
//! verdict de réconciliation qui compare la résolution à CET enregistrement (pas au
//! seul résultat de `resoudre_reference`).
//!
//! # Taxonomie préservée (K / F / V / G / E / B)
//! - **K** : identité logique de l'opération, construite par le consommateur, stable
//!   entre tentatives d'une même opération. Jamais lisible dans la référence.
//! - **F** : empreinte de transformation (`TransformationPrete::empreinte()`) — signature
//!   de CONTENU, PAS une identité. Ne pas dédupliquer par empreinte.
//! - **V** : versions des dépendances (hors périmètre de ce module, portées par l'appelant).
//! - **G** : génération de tentative (`this_harness::attribution::Attribution::reserver`).
//! - **E** : événement physique engagé — l'estampille `(auteur, horloge)` = writer_id + HLC,
//!   portée par les octets de la `ReferenceTentative`.
//! - **B** : autorité backend (writer_id / auteur du store).
//!
//! La référence (E) porte auteur/horloge/cible/effets, mais NI K, NI F, NI V, NI G :
//! ces quatre relèvent de l'enregistrement du consommateur, et seule la comparaison à
//! l'enregistrement les vérifie.

use obrain_common::types::{NodeId, PropertyKey, Value};
use obrain_core::graph::traits::GraphStore;
use obrain_substrate::{
    IntegriteTentative, ReferenceTentative, ResolutionTentative, SubstrateStore,
};
use this_harness::attribution::RésolutionBackend;

/// Magic des octets d'un enregistrement encodé.
pub const ENREGISTREMENT_MAGIC: [u8; 4] = *b"OBRR";
/// Version courante du format d'encodage.
pub const ENREGISTREMENT_VERSION: u32 = 1;

/// L'enregistrement original durable du consommateur — la chaîne d'autorité.
/// Un SEUL bloc cohérent (pas deux fichiers qui peuvent diverger).
#[derive(Debug, Clone, PartialEq)]
pub struct EnregistrementOriginal {
    pub version: u32,
    /// Identité logique K.
    pub k: String,
    /// Autorité backend B (writer_id du store qui a produit la référence).
    pub backend: u64,
    /// Génération de tentative G.
    pub generation: u64,
    /// Cible originale.
    pub cible: NodeId,
    /// Effets attendus (clé + contenu) — fournis par le consommateur, PAS une constante.
    pub effets_attendus: Vec<(String, i64)>,
    /// Octets de la `ReferenceTentative` (E) — conservés AVANT publication.
    pub reference_bytes: Vec<u8>,
}

/// Verdict de réconciliation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VerdictReconciliation {
    /// Intégral, canonique, contenu conforme → l'effet original est en place.
    Confirme,
    /// Intégral (engagé) mais masqué ET contenu courant conforme → pas incompatible.
    EngageMasqueCompatible,
    /// Contenu courant RELU et DIFFÉRENT de l'attendu (incompatibilité de contenu).
    Incompatible,
    /// Intégrité PARTIELLE : au moins un effet engagé, au moins un non retrouvé.
    /// N'est PAS une incompatibilité de contenu.
    Partielle,
    /// La référence résolue n'est pas celle de l'enregistrement (substitution).
    Substitution,
    /// La liste des effets est incomplète (clé amputée).
    Incomplet,
    /// Jamais engagé ou compacté — jamais une absence définitive.
    Indetermine,
}

impl VerdictReconciliation {
    /// Projection PARTIELLE vers le verdict du backend. Les correspondances non
    /// ambiguës sont projetées : `Confirme` → `EffetConfirme`, `Indetermine` →
    /// `Indetermine` (jamais `AbsenceDefinitive`), `Incompatible` (contenu relu ≠
    /// attendu) → `Incompatible`. Les verdicts NUANCÉS (`EngageMasqueCompatible`,
    /// `Partielle`, `Substitution`, `Incomplet`) renvoient `None` : ils exigent une
    /// DÉCISION EXPLICITE du consommateur et ne doivent jamais être écrasés en
    /// `Incompatible` ni en `EffetConfirme`.
    pub fn vers_backend(&self) -> Option<RésolutionBackend> {
        match self {
            VerdictReconciliation::Confirme => Some(RésolutionBackend::EffetConfirme),
            VerdictReconciliation::Indetermine => Some(RésolutionBackend::Indetermine),
            VerdictReconciliation::Incompatible => Some(RésolutionBackend::Incompatible),
            _ => None,
        }
    }
}

/// Erreur de décodage d'un enregistrement — nommée, jamais de panique.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DecodageEnregistrement {
    /// Troncature, dépassement, clé non UTF-8, octets excédentaires.
    Malforme(String),
    /// Version inconnue : l'enregistrement est refusé, pas ignoré.
    VersionInconnue(u32),
}

fn lire_u32_le(o: &[u8], pos: &mut usize) -> Result<u32, DecodageEnregistrement> {
    let fin = pos
        .checked_add(4)
        .ok_or_else(|| DecodageEnregistrement::Malforme("dépassement u32".into()))?;
    let t = o
        .get(*pos..fin)
        .ok_or_else(|| DecodageEnregistrement::Malforme("troncature u32".into()))?;
    *pos = fin;
    let mut b = [0u8; 4];
    b.copy_from_slice(t);
    Ok(u32::from_le_bytes(b))
}

fn lire_u64_le(o: &[u8], pos: &mut usize) -> Result<u64, DecodageEnregistrement> {
    let fin = pos
        .checked_add(8)
        .ok_or_else(|| DecodageEnregistrement::Malforme("dépassement u64".into()))?;
    let t = o
        .get(*pos..fin)
        .ok_or_else(|| DecodageEnregistrement::Malforme("troncature u64".into()))?;
    *pos = fin;
    let mut b = [0u8; 8];
    b.copy_from_slice(t);
    Ok(u64::from_le_bytes(b))
}

fn lire_chaine(o: &[u8], pos: &mut usize) -> Result<String, DecodageEnregistrement> {
    let n = lire_u32_le(o, pos)? as usize;
    let fin = pos
        .checked_add(n)
        .ok_or_else(|| DecodageEnregistrement::Malforme("troncature chaîne".into()))?;
    let t = o
        .get(*pos..fin)
        .ok_or_else(|| DecodageEnregistrement::Malforme("troncature chaîne".into()))?;
    *pos = fin;
    String::from_utf8(t.to_vec()).map_err(|_| DecodageEnregistrement::Malforme("clé non UTF-8".into()))
}

impl EnregistrementOriginal {
    /// Encodage versionné en un SEUL bloc cohérent :
    /// `magic(4) | version(4) | k_len(4)+k | backend(8) | generation(8) | cible(8) |
    /// n_effets(4) | [k_len(4)+k | valeur(8)]*n | ref_len(8) | reference_bytes`.
    pub fn encoder(&self) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&ENREGISTREMENT_MAGIC);
        out.extend_from_slice(&self.version.to_le_bytes());
        out.extend_from_slice(&(self.k.len() as u32).to_le_bytes());
        out.extend_from_slice(self.k.as_bytes());
        out.extend_from_slice(&self.backend.to_le_bytes());
        out.extend_from_slice(&self.generation.to_le_bytes());
        out.extend_from_slice(&self.cible.0.to_le_bytes());
        out.extend_from_slice(&(self.effets_attendus.len() as u32).to_le_bytes());
        for (cle, val) in &self.effets_attendus {
            out.extend_from_slice(&(cle.len() as u32).to_le_bytes());
            out.extend_from_slice(cle.as_bytes());
            out.extend_from_slice(&val.to_le_bytes());
        }
        out.extend_from_slice(&(self.reference_bytes.len() as u64).to_le_bytes());
        out.extend_from_slice(&self.reference_bytes);
        out
    }

    /// Décodage CONTRÔLÉ : refuse magic invalide, version inconnue, troncature,
    /// clé non UTF-8, octets excédentaires. Ne panique sur aucune entrée.
    pub fn decoder(octets: &[u8]) -> Result<Self, DecodageEnregistrement> {
        if octets.len() < 4 {
            return Err(DecodageEnregistrement::Malforme("trop court".into()));
        }
        if octets[..4] != ENREGISTREMENT_MAGIC {
            return Err(DecodageEnregistrement::Malforme("magic invalide".into()));
        }
        let mut pos = 4usize;
        let version = lire_u32_le(octets, &mut pos)?;
        if version != ENREGISTREMENT_VERSION {
            return Err(DecodageEnregistrement::VersionInconnue(version));
        }
        let k = lire_chaine(octets, &mut pos)?;
        let backend = lire_u64_le(octets, &mut pos)?;
        let generation = lire_u64_le(octets, &mut pos)?;
        let cible = NodeId(lire_u64_le(octets, &mut pos)?);
        let n = lire_u32_le(octets, &mut pos)? as usize;
        let mut effets_attendus = Vec::with_capacity(n);
        for _ in 0..n {
            let cle = lire_chaine(octets, &mut pos)?;
            let val = lire_u64_le(octets, &mut pos)? as i64;
            effets_attendus.push((cle, val));
        }
        let ref_len = lire_u64_le(octets, &mut pos)? as usize;
        let fin = pos
            .checked_add(ref_len)
            .ok_or_else(|| DecodageEnregistrement::Malforme("troncature référence".into()))?;
        let reference_bytes = octets
            .get(pos..fin)
            .ok_or_else(|| DecodageEnregistrement::Malforme("troncature référence".into()))?
            .to_vec();
        pos = fin;
        if pos != octets.len() {
            return Err(DecodageEnregistrement::Malforme("octets excédentaires".into()));
        }
        Ok(EnregistrementOriginal {
            version,
            k,
            backend,
            generation,
            cible,
            effets_attendus,
            reference_bytes,
        })
    }
}

/// Réconcilie la résolution contre l'ENREGISTREMENT ORIGINAL — pas contre le seul
/// résultat de `resoudre_reference`.
///
/// Ordre des vérifications : complétude des effets (L1, amputation), correspondance
/// de la référence (L2, substitution), cohérence cible/autorité (métadonnées ↔
/// référence), autorité résolvante, puis intégrité + état courant + compatibilité.
pub fn reconcilier(
    enr: &EnregistrementOriginal,
    reference: &ReferenceTentative,
    res: &ResolutionTentative,
    store: &SubstrateStore,
) -> VerdictReconciliation {
    // 1. Complétude des effets (L1).
    let attendus: Vec<&str> = enr.effets_attendus.iter().map(|(k, _)| k.as_str()).collect();
    if reference.effets() != attendus.as_slice() {
        return VerdictReconciliation::Incomplet;
    }
    // 2. Correspondance de la référence (L2).
    if reference.to_bytes() != enr.reference_bytes {
        return VerdictReconciliation::Substitution;
    }
    // 3. Cohérence cible / autorité (métadonnées ↔ référence).
    if reference.cible() != enr.cible || res.cible != enr.cible {
        return VerdictReconciliation::Incompatible;
    }
    if reference.auteur() != enr.backend {
        return VerdictReconciliation::Incompatible;
    }
    // 4. Autorité résolvante.
    if res.auteur_resolvant != enr.backend {
        return VerdictReconciliation::Incompatible;
    }
    // 5. Intégrité + état courant + compatibilité.
    match res.integrite {
        IntegriteTentative::Indeterminee => VerdictReconciliation::Indetermine,
        IntegriteTentative::Partielle => VerdictReconciliation::Partielle,
        IntegriteTentative::Integrale => {
            let tous_canoniques = res.par_cle.iter().all(|e| {
                e.estampille_courante == Some((enr.backend, reference.horloge()))
            });
            // Contenu à TROIS états : conforme / incompatible (valeur relue ≠ attendue)
            // / illisible (lecture absente ou en erreur — PAS une preuve d'incompatibilité).
            let mut incompatible = false;
            let mut illisible = false;
            for (cle, attendu) in &enr.effets_attendus {
                match lire_i64(store, enr.cible, cle) {
                    Some(v) if v == *attendu => {}
                    Some(_) => incompatible = true,
                    None => illisible = true,
                }
            }
            if incompatible {
                VerdictReconciliation::Incompatible
            } else if illisible {
                // Contenu illisible : ni confirmé, ni incompatible → indéterminé.
                VerdictReconciliation::Indetermine
            } else if tous_canoniques {
                VerdictReconciliation::Confirme
            } else {
                VerdictReconciliation::EngageMasqueCompatible
            }
        }
    }
}

fn lire_i64(store: &SubstrateStore, id: NodeId, cle: &str) -> Option<i64> {
    store.get_node_property(id, &PropertyKey::new(cle)).and_then(|v| match v {
        Value::Int64(n) => Some(n),
        _ => None,
    })
}
