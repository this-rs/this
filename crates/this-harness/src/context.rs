//! La troisième borne d'un run : le **contexte**.
//!
//! Le harnais compte ses tours (`Limits::max_tours`) et son temps
//! (`Limits::budget_ms`), rien d'autre. Or `Request::thread()` renvoie tout le
//! transcript à chaque tour : la conversation grossit sans fin, et un jour le
//! moteur refuse la requête — le défaut « prompt is too long », déjà vu en
//! panne ailleurs dans l'écosystème, qui arrive ici sans prévenir.
//!
//! Ce module est **pur** : ni entrée/sortie, ni horloge. Il ramène un fil sous
//! un budget en retirant les plus anciens messages, et **dit** ce qu'il a
//! retiré. Le branchement (appeler [`fit`] avant le transport, remonter
//! [`Dropped`] dans un événement) appartient à l'appelant.
//!
//! ## Ce que [`estimate_tokens`] n'est pas
//!
//! C'est une **estimation**, jamais une mesure. Le compte exact demande le
//! tokenizer du fournisseur (et il diffère d'un moteur à l'autre), que ce
//! crate n'a pas et ne doit pas avoir. L'heuristique : ≈ 4 caractères par
//! jeton, plus un coût fixe par message (rôle, séparateurs) et par appel
//! d'outil, plus un forfait par image. Elle se trompe, dans les deux sens :
//! le code, le JSON et les langues non latines coûtent plus de jetons par
//! caractère ; un budget doit donc garder une marge sous la limite réelle du
//! moteur.

use crate::wire::Message;

/// Caractères par jeton supposés (texte courant latin).
const CHARS_PAR_JETON: usize = 4;
/// Coût fixe d'un message : rôle, délimiteurs.
const COUT_MESSAGE: usize = 4;
/// Coût fixe d'un appel d'outil, en plus de son nom et de ses arguments.
const COUT_APPEL: usize = 4;
/// Forfait par image : leur coût réel dépend de la taille, que l'on ne lit pas.
const COUT_IMAGE: usize = 1000;

fn jetons_texte(caracteres: usize) -> usize {
    caracteres.div_ceil(CHARS_PAR_JETON)
}

fn cout_message(message: &Message) -> usize {
    let corps = jetons_texte(message.text().chars().count());
    let images = message.images().len().saturating_mul(COUT_IMAGE);
    let appels = match message {
        Message::Assistant { calls, .. } => calls
            .iter()
            .map(|appel| {
                let texte = appel.id.chars().count()
                    + appel.name.chars().count()
                    + appel.arguments.to_string().chars().count();
                COUT_APPEL + jetons_texte(texte)
            })
            .fold(0usize, usize::saturating_add),
        _ => 0,
    };
    COUT_MESSAGE
        .saturating_add(corps)
        .saturating_add(images)
        .saturating_add(appels)
}

/// Estimation du coût d'un fil, en jetons. **Estimation, pas mesure** : voir la
/// documentation du module.
pub fn estimate_tokens(messages: &[Message]) -> usize {
    messages
        .iter()
        .map(cout_message)
        .fold(0usize, usize::saturating_add)
}

/// Ce qu'un ajustement a retiré, et s'il a dû dépasser le budget.
///
/// Rien n'est silencieux : un fil rogné sans le dire ressemble à un modèle qui
/// « oublie » sans raison.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Dropped {
    /// Nombre de messages retirés.
    pub messages: usize,
    /// Jetons estimés retirés.
    pub tokens: usize,
    /// Vrai si le fil gardé dépasse quand même le budget (le dernier message
    /// utilisateur et les messages système sont gardés quoi qu'il arrive).
    pub over_budget: bool,
    /// De combien, en jetons estimés (0 si `over_budget` est faux).
    pub excess: usize,
}

impl Dropped {
    /// Vrai si le fil est resté intact.
    pub fn is_empty(&self) -> bool {
        self.messages == 0
    }
}

