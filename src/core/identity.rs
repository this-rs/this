//! Identité d'entité **engine-agnostique**.
//!
//! # Le problème qu'il résout
//!
//! `ObrainDataService` (voir `storage::obrain`) exige trois conventions sur les
//! nœuds : `_uuid` (l'entité est identifiable), `_data` (le JSON qui fait
//! autorité) et `f_<champ>` (les champs indexés). Une base écrite par un autre
//! outil — un corpus aletheia (`Page`, `Lemma`, `SeatVoice`), un graphe de code
//! (`Function`, `CALLS`) — n'a **aucune** de ces trois marques : elle rend
//! zéro entité. Or la promesse est l'inverse : *la même entité, quel que soit le
//! moteur*.
//!
//! # La séparation qui rend la propriété testable
//!
//! - **La projection** (ce module) est *pure* : par nœud, sans I/O, sans moteur.
//!   Toute la sémantique d'identité vit ici — donc elle se teste avec des
//!   [`NodeView`] synthétiques, sans base, sans feature.
//! - **La localisation** (I/O par moteur) vit ailleurs, sous feature
//!   (`storage::obrain_identity`).
//!
//! # Trois règles non négociables
//!
//! 1. **La convention `_uuid`/`_data` est prioritaire et non déplaçable.** Elle
//!    occupe l'index 0 du registre ; [`IdentityRegistry::push`] insère *après*.
//!    Conséquence : sur une base écrite par this-rs, la résolution est
//!    exactement celle d'avant — aucun test existant ne bouge.
//! 2. **Un `_uuid` illisible arrête la chaîne** ([`IdentityError::Malformed`],
//!    distinct de [`IdentityError::NotApplicable`]) : re-cléer ici donnerait une
//!    **seconde identité** à une entité qui en a déjà une.
//! 3. **Un motif non branché se déclare.** [`ApiProjection`] et
//!    [`ChainProjection`] rendent `Unresolvable { reason: "non implémenté" }` —
//!    jamais un silence, jamais un zéro (même doctrine que `graph::relief`).
//!
//! # Stabilité de l'identité dérivée
//!
//! L'`uuid` dérivé est une **clé permanente** : il est fonction pure de
//! `(space, label, qualifier)`, jamais du `NodeId`, de l'ordre d'insertion, de
//! l'horloge, d'un RNG ou de l'ordre physique des pages. Le schéma est
//! **versionné et gelé** ([`ENTITY_ID_SCHEME`]) : le changer re-clé toutes les
//! entités, donc il ne se change pas en place.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

/// Version du schéma d'identité dérivée. **GELÉE** : la changer change tous les ids.
pub const ENTITY_ID_SCHEME: &str = "this-rs/entity-id/sha256-128/v1";

/// Séparateur de domaine — haché en tête, jamais concaténé à une chaîne, pour
/// qu'aucune entrée ne puisse simuler la structure d'une autre.
const DOMAIN: &[u8] = b"this-rs/entity-id/v1\0";

/// Taille maximale d'un champ d'identité (label, espace, nom de propriété, valeur).
pub const MAX_FIELD_BYTES: usize = 4096;

/// Propriétés examinées, dans l'ordre, pour une entité sans `_uuid`.
pub const DEFAULT_KEY_PROPERTIES: &[&str] =
    &["qualified_name", "qn", "id", "_id", "key", "name", "title"];

/// Propriétés **volatiles**, exclues d'un digest de contenu : sans cette liste,
/// l'identité changerait à chaque écriture.
pub const DEFAULT_VOLATILE: &[&str] = &[
    "updated_at",
    "created_at",
    "ts",
    "epoch",
    "_rev",
    "_version",
    "deleted_at",
];

/// Préfixe des labels réservés (this-rs stocke ses liens en nœuds `_link:<type>`).
pub const DEFAULT_RESERVED_PREFIX: &str = "_";

// ---------------------------------------------------------------------------
// Erreurs
// ---------------------------------------------------------------------------

/// Pourquoi une identité n'a pas pu être établie.
///
/// La distinction est le cœur de la règle 2 : `NotApplicable` fait *continuer*
/// la chaîne de motifs, `Malformed` l'*arrête*.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum IdentityError {
    /// Ce motif ne s'applique pas à ce nœud — la chaîne continue.
    #[error("motif `{motive}` non applicable : {reason}")]
    NotApplicable {
        /// Motif concerné.
        motive: String,
        /// Pourquoi il ne s'applique pas.
        reason: String,
    },
    /// Le nœud prétendait au motif mais est incohérent — la chaîne s'arrête.
    #[error("motif `{motive}` : nœud incohérent ({node}) : {reason}")]
    Malformed {
        /// Motif concerné.
        motive: String,
        /// Ce qui désigne le nœud.
        node: String,
        /// L'incohérence constatée.
        reason: String,
    },
    /// Bien formé mais inexploitable : label réservé, aucune clé, hors bornes.
    #[error("entité non résoluble : {reason}")]
    Unresolvable {
        /// La raison, telle qu'elle sera publiée.
        reason: String,
    },
}

// ---------------------------------------------------------------------------
// Clé canonique
// ---------------------------------------------------------------------------

/// Ce qui nomme une entité — la partie *dérivable* de son identité.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Qualifier {
    /// Nom naturel, porté par une propriété nommée.
    ///
    /// **Le nom de la propriété fait partie de la clé** : sans lui, un nœud dont
    /// `name` vaudrait le `qualified_name` d'un autre recevrait l'identité de
    /// cet autre (fusion silencieuse de deux entités).
    Natural {
        /// Propriété porteuse du nom.
        property: String,
        /// Valeur du nom.
        value: String,
    },
    /// Aucun nom naturel : digest canonique des propriétés (clé de déduplication).
    Content {
        /// Digest hexadécimal (32 caractères).
        digest: String,
    },
}

impl Qualifier {
    fn tag(&self) -> u8 {
        match self {
            Qualifier::Natural { .. } => 0x01,
            Qualifier::Content { .. } => 0x02,
        }
    }

    fn write_canonical(&self, out: &mut Vec<u8>) {
        out.push(self.tag());
        match self {
            Qualifier::Natural { property, value } => {
                push_lp(out, property);
                push_lp(out, value);
            }
            Qualifier::Content { digest } => push_lp(out, digest),
        }
    }
}

