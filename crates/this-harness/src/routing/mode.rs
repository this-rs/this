//! Les modes de routage, l'étape d'apprentissage et les réglages de la note.
//!
//! Deux vocabulaires, à ne pas confondre (PO, décision R-S4) :
//! - [`ProviderRoutingMode`] : ce que disent les **réglages** (`primary`,
//!   `executors_only`, `full`) ;
//! - [`ConversationRoutingMode`] : ce qu'UNE conversation a demandé dans son
//!   menu (`primary` / `mixed` / `full` — à l'écran : strict / mixed / auto).
//!
//! Portage de PO : `src/chat/provider/cognitive/mode.rs` et
//! `src/chat/types.rs` (`RoutingPoolEntry`, `pool_spans_providers`).

use serde::{Deserialize, Serialize};

/// Taux d'exploration par défaut.
pub const DEFAULT_EXPLORATION_EPSILON: f64 = 0.05;
/// Plafond dur du taux d'exploration.
pub const MAX_EXPLORATION_EPSILON: f64 = 0.25;
/// Poids par défaut du coût normalisé dans l'utilité.
pub const DEFAULT_COST_WEIGHT: f64 = 0.3;
/// Poids par défaut de la latence normalisée.
pub const DEFAULT_LATENCY_WEIGHT: f64 = 0.1;
/// Poids par défaut de la pression de quota.
pub const DEFAULT_QUOTA_WEIGHT: f64 = 0.2;
/// Au-delà de ce remplissage, un compte est « près de sa limite » (PO `quota.rs`).
pub const QUOTA_NEAR_LIMIT: f64 = 0.9;

/// Qui choisit le fournisseur et le modèle, d'après les RÉGLAGES.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProviderRoutingMode {
    /// Un fournisseur principal exclusif. Le routeur note seulement ce qu'il aurait choisi.
    #[default]
    Primary,
    /// Le principal pilote la conversation ; seuls les exécuteurs sont routés.
    /// Un document d'avant R-S4 dit `mixed` : lu comme cette variante.
    #[serde(rename = "executors_only", alias = "mixed")]
    ExecutorsOnly,
    /// Tout est routé, le pilote compris.
    Full,
}

impl ProviderRoutingMode {
    pub const ALL: [Self; 3] = [Self::Primary, Self::ExecutorsOnly, Self::Full];

    /// Nom sérialisé stable.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Primary => "primary",
            Self::ExecutorsOnly => "executors_only",
            Self::Full => "full",
        }
    }
}

/// Le routage qu'UNE conversation a demandé dans son menu : Auto = `full`, deux
/// modèles cochés ou plus = `mixed`, un seul = `primary` (strict).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ConversationRoutingMode {
    /// Strict : le modèle coché est imposé, jamais remplacé.
    Primary,
    /// Le routeur choisit parmi les modèles cochés, le pilote compris.
    Mixed,
    /// Auto : le routeur choisit tout, le fournisseur compris.
    Full,
}

impl ConversationRoutingMode {
    pub const ALL: [Self; 3] = [Self::Primary, Self::Mixed, Self::Full];

    /// Nom sérialisé stable (celui du code).
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Primary => "primary",
            Self::Mixed => "mixed",
            Self::Full => "full",
        }
    }

    /// Lit le mot du code (`primary` / `mixed` / `full`) ou celui de l'écran
    /// (`strict` / `mixed` / `auto`). Tout le reste est `None`.
    pub fn parse(text: &str) -> Option<Self> {
        Some(match text.trim().to_lowercase().as_str() {
            "primary" | "strict" => Self::Primary,
            "mixed" => Self::Mixed,
            "full" | "auto" => Self::Full,
            _ => return None,
        })
    }

    /// Le mode des réglages avec lequel les décisions du PILOTE de cette
    /// conversation sont demandées. `mixed` ne route le pilote que dans son
    /// pool (là, la décision est demandée en `full` restreinte au pool).
    pub fn pilot_mode(self) -> ProviderRoutingMode {
        match self {
            Self::Primary | Self::Mixed => ProviderRoutingMode::Primary,
            Self::Full => ProviderRoutingMode::Full,
        }
    }
}

/// Jusqu'où les choix appris sont crus.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LearningStage {
    /// Tout est calculé et noté, rien n'est appliqué.
    #[default]
    Shadow,
    /// Le choix appris est proposé, une personne confirme.
    Advisory,
    /// Le choix appris est appliqué.
    Auto,
}

impl LearningStage {
    pub const ALL: [Self; 3] = [Self::Shadow, Self::Advisory, Self::Auto];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Shadow => "shadow",
            Self::Advisory => "advisory",
            Self::Auto => "auto",
        }
    }
}

