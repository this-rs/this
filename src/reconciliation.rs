//! Contrat de réconciliation de l'autorité durable — composant RÉUTILISABLE.
//!
//! Ce module extrait, hors des tests, la chaîne d'autorité mesurée dans
//! `tests/h3_v2_chaine_autorite.rs` : un enregistrement original versionné et un
//! verdict de réconciliation qui compare la résolution à CET enregistrement (pas au
//! seul résultat de `resoudre_reference`).
//!
//! # Taxonomie préservée (K / F / V / G / E / D / W)
//! - **K** : identité logique de l'opération, un **u128** (compatible
//!   `ObservationId`), construite par le consommateur, stable entre tentatives.
//!   Jamais lisible dans la référence.
//! - **F** : empreinte de transformation (`TransformationPrete::empreinte()`) — signature
//!   de CONTENU, PAS une identité. Ne pas dédupliquer par empreinte.
//! - **V** : versions des dépendances (hors périmètre de ce module, portées par l'appelant).
//! - **G** : génération de tentative (`this_harness::attribution::Attribution::reserver`).
//! - **E** : événement physique engagé — l'estampille `(auteur, horloge)` = writer_id + HLC,
//!   portée par les octets de la `ReferenceTentative`.
//! - **D** : domaine de stockage (hors-bande) — l'identité de la BASE. N'apparaît
//!   NULLE PART dans la référence : le consommateur la conserve hors-bande.
//! - **W** : écrivain (writer_id / auteur du store). PAS un domaine : deux bases
//!   peuvent partager le même `writer.id` (collision HLC, obrain 62ecd940).
//!
//! La référence (E) porte auteur/horloge/cible/effets, mais NI K, NI F, NI V, NI G,
//! NI D (domaine de stockage) : ces cinq relèvent de l'enregistrement du consommateur,
//! et seule la comparaison à l'enregistrement les vérifie. La référence ne constitue
//! JAMAIS sa propre preuve d'origine : `writer.id` ne discrimine pas les bases.
//!
//! # Delta EffetResolu (a5447cc6 → c26824ea)
//! Le contenu courant des effets est CONSOMMÉ depuis l'observation canonique
//! `EffetResolu` (même lecture sous le verrou de nœud que `estampille_courante`) —
//! plus aucune seconde lecture indépendante (`lire_i64` / `get_node_property`).
//! L'association se fait PAR CLÉ, jamais par position. Les valeurs d'effet sont
//! encodées TAGGÉES (Int64 / Float64 / Bytes) : aucune conversion arbitraire vers
//! Int64.
//!
//! # Delta collision HLC + état courant (secondaire, 62ecd940 / 6535ddbd / c26824ea)
//! - Le DOMAINE DE STOCKAGE (`domaine`) est une identité DISTINCTE de l'écrivain
//!   (`writer_id`), conservée HORS-BANDE : `reconcilier` ne peut PAS la prouver
//!   depuis la référence (`writer.id` ne discrimine pas les bases — collision HLC).
//! - L'état courant est lu depuis `EffetResolu.etat_courant` (`EtatPropriete`), qui
//!   distingue EXPLICITEMENT `Presente` / `Absente` (clé jamais écrite) /
//!   `Supprimee` (tombstone) / `RessourceInexistante` (nœud mort). Une erreur de
//!   lecture/corruption ne passe JAMAIS par un état d'absence : elle remonte en
//!   `Err` via `resoudre_reference` (6535ddbd), jamais convertie en absence.

use std::sync::Arc;

use obrain_common::types::{NodeId, Value};
use obrain_substrate::{
    EtatPropriete, IntegriteTentative, ReferenceTentative, ResolutionTentative, SubstrateStore,
};
use this_harness::attribution::{Attribution, RésolutionBackend, Tentative};

/// Magic des octets d'un enregistrement encodé.
pub const ENREGISTREMENT_MAGIC: [u8; 4] = *b"OBRR";
/// Version courante du format d'encodage.
///
/// v5 : K en **u128** (arbitrage K/ObservationId), champ `domaine` (domaine de
/// stockage hors-bande), `ecrivain` (writer_id, ex-`backend`), effets TAGGÉS
/// (Int64 / Float64 / Bytes). Les enregistrements v1..=v4 sont refusés
/// (`VersionInconnue`), jamais réinterprétés.
pub const ENREGISTREMENT_VERSION: u32 = 5;

/// Balises de type pour l'encodage d'une valeur d'effet. Seuls Int64, Float64 et
/// Bytes sont représentés : tout autre variant de `Value` est REFUSÉ (jamais
/// converti ni réduit à Int64).
const TAG_VALEUR_INT64: u8 = 0x01;
const TAG_VALEUR_FLOAT64: u8 = 0x02;
const TAG_VALEUR_BYTES: u8 = 0x03;

