//! # Le catalogue d'outils d'auteur
//!
//! Un agent qui sait écrire ses outils en écrira trop : le même, sous trois noms, avec
//! trois descriptions que le modèle relira à chaque tour. Ce module est le **noyau pur**
//! qui l'en empêche — il ne fait ni entrée/sortie ni horloge (le « temps » est un
//! compteur logique), donc il tourne identique dans une page wasm et dans une coquille.
//!
//! ## Ce qu'est un outil d'auteur
//!
//! Une **recette** : une suite bornée d'appels à des outils que l'hôte offre DÉJÀ, avec
//! des arguments qui reprennent les paramètres de l'appel (`"{{nom}}"`). Jamais du code.
//! Un outil d'auteur ne peut donc rien que ses étapes ne pouvaient déjà : le plafond de
//! capacités est hérité, jamais élargi (la garde vit dans
//! [`CatalogToolbox`](crate::CatalogToolbox)).
//!
//! **Deux non-fonctions, voulues.** Pas de `{{prev}}` : la sortie d'une étape n'entre pas
//! dans les arguments de la suivante. Le gate juge le NOM d'un outil, pas ses arguments :
//! un texte hostile rendu par un premier outil piloterait le second (confused deputy). Et
//! pas de récursion : une étape ne vise ni un outil du catalogue ni ses méta-outils.
//!
//! ## Ce que le catalogue garantit
//!
//! - **Pas de doublon.** Un nom pris, une recette identique à celle d'un outil actif, ou
//!   un outil trop proche (similarité lexicale) sont refusés — avec la liste des proches,
//!   pour que le modèle édite au lieu de recréer. Le « proche » seul se lève par une
//!   confirmation explicite ; le nom pris et la recette identique, jamais.
//! - **Versions append-only.** Éditer crée une version ; revenir en arrière crée une
//!   version qui COPIE l'ancienne. Rien n'est réécrit. L'édition exige la version
//!   courante attendue : deux éditeurs ne s'écrasent pas sans le savoir.
//! - **Renforcement mesuré.** Chaque appel compte (réussi, échoué, refusé) PAR VERSION.
//!   La fiabilité (lissée de Laplace) départage les résultats d'une recherche, et un outil
//!   qui échoue plus qu'il ne réussit est signalé « fragile ».
//! - **Recherche.** Lexicale, pondérée (nom ×3, étiquettes ×2, description ×1), sans
//!   accents ni pluriel simple : le coût d'un outil dans le contexte est payé à la demande.
//!
//! ## Ce qu'il ne garantit pas
//!
//! La recherche est lexicale : « liste mes courses » ne retrouve pas « shopping » si aucun
//! mot n'est partagé. Un vecteur dense ([`crate::Embed`]) la compléterait ; ce n'est pas fait.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Version du format de persistance. Un format inconnu est REFUSÉ à la lecture : lire un
/// état qu'on ne comprend pas, c'est l'appliquer de travers.
pub const FORMAT: u32 = 1;

/// Longueur maximale d'un nom, en caractères.
pub const MAX_NAME_CHARS: usize = 40;
/// Longueur minimale d'un nom.
pub const MIN_NAME_CHARS: usize = 3;
/// Longueur maximale d'une description : elle est relue à chaque tour où l'outil est chargé.
pub const MAX_DESCRIPTION_CHARS: usize = 300;
/// Nombre maximal d'étapes d'une recette.
pub const MAX_STEPS: usize = 8;
/// Nombre maximal d'étiquettes.
pub const MAX_TAGS: usize = 8;
/// Longueur maximale d'une étiquette.
pub const MAX_TAG_CHARS: usize = 24;
/// Nombre maximal de paramètres.
pub const MAX_PARAMS: usize = 12;
/// Taille maximale du schéma de paramètres, sérialisé.
pub const MAX_SCHEMA_BYTES: usize = 4_096;
/// Taille maximale des arguments d'une étape, une fois les paramètres substitués.
pub const MAX_STEP_ARGS_BYTES: usize = 16_384;
/// Nombre maximal d'outils (actifs ou retirés) : un catalogue sans borne est une fuite.
pub const MAX_TOOLS: usize = 200;
/// Versions gardées par outil. Au-delà, la plus ancienne sort ; la courante jamais.
pub const MAX_VERSIONS: usize = 20;
/// Similarité (cosinus, 0..1) à partir de laquelle un outil est « proche » d'un autre.
pub const SIMILAR_MIN: f64 = 0.6;
/// Score minimal d'un résultat de recherche : en dessous, c'est du bruit.
pub const SEARCH_MIN: f64 = 0.08;

/// Une étape : un outil de l'hôte et ses arguments (`"{{param}}"` = un paramètre de l'appel).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RecipeStep {
    pub tool: String,
    #[serde(default)]
    pub args: Value,
}

/// Les compteurs d'une version.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolStats {
    pub uses: u32,
    pub ok: u32,
    pub failed: u32,
    pub refused: u32,
    /// Horloge logique du dernier appel (0 = jamais appelé).
    pub last_used: u64,
}

impl ToolStats {
    /// Fiabilité lissée : un outil jamais appelé vaut 0,5, pas 0 ni 1.
    pub fn reliability(&self) -> f64 {
        (f64::from(self.ok) + 1.0) / (f64::from(self.uses) + 2.0)
    }

    /// Vrai si l'outil a servi au moins trois fois et échoue plus qu'il ne réussit.
    pub fn fragile(&self) -> bool {
        self.uses >= 3 && self.ok * 2 < self.uses
    }
}

/// Ce que l'appel d'un outil a donné, pour le renforcement.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Usage {
    Ok,
    Failed,
    Refused,
}

/// Une version d'un outil.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ToolDef {
    pub version: u32,
    pub description: String,
    /// Un JSON Schema d'objet : `{"type":"object","properties":{...},"required":[...]}`.
    pub parameters: Value,
    pub steps: Vec<RecipeStep>,
    pub tags: Vec<String>,
    /// Pourquoi cette version existe (une phrase).
    pub note: String,
    /// Horloge logique de la création.
    pub seq: u64,
    #[serde(default)]
    pub stats: ToolStats,
}

impl ToolDef {
    fn same_content(&self, other: &ToolDef) -> bool {
        self.description == other.description
            && self.parameters == other.parameters
            && self.steps == other.steps
            && self.tags == other.tags
    }
}

/// L'état d'un outil.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum ToolStatus {
    Active,
    Retired {
        reason: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        replaced_by: Option<String>,
    },
}

/// Un outil et tout son historique.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ToolRecord {
    pub name: String,
    /// Du plus ancien au plus récent ; la dernière est la courante. Jamais vide.
    pub versions: Vec<ToolDef>,
    pub status: ToolStatus,
    /// Versions anciennes sorties du plafond [`MAX_VERSIONS`].
    #[serde(default)]
    pub dropped: u32,
}

impl ToolRecord {
    pub fn current(&self) -> &ToolDef {
        self.versions
            .last()
            .expect("un enregistrement a toujours une version")
    }

    pub fn version(&self, version: u32) -> Option<&ToolDef> {
        self.versions.iter().find(|d| d.version == version)
    }

    pub fn is_active(&self) -> bool {
        self.status == ToolStatus::Active
    }
}

/// Ce qu'on propose de créer.
#[derive(Clone, Debug)]
pub struct ToolDraft {
    pub name: String,
    pub description: String,
    pub parameters: Value,
    pub steps: Vec<RecipeStep>,
    pub tags: Vec<String>,
    /// « Je sais qu'il ressemble à un autre, et il en est distinct. » Ne lève ni un nom
    /// pris ni une recette identique.
    pub confirm_distinct: bool,
}