fn default_epsilon() -> f64 {
    DEFAULT_EXPLORATION_EPSILON
}
fn default_cost_weight() -> f64 {
    DEFAULT_COST_WEIGHT
}
fn default_latency_weight() -> f64 {
    DEFAULT_LATENCY_WEIGHT
}
fn default_quota_weight() -> f64 {
    DEFAULT_QUOTA_WEIGHT
}

/// Les réglages de routage d'une portée. Chaque champ a un défaut : un document
/// partiel ou ancien se lit quand même.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RoutingSettings {
    #[serde(default)]
    pub mode: ProviderRoutingMode,
    #[serde(default)]
    pub stage: LearningStage,
    /// Taux d'exploration, `0.0..=0.25`.
    #[serde(default = "default_epsilon")]
    pub exploration_epsilon: f64,
    /// Poids du coût normalisé dans l'utilité, `0.0..=1.0`.
    #[serde(default = "default_cost_weight")]
    pub cost_weight: f64,
    /// Poids de la latence normalisée, `0.0..=1.0`.
    #[serde(default = "default_latency_weight")]
    pub latency_weight: f64,
    /// Poids de la pression de quota, `0.0..=1.0`. 0 désactive la préférence.
    #[serde(default = "default_quota_weight")]
    pub quota_weight: f64,
}

impl RoutingSettings {
    /// Les mêmes réglages, ramenés dans leurs bornes : un poids ou un taux qui
    /// n'est pas un nombre fini (`NaN`, infini) reprend son défaut, les autres
    /// sont bornés. Les réglages viennent de l'hôte : le cœur ne note jamais
    /// avec un `NaN`.
    pub fn sanitized(&self) -> Self {
        let bounded = |v: f64, default: f64, max: f64| {
            if v.is_finite() {
                v.clamp(0.0, max)
            } else {
                default
            }
        };
        Self {
            mode: self.mode,
            stage: self.stage,
            exploration_epsilon: bounded(
                self.exploration_epsilon,
                DEFAULT_EXPLORATION_EPSILON,
                MAX_EXPLORATION_EPSILON,
            ),
            cost_weight: bounded(self.cost_weight, DEFAULT_COST_WEIGHT, 1.0),
            latency_weight: bounded(self.latency_weight, DEFAULT_LATENCY_WEIGHT, 1.0),
            quota_weight: bounded(self.quota_weight, DEFAULT_QUOTA_WEIGHT, 1.0),
        }
    }
}

impl Default for RoutingSettings {
    fn default() -> Self {
        Self {
            mode: ProviderRoutingMode::Primary,
            stage: LearningStage::Shadow,
            exploration_epsilon: DEFAULT_EXPLORATION_EPSILON,
            cost_weight: DEFAULT_COST_WEIGHT,
            latency_weight: DEFAULT_LATENCY_WEIGHT,
            quota_weight: DEFAULT_QUOTA_WEIGHT,
        }
    }
}

/// Une paire (fournisseur, modèle) qu'une conversation — ou la liste Auto d'une
/// personne — autorise : un modèle coché.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PoolEntry {
    /// Identifiant de l'instance.
    pub provider: String,
    /// Identifiant du modèle sur cette instance (jamais un alias).
    pub model: String,
}

impl PoolEntry {
    pub fn new(provider: impl Into<String>, model: impl Into<String>) -> Self {
        Self {
            provider: provider.into(),
            model: model.into(),
        }
    }
}

/// Un pool nomme-t-il des modèles sur deux fournisseurs ou plus ? Une telle
/// conversation peut changer de fournisseur d'un tour à l'autre, comme `full`
/// restreint au pool (PO, décision R-S2).
pub fn pool_spans_providers(pool: Option<&[PoolEntry]>) -> bool {
    let Some(pool) = pool else {
        return false;
    };
    let mut providers: Vec<&str> = pool
        .iter()
        .map(|e| e.provider.trim())
        .filter(|p| !p.is_empty())
        .collect();
    providers.sort_unstable();
    providers.dedup();
    providers.len() > 1
}

/// La personne a-t-elle confié le routage de CETTE conversation au routeur ?
/// Auto (`full`) ou deux modèles cochés ou plus. Un seul modèle coché est
/// strict, et une conversation qui n'a rien demandé suit les réglages.
pub fn conversation_routes(mode: Option<ConversationRoutingMode>, pool_len: usize) -> bool {
    mode == Some(ConversationRoutingMode::Full) || pool_len > 1
}