/// L'enregistrement original durable du consommateur — la chaîne d'autorité.
/// Un SEUL bloc cohérent (pas deux fichiers qui peuvent diverger).
#[derive(Debug, Clone, PartialEq)]
pub struct EnregistrementOriginal {
    pub version: u32,
    /// Identité logique K — u128, compatible `ObservationId` (`from_u128`/`as_u128`).
    pub k: u128,
    /// Domaine de stockage D — l'identité de la BASE (hors-bande). N'apparaît
    /// NULLE PART dans la référence : le consommateur la conserve hors-bande.
    /// `reconcilier` ne la vérifie PAS (indémontrable depuis la référence) :
    /// la preuve du domaine est la responsabilité du CONSOMMATEUR.
    pub domaine: String,
    /// Écrivain W (writer_id / auteur du store qui a produit la référence).
    /// PAS un domaine de stockage : deux bases peuvent partager le même
    /// `writer.id` (collision HLC). Ne fusionnez pas les deux identités.
    pub ecrivain: u64,
    /// Génération de tentative G.
    pub generation: u64,
    /// Identifiant de décision (decision_id) — distinct de K : K est l'identité
    /// LOGIQUE stable entre tentatives, decision_id est l'identifiant physique
    /// du tour. Sa durabilité (unicité + stabilité après redémarrage) relève du
    /// contrat du consommateur, pas de ce codec.
    pub decision_id: u128,
    /// Autorité CONSOMMATEUR (l'instance d'`Attribution` qui a réservé), distincte
    /// de l'autorité BACKEND (writer_id) portée par la référence.
    pub autorite_consommateur: u64,
    /// Cible originale.
    pub cible: NodeId,
    /// Effets attendus (clé + contenu) — fournis par le consommateur, PAS une constante.
    /// Le contenu est une `Value` (Int64 / Float64 / Bytes), jamais réduit à Int64.
    pub effets_attendus: Vec<(String, Value)>,
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

/// Erreur d'encodage d'un enregistrement — nommée, jamais de panique.
#[derive(Debug, Clone, PartialEq)]
pub enum EncodageEnregistrement {
    /// Valeur d'effet d'un type que le format ne représente pas (seuls Int64,
    /// Float64 et Bytes sont encodés). Refusée, jamais convertie.
    ValeurNonRepresentable(String),
}

/// Erreur de décodage d'un enregistrement — nommée, jamais de panique.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DecodageEnregistrement {
    /// Troncature, dépassement, clé non UTF-8, octets excédentaires.
    Malforme(String),
    /// Version inconnue : l'enregistrement est refusé, pas ignoré.
    VersionInconnue(u32),
    /// Balise de valeur inconnue : le format ne décode que Int64 / Float64 / Bytes.
    ValeurNonRepresentable(String),
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

fn lire_u128_le(o: &[u8], pos: &mut usize) -> Result<u128, DecodageEnregistrement> {
    let fin = pos
        .checked_add(16)
        .ok_or_else(|| DecodageEnregistrement::Malforme("dépassement u128".into()))?;
    let t = o
        .get(*pos..fin)
        .ok_or_else(|| DecodageEnregistrement::Malforme("troncature u128".into()))?;
    *pos = fin;
    let mut b = [0u8; 16];
    b.copy_from_slice(t);
    Ok(u128::from_le_bytes(b))
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

/// Nom du variant d'une `Value` — pour nommer précisément le type refusé.
fn nom_variant(v: &Value) -> &'static str {
    match v {
        Value::Null => "Null",
        Value::Bool(_) => "Bool",
        Value::Int64(_) => "Int64",
        Value::Float64(_) => "Float64",
        Value::String(_) => "String",
        Value::Bytes(_) => "Bytes",
        Value::Timestamp(_) => "Timestamp",
        Value::Date(_) => "Date",
        Value::Time(_) => "Time",
        Value::Duration(_) => "Duration",
        Value::ZonedDatetime(_) => "ZonedDatetime",
        Value::List(_) => "List",
        Value::Map(_) => "Map",
        Value::Vector(_) => "Vector",
        Value::Path { .. } => "Path",
    }
}

/// Encode une valeur d'effet TAGGÉE (Int64 / Float64 / Bytes). Tout autre type
/// est REFUSÉ — jamais converti.
fn encoder_valeur(v: &Value, out: &mut Vec<u8>) -> Result<(), EncodageEnregistrement> {
    match v {
        Value::Int64(n) => {
            out.push(TAG_VALEUR_INT64);
            out.extend_from_slice(&n.to_le_bytes());
        }
        Value::Float64(f) => {
            out.push(TAG_VALEUR_FLOAT64);
            out.extend_from_slice(&f.to_le_bytes());
        }
        Value::Bytes(b) => {
            out.push(TAG_VALEUR_BYTES);
            out.extend_from_slice(&(b.len() as u32).to_le_bytes());
            out.extend_from_slice(b);
        }
        autre => {
            return Err(EncodageEnregistrement::ValeurNonRepresentable(
                nom_variant(autre).to_string(),
            ));
        }
    }
    Ok(())
}

/// Décode une valeur d'effet TAGGÉE (Int64 / Float64 / Bytes). Balise inconnue →
/// refus NOMÉ, jamais de conversion arbitraire.
fn lire_valeur(o: &[u8], pos: &mut usize) -> Result<Value, DecodageEnregistrement> {
    let tag = o
        .get(*pos)
        .copied()
        .ok_or_else(|| DecodageEnregistrement::Malforme("troncature balise valeur".into()))?;
    *pos += 1;
    match tag {
        TAG_VALEUR_INT64 => {
            let n = lire_u64_le(o, pos)?;
            Ok(Value::Int64(n as i64))
        }
        TAG_VALEUR_FLOAT64 => {
            let bits = lire_u64_le(o, pos)?;
            Ok(Value::Float64(f64::from_bits(bits)))
        }
        TAG_VALEUR_BYTES => {
            let n = lire_u32_le(o, pos)? as usize;
            let fin = pos
                .checked_add(n)
                .ok_or_else(|| DecodageEnregistrement::Malforme("dépassement bytes".into()))?;
            let t = o
                .get(*pos..fin)
                .ok_or_else(|| DecodageEnregistrement::Malforme("troncature bytes".into()))?;
            *pos = fin;
            Ok(Value::Bytes(Arc::from(t)))
        }
        autre => Err(DecodageEnregistrement::ValeurNonRepresentable(format!(
            "balise {autre}"
        ))),
    }
}

impl EnregistrementOriginal {
    /// Liaison effective à la décision et à la réservation RÉELLES : vérifie que
    /// `generation`, `decision_id` et `autorite_consommateur` de l'enregistrement
    /// correspondent au descripteur de tentative qui l'a produit. K n'est PAS
    /// lisible dans la référence : sa cohérence est le contrat du CONSOMMATEUR ;
    /// ici on ne vérifie que sa présence (un K nul — 0, jamais émis — est une
    /// corruption valide détectée).
    pub fn verifier_liaison(&self, tentative: &Tentative) -> bool {
        self.generation == tentative.generation()
            && self.decision_id == tentative.decision_id()
            && self.autorite_consommateur == tentative.autorite()
            && self.k != 0
    }