/// Ce qu'on change : seul ce qui est `Some` change.
#[derive(Clone, Debug, Default)]
pub struct ToolChange {
    pub description: Option<String>,
    pub parameters: Option<Value>,
    pub steps: Option<Vec<RecipeStep>>,
    pub tags: Option<Vec<String>>,
    pub note: String,
}

/// Un résultat de recherche.
#[derive(Clone, Debug, PartialEq)]
pub struct CatalogHit {
    pub name: String,
    pub version: u32,
    pub description: String,
    /// Pertinence seule, 0..1.
    pub score: f64,
    pub reliability: f64,
    pub uses: u32,
    pub fragile: bool,
}

/// Pourquoi le catalogue refuse. Chaque message dit quoi faire à la place.
#[derive(Clone, Debug, PartialEq)]
pub enum CatalogError {
    BadName(String),
    BadDescription(String),
    BadParameters(String),
    BadSteps(String),
    BadTags(String),
    NameTaken {
        name: String,
        version: u32,
    },
    Duplicate {
        of: String,
    },
    Similar {
        candidates: Vec<(String, u8)>,
    },
    Unknown(String),
    UnknownVersion {
        name: String,
        version: u32,
    },
    Stale {
        name: String,
        expected: u32,
        current: u32,
    },
    Retired(String),
    Full,
    NothingChanged,
}

impl CatalogError {
    /// Un code stable, pour les tests et les journaux.
    pub fn code(&self) -> &'static str {
        match self {
            Self::BadName(_) => "bad_name",
            Self::BadDescription(_) => "bad_description",
            Self::BadParameters(_) => "bad_parameters",
            Self::BadSteps(_) => "bad_steps",
            Self::BadTags(_) => "bad_tags",
            Self::NameTaken { .. } => "name_taken",
            Self::Duplicate { .. } => "duplicate",
            Self::Similar { .. } => "similar",
            Self::Unknown(_) => "unknown",
            Self::UnknownVersion { .. } => "unknown_version",
            Self::Stale { .. } => "stale",
            Self::Retired(_) => "retired",
            Self::Full => "full",
            Self::NothingChanged => "nothing_changed",
        }
    }
}

impl std::fmt::Display for CatalogError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::BadName(why) => write!(f, "nom refusé : {why}"),
            Self::BadDescription(why) => write!(f, "description refusée : {why}"),
            Self::BadParameters(why) => write!(f, "paramètres refusés : {why}"),
            Self::BadSteps(why) => write!(f, "recette refusée : {why}"),
            Self::BadTags(why) => write!(f, "étiquettes refusées : {why}"),
            Self::NameTaken { name, version } => write!(
                f,
                "« {name} » existe déjà (v{version}) : modifie-le avec tools_author action=edit \
                 plutôt que d'en créer un second"
            ),
            Self::Duplicate { of } => write!(
                f,
                "cette recette est identique à celle de « {of} » : utilise-le, ou modifie-le"
            ),
            Self::Similar { candidates } => {
                let liste = candidates
                    .iter()
                    .map(|(n, p)| format!("{n} ({p} %)"))
                    .collect::<Vec<_>>()
                    .join(", ");
                write!(
                    f,
                    "outil trop proche de : {liste}. Regarde-les (tools_find action=show) et \
                     modifie-en un ; s'il est vraiment distinct, rappelle avec confirm_distinct=true"
                )
            }
            Self::Unknown(name) => write!(f, "aucun outil « {name} » dans le catalogue"),
            Self::UnknownVersion { name, version } => {
                write!(f, "« {name} » n'a pas de version {version}")
            }
            Self::Stale {
                name,
                expected,
                current,
            } => write!(
                f,
                "« {name} » est en v{current}, pas v{expected} : relis-le (tools_find \
                 action=show) avant de le modifier"
            ),
            Self::Retired(name) => write!(
                f,
                "« {name} » est retiré : rollback vers une de ses versions le réactive"
            ),
            Self::Full => write!(
                f,
                "catalogue plein ({MAX_TOOLS} outils) : retire un outil inutilisé"
            ),
            Self::NothingChanged => {
                write!(f, "rien ne change : la version courante est déjà celle-là")
            }
        }
    }
}

/// Le catalogue : des outils nommés, leur historique, et une horloge logique.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Catalog {
    tools: BTreeMap<String, ToolRecord>,
    seq: u64,
}

/// Le contrôle que l'hôte fait d'un outil visé par une étape (« existe-t-il, est-il permis
/// dans une recette ? »). Rend le motif du refus.
pub type StepCheck<'a> = &'a dyn Fn(&str) -> Result<(), String>;

impl Catalog {
    pub fn new() -> Self {
        Self::default()
    }

    /// Nombre d'outils (actifs ou retirés).
    pub fn len(&self) -> usize {
        self.tools.len()
    }

    pub fn is_empty(&self) -> bool {
        self.tools.is_empty()
    }

    pub fn get(&self, name: &str) -> Option<&ToolRecord> {
        self.tools.get(name)
    }

    /// Les outils actifs, par nom.
    pub fn active(&self) -> impl Iterator<Item = &ToolRecord> {
        self.tools.values().filter(|r| r.is_active())
    }

    fn tick(&mut self) -> u64 {
        self.seq += 1;
        self.seq
    }

    // ----- création ---------------------------------------------------------------

    /// Crée un outil (version 1), ou dit pourquoi pas.
    pub fn create(
        &mut self,
        draft: ToolDraft,
        step_ok: StepCheck<'_>,
    ) -> Result<u32, CatalogError> {
        let name = draft.name.trim().to_string();
        check_name(&name)?;
        if let Some(existing) = self.tools.get(&name) {
            return Err(CatalogError::NameTaken {
                name,
                version: existing.current().version,
            });
        }
        let description = check_description(&draft.description)?;
        check_parameters(&draft.parameters)?;
        let parameters = normalize_parameters(&draft.parameters);
        let tags = check_tags(&draft.tags)?;
        check_steps(&draft.steps, &parameters, step_ok)?;
        if self.tools.len() >= MAX_TOOLS {
            return Err(CatalogError::Full);
        }
        if let Some(of) = self.identical_steps(&draft.steps, None) {
            return Err(CatalogError::Duplicate { of });
        }
        if !draft.confirm_distinct {
            let near = self.similar_to(&name, &tags, &description);
            if !near.is_empty() {
                return Err(CatalogError::Similar { candidates: near });
            }
        }
        let seq = self.tick();
        self.tools.insert(
            name.clone(),
            ToolRecord {
                name,
                versions: vec![ToolDef {
                    version: 1,
                    description,
                    parameters,
                    steps: draft.steps,
                    tags,
                    note: "création".to_string(),
                    seq,
                    stats: ToolStats::default(),
                }],
                status: ToolStatus::Active,
                dropped: 0,
            },
        );
        Ok(1)
    }

    // ----- édition ----------------------------------------------------------------

