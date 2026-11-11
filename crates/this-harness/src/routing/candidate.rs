//! Les candidats : les paires (fournisseur, modèle) vers lesquelles un tour
//! peut être routé.
//!
//! Contraintes dures seulement ; la note n'est pas ici. Une paire qui échoue à
//! une contrainte est rejetée avec un [`RejectReason`] typé, pour qu'une
//! décision puisse dire pourquoi une alternative n'était pas éligible.
//!
//! Le filtre ne s'applique qu'aux emplacements automatiques. Un choix que
//! l'appelant a nommé n'est jamais filtré ni remplacé.
//!
//! Portage de PO : `src/chat/provider/cognitive/candidates.rs`.

use serde::{Deserialize, Serialize};

use super::signature::TurnSignature;

/// Jetons de sortie supposés pour l'estimation du coût d'un tour médian.
const TURN_OUTPUT_TOKENS: f64 = 2_000.0;

/// `auto` est une sentinelle (« laisse le harnais choisir »), **pas** un nom de
/// modèle : transmis tel quel à un moteur, il est rejeté. Insensible à la
/// casse, espaces ignorés. Une chaîne vide n'est pas un modèle non plus.
pub fn is_auto_model(model: &str) -> bool {
    let model = model.trim();
    model.is_empty() || model.eq_ignore_ascii_case("auto")
}

/// Une instance de fournisseur et l'un de ses modèles.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Pick {
    /// Identifiant de l'instance.
    pub provider_id: String,
    /// Identifiant du modèle.
    pub model: String,
}

impl Pick {
    pub fn new(provider_id: impl Into<String>, model: impl Into<String>) -> Self {
        Self {
            provider_id: provider_id.into(),
            model: model.into(),
        }
    }
}

/// Prix par million de jetons.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Price {
    pub input_per_mtok: f64,
    pub output_per_mtok: f64,
}

/// Comment le coût est compté.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CostBasis {
    /// Rapporté par le fournisseur.
    Reported,
    /// Calculé depuis l'usage et le prix.
    Priced,
    /// Gratuit (modèle local).
    Free,
    /// Couvert par un abonnement : aucun coût marginal.
    Subscription,
    /// Aucun prix connu : le montant est `None`, jamais zéro.
    #[default]
    Unknown,
}

/// Ce qu'un modèle sait faire.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Capabilities {
    /// Le modèle appelle des outils.
    pub tools: bool,
    /// Le modèle lit des images.
    pub images: bool,
    /// Le modèle raisonne (réflexion étendue). Informatif dans ce lot : aucun
    /// filtre ne le lit (PO n'en a pas non plus).
    pub reasoning: bool,
    /// Fenêtre de contexte en jetons ; `None` quand elle est inconnue (jamais supposée).
    pub context_window: Option<u64>,
}

/// Pourquoi la fenêtre d'un modèle est inconnue.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UnknownWindow {
    /// Le catalogue vivant n'a pas été joint.
    CatalogOffline,
    /// Le catalogue vivant ne liste pas ce modèle, ou sans fenêtre.
    NotInCatalog,
}

impl UnknownWindow {
    /// Code stable, identique à la forme sérialisée.
    pub fn code(&self) -> &'static str {
        match self {
            Self::CatalogOffline => "catalog_offline",
            Self::NotInCatalog => "not_in_catalog",
        }
    }
}

/// Jusqu'à quand un compte refuse.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuotaExhaustion {
    /// L'instant où il sert de nouveau (ms depuis l'époque).
    pub resets_at_ms: u64,
    /// La fenêtre pleine, quand elle est nommée (`seven_day`) ; `model:<nom>`
    /// quand la retenue ne vise qu'un modèle.
    pub window: Option<String>,
}