    /// Encodage versionné en un SEUL bloc cohérent :
    /// `magic(4) | version(4) | k(16) | domaine_len(4)+domaine | ecrivain(8) |
    /// generation(8) | decision_id(16) | autorite_consommateur(8) | cible(8) |
    /// n_effets(4) | [cle_len(4)+cle | valeur_tag(1)+payload]*n | ref_len(8) |
    /// reference_bytes`.
    pub fn encoder(&self) -> Result<Vec<u8>, EncodageEnregistrement> {
        let mut out = Vec::new();
        out.extend_from_slice(&ENREGISTREMENT_MAGIC);
        out.extend_from_slice(&self.version.to_le_bytes());
        out.extend_from_slice(&self.k.to_le_bytes());
        out.extend_from_slice(&(self.domaine.len() as u32).to_le_bytes());
        out.extend_from_slice(self.domaine.as_bytes());
        out.extend_from_slice(&self.ecrivain.to_le_bytes());
        out.extend_from_slice(&self.generation.to_le_bytes());
        out.extend_from_slice(&self.decision_id.to_le_bytes());
        out.extend_from_slice(&self.autorite_consommateur.to_le_bytes());
        out.extend_from_slice(&self.cible.0.to_le_bytes());
        out.extend_from_slice(&(self.effets_attendus.len() as u32).to_le_bytes());
        for (cle, val) in &self.effets_attendus {
            out.extend_from_slice(&(cle.len() as u32).to_le_bytes());
            out.extend_from_slice(cle.as_bytes());
            encoder_valeur(val, &mut out)?;
        }
        out.extend_from_slice(&(self.reference_bytes.len() as u64).to_le_bytes());
        out.extend_from_slice(&self.reference_bytes);
        Ok(out)
    }

