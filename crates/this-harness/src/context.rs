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

use crate::wire::{Message, Request, ToolSpec};

/// Caractères par jeton supposés (texte courant latin).
const CHARS_PAR_JETON: usize = 4;
/// Coût fixe d'un message : rôle, délimiteurs.
const COUT_MESSAGE: usize = 4;
/// Coût fixe d'un appel d'outil, en plus de son nom et de ses arguments.
const COUT_APPEL: usize = 4;
/// Forfait par image : leur coût réel dépend de la taille, que l'on ne lit pas.
const COUT_IMAGE: usize = 1000;
/// Coût fixe d'un outil présenté au modèle : nom, description et schéma sont
/// encadrés par le protocole, et un schéma vide n'est pas gratuit.
const COUT_OUTIL: usize = 4;

/// DÉFAUT corrigé : tout caractère comptait un quart de jeton. Le CJK, les emoji et
/// l'écriture non latine coûtent environ un jeton PAR caractère : 4 000 idéogrammes
/// étaient estimés 1 000 jetons, et `fit` ne coupait rien avant le refus du moteur.
/// ASCII : 4 caractères par jeton ; tout autre caractère : un jeton (borne haute prudente).
fn jetons_texte(texte: &str) -> usize {
    let ascii = texte.bytes().filter(|b| b.is_ascii()).count();
    let autres = texte.chars().count().saturating_sub(ascii);
    ascii.div_ceil(CHARS_PAR_JETON).saturating_add(autres)
}

