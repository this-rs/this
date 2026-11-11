//! La note : un tirage de Thompson par paire éligible, moins un coût, une
//! latence et une pression de quota normalisés, avec une exploration
//! epsilon-gloutonne bornée.
//!
//! Pur et déterministe : pour un `(candidats, bras, réglages, graine)` donné, **à
//! plateforme égale**, le résultat est toujours le même, donc une décision se
//! rejoue. L'échantillonneur est un petit gamma de Marsaglia-Tsang à graine ; rien
//! ici ne lit une horloge ni une source de hasard globale — la graine est
//! **injectée**.
//!
//! **Ce que la promesse n'est PAS** : « même graine, même décision, natif ↔ wasm ».
//! `ln`, `cos`, `powf`, `sqrt` n'ont pas de précision garantie d'une plateforme
//! (d'une `libm`) à l'autre : deux plateformes peuvent différer au dernier bit d'un
//! tirage, et un cas serré (écart de notes sous ce bruit) peut alors tomber de
//! l'autre côté. Conséquence pour un rejeu : rejouer une décision **persistée** sur
//! une autre plateforme que celle qui l'a prise ne doit jamais *corriger* son
//! choix — le choix enregistré (`Decision::chosen`, `Decision::used`) fait foi, et
//! le recalcul ne sert qu'à comparer les notes, avec une tolérance (≥ 1e-9). Un
//! rejeu bit à bit exige la même plateforme, ou des fonctions mathématiques
//! déterministes fournies par l'hôte — ce que ce lot ne fait pas.
//!
//! Les bras ([`ArmStats`]) sont l'interface par laquelle l'apprentissage se
//! branchera : ce lot les LIT (vides, la note retombe sur l'a priori), il ne les
//! écrit pas.
//!
//! Portage ligne à ligne de PO : `src/chat/provider/cognitive/scorer.rs`
//! (mêmes constantes, même ordre des tirages), pour qu'une même graine donne la
//! même décision des deux côtés, à plateforme égale.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use super::candidate::Candidate;
use super::mode::{MAX_EXPLORATION_EPSILON, QUOTA_NEAR_LIMIT, RoutingSettings};
use super::signature::{ChatIntent, TaskClass, TurnSignature};

/// Alias qui marque un modèle comme le fort et lent.
pub const ALIAS_DEEP: &str = "deep";
/// Alias qui marque un modèle comme le rapide et bon marché.
pub const ALIAS_FAST: &str = "fast";

/// La clé d'un bras : classe × instance × modèle.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ArmKey {
    pub class: String,
    pub provider_id: String,
    pub model: String,
}

impl ArmKey {
    pub fn new(
        class: impl Into<String>,
        provider_id: impl Into<String>,
        model: impl Into<String>,
    ) -> Self {
        Self {
            class: class.into(),
            provider_id: provider_id.into(),
            model: model.into(),
        }
    }
}

/// Ce qui est su d'un bras : un postérieur Beta et des moyennes courantes.
/// L'hôte le persiste ; le cœur ne fait que le lire.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ArmStats {
    pub key: ArmKey,
    /// Succès Beta (a priori compris).
    pub alpha: f64,
    /// Échecs Beta (a priori compris).
    pub beta: f64,
    /// Observations.
    pub n: u64,
    /// Somme des coûts marginaux connus.
    pub cost_sum_usd: f64,
    /// Observations dont le coût était connu.
    pub cost_n: u64,
    /// Somme des durées.
    pub latency_sum_ms: u64,
    /// Observations dont la durée était connue.
    pub latency_n: u64,
}

impl ArmStats {
    /// Un bras neuf avec l'a priori donné.
    pub fn fresh(key: ArmKey, prior_alpha: f64, prior_beta: f64) -> Self {
        Self {
            key,
            alpha: prior_alpha,
            beta: prior_beta,
            n: 0,
            cost_sum_usd: 0.0,
            cost_n: 0,
            latency_sum_ms: 0,
            latency_n: 0,
        }
    }