    /// Décodage CONTRÔLÉ : refuse magic invalide, version inconnue, troncature,
    /// clé non UTF-8, balise de valeur inconnue, octets excédentaires. Ne panique
    /// sur aucune entrée.
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
        let k = lire_u128_le(octets, &mut pos)?;
        let domaine = lire_chaine(octets, &mut pos)?;
        let ecrivain = lire_u64_le(octets, &mut pos)?;
        let generation = lire_u64_le(octets, &mut pos)?;
        let decision_id = lire_u128_le(octets, &mut pos)?;
        let autorite_consommateur = lire_u64_le(octets, &mut pos)?;
        let cible = NodeId(lire_u64_le(octets, &mut pos)?);
        let n = lire_u32_le(octets, &mut pos)? as usize;
        // Borne AVANT toute allocation : chaque effet occupe au moins 9 octets
        // (longueur de clé u32 + clé vide + balise + longueur bytes u32). Sans
        // cette borne, un `n_effets` de 2^32-1 est SYNTAXIQUEMENT valide et fait
        // réserver une mémoire absurde. Mesuré.
        let reste = octets.len() - pos;
        if n > reste / 9 {
            return Err(DecodageEnregistrement::Malforme(format!(
                "n_effets={n} dépasse les {reste} octets disponibles"
            )));
        }
        let mut effets_attendus = Vec::with_capacity(n);
        for _ in 0..n {
            let cle = lire_chaine(octets, &mut pos)?;
            let val = lire_valeur(octets, &mut pos)?;
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
            domaine,
            ecrivain,
            generation,
            decision_id,
            autorite_consommateur,
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
/// de la référence (L2, substitution), cohérence cible/écrivain (métadonnées ↔
/// référence), écrivain résolvant, correspondance résolution ↔ référence PAR CLÉ
/// (substitution de la résolution), puis intégrité + état courant + compatibilité.
///
/// LIMITE (collision HLC, obrain 62ecd940) : cette fonction ne vérifie PAS le
/// domaine de stockage (`enr.domaine`) — la référence ne le porte pas, `res` non
/// plus. La preuve du domaine est HORS-BANDE, responsabilité du consommateur.
pub fn reconcilier(
    enr: &EnregistrementOriginal,
    reference: &ReferenceTentative,
    res: &ResolutionTentative,
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
    // 3. Cohérence cible / ÉCRIVAIN (métadonnées ↔ référence). `auteur()` est un
    //    writer_id (écrivain), PAS un domaine de stockage.
    if reference.cible() != enr.cible || res.cible != enr.cible {
        return VerdictReconciliation::Incompatible;
    }
    if reference.auteur() != enr.ecrivain {
        return VerdictReconciliation::Incompatible;
    }
    // 4. ÉCRIVAIN résolvant (writer_id). Pas une preuve de domaine : deux bases
    //    peuvent partager le même writer.id (collision HLC).
    if res.auteur_resolvant != enr.ecrivain {
        return VerdictReconciliation::Incompatible;
    }
    // 5. Correspondance résolution ↔ référence PAR CLÉ : aucune clé manquante,
    //    supplémentaire ni dupliquée.
    if !cles_correspondent(reference, res) {
        return VerdictReconciliation::Substitution;
    }
    // 6. Intégrité + état courant + compatibilité. L'état courant est lu depuis
    //    `EffetResolu.etat_courant` (même observation canonique que
    //    `estampille_courante`), associé PAR CLÉ.
    match res.integrite {
        IntegriteTentative::Indeterminee => VerdictReconciliation::Indetermine,
        IntegriteTentative::Partielle => VerdictReconciliation::Partielle,
        IntegriteTentative::Integrale => {
            let tous_canoniques = res
                .par_cle
                .iter()
                .all(|e| e.estampille_courante == Some((enr.ecrivain, reference.horloge())));
            // Contenu à TROIS états : conforme / incompatible (valeur courante ≠
            // attendue, sans conversion de type) / illisible (`Absente` /
            // `Supprimee` / `RessourceInexistante` — PAS une preuve
            // d'incompatibilité). Une ERREUR de lecture n'apparaît JAMAIS ici :
            // elle remonte en `Err` via `resoudre_reference` (6535ddbd).
            let mut incompatible = false;
            let mut illisible = false;
            for (cle, attendu) in &enr.effets_attendus {
                // Association PAR CLÉ (pas par position).
                let effet = res.par_cle.iter().find(|e| &e.cle == cle);
                match effet.map(|e| &e.etat_courant) {
                    Some(EtatPropriete::Presente { valeur, .. }) => {
                        if valeur == attendu {
                            // conforme
                        } else {
                            incompatible = true;
                        }
                    }
                    // Tombstone, clé jamais écrite, ou nœud mort : le contenu
                    // courant n'est PAS lisible — ni confirmé, ni incompatible.
                    Some(EtatPropriete::Supprimee(_))
                    | Some(EtatPropriete::Absente)
                    | Some(EtatPropriete::RessourceInexistante)
                    | None => illisible = true,
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

/// Vérifie que `res.par_cle` correspond à `reference` PAR CLÉ : aucune clé
/// manquante, supplémentaire ni dupliquée. Une `resoudre_reference(&reference)`
/// produit exactement une entrée par clé de la référence.
///
/// # Contrat Obrain manquant (nommé précisément)
/// `ResolutionTentative` ne porte NI l'horloge NI aucun identifiant de la
/// `ReferenceTentative` qu'elle a résolue : elle n'expose que `cible`,
/// `auteur_resolvant`, `par_cle` et `integrite`. Cette fonction vérifie la
/// correspondance des CLÉS et des métadonnées, PAS l'identité d'horloge. Aucune
/// garantie n'est fabriquée au-delà de ce qui est vérifiable.
fn cles_correspondent(reference: &ReferenceTentative, res: &ResolutionTentative) -> bool {
    let mut vues: std::collections::HashSet<String> = std::collections::HashSet::new();
    for e in &res.par_cle {
        if !vues.insert(e.cle.clone()) {
            return false; // clé dupliquée
        }
    }
    for cle in reference.effets() {
        if !vues.remove(cle.as_str()) {
            return false; // clé manquante
        }
    }
    vues.is_empty() // false si clé supplémentaire
}

/// Issue du chemin opérationnel de résolution — la vérification de liaison est
/// exécutée AVANT toute transition `Attribution::resoudre`, jamais après.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IssueResolutionOperation {
    /// Liaison refusée : l'enregistrement ne correspond pas à la réservation
    /// réelle (K vide, génération/decision_id/autorité divergents) — tentative
    /// étrangère ou périmée. Aucune transition du cycle n'a eu lieu.
    LiaisonRefusee,
    /// La réconciliation a rendu un verdict NUANCÉ : aucune transition, une
    /// DÉCISION EXPLICITE du consommateur est exigée (jamais d'écrasement en
    /// résolution automatique).
    VerdictNuance(VerdictReconciliation),
    /// Résolution DÉLÉGUÉE à `Attribution::resoudre` ; le booléen est son
    /// résultat (jamais réimplémenté ici).
    Resolu(bool),
}

/// Chemin opérationnel de résolution : vérifie la liaison durable AVANT de
/// résoudre, réconcilie contre l'enregistrement original, puis délègue la
/// transition au cycle `Attribution`. C'est une COMPOSITION, pas un second moteur
/// — ni `reconcilier` ni `Attribution::resoudre` ne sont dupliqués.
///
/// Ordre : liaison (`verifier_liaison`) → réconciliation (`reconcilier`) →
/// résolution (`Attribution::resoudre`). Un verdict nuancé n'est jamais écrasé
/// en résolution automatique ; une liaison refusée n'atteint jamais la
/// résolution ; `Indetermine` n'est jamais `AbsenceDefinitive`.
pub fn resoudre_operation(
    attr: &mut Attribution,
    id: u128,
    tentative: Tentative,
    enr: &EnregistrementOriginal,
    reference: &ReferenceTentative,
    res: &ResolutionTentative,
) -> IssueResolutionOperation {
    if !enr.verifier_liaison(&tentative) {
        return IssueResolutionOperation::LiaisonRefusee;
    }
    let verdict = reconcilier(enr, reference, res);
    match verdict.vers_backend() {
        Some(backend) => IssueResolutionOperation::Resolu(attr.resoudre(id, tentative, backend)),
        None => IssueResolutionOperation::VerdictNuance(verdict),
    }
}


// ═══════════════════════════════════════════════════════════════════════════════
// Durabilité du record : écriture atomique + qualification d'admissibilité
// ═══════════════════════════════════════════════════════════════════════════════
//
// Ce que ce bloc ÉTABLIT, et ce qu'il n'établit PAS :
//
// - ATOMICITÉ (par construction, mesurable) : les octets vont dans un temporaire
//   du MÊME répertoire, puis `rename`. Un lecteur voit l'ancien record ou le
//   nouveau, jamais un entre-deux. Un écrivain tué laisse au pire un
//   `<record>.tmp.*` orphelin — jamais un record tronqué.
// - DURABILITÉ POSIX (par l'ORDRE des opérations) : les octets sont `fsync`és
//   AVANT le rename, et l'entrée de répertoire APRÈS. Sans le premier, le rename
//   peut précéder les données ; sans le second, le rename lui-même peut être
//   perdu. L'ordre est OBSERVABLE (`ecrire_durable_observe`), donc vérifié, pas
//   seulement affirmé.
// - NON ÉTABLIE : la survie à une coupure d'alimentation. `fsync` en est la
//   condition nécessaire ; la preuve exigerait une panne matérielle réelle.
//
// Rien ici ne fabrique d'autorité : `autorite_consommateur` et `decision_id`
// n'existent PAS dans le format V1, et la lecture d'un V1 rend une vue V1 —
// jamais un `EnregistrementOriginal` complété par des zéros.

use std::path::{Path, PathBuf};

/// Les étapes observables d'une écriture durable, dans l'ORDRE où elles doivent
/// se produire. L'observateur sert à MESURER l'ordre (test) ; il ne le décide pas.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EtapeEcriture {
    TemporaireCree,
    OctetsEcrits,
    FichierSynchronise,
    Renomme,
    RepertoireSynchronise,
}

fn horloge_nanos() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0)
}

/// Nom du temporaire : MÊME répertoire que la cible (sinon `rename` traverserait
/// un système de fichiers et perdrait son atomicité), unique par processus + temps.
pub fn nom_temporaire(chemin: &Path) -> PathBuf {
    let mut nom = chemin.as_os_str().to_os_string();
    nom.push(format!(".tmp.{}.{}", std::process::id(), horloge_nanos()));
    PathBuf::from(nom)
}

/// Écriture atomique ET durable : temporaire → octets → `fsync` fichier →
/// `rename` → `fsync` répertoire. En cas d'échec avant le rename, le temporaire
/// est retiré : aucun résidu n'est laissé par une écriture qui a échoué.
pub fn ecrire_durable(chemin: &Path, octets: &[u8]) -> std::io::Result<()> {
    ecrire_durable_observe(chemin, octets, &mut |_| {})
}

/// Même chemin de code que [`ecrire_durable`], avec l'ORDRE des étapes exposé.
/// Il n'existe pas de seconde implémentation : l'observation ne change rien à ce
/// qui est exécuté.
pub fn ecrire_durable_observe(
    chemin: &Path,
    octets: &[u8],
    observe: &mut dyn FnMut(EtapeEcriture),
) -> std::io::Result<()> {
    use std::io::Write;
    let dossier = chemin.parent().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "chemin sans répertoire parent",
        )
    })?;
    let tmp = nom_temporaire(chemin);
    let resultat = (|| -> std::io::Result<()> {
        let mut f = std::fs::File::create(&tmp)?;
        observe(EtapeEcriture::TemporaireCree);
        f.write_all(octets)?;
        observe(EtapeEcriture::OctetsEcrits);
        // Les OCTETS avant le rename : c'est l'ordre qui rend l'échange sûr.
        f.sync_all()?;
        observe(EtapeEcriture::FichierSynchronise);
        drop(f);
        std::fs::rename(&tmp, chemin)?;
        observe(EtapeEcriture::Renomme);
        // Le NOM du fichier après le rename : sans cela l'entrée de répertoire
        // peut être perdue, et le record avec elle.
        std::fs::File::open(dossier)?.sync_all()?;
        observe(EtapeEcriture::RepertoireSynchronise);
        Ok(())
    })();
    if resultat.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    resultat
}

/// Les temporaires ORPHELINS d'un écrivain mort : `<record>.tmp.*`. Le record
/// publié n'est jamais touché. Un temporaire n'est pas un record : il n'est ni
/// lu, ni interprété, ni promu — seulement constaté.
pub fn temporaires_orphelins(chemin: &Path) -> std::io::Result<Vec<PathBuf>> {
    let dossier = match chemin.parent() {
        Some(d) => d,
        None => return Ok(Vec::new()),
    };
    let Some(nom) = chemin.file_name().and_then(|n| n.to_str()) else {
        return Ok(Vec::new());
    };
    let prefixe = format!("{nom}.tmp.");
    let mut orphelins = Vec::new();
    for entree in std::fs::read_dir(dossier)? {
        let entree = entree?;
        if entree.file_name().to_string_lossy().starts_with(&prefixe) {
            orphelins.push(entree.path());
        }
    }
    orphelins.sort();
    Ok(orphelins)
}

/// Empreinte SHA-256 en hexadécimal — sert à PROUVER qu'une relecture n'a pas
/// modifié les octets (provenance), jamais à identifier une opération (ce n'est
/// pas K, et ce n'est pas une signature).
pub fn empreinte_sha256(octets: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(octets);
    let mut hex = String::with_capacity(64);
    for b in digest {
        hex.push_str(&format!("{b:02x}"));
    }
    hex
}

/// D'où viennent les octets, et à quoi ils correspondent exactement.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Provenance {
    /// Chemin du fichier, ou `"<mémoire>"` pour des octets fournis directement.
    pub source: String,
    pub longueur: usize,
    pub empreinte_sha256: String,
}