/// Encodage **longueur-préfixé** : `u32 LE len || octets`.
///
/// C'est ce qui rend l'encodage **injectif** — `("a","bc")` et `("ab","c")` ne
/// peuvent pas produire les mêmes octets, contrairement à une concaténation.
fn push_lp(out: &mut Vec<u8>, s: &str) {
    let bytes = s.as_bytes();
    out.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
    out.extend_from_slice(bytes);
}

/// L'identité canonique d'une entité : `(espace, label, qualifier)`.
///
/// Construite par [`EntityKey::new`], qui **valide** — les méthodes de dérivation
/// sont donc infaillibles ensuite.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
pub struct EntityKey {
    /// Espace (graphe nommé) — `None` = graphe par défaut.
    pub space: Option<String>,
    /// Label canonique (voir [`IdentityRegistry::canonical_label`]).
    pub label: String,
    /// Le nom, ou son digest.
    pub qualifier: Qualifier,
}

impl EntityKey {
    /// Construit une clé **valide**, ou explique pourquoi elle ne peut pas l'être.
    ///
    /// Validation : champs non vides, ≤ [`MAX_FIELD_BYTES`], et digest de contenu
    /// hexadécimal de 32 caractères.
    pub fn new(
        space: Option<String>,
        label: impl Into<String>,
        qualifier: Qualifier,
    ) -> Result<Self, IdentityError> {
        let label = label.into();
        let key = Self {
            space,
            label,
            qualifier,
        };
        key.validate()?;
        Ok(key)
    }

    fn validate(&self) -> Result<(), IdentityError> {
        let fail = |reason: String| Err(IdentityError::Unresolvable { reason });

        if self.label.is_empty() {
            return fail("label vide".into());
        }
        if self.label.len() > MAX_FIELD_BYTES {
            return fail(format!("label > {MAX_FIELD_BYTES} octets"));
        }
        if let Some(space) = &self.space {
            if space.is_empty() {
                return fail("espace vide (utiliser None pour le graphe par défaut)".into());
            }
            if space.len() > MAX_FIELD_BYTES {
                return fail(format!("espace > {MAX_FIELD_BYTES} octets"));
            }
        }
        match &self.qualifier {
            Qualifier::Natural { property, value } => {
                if property.is_empty() {
                    return fail("nom de propriété vide".into());
                }
                if property.len() > MAX_FIELD_BYTES || value.len() > MAX_FIELD_BYTES {
                    return fail(format!("propriété ou valeur > {MAX_FIELD_BYTES} octets"));
                }
                if value.is_empty() {
                    return fail("valeur d'identité vide".into());
                }
            }
            Qualifier::Content { digest } => {
                if digest.len() != 32 || !digest.bytes().all(|b| b.is_ascii_hexdigit()) {
                    return fail("digest de contenu : 32 caractères hexadécimaux attendus".into());
                }
            }
        }
        Ok(())
    }

    /// Les octets exactement hachés.
    ///
    /// **Publiés** par la route `GET /graph/identity/{id}` : un tiers peut ainsi
    /// recalculer l'identifiant sans faire confiance à this-rs. C'est la
    /// définition opérationnelle de « la même entité quel que soit le moteur ».
    pub fn canonical_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(64);
        out.extend_from_slice(DOMAIN);
        match &self.space {
            Some(space) => {
                out.push(0x01);
                push_lp(&mut out, space);
            }
            None => out.push(0x00),
        }
        push_lp(&mut out, &self.label);
        self.qualifier.write_canonical(&mut out);
        out
    }

    /// L'identifiant dérivé : `sha256(octets canoniques)[..16]`, **version 8**.
    ///
    /// Version 8 (RFC 9562) et non 4 : un id **dérivé** est *reconnaissable*, et
    /// ne peut donc pas être confondu avec un v4 écrit par la convention.
    pub fn uuid(&self) -> Uuid {
        let digest = Sha256::digest(self.canonical_bytes());
        let mut bytes = [0u8; 16];
        bytes.copy_from_slice(&digest[..16]);
        bytes[6] = (bytes[6] & 0x0f) | 0x80; // version 8
        bytes[8] = (bytes[8] & 0x3f) | 0x80; // variante RFC 4122
        Uuid::from_bytes(bytes)
    }

    /// Les octets canoniques, en hexadécimal — pour publication et comparaison.
    pub fn canonical_hex(&self) -> String {
        self.canonical_bytes()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect()
    }
}

impl<'de> Deserialize<'de> for EntityKey {
    /// Désérialisation **validante** : une clé reçue de l'extérieur ne peut pas
    /// contourner [`EntityKey::new`].
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct Mirror {
            space: Option<String>,
            label: String,
            qualifier: Qualifier,
        }
        let m = Mirror::deserialize(deserializer)?;
        EntityKey::new(m.space, m.label, m.qualifier).map_err(serde::de::Error::custom)
    }
}

// ---------------------------------------------------------------------------
// Vue de nœud — la seule entrée d'une projection
// ---------------------------------------------------------------------------

/// Valeur de propriété, réduite à ce dont l'identité a besoin.
///
/// `Bytes` existe pour une raison précise : le rendu JSON d'un blob est
/// **lossy** (« <123 bytes> »), donc un digest calculé sur du JSON n'est pas
/// stable entre moteurs. `write_canonical` est lossless ; `to_json` est l'API.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "t", content = "v", rename_all = "snake_case")]
pub enum PropValue {
    /// Absent / nul.
    Null,
    /// Booléen.
    Bool(bool),
    /// Entier signé.
    Int(i64),
    /// Flottant (encodage canonique : `-0.0` normalisé, NaN figé).
    Float(f64),
    /// Chaîne.
    Str(String),
    /// Octets bruts.
    Bytes(Vec<u8>),
    /// JSON structuré.
    Json(serde_json::Value),
}

impl PropValue {
    /// Rendu JSON — **lossy sur `Bytes`**, assumé : c'est l'API, pas l'identité.
    pub fn to_json(&self) -> serde_json::Value {
        match self {
            PropValue::Null => serde_json::Value::Null,
            PropValue::Bool(b) => serde_json::Value::Bool(*b),
            PropValue::Int(i) => serde_json::Value::Number((*i).into()),
            PropValue::Float(f) => serde_json::Number::from_f64(*f)
                .map(serde_json::Value::Number)
                .unwrap_or(serde_json::Value::Null),
            PropValue::Str(s) => serde_json::Value::String(s.clone()),
            PropValue::Bytes(b) => serde_json::Value::String(format!("<{} bytes>", b.len())),
            PropValue::Json(v) => v.clone(),
        }
    }

