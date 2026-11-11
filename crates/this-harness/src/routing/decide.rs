//! La décision : candidats, note, hystérésis, un motif lisible, et la règle
//! mode × étape × rôle qui dit si le choix est utilisé.
//!
//! [`decide`] est pure : le même `(id, demande, bras, indices)` donne toujours
//! la même décision — c'est ce qui la rend rejouable. Elle ne lit ni horloge ni
//! magasin : l'identifiant (donc la graine) est **injecté**.
//!
//! Portage de PO : `src/chat/provider/cognitive/decider.rs` (`decide_with`,
//! `decide_free`, `is_applied`, `utility_of`, `hysteresis_margin`) et
//! `feedback.rs` (`confidence_from_scores`).

use serde::{Deserialize, Serialize};

use super::candidate::{self, Candidate, Pick, RejectReason, Slot};
use super::mode::{LearningStage, MAX_EXPLORATION_EPSILON, ProviderRoutingMode, RoutingSettings};
use super::scorer::{self, ArmStats, PriorHints, Scored};
use super::signature::{Role, TurnSignature};

/// Un challenger doit battre la paire en vigueur d'autant pour la remplacer.
pub const HYSTERESIS_MARGIN: f64 = 0.1;

/// Observations minimales avant que la marge dérivée de la confiance s'applique.
/// En dessous, un gagnant à peine observé ne doit pas se verrouiller sur une
/// évidence mince : la marge de base décide.
pub const HYSTERESIS_WARMUP: u64 = 10;

/// Motif d'une décision qui n'a trouvé personne d'éligible.
pub const NO_CANDIDATE: &str = "no_candidate";

/// Un choix est-il utilisé, d'après le mode, l'étape et le rôle ?
///
/// `shadow` n'applique jamais ; `primary` n'applique rien ; `executors_only`
/// n'applique qu'aux exécuteurs ; `full` applique à tout le monde. À l'étape
/// `advisory`, le pilote est conseillé, jamais appliqué, dans tous les modes.
pub fn is_applied(mode: ProviderRoutingMode, stage: LearningStage, role: Role) -> bool {
    let by_mode = match mode {
        ProviderRoutingMode::Primary => false,
        ProviderRoutingMode::ExecutorsOnly => role == Role::Executor,
        ProviderRoutingMode::Full => true,
    };
    match stage {
        LearningStage::Shadow => false,
        LearningStage::Advisory => by_mode && role == Role::Executor,
        LearningStage::Auto => by_mode,
    }
}

/// Une option que le routeur a regardée.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Alternative {
    pub pick: Pick,
    /// Son utilité, `None` quand elle n'a pas été notée (rejetée avant).
    pub score: Option<f64>,
    /// Pourquoi elle n'était pas éligible, quand elle ne l'était pas.
    pub rejected: Option<RejectReason>,
}

/// Une demande de décision.
#[derive(Debug, Clone)]
pub struct DecideRequest {
    /// Ce qui est demandé.
    pub signature: TurnSignature,
    /// Les réglages en vigueur.
    pub settings: RoutingSettings,
    /// Toutes les paires utilisables, avec leurs faits.
    pub pool: Vec<Candidate>,
    /// L'emplacement est-il automatique ? Un emplacement explicite n'est jamais décidé.
    pub slot: Slot,
    /// Restreint les candidats à une instance.
    pub restrict_provider: Option<String>,
    /// La paire en vigueur, pour l'hystérésis d'une décision par tour.
    pub current: Option<Pick>,
    /// Indice du tour.
    pub turn_index: Option<u32>,
    /// Le tour suit un changement de modèle fait pour la MÊME classe au tour
    /// précédent : pas de tirage d'exploration, et la marge d'hystérésis
    /// s'applique aux MOYENNES des postérieurs (pas aux tirages) — deux tours de
    /// même classe ne peuvent donc jamais faire A → B → A sur des tirages bruités.
    pub steady: bool,
    /// Les gardes de l'appelant qui ont rétréci cette décision : chacune est
    /// ajoutée au motif sous la forme `; narrowed: <code>`.
    pub narrowed: Vec<&'static str>,
}

