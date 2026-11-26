//! La signature d'un tour : ce qu'une demande exige, dans les termes qu'un
//! routeur sait noter.
//!
//! Calculée sans réseau et sans modèle : l'intention vient de mots-clés
//! (bilingue FR/EN), le besoin de contexte d'une estimation. Sa
//! [`TurnSignature::arm_key`] nomme la famille de bras à laquelle une décision
//! appartient : elle ne doit **jamais** être renommée (l'apprentissage s'y range).
//!
//! Portage de PO : `src/chat/provider/cognitive/signature.rs` et
//! `src/neurons/intent.rs` (`IntentDetector`).

use serde::{Deserialize, Serialize};

/// Plus petit contexte qu'un tour est supposé demander, en jetons.
pub const MIN_CONTEXT_TOKENS: u64 = 8_000;
/// Plus grande estimation de contexte, en jetons (un routeur ne planifie jamais au-delà).
pub const MAX_CONTEXT_TOKENS: u64 = 200_000;
/// Caractères par jeton (la même estimation que le reste du harnais).
const CHARS_PER_TOKEN: u64 = 4;

/// Qui parle : une personne (pilote) ou un travail délégué (exécuteur).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    /// La conversation d'une personne.
    Pilot,
    /// Un sous-agent, un utilitaire, une délégation.
    Executor,
}

/// L'intention d'une demande conversationnelle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChatIntent {
    /// Quelque chose échoue : erreurs, plantages, « pourquoi ».
    Debug,
    /// Comprendre : comment ça marche, explique.
    Explore,
    /// Changer ou relire du code existant.
    Impact,
    /// Créer, implémenter, planifier.
    Plan,
    /// Aucune intention particulière détectée.
    General,
}

impl ChatIntent {
    /// Nom stable, utilisé dans les clés de bras.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Debug => "debug",
            Self::Explore => "explore",
            Self::Impact => "impact",
            Self::Plan => "plan",
            Self::General => "general",
        }
    }
}

/// La classe de travail.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskClass {
    /// Un fichier, peu d'étapes.
    Simple,
    /// Plusieurs fichiers, beaucoup d'étapes, refonte.
    Complex,
    /// Conception ou fonctionnalité nouvelle.
    Creative,
    /// Toute tentative après la première.
    Retry,
    /// Proposition ponctuelle (graphe de fonctionnalités).
    UtilityFeatureGraph,
    /// Résumé de compaction.
    UtilityCompaction,
    /// Un tour de conversation.
    Chat(ChatIntent),
}

impl TaskClass {
    /// Clé stable de la classe : la famille de bras. Ne jamais renommer.
    pub fn key(self) -> String {
        match self {
            Self::Simple => "simple".into(),
            Self::Complex => "complex".into(),
            Self::Creative => "creative".into(),
            Self::Retry => "retry".into(),
            Self::UtilityFeatureGraph => "utility.feature_graph".into(),
            Self::UtilityCompaction => "utility.compaction".into(),
            Self::Chat(intent) => format!("chat.{}", intent.as_str()),
        }
    }

    /// Lit une classe énoncée par un appelant. Tout le reste est `None`.
    pub fn parse(text: &str) -> Option<Self> {
        let text = text.trim().to_lowercase();
        Some(match text.as_str() {
            "simple" => Self::Simple,
            "complex" => Self::Complex,
            "creative" => Self::Creative,
            "retry" => Self::Retry,
            "utility.feature_graph" => Self::UtilityFeatureGraph,
            "utility.compaction" => Self::UtilityCompaction,
            "chat" | "chat.general" => Self::Chat(ChatIntent::General),
            "chat.debug" => Self::Chat(ChatIntent::Debug),
            "chat.explore" => Self::Chat(ChatIntent::Explore),
            "chat.impact" => Self::Chat(ChatIntent::Impact),
            "chat.plan" => Self::Chat(ChatIntent::Plan),
            _ => return None,
        })
    }

    /// Complexité de base (1..=10) de la classe, avant les signaux du tour.
    fn base_complexity(self) -> u8 {
        match self {
            Self::Simple | Self::UtilityCompaction => 2,
            Self::UtilityFeatureGraph => 3,
            Self::Complex => 6,
            Self::Creative => 7,
            Self::Retry => 5,
            Self::Chat(ChatIntent::Debug) => 5,
            Self::Chat(ChatIntent::Plan | ChatIntent::Impact) => 4,
            Self::Chat(ChatIntent::Explore) => 3,
            Self::Chat(ChatIntent::General) => 2,
        }
    }
}

/// Ce que l'appelant sait du contexte qu'un tour demandera. Tout est optionnel :
/// rien ici n'est lu sur un disque par le code de signature.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct ContextHints {
    /// Jetons estimés du fil que le tour emportera (le harnais tient le
    /// transcript : il le sait — PO, lui, ne le compte pas, voir l'inventaire).
    pub history_tokens: u64,
    /// Jetons estimés des schémas d'outils que la requête portera.
    pub tool_schema_tokens: u64,
    /// USD restants dans le budget marginal, quand un budget s'applique.
    pub budget_remaining_usd: Option<f64>,
}