    /// Moyenne du postérieur Beta.
    pub fn mean(&self) -> f64 {
        self.alpha / (self.alpha + self.beta)
    }

    /// Coût marginal moyen connu par observation, `None` s'il n'a jamais été connu.
    pub fn mean_cost_usd(&self) -> Option<f64> {
        (self.cost_n > 0).then(|| self.cost_sum_usd / self.cost_n as f64)
    }

    /// Durée moyenne par observation, `None` si elle n'a jamais été connue.
    pub fn mean_latency_ms(&self) -> Option<f64> {
        (self.latency_n > 0).then(|| self.latency_sum_ms as f64 / self.latency_n as f64)
    }
}

/// Quel alias (s'il y en a un) pointe sur chaque `(instance, modèle)` : la
/// configuration déclarative, lue comme un a priori — jamais une règle.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PriorHints {
    aliases: HashMap<(String, String), String>,
}

impl PriorHints {
    pub fn new() -> Self {
        Self::default()
    }

    /// Ajoute un indice.
    pub fn with(mut self, provider: &str, model: &str, alias: &str) -> Self {
        self.aliases
            .insert((provider.to_owned(), model.to_owned()), alias.to_owned());
        self
    }

    /// L'alias qui pointe sur une paire.
    pub fn alias_of(&self, provider: &str, model: &str) -> Option<&str> {
        self.aliases
            .get(&(provider.to_owned(), model.to_owned()))
            .map(String::as_str)
    }
}

/// L'a priori Beta d'un bras neuf : Beta(1, 1), plus un succès quand l'alias
/// convient à la classe (`deep` sur le travail complexe, créatif et les
/// reprises ; `fast` sur le travail simple et la conversation ordinaire).
pub fn prior_for(class: TaskClass, alias: Option<&str>) -> (f64, f64) {
    let bonus = match (alias, class) {
        (Some(ALIAS_DEEP), TaskClass::Complex | TaskClass::Creative | TaskClass::Retry) => 1.0,
        (Some(ALIAS_FAST), TaskClass::Simple | TaskClass::Chat(ChatIntent::General)) => 1.0,
        _ => 0.0,
    };
    (1.0 + bonus, 1.0)
}

/// Un candidat noté.
#[derive(Debug, Clone, PartialEq)]
pub struct Scored {
    pub candidate: Candidate,
    /// Succès Beta utilisés (bras stocké, ou a priori d'un bras neuf).
    pub alpha: f64,
    /// Échecs Beta utilisés.
    pub beta: f64,
    /// Observations derrière le bras (0 pour un neuf).
    pub n: u64,
    /// Le tirage de Thompson.
    pub draw: f64,
    /// Pénalité de coût, dans `[0, 1]`.
    pub cost_norm: f64,
    /// Pénalité de latence, dans `[0, 1]`.
    pub latency_norm: f64,
    /// Pénalité de pression de quota, dans `[0, 1]`.
    pub quota_norm: f64,
    /// `draw - cost_weight·cost_norm - latency_weight·latency_norm - quota_weight·quota_norm`.
    pub utility: f64,
    /// Coût moyen connu du bras, quand il en a un.
    pub arm_mean_cost_usd: Option<f64>,
    /// Coût estimé d'un tour, quand le prix est connu.
    pub est_turn_cost_usd: Option<f64>,
    /// Cette paire a été prise par le tirage d'exploration, pas par son utilité.
    pub explored: bool,
}