impl Provenance {
    pub fn de(octets: &[u8], source: &str) -> Self {
        Provenance {
            source: source.to_string(),
            longueur: octets.len(),
            empreinte_sha256: empreinte_sha256(octets),
        }
    }
}

/// La vue V1 d'un record : EXACTEMENT les champs que le format V1 portait.
/// Ni `decision_id`, ni `autorite_consommateur` — ils n'existaient pas. Les
/// compléter par 0 serait fabriquer une autorité : ce type rend cela impossible.
#[derive(Debug, Clone, PartialEq)]
pub struct EnregistrementV1 {
    pub version: u32,
    pub k: String,
    pub backend: u64,
    pub generation: u64,
    pub cible: NodeId,
    pub effets_attendus: Vec<(String, i64)>,
    pub reference_bytes: Vec<u8>,
}

/// Pourquoi un record est inexploitable comme record COURANT.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MotifIllisibilite {
    /// Magic invalide, troncature, clé non UTF-8, octets excédentaires.
    Malforme(String),
    /// Version qui n'est ni la courante, ni une version legacy connue.
    VersionInconnue(u32),
    /// Décodable octet par octet, mais INADMISSIBLE : K vide, génération 0,
    /// autorité consommateur jamais émise. Le codec ne peut pas le voir ; la
    /// qualification si. C'est la corruption la plus dangereuse, parce qu'elle
    /// passe toutes les vérifications de forme.
    CorruptionSyntactique(String),
}