    /// Rendu canonique — **lossless**, étiqueté par type (un `Int(1)` et un
    /// `Str("1")` ne peuvent pas produire les mêmes octets).
    pub fn write_canonical(&self, out: &mut Vec<u8>) {
        match self {
            PropValue::Null => out.push(0x00),
            PropValue::Bool(b) => {
                out.push(0x01);
                out.push(u8::from(*b));
            }
            PropValue::Int(i) => {
                out.push(0x02);
                out.extend_from_slice(&i.to_le_bytes());
            }
            PropValue::Float(f) => {
                out.push(0x03);
                let f = if *f == 0.0 { 0.0 } else { *f }; // -0.0 → 0.0
                let bits = if f.is_nan() {
                    f64::NAN.to_bits()
                } else {
                    f.to_bits()
                };
                out.extend_from_slice(&bits.to_le_bytes());
            }
            PropValue::Str(s) => {
                out.push(0x04);
                push_lp(out, s);
            }
            PropValue::Bytes(b) => {
                out.push(0x05);
                out.extend_from_slice(&(b.len() as u32).to_le_bytes());
                out.extend_from_slice(b);
            }
            PropValue::Json(v) => {
                out.push(0x06);
                push_lp(out, &canonical_json(v));
            }
        }
    }
}

/// JSON canonique : clés triées, pour que l'ordre d'insertion n'influence pas le digest.
fn canonical_json(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            let inner: Vec<String> = keys
                .iter()
                .map(|k| {
                    format!(
                        "{}:{}",
                        serde_json::to_string(k).unwrap_or_default(),
                        canonical_json(&map[*k])
                    )
                })
                .collect();
            format!("{{{}}}", inner.join(","))
        }
        serde_json::Value::Array(items) => {
            let inner: Vec<String> = items.iter().map(canonical_json).collect();
            format!("[{}]", inner.join(","))
        }
        other => serde_json::to_string(other).unwrap_or_default(),
    }
}

/// Réduction minimale d'un nœud : ce qu'un moteur **doit** fournir, rien de plus.
#[derive(Debug, Clone, PartialEq)]
pub struct NodeView {
    /// Espace du nœud — `None` = graphe par défaut.
    pub space: Option<String>,
    /// Labels **tels que le moteur les rend**. L'ordre n'est **pas** significatif
    /// (obrain rend un `SmallVec` en ordre d'insertion) : le label canonique est
    /// choisi par le registre, jamais pris en `labels[0]`.
    pub labels: Vec<String>,
    /// Propriétés en **ordre lexicographique** — la seule vue canonique.
    pub properties: BTreeMap<String, PropValue>,
}

impl NodeView {
    /// Vue d'un nœud sans propriété.
    pub fn new(space: Option<String>, labels: Vec<String>) -> Self {
        Self {
            space,
            labels,
            properties: BTreeMap::new(),
        }
    }

    /// Ajoute une propriété (constructeur fluide, pour les tests).
    pub fn with(mut self, key: &str, value: PropValue) -> Self {
        self.properties.insert(key.to_string(), value);
        self
    }

    /// Valeur d'une propriété, si elle est présente et non nulle.
    pub fn get(&self, key: &str) -> Option<&PropValue> {
        match self.properties.get(key) {
            Some(PropValue::Null) | None => None,
            Some(v) => Some(v),
        }
    }

    /// Rendu textuel d'une propriété (les chaînes sont la forme naturelle d'un nom).
    pub fn get_str(&self, key: &str) -> Option<String> {
        match self.get(key)? {
            PropValue::Str(s) => Some(s.clone()),
            PropValue::Int(i) => Some(i.to_string()),
            PropValue::Float(f) => Some(f.to_string()),
            PropValue::Bool(b) => Some(b.to_string()),
            PropValue::Json(v) => serde_json::to_string(v).ok(),
            PropValue::Bytes(_) | PropValue::Null => None,
        }
    }

    /// Un identifiant textuel du nœud, pour les rapports d'anomalie.
    pub fn describe(&self) -> String {
        let label = self.labels.first().cloned().unwrap_or_else(|| "?".into());
        match self
            .get_str("qualified_name")
            .or_else(|| self.get_str("name"))
        {
            Some(name) => format!("{label}:{name}"),
            None => format!("{label}#[{} props]", self.properties.len()),
        }
    }
}

// ---------------------------------------------------------------------------
// Résultat
// ---------------------------------------------------------------------------

/// D'où vient l'identité retenue.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Provenance {
    /// Lue telle quelle dans `_uuid` — la convention historique de this-rs.
    Convention,
    /// Recalculée depuis le contenu (id dérivé, version 8).
    Derived,
    /// Annoncée par un motif qui ne produit pas encore d'id (api, chaîne).
    Declared,
}

/// Une entité résolue, quelle que soit sa provenance.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ResolvedEntity {
    /// Identifiant : v4 (convention) ou v8 (dérivé).
    pub id: Uuid,
    /// La clé canonique.
    pub key: EntityKey,
    /// Le label canonique retenu (pas nécessairement `labels[0]`).
    pub label: String,
    /// Le type d'entité au sens de this-rs (label canonique).
    pub entity_type: String,
    /// Nom lisible.
    pub name: String,
    /// Le contenu : `_data` désérialisé s'il existe, sinon les propriétés plates.
    pub data: serde_json::Value,
    /// Provenance de l'identité.
    pub origin: Provenance,
    /// Le motif qui a produit cette résolution.
    pub motive: String,
}

// ---------------------------------------------------------------------------
// Projections
// ---------------------------------------------------------------------------

/// Un motif d'identité : **pur**, sans I/O, sans état.
pub trait EntityProjection: Send + Sync {
    /// Nom stable du motif, publié.
    fn motive(&self) -> &'static str;
    /// Ce motif prétend-il traiter ce label ?
    fn claims(&self, label: &str) -> bool;
    /// Propriétés examinées, dans l'ordre de priorité — publiées telles quelles.
    fn key_properties(&self) -> &'static [&'static str];
    /// Projette la vue, ou explique pourquoi pas.
    fn project(
        &self,
        view: &NodeView,
        label: &str,
        space: Option<&str>,
    ) -> Result<ResolvedEntity, IdentityError>;
}