/// La pénalité douce de la pression de quota d'un compte : 0 tant que la
/// fenêtre la plus pleine est à [`QUOTA_NEAR_LIMIT`] ou dessous, puis linéaire
/// jusqu'à 1 quand elle est pleine. Un compte que personne n'a mesuré (`None`)
/// n'est pas pénalisé — il est inconnu, pas vide.
pub fn quota_penalty(utilization: Option<f64>) -> f64 {
    let Some(used) = utilization.filter(|u| u.is_finite()) else {
        return 0.0;
    };
    let span = 1.0 - QUOTA_NEAR_LIMIT;
    if span <= 0.0 {
        return if used > QUOTA_NEAR_LIMIT { 1.0 } else { 0.0 };
    }
    ((used - QUOTA_NEAR_LIMIT) / span).clamp(0.0, 1.0)
}

/// La graine d'une décision : un hachage stable de son identifiant (128 bits,
/// la valeur d'un UUID chez PO).
pub fn seed_of(id: u128) -> u64 {
    let mut state = (id as u64) ^ ((id >> 64) as u64).rotate_left(17);
    splitmix64(&mut state)
}

fn splitmix64(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// FNV-1a : un hachage dont la valeur ne change jamais d'une compilation à
/// l'autre, pour qu'une décision persistée se rejoue sur un binaire plus récent.
fn stable_hash(parts: &[&str]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for part in parts {
        for byte in part.bytes().chain(std::iter::once(0u8)) {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(0x0100_0000_01b3);
        }
    }
    hash
}

struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self {
        Self(seed)
    }

    /// Uniforme dans l'intervalle ouvert (0, 1).
    fn uniform(&mut self) -> f64 {
        let bits = splitmix64(&mut self.0) >> 11;
        (bits as f64 + 0.5) / (1u64 << 53) as f64
    }

    fn normal(&mut self) -> f64 {
        let (u1, u2) = (self.uniform(), self.uniform());
        (-2.0 * u1.ln()).sqrt() * (2.0 * std::f64::consts::PI * u2).cos()
    }

    /// Gamma(shape, 1), Marsaglia-Tsang.
    fn gamma(&mut self, shape: f64) -> f64 {
        if shape < 1.0 {
            let u = self.uniform();
            return self.gamma(shape + 1.0) * u.powf(1.0 / shape);
        }
        let d = shape - 1.0 / 3.0;
        let c = 1.0 / (9.0 * d).sqrt();
        loop {
            let x = self.normal();
            let v = 1.0 + c * x;
            if v <= 0.0 {
                continue;
            }
            let v = v * v * v;
            let u = self.uniform();
            if u.ln() < 0.5 * x * x + d - d * v + d * v.ln() {
                return d * v;
            }
        }
    }
}

/// Un tirage de Beta(alpha, beta), déterministe pour une graine.
pub fn beta_draw(alpha: f64, beta: f64, seed: u64) -> f64 {
    let (alpha, beta) = (alpha.max(1e-3), beta.max(1e-3));
    let mut rng = Rng::new(seed);
    let x = rng.gamma(alpha);
    let y = rng.gamma(beta);
    let total = x + y;
    if total > 0.0 {
        (x / total).clamp(0.0, 1.0)
    } else {
        alpha / (alpha + beta)
    }
}

fn median(mut values: Vec<f64>) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    values.sort_by(f64::total_cmp);
    let mid = values.len() / 2;
    Some(if values.len() % 2 == 1 {
        values[mid]
    } else {
        (values[mid - 1] + values[mid]) / 2.0
    })
}

/// Chaque valeur divisée par le maximum ; les inconnues prennent la médiane des
/// connues ; tout vaut 0 quand aucune n'est connue (ou que le maximum est 0).
fn normalise(values: &[Option<f64>]) -> Vec<f64> {
    let known: Vec<f64> = values.iter().flatten().copied().collect();
    let Some(fill) = median(known) else {
        return vec![0.0; values.len()];
    };
    let filled: Vec<f64> = values.iter().map(|v| v.unwrap_or(fill)).collect();
    let max = filled.iter().copied().fold(0.0_f64, f64::max);
    if max <= 0.0 {
        return vec![0.0; values.len()];
    }
    filled.iter().map(|v| (v / max).clamp(0.0, 1.0)).collect()
}

