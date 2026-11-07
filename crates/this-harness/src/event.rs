//! Ce que le harnais raconte pendant qu'il travaille.
//!
//! Un harnais qui ne rend que le texte final oblige l'interface à deviner :
//! elle ne sait pas si le modèle réfléchit, s'il attend un outil, ou s'il a
//! fini. Ces événements sont la seule chose que l'interface doit savoir lire —
//! et ils suffisent : chaque bulle, chaque étape, chaque état affiché en
//! dérive.
//!
//! Les noms suivent Project Orchestrator (`stream_delta`, `tool_use`,
//! `tool_result`, `result`, `pending_queue`) : deux harnais qui nomment
//! différemment la même chose se traduisent une fois de trop.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use this_agent::Stop;
use this_agent::evidence::Verdict;

/// Un message écrit par l'utilisateur pendant qu'un tour tourne.
///
/// Il est **tenu**, pas perdu : il part à la frontière du tour suivant. C'est
/// la moitié de la bidirectionnalité ; l'autre moitié est `interrupt`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Pending {
    pub id: u64,
    pub text: String,
    /// Date de mise en file, en millisecondes epoch.
    pub at_ms: f64,
    /// Les images jointes : elles attendent avec le texte, et partent avec lui.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub images: Vec<crate::wire::ImagePart>,
}

/// Ce qui sort du harnais, dans l'ordre où ça arrive.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Event {
    /// Un message tenu vient d'entrer dans la conversation.
    User { text: String },
    /// La file d'attente, en entier, à chaque changement.
    ///
    /// Toujours la liste complète, jamais un delta : une interface qui reçoit
    /// des diffs doit les rejouer, et se trompe dès qu'un message est déplacé.
    Pending {
        messages: Vec<crate::event::Pending>,
    },
    /// Une consigne interne du harnais (relance après un refus, après un
    /// silence). Ce n'est pas l'utilisateur qui parle : l'interface la rend
    /// discrètement, ou pas du tout.
    ///
    /// `kind` dit de quel genre elle est : l'interface n'a pas à le deviner
    /// d'après le texte. Absent d'un ancien message, `null`, ou d'un genre inconnu, il
    /// vaut [`HintKind::Other`].
    Hint {
        text: String,
        #[serde(default, deserialize_with = "kind_or_other")]
        kind: HintKind,
    },
    /// Un tour commence.
    Turn { index: u32 },
    /// Un morceau de texte du modèle. S'accumule dans le tour courant.
    ///
    /// Jamais vide : un `delta("")` du transport est un signe de vie (appel d'outil ou
    /// raisonnement en cours) qui réarme la garde de silence sans rien afficher.
    Delta { text: String },
    /// Le modèle demande un outil.
    ToolUse {
        id: String,
        tool: String,
        input: Value,
    },
    /// L'outil a répondu.
    ToolResult {
        id: String,
        tool: String,
        text: String,
        /// L'outil a refusé — ce n'est pas la même chose qu'une erreur.
        refused: bool,
        /// L'outil a échoué (il a planté, ou n'a pas pu faire ce qu'on lui demandait). Distinct
        /// d'un refus : une décision se contourne, une erreur se corrige.
        #[serde(default)]
        failed: bool,
        /// La cause de l'échec (`Outcome::failed`), pour que l'interface dise
        /// *pourquoi* au lieu d'afficher un simple « échec ».
        #[serde(default, skip_serializing_if = "Option::is_none")]
        failure: Option<String>,
        /// L'état n'a pas bougé : une action qui ne change rien n'est pas un
        /// progrès, et la boucle doit le voir.
        unchanged: bool,
        /// Empreintes de l'état avant et après l'appel (`Outcome::before` /
        /// `Outcome::after`) — vides quand l'outil n'observe rien.
        #[serde(default)]
        before: String,
        #[serde(default)]
        after: String,
        ms: u64,
    },
    /// Le texte définitif du tour (il remplace les deltas accumulés).
    Spoke { text: String },
    /// La requête a été ramenée sous le budget de contexte, et voici ce qui n'y
    /// est plus. Un tour qui oublie doit le dire : un fil coupé en silence se lit
    /// comme un modèle qui a perdu la mémoire, et personne ne peut le distinguer
    /// d'une hallucination.
    Trimmed {
        /// Messages retirés de la requête.
        messages: usize,
        /// Jetons estimés retirés.
        tokens: usize,
        /// Le fil gardé dépasse quand même le budget : le dernier message
        /// utilisateur ou une consigne système ne se coupent pas.
        over_budget: bool,
        /// De combien, en jetons estimés.
        excess: usize,
    },
    /// Des messages anciens ont été **résumés** au lieu d'être jetés : un seul
    /// message les remplace dans le fil (et dans le transcript du harnais).
    ///
    /// Distinct de [`Event::Trimmed`] parce que ce n'est pas le même oubli : ici
    /// l'essentiel est gardé, sous forme de résumé écrit par le compacteur — donc
    /// résumé *avec perte*, et à lire comme tel. Additif : un client qui ne connaît
    /// pas ce type l'ignore.
    Compacted {
        /// Messages anciens remplacés par le résumé.
        messages: usize,
        /// Jetons estimés de ce qui a été remplacé.
        tokens: usize,
        /// Jetons estimés du résumé qui le remplace (toujours ≤ `tokens`).
        summary_tokens: usize,
    },
    /// Le run s'est arrêté, et il dit pourquoi.
    Stopped {
        stop: Stop,
        /// Le dernier texte que le modèle a écrit.
        text: String,
        turns: u32,
        ms: u64,
        /// Code de panne du transport, quand l'arrêt vient d'une panne.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        code: Option<String>,
        /// Ce que les faits du run disent de lui (`Run::verdict`) : prouvé, non
        /// vérifié ou échec. Absent d'un ancien message.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        verdict: Option<Verdict>,
    },
}