    /// Crée une version de `name` à partir de la version `expected_version`, qui doit
    /// être la courante. Rend la nouvelle version.
    pub fn edit(
        &mut self,
        name: &str,
        expected_version: u32,
        change: ToolChange,
        step_ok: StepCheck<'_>,
    ) -> Result<u32, CatalogError> {
        let record = self
            .tools
            .get(name)
            .ok_or_else(|| CatalogError::Unknown(name.to_string()))?;
        if !record.is_active() {
            return Err(CatalogError::Retired(name.to_string()));
        }
        let current = record.current();
        if current.version != expected_version {
            return Err(CatalogError::Stale {
                name: name.to_string(),
                expected: expected_version,
                current: current.version,
            });
        }
        let description = match &change.description {
            Some(d) => check_description(d)?,
            None => current.description.clone(),
        };
        let parameters = match &change.parameters {
            Some(p) => {
                check_parameters(p)?;
                normalize_parameters(p)
            }
            None => current.parameters.clone(),
        };
        let tags = match &change.tags {
            Some(t) => check_tags(t)?,
            None => current.tags.clone(),
        };
        let steps = change
            .steps
            .clone()
            .unwrap_or_else(|| current.steps.clone());
        // Les étapes se revalident TOUJOURS : changer les paramètres peut rendre un
        // `{{x}}` orphelin, et l'hôte a pu retirer un outil depuis la création.
        check_steps(&steps, &parameters, step_ok)?;
        let candidate = ToolDef {
            version: current.version + 1,
            description,
            parameters,
            steps,
            tags,
            note: clip(change.note.trim(), MAX_DESCRIPTION_CHARS),
            seq: 0,
            stats: ToolStats::default(),
        };
        if candidate.same_content(current) {
            return Err(CatalogError::NothingChanged);
        }
        if let Some(of) = self.identical_steps(&candidate.steps, Some(name)) {
            return Err(CatalogError::Duplicate { of });
        }
        let seq = self.tick();
        Ok(self.push_version(name, ToolDef { seq, ..candidate }))
    }

    /// Crée une version qui COPIE `to_version`. L'historique n'est jamais réécrit ; un
    /// outil retiré redevient actif.
    pub fn rollback(
        &mut self,
        name: &str,
        to_version: u32,
        expected_version: u32,
        step_ok: StepCheck<'_>,
    ) -> Result<u32, CatalogError> {
        let record = self
            .tools
            .get(name)
            .ok_or_else(|| CatalogError::Unknown(name.to_string()))?;
        let current = record.current();
        if current.version != expected_version {
            return Err(CatalogError::Stale {
                name: name.to_string(),
                expected: expected_version,
                current: current.version,
            });
        }
        let next = current.version + 1;
        let old = record
            .version(to_version)
            .ok_or_else(|| CatalogError::UnknownVersion {
                name: name.to_string(),
                version: to_version,
            })?
            .clone();
        if old.same_content(current) && record.is_active() {
            return Err(CatalogError::NothingChanged);
        }
        check_steps(&old.steps, &old.parameters, step_ok)?;
        if let Some(of) = self.identical_steps(&old.steps, Some(name)) {
            return Err(CatalogError::Duplicate { of });
        }
        let seq = self.tick();
        let version = self.push_version(
            name,
            ToolDef {
                version: next,
                note: format!("retour à la v{to_version}"),
                seq,
                stats: ToolStats::default(),
                ..old
            },
        );
        if let Some(record) = self.tools.get_mut(name) {
            record.status = ToolStatus::Active;
        }
        Ok(version)
    }

    /// Retire un outil du catalogue actif (jamais supprimé : son historique reste lisible).
    pub fn retire(
        &mut self,
        name: &str,
        reason: &str,
        replaced_by: Option<&str>,
    ) -> Result<(), CatalogError> {
        let record = self
            .tools
            .get(name)
            .ok_or_else(|| CatalogError::Unknown(name.to_string()))?;
        if !record.is_active() {
            return Err(CatalogError::Retired(name.to_string()));
        }
        if let Some(other) = replaced_by {
            match self.tools.get(other) {
                Some(r) if r.is_active() && other != name => {}
                _ => return Err(CatalogError::Unknown(other.to_string())),
            }
        }
        if let Some(record) = self.tools.get_mut(name) {
            record.status = ToolStatus::Retired {
                reason: clip(reason.trim(), MAX_DESCRIPTION_CHARS),
                replaced_by: replaced_by.map(str::to_string),
            };
        }
        Ok(())
    }

    fn push_version(&mut self, name: &str, def: ToolDef) -> u32 {
        let version = def.version;
        if let Some(record) = self.tools.get_mut(name) {
            record.versions.push(def);
            while record.versions.len() > MAX_VERSIONS {
                record.versions.remove(0);
                record.dropped += 1;
            }
        }
        version
    }

    // ----- renforcement -----------------------------------------------------------

    /// Compte un appel de `name` en `version`. Un outil ou une version inconnus (retirée
    /// du plafond) ne comptent pour personne : rien n'est inventé.
    pub fn record_use(&mut self, name: &str, version: u32, used: Usage) {
        let tick = self.tick();
        let Some(record) = self.tools.get_mut(name) else {
            return;
        };
        let Some(def) = record.versions.iter_mut().find(|d| d.version == version) else {
            return;
        };
        def.stats.uses = def.stats.uses.saturating_add(1);
        match used {
            Usage::Ok => def.stats.ok = def.stats.ok.saturating_add(1),
            Usage::Failed => def.stats.failed = def.stats.failed.saturating_add(1),
            Usage::Refused => def.stats.refused = def.stats.refused.saturating_add(1),
        }
        def.stats.last_used = tick;
    }

    // ----- recherche --------------------------------------------------------------

    /// Les `limit` meilleurs outils ACTIFS pour `query`, du plus pertinent au moins.
    ///
    /// Le classement est `pertinence × (0,75 + 0,25 × fiabilité)` : à pertinence égale, un
    /// outil qui réussit passe devant un outil qui échoue. La pertinence seule est rendue
    /// dans [`CatalogHit::score`].
    pub fn search(&self, query: &str, limit: usize) -> Vec<CatalogHit> {
        let q_tokens = tokens(query);
        if q_tokens.is_empty() || limit == 0 {
            return Vec::new();
        }
        let docs: Vec<(&ToolRecord, BTreeMap<String, f64>)> = self
            .active()
            .map(|r| {
                let d = r.current();
                (r, doc_vector(&r.name, &d.tags, &d.description))
            })
            .collect();
        let n = docs.len() as f64;
        let mut df: BTreeMap<&str, f64> = BTreeMap::new();
        for (_, v) in &docs {
            for t in v.keys() {
                *df.entry(t.as_str()).or_insert(0.0) += 1.0;
            }
        }
        let idf = |t: &str| (1.0 + n / df.get(t).copied().unwrap_or(1.0)).ln();
        let mut q: BTreeMap<String, f64> = BTreeMap::new();
        for t in &q_tokens {
            *q.entry(t.clone()).or_insert(0.0) += 1.0;
        }
        let q: BTreeMap<String, f64> = q
            .into_iter()
            .filter(|(t, _)| df.contains_key(t.as_str()))
            .map(|(t, w)| {
                let weight = w * idf(&t);
                (t, weight)
            })
            .collect();
        if q.is_empty() {
            return Vec::new();
        }
        let mut hits: Vec<(f64, u64, CatalogHit)> = docs
            .iter()
            .filter_map(|(record, vector)| {
                let weighted: BTreeMap<String, f64> = vector
                    .iter()
                    .map(|(t, w)| (t.clone(), w * idf(t)))
                    .collect();
                let score = cosine(&q, &weighted);
                if score < SEARCH_MIN {
                    return None;
                }
                let def = record.current();
                let reliability = def.stats.reliability();
                Some((
                    score * (0.75 + 0.25 * reliability),
                    def.stats.last_used.max(def.seq),
                    CatalogHit {
                        name: record.name.clone(),
                        version: def.version,
                        description: def.description.clone(),
                        score,
                        reliability,
                        uses: def.stats.uses,
                        fragile: def.stats.fragile(),
                    },
                ))
            })
            .collect();
        hits.sort_by(|a, b| {
            b.0.partial_cmp(&a.0)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then(b.1.cmp(&a.1))
                .then(a.2.name.cmp(&b.2.name))
        });
        hits.into_iter().take(limit).map(|(_, _, h)| h).collect()
    }