/// Octets conservés VERBATIM + provenance, avec le motif du refus. Jamais
/// supprimés, jamais « réparés », jamais réécrits.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordIllisible {
    pub octets: Vec<u8>,
    pub provenance: Provenance,
    pub motif: MotifIllisibilite,
}

/// Un record V1 : il peut représenter une tentative ENGAGÉE ou INDÉTERMINÉE.
/// Il est mis en quarantaine EXPLICITE — refusé comme record courant, conservé
/// comme obligation. Sa résolution est possible auprès du backend, mais elle ne
/// produit jamais `Confirme` : K, la décision et l'autorité consommateur ne sont
/// pas attestables dans ce format, et on ne les invente pas.
#[derive(Debug, Clone, PartialEq)]
pub struct RecordLegacy {
    pub version: u32,
    pub octets: Vec<u8>,
    pub provenance: Provenance,
    /// Vue V1 si les octets sont un V1 bien formé. `None` = V1 illisible :
    /// le record reste conservé, sa lecture est refusée nommément.
    pub vue: Option<EnregistrementV1>,
}

/// L'état d'un record tel qu'il doit être MANIPULÉ : jamais « décodé ou rien ».
#[derive(Debug, Clone, PartialEq)]
pub enum EtatEnregistrement {
    /// Record courant, admissible : la chaîne d'autorité est complète.
    Courant(EnregistrementOriginal),
    /// Record V1 : quarantaine legacy, obligation préservée.
    QuarantaineLegacy(RecordLegacy),
    /// Octets illisibles ou inadmissibles : conservés, refusés, nommés.
    QuarantaineIllisible(RecordIllisible),
}