/// Motif **convention** : `_uuid` (+ `_data` s'il existe). Toujours en tête.
#[derive(Debug, Default, Clone, Copy)]
pub struct ConventionProjection;

impl EntityProjection for ConventionProjection {
    fn motive(&self) -> &'static str {
        "convention:_uuid/_data"
    }

    fn claims(&self, _label: &str) -> bool {
        true
    }

    fn key_properties(&self) -> &'static [&'static str] {
        &["_uuid"]
    }

    fn project(
        &self,
        view: &NodeView,
        label: &str,
        space: Option<&str>,
    ) -> Result<ResolvedEntity, IdentityError> {
        let Some(raw) = view.get_str("_uuid") else {
            return Err(IdentityError::NotApplicable {
                motive: self.motive().into(),
                reason: "pas de `_uuid`".into(),
            });
        };
        // Règle 2 : illisible ⇒ ARRÊT. Re-cléer donnerait une seconde identité.
        let Ok(id) = Uuid::parse_str(&raw) else {
            return Err(IdentityError::Malformed {
                motive: self.motive().into(),
                node: view.describe(),
                reason: format!("`_uuid` illisible : {raw:?}"),
            });
        };

        // `_data` fait autorité quand il est présent.
        let (data, name) = match view.get_str("_data") {
            Some(json) => match serde_json::from_str::<serde_json::Value>(&json) {
                Ok(v) => {
                    let name = v
                        .get("name")
                        .and_then(|n| n.as_str())
                        .map(str::to_string)
                        .unwrap_or_else(|| id.to_string());
                    (v, name)
                }
                Err(e) => {
                    return Err(IdentityError::Malformed {
                        motive: self.motive().into(),
                        node: view.describe(),
                        reason: format!("`_data` non-JSON : {e}"),
                    });
                }
            },
            None => {
                let name = view.get_str("name").unwrap_or_else(|| id.to_string());
                (flat_properties_json(view), name)
            }
        };

        Ok(ResolvedEntity {
            id,
            key: EntityKey::new(
                space.map(str::to_string),
                label,
                Qualifier::Natural {
                    property: "_uuid".into(),
                    value: raw,
                },
            )?,
            label: label.to_string(),
            entity_type: label.to_string(),
            name,
            data,
            origin: Provenance::Convention,
            motive: self.motive().into(),
        })
    }
}

/// Motif **propriétés plates** : un nom naturel, sans `_uuid`.
#[derive(Debug, Clone)]
pub struct FlatPropertyProjection {
    candidates: &'static [&'static str],
}

impl Default for FlatPropertyProjection {
    fn default() -> Self {
        Self {
            candidates: DEFAULT_KEY_PROPERTIES,
        }
    }
}

impl FlatPropertyProjection {
    /// Avec une liste de propriétés candidates sur mesure.
    pub fn with_candidates(candidates: &'static [&'static str]) -> Self {
        Self { candidates }
    }
}

impl EntityProjection for FlatPropertyProjection {
    fn motive(&self) -> &'static str {
        "derived:flat-property"
    }

    fn claims(&self, _label: &str) -> bool {
        true
    }

    fn key_properties(&self) -> &'static [&'static str] {
        self.candidates
    }

    fn project(
        &self,
        view: &NodeView,
        label: &str,
        space: Option<&str>,
    ) -> Result<ResolvedEntity, IdentityError> {
        for prop in self.candidates {
            let Some(value) = view.get_str(prop) else {
                continue;
            };
            if value.is_empty() {
                continue;
            }
            let key = EntityKey::new(
                space.map(str::to_string),
                label,
                Qualifier::Natural {
                    property: (*prop).to_string(),
                    value: value.clone(),
                },
            )?;
            return Ok(ResolvedEntity {
                id: key.uuid(),
                key,
                label: label.to_string(),
                entity_type: label.to_string(),
                name: value,
                data: flat_properties_json(view),
                origin: Provenance::Derived,
                motive: self.motive().into(),
            });
        }
        Err(IdentityError::NotApplicable {
            motive: self.motive().into(),
            reason: format!("aucune propriété d'identité parmi {:?}", self.candidates),
        })
    }
}

/// Motif **digest de contenu** — clé de **déduplication à la lecture**, jamais
/// une clé primaire persistante. Désactivé par défaut (voir [`Fallback`]).
#[derive(Debug, Clone)]
pub struct ContentDigestProjection {
    volatile: &'static [&'static str],
}

impl Default for ContentDigestProjection {
    fn default() -> Self {
        Self {
            volatile: DEFAULT_VOLATILE,
        }
    }
}

impl ContentDigestProjection {
    /// Avec une liste de propriétés volatiles sur mesure.
    pub fn with_volatile(volatile: &'static [&'static str]) -> Self {
        Self { volatile }
    }

    /// Digest des propriétés **hors** volatiles, en encodage canonique.
    pub fn digest(&self, view: &NodeView) -> String {
        let mut out = Vec::new();
        for (k, v) in &view.properties {
            if self.volatile.contains(&k.as_str()) || k == "_data" {
                continue;
            }
            push_lp(&mut out, k);
            v.write_canonical(&mut out);
        }
        let d = Sha256::digest(&out);
        d[..16].iter().map(|b| format!("{b:02x}")).collect()
    }
}

impl EntityProjection for ContentDigestProjection {
    fn motive(&self) -> &'static str {
        "derived:content-digest"
    }

    fn claims(&self, _label: &str) -> bool {
        true
    }

    fn key_properties(&self) -> &'static [&'static str] {
        &[]
    }

    fn project(
        &self,
        view: &NodeView,
        label: &str,
        space: Option<&str>,
    ) -> Result<ResolvedEntity, IdentityError> {
        if view.properties.is_empty() {
            return Err(IdentityError::NotApplicable {
                motive: self.motive().into(),
                reason: "aucune propriété à digérer".into(),
            });
        }
        let digest = self.digest(view);
        let key = EntityKey::new(
            space.map(str::to_string),
            label,
            Qualifier::Content {
                digest: digest.clone(),
            },
        )?;
        Ok(ResolvedEntity {
            id: key.uuid(),
            key,
            label: label.to_string(),
            entity_type: label.to_string(),
            name: digest,
            data: flat_properties_json(view),
            origin: Provenance::Derived,
            motive: self.motive().into(),
        })
    }
}