/// Note `candidates` pour `signature`.
///
/// `arms` sont les bras stockés (de n'importe quelle classe : seuls ceux de la
/// classe de la signature comptent). Le résultat est trié, le meilleur d'abord ;
/// quand le tirage d'exploration tombe, la paire explorée passe en tête avec
/// `explored = true`. Les égalités se départagent sur l'instance puis le modèle :
/// l'ordre ne dépend **jamais** de l'ordre d'entrée.
pub fn score(
    signature: &TurnSignature,
    candidates: &[Candidate],
    arms: &[ArmStats],
    hints: &PriorHints,
    settings: &RoutingSettings,
    seed: u64,
) -> Vec<Scored> {
    if candidates.is_empty() {
        return Vec::new();
    }
    let class = signature.arm_key();
    let index = arms_of_class(arms, &class);
    let stored = |facts: &Candidate| {
        index
            .get(&(facts.provider_id.as_str(), facts.model.as_str()))
            .copied()
    };

    let costs: Vec<Option<f64>> = candidates
        .iter()
        .map(|facts| {
            stored(facts)
                .and_then(ArmStats::mean_cost_usd)
                .or_else(|| facts.estimated_turn_cost_usd(signature))
        })
        .collect();
    let latencies: Vec<Option<f64>> = candidates
        .iter()
        .map(|facts| stored(facts).and_then(ArmStats::mean_latency_ms))
        .collect();
    let cost_norm = normalise(&costs);
    let latency_norm = normalise(&latencies);

    let mut scored: Vec<Scored> = candidates
        .iter()
        .enumerate()
        .map(|(i, facts)| {
            let (alpha, beta, n) = match stored(facts) {
                Some(arm) => (arm.alpha, arm.beta, arm.n),
                None => {
                    let (a, b) = prior_for(
                        signature.class,
                        hints.alias_of(&facts.provider_id, &facts.model),
                    );
                    (a, b, 0)
                }
            };
            // Graine par l'identité du bras, pas par sa position.
            let arm_seed = seed ^ stable_hash(&[&class, &facts.provider_id, &facts.model]);
            let draw = beta_draw(alpha, beta, arm_seed);
            let quota_norm = quota_penalty(facts.quota_utilization);
            let utility = draw
                - settings.cost_weight * cost_norm[i]
                - settings.latency_weight * latency_norm[i]
                - settings.quota_weight * quota_norm;
            Scored {
                candidate: facts.clone(),
                alpha,
                beta,
                n,
                draw,
                cost_norm: cost_norm[i],
                latency_norm: latency_norm[i],
                quota_norm,
                utility,
                arm_mean_cost_usd: stored(facts).and_then(ArmStats::mean_cost_usd),
                est_turn_cost_usd: facts.estimated_turn_cost_usd(signature),
                explored: false,
            }
        })
        .collect();

    scored.sort_by(|a, b| {
        b.utility
            .total_cmp(&a.utility)
            .then_with(|| a.candidate.provider_id.cmp(&b.candidate.provider_id))
            .then_with(|| a.candidate.model.cmp(&b.candidate.model))
    });

    // Exploration : avec la probabilité epsilon (jamais au-dessus du plafond
    // dur), prendre une paire tirée uniformément à la place de la meilleure.
    let epsilon = settings
        .exploration_epsilon
        .clamp(0.0, MAX_EXPLORATION_EPSILON);
    let mut rng = Rng::new(seed ^ 0xA5A5_A5A5_5A5A_5A5A);
    let fires = rng.uniform() < epsilon;
    let pick = (rng.uniform() * scored.len() as f64) as usize;
    if fires && scored.len() > 1 {
        let mut explored = scored.remove(pick.min(scored.len() - 1));
        explored.explored = true;
        scored.insert(0, explored);
    }
    scored
}