/// Une demande, telle qu'un routeur la voit.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TurnSignature {
    /// Pilote ou exécuteur.
    pub role: Role,
    /// Classe de travail.
    pub class: TaskClass,
    /// 1 (trivial) ..= 10 (le plus dur).
    pub complexity: u8,
    /// Jetons que le modèle doit pouvoir tenir, bornés à
    /// [`MIN_CONTEXT_TOKENS`]`..=`[`MAX_CONTEXT_TOKENS`].
    pub context_need_tokens: u64,
    /// Le tour appelle des outils.
    pub needs_tools: bool,
    /// L'entrée porte des images.
    pub needs_images: bool,
    /// USD restants dans le budget marginal, `None` sans budget.
    pub budget_remaining_usd: Option<f64>,
    /// Numéro de tentative, à partir de 1.
    pub attempt: u32,
}

impl TurnSignature {
    /// Clé de la famille de bras de cette signature (la clé de classe).
    pub fn arm_key(&self) -> String {
        self.class.key()
    }

    /// Un tour de conversation tapé par une personne.
    ///
    /// La classe est une métadonnée de compatibilité constante (`General`) : la
    /// longueur du message ajoute au plus deux points de complexité, et le
    /// message lui-même au besoin de contexte. Aucune intention n'est déduite du
    /// texte ici — la structure cognitive vient de `TaskRepr` / `LearnedClass`.
    pub fn from_chat(message: &str, has_images: bool, hints: ContextHints) -> Self {
        // MÉTADONNÉE DE COMPATIBILITÉ UNIQUEMENT. La classe n'est plus déduite du
        // texte : aucun lexique n'intervient dans le chemin cognitif. La structure
        // des connaissances et des apprentissages vient de `TaskRepr` / `LearnedClass`
        // (cf. `crate::cognition`), pas d'ici.
        let class = TaskClass::Chat(ChatIntent::General);
        let chars = message.chars().count();
        let length_bonus = (chars / 800).min(2) as u8;
        let message_tokens = chars as u64 / CHARS_PER_TOKEN;
        Self {
            role: Role::Pilot,
            class,
            complexity: clamp_complexity(class.base_complexity() + length_bonus),
            context_need_tokens: context_need(message_tokens, &hints),
            needs_tools: true,
            needs_images: has_images,
            budget_remaining_usd: hints.budget_remaining_usd,
            attempt: 1,
        }
    }

    /// Un utilitaire ponctuel (proposition, résumé de compaction) : pas
    /// d'outils, un exécuteur, le contexte qu'on lui confie.
    pub fn utility(class: TaskClass, context_tokens: u64) -> Self {
        let class = match class {
            TaskClass::UtilityCompaction => TaskClass::UtilityCompaction,
            _ => TaskClass::UtilityFeatureGraph,
        };
        Self {
            role: Role::Executor,
            class,
            complexity: clamp_complexity(class.base_complexity()),
            context_need_tokens: context_tokens.clamp(MIN_CONTEXT_TOKENS, MAX_CONTEXT_TOKENS),
            needs_tools: false,
            needs_images: false,
            budget_remaining_usd: None,
            attempt: 1,
        }
    }
}

fn clamp_complexity(value: u8) -> u8 {
    value.clamp(1, 10)
}