/// Ce qui est su d'un modèle d'une instance, au moment de décider.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Candidate {
    /// Identifiant de l'instance.
    pub provider_id: String,
    /// Identifiant du modèle.
    pub model: String,
    /// Ce que le modèle sait faire.
    pub capabilities: Capabilities,
    /// Pourquoi la fenêtre est inconnue, quand sa source sait le dire.
    pub window_unknown: Option<UnknownWindow>,
    /// Prix par million de jetons ; `None` quand il est inconnu.
    pub price: Option<Price>,
    /// Comment le coût est compté.
    pub cost_basis: CostBasis,
    /// Santé vivante : `Some(false)` est en panne, `None` n'a pas été mesuré.
    pub healthy: Option<bool>,
    /// L'hôte autorise cette paire (consentement du projet, moteur disponible).
    pub allowed: bool,
    /// Une machine distante : choisie par une personne, jamais par le routeur.
    pub remote: bool,
    /// Le compte refuse jusqu'à cet instant. Pas de la santé : un compte épuisé
    /// répond, et dit non.
    pub quota_exhausted: Option<QuotaExhaustion>,
    /// À quel point la fenêtre la plus pleine de l'instance l'est (`0.0..=1.0`).
    /// Préférence douce du scoreur : `None` est « personne n'a mesuré », jamais « vide ».
    pub quota_utilization: Option<f64>,
}

impl Candidate {
    /// Une paire saine, autorisée, sans prix ni quota connus.
    pub fn new(
        provider_id: impl Into<String>,
        model: impl Into<String>,
        capabilities: Capabilities,
    ) -> Self {
        Self {
            provider_id: provider_id.into(),
            model: model.into(),
            capabilities,
            window_unknown: None,
            price: None,
            cost_basis: CostBasis::Unknown,
            healthy: None,
            allowed: true,
            remote: false,
            quota_exhausted: None,
            quota_utilization: None,
        }
    }

    /// La paire.
    pub fn pick(&self) -> Pick {
        Pick::new(&self.provider_id, &self.model)
    }

    /// USD estimés d'un tour médian pour `signature` ; `None` quand le prix est
    /// inconnu (jamais zéro) ou que le coût n'est pas de l'argent marginal.
    pub fn estimated_turn_cost_usd(&self, signature: &TurnSignature) -> Option<f64> {
        if matches!(self.cost_basis, CostBasis::Free | CostBasis::Subscription) {
            return None;
        }
        let price = self.price?;
        Some(
            (signature.context_need_tokens as f64 * price.input_per_mtok
                + TURN_OUTPUT_TOKENS * price.output_per_mtok)
                / 1_000_000.0,
        )
    }
}

/// Pourquoi une paire n'est pas éligible.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "reason", rename_all = "snake_case")]
pub enum RejectReason {
    /// `auto` (ou vide) n'est pas un nom de modèle : jamais un candidat.
    /// Ajout de this-rs : PO traite la sentinelle à l'ouverture, pas dans le filtre.
    Sentinel,
    /// Le prix donné par l'hôte n'est pas un nombre utilisable (`NaN`, infini,
    /// négatif) : la paire est écartée plutôt que notée sur une valeur qui
    /// empoisonnerait le classement. Ajout de this-rs.
    InvalidPrice,
    /// Le bras stocké de cette paire n'est pas un postérieur (`alpha` ou `beta`
    /// nul, négatif, infini ou `NaN`, coût non fini) : elle est écartée, le
    /// magasin de l'hôte est à réparer. Posé par la décision, pas par le filtre.
    /// Ajout de this-rs.
    InvalidArm,
    /// Une machine est choisie par une personne, jamais par le routeur.
    Remote,
    /// L'hôte n'autorise pas cette instance.
    NotAllowed,
    /// Le compte refuse jusqu'à `resets_at_ms` : pas une panne, l'instance
    /// revient d'elle-même à cet instant.
    QuotaExhausted {
        resets_at_ms: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        window: Option<String>,
    },
    /// UN modèle de l'instance refuse jusqu'à `resets_at_ms` : les autres
    /// modèles de l'instance restent éligibles.
    QuotaExhaustedModel { resets_at_ms: u64, model: String },
    /// L'instance est en panne maintenant.
    Unhealthy,
    /// Le tour a besoin d'outils et le modèle ne sait pas les appeler.
    NoTools,
    /// La fenêtre est plus petite que le besoin, ou inconnue.
    ContextTooSmall { need: u64, have: Option<u64> },
    /// La fenêtre est inconnue et sa source dit pourquoi. Jamais supposée.
    WindowUnknown { need: u64, why: UnknownWindow },
    /// L'entrée porte des images et le modèle ne les lit pas.
    NoImages,
    /// Un tour médian coûte plus que le budget marginal restant.
    OverBudget,
}