impl EtatEnregistrement {
    /// Les octets tels qu'ils étaient — dans TOUS les cas, y compris le refus.
    /// C'est la garantie qu'un refus ne fait pas disparaître le record.
    pub fn octets_preserves(&self) -> &[u8] {
        match self {
            EtatEnregistrement::Courant(_) => &[], // reconstruit, pas conservé : voir `encoder`
            EtatEnregistrement::QuarantaineLegacy(r) => &r.octets,
            EtatEnregistrement::QuarantaineIllisible(r) => &r.octets,
        }
    }

    /// Un record est EXPLOITABLE comme chaîne d'autorité complète seulement s'il
    /// est courant. Tout le reste exige une décision explicite du consommateur.
    pub fn est_courant(&self) -> bool {
        matches!(self, EtatEnregistrement::Courant(_))
    }

    /// Le refus est-il NOMMÉ (et non une absence) ?
    pub fn refus(&self) -> Option<&MotifIllisibilite> {
        match self {
            EtatEnregistrement::QuarantaineIllisible(r) => Some(&r.motif),
            _ => None,
        }
    }
}

/// Version legacy connue en lecture seule.
pub const ENREGISTREMENT_VERSION_LEGACY: u32 = 1;

/// Décode un record au format V1 (l'ANCIEN codec). Sert à deux choses :
/// relire un V1 mis en quarantaine, et exercer la lecture croisée — un lecteur
/// V1 qui reçoit un record V2 le REFUSE (`VersionInconnue(2)`) au lieu de le
/// mal interpréter. Sans ce refus, les octets V2 se liraient comme un V1 décalé.
pub fn decoder_v1(octets: &[u8]) -> Result<EnregistrementV1, DecodageEnregistrement> {
    if octets.len() < 4 {
        return Err(DecodageEnregistrement::Malforme("trop court".into()));
    }
    if octets[..4] != ENREGISTREMENT_MAGIC {
        return Err(DecodageEnregistrement::Malforme("magic invalide".into()));
    }
    let mut pos = 4usize;
    let version = lire_u32_le(octets, &mut pos)?;
    if version != ENREGISTREMENT_VERSION_LEGACY {
        return Err(DecodageEnregistrement::VersionInconnue(version));
    }
    let k = lire_chaine(octets, &mut pos)?;
    let backend = lire_u64_le(octets, &mut pos)?;
    let generation = lire_u64_le(octets, &mut pos)?;
    let cible = NodeId(lire_u64_le(octets, &mut pos)?);
    let n = lire_u32_le(octets, &mut pos)? as usize;
    let reste = octets.len() - pos;
    if n > reste / 12 {
        return Err(DecodageEnregistrement::Malforme(format!(
            "n_effets={n} dépasse les {reste} octets disponibles"
        )));
    }
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
        return Err(DecodageEnregistrement::Malforme(
            "octets excédentaires".into(),
        ));
    }
    Ok(EnregistrementV1 {
        version,
        k,
        backend,
        generation,
        cible,
        effets_attendus,
        reference_bytes,
    })
}

impl EnregistrementOriginal {
    /// Admissibilité SÉMANTIQUE d'un record décodé. Le codec ne peut pas la
    /// trancher : `k` nul, `generation` 0 et `autorite_consommateur` 0 sont
    /// des octets parfaitement valides.
    ///
    /// `k == 0`, `generation == 0` et `autorite_consommateur == 0` sont refusés
    /// parce que la primitive ne les émet JAMAIS : `AutoriteK` émet depuis 1,
    /// `Attribution::reserver` incrémente depuis 0 (première génération 1) et
    /// `PROCHAINE_AUTORITE` part de 1. Une telle valeur n'a pas été produite par
    /// une réservation : elle a été fabriquée.
    pub fn admissibilite(&self) -> Result<(), MotifIllisibilite> {
        if self.k == 0 {
            return Err(MotifIllisibilite::CorruptionSyntactique(
                "K nul : identité logique absente (jamais émise)".into(),
            ));
        }
        if self.generation == 0 {
            return Err(MotifIllisibilite::CorruptionSyntactique(
                "génération 0 : jamais émise par une réservation".into(),
            ));
        }
        if self.autorite_consommateur == 0 {
            return Err(MotifIllisibilite::CorruptionSyntactique(
                "autorité consommateur 0 : jamais émise par une instance".into(),
            ));
        }
        if self.version != ENREGISTREMENT_VERSION {
            return Err(MotifIllisibilite::VersionInconnue(self.version));
        }
        Ok(())
    }
}