impl DecideRequest {
    /// Une demande qui n'a qu'une signature, des réglages et un pool.
    pub fn new(signature: TurnSignature, settings: RoutingSettings, pool: Vec<Candidate>) -> Self {
        Self {
            signature,
            settings,
            pool,
            slot: Slot::Automatic,
            restrict_provider: None,
            current: None,
            turn_index: None,
            steady: false,
            narrowed: Vec::new(),
        }
    }
}

/// Ce que le routeur répond — et ce que l'hôte peut persister, appliqué ou non.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Decision {
    /// L'identifiant de la décision : c'est aussi sa graine.
    pub id: u128,
    pub signature: TurnSignature,
    /// La paire choisie, `None` quand personne n'était éligible (ou emplacement explicite).
    pub chosen: Option<Pick>,
    /// L'utilité de la paire choisie.
    pub score: Option<f64>,
    /// La paire a été prise par le tirage d'exploration.
    pub explored: bool,
    /// Le taux d'exploration en vigueur.
    pub exploration_epsilon: f64,
    /// Confiance (1 − entropie normalisée des utilités), `None` sans note.
    pub confidence: Option<f64>,
    /// Une phrase lisible, sans URL ni secret.
    pub reason: String,
    /// Les paires notées, la meilleure d'abord, puis les rejetées.
    pub alternatives: Vec<Alternative>,
    /// Le choix est-il utilisé (mode × étape × rôle) ?
    pub applied: bool,
    pub mode: ProviderRoutingMode,
    pub stage: LearningStage,
    pub turn_index: Option<u32>,
}

fn class_words(signature: &TurnSignature) -> String {
    let key = signature.arm_key().replace(['.', '_'], " ");
    if key.starts_with("chat") {
        format!("{key} turn")
    } else {
        format!("{key} task")
    }
}

fn tokens(value: u64) -> String {
    if value >= 1_000 {
        format!("{}k", value / 1_000)
    } else {
        value.to_string()
    }
}

/// Une phrase lisible, sans URL ni secret : classe, fenêtre contre besoin, coût
/// estimé, taux de succès observé avec son compte. Le texte est celui de PO, mot
/// pour mot : un journal de décisions se lit pareil des deux côtés.
fn reason_of(signature: &TurnSignature, best: &Scored, explored: bool, kept: bool) -> String {
    let mut parts = vec![class_words(signature)];
    match best.candidate.capabilities.context_window {
        Some(window) => parts.push(format!(
            "window {} for {} needed",
            tokens(window),
            tokens(signature.context_need_tokens)
        )),
        None => parts.push(format!(
            "window unknown, {} needed",
            tokens(signature.context_need_tokens)
        )),
    }
    parts.push(match best.arm_mean_cost_usd.or(best.est_turn_cost_usd) {
        Some(cost) => format!("about {cost:.2} USD per turn estimated"),
        None => "cost unknown".to_owned(),
    });
    parts.push(if best.n > 0 {
        let rate = (best.alpha / (best.alpha + best.beta) * 100.0).round();
        format!("{rate:.0}% success over {} runs", best.n)
    } else {
        "no run observed yet".to_owned()
    });
    if signature.needs_images {
        parts.push("reads the attached images (models without vision excluded)".to_owned());
    }
    if explored {
        parts.push("picked by the exploration draw".to_owned());
    }
    if kept {
        parts.push("kept because no challenger is clearly better".to_owned());
    }
    format!(
        "{}/{}: {}",
        best.candidate.provider_id,
        best.candidate.model,
        parts.join(", ")
    )
}

/// L'utilité moyenne du postérieur, **depuis les nombres seuls**. Une seule
/// implémentation, partagée par le chemin vivant et tout rejeu.
pub fn utility_of(
    alpha: f64,
    beta: f64,
    cost_norm: f64,
    latency_norm: f64,
    quota_norm: f64,
    settings: &RoutingSettings,
) -> f64 {
    alpha / (alpha + beta)
        - settings.cost_weight * cost_norm
        - settings.latency_weight * latency_norm
        - settings.quota_weight * quota_norm
}

fn mean_utility(scored: &Scored, settings: &RoutingSettings) -> f64 {
    utility_of(
        scored.alpha,
        scored.beta,
        scored.cost_norm,
        scored.latency_norm,
        scored.quota_norm,
        settings,
    )
}