/// Les bras de la classe `class`, indexés par (instance, modèle) : une recherche
/// en temps constant au lieu d'un parcours par candidat. À clé égale, le **premier**
/// bras gagne — la même règle que le parcours linéaire qu'il remplace (et que PO).
pub(crate) fn arms_of_class<'a>(
    arms: &'a [ArmStats],
    class: &str,
) -> HashMap<(&'a str, &'a str), &'a ArmStats> {
    let mut index = HashMap::new();
    for arm in arms.iter().filter(|arm| arm.key.class == class) {
        index
            .entry((arm.key.provider_id.as_str(), arm.key.model.as_str()))
            .or_insert(arm);
    }
    index
}

/// La clé du bras auquel une paire notée appartient.
pub fn arm_key_of(signature: &TurnSignature, candidate: &Candidate) -> ArmKey {
    ArmKey::new(
        signature.arm_key(),
        &candidate.provider_id,
        &candidate.model,
    )
}

#[cfg(test)]
mod tests {
    use super::super::candidate::{Capabilities, CostBasis, Price};
    use super::super::signature::ContextHints;
    use super::*;

    /// PO `scorer.rs::tests::facts`.
    fn facts(provider: &str, model: &str, input_price: Option<f64>) -> Candidate {
        Candidate {
            price: input_price.map(|p| Price {
                input_per_mtok: p,
                output_per_mtok: p,
            }),
            cost_basis: if input_price.is_some() {
                CostBasis::Priced
            } else {
                CostBasis::Subscription
            },
            healthy: Some(true),
            ..Candidate::new(
                provider,
                model,
                Capabilities {
                    tools: true,
                    images: false,
                    reasoning: false,
                    context_window: Some(200_000),
                },
            )
        }
    }

    fn signature(class: TaskClass) -> TurnSignature {
        let mut s = TurnSignature::from_chat("hello", false, ContextHints::default());
        s.class = class;
        s
    }

    fn arm(sig: &TurnSignature, f: &Candidate, alpha: f64, beta: f64) -> ArmStats {
        let mut a = ArmStats::fresh(arm_key_of(sig, f), alpha, beta);
        a.n = (alpha + beta) as u64;
        a
    }

    fn settings(epsilon: f64) -> RoutingSettings {
        RoutingSettings {
            exploration_epsilon: epsilon,
            ..RoutingSettings::default()
        }
    }

    /// PO `scorer.rs::the_quota_penalty_starts_only_past_the_named_threshold`.
    #[test]
    fn la_penalite_de_quota_ne_commence_qu_apres_le_seuil() {
        assert_eq!(quota_penalty(None), 0.0, "inconnu n'est pas vide");
        assert_eq!(quota_penalty(Some(0.0)), 0.0);
        assert_eq!(quota_penalty(Some(QUOTA_NEAR_LIMIT)), 0.0, "au seuil");
        assert_eq!(quota_penalty(Some(1.0)), 1.0, "fenêtre pleine");
        let mid = QUOTA_NEAR_LIMIT + (1.0 - QUOTA_NEAR_LIMIT) / 2.0;
        assert!((quota_penalty(Some(mid)) - 0.5).abs() < 1e-9);
        assert!(quota_penalty(Some(0.95)) < quota_penalty(Some(0.99)));
        assert_eq!(quota_penalty(Some(2.0)), 1.0, "borné en haut");
        assert_eq!(quota_penalty(Some(-1.0)), 0.0, "borné en bas");
        assert_eq!(quota_penalty(Some(f64::NAN)), 0.0, "jamais une utilité NaN");
    }