impl RejectReason {
    /// Code stable, pour les journaux et l'interface.
    pub fn code(&self) -> &'static str {
        match self {
            Self::Sentinel => "sentinel",
            Self::InvalidPrice => "invalid_price",
            Self::InvalidArm => "invalid_arm",
            Self::Remote => "remote",
            Self::NotAllowed => "not_allowed",
            Self::QuotaExhausted { .. } => "quota_exhausted",
            Self::QuotaExhaustedModel { .. } => "quota_exhausted_model",
            Self::Unhealthy => "unhealthy",
            Self::NoTools => "no_tools",
            Self::ContextTooSmall { .. } => "context_too_small",
            Self::WindowUnknown { .. } => "window_unknown",
            Self::NoImages => "no_images",
            Self::OverBudget => "over_budget",
        }
    }
}

/// Une paire qui n'était pas éligible, et pourquoi.
#[derive(Debug, Clone, PartialEq)]
pub struct Rejected {
    pub candidate: Candidate,
    /// La première contrainte qu'elle a manquée.
    pub reason: RejectReason,
}

/// Qui a nommé la paire qu'un emplacement tiendrait.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Slot {
    /// La demande l'a nommée : jamais filtrée, jamais remplacée.
    Explicit,
    /// Une règle, le défaut ou le routeur l'a choisie : soumise aux contraintes.
    Automatic,
}

/// Le résultat du filtre.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Filtered {
    /// Paires éligibles, dans l'ordre d'entrée.
    pub eligible: Vec<Candidate>,
    /// Paires rejetées, avec leur première contrainte manquée.
    pub rejected: Vec<Rejected>,
}

/// La première contrainte manquée, s'il y en a une. Ordre : à qui appartient le
/// choix (distant, consentement, quota, santé) avant la capacité avant l'argent.
fn check(signature: &TurnSignature, facts: &Candidate) -> Option<RejectReason> {
    if is_auto_model(&facts.model) {
        return Some(RejectReason::Sentinel);
    }
    // Les faits viennent de l'hôte : un prix qui n'est pas un nombre ne doit ni
    // passer le filtre de budget par accident, ni donner une note `NaN`.
    if let Some(price) = facts.price {
        let usable = |v: f64| v.is_finite() && v >= 0.0;
        if !usable(price.input_per_mtok) || !usable(price.output_per_mtok) {
            return Some(RejectReason::InvalidPrice);
        }
    }
    if facts.remote {
        return Some(RejectReason::Remote);
    }
    if !facts.allowed {
        return Some(RejectReason::NotAllowed);
    }
    // Avant la santé : un compte épuisé est sain, et le motif doit dire quand il revient.
    if let Some(quota) = &facts.quota_exhausted {
        if let Some(model) = quota
            .window
            .as_deref()
            .and_then(|w| w.strip_prefix("model:"))
        {
            return Some(RejectReason::QuotaExhaustedModel {
                resets_at_ms: quota.resets_at_ms,
                model: model.to_owned(),
            });
        }
        return Some(RejectReason::QuotaExhausted {
            resets_at_ms: quota.resets_at_ms,
            window: quota.window.clone(),
        });
    }
    if facts.healthy == Some(false) {
        return Some(RejectReason::Unhealthy);
    }
    if signature.needs_tools && !facts.capabilities.tools {
        return Some(RejectReason::NoTools);
    }
    match (facts.capabilities.context_window, facts.window_unknown) {
        (Some(window), _) if window >= signature.context_need_tokens => {}
        (None, Some(why)) => {
            return Some(RejectReason::WindowUnknown {
                need: signature.context_need_tokens,
                why,
            });
        }
        (have, _) => {
            return Some(RejectReason::ContextTooSmall {
                need: signature.context_need_tokens,
                have,
            });
        }
    }
    if signature.needs_images && !facts.capabilities.images {
        return Some(RejectReason::NoImages);
    }
    let cost = facts.estimated_turn_cost_usd(signature);
    if matches!((signature.budget_remaining_usd, cost), (Some(budget), Some(cost)) if cost > budget)
    {
        return Some(RejectReason::OverBudget);
    }
    None
}