/// Confiance d'un champ de notes : 1 − entropie normalisée. 0 quand tout se
/// vaut (ou moins de deux notes), proche de 1 quand une note domine.
pub fn confidence_from_scores(scores: &[f64]) -> f64 {
    let scores: Vec<f64> = scores
        .iter()
        .copied()
        .filter(|s| s.is_finite())
        .map(|s| s.max(0.0))
        .collect();
    let k = scores.len();
    if k < 2 {
        return 0.0;
    }
    let sum: f64 = scores.iter().sum();
    if sum <= 0.0 {
        return 0.0;
    }
    let entropy: f64 = scores
        .iter()
        .map(|s| s / sum)
        .filter(|p| *p > 0.0)
        .map(|p| -p * p.ln())
        .sum();
    (1.0 - entropy / (k as f64).ln()).clamp(0.0, 1.0)
}

/// La marge d'hystérésis : de combien un challenger doit dépasser la paire en
/// vigueur. Dérivée de la confiance une fois les bras tièdes : un champ serré —
/// confiance basse, l'avantage est sans doute du bruit — élargit la marge ; un
/// gagnant net garde la marge de base. Sous [`HYSTERESIS_WARMUP`] observations
/// d'un côté ou de l'autre, la marge de base décide.
fn hysteresis_margin(
    scored: &[Scored],
    current_index: usize,
    settings: &RoutingSettings,
    steady: bool,
) -> f64 {
    if scored[0].n < HYSTERESIS_WARMUP || scored[current_index].n < HYSTERESIS_WARMUP {
        return HYSTERESIS_MARGIN;
    }
    // La confiance porte sur le MÊME signal que celui qui décide du changement.
    let utilities: Vec<f64> = scored
        .iter()
        .map(|s| {
            if steady {
                mean_utility(s, settings)
            } else {
                s.utility
            }
        })
        .collect();
    HYSTERESIS_MARGIN * (2.0 - confidence_from_scores(&utilities))
}

/// La décision pour `request`, sans toucher à aucun magasin.
///
/// - emplacement explicite : aucun choix, jamais appliquée ;
/// - **zéro** candidat éligible : `chosen: None`, motif [`NO_CANDIDATE`], les
///   rejets listés — c'est l'appelant qui se replie (sur le modèle en vigueur),
///   ce n'est jamais une erreur ;
/// - **un seul** candidat éligible : c'est lui, sans exploration ;
/// - sinon : la meilleure utilité, la paire en vigueur gardée tant qu'aucun
///   challenger ne la dépasse de la marge, égalités départagées sur
///   l'instance puis le modèle.
pub fn decide(
    id: u128,
    request: &DecideRequest,
    arms: &[ArmStats],
    hints: &PriorHints,
) -> Decision {
    let mut decision = decide_free(id, request, arms, hints);
    for code in &request.narrowed {
        decision.reason = format!("{}; narrowed: {code}", decision.reason);
    }
    decision
}