    /// PO `scorer.rs::an_account_near_its_limit_is_preferred_last_but_still_scored`
    /// (même intention ; bras tièdes identiques pour que seul le quota sépare).
    #[test]
    fn un_compte_pres_de_sa_limite_passe_dernier_mais_reste_note() {
        let sig = signature(TaskClass::Simple);
        let mut near = facts("near", "m", None);
        near.quota_utilization = Some(0.99);
        let calm = facts("calm", "m", None);
        let arms = [
            arm(&sig, &near, 500.0, 500.0),
            arm(&sig, &calm, 500.0, 500.0),
        ];
        let pool = [near.clone(), calm.clone()];
        let out = score(&sig, &pool, &arms, &PriorHints::new(), &settings(0.0), 7);
        assert_eq!(out.len(), 2, "le compte presque plein reste candidat");
        assert_eq!(out[0].candidate.provider_id, "calm");
        assert!(out[1].quota_norm > 0.8);
        // `quota_weight: 0` éteint la préférence : la pénalité ne pèse plus.
        let off = RoutingSettings {
            quota_weight: 0.0,
            ..settings(0.0)
        };
        let out = score(&sig, &pool, &arms, &PriorHints::new(), &off, 7);
        assert!(out.iter().all(|x| (x.utility - x.draw).abs() < 1e-12));
    }

    /// PO `scorer.rs::a_seed_gives_the_same_scores_and_other_seeds_differ`.
    #[test]
    fn une_graine_donne_les_memes_notes_et_d_autres_graines_different() {
        let sig = signature(TaskClass::Simple);
        let pool = vec![facts("a", "m", Some(1.0)), facts("b", "m", Some(1.0))];
        let hints = PriorHints::new();
        let first = score(&sig, &pool, &[], &hints, &settings(0.1), 42);
        assert_eq!(first, score(&sig, &pool, &[], &hints, &settings(0.1), 42));
        // Le départage est déterministe : l'ordre d'entrée ne compte pas.
        let reversed: Vec<_> = pool.iter().rev().cloned().collect();
        assert_eq!(
            first,
            score(&sig, &reversed, &[], &hints, &settings(0.1), 42)
        );
        let differs = (0..20).any(|s| {
            score(&sig, &pool, &[], &hints, &settings(0.1), s)[0]
                .candidate
                .provider_id
                != first[0].candidate.provider_id
        });
        assert!(differs, "le tirage dépend de la graine");
        assert_eq!(seed_of(0xDEAD_BEEF), seed_of(0xDEAD_BEEF));
    }

    /// PO `scorer.rs::utility_is_the_draw_minus_the_weighted_normalised_cost_and_latency`.
    #[test]
    fn l_utilite_est_le_tirage_moins_le_cout_et_la_latence_ponderes() {
        let sig = signature(TaskClass::Simple);
        let cheap = facts("cheap", "m", Some(1.0));
        let dear = facts("dear", "m", Some(10.0));
        let mut slow_arm = arm(&sig, &dear, 500.0, 500.0);
        slow_arm.latency_sum_ms = 20_000;
        slow_arm.latency_n = 2;
        let mut fast_arm = arm(&sig, &cheap, 500.0, 500.0);
        fast_arm.latency_sum_ms = 2_000;
        fast_arm.latency_n = 2;
        let s = RoutingSettings {
            cost_weight: 0.5,
            latency_weight: 0.2,
            exploration_epsilon: 0.0,
            ..RoutingSettings::default()
        };
        let out = score(
            &sig,
            &[dear.clone(), cheap.clone()],
            &[slow_arm, fast_arm],
            &PriorHints::new(),
            &s,
            7,
        );
        let by = |p: &str| out.iter().find(|x| x.candidate.provider_id == p).unwrap();
        assert!((by("dear").cost_norm - 1.0).abs() < 1e-9);
        assert!((by("cheap").cost_norm - 0.1).abs() < 1e-9);
        assert!((by("dear").latency_norm - 1.0).abs() < 1e-9);
        assert!((by("cheap").latency_norm - 0.1).abs() < 1e-9);
        for x in &out {
            let expected = x.draw - 0.5 * x.cost_norm - 0.2 * x.latency_norm;
            assert!((x.utility - expected).abs() < 1e-12);
        }
        assert_eq!(out[0].candidate.provider_id, "cheap");
    }