/// Filtre `pool` pour `signature`. Un emplacement explicite est hors filtre : `None`.
pub fn filter(slot: Slot, signature: &TurnSignature, pool: &[Candidate]) -> Option<Filtered> {
    if slot == Slot::Explicit {
        return None;
    }
    let mut filtered = Filtered::default();
    for facts in pool {
        match check(signature, facts) {
            None => filtered.eligible.push(facts.clone()),
            Some(reason) => filtered.rejected.push(Rejected {
                candidate: facts.clone(),
                reason,
            }),
        }
    }
    Some(filtered)
}

/// Ce que l'hôte sait des quotas, **maintenant** (c'est lui qui a l'horloge).
///
/// Le cœur ne lit aucune horloge : une retenue dont l'échéance est passée n'est
/// simplement plus rendue par l'hôte, et la paire redevient éligible sans que
/// personne ne fasse rien.
pub trait QuotaView {
    /// Le compte entier de l'instance refuse.
    fn exhausted(&self, provider_id: &str) -> Option<QuotaExhaustion>;
    /// Un seul modèle de l'instance refuse. `window` doit valoir `model:<nom>`.
    fn model_exhausted(&self, provider_id: &str, model: &str) -> Option<QuotaExhaustion>;
    /// Remplissage de la fenêtre la plus pleine de l'instance (`0.0..=1.0`).
    fn pressure(&self, provider_id: &str) -> Option<f64>;
}

/// Marque chaque paire de `pool` dont l'instance refuse maintenant.
///
/// Un épuisement du compte condamne tous les modèles de l'instance et gagne le
/// motif (il peut revenir plus tard qu'une retenue de modèle : le modèle ne doit
/// pas se glisser de nouveau avant). Compte libre, une retenue par modèle ne
/// marque QUE ce modèle.
pub fn apply_quota(pool: &mut [Candidate], quota: &dyn QuotaView) {
    for facts in pool {
        facts.quota_exhausted = quota
            .exhausted(&facts.provider_id)
            .or_else(|| quota.model_exhausted(&facts.provider_id, &facts.model));
        facts.quota_utilization = quota.pressure(&facts.provider_id);
    }
}

/// Un registre de quotas en mémoire, avec une heure **posée à la main** : pour
/// les tests, et pour un hôte qui n'a rien de plus fin.
#[derive(Debug, Clone, Default)]
pub struct MemoryQuota {
    now_ms: u64,
    accounts: Vec<(String, QuotaExhaustion)>,
    models: Vec<(String, String, u64)>,
    pressure: Vec<(String, f64)>,
}

impl MemoryQuota {
    pub fn at(now_ms: u64) -> Self {
        Self {
            now_ms,
            ..Self::default()
        }
    }

    /// L'heure avance : c'est l'hôte qui le dit.
    pub fn set_now(&mut self, now_ms: u64) {
        self.now_ms = now_ms;
    }

    pub fn now_ms(&self) -> u64 {
        self.now_ms
    }

    /// Le compte de `provider_id` refuse jusqu'à `resets_at_ms`.
    pub fn hold_account(&mut self, provider_id: &str, resets_at_ms: u64, window: Option<&str>) {
        self.accounts.push((
            provider_id.to_owned(),
            QuotaExhaustion {
                resets_at_ms,
                window: window.map(str::to_owned),
            },
        ));
    }

    /// Le modèle `model` de `provider_id` refuse jusqu'à `resets_at_ms`.
    pub fn hold_model(&mut self, provider_id: &str, model: &str, resets_at_ms: u64) {
        self.models
            .push((provider_id.to_owned(), model.to_owned(), resets_at_ms));
    }

    /// Le remplissage mesuré de `provider_id`.
    pub fn set_pressure(&mut self, provider_id: &str, used: f64) {
        self.pressure.retain(|(p, _)| p != provider_id);
        self.pressure.push((provider_id.to_owned(), used));
    }
}

impl QuotaView for MemoryQuota {
    fn exhausted(&self, provider_id: &str) -> Option<QuotaExhaustion> {
        self.accounts
            .iter()
            .filter(|(p, held)| p == provider_id && held.resets_at_ms > self.now_ms)
            .map(|(_, held)| held.clone())
            .max_by_key(|held| held.resets_at_ms)
    }