    /// Les outils actifs proches d'une description (cosinus brut, sans idf), jusqu'à trois.
    fn similar_to(&self, name: &str, tags: &[String], description: &str) -> Vec<(String, u8)> {
        let candidate = doc_vector(name, tags, description);
        let mut near: Vec<(String, f64)> = self
            .active()
            .filter_map(|r| {
                let d = r.current();
                let score = cosine(&candidate, &doc_vector(&r.name, &d.tags, &d.description));
                (score >= SIMILAR_MIN).then(|| (r.name.clone(), score))
            })
            .collect();
        near.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        near.into_iter()
            .take(3)
            .map(|(n, s)| (n, (s * 100.0).round() as u8))
            .collect()
    }

    /// Un outil ACTIF (autre que `except`) dont la recette est exactement celle-ci.
    fn identical_steps(&self, steps: &[RecipeStep], except: Option<&str>) -> Option<String> {
        let canon = canonical_steps(steps);
        self.active()
            .filter(|r| Some(r.name.as_str()) != except)
            .find(|r| canonical_steps(&r.current().steps) == canon)
            .map(|r| r.name.clone())
    }

    // ----- persistance ------------------------------------------------------------

    /// Le catalogue entier, sous une enveloppe versionnée.
    pub fn to_json(&self) -> String {
        serde_json::json!({
            "format": FORMAT,
            "seq": self.seq,
            "tools": self.tools.values().collect::<Vec<_>>(),
        })
        .to_string()
    }

    /// Relit un catalogue. Un format inconnu, un enregistrement vide ou une version
    /// courante incohérente est une ERREUR : un état qu'on ne comprend pas n'est pas
    /// appliqué.
    pub fn from_json(json: &str) -> Result<Self, String> {
        #[derive(Deserialize)]
        struct Envelope {
            format: Option<u32>,
            #[serde(default)]
            seq: u64,
            #[serde(default)]
            tools: Vec<ToolRecord>,
        }
        let envelope: Envelope =
            serde_json::from_str(json).map_err(|e| format!("catalogue illisible : {e}"))?;
        match envelope.format {
            Some(FORMAT) => {}
            Some(other) => {
                return Err(format!(
                    "format de catalogue {other} inconnu (celui-ci lit le {FORMAT})"
                ));
            }
            None => return Err("catalogue sans `format`".to_string()),
        }
        if envelope.tools.len() > MAX_TOOLS {
            return Err(format!("catalogue de plus de {MAX_TOOLS} outils"));
        }
        let mut tools = BTreeMap::new();
        for record in envelope.tools {
            check_name(&record.name).map_err(|e| e.to_string())?;
            let Some(last) = record.versions.last() else {
                return Err(format!("« {} » n'a aucune version", record.name));
            };
            if record
                .versions
                .windows(2)
                .any(|w| w[0].version >= w[1].version)
            {
                return Err(format!("« {} » : versions hors d'ordre", record.name));
            }
            if last.steps.len() > MAX_STEPS {
                return Err(format!("« {} » : plus de {MAX_STEPS} étapes", record.name));
            }
            tools.insert(record.name.clone(), record);
        }
        Ok(Self {
            tools,
            seq: envelope.seq,
        })
    }
}

// ----- validation -----------------------------------------------------------------

fn check_name(name: &str) -> Result<(), CatalogError> {
    let ok_len = (MIN_NAME_CHARS..=MAX_NAME_CHARS).contains(&name.chars().count());
    let mut chars = name.chars();
    let ok_first = chars.next().is_some_and(|c| c.is_ascii_lowercase());
    let ok_rest = name
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_');
    if ok_len && ok_first && ok_rest {
        Ok(())
    } else {
        Err(CatalogError::BadName(format!(
            "{MIN_NAME_CHARS} à {MAX_NAME_CHARS} caractères, une minuscule d'abord, puis \
             minuscules, chiffres ou « _ » (reçu « {} »)",
            clip(name, 60)
        )))
    }
}

fn check_description(description: &str) -> Result<String, CatalogError> {
    let d = description.trim();
    if d.is_empty() {
        return Err(CatalogError::BadDescription(
            "une description est nécessaire : c'est ce que la recherche lit".to_string(),
        ));
    }
    if d.chars().count() > MAX_DESCRIPTION_CHARS {
        return Err(CatalogError::BadDescription(format!(
            "plus de {MAX_DESCRIPTION_CHARS} caractères : elle est relue à chaque tour"
        )));
    }
    Ok(d.to_string())
}

fn check_tags(tags: &[String]) -> Result<Vec<String>, CatalogError> {
    let mut out: Vec<String> = Vec::new();
    for tag in tags {
        let t = tag.trim().to_lowercase();
        if t.is_empty() {
            continue;
        }
        if t.chars().count() > MAX_TAG_CHARS
            || !t
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-')
        {
            return Err(CatalogError::BadTags(format!(
                "« {} » : {MAX_TAG_CHARS} caractères au plus, minuscules, chiffres, « _ » ou « - »",
                clip(&t, 40)
            )));
        }
        if !out.contains(&t) {
            out.push(t);
        }
    }
    if out.len() > MAX_TAGS {
        return Err(CatalogError::BadTags(format!(
            "plus de {MAX_TAGS} étiquettes"
        )));
    }
    Ok(out)
}

const PARAM_TYPES: [&str; 6] = ["string", "integer", "number", "boolean", "array", "object"];

fn check_parameters(parameters: &Value) -> Result<(), CatalogError> {
    let bad = |why: &str| Err(CatalogError::BadParameters(why.to_string()));
    let Some(obj) = parameters.as_object() else {
        return bad("un objet `{\"type\":\"object\",\"properties\":{…}}` est attendu");
    };
    if obj.get("type").and_then(Value::as_str) != Some("object") {
        return bad("`type` doit valoir \"object\"");
    }
    let Some(props) = obj.get("properties").and_then(Value::as_object) else {
        return bad("`properties` doit être un objet (vide si l'outil n'a aucun paramètre)");
    };
    if props.len() > MAX_PARAMS {
        return Err(CatalogError::BadParameters(format!(
            "plus de {MAX_PARAMS} paramètres"
        )));
    }
    for (key, spec) in props {
        let valid_key = !key.is_empty()
            && key.chars().next().is_some_and(|c| c.is_ascii_lowercase())
            && key
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_');
        if !valid_key {
            return Err(CatalogError::BadParameters(format!(
                "nom de paramètre « {} » : minuscules, chiffres, « _ »",
                clip(key, 40)
            )));
        }
        let kind = spec.get("type").and_then(Value::as_str).unwrap_or("");
        if !PARAM_TYPES.contains(&kind) {
            return Err(CatalogError::BadParameters(format!(
                "paramètre « {key} » : `type` parmi {}",
                PARAM_TYPES.join(", ")
            )));
        }
    }
    if let Some(required) = obj.get("required") {
        let Some(list) = required.as_array() else {
            return bad("`required` doit être une liste de noms");
        };
        for r in list {
            match r.as_str() {
                Some(name) if props.contains_key(name) => {}
                _ => return bad("`required` ne nomme que des paramètres déclarés"),
            }
        }
    }
    if parameters.to_string().len() > MAX_SCHEMA_BYTES {
        return Err(CatalogError::BadParameters(format!(
            "schéma de plus de {MAX_SCHEMA_BYTES} octets"
        )));
    }
    Ok(())
}