fn decide_free(
    id: u128,
    request: &DecideRequest,
    arms: &[ArmStats],
    hints: &PriorHints,
) -> Decision {
    let signature = &request.signature;
    let settings = &request.settings;
    let mut decision = Decision {
        id,
        signature: signature.clone(),
        chosen: None,
        score: None,
        explored: false,
        exploration_epsilon: 0.0,
        confidence: None,
        reason: String::new(),
        alternatives: Vec::new(),
        applied: false,
        mode: settings.mode,
        stage: settings.stage,
        turn_index: request.turn_index,
    };

    let pool: Vec<Candidate> = request
        .pool
        .iter()
        .filter(|facts| {
            request
                .restrict_provider
                .as_deref()
                .is_none_or(|only| facts.provider_id == only)
        })
        .cloned()
        .collect();
    let Some(filtered) = candidate::filter(request.slot, signature, &pool) else {
        decision.reason =
            "explicit choice: a provider named by the caller is never replaced".into();
        return decision;
    };
    let rejected = |decision: &mut Decision| {
        decision
            .alternatives
            .extend(filtered.rejected.iter().map(|r| Alternative {
                pick: r.candidate.pick(),
                score: None,
                rejected: Some(r.reason.clone()),
            }));
    };
    if filtered.eligible.is_empty() {
        decision.reason = NO_CANDIDATE.into();
        rejected(&mut decision);
        return decision;
    }

    // Un tour stable ne tire jamais d'exploration.
    let steady_settings;
    let score_settings = if request.steady {
        let mut s = settings.clone();
        s.exploration_epsilon = 0.0;
        steady_settings = s;
        &steady_settings
    } else {
        settings
    };
    let mut scored = scorer::score(
        signature,
        &filtered.eligible,
        arms,
        hints,
        score_settings,
        scorer::seed_of(id),
    );
    // Hystérésis : la paire en vigueur reste, sauf si un challenger est
    // nettement meilleur — par son tirage, ou, sur un tour stable, par sa
    // moyenne (une borne qui ne dépend pas du bruit des tirages).
    let gap = |best: &Scored, now: &Scored| {
        if request.steady {
            mean_utility(best, settings) - mean_utility(now, settings)
        } else {
            best.utility - now.utility
        }
    };
    let mut kept = false;
    if !scored[0].explored
        && let Some(current) = &request.current
        && let Some(index) = scored.iter().position(|s| s.candidate.pick() == *current)
        && index > 0
    {
        let margin = hysteresis_margin(&scored, index, settings, request.steady);
        if gap(&scored[0], &scored[index]) < margin {
            let stay = scored.remove(index);
            scored.insert(0, stay);
            kept = true;
        }
    }
    let best = &scored[0];
    decision.chosen = Some(best.candidate.pick());
    decision.score = Some(best.utility);
    decision.explored = best.explored;
    decision.reason = reason_of(signature, best, best.explored, kept);
    decision.applied = is_applied(settings.mode, settings.stage, signature.role);
    decision.exploration_epsilon = score_settings
        .exploration_epsilon
        .clamp(0.0, MAX_EXPLORATION_EPSILON);
    decision.alternatives = scored
        .iter()
        .map(|s| Alternative {
            pick: s.candidate.pick(),
            score: Some(s.utility),
            rejected: None,
        })
        .collect();
    rejected(&mut decision);
    let scores: Vec<f64> = decision
        .alternatives
        .iter()
        .filter_map(|a| a.score)
        .collect();
    decision.confidence = Some(confidence_from_scores(&scores));
    decision
}

#[cfg(test)]
mod tests {
    use super::super::candidate::{Capabilities, CostBasis, Price};
    use super::super::scorer::ArmKey;
    use super::super::signature::{ContextHints, TaskClass};
    use super::*;

    /// PO `decider.rs::tests::facts`.
    fn facts(provider: &str, model: &str, price: f64) -> Candidate {
        Candidate {
            price: Some(Price {
                input_per_mtok: price,
                output_per_mtok: price,
            }),
            cost_basis: CostBasis::Priced,
            healthy: Some(true),
            ..Candidate::new(
                provider,
                model,
                Capabilities {
                    tools: true,
                    images: false,
                    reasoning: false,
                    context_window: Some(128_000),
                },
            )
        }
    }

    fn pilot() -> TurnSignature {
        TurnSignature::from_chat("hello", false, ContextHints::default())
    }

    fn executor() -> TurnSignature {
        TurnSignature::utility(TaskClass::UtilityFeatureGraph, 9_000)
    }

    /// PO `decider.rs::tests::request`.
    fn request(
        signature: TurnSignature,
        mode: ProviderRoutingMode,
        stage: LearningStage,
    ) -> DecideRequest {
        DecideRequest::new(
            signature,
            RoutingSettings {
                mode,
                stage,
                exploration_epsilon: 0.0,
                ..RoutingSettings::default()
            },
            vec![facts("deepseek", "v4", 1.0), facts("glm", "big", 2.0)],
        )
    }

    fn full_auto(signature: TurnSignature) -> DecideRequest {
        request(signature, ProviderRoutingMode::Full, LearningStage::Auto)
    }

    fn go(id: u128, request: &DecideRequest, arms: &[ArmStats]) -> Decision {
        decide(id, request, arms, &PriorHints::new())
    }