    fn model_exhausted(&self, provider_id: &str, model: &str) -> Option<QuotaExhaustion> {
        self.models
            .iter()
            .filter(|(p, m, until)| p == provider_id && m == model && *until > self.now_ms)
            .map(|(_, m, until)| QuotaExhaustion {
                resets_at_ms: *until,
                window: Some(format!("model:{m}")),
            })
            .max_by_key(|held| held.resets_at_ms)
    }

    fn pressure(&self, provider_id: &str) -> Option<f64> {
        self.pressure
            .iter()
            .find(|(p, _)| p == provider_id)
            .map(|(_, used)| *used)
    }
}

#[cfg(test)]
mod tests {
    use super::super::signature::{ContextHints, TaskClass};
    use super::*;

    /// PO `candidates.rs::tests::good`.
    fn good() -> Candidate {
        Candidate {
            price: Some(Price {
                input_per_mtok: 1.0,
                output_per_mtok: 2.0,
            }),
            cost_basis: CostBasis::Priced,
            healthy: Some(true),
            ..Candidate::new(
                "deepseek",
                "v4",
                Capabilities {
                    tools: true,
                    images: true,
                    reasoning: false,
                    context_window: Some(128_000),
                },
            )
        }
    }

    fn signature() -> TurnSignature {
        TurnSignature::utility(TaskClass::UtilityFeatureGraph, 20_000)
    }

    fn tooled() -> TurnSignature {
        TurnSignature::from_chat("hello", false, ContextHints::default())
    }

    fn reason(signature: &TurnSignature, facts: Candidate) -> Option<RejectReason> {
        let out = filter(Slot::Automatic, signature, &[facts]).unwrap();
        out.rejected.into_iter().next().map(|r| r.reason)
    }

    /// PO `candidates.rs::a_pair_that_meets_every_constraint_is_eligible`.
    #[test]
    fn une_paire_qui_tient_toutes_les_contraintes_est_eligible() {
        let out = filter(Slot::Automatic, &tooled(), &[good()]).unwrap();
        assert_eq!(out.eligible, vec![good()]);
        assert!(out.rejected.is_empty());
    }

    /// PO `candidates.rs::a_remote_machine_is_never_a_candidate`. Divergence :
    /// l'hôte dit `remote`, le cœur ne lit pas de préfixe d'identifiant.
    #[test]
    fn une_machine_distante_n_est_jamais_candidate() {
        let mut facts = good();
        facts.remote = true;
        assert_eq!(reason(&tooled(), facts), Some(RejectReason::Remote));
    }

    /// PO `candidates.rs::a_pair_without_the_projects_consent_is_not_allowed`.
    #[test]
    fn une_paire_sans_consentement_n_est_pas_autorisee() {
        let mut facts = good();
        facts.allowed = false;
        assert_eq!(reason(&tooled(), facts), Some(RejectReason::NotAllowed));
    }

    /// PO `candidates.rs::an_unhealthy_instance_is_out_and_an_unmeasured_one_is_in`.
    #[test]
    fn une_instance_en_panne_sort_et_une_non_mesuree_reste() {
        let mut facts = good();
        facts.healthy = Some(false);
        assert_eq!(
            reason(&tooled(), facts.clone()),
            Some(RejectReason::Unhealthy)
        );
        facts.healthy = None;
        assert_eq!(reason(&tooled(), facts), None);
    }

    /// PO `candidates.rs::a_request_that_needs_tools_refuses_a_model_that_cannot_call_them`.
    #[test]
    fn un_tour_a_outils_refuse_un_modele_qui_n_en_appelle_pas() {
        let mut facts = good();
        facts.capabilities.tools = false;
        assert_eq!(
            reason(&tooled(), facts.clone()),
            Some(RejectReason::NoTools)
        );
        // Un utilitaire n'en a pas besoin.
        assert_eq!(reason(&signature(), facts), None);
    }