/// Le contexte qu'un tour demande : un plancher, plus le message, le fil déjà
/// tenu et les schémas d'outils.
fn context_need(message_tokens: u64, hints: &ContextHints) -> u64 {
    MIN_CONTEXT_TOKENS
        .saturating_add(message_tokens)
        .saturating_add(hints.history_tokens)
        .saturating_add(hints.tool_schema_tokens)
        .clamp(MIN_CONTEXT_TOKENS, MAX_CONTEXT_TOKENS)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chat(message: &str) -> TurnSignature {
        TurnSignature::from_chat(message, false, ContextHints::default())
    }

    /// INVARIANT DE CONCEPTION : aucun lexique ne classe une demande.
    ///
    /// Ce test garde la **suppression** : les quatre dictionnaires de mots-clés et
    /// `detect_intent` ne doivent jamais revenir dans le chemin de décision. Il lit
    /// la source du module, donc il voit une réintroduction que le typage ne verrait
    /// pas (un dictionnaire déplacé dans une constante, une regex, une table).
    #[test]
    fn aucun_lexique_dans_le_chemin_cognitif() {
        // On ne scanne que le code de PRODUCTION : le test lui-même nomme les motifs
        // interdits, il se trouverait sinon lui-même en faute.
        let source = include_str!("signature.rs");
        let production = source
            .split("#[cfg(test)]")
            .next()
            .expect("le module de tests suit le code de production");
        // `to_lowercase` n'est PAS dans la liste : `TaskClass::parse` l'utilise pour
        // lire une classe énoncée par un appelant — c'est un parseur, explicitement
        // autorisé. Ce qui est interdit, c'est de *classer* un texte libre.
        for interdit in ["KEYWORDS", "detect_intent", "lower.contains"] {
            assert!(
                !production.contains(interdit),
                "« {interdit} » est revenu dans signature.rs : un lexique classe à nouveau les demandes, ce que la directive interdit"
            );
        }
    }

    /// La classe n'est plus qu'une métadonnée de compatibilité : elle ne dépend
    /// plus du texte, donc deux textes opposés rendent la même métadonnée.
    #[test]
    fn la_classe_est_une_metadonnee_et_ne_lit_pas_le_texte() {
        for texte in [
            "pourquoi le paiement échoue ?",
            "why does auth fail?",
            "planifie un endpoint",
            "",
            "لا يعمل الدفع",
            "支払いが失敗する理由",
        ] {
            assert_eq!(
                chat(texte).class,
                TaskClass::Chat(ChatIntent::General),
                "la métadonnée de compatibilité ne doit pas être déduite du texte ({texte})"
            );
        }
    }

    /// PO `signature.rs::a_chat_request_is_a_pilot_with_tools_and_reads_images_from_the_caller`.
    #[test]
    fn un_tour_de_conversation_est_un_pilote_avec_outils() {
        let s = chat("hello");
        assert_eq!(s.role, Role::Pilot);
        assert!(s.needs_tools);
        assert!(!s.needs_images);
        assert_eq!(s.attempt, 1);
        assert_eq!(s.complexity, 2);
        assert_eq!(s.context_need_tokens, MIN_CONTEXT_TOKENS + 1);
        assert!(TurnSignature::from_chat("hello", true, ContextHints::default()).needs_images);
    }

    /// PO `signature.rs::the_context_need_grows_with_the_files_and_stays_bounded`
    /// (ici : le fil et les schémas d'outils à la place des fichiers).
    #[test]
    fn le_besoin_de_contexte_grandit_et_reste_borne() {
        let small = chat("hello");
        let big = TurnSignature::from_chat(
            "hello",
            false,
            ContextHints {
                history_tokens: 50_000,
                tool_schema_tokens: 4_000,
                budget_remaining_usd: None,
            },
        );
        assert!(big.context_need_tokens > small.context_need_tokens);
        assert_eq!(big.context_need_tokens, 8_000 + 1 + 54_000);
        let huge = TurnSignature::from_chat(
            "hello",
            false,
            ContextHints {
                history_tokens: u64::MAX,
                ..ContextHints::default()
            },
        );
        assert_eq!(huge.context_need_tokens, MAX_CONTEXT_TOKENS);
    }

    /// PO `signature.rs::a_utility_call_needs_no_tools_and_is_an_executor`.
    #[test]
    fn un_utilitaire_est_un_executeur_sans_outils() {
        let s = TurnSignature::utility(TaskClass::UtilityCompaction, 500);
        assert_eq!(s.role, Role::Executor);
        assert!(!s.needs_tools);
        assert_eq!(s.context_need_tokens, MIN_CONTEXT_TOKENS);
        assert_eq!(s.arm_key(), "utility.compaction");
        // Une classe qui n'est pas un utilitaire devient la proposition.
        assert_eq!(
            TurnSignature::utility(TaskClass::Simple, 9_000).class,
            TaskClass::UtilityFeatureGraph
        );
    }

    /// PO `signature.rs::arm_keys_are_stable_and_parse_round_trips`.
    #[test]
    fn les_cles_de_bras_sont_stables_et_se_relisent() {
        for (class, key) in [
            (TaskClass::Simple, "simple"),
            (TaskClass::Complex, "complex"),
            (TaskClass::Creative, "creative"),
            (TaskClass::Retry, "retry"),
            (TaskClass::UtilityFeatureGraph, "utility.feature_graph"),
            (TaskClass::UtilityCompaction, "utility.compaction"),
            (TaskClass::Chat(ChatIntent::Debug), "chat.debug"),
            (TaskClass::Chat(ChatIntent::Explore), "chat.explore"),
            (TaskClass::Chat(ChatIntent::Impact), "chat.impact"),
            (TaskClass::Chat(ChatIntent::Plan), "chat.plan"),
            (TaskClass::Chat(ChatIntent::General), "chat.general"),
        ] {
            assert_eq!(class.key(), key);
            assert_eq!(TaskClass::parse(key), Some(class));
        }
        assert_eq!(
            TaskClass::parse(" Chat "),
            Some(TaskClass::Chat(ChatIntent::General))
        );
        assert_eq!(TaskClass::parse("explore"), None);
    }

    /// PO `signature.rs::a_signature_survives_a_json_round_trip`.
    #[test]
    fn une_signature_survit_a_un_aller_retour_json() {
        let s = TurnSignature::from_chat("why does it fail", true, ContextHints::default());
        let json = serde_json::to_string(&s).unwrap();
        assert_eq!(serde_json::from_str::<TurnSignature>(&json).unwrap(), s);
    }
}