/// Le schéma tel qu'il est RANGÉ, validé et annoncé : `type`, `properties` réduites à leur
/// `type`, `required`. Tout le reste (`description` d'un paramètre, `enum`, clés inconnues)
/// disparaît : ce texte d'auteur entrerait dans la liste d'outils sans passer par le
/// conteneur non fiable. Le sens d'un paramètre se dit dans la description de l'OUTIL.
fn normalize_parameters(parameters: &Value) -> Value {
    let props: serde_json::Map<String, Value> = parameters
        .get("properties")
        .and_then(Value::as_object)
        .map(|p| {
            p.iter()
                .map(|(k, v)| {
                    let kind = v.get("type").and_then(Value::as_str).unwrap_or("string");
                    (k.clone(), serde_json::json!({ "type": kind }))
                })
                .collect()
        })
        .unwrap_or_default();
    let required: Vec<Value> = parameters
        .get("required")
        .and_then(Value::as_array)
        .map(|r| r.iter().filter(|n| n.is_string()).cloned().collect())
        .unwrap_or_default();
    serde_json::json!({ "type": "object", "properties": props, "required": required })
}

fn check_steps(
    steps: &[RecipeStep],
    parameters: &Value,
    step_ok: StepCheck<'_>,
) -> Result<(), CatalogError> {
    if steps.is_empty() {
        return Err(CatalogError::BadSteps(
            "au moins une étape : un outil sans étape ne fait rien".to_string(),
        ));
    }
    if steps.len() > MAX_STEPS {
        return Err(CatalogError::BadSteps(format!(
            "{} étapes, au plus {MAX_STEPS} : découpe en deux outils",
            steps.len()
        )));
    }
    let declared: BTreeSet<&str> = parameters
        .get("properties")
        .and_then(Value::as_object)
        .map(|p| p.keys().map(String::as_str).collect())
        .unwrap_or_default();
    let mut used: BTreeSet<String> = BTreeSet::new();
    for (i, step) in steps.iter().enumerate() {
        step_ok(&step.tool).map_err(|why| {
            CatalogError::BadSteps(format!("étape {} « {} » : {why}", i + 1, step.tool))
        })?;
        if !(step.args.is_null() || step.args.is_object()) {
            return Err(CatalogError::BadSteps(format!(
                "étape {} : `args` est un objet",
                i + 1
            )));
        }
        placeholders(&step.args, &mut used)
            .map_err(|why| CatalogError::BadSteps(format!("étape {} : {why}", i + 1)))?;
        if step.args.to_string().len() > MAX_STEP_ARGS_BYTES {
            return Err(CatalogError::BadSteps(format!(
                "étape {} : arguments de plus de {MAX_STEP_ARGS_BYTES} octets",
                i + 1
            )));
        }
    }
    if let Some(orphan) = used.iter().find(|p| !declared.contains(p.as_str())) {
        return Err(CatalogError::BadSteps(format!(
            "« {{{{{orphan}}}}} » n'est pas un paramètre de l'outil (paramètres : {})",
            if declared.is_empty() {
                "aucun".to_string()
            } else {
                declared.iter().copied().collect::<Vec<_>>().join(", ")
            }
        )));
    }
    Ok(())
}

// ----- paramètres : substitution et validation d'appel ----------------------------

/// Les noms `{{x}}` d'un arbre JSON. Une accolade ouverte sans fermeture, ou un nom
/// invalide, est une erreur : un `{{` qui n'en est pas un partirait tel quel vers l'outil.
fn placeholders(value: &Value, out: &mut BTreeSet<String>) -> Result<(), String> {
    match value {
        Value::String(s) => {
            let mut rest = s.as_str();
            while let Some(start) = rest.find("{{") {
                let after = &rest[start + 2..];
                let Some(end) = after.find("}}") else {
                    return Err("« {{ » sans « }} »".to_string());
                };
                let name = after[..end].trim();
                let valid = !name.is_empty()
                    && name.chars().next().is_some_and(|c| c.is_ascii_lowercase())
                    && name
                        .chars()
                        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_');
                if !valid {
                    return Err(format!(
                        "« {{{{{}}}}} » : un nom de paramètre est attendu",
                        clip(name, 40)
                    ));
                }
                out.insert(name.to_string());
                rest = &after[end + 2..];
            }
            Ok(())
        }
        Value::Array(items) => items.iter().try_for_each(|v| placeholders(v, out)),
        Value::Object(map) => map.values().try_for_each(|v| placeholders(v, out)),
        _ => Ok(()),
    }
}

/// Remplace les `{{x}}` par les paramètres de l'appel.
///
/// Une chaîne qui est EXACTEMENT `{{x}}` reprend la valeur typée du paramètre ; ailleurs
/// elle s'intercale en texte. Un paramètre optionnel absent retire la clé (ou l'élément)
/// au lieu d'envoyer `null` : un outil qui reçoit `"limit": null` ne lit pas un défaut.
pub fn render_args(template: &Value, call: &Value) -> Result<Value, String> {
    let rendered = render(template, call).unwrap_or(Value::Null);
    let out = if rendered.is_null() {
        Value::Object(serde_json::Map::new())
    } else {
        rendered
    };
    if out.to_string().len() > MAX_STEP_ARGS_BYTES {
        return Err(format!(
            "arguments de plus de {MAX_STEP_ARGS_BYTES} octets après substitution"
        ));
    }
    Ok(out)
}

fn render(template: &Value, call: &Value) -> Option<Value> {
    match template {
        Value::String(s) => {
            let t = s.trim();
            if t.starts_with("{{") && t.ends_with("}}") && t.matches("{{").count() == 1 {
                let name = t[2..t.len() - 2].trim();
                return call.get(name).filter(|v| !v.is_null()).cloned();
            }
            let mut out = String::new();
            let mut rest = s.as_str();
            while let Some(start) = rest.find("{{") {
                out.push_str(&rest[..start]);
                let after = &rest[start + 2..];
                let Some(end) = after.find("}}") else {
                    out.push_str(&rest[start..]);
                    rest = "";
                    break;
                };
                if let Some(v) = call.get(after[..end].trim()) {
                    match v {
                        Value::String(text) => out.push_str(text),
                        Value::Null => {}
                        other => out.push_str(&other.to_string()),
                    }
                }
                rest = &after[end + 2..];
            }
            out.push_str(rest);
            Some(Value::String(out))
        }
        Value::Array(items) => Some(Value::Array(
            items.iter().filter_map(|v| render(v, call)).collect(),
        )),
        Value::Object(map) => Some(Value::Object(
            map.iter()
                .filter_map(|(k, v)| render(v, call).map(|r| (k.clone(), r)))
                .collect(),
        )),
        other => Some(other.clone()),
    }
}

/// Vérifie les arguments d'un appel contre le schéma de l'outil : un objet, les `required`
/// présents, aucune clé inconnue, chaque type respecté. Rend le motif du refus.
pub fn validate_args(parameters: &Value, args: &Value) -> Result<(), String> {
    let empty = serde_json::Map::new();
    let given = match args {
        Value::Null => &empty,
        Value::Object(map) => map,
        _ => return Err("les arguments doivent former un objet".to_string()),
    };
    let props = parameters
        .get("properties")
        .and_then(Value::as_object)
        .unwrap_or(&empty);
    if let Some(unknown) = given.keys().find(|k| !props.contains_key(k.as_str())) {
        return Err(format!("paramètre inconnu « {} »", clip(unknown, 40)));
    }
    if let Some(required) = parameters.get("required").and_then(Value::as_array) {
        for name in required.iter().filter_map(Value::as_str) {
            if given.get(name).is_none_or(Value::is_null) {
                return Err(format!("paramètre « {name} » manquant"));
            }
        }
    }
    for (key, value) in given {
        if value.is_null() {
            continue;
        }
        let kind = props
            .get(key)
            .and_then(|s| s.get("type"))
            .and_then(Value::as_str)
            .unwrap_or("");
        let fits = match kind {
            "string" => value.is_string(),
            "integer" => value.is_i64() || value.is_u64(),
            "number" => value.is_number(),
            "boolean" => value.is_boolean(),
            "array" => value.is_array(),
            "object" => value.is_object(),
            _ => false,
        };
        if !fits {
            return Err(format!("paramètre « {key} » : attendu {kind}"));
        }
    }
    Ok(())
}