/// Le genre d'un [`Event::Hint`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HintKind {
    /// Le rappel de la décision : le récit du modèle n'est pas prouvé par les
    /// étapes (en-tête `evidence::RAPPEL`).
    Claim,
    /// Une panne passagère du transport : le tour est rejoué.
    Retry,
    /// Le tour est interrompu : silence du flux, ou transport qui ne rend pas
    /// la main après l'annulation.
    Interrupted,
    /// La relance après un refus ou un échec d'outil.
    Refused,
    /// Toute autre consigne (action sans effet, tour muet…), et tout genre
    /// inconnu d'un message plus récent.
    #[default]
    #[serde(other)]
    Other,
}

/// `"kind": null` vaut [`HintKind::Other`], comme un genre absent : un client qui
/// écrit `null` pour « pas de genre » ne doit pas faire rejeter tout l'événement.
fn kind_or_other<'de, D>(deserializer: D) -> Result<HintKind, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Ok(Option::<HintKind>::deserialize(deserializer)?.unwrap_or_default())
}

impl Event {
    /// Un nom stable, pour les journaux et les tests.
    pub fn name(&self) -> &'static str {
        match self {
            Self::User { .. } => "user",
            Self::Pending { .. } => "pending",
            Self::Hint { .. } => "hint",
            Self::Turn { .. } => "turn",
            Self::Delta { .. } => "delta",
            Self::ToolUse { .. } => "tool_use",
            Self::ToolResult { .. } => "tool_result",
            Self::Spoke { .. } => "spoke",
            Self::Trimmed { .. } => "trimmed",
            Self::Compacted { .. } => "compacted",
            Self::Stopped { .. } => "stopped",
        }
    }
}

/// L'état du harnais, sérialisable : ce qu'il faut pour reprendre après une
/// fermeture d'app.
///
/// La file d'attente en fait partie : un message tapé juste avant que
/// l'application soit suspendue ne doit pas disparaître avec la page.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Snapshot {
    #[serde(default = "one")]
    pub version: u32,
    #[serde(default)]
    pub transcript: Vec<crate::wire::Message>,
    #[serde(default)]
    pub pending: Vec<Pending>,
}