    /// PO `candidates.rs::a_window_smaller_than_the_need_or_unknown_is_rejected`.
    #[test]
    fn une_fenetre_trop_petite_ou_inconnue_est_rejetee() {
        let mut facts = good();
        facts.capabilities.context_window = Some(4_000);
        let need = signature().context_need_tokens;
        assert_eq!(
            reason(&signature(), facts.clone()),
            Some(RejectReason::ContextTooSmall {
                need,
                have: Some(4_000)
            })
        );
        facts.capabilities.context_window = None;
        assert_eq!(
            reason(&signature(), facts.clone()),
            Some(RejectReason::ContextTooSmall { need, have: None })
        );
        // PO `claude_code_window::a_model_the_live_catalog_gives_no_window_is_rejected_by_name`.
        facts.window_unknown = Some(UnknownWindow::NotInCatalog);
        let rejected = reason(&signature(), facts).unwrap();
        assert_eq!(
            rejected,
            RejectReason::WindowUnknown {
                need,
                why: UnknownWindow::NotInCatalog
            }
        );
        assert_eq!(rejected.code(), "window_unknown");
    }

    /// PO `candidates.rs::images_need_a_model_that_reads_them`.
    #[test]
    fn les_images_demandent_un_modele_qui_les_lit() {
        let mut with_images = tooled();
        with_images.needs_images = true;
        let mut facts = good();
        facts.capabilities.images = false;
        assert_eq!(
            reason(&with_images, facts.clone()),
            Some(RejectReason::NoImages)
        );
        assert_eq!(reason(&tooled(), facts), None);
    }

    /// PO `candidates.rs::a_turn_dearer_than_the_budget_left_is_rejected_and_an_unknown_price_passes`.
    #[test]
    fn un_tour_plus_cher_que_le_budget_est_rejete_et_un_prix_inconnu_passe() {
        let mut signature = signature();
        signature.budget_remaining_usd = Some(0.000_001);
        assert_eq!(reason(&signature, good()), Some(RejectReason::OverBudget));
        let mut unpriced = good();
        unpriced.price = None;
        assert_eq!(reason(&signature, unpriced), None);
        let mut free = good();
        free.cost_basis = CostBasis::Free;
        assert_eq!(reason(&signature, free), None);
    }

    /// PO `candidates.rs::an_explicit_choice_never_enters_the_filter`.
    #[test]
    fn un_choix_explicite_n_entre_jamais_dans_le_filtre() {
        let mut facts = good();
        facts.allowed = false;
        facts.capabilities.tools = false;
        assert_eq!(filter(Slot::Explicit, &tooled(), &[facts]), None);
    }

    /// PO `candidates.rs::the_first_failing_constraint_is_the_one_reported`.
    #[test]
    fn la_premiere_contrainte_manquee_est_celle_qu_on_rapporte() {
        let mut facts = good();
        facts.allowed = false;
        facts.healthy = Some(false);
        facts.capabilities.tools = false;
        assert_eq!(reason(&tooled(), facts), Some(RejectReason::NotAllowed));
    }

    /// PO `candidates.rs::the_estimate_is_none_without_a_price_and_grows_with_the_context`.
    #[test]
    fn l_estimation_est_absente_sans_prix_et_grandit_avec_le_contexte() {
        let small = TurnSignature::utility(TaskClass::UtilityCompaction, 10_000);
        let big = TurnSignature::utility(TaskClass::UtilityCompaction, 100_000);
        let facts = good();
        assert!(
            facts.estimated_turn_cost_usd(&small).unwrap()
                < facts.estimated_turn_cost_usd(&big).unwrap()
        );
        let mut unpriced = good();
        unpriced.price = None;
        assert_eq!(unpriced.estimated_turn_cost_usd(&small), None);
    }

    /// Ajout de this-rs (pas de test PO d'origine) : la sentinelle n'est jamais un candidat.
    #[test]
    fn auto_n_est_jamais_un_candidat() {
        for name in ["auto", " AUTO ", "Auto", "", "  "] {
            let mut facts = good();
            facts.model = name.into();
            assert_eq!(
                reason(&tooled(), facts),
                Some(RejectReason::Sentinel),
                "{name:?}"
            );
        }
        assert!(!is_auto_model("auto-router-v2"));
    }

    // ── Retenues de quota par modèle ────────────────────────────────────