fn canonical_steps(steps: &[RecipeStep]) -> Vec<(String, Value)> {
    steps
        .iter()
        .map(|s| {
            (
                s.tool.clone(),
                if s.args.is_null() {
                    Value::Object(serde_json::Map::new())
                } else {
                    s.args.clone()
                },
            )
        })
        .collect()
}

// ----- texte : jetons et similarité ------------------------------------------------

const STOP: [&str; 24] = [
    "le", "la", "les", "un", "une", "des", "du", "de", "et", "ou", "au", "aux", "en", "pour",
    "the", "an", "of", "to", "for", "and", "or", "in", "on", "with",
];

fn fold(c: char) -> char {
    match c {
        'à' | 'á' | 'â' | 'ä' => 'a',
        'ç' => 'c',
        'è' | 'é' | 'ê' | 'ë' => 'e',
        'ì' | 'í' | 'î' | 'ï' => 'i',
        'ñ' => 'n',
        'ò' | 'ó' | 'ô' | 'ö' | 'œ' => 'o',
        'ù' | 'ú' | 'û' | 'ü' => 'u',
        other => other,
    }
}

/// Jetons d'un texte : minuscules, sans accents, mots vides retirés, pluriel simple en `s`
/// retiré. Volontairement rudimentaire : il doit être prévisible, pas linguistique.
pub(crate) fn tokens(text: &str) -> Vec<String> {
    let flat: String = text.to_lowercase().chars().map(fold).collect();
    flat.split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|t| t.len() >= 2 && !STOP.contains(t))
        .map(|t| {
            if t.len() > 4 && t.ends_with('s') {
                t[..t.len() - 1].to_string()
            } else {
                t.to_string()
            }
        })
        .collect()
}

/// Vecteur pondéré d'un outil : nom ×3, étiquettes ×2, description ×1.
fn doc_vector(name: &str, tags: &[String], description: &str) -> BTreeMap<String, f64> {
    let mut v: BTreeMap<String, f64> = BTreeMap::new();
    for t in tokens(name) {
        *v.entry(t).or_insert(0.0) += 3.0;
    }
    for tag in tags {
        for t in tokens(tag) {
            *v.entry(t).or_insert(0.0) += 2.0;
        }
    }
    for t in tokens(description) {
        *v.entry(t).or_insert(0.0) += 1.0;
    }
    v
}

fn cosine(a: &BTreeMap<String, f64>, b: &BTreeMap<String, f64>) -> f64 {
    let dot: f64 = a
        .iter()
        .filter_map(|(t, wa)| b.get(t).map(|wb| wa * wb))
        .sum();
    let norm = |m: &BTreeMap<String, f64>| m.values().map(|w| w * w).sum::<f64>().sqrt();
    let (na, nb) = (norm(a), norm(b));
    if na == 0.0 || nb == 0.0 {
        0.0
    } else {
        dot / (na * nb)
    }
}