    /// PO `decider.rs::utility_core_tests::the_utility_is_the_posterior_mean_minus_the_weighted_penalties`
    /// et `a_zero_weight_setting_leaves_only_the_posterior_mean`.
    #[test]
    fn l_utilite_est_la_moyenne_du_posterieur_moins_les_penalites() {
        let settings = RoutingSettings {
            cost_weight: 0.3,
            latency_weight: 0.2,
            quota_weight: 0.1,
            ..Default::default()
        };
        let utility = utility_of(3.0, 1.0, 0.5, 0.25, 0.75, &settings);
        assert!((utility - 0.475).abs() < 1e-12, "{utility}");
        let zero = RoutingSettings {
            cost_weight: 0.0,
            latency_weight: 0.0,
            quota_weight: 0.0,
            ..Default::default()
        };
        assert!((utility_of(1.0, 3.0, 9.0, 9.0, 9.0, &zero) - 0.25).abs() < 1e-12);
    }

    /// PO `decider.rs::applied_follows_the_mode_stage_and_role_table`.
    #[test]
    fn applique_suit_la_table_mode_etape_role() {
        use LearningStage::*;
        use ProviderRoutingMode::*;
        // (mode, étape, pilote appliqué, exécuteur appliqué)
        let table = [
            (Primary, Shadow, false, false),
            (Primary, Advisory, false, false),
            (Primary, Auto, false, false),
            (ExecutorsOnly, Shadow, false, false),
            (ExecutorsOnly, Advisory, false, true),
            (ExecutorsOnly, Auto, false, true),
            (Full, Shadow, false, false),
            (Full, Advisory, false, true),
            (Full, Auto, true, true),
        ];
        for (mode, stage, pilot_applied, executor_applied) in table {
            let p = go(1, &request(pilot(), mode, stage), &[]);
            let e = go(1, &request(executor(), mode, stage), &[]);
            assert!(p.chosen.is_some() && e.chosen.is_some());
            assert_eq!(p.applied, pilot_applied, "pilote {mode:?} {stage:?}");
            assert_eq!(e.applied, executor_applied, "exécuteur {mode:?} {stage:?}");
            assert_eq!((p.mode, p.stage), (mode, stage));
        }
    }

    /// PO `decider.rs::nothing_eligible_is_no_candidate_with_the_rejections_listed`.
    #[test]
    fn rien_d_eligible_est_no_candidate_avec_les_rejets_listes() {
        let mut req = full_auto(pilot());
        for f in &mut req.pool {
            f.allowed = false;
        }
        let decision = go(1, &req, &[]);
        assert_eq!(decision.chosen, None);
        assert!(!decision.applied);
        assert_eq!(decision.reason, NO_CANDIDATE);
        assert_eq!(decision.alternatives.len(), 2);
        assert!(
            decision
                .alternatives
                .iter()
                .all(|a| a.rejected == Some(RejectReason::NotAllowed) && a.score.is_none())
        );
    }

    /// PO `decider.rs::an_explicit_slot_is_returned_unapplied_without_a_choice`.
    #[test]
    fn un_emplacement_explicite_revient_sans_choix_et_non_applique() {
        let mut req = full_auto(executor());
        req.slot = Slot::Explicit;
        let decision = go(1, &req, &[]);
        assert_eq!(decision.chosen, None);
        assert!(!decision.applied);
        assert!(decision.reason.contains("explicit"));
    }

    /// PO `decider.rs::the_candidates_can_be_restricted_to_one_instance` — c'est
    /// aussi le cas mono-candidat : un seul éligible, on part dessus.
    #[test]
    fn les_candidats_se_restreignent_a_une_instance() {
        let mut req = full_auto(pilot());
        req.restrict_provider = Some("glm".into());
        for id in 0..50u128 {
            let decision = go(id, &req, &[]);
            assert_eq!(decision.chosen, Some(Pick::new("glm", "big")));
            assert_eq!(decision.alternatives.len(), 1);
            assert!(decision.applied && !decision.explored);
        }
    }

    /// Contrainte produit de PO (un seul moteur est un cas de PREMIER ORDRE) :
    /// quand un seul candidat est éligible, c'est lui — quelle que soit la
    /// graine, même à l'exploration maximale, même si la paire en vigueur est
    /// une autre. Pas de test PO unique : `scorer.rs::exploration_never_fires…`
    /// (« a single candidate has nothing to explore ») et le test ci-dessus.
    #[test]
    fn un_seul_candidat_eligible_est_choisi_quelle_que_soit_la_graine() {
        let mut req = full_auto(pilot());
        req.settings.exploration_epsilon = 0.25;
        req.pool[0].healthy = Some(false);
        req.current = Some(Pick::new("deepseek", "v4"));
        for id in 0..200u128 {
            let decision = go(id, &req, &[]);
            assert_eq!(decision.chosen, Some(Pick::new("glm", "big")), "{id}");
            assert!(!decision.explored);
            assert_eq!(
                decision.alternatives[1].rejected,
                Some(RejectReason::Unhealthy)
            );
        }
    }