fn one() -> u32 {
    1
}

impl Snapshot {
    pub fn empty() -> Self {
        Self::default()
    }
}

/// Ce qu'un appel d'outil a produit, vu par le harnais.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Outcome {
    /// Ce que le modèle lira.
    pub text: String,
    /// Le motif, si l'outil a refusé.
    pub refused: Option<String>,
    /// La cause, si l'outil a échoué.
    pub failed: Option<String>,
    /// Empreinte de l'état avant l'appel — vide quand l'outil ne touche rien.
    pub before: String,
    /// Empreinte de l'état après l'appel.
    pub after: String,
}

impl Outcome {
    pub fn ok(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            ..Self::default()
        }
    }
    pub fn refused(text: impl Into<String>, motif: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            refused: Some(motif.into()),
            ..Self::default()
        }
    }
    /// L'outil a échoué : le texte est ce que lira le modèle, `cause` ce que le harnais relance.
    pub fn failed(text: impl Into<String>, cause: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            failed: Some(cause.into()),
            ..Self::default()
        }
    }
    /// Un outil qui écrit observe son propre effet : c'est la seule mesure
    /// honnête de « ça a marché ».
    pub fn observed(mut self, before: impl Into<String>, after: impl Into<String>) -> Self {
        self.before = before.into();
        self.after = after.into();
        self
    }
    /// Vrai si l'appel a été observé **et** n'a rien changé.
    pub fn unchanged(&self) -> bool {
        !self.before.is_empty() && self.before == self.after
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hint_of(json: &str) -> Event {
        serde_json::from_str(json).expect("un indice lisible")
    }

    #[test]
    fn un_genre_nul_absent_ou_inconnu_vaut_other() {
        for json in [
            r#"{"type":"hint","text":"t","kind":null}"#,
            r#"{"type":"hint","text":"t"}"#,
            r#"{"type":"hint","text":"t","kind":"demain"}"#,
        ] {
            assert_eq!(
                hint_of(json),
                Event::Hint {
                    text: "t".to_string(),
                    kind: HintKind::Other
                },
                "{json}"
            );
        }
        assert_eq!(
            hint_of(r#"{"type":"hint","text":"t","kind":"retry"}"#),
            Event::Hint {
                text: "t".to_string(),
                kind: HintKind::Retry
            }
        );
    }

    /// La casse du fil ne change pas : `snake_case` pour le genre d'un indice,
    /// `PascalCase` pour le verdict (forme historique, lue par les journaux).
    #[test]
    fn la_casse_du_fil_est_stable() {
        let event = Event::Stopped {
            stop: Stop::Done,
            text: String::new(),
            turns: 1,
            ms: 0,
            code: None,
            verdict: Some(Verdict::Unverified),
        };
        let json = serde_json::to_value(&event).unwrap();
        assert_eq!(json["verdict"], "Unverified");
        let hint = serde_json::to_value(Event::Hint {
            text: "t".to_string(),
            kind: HintKind::Interrupted,
        })
        .unwrap();
        assert_eq!(hint["kind"], "interrupted");
    }

    #[test]
    fn un_verdict_inconnu_se_lit_unknown() {
        let lu: Verdict = serde_json::from_str("\"Demain\"").unwrap();
        assert_eq!(lu, Verdict::Unknown);
        let mut json = serde_json::to_value(Event::Stopped {
            stop: Stop::Done,
            text: String::new(),
            turns: 1,
            ms: 0,
            code: None,
            verdict: Some(Verdict::Proven),
        })
        .unwrap();
        json["verdict"] = serde_json::json!("PasEncore");
        let stop: Event = serde_json::from_value(json)
            .expect("un verdict plus récent ne fait pas rejeter l'arrêt");
        assert!(matches!(
            stop,
            Event::Stopped {
                verdict: Some(Verdict::Unknown),
                ..
            }
        ));
        let proven: Verdict = serde_json::from_str("\"Proven\"").unwrap();
        assert_eq!(proven, Verdict::Proven);
    }
}