/// L'étape à laquelle les décisions d'une conversation sont prises : un choix
/// fait sur la conversation vaut `auto` pour elle, quoi que disent les réglages.
/// L'étape des réglages est un réglage d'APPRENTISSAGE : elle ne met jamais son
/// veto à un choix explicite de la personne.
pub fn conversation_stage(settings: LearningStage, conversation_routes: bool) -> LearningStage {
    if conversation_routes {
        LearningStage::Auto
    } else {
        settings
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// PO `mode.rs::a_choice_on_the_conversation_is_the_auto_stage_for_it_only`.
    #[test]
    fn un_choix_sur_la_conversation_vaut_l_etape_auto_pour_elle_seule() {
        use ConversationRoutingMode::*;
        assert!(conversation_routes(Some(Full), 0));
        assert!(conversation_routes(Some(Mixed), 2));
        assert!(conversation_routes(None, 2));
        assert!(!conversation_routes(Some(Primary), 1));
        assert!(!conversation_routes(None, 0));
        for stage in LearningStage::ALL {
            assert_eq!(conversation_stage(stage, true), LearningStage::Auto);
            assert_eq!(conversation_stage(stage, false), stage);
        }
    }

    /// PO `mode.rs::the_default_is_primary_and_shadow_with_the_documented_knobs`.
    #[test]
    fn le_defaut_est_primary_et_shadow_avec_les_poids_documentes() {
        let s = RoutingSettings::default();
        assert_eq!(s.mode, ProviderRoutingMode::Primary);
        assert_eq!(s.stage, LearningStage::Shadow);
        assert_eq!(s.exploration_epsilon, 0.05);
        assert_eq!(s.cost_weight, 0.3);
        assert_eq!(s.latency_weight, 0.1);
        assert_eq!(s.quota_weight, 0.2);
        // Un document vide se lit avec les défauts.
        assert_eq!(serde_json::from_str::<RoutingSettings>("{}").unwrap(), s);
    }

    /// PO `mode.rs::modes_and_stages_round_trip_through_serde_and_as_str`.
    #[test]
    fn les_modes_et_les_etapes_font_l_aller_retour() {
        for mode in ProviderRoutingMode::ALL {
            let json = serde_json::to_string(&mode).unwrap();
            assert_eq!(json, format!("\"{}\"", mode.as_str()));
            assert_eq!(
                serde_json::from_str::<ProviderRoutingMode>(&json).unwrap(),
                mode
            );
        }
        for mode in ConversationRoutingMode::ALL {
            let json = serde_json::to_string(&mode).unwrap();
            assert_eq!(json, format!("\"{}\"", mode.as_str()));
            assert_eq!(ConversationRoutingMode::parse(mode.as_str()), Some(mode));
        }
        for stage in LearningStage::ALL {
            let json = serde_json::to_string(&stage).unwrap();
            assert_eq!(json, format!("\"{}\"", stage.as_str()));
        }
    }

    /// PO `mode.rs::the_settings_and_the_conversation_name_their_modes_apart` et
    /// `a_settings_document_stored_with_mixed_still_reads_as_executors_only`.
    #[test]
    fn les_reglages_et_la_conversation_nomment_leurs_modes_a_part() {
        use ConversationRoutingMode as C;
        use ProviderRoutingMode as P;
        // `mixed` dans un document de réglages est l'ancien nom d'`executors_only`.
        assert_eq!(
            serde_json::from_str::<P>("\"mixed\"").unwrap(),
            P::ExecutorsOnly
        );
        assert_eq!(
            serde_json::to_string(&P::ExecutorsOnly).unwrap(),
            "\"executors_only\""
        );
        // Sur une conversation, `mixed` est son propre mot.
        assert_eq!(serde_json::from_str::<C>("\"mixed\"").unwrap(), C::Mixed);
        assert!(serde_json::from_str::<C>("\"executors_only\"").is_err());
        // Le pilote d'un `mixed` n'est jamais `executors_only`.
        assert_eq!(C::Primary.pilot_mode(), P::Primary);
        assert_eq!(C::Mixed.pilot_mode(), P::Primary);
        assert_eq!(C::Full.pilot_mode(), P::Full);
    }

    /// Les mots de l'écran (auto / mixed / strict) se lisent comme ceux du code.
    /// Ajout de this-rs : PO fait cette traduction dans son frontal.
    #[test]
    fn les_mots_de_l_ecran_se_lisent_comme_ceux_du_code() {
        use ConversationRoutingMode::*;
        assert_eq!(ConversationRoutingMode::parse("auto"), Some(Full));
        assert_eq!(ConversationRoutingMode::parse(" Strict "), Some(Primary));
        assert_eq!(ConversationRoutingMode::parse("mixed"), Some(Mixed));
        assert_eq!(ConversationRoutingMode::parse("executors_only"), None);
    }

    /// PO `types.rs` (module de tests de `pool_spans_providers`).
    #[test]
    fn un_pool_couvre_plusieurs_fournisseurs_ou_non() {
        let e = PoolEntry::new;
        assert!(!pool_spans_providers(None));
        assert!(!pool_spans_providers(Some(&[])));
        assert!(!pool_spans_providers(Some(&[e("a", "m1"), e("a", "m2")])));
        assert!(pool_spans_providers(Some(&[e("a", "m1"), e("b", "m2")])));
        // Un fournisseur vide n'est pas un fournisseur.
        assert!(!pool_spans_providers(Some(&[e("a", "m1"), e(" ", "m2")])));
    }
}