    /// PO `decider.rs::a_learnt_arm_beats_a_dear_one_and_the_alternatives_are_best_first`.
    /// Les bras sont ceux que 20 observations laissent dans le magasin de PO
    /// (Beta(1,1) + 20 succès / + 20 échecs, coût 0,05 USD, 1 s).
    #[test]
    fn un_bras_appris_bat_un_bras_cher_et_les_alternatives_sont_triees() {
        let sig = executor();
        let observed = |provider: &str, model: &str, alpha: f64, beta: f64| ArmStats {
            n: 20,
            cost_sum_usd: 1.0,
            cost_n: 20,
            latency_sum_ms: 20_000,
            latency_n: 20,
            ..ArmStats::fresh(ArmKey::new(sig.arm_key(), provider, model), alpha, beta)
        };
        let arms = [
            observed("glm", "big", 21.0, 1.0),
            observed("deepseek", "v4", 1.0, 21.0),
        ];
        let mut req = full_auto(sig);
        req.pool.push({
            let mut gone = facts("sick", "m", 1.0);
            gone.healthy = Some(false);
            gone
        });
        let decision = go(0x5EED, &req, &arms);
        assert_eq!(decision.chosen, Some(Pick::new("glm", "big")));
        let scores: Vec<_> = decision
            .alternatives
            .iter()
            .filter_map(|a| a.score)
            .collect();
        assert!(scores.windows(2).all(|w| w[0] >= w[1]));
        assert_eq!(
            decision.alternatives.last().unwrap().rejected,
            Some(RejectReason::Unhealthy)
        );
        assert!(
            decision.reason.contains("95% success over 20 runs"),
            "{}",
            decision.reason
        );
    }

    /// PO `decider.rs::the_alias_hint_is_a_prior_the_decider_reads`.
    #[test]
    fn l_indice_d_alias_est_un_a_priori_que_le_decideur_lit() {
        let hints = PriorHints::new().with("glm", "big", "deep");
        let mut sig = pilot();
        sig.class = TaskClass::Complex;
        let mut req = full_auto(sig);
        req.settings.cost_weight = 0.0;
        let wins = (0..200u128)
            .filter(|i| {
                decide(i * 7919 + 1, &req, &[], &hints).chosen == Some(Pick::new("glm", "big"))
            })
            .count();
        assert!(wins > 110, "{wins}");
    }

    /// PO `decider.rs::the_same_id_arms_and_pool_give_the_same_decision`.
    #[test]
    fn le_meme_id_les_memes_bras_et_le_meme_pool_donnent_la_meme_decision() {
        let mut req = full_auto(pilot());
        req.settings.exploration_epsilon = 0.25;
        let arms = vec![ArmStats::fresh(
            ArmKey::new(req.signature.arm_key(), "glm", "big"),
            3.0,
            2.0,
        )];
        for i in 0..50u128 {
            let first = go(i + 1, &req, &arms);
            assert_eq!(first, go(i + 1, &req, &arms));
            // Départage déterministe : l'ordre du pool ne change rien.
            let mut reversed = req.clone();
            reversed.pool.reverse();
            assert_eq!(first, go(i + 1, &reversed, &arms));
        }
    }