/// Tronque à `max` caractères, avec « … » quand on coupe.
pub(crate) fn clip(text: &str, max: usize) -> String {
    match text.char_indices().nth(max) {
        None => text.to_string(),
        Some((byte, _)) => format!("{}…", &text[..byte]),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn any_tool(_: &str) -> Result<(), String> {
        Ok(())
    }

    fn schema(props: &[&str]) -> Value {
        let properties: serde_json::Map<String, Value> = props
            .iter()
            .map(|p| ((*p).to_string(), json!({"type": "string"})))
            .collect();
        json!({"type": "object", "properties": properties, "required": props})
    }

    fn draft(name: &str, description: &str, tool: &str, arg: &str) -> ToolDraft {
        ToolDraft {
            name: name.to_string(),
            description: description.to_string(),
            parameters: schema(&["x"]),
            steps: vec![RecipeStep {
                tool: tool.to_string(),
                args: json!({ arg: "{{x}}" }),
            }],
            tags: vec![],
            confirm_distinct: false,
        }
    }

    #[test]
    fn creer_un_nom_pris_est_refuse_et_designe_l_edition() {
        let mut c = Catalog::new();
        c.create(
            draft(
                "ouvrir_taches",
                "Liste les tâches ouvertes",
                "t/list",
                "plan",
            ),
            &any_tool,
        )
        .unwrap();
        let err = c
            .create(
                draft("ouvrir_taches", "autre chose", "t/other", "id"),
                &any_tool,
            )
            .unwrap_err();
        assert_eq!(err.code(), "name_taken");
        assert!(err.to_string().contains("action=edit"), "{err}");
    }

    #[test]
    fn une_recette_identique_est_refusee_sous_un_autre_nom_meme_confirmee() {
        let mut c = Catalog::new();
        c.create(
            draft("alpha_un", "Premier outil de listing", "t/list", "plan"),
            &any_tool,
        )
        .unwrap();
        let mut d = draft("beta_deux", "Tout autre vocabulaire ici", "t/list", "plan");
        assert_eq!(
            c.create(d.clone(), &any_tool).unwrap_err().code(),
            "duplicate"
        );
        d.confirm_distinct = true;
        assert_eq!(
            c.create(d, &any_tool).unwrap_err().code(),
            "duplicate",
            "la confirmation ne lève pas une recette identique"
        );
    }

    #[test]
    fn un_outil_proche_est_refuse_avec_ses_proches_et_la_confirmation_le_leve() {
        let mut c = Catalog::new();
        c.create(
            draft(
                "list_open_tasks",
                "Liste les tâches ouvertes du plan",
                "t/list",
                "plan",
            ),
            &any_tool,
        )
        .unwrap();
        let near = draft(
            "show_open_tasks",
            "Affiche les tâches ouvertes du plan",
            "t/show",
            "plan",
        );
        let err = c.create(near.clone(), &any_tool).unwrap_err();
        let CatalogError::Similar { candidates } = &err else {
            panic!("attendu Similar, reçu {err:?}")
        };
        assert_eq!(candidates[0].0, "list_open_tasks");
        assert!(candidates[0].1 >= 60, "{candidates:?}");
        let mut confirmed = near;
        confirmed.confirm_distinct = true;
        assert_eq!(c.create(confirmed, &any_tool), Ok(1));
    }

    #[test]
    fn un_outil_sans_rapport_n_est_pas_proche() {
        let mut c = Catalog::new();
        c.create(
            draft(
                "list_open_tasks",
                "Liste les tâches ouvertes du plan",
                "t/list",
                "plan",
            ),
            &any_tool,
        )
        .unwrap();
        assert_eq!(
            c.create(
                draft(
                    "rotate_backup_keys",
                    "Fait tourner les clés de sauvegarde",
                    "k/rotate",
                    "kid"
                ),
                &any_tool
            ),
            Ok(1)
        );
    }

    #[test]
    fn editer_exige_la_version_courante_et_garde_tout_l_historique() {
        let mut c = Catalog::new();
        c.create(draft("alpha_un", "Premier outil", "t/a", "k"), &any_tool)
            .unwrap();
        let change = ToolChange {
            description: Some("Premier outil, corrigé".to_string()),
            note: "précision".to_string(),
            ..ToolChange::default()
        };
        assert_eq!(c.edit("alpha_un", 1, change.clone(), &any_tool), Ok(2));
        let err = c.edit("alpha_un", 1, change, &any_tool).unwrap_err();
        assert_eq!(
            err,
            CatalogError::Stale {
                name: "alpha_un".into(),
                expected: 1,
                current: 2
            }
        );
        let record = c.get("alpha_un").unwrap();
        assert_eq!(record.versions.len(), 2);
        assert_eq!(record.version(1).unwrap().description, "Premier outil");
        assert_eq!(record.current().note, "précision");
    }

    #[test]
    fn editer_sans_rien_changer_est_refuse() {
        let mut c = Catalog::new();
        c.create(draft("alpha_un", "Premier outil", "t/a", "k"), &any_tool)
            .unwrap();
        let same = ToolChange {
            description: Some("Premier outil".to_string()),
            ..ToolChange::default()
        };
        assert_eq!(
            c.edit("alpha_un", 1, same, &any_tool).unwrap_err().code(),
            "nothing_changed"
        );
    }

    #[test]
    fn le_retour_arriere_copie_l_ancienne_version_dans_une_nouvelle() {
        let mut c = Catalog::new();
        c.create(draft("alpha_un", "Version un", "t/a", "k"), &any_tool)
            .unwrap();
        c.edit(
            "alpha_un",
            1,
            ToolChange {
                description: Some("Version deux".into()),
                ..ToolChange::default()
            },
            &any_tool,
        )
        .unwrap();
        assert_eq!(c.rollback("alpha_un", 1, 2, &any_tool), Ok(3));
        let r = c.get("alpha_un").unwrap();
        assert_eq!(r.versions.len(), 3, "rien n'est réécrit");
        assert_eq!(r.current().description, "Version un");
        assert_eq!(r.current().note, "retour à la v1");
        assert_eq!(r.version(2).unwrap().description, "Version deux");
    }

    #[test]
    fn un_outil_retire_n_est_plus_cherche_et_le_retour_arriere_le_reactive() {
        let mut c = Catalog::new();
        c.create(
            draft("alpha_un", "Liste les factures", "t/a", "k"),
            &any_tool,
        )
        .unwrap();
        assert_eq!(c.search("factures", 5).len(), 1);
        c.retire("alpha_un", "obsolète", None).unwrap();
        assert!(c.search("factures", 5).is_empty());
        assert_eq!(
            c.edit("alpha_un", 1, ToolChange::default(), &any_tool)
                .unwrap_err()
                .code(),
            "retired"
        );
        assert_eq!(c.rollback("alpha_un", 1, 1, &any_tool), Ok(2));
        assert_eq!(c.search("factures", 5).len(), 1);
    }

    #[test]
    fn le_plafond_de_versions_ne_sort_jamais_la_courante() {
        let mut c = Catalog::new();
        c.create(draft("alpha_un", "d0", "t/a", "k"), &any_tool)
            .unwrap();
        for i in 1..=(MAX_VERSIONS as u32 + 5) {
            c.edit(
                "alpha_un",
                i,
                ToolChange {
                    description: Some(format!("d{i}")),
                    ..ToolChange::default()
                },
                &any_tool,
            )
            .unwrap();
        }
        let r = c.get("alpha_un").unwrap();
        assert_eq!(r.versions.len(), MAX_VERSIONS);
        assert_eq!(r.dropped, 6);
        assert_eq!(r.current().version, MAX_VERSIONS as u32 + 6);
    }

    #[test]
    fn la_recherche_trouve_sans_accent_ni_pluriel_et_classe_par_pertinence() {
        let mut c = Catalog::new();
        c.create(
            draft(
                "list_open_tasks",
                "Liste les tâches ouvertes",
                "t/list",
                "plan",
            ),
            &any_tool,
        )
        .unwrap();
        c.create(
            draft("rotate_keys", "Fait tourner les clés", "k/rot", "kid"),
            &any_tool,
        )
        .unwrap();
        let hits = c.search("taches ouvertes", 5);
        assert_eq!(hits[0].name, "list_open_tasks");
        assert!(hits.iter().all(|h| h.name != "rotate_keys"), "{hits:?}");
        assert!(c.search("zzzz", 5).is_empty());
        assert!(c.search("", 5).is_empty());
    }

    #[test]
    fn a_pertinence_egale_l_outil_fiable_passe_devant_l_outil_fragile() {
        let mut c = Catalog::new();
        let mut a = draft("rapport_alpha", "Génère le rapport mensuel", "t/a", "k");
        a.confirm_distinct = true;
        let mut b = draft("rapport_bravo", "Génère le rapport mensuel", "t/b", "k");
        b.confirm_distinct = true;
        c.create(a, &any_tool).unwrap();
        c.create(b, &any_tool).unwrap();
        // bravo réussit quatre fois PUIS alpha échoue quatre fois : alpha est le plus RÉCENT.
        // Le repli à égalité est la récence ; si la fiabilité n'entrait pas dans le classement,
        // alpha passerait devant. Seule la fiabilité peut donc expliquer l'ordre attendu.
        for _ in 0..4 {
            c.record_use("rapport_bravo", 1, Usage::Ok);
        }
        for _ in 0..4 {
            c.record_use("rapport_alpha", 1, Usage::Failed);
        }
        let hits = c.search("rapport mensuel", 5);
        assert_eq!(hits[0].name, "rapport_bravo", "{hits:?}");
        assert!(hits[1].fragile && !hits[0].fragile, "{hits:?}");
        assert!(
            (hits[0].score - hits[1].score).abs() < 1e-9,
            "même pertinence"
        );
    }

    #[test]
    fn le_renforcement_se_compte_par_version() {
        let mut c = Catalog::new();
        c.create(draft("alpha_un", "d0", "t/a", "k"), &any_tool)
            .unwrap();
        c.record_use("alpha_un", 1, Usage::Ok);
        c.record_use("alpha_un", 1, Usage::Refused);
        c.edit(
            "alpha_un",
            1,
            ToolChange {
                description: Some("d1".into()),
                ..ToolChange::default()
            },
            &any_tool,
        )
        .unwrap();
        let r = c.get("alpha_un").unwrap();
        assert_eq!(
            (
                r.version(1).unwrap().stats.uses,
                r.version(1).unwrap().stats.refused
            ),
            (2, 1)
        );
        assert_eq!(
            r.current().stats,
            ToolStats::default(),
            "une version neuve repart de zéro"
        );
        // Une version ou un outil inconnus ne comptent pour personne.
        c.record_use("alpha_un", 99, Usage::Ok);
        c.record_use("nulle_part", 1, Usage::Ok);
        assert_eq!(c.get("alpha_un").unwrap().version(1).unwrap().stats.uses, 2);
    }

    #[test]
    fn une_recette_vide_trop_longue_ou_orpheline_est_refusee() {
        let mut c = Catalog::new();
        let mut d = draft("alpha_un", "d", "t/a", "k");
        d.steps.clear();
        assert_eq!(c.create(d, &any_tool).unwrap_err().code(), "bad_steps");
        let mut d = draft("alpha_un", "d", "t/a", "k");
        d.steps = (0..=MAX_STEPS)
            .map(|i| RecipeStep {
                tool: format!("t/{i}"),
                args: json!({"k": "{{x}}"}),
            })
            .collect();
        assert_eq!(c.create(d, &any_tool).unwrap_err().code(), "bad_steps");
        let mut d = draft("alpha_un", "d", "t/a", "k");
        d.steps[0].args = json!({"k": "{{absent}}"});
        let err = c.create(d, &any_tool).unwrap_err();
        assert!(err.to_string().contains("absent"), "{err}");
        let mut d = draft("alpha_un", "d", "t/a", "k");
        d.steps[0].args = json!({"k": "{{x"});
        assert_eq!(c.create(d, &any_tool).unwrap_err().code(), "bad_steps");
    }

    #[test]
    fn le_controle_de_l_hote_sur_les_etapes_est_applique_a_la_creation_et_a_l_edition() {
        let mut c = Catalog::new();
        let only_t = |t: &str| {
            if t.starts_with("t/") {
                Ok(())
            } else {
                Err("non offert par l'hôte".to_string())
            }
        };
        assert_eq!(
            c.create(draft("alpha_un", "d", "shell/run", "k"), &only_t)
                .unwrap_err()
                .code(),
            "bad_steps"
        );
        c.create(draft("alpha_un", "d", "t/a", "k"), &only_t)
            .unwrap();
        let change = ToolChange {
            steps: Some(vec![RecipeStep {
                tool: "shell/run".into(),
                args: json!({"k": "{{x}}"}),
            }]),
            ..ToolChange::default()
        };
        assert_eq!(
            c.edit("alpha_un", 1, change, &only_t).unwrap_err().code(),
            "bad_steps"
        );
    }

    #[test]
    fn les_noms_invalides_sont_refuses() {
        let mut c = Catalog::new();
        for bad in [
            "",
            "ab",
            "Alpha",
            "1alpha",
            "al pha",
            "alpha-un",
            &"a".repeat(41),
        ] {
            assert_eq!(
                c.create(draft(bad, "d", "t/a", "k"), &any_tool)
                    .unwrap_err()
                    .code(),
                "bad_name",
                "{bad:?}"
            );
        }
    }

    #[test]
    fn les_parametres_mal_formes_sont_refuses() {
        let mut c = Catalog::new();
        for schema in [
            json!("x"),
            json!({"type": "array"}),
            json!({"type": "object"}),
            json!({"type": "object", "properties": {"x": {}}}),
            json!({"type": "object", "properties": {"x": {"type": "string"}}, "required": ["y"]}),
            json!({"type": "object", "properties": {"X": {"type": "string"}}}),
        ] {
            let mut d = draft("alpha_un", "d", "t/a", "k");
            d.parameters = schema.clone();
            d.steps[0].args = json!({});
            assert_eq!(
                c.create(d, &any_tool).unwrap_err().code(),
                "bad_parameters",
                "{schema}"
            );
        }
    }

    #[test]
    fn le_catalogue_plein_refuse_une_creation() {
        let mut c = Catalog::new();
        for i in 0..MAX_TOOLS {
            let mut d = draft(
                &format!("outil_{i:03}"),
                &format!("zz{i} qq{i}"),
                &format!("t/{i}"),
                "k",
            );
            d.confirm_distinct = true;
            c.create(d, &any_tool).unwrap();
        }
        let mut d = draft("un_de_trop", "ww vv", "t/extra", "k");
        d.confirm_distinct = true;
        assert_eq!(c.create(d, &any_tool).unwrap_err(), CatalogError::Full);
    }

    #[test]
    fn la_persistance_fait_l_aller_retour_et_refuse_un_format_inconnu() {
        let mut c = Catalog::new();
        c.create(draft("alpha_un", "d", "t/a", "k"), &any_tool)
            .unwrap();
        c.record_use("alpha_un", 1, Usage::Ok);
        let back = Catalog::from_json(&c.to_json()).unwrap();
        assert_eq!(back, c);
        let mut v: Value = serde_json::from_str(&c.to_json()).unwrap();
        v["format"] = json!(FORMAT + 1);
        let err = Catalog::from_json(&v.to_string()).unwrap_err();
        assert!(err.contains("inconnu"), "{err}");
        assert!(
            Catalog::from_json(r#"{"tools":[]}"#)
                .unwrap_err()
                .contains("format")
        );
        assert!(Catalog::from_json("pas du json").is_err());
    }

    #[test]
    fn une_relecture_refuse_un_enregistrement_incoherent() {
        let mut c = Catalog::new();
        c.create(draft("alpha_un", "d", "t/a", "k"), &any_tool)
            .unwrap();
        let mut v: Value = serde_json::from_str(&c.to_json()).unwrap();
        v["tools"][0]["versions"] = json!([]);
        assert!(
            Catalog::from_json(&v.to_string())
                .unwrap_err()
                .contains("aucune version")
        );
        let mut v: Value = serde_json::from_str(&c.to_json()).unwrap();
        v["tools"][0]["name"] = json!("Mauvais Nom");
        assert!(Catalog::from_json(&v.to_string()).is_err());
    }

    // ----- paramètres -------------------------------------------------------------

    #[test]
    fn la_substitution_garde_le_type_d_une_valeur_entiere_et_intercale_ailleurs() {
        let call = json!({"n": 7, "who": "Ada", "tags": ["a", "b"]});
        let out = render_args(
            &json!({"count": "{{n}}", "greeting": "Bonjour {{who}} ({{n}})", "list": "{{tags}}", "fixe": 3}),
            &call,
        )
        .unwrap();
        assert_eq!(
            out,
            json!({"count": 7, "greeting": "Bonjour Ada (7)", "list": ["a", "b"], "fixe": 3})
        );
    }

    #[test]
    fn un_parametre_optionnel_absent_retire_la_cle_au_lieu_d_envoyer_null() {
        let out = render_args(
            &json!({"q": "{{q}}", "limit": "{{limit}}"}),
            &json!({"q": "x"}),
        )
        .unwrap();
        assert_eq!(out, json!({"q": "x"}));
        let out = render_args(
            &json!({"q": "{{q}}", "limit": "{{limit}}"}),
            &json!({"q": "x", "limit": null}),
        )
        .unwrap();
        assert_eq!(
            out,
            json!({"q": "x"}),
            "un null explicite est traité comme absent"
        );
    }

    #[test]
    fn la_substitution_ne_reevalue_pas_une_valeur_qui_contient_des_accolades() {
        // Une valeur d'appel qui ressemble à un paramètre reste du TEXTE : pas d'injection
        // de second niveau.
        let out = render_args(
            &json!({"a": "{{v}}", "b": "pre {{v}}"}),
            &json!({"v": "{{w}}", "w": "secret"}),
        )
        .unwrap();
        assert_eq!(out, json!({"a": "{{w}}", "b": "pre {{w}}"}));
    }

    #[test]
    fn des_arguments_trop_gros_apres_substitution_sont_refuses() {
        let big = "x".repeat(MAX_STEP_ARGS_BYTES);
        assert!(render_args(&json!({"a": "{{v}}"}), &json!({"v": big})).is_err());
    }

    #[test]
    fn la_validation_d_appel_controle_requis_inconnus_et_types() {
        let schema = json!({
            "type": "object",
            "properties": {"q": {"type": "string"}, "n": {"type": "integer"}, "f": {"type": "boolean"}},
            "required": ["q"]
        });
        assert_eq!(
            validate_args(&schema, &json!({"q": "x", "n": 3, "f": true})),
            Ok(())
        );
        assert!(
            validate_args(&schema, &json!({}))
                .unwrap_err()
                .contains("manquant")
        );
        assert!(
            validate_args(&schema, &json!({"q": null}))
                .unwrap_err()
                .contains("manquant")
        );
        assert!(
            validate_args(&schema, &json!({"q": "x", "z": 1}))
                .unwrap_err()
                .contains("inconnu")
        );
        assert!(
            validate_args(&schema, &json!({"q": 5}))
                .unwrap_err()
                .contains("attendu string")
        );
        assert!(
            validate_args(&schema, &json!({"q": "x", "n": 1.5}))
                .unwrap_err()
                .contains("integer")
        );
        assert!(validate_args(&schema, &json!("texte")).is_err());
    }

    #[test]
    fn les_jetons_retirent_accents_mots_vides_et_pluriel() {
        assert_eq!(
            tokens("Liste les tâches OUVERTES du plan"),
            ["liste", "tache", "ouverte", "plan"]
        );
        assert!(tokens("le la de").is_empty());
    }
}