/// Classe des octets de record — la SEULE entrée qui ne perd rien :
/// courant, quarantaine legacy, ou refus nommé avec octets + provenance.
pub fn qualifier(octets: &[u8], source: &str) -> EtatEnregistrement {
    let provenance = Provenance::de(octets, source);
    // 8 octets au minimum : le magic ET la version doivent être lisibles.
    if octets.len() < 8 {
        return EtatEnregistrement::QuarantaineIllisible(RecordIllisible {
            octets: octets.to_vec(),
            provenance,
            motif: MotifIllisibilite::Malforme(format!(
                "trop court : {} octets pour un en-tête de 8",
                octets.len()
            )),
        });
    }
    if octets[..4] != ENREGISTREMENT_MAGIC {
        return EtatEnregistrement::QuarantaineIllisible(RecordIllisible {
            octets: octets.to_vec(),
            provenance,
            motif: MotifIllisibilite::Malforme("magic invalide".into()),
        });
    }
    let version = u32::from_le_bytes([octets[4], octets[5], octets[6], octets[7]]);
    match version {
        ENREGISTREMENT_VERSION => match EnregistrementOriginal::decoder(octets) {
            Ok(enr) => match enr.admissibilite() {
                Ok(()) => EtatEnregistrement::Courant(enr),
                Err(motif) => EtatEnregistrement::QuarantaineIllisible(RecordIllisible {
                    octets: octets.to_vec(),
                    provenance,
                    motif,
                }),
            },
            Err(DecodageEnregistrement::Malforme(raison)) => {
                EtatEnregistrement::QuarantaineIllisible(RecordIllisible {
                    octets: octets.to_vec(),
                    provenance,
                    motif: MotifIllisibilite::Malforme(raison),
                })
            }
            Err(DecodageEnregistrement::VersionInconnue(v)) => {
                EtatEnregistrement::QuarantaineIllisible(RecordIllisible {
                    octets: octets.to_vec(),
                    provenance,
                    motif: MotifIllisibilite::VersionInconnue(v),
                })
            }
            Err(DecodageEnregistrement::ValeurNonRepresentable(raison)) => {
                EtatEnregistrement::QuarantaineIllisible(RecordIllisible {
                    octets: octets.to_vec(),
                    provenance,
                    motif: MotifIllisibilite::Malforme(raison),
                })
            }
        },
        ENREGISTREMENT_VERSION_LEGACY => {
            // V1 : on CONSERVE toujours les octets. La vue n'est qu'un confort
            // de lecture ; son absence ne fait pas disparaître le record.
            let vue = decoder_v1(octets).ok();
            EtatEnregistrement::QuarantaineLegacy(RecordLegacy {
                version,
                octets: octets.to_vec(),
                provenance,
                vue,
            })
        }
        autre => EtatEnregistrement::QuarantaineIllisible(RecordIllisible {
            octets: octets.to_vec(),
            provenance,
            motif: MotifIllisibilite::VersionInconnue(autre),
        }),
    }
}

/// Relit un record depuis le médium et le QUALIFIE. L'octet est la seule source :
/// aucun état RAM n'intervient, donc la qualification est reproductible dans un
/// autre processus.
pub fn relire_qualifie(chemin: &Path) -> std::io::Result<EtatEnregistrement> {
    let octets = std::fs::read(chemin)?;
    Ok(qualifier(&octets, &chemin.to_string_lossy()))
}

/// Ce qu'un record V1 permet d'établir auprès du backend — et rien de plus.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolutionLegacy {
    /// Les effets attendus sont retrouvés ENGAGÉS dans le backend. L'identité
    /// logique (K/G) de l'opération n'est PAS attestée par ce format.
    EffetEngageIdentiteNonAttestee,
    /// Au moins un effet retrouvé, au moins un non retrouvé.
    EffetPartielIdentiteNonAttestee,
    /// Aucun effet retrouvé. JAMAIS « absence définitive » : l'absence d'effet
    /// constatée aujourd'hui ne prouve pas qu'aucune tentative n'a été engagée.
    EffetNonRetrouveJamaisAbsenceDefinitive,
}

impl RecordLegacy {
    /// Résout l'OBLIGATION d'un record V1 auprès du backend, sans jamais
    /// fabriquer l'autorité qui manque. La référence (E) est dans les octets ;
    /// on la relit, on interroge la base, et on rapporte ce que la base dit.
    pub fn resoudre_obligation(&self, store: &SubstrateStore) -> Result<ResolutionLegacy, String> {
        let vue = self
            .vue
            .as_ref()
            .ok_or_else(|| "record V1 illisible : aucune obligation résoluble".to_string())?;
        let reference = ReferenceTentative::from_bytes(&vue.reference_bytes)
            .map_err(|e| format!("référence V1 illisible : {e}"))?;
        let res = store
            .resoudre_reference(&reference)
            .map_err(|e| format!("résolution refusée par le backend : {e}"))?;
        Ok(match res.integrite {
            IntegriteTentative::Integrale => ResolutionLegacy::EffetEngageIdentiteNonAttestee,
            IntegriteTentative::Partielle => ResolutionLegacy::EffetPartielIdentiteNonAttestee,
            IntegriteTentative::Indeterminee => {
                ResolutionLegacy::EffetNonRetrouveJamaisAbsenceDefinitive
            }
        })
    }
}