    /// PO `scorer.rs::an_unknown_cost_takes_the_median_and_none_known_means_no_penalty`.
    #[test]
    fn un_cout_inconnu_prend_la_mediane_et_aucun_connu_ne_penalise_pas() {
        let sig = signature(TaskClass::Simple);
        let pool = vec![
            facts("a", "m", Some(1.0)),
            facts("b", "m", Some(3.0)),
            facts("c", "m", Some(9.0)),
            facts("sub", "m", None),
        ];
        let out = score(&sig, &pool, &[], &PriorHints::new(), &settings(0.0), 1);
        let by = |p: &str| out.iter().find(|x| x.candidate.provider_id == p).unwrap();
        assert!(
            (by("sub").cost_norm - by("b").cost_norm).abs() < 1e-9,
            "médiane"
        );
        assert!((by("c").cost_norm - 1.0).abs() < 1e-9);

        let free = vec![facts("x", "m", None), facts("y", "m", None)];
        let out = score(&sig, &free, &[], &PriorHints::new(), &settings(0.0), 1);
        assert!(
            out.iter()
                .all(|x| x.cost_norm == 0.0 && x.latency_norm == 0.0)
        );
        assert!(score(&sig, &[], &[], &PriorHints::new(), &settings(0.0), 1).is_empty());
    }

    /// PO `scorer.rs::a_stored_arm_cost_wins_over_the_estimate`.
    #[test]
    fn le_cout_d_un_bras_stocke_gagne_sur_l_estimation() {
        let sig = signature(TaskClass::Simple);
        let a = facts("a", "m", Some(1.0));
        let b = facts("b", "m", Some(1.0));
        let mut measured = arm(&sig, &a, 10.0, 10.0);
        measured.cost_sum_usd = 4.0;
        measured.cost_n = 2;
        let out = score(
            &sig,
            &[a, b],
            &[measured],
            &PriorHints::new(),
            &settings(0.0),
            1,
        );
        let a = out.iter().find(|x| x.candidate.provider_id == "a").unwrap();
        assert_eq!(a.arm_mean_cost_usd, Some(2.0));
        assert!((a.cost_norm - 1.0).abs() < 1e-9);
    }

    /// PO `scorer.rs::the_alias_is_a_prior_on_a_fresh_arm_only_and_evidence_overrides_it`.
    #[test]
    fn l_alias_est_un_a_priori_de_bras_neuf_et_l_evidence_le_remplace() {
        assert_eq!(prior_for(TaskClass::Complex, Some("deep")), (2.0, 1.0));
        assert_eq!(prior_for(TaskClass::Creative, Some("deep")), (2.0, 1.0));
        assert_eq!(prior_for(TaskClass::Retry, Some("deep")), (2.0, 1.0));
        assert_eq!(prior_for(TaskClass::Simple, Some("deep")), (1.0, 1.0));
        assert_eq!(prior_for(TaskClass::Simple, Some("fast")), (2.0, 1.0));
        assert_eq!(
            prior_for(TaskClass::Chat(ChatIntent::General), Some("fast")),
            (2.0, 1.0)
        );
        assert_eq!(prior_for(TaskClass::Complex, Some("fast")), (1.0, 1.0));
        assert_eq!(prior_for(TaskClass::Complex, None), (1.0, 1.0));

        let sig = signature(TaskClass::Complex);
        let deep = facts("p", "big", None);
        let fast = facts("p", "small", None);
        let hints = PriorHints::new()
            .with("p", "big", "deep")
            .with("p", "small", "fast");
        let pool = [deep.clone(), fast.clone()];
        let out = score(&sig, &pool, &[], &hints, &settings(0.0), 3);
        let big = out.iter().find(|x| x.candidate.model == "big").unwrap();
        assert_eq!((big.alpha, big.beta, big.n), (2.0, 1.0, 0));

        // L'a priori déplace la moyenne du tirage...
        let mean_draw = |model: &str| {
            (0..400)
                .map(|s| {
                    score(&sig, &pool, &[], &hints, &settings(0.0), s)
                        .into_iter()
                        .find(|x| x.candidate.model == model)
                        .unwrap()
                        .draw
                })
                .sum::<f64>()
                / 400.0
        };
        assert!(mean_draw("big") > mean_draw("small") + 0.08);

        // ...mais un bras stocké l'ignore.
        let stored = arm(&sig, &deep, 1.0, 9.0);
        let out = score(&sig, &pool, &[stored], &hints, &settings(0.0), 3);
        let big = out.iter().find(|x| x.candidate.model == "big").unwrap();
        assert_eq!((big.alpha, big.beta), (1.0, 9.0));
    }