fn cout_message(message: &Message) -> usize {
    let corps = jetons_texte(message.text());
    let images = message.images().len().saturating_mul(COUT_IMAGE);
    let appels = match message {
        Message::Assistant { calls, .. } => calls
            .iter()
            .map(|appel| {
                COUT_APPEL
                    .saturating_add(jetons_texte(&appel.id))
                    .saturating_add(jetons_texte(&appel.name))
                    .saturating_add(jetons_texte(&appel.arguments.to_string()))
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

/// Coût estimé d'une **surface d'outils** : ce que les schémas ajoutent à chaque
/// requête, et que le fil ne contient pas.
///
/// POURQUOI cette estimation existe : `fit` ne voit que des messages. Or un tour
/// envoie aussi `Toolbox::specs()` (`wire.rs`), et une surface large pèse plus
/// qu'une conversation courte. Tant que ce coût n'était nulle part, le harnais
/// pouvait annoncer « sous budget » une requête plus grosse que la fenêtre du
/// moteur — mesuré : 10 outils ≈ 1 370 jetons, soit 45 % d'un budget de 3 000
/// pris hors du compte (`tests/budget_requete.rs`).
///
/// Même heuristique que le texte : ≈ 4 caractères par jeton ASCII, un jeton par
/// caractère non ASCII, plus un coût fixe par outil (les délimiteurs du protocole).
pub fn estimate_tools_tokens(specs: &[ToolSpec]) -> usize {
    specs
        .iter()
        .map(|outil| {
            // Le schéma est sérialisé comme le transport l'écrira : le compte ne
            // doit pas dépendre de la forme en mémoire.
            let corps = format!(
                "{}{}{}",
                outil.name,
                outil.description,
                outil.parameters.to_string()
            );
            COUT_OUTIL.saturating_add(jetons_texte(&corps))
        })
        .fold(0usize, usize::saturating_add)
}

/// Coût estimé de la **requête envoyée** : le fil tel que le moteur le lit
/// ([`Request::thread`] : consigne système comprise) **plus** les schémas d'outils.
///
/// C'est le chiffre que `context_budget` doit borner, et celui qu'un test doit
/// mesurer pour dire quelque chose du tour réel. L'appeler sur la requête d'un
/// transport est la façon honnête de vérifier « la borne a mordu ».
pub fn estimate_request_tokens(request: &Request) -> usize {
    estimate_tokens(&request.thread()).saturating_add(estimate_tools_tokens(request.tools()))
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
        let mut systeme_dedans = false;
        if matches!(&messages[i], Message::Assistant { calls, .. } if !calls.is_empty()) {
            // DÉFAUT corrigé : un message système intercalé (contexte éphémère) arrêtait le
            // groupe ; la réponse `Tool` d'après devenait une unité à part, gardable alors
            // que son appel était retiré. Les systèmes intercalés restent dans le groupe
            // tant qu'une réponse suit ; le groupe est alors épinglé (les consignes ne
            // se retirent pas).
            loop {
                let mut suite = fin;
                while suite < messages.len() && matches!(messages[suite], Message::System { .. }) {
                    suite += 1;
                }
                if suite < messages.len() && matches!(messages[suite], Message::Tool { .. }) {
                    if suite > fin {
                        systeme_dedans = true;
                    }
                    fin = suite + 1;
                } else {
                    break;
                }
            }
        }
        let cout = estimate_tokens(&messages[i..fin]);
        let epinglee = systeme_dedans || matches!(messages[i], Message::System { .. });
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
    let (unites, garde) = planifier(messages, budget);
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

/// Ce que [`fit`] voudrait retirer, sans rien retirer : les indices des messages
/// et leur coût estimé.
///
/// POURQUOI une fonction à part et pas une copie de la logique : une compaction
/// doit résumer **exactement** ce que `fit` jetterait, sur les **mêmes frontières
/// de groupe** (un appel d'outil et ses réponses ne se séparent pas). Deux
/// découpages qui divergent produiraient un `Tool` orphelin, que le moteur refuse.
/// Les indices sont croissants ; ils ne sont pas forcément contigus (un message
/// système épinglé peut s'intercaler entre deux messages retirés).
pub fn removable(messages: &[Message], budget: usize) -> Removal {
    let (unites, garde) = planifier(messages, budget);
    let mut removal = Removal::default();
    for (k, u) in unites.iter().enumerate() {
        if !garde[k] {
            removal.indices.extend(u.debut..u.fin);
            removal.tokens = removal.tokens.saturating_add(u.cout);
        }
    }
    removal
}

/// Jetons attendus pour le texte d'un résumé (hors enveloppe). Un résumé plus gros
/// que cela n'est pas refusé d'avance : la garde de refit de la compaction le
/// rejette s'il ne tient plus, et le tour replie sur le rognage.
pub const RESUME_ATTENDU_TOKENS: usize = 128;

/// Jetons à réserver, avant d'appeler le compacteur, pour le résumé qu'il rendra.
///
/// Le résumé revient enfermé dans un conteneur `<untrusted_data>` : c'est un
/// surcoût **constant**, mesuré et non codé en dur — on enveloppe un corps vide
/// avec le même `wrap_with` et le même `nonce` que le résumé réel, puis on
/// compte ce message comme [`fit`] le comptera. S'y ajoute
/// [`RESUME_ATTENDU_TOKENS`] pour le texte lui-même.
///
/// POURQUOI réserver : [`removable`] ne retire qu'assez pour que le fil restant
/// tienne. La place laissée est arbitraire ; un résumé enveloppé qui dépasse
/// cette place est écarté par `fit` (il est le plus ancien). Sans réserve, la
/// compaction échoue dès que la marge est plus petite que l'enveloppe.
pub fn reserve_resume(nonce: &str) -> usize {
    let enveloppe = this_agent::untrusted::wrap_with(nonce, usize::MAX, "compaction", &[], "");
    estimate_tokens(&[Message::user(enveloppe)]).saturating_add(RESUME_ATTENDU_TOKENS)
}

/// [`removable`] avec la place du résumé réservée ([`reserve_resume`]) : ce qui
/// est résumé est un peu plus que ce que [`fit`] jetterait, voulu. Un tour de
/// plus part dans le résumé pour que celui-ci tienne après enveloppe.
pub fn removable_avec_resume(messages: &[Message], budget: usize, nonce: &str) -> Removal {
    removable(messages, budget.saturating_sub(reserve_resume(nonce)))
}

/// Les messages que [`fit`] retirerait, et ce qu'ils coûtent.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Removal {
    /// Indices (croissants) dans le fil d'origine.
    pub indices: Vec<usize>,
    /// Jetons estimés qu'ils représentent.
    pub tokens: usize,
}

/// Le plan commun de [`fit`] et [`removable`] : les unités, et lesquelles sont gardées.
fn planifier(messages: &[Message], budget: usize) -> (Vec<Unite>, Vec<bool>) {
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
    (unites, garde)
}