    const CLAUDE_CODE: &str = "claude-code";
    const T0: u64 = 1_792_576_800_000; // 2026-10-21T10:00:00Z

    fn claude(model: &str) -> Candidate {
        let mut facts = good();
        facts.provider_id = CLAUDE_CODE.into();
        facts.model = model.into();
        facts
    }

    fn run(pool: &[Candidate], registry: &MemoryQuota) -> Filtered {
        let mut pool = pool.to_vec();
        apply_quota(&mut pool, registry);
        filter(Slot::Automatic, &tooled(), &pool).unwrap()
    }

    /// PO `candidates.rs::a_model_hold_removes_only_that_model_from_the_pool`.
    #[test]
    fn une_retenue_de_modele_ne_retire_que_ce_modele() {
        let mut registry = MemoryQuota::at(T0);
        registry.hold_model(CLAUDE_CODE, "claude-fable-5-1", T0 + 3_600_000);
        let out = run(
            &[claude("claude-fable-5-1"), claude("claude-sonnet-4-5")],
            &registry,
        );
        let eligible: Vec<_> = out.eligible.iter().map(|f| f.model.as_str()).collect();
        assert_eq!(eligible, vec!["claude-sonnet-4-5"], "{:?}", out.rejected);
        let rejected: Vec<_> = out
            .rejected
            .iter()
            .map(|r| r.candidate.model.as_str())
            .collect();
        assert_eq!(rejected, vec!["claude-fable-5-1"]);
    }

    /// PO `candidates.rs::an_instance_exhaustion_still_removes_every_model`.
    #[test]
    fn un_epuisement_d_instance_retire_tous_ses_modeles() {
        let mut registry = MemoryQuota::at(T0);
        registry.hold_account(CLAUDE_CODE, T0 + 60_000, None);
        let out = run(
            &[claude("claude-fable-5-1"), claude("claude-sonnet-4-5")],
            &registry,
        );
        assert!(out.eligible.is_empty(), "{:?}", out.eligible);
        assert!(
            out.rejected
                .iter()
                .all(|r| r.reason.code() == "quota_exhausted")
        );
    }

    /// PO `candidates.rs::an_expired_model_hold_filters_nothing`.
    #[test]
    fn une_retenue_expiree_ne_filtre_rien() {
        let mut registry = MemoryQuota::at(T0);
        registry.hold_model(CLAUDE_CODE, "claude-fable-5-1", T0 + 90_000);
        registry.set_now(T0 + 90_001);
        let out = run(
            &[claude("claude-fable-5-1"), claude("claude-sonnet-4-5")],
            &registry,
        );
        assert!(out.rejected.is_empty(), "{:?}", out.rejected);
        assert_eq!(out.eligible.len(), 2);
    }

    /// PO `candidates.rs::a_model_scoped_rejection_names_the_model`.
    #[test]
    fn un_rejet_de_modele_nomme_le_modele() {
        let mut registry = MemoryQuota::at(T0);
        let until = T0 + 3_600_000;
        registry.hold_model(CLAUDE_CODE, "claude-fable-5-1", until);
        let out = run(&[claude("claude-fable-5-1")], &registry);
        let rejected = out.rejected.into_iter().next().expect("rejeté");
        assert_eq!(
            rejected.reason,
            RejectReason::QuotaExhaustedModel {
                resets_at_ms: until,
                model: "claude-fable-5-1".into()
            }
        );
        assert_eq!(rejected.reason.code(), "quota_exhausted_model");
    }

    /// PO `candidates.rs::an_instance_exhaustion_outranks_a_model_hold`.
    #[test]
    fn un_epuisement_d_instance_passe_avant_une_retenue_de_modele() {
        let mut registry = MemoryQuota::at(T0);
        let account_until = T0 + 7_200_000;
        registry.hold_account(CLAUDE_CODE, account_until, None);
        registry.hold_model(CLAUDE_CODE, "claude-fable-5-1", T0 + 3_600_000);
        let out = run(&[claude("claude-fable-5-1")], &registry);
        let rejected = out.rejected.into_iter().next().expect("rejeté");
        assert_eq!(
            rejected.reason,
            RejectReason::QuotaExhausted {
                resets_at_ms: account_until,
                window: None
            }
        );
    }
}