/// Un fil ramené sous un budget, et ce qui a été retiré pour y arriver.
#[derive(Clone, Debug, PartialEq)]
pub struct Fitted {
    pub kept: Vec<Message>,
    pub dropped: Dropped,
}

/// Une unité insécable : un message, ou un appel d'outil et ses réponses.
///
/// Garder une réponse `Tool` sans son appel est refusé par le moteur
/// (« tool_calls without tool messages »).
struct Unite {
    debut: usize,
    fin: usize, // exclu
    cout: usize,
    epinglee: bool,
}

fn decouper(messages: &[Message]) -> Vec<Unite> {
    let mut unites = Vec::new();
    let mut i = 0;
    while i < messages.len() {
        let mut fin = i + 1;
        if matches!(&messages[i], Message::Assistant { calls, .. } if !calls.is_empty()) {
            while fin < messages.len() && matches!(messages[fin], Message::Tool { .. }) {
                fin += 1;
            }
        }
        let cout = estimate_tokens(&messages[i..fin]);
        let epinglee = matches!(messages[i], Message::System { .. });
        unites.push(Unite {
            debut: i,
            fin,
            cout,
            epinglee,
        });
        i = fin;
    }
    unites
}

/// Ramène `messages` sous `budget` jetons estimés.
///
/// - On retire les **plus anciens** ; le fil gardé est un suffixe contigu du
///   fil d'origine (plus les messages épinglés), dans l'ordre d'origine.
/// - Un appel d'outil et ses réponses partent ou restent **ensemble**.
/// - Sont épinglés, gardés même au-delà du budget : le **dernier message
///   utilisateur non synthétique** — la question, pas une relance du harnais
///   (`Message::hint`) ; sans question, la dernière relance (un fil sans la question est pire qu'un dépassement) et les
///   messages **système** (les consignes ne sont pas de l'historique). Leur
///   coût est décompté du budget d'abord ; si le total dépasse, c'est signalé
///   dans [`Dropped::over_budget`].
/// - Un `Message::Tool` déjà orphelin dans l'entrée n'est pas réparé : il est
///   traité comme un message ordinaire.
pub fn fit(messages: &[Message], budget: usize) -> Fitted {
    let mut unites = decouper(messages);
    // La question épinglée est le dernier message utilisateur NON synthétique :
    // le harnais pousse ses relances au rôle utilisateur, et épingler l'une
    // d'elles laissait la borne retirer la vraie question. Faute de question,
    // la dernière relance vaut mieux que rien.
    let derniere_question = messages
        .iter()
        .rposition(|m| m.is_user() && !m.is_synthetic())
        .or_else(|| messages.iter().rposition(Message::is_user));
    if let Some(derniere) = derniere_question
        && let Some(u) = unites
            .iter_mut()
            .find(|u| u.debut <= derniere && derniere < u.fin)
    {
        u.epinglee = true;
    }

    let epingle: usize = unites
        .iter()
        .filter(|u| u.epinglee)
        .map(|u| u.cout)
        .fold(0, usize::saturating_add);
    let mut reste = budget.saturating_sub(epingle);

    let mut garde = vec![false; unites.len()];
    for (k, u) in unites.iter().enumerate() {
        garde[k] = u.epinglee;
    }
    for (k, u) in unites.iter().enumerate().rev() {
        if u.epinglee {
            continue;
        }
        if u.cout > reste {
            break; // contigu : on ne ressuscite pas plus ancien après un trou
        }
        reste -= u.cout;
        garde[k] = true;
    }

    let mut kept = Vec::new();
    let mut dropped = Dropped::default();
    for (k, u) in unites.iter().enumerate() {
        if garde[k] {
            kept.extend_from_slice(&messages[u.debut..u.fin]);
        } else {
            dropped.messages += u.fin - u.debut;
            dropped.tokens = dropped.tokens.saturating_add(u.cout);
        }
    }
    let total = estimate_tokens(&kept);
    if total > budget {
        dropped.over_budget = true;
        dropped.excess = total - budget;
    }
    Fitted { kept, dropped }
}