    /// PO `decider.rs::hysteresis_keeps_the_current_pair_unless_a_challenger_clears_the_margin`.
    #[test]
    fn l_hysteresis_garde_la_paire_en_vigueur_sauf_challenger_net() {
        let mut req = full_auto(pilot());
        let id = (1..200u128)
            .find(|id| {
                let d = go(*id, &req, &[]);
                d.chosen == Some(Pick::new("deepseek", "v4"))
                    && d.alternatives[0].score.unwrap() - d.alternatives[1].score.unwrap() < 0.1
            })
            .expect("un cas serré existe");
        req.current = Some(Pick::new("glm", "big"));
        let kept = go(id, &req, &[]);
        assert_eq!(kept.chosen, Some(Pick::new("glm", "big")));
        assert!(kept.reason.contains("kept"));
        // Une marge large fait changer.
        let wide = (1..2_000u128)
            .find(|id| {
                let mut no_current = req.clone();
                no_current.current = None;
                let d = go(*id, &no_current, &[]);
                d.chosen == Some(Pick::new("deepseek", "v4"))
                    && d.alternatives[0].score.unwrap() - d.alternatives[1].score.unwrap() >= 0.1
            })
            .expect("une victoire nette existe");
        assert_eq!(
            go(wide, &req, &[]).chosen,
            Some(Pick::new("deepseek", "v4"))
        );
        // Une paire en vigueur qui n'est pas éligible n'est pas gardée.
        req.current = Some(Pick::new("ghost", "x"));
        assert_ne!(go(id, &req, &[]).chosen, Some(Pick::new("ghost", "x")));
    }

    /// PO `decider.rs::the_reason_is_one_readable_sentence_without_url_or_secret`.
    #[test]
    fn le_motif_est_une_phrase_lisible_sans_url_ni_secret() {
        let d = go(5, &full_auto(pilot()), &[]);
        assert!(!d.reason.contains("://") && !d.reason.contains("sk-"));
        assert!(d.reason.contains("window 128k"), "{}", d.reason);
        assert!(d.reason.contains("USD per turn"), "{}", d.reason);
        assert!(d.reason.contains("no run observed yet"), "{}", d.reason);
        assert!(d.reason.contains("chat general turn"), "{}", d.reason);
    }

    /// PO `decider.rs::tests::barely_observed`.
    fn barely_observed(sig: &TurnSignature) -> Vec<ArmStats> {
        let arm = |model: &str, alpha: f64, beta: f64, n: u64| ArmStats {
            n,
            ..ArmStats::fresh(ArmKey::new(sig.arm_key(), "local", model), alpha, beta)
        };
        vec![arm("a", 2.0, 1.0, 1), arm("b", 3.0, 2.0, 2)]
    }

    fn local_pair(sig: TurnSignature) -> DecideRequest {
        let mut req = full_auto(sig);
        req.pool = vec![facts("local", "a", 1.0), facts("local", "b", 1.0)];
        req
    }

    /// PO `decider.rs::tests::changes_over`.
    fn changes_over(turns: u128, steady: bool) -> usize {
        let sig = pilot();
        let arms = barely_observed(&sig);
        let mut req = local_pair(sig);
        req.settings.exploration_epsilon = 0.2;
        req.steady = steady;
        let mut current = Pick::new("local", "a");
        let mut changes = 0;
        for seed in 0..turns {
            req.current = Some(current.clone());
            let chosen = go(seed, &req, &arms).chosen.unwrap();
            if chosen != current {
                changes += 1;
                current = chosen;
            }
        }
        changes
    }

    /// PO `decider.rs::steady_turns_never_oscillate_between_two_close_barely_observed_arms`.
    #[test]
    fn des_tours_stables_n_oscillent_jamais_entre_deux_bras_proches() {
        assert_eq!(changes_over(1_000, true), 0);
        let noisy = changes_over(1_000, false);
        assert!(noisy > 100, "les tirages seuls oscillent : {noisy}");
    }

    /// PO `decider.rs::a_steady_turn_follows_a_clearly_better_mean_and_its_reason_says_it_was_narrowed`.
    #[test]
    fn un_tour_stable_suit_une_moyenne_nettement_meilleure_et_le_dit() {
        let sig = pilot();
        let arms = vec![
            ArmStats {
                n: 20,
                ..ArmStats::fresh(ArmKey::new(sig.arm_key(), "local", "a"), 6.0, 16.0)
            },
            ArmStats {
                n: 20,
                ..ArmStats::fresh(ArmKey::new(sig.arm_key(), "local", "b"), 16.0, 6.0)
            },
        ];
        let mut req = local_pair(sig);
        req.current = Some(Pick::new("local", "a"));
        req.steady = true;
        req.narrowed = vec!["same_class_no_explore", "after_provider_move"];
        let d = go(7, &req, &arms);
        assert_eq!(d.chosen, Some(Pick::new("local", "b")));
        assert!(
            d.reason
                .ends_with("; narrowed: same_class_no_explore; narrowed: after_provider_move"),
            "{}",
            d.reason
        );
    }