/// Motif **api** — déclaré, non branché (pas de source I/O câblée).
#[derive(Debug, Default, Clone, Copy)]
pub struct ApiProjection;

impl EntityProjection for ApiProjection {
    fn motive(&self) -> &'static str {
        "declared:api"
    }
    fn claims(&self, _label: &str) -> bool {
        false
    }
    fn key_properties(&self) -> &'static [&'static str] {
        &[]
    }
    fn project(
        &self,
        _view: &NodeView,
        _label: &str,
        _space: Option<&str>,
    ) -> Result<ResolvedEntity, IdentityError> {
        Err(IdentityError::Unresolvable {
            reason: "motif `api` déclaré mais non implémenté (aucune source I/O câblée)".into(),
        })
    }
}

/// Motif **chaîne** — déclaré, non branché.
#[derive(Debug, Default, Clone, Copy)]
pub struct ChainProjection;

impl EntityProjection for ChainProjection {
    fn motive(&self) -> &'static str {
        "declared:chain"
    }
    fn claims(&self, _label: &str) -> bool {
        false
    }
    fn key_properties(&self) -> &'static [&'static str] {
        &[]
    }
    fn project(
        &self,
        _view: &NodeView,
        _label: &str,
        _space: Option<&str>,
    ) -> Result<ResolvedEntity, IdentityError> {
        Err(IdentityError::Unresolvable {
            reason: "motif `chain` déclaré mais non implémenté (reconstruction à venir)".into(),
        })
    }
}

/// Rendu JSON des propriétés plates (hors propriétés internes `_`).
fn flat_properties_json(view: &NodeView) -> serde_json::Value {
    let mut map = serde_json::Map::new();
    for (k, v) in &view.properties {
        if k.starts_with('_') {
            continue;
        }
        map.insert(k.clone(), v.to_json());
    }
    serde_json::Value::Object(map)
}

// ---------------------------------------------------------------------------
// Registre
// ---------------------------------------------------------------------------

/// Que faire quand aucun motif ne s'applique.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Fallback {
    /// Le nœud n'a pas d'identité — il est ignoré (défaut).
    Skip,
    /// Digest de contenu (clé de déduplication — à activer sciemment).
    ContentDigest,
}

/// Description publiée d'un motif.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MotiveSpec {
    /// Nom du motif.
    pub motive: String,
    /// Provenance qu'il produit.
    pub origin: Provenance,
    /// Propriétés qu'il examine.
    pub key_properties: Vec<String>,
    /// Position dans la chaîne (0 = tête, non déplaçable).
    pub rank: usize,
}

/// La chaîne de résolution d'identité.
pub struct IdentityRegistry {
    motives: Vec<Arc<dyn EntityProjection>>,
    fallback: Fallback,
    digest: ContentDigestProjection,
    reserved_prefix: String,
}

impl Default for IdentityRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl IdentityRegistry {
    /// Registre minimal : la **convention seule** (règle 1), repli `Skip`.
    pub fn new() -> Self {
        Self {
            motives: vec![Arc::new(ConventionProjection)],
            fallback: Fallback::Skip,
            digest: ContentDigestProjection::default(),
            reserved_prefix: DEFAULT_RESERVED_PREFIX.to_string(),
        }
    }

    /// Le registre d'un hôte adossé à obrain : convention **puis** propriétés
    /// plates. C'est ce que l'hôte auto-câble quand un store est attaché.
    pub fn obrain_default() -> Self {
        Self {
            motives: vec![
                Arc::new(ConventionProjection),
                Arc::new(FlatPropertyProjection::default()),
            ],
            fallback: Fallback::Skip,
            digest: ContentDigestProjection::default(),
            reserved_prefix: DEFAULT_RESERVED_PREFIX.to_string(),
        }
    }

    /// Change le repli.
    pub fn with_fallback(mut self, fallback: Fallback) -> Self {
        self.fallback = fallback;
        self
    }

    /// Change les propriétés candidates du motif « propriétés plates ».
    pub fn with_key_properties(mut self, candidates: &'static [&'static str]) -> Self {
        self.motives = vec![
            Arc::new(ConventionProjection),
            Arc::new(FlatPropertyProjection::with_candidates(candidates)),
        ];
        self
    }

    /// Change le préfixe des labels réservés.
    pub fn with_reserved_prefix(mut self, prefix: impl Into<String>) -> Self {
        self.reserved_prefix = prefix.into();
        self
    }

    /// Insère un motif **après** la convention, qui ne peut pas être déplacée.
    pub fn push(&mut self, projection: Arc<dyn EntityProjection>) {
        self.motives.push(projection);
    }

    /// Les motifs, dans l'ordre, tels qu'ils seront publiés.
    pub fn motives(&self) -> Vec<MotiveSpec> {
        self.motives
            .iter()
            .enumerate()
            .map(|(rank, m)| MotiveSpec {
                motive: m.motive().to_string(),
                origin: match m.motive() {
                    "convention:_uuid/_data" => Provenance::Convention,
                    m if m.starts_with("declared:") => Provenance::Declared,
                    _ => Provenance::Derived,
                },
                key_properties: m
                    .key_properties()
                    .iter()
                    .map(|s| (*s).to_string())
                    .collect(),
                rank,
            })
            .collect()
    }

    /// Le repli courant.
    pub fn fallback(&self) -> Fallback {
        self.fallback
    }

    /// Le label canonique : le **plus petit** label non réservé, en ordre
    /// lexicographique.
    ///
    /// Jamais `labels[0]` : l'ordre rendu par le moteur est un ordre d'insertion,
    /// pas une sémantique. `None` si tous les labels sont réservés.
    pub fn canonical_label(&self, labels: &[String]) -> Option<String> {
        labels
            .iter()
            .filter(|l| !l.is_empty() && !l.starts_with(&self.reserved_prefix))
            .min()
            .cloned()
    }

    /// Résout l'identité d'une vue, en parcourant la chaîne puis le repli.
    pub fn project(&self, view: &NodeView) -> Result<ResolvedEntity, IdentityError> {
        let label =
            self.canonical_label(&view.labels)
                .ok_or_else(|| IdentityError::Unresolvable {
                    reason: format!(
                        "aucun label utilisable (tous réservés par le préfixe {:?}) : {:?}",
                        self.reserved_prefix, view.labels
                    ),
                })?;
        let space = view.space.as_deref();

        let mut last: Option<IdentityError> = None;
        for motive in &self.motives {
            if !motive.claims(&label) {
                continue;
            }
            match motive.project(view, &label, space) {
                Ok(entity) => return Ok(entity),
                // La chaîne CONTINUE : ce motif ne s'applique pas.
                Err(err @ IdentityError::NotApplicable { .. }) => last = Some(err),
                // La chaîne S'ARRÊTE : le nœud est incohérent, on ne re-clée pas.
                Err(err) => return Err(err),
            }
        }

        if self.fallback == Fallback::ContentDigest {
            match self.digest.project(view, &label, space) {
                Ok(entity) => return Ok(entity),
                Err(err @ IdentityError::NotApplicable { .. }) => last = Some(err),
                Err(err) => return Err(err),
            }
        }

        Err(last.unwrap_or(IdentityError::NotApplicable {
            motive: "chaîne".into(),
            reason: "aucun motif applicable et repli désactivé".into(),
        }))
    }

    /// Comme [`IdentityRegistry::project`], mais `None` quand aucun motif ne
    /// s'applique (le repli `Skip`), tout en **remontant** les incohérences.
    pub fn project_or_skip(
        &self,
        view: &NodeView,
    ) -> Result<Option<ResolvedEntity>, IdentityError> {
        match self.project(view) {
            Ok(e) => Ok(Some(e)),
            Err(IdentityError::NotApplicable { .. }) => Ok(None),
            Err(e) => Err(e),
        }
    }
}