    /// PO `scorer.rs::arms_of_another_class_do_not_count`.
    #[test]
    fn les_bras_d_une_autre_classe_ne_comptent_pas() {
        let sig = signature(TaskClass::Simple);
        let f = facts("a", "m", None);
        let other = signature(TaskClass::Complex);
        let wrong = arm(&other, &f, 50.0, 1.0);
        let out = score(&sig, &[f], &[wrong], &PriorHints::new(), &settings(0.0), 1);
        assert_eq!((out[0].alpha, out[0].beta, out[0].n), (1.0, 1.0, 0));
    }

    /// PO `scorer.rs::exploration_never_fires_at_zero_and_is_bounded_by_the_hard_cap`.
    #[test]
    fn l_exploration_ne_tombe_jamais_a_zero_et_reste_sous_le_plafond() {
        let sig = signature(TaskClass::Simple);
        let pool = vec![
            facts("a", "m", None),
            facts("b", "m", None),
            facts("c", "m", None),
        ];
        let rate = |epsilon: f64| {
            let fired = (0..3_000u64)
                .filter(|s| {
                    score(&sig, &pool, &[], &PriorHints::new(), &settings(epsilon), *s)
                        .iter()
                        .any(|x| x.explored)
                })
                .count();
            fired as f64 / 3_000.0
        };
        assert_eq!(rate(0.0), 0.0);
        let at_cap = rate(0.25);
        assert!((0.20..=0.30).contains(&at_cap), "{at_cap}");
        let above = rate(0.9);
        assert!(above <= 0.30, "epsilon est borné à 0,25 : {above}");
        // Un candidat seul n'a rien à explorer.
        let one = score(
            &sig,
            &pool[..1],
            &[],
            &PriorHints::new(),
            &settings(0.25),
            1,
        );
        assert!(!one[0].explored);
        // La paire explorée passe en tête.
        let seed = (0..3_000u64)
            .find(|s| score(&sig, &pool, &[], &PriorHints::new(), &settings(0.25), *s)[0].explored)
            .unwrap();
        let out = score(&sig, &pool, &[], &PriorHints::new(), &settings(0.25), seed);
        assert_eq!(out.iter().filter(|x| x.explored).count(), 1);
    }

    /// PO `scorer.rs::the_beta_draw_stays_in_range_and_follows_its_mean`.
    #[test]
    fn le_tirage_beta_reste_dans_l_intervalle_et_suit_sa_moyenne() {
        for (a, b) in [(0.5, 0.5), (1.0, 1.0), (30.0, 3.0), (2.0, 40.0)] {
            let draws: Vec<f64> = (0..2_000).map(|s| beta_draw(a, b, s)).collect();
            assert!(draws.iter().all(|d| (0.0..=1.0).contains(d)));
            let mean = draws.iter().sum::<f64>() / draws.len() as f64;
            assert!((mean - a / (a + b)).abs() < 0.03, "{a},{b}: {mean}");
        }
        assert_eq!(beta_draw(2.0, 3.0, 9), beta_draw(2.0, 3.0, 9));
    }
}