    /// PO `decider.rs::a_steady_turn_keeps_the_pair_in_force_against_a_mean_gap_under_the_margin`.
    #[test]
    fn un_tour_stable_garde_la_paire_contre_un_ecart_de_moyenne_sous_la_marge() {
        let sig = pilot();
        let arms = barely_observed(&sig);
        let mean = |m: &str| {
            arms.iter()
                .find(|a| a.key.model == m)
                .map(ArmStats::mean)
                .unwrap()
        };
        let gap = mean("a") - mean("b");
        assert!(gap > 0.0 && gap < HYSTERESIS_MARGIN, "{gap}");
        let mut req = local_pair(sig);
        req.settings.exploration_epsilon = 0.2;
        req.steady = true;
        req.current = Some(Pick::new("local", "b"));
        let changes = (0..1_000u128)
            .filter(|seed| go(*seed, &req, &arms).chosen != Some(Pick::new("local", "b")))
            .count();
        assert_eq!(changes, 0);
    }

    fn warm_or_cold(n_a: u64, n_b: u64) -> (DecideRequest, Vec<ArmStats>) {
        let sig = pilot();
        let arms = vec![
            // Challenger : moyenne 0,60.
            ArmStats {
                n: n_a,
                ..ArmStats::fresh(ArmKey::new(sig.arm_key(), "local", "a"), 12.0, 8.0)
            },
            // Paire en vigueur : moyenne 0,45.
            ArmStats {
                n: n_b,
                ..ArmStats::fresh(ArmKey::new(sig.arm_key(), "local", "b"), 9.0, 11.0)
            },
        ];
        let mut req = local_pair(sig);
        req.settings.cost_weight = 0.0;
        req.settings.latency_weight = 0.0;
        req.steady = true;
        req.current = Some(Pick::new("local", "b"));
        (req, arms)
    }

    /// PO `decider.rs::a_warm_tight_field_widens_the_margin_and_keeps_the_pair_in_force`.
    #[test]
    fn un_champ_tiede_et_serre_elargit_la_marge_et_garde_la_paire() {
        let (req, arms) = warm_or_cold(20, 20);
        let gap = arms[0].mean() - arms[1].mean();
        assert!(
            gap > HYSTERESIS_MARGIN && gap < 2.0 * HYSTERESIS_MARGIN,
            "{gap}"
        );
        let id = (0..2_000u128)
            .find(|id| {
                let mut no_current = req.clone();
                no_current.current = None;
                go(*id, &no_current, &arms).chosen == Some(Pick::new("local", "a"))
            })
            .expect("une graine où le challenger mène existe");
        let d = go(id, &req, &arms);
        assert_eq!(d.chosen, Some(Pick::new("local", "b")), "{}", d.reason);
        assert!(d.reason.contains("kept"), "{}", d.reason);
    }

    /// PO `decider.rs::a_barely_observed_pair_stays_on_the_base_margin`.
    #[test]
    fn une_paire_a_peine_observee_reste_sur_la_marge_de_base() {
        let (req, arms) = warm_or_cold(1, 2);
        let switched =
            (0..500u128).any(|s| go(s, &req, &arms).chosen == Some(Pick::new("local", "a")));
        assert!(
            switched,
            "une paire froide doit encore changer au-delà de la marge de base"
        );
    }

    /// PO `decider.rs::the_quota_reason_comes_before_health_and_all_exhausted_picks_nobody`
    /// (lu par son nom et par `candidates.rs::check` ; les entrées sont reconstruites).
    #[test]
    fn le_motif_de_quota_passe_avant_la_sante_et_tout_epuise_ne_choisit_personne() {
        use super::super::candidate::QuotaExhaustion;
        let mut req = full_auto(pilot());
        for f in &mut req.pool {
            f.healthy = Some(false);
            f.quota_exhausted = Some(QuotaExhaustion {
                resets_at_ms: 42,
                window: Some("seven_day".into()),
            });
        }
        let d = go(1, &req, &[]);
        assert_eq!(d.chosen, None);
        assert_eq!(d.reason, NO_CANDIDATE);
        assert!(d.alternatives.iter().all(|a| a.rejected
            == Some(RejectReason::QuotaExhausted {
                resets_at_ms: 42,
                window: Some("seven_day".into())
            })));
    }
}