// ---------------------------------------------------------------------------
// Index espace-aware
// ---------------------------------------------------------------------------

/// Index `Uuid → handle` **cloisonné par espace**.
///
/// La garde est **dans le type** : [`IdentityIndex::insert`] refuse (et rend
/// `false`) dès que l'espace demandé diffère de celui indexé. On ne peut donc pas
/// l'oublier à un site d'appel — c'est ce qui neutralise le piège « deux espaces
/// sont deux substrats qui allouent chacun leurs `NodeId` à partir de 1 ».
#[derive(Debug, Default)]
pub struct IdentityIndex {
    space: Option<String>,
    by_uuid: HashMap<Uuid, u64>,
}

impl IdentityIndex {
    /// Vide l'index et fixe son espace.
    pub fn clear(&mut self, space: Option<String>) {
        self.by_uuid.clear();
        self.space = space;
    }

    /// Insère une correspondance **pour un espace donné**.
    ///
    /// Rend `false` — sans rien écrire — si l'espace demandé diffère de celui de
    /// l'index : c'est la garde qui empêche de mélanger deux espaces (dont les
    /// `NodeId` repartent chacun de 1).
    pub fn insert(&mut self, space: Option<&str>, id: Uuid, handle: u64) -> bool {
        if self.space.as_deref() != space {
            return false;
        }
        self.by_uuid.insert(id, handle);
        true
    }

    /// Résout un identifiant.
    pub fn get(&self, id: &Uuid) -> Option<u64> {
        self.by_uuid.get(id).copied()
    }

    /// Nombre d'entrées indexées.
    pub fn len(&self) -> usize {
        self.by_uuid.len()
    }

    /// L'index est-il vide ?
    pub fn is_empty(&self) -> bool {
        self.by_uuid.is_empty()
    }

    /// L'espace de l'index.
    pub fn space(&self) -> Option<&str> {
        self.space.as_deref()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn natural(label: &str, prop: &str, value: &str) -> EntityKey {
        EntityKey::new(
            None,
            label,
            Qualifier::Natural {
                property: prop.into(),
                value: value.into(),
            },
        )
        .expect("clé valide")
    }

    fn view(labels: &[&str]) -> NodeView {
        NodeView::new(None, labels.iter().map(|s| (*s).to_string()).collect())
    }

    // ── Encodage canonique ───────────────────────────────────────────────

    #[test]
    fn encoding_is_injective() {
        // ("a","bc") et ("ab","c") doivent différer : c'est tout l'intérêt du
        // préfixe de longueur.
        let a = natural("L", "a", "bc");
        let b = natural("L", "ab", "c");
        assert_ne!(a.uuid(), b.uuid());
        assert_ne!(a.canonical_bytes(), b.canonical_bytes());
    }

    #[test]
    fn space_is_part_of_the_identity() {
        let base = Qualifier::Natural {
            property: "qualified_name".into(),
            value: "entity:mp_hodge".into(),
        };
        let default = EntityKey::new(None, "Page", base.clone()).unwrap();
        let tenant = EntityKey::new(Some("tenant-a".into()), "Page", base).unwrap();
        assert_ne!(
            default.uuid(),
            tenant.uuid(),
            "deux espaces ne peuvent pas partager une identité"
        );
    }

    #[test]
    fn property_name_is_part_of_the_identity() {
        // Sinon un `name` égal au `qualified_name` d'un autre fusionnerait deux entités.
        let as_name = natural("Page", "name", "entity:x");
        let as_qn = natural("Page", "qualified_name", "entity:x");
        assert_ne!(as_name.uuid(), as_qn.uuid());
    }

    #[test]
    fn uuid_is_version_8_with_rfc_variant() {
        let id = natural("Page", "qualified_name", "entity:mp_hodge").uuid();
        let b = id.as_bytes();
        assert_eq!(b[6] >> 4, 8, "version 8 : un id dérivé est reconnaissable");
        assert_eq!(b[8] & 0xc0, 0x80, "variante RFC 4122");
    }

    #[test]
    fn derivation_is_deterministic_and_survives_a_roundtrip() {
        let key = natural("Page", "qualified_name", "entity:mp_hodge");
        assert_eq!(key.uuid(), key.uuid());
        let json = serde_json::to_string(&key).unwrap();
        let back: EntityKey = serde_json::from_str(&json).expect("aller-retour");
        assert_eq!(back, key);
        assert_eq!(
            back.uuid(),
            key.uuid(),
            "l'identité survit à un aller-retour"
        );
    }

    #[test]
    fn golden_vectors_freeze_the_scheme() {
        // ⚠️ VECTEURS D'OR — GELÉS. Ces valeurs *sont* le schéma `v1`.
        // Les changer re-clérait TOUTES les entités déjà identifiées (liens,
        // références, empreintes de journal). Une évolution ⇒ nouveau schéma
        // versionné, jamais une modification de ces constantes.
        const K1_UUID: &str = "11049cd9-cc1d-82de-8571-3f307684cdac";
        const K1_BYTES: &str = concat!(
            "746869732d72732f656e746974792d69642f763100", // domaine "this-rs/entity-id/v1\0"
            "00",                                         // espace : None
            "0400000050616765",                           // label "Page"
            "01",                                         // qualifier : naturel
            "0e0000007175616c69666965645f6e616d65",       // "qualified_name"
            "0f000000656e746974793a6d705f686f646765"      // "entity:mp_hodge"
        );
        const K2_UUID: &str = "848c7e05-d578-83bc-87dd-15bc2ee2b490";

        let k1 = natural("Page", "qualified_name", "entity:mp_hodge");
        assert_eq!(ENTITY_ID_SCHEME, "this-rs/entity-id/sha256-128/v1");
        assert_eq!(k1.canonical_hex(), K1_BYTES);
        assert_eq!(k1.uuid().to_string(), K1_UUID);

        let k2 = EntityKey::new(
            Some("tenant-a".into()),
            "Function",
            Qualifier::Content {
                digest: "0123456789abcdef0123456789abcdef".into(),
            },
        )
        .unwrap();
        assert_eq!(k2.uuid().to_string(), K2_UUID);

        // Un décalage d'un seul octet doit tout changer (avalanche).
        let k1_off = natural("Page", "qualified_name", "entity:mp_hodg");
        assert_ne!(k1_off.uuid().to_string(), K1_UUID);
    }

    #[test]
    fn invalid_keys_are_rejected() {
        assert!(
            EntityKey::new(
                None,
                "",
                Qualifier::Natural {
                    property: "a".into(),
                    value: "b".into()
                }
            )
            .is_err()
        );
        assert!(
            EntityKey::new(
                None,
                "L",
                Qualifier::Natural {
                    property: "".into(),
                    value: "b".into()
                }
            )
            .is_err()
        );
        assert!(
            EntityKey::new(
                None,
                "L",
                Qualifier::Natural {
                    property: "a".into(),
                    value: "".into()
                }
            )
            .is_err()
        );
        assert!(
            EntityKey::new(
                None,
                "L",
                Qualifier::Content {
                    digest: "zz".into()
                }
            )
            .is_err()
        );
        assert!(
            EntityKey::new(
                Some("".into()),
                "L",
                Qualifier::Content {
                    digest: "0123456789abcdef0123456789abcdef".into()
                }
            )
            .is_err()
        );
        let long = "x".repeat(MAX_FIELD_BYTES + 1);
        assert!(
            EntityKey::new(
                None,
                long,
                Qualifier::Content {
                    digest: "0123456789abcdef0123456789abcdef".into()
                }
            )
            .is_err()
        );
        // …et la désérialisation valide aussi.
        let bad = r#"{"space":null,"label":"","qualifier":{"kind":"natural","property":"a","value":"b"}}"#;
        assert!(serde_json::from_str::<EntityKey>(bad).is_err());
    }

    // ── Chaîne de motifs ─────────────────────────────────────────────────

    #[test]
    fn convention_wins_and_produces_the_same_id_as_before() {
        let uuid = Uuid::new_v4();
        let v = view(&["Page"])
            .with("_uuid", PropValue::Str(uuid.to_string()))
            .with("qualified_name", PropValue::Str("entity:x".into()));
        let reg = IdentityRegistry::obrain_default();
        let e = reg.project(&v).expect("résolution");
        assert_eq!(e.origin, Provenance::Convention);
        assert_eq!(
            e.id, uuid,
            "l'id de la convention ne doit jamais être recalculé"
        );
        assert_eq!(e.motive, "convention:_uuid/_data");
    }

    #[test]
    fn unreadable_uuid_stops_the_chain_instead_of_rekeying() {
        let v = view(&["Page"])
            .with("_uuid", PropValue::Str("pas-un-uuid".into()))
            .with("qualified_name", PropValue::Str("entity:x".into()));
        let reg = IdentityRegistry::obrain_default();
        match reg.project(&v) {
            Err(IdentityError::Malformed { motive, .. }) => {
                assert_eq!(motive, "convention:_uuid/_data")
            }
            other => panic!("attendu Malformed (arrêt), obtenu {other:?}"),
        }
    }

    #[test]
    fn flat_properties_derive_an_identity_when_no_uuid() {
        let v = view(&["Page"]).with("qualified_name", PropValue::Str("entity:mp_hodge".into()));
        let reg = IdentityRegistry::obrain_default();
        let e = reg.project(&v).expect("résolution dérivée");
        assert_eq!(e.origin, Provenance::Derived);
        assert_eq!(e.motive, "derived:flat-property");
        assert_eq!(
            e.key.qualifier,
            Qualifier::Natural {
                property: "qualified_name".into(),
                value: "entity:mp_hodge".into()
            }
        );
        assert_eq!(e.id, e.key.uuid());
    }

    #[test]
    fn first_candidate_wins_in_priority_order() {
        let v = view(&["Page"])
            .with("name", PropValue::Str("nom-secondaire".into()))
            .with("qualified_name", PropValue::Str("entity:x".into()));
        let reg = IdentityRegistry::obrain_default();
        let e = reg.project(&v).unwrap();
        assert_eq!(
            e.key.qualifier,
            Qualifier::Natural {
                property: "qualified_name".into(),
                value: "entity:x".into()
            },
            "`qualified_name` est prioritaire sur `name`"
        );
    }

    #[test]
    fn canonical_label_is_the_smallest_non_reserved_one() {
        let reg = IdentityRegistry::new();
        // L'ordre rendu par le moteur n'est pas significatif.
        assert_eq!(
            reg.canonical_label(&["Z".into(), "Page".into()]).unwrap(),
            "Page"
        );
        assert_eq!(
            reg.canonical_label(&["Page".into(), "Z".into()]).unwrap(),
            "Page"
        );
        // this-rs stocke ses liens en nœuds `_link:*` : réservés.
        assert_eq!(
            reg.canonical_label(&["_link:owner".into(), "Page".into()])
                .unwrap(),
            "Page"
        );
        assert!(reg.canonical_label(&["_link:owner".into()]).is_none());
    }

    #[test]
    fn reserved_labels_are_unresolvable_not_entities() {
        let v = view(&["_link:owner"]);
        let reg = IdentityRegistry::obrain_default();
        match reg.project(&v) {
            Err(IdentityError::Unresolvable { reason }) => {
                assert!(
                    reason.contains("réservés"),
                    "raison explicite attendue : {reason}"
                )
            }
            other => panic!("attendu Unresolvable, obtenu {other:?}"),
        }
    }

    #[test]
    fn projection_is_pure_and_order_independent() {
        let v1 = view(&["Page", "Z"]).with("qualified_name", PropValue::Str("entity:x".into()));
        let v2 = view(&["Z", "Page"]).with("qualified_name", PropValue::Str("entity:x".into()));
        let reg = IdentityRegistry::obrain_default();
        let a = reg.project(&v1).unwrap();
        let b = reg.project(&v2).unwrap();
        assert_eq!(a.id, b.id, "permuter les labels ne change pas l'identité");
        // Deux projections successives rendent le même id (pureté).
        assert_eq!(reg.project(&v1).unwrap().id, a.id);
    }

    #[test]
    fn content_digest_is_opt_in_and_insensitive_to_volatile_props() {
        // `body` n'est PAS une propriété d'identité : le motif « propriétés
        // plates » ne peut donc pas résoudre ce nœud, et le repli décide.
        let mk = |updated: &str| {
            view(&["Doc"])
                .with("body", PropValue::Str("corps".into()))
                .with("updated_at", PropValue::Str(updated.into()))
        };
        // Par défaut : repli Skip ⇒ aucune identité (le nœud est ignoré).
        let reg = IdentityRegistry::obrain_default();
        assert_eq!(reg.project_or_skip(&mk("2026-01-01")).unwrap(), None);

        // Activé : le volatile est exclu du digest.
        let reg = IdentityRegistry::obrain_default().with_fallback(Fallback::ContentDigest);
        let a = reg.project(&mk("2026-01-01")).unwrap();
        let b = reg.project(&mk("2026-09-11")).unwrap();
        assert_eq!(a.id, b.id, "`updated_at` ne doit pas re-cléer l'entité");
        assert_eq!(a.origin, Provenance::Derived);
        assert_eq!(a.motive, "derived:content-digest");

        // …mais une propriété métier change l'identité.
        let c = reg
            .project(&view(&["Doc"]).with("body", PropValue::Str("autre".into())))
            .unwrap();
        assert_ne!(a.id, c.id);
    }

    #[test]
    fn declared_motives_never_produce_silence() {
        let v = view(&["Page"]).with("qualified_name", PropValue::Str("entity:x".into()));
        let declared: [&dyn EntityProjection; 2] = [&ApiProjection, &ChainProjection];
        for p in declared {
            match p.project(&v, "Page", None) {
                Err(IdentityError::Unresolvable { reason }) => {
                    assert!(reason.contains("non implémenté"))
                }
                other => panic!("attendu Unresolvable déclaré, obtenu {other:?}"),
            }
        }
    }

    #[test]
    fn registry_publishes_its_motives_with_convention_first() {
        let reg = IdentityRegistry::obrain_default();
        let m = reg.motives();
        assert_eq!(m[0].motive, "convention:_uuid/_data");
        assert_eq!(m[0].rank, 0);
        assert_eq!(m[0].origin, Provenance::Convention);
        assert_eq!(m[1].motive, "derived:flat-property");
        assert_eq!(m[1].origin, Provenance::Derived);

        // `push` insère APRÈS la convention, qui ne peut pas être déplacée.
        let mut reg2 = IdentityRegistry::new();
        reg2.push(Arc::new(FlatPropertyProjection::default()));
        assert_eq!(reg2.motives()[0].motive, "convention:_uuid/_data");
        assert_eq!(reg2.motives()[1].motive, "derived:flat-property");
    }

    #[test]
    fn project_or_skip_separates_skip_from_malformed() {
        let reg = IdentityRegistry::obrain_default();
        // Aucun motif applicable ⇒ None (ignoré), pas une erreur.
        let nothing = view(&["Page"]);
        assert_eq!(reg.project_or_skip(&nothing).unwrap(), None);
        // Incohérence ⇒ erreur remontée.
        let broken = view(&["Page"]).with("_uuid", PropValue::Str("pas-un-uuid".into()));
        assert!(reg.project_or_skip(&broken).is_err());
    }

    #[test]
    fn propvalue_canonical_encoding_is_typed_and_normalized() {
        let mut a = Vec::new();
        let mut b = Vec::new();
        PropValue::Int(1).write_canonical(&mut a);
        PropValue::Str("1".into()).write_canonical(&mut b);
        assert_ne!(a, b, "un entier et une chaîne ne peuvent pas coïncider");

        let mut z1 = Vec::new();
        let mut z2 = Vec::new();
        PropValue::Float(0.0).write_canonical(&mut z1);
        PropValue::Float(-0.0).write_canonical(&mut z2);
        assert_eq!(z1, z2, "-0.0 doit être normalisé");

        // JSON : l'ordre des clés ne change rien.
        let j1: serde_json::Value = serde_json::from_str(r#"{"b":2,"a":1}"#).unwrap();
        let j2: serde_json::Value = serde_json::from_str(r#"{"a":1,"b":2}"#).unwrap();
        let mut o1 = Vec::new();
        let mut o2 = Vec::new();
        PropValue::Json(j1).write_canonical(&mut o1);
        PropValue::Json(j2).write_canonical(&mut o2);
        assert_eq!(o1, o2);
    }

    // ── Index ────────────────────────────────────────────────────────────

    #[test]
    fn identity_index_refuses_a_foreign_space() {
        let mut idx = IdentityIndex::default();
        idx.clear(None);
        let a = Uuid::new_v4();
        assert!(
            idx.insert(None, a, 7),
            "graphe par défaut : insertion acceptée"
        );
        assert_eq!(idx.get(&a), Some(7));
        assert_eq!(idx.space(), None);
        assert_eq!(idx.len(), 1);

        // Un autre espace ne peut PAS écrire dans cet index : la garde est dans
        // le type, on ne peut pas l'oublier à un site d'appel.
        assert!(
            !idx.insert(Some("tenant-a"), Uuid::new_v4(), 1),
            "un espace étranger doit être refusé"
        );
        assert_eq!(idx.len(), 1, "le refus n'écrit rien");

        idx.clear(Some("tenant-a".into()));
        assert!(idx.is_empty(), "changer d'espace purge l'index");
        assert_eq!(idx.space(), Some("tenant-a"));
        assert!(idx.insert(Some("tenant-a"), a, 9));
        assert!(!idx.insert(None, a, 9));
    }
}
