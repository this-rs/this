//! # La preuve contre le récit
//!
//! Le texte final du modèle est un **récit** : « j'ai créé la fiche », « je me souviens que… ».
//! Les étapes du run sont des **faits** : un outil a été appelé, il a réussi, refusé ou échoué,
//! et l'état a bougé ou non. Ce module confronte l'un à l'autre, sans entrée/sortie et sans
//! dépendance (pas de regex : le crate n'en a pas, et les motifs restent lisibles).
//!
//! Règle reprise de Project Orchestrator (`episodes/collector.rs`, `run_validation`, et
//! `neo4j/trajectory_migration.rs`, `estimate_outcome_reward`) :
//! - aucune preuve ⇒ [`Verdict::Unverified`], **jamais** un succès ;
//! - un run non terminé n'est jamais [`Verdict::Proven`] ;
//! - une note écrite par l'agent (`remember`) n'est pas une preuve : compter ses propres
//!   notes, c'est récompenser la production de texte.

use crate::Step;

/// Ce que les faits disent d'un run.
///
/// **Sur le fil**, le nom de la variante tel quel, en `PascalCase` (`"Proven"`,
/// `"Unverified"`, `"Failed"`) — et non en `snake_case` comme `HintKind` : c'est la forme
/// déjà rangée dans les journaux et le graphe (`_verdict` d'un `_run`), on ne la change pas.
/// Un nom inconnu (venu d'une version plus récente) se lit [`Verdict::Unknown`] au lieu de
/// faire rejeter tout le message.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Verdict {
    /// Le run est terminé et au moins une écriture réussie a changé l'état observé.
    Proven,
    /// Rien ne contredit le run, mais rien ne le prouve : ce n'est pas un échec, c'est
    /// « pas d'avis ».
    Unverified,
    /// Un refus ou un échec n'a été suivi d'aucune reprise réussie.
    Failed,
    /// Un verdict que cette version ne connaît pas (lecture tolérante). [`verdict`] ne le
    /// rend jamais ; un lecteur le traite comme « pas d'avis ».
    #[serde(other)]
    Unknown,
}

/// L'issue d'une étape, telle que l'outil l'a rendue.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    Ok,
    Refused,
    Failed,
}

/// La nature d'une étape, du point de vue de la preuve.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// Une action sur le monde (créer, relier, installer…).
    Write,
    /// Une lecture : `recall`, une requête au graphe, une liste.
    Read,
    /// Une note de l'agent pour lui-même (`remember`) : jamais une preuve.
    Note,
    /// Une action dont l'empreinte avant/après est égale : elle n'a rien fait.
    NoEffect,
}

/// Ce qu'un outil fait au monde, tel que l'hôte le déclare.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Effect {
    /// Il agit sur le monde : sa réussite observée est une preuve, son refus contredit
    /// « j'ai … ».
    Write,
    /// Il lit sans écrire : il prouve « je me souviens », jamais « j'ai créé ».
    Read,
    /// L'agent écrit pour lui-même (`remember`) : jamais une preuve d'action.
    Note,
}

/// La déclaration d'un outil par l'hôte : son effet, et les mots qui le désignent dans un
/// récit (verbes « déployer », "deploy" ; objets « module »).
///
/// Sans déclaration, le module devine : `remember`/`note` sont des notes, les préfixes
/// `get_`, `list`… des lectures, et la table interne relie les verbes usuels (« installé »
/// ⇒ un outil dont le nom contient `install`). Un hôte dont les outils portent d'autres noms
/// (`deploy_app`) déclare, et le contrôle « j'ai déployé » contre « `deploy_app` refusé »
/// fonctionne comme pour l'Atelier.
///
/// Les mots s'écrivent comme on veut (accents, majuscules, infinitif en -er) : « Déployer »
/// se lit `deploy` et désigne « déployé », « déployée », "deployed", "deploys".
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Declaration {
    /// Le nom de l'outil, tel que le modèle l'appelle.
    pub tool: String,
    pub effect: Effect,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub words: Vec<String>,
}

impl Declaration {
    pub fn new(tool: impl Into<String>, effect: Effect) -> Self {
        Self {
            tool: tool.into(),
            effect,
            words: Vec::new(),
        }
    }

    /// Un outil qui écrit.
    pub fn writes(tool: impl Into<String>) -> Self {
        Self::new(tool, Effect::Write)
    }

    /// Un outil qui lit.
    pub fn reads(tool: impl Into<String>) -> Self {
        Self::new(tool, Effect::Read)
    }

    /// Un outil de notes de l'agent.
    pub fn notes(tool: impl Into<String>) -> Self {
        Self::new(tool, Effect::Note)
    }

    /// Les mots qui désignent l'outil dans un récit.
    pub fn words<I, S>(mut self, words: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.words.extend(words.into_iter().map(Into::into));
        self
    }

    /// Le mot (déjà normalisé) désigne-t-il cet outil ?
    fn designe(&self, mot: &str) -> bool {
        self.words.iter().any(|w| forme_de(mot, &radical(w)))
    }
}

/// Les déclarations d'un run ; vide = le comportement par défaut.
type Decls<'a> = &'a [Declaration];

/// La déclaration de `tool`, s'il y en a une (le nom se compare sans la casse).
fn declaration<'a>(tool: &str, d: Decls<'a>) -> Option<&'a Declaration> {
    d.iter().find(|x| x.tool.eq_ignore_ascii_case(tool))
}

/// Le radical d'un mot déclaré : normalisé, sans la terminaison d'infinitif `-er`.
fn radical(mot: &str) -> String {
    let mot = normaliser(mot.trim());
    match mot.strip_suffix("er") {
        Some(r) if r.chars().count() >= 4 => r.to_string(),
        _ => mot,
    }
}

/// `mot` est-il une forme de `radical` : lui-même, un participe (`-e`, `-ee`, `-es`, `-ees`),
/// un passé ou un pluriel anglais (`-ed`, `-d`, `-s`), un gérondif (`-ing`) ?
fn forme_de(mot: &str, radical: &str) -> bool {
    const SUFFIXES: &[&str] = &["", "e", "ee", "es", "ees", "s", "d", "ed", "ing"];
    radical.chars().count() >= 3
        && mot
            .strip_prefix(radical)
            .is_some_and(|reste| SUFFIXES.contains(&reste))
}

/// L'affirmation que porte un verbe déclaré : une action pour un outil qui écrit, une note
/// pour un outil de notes ; rien pour une lecture.
fn declare(mot: &str, d: Decls) -> Option<Trouve> {
    let x = d
        .iter()
        .find(|x| x.effect != Effect::Read && x.designe(mot))?;
    Some(Trouve {
        claim: if x.effect == Effect::Note {
            Claim::Noted
        } else {
            Claim::Action
        },
        verbe: Some(mot.to_string()),
    })
}

/// L'issue d'une étape.
pub fn outcome(step: &Step) -> Outcome {
    if step.refused {
        Outcome::Refused
    } else if step.failure.is_some() {
        Outcome::Failed
    } else {
        Outcome::Ok
    }
}

fn nom(step: &Step) -> String {
    step.tool.to_lowercase()
}

/// `remember` et ses variantes : l'agent écrit pour lui-même.
fn est_note(tool: &str) -> bool {
    tool == "remember"
        || tool.starts_with("remember_")
        || tool == "note"
        || tool.starts_with("note_")
}

/// Les outils qui lisent sans écrire. La liste nomme ceux de l'atelier ; les préfixes
/// couvrent les conventions usuelles.
fn est_lecture(tool: &str) -> bool {
    const LECTURES: &[&str] = &[
        "recall",
        "graph_query",
        "graph_node",
        "graph_status",
        "link_list",
        "history",
        "search",
        "query",
    ];
    const PREFIXES: &[&str] = &[
        "recall", "read", "get_", "list", "search", "find", "query", "show", "lookup",
    ];
    LECTURES.contains(&tool)
        || PREFIXES.iter().any(|p| tool.starts_with(p))
        || tool.ends_with("_list")
        || tool.ends_with("_query")
        || tool.ends_with("_search")
        || tool.ends_with("_get")
}

/// La nature d'une étape.
///
/// Une action sans empreinte reste une écriture : l'outil n'observe pas toujours l'état,
/// et le récit « j'ai créé » ne doit pas être contredit faute de mesure. Elle ne suffit
/// pas pour autant à [`Verdict::Proven`], qui exige l'observation.
pub fn kind(step: &Step) -> Kind {
    kind_with(step, &[])
}

/// La nature d'une étape, d'après la déclaration de son outil quand l'hôte en a fait une
/// ([`Declaration`]), et sinon d'après son nom.
pub fn kind_with(step: &Step, d: &[Declaration]) -> Kind {
    let tool = nom(step);
    let effet = declaration(&step.tool, d).map(|x| x.effect);
    if effet == Some(Effect::Note) || (effet.is_none() && est_note(&tool)) {
        Kind::Note
    } else if effet == Some(Effect::Read) || (effet.is_none() && est_lecture(&tool)) {
        Kind::Read
    } else if !step.before.is_empty() && !step.a_change() {
        Kind::NoEffect
    } else {
        Kind::Write
    }
}

/// Une étape est-elle une preuve au sens de PO : une écriture réussie dont l'état a bougé.
pub fn is_proof(step: &Step) -> bool {
    is_proof_with(step, &[])
}

/// [`is_proof`], avec les déclarations de l'hôte.
pub fn is_proof_with(step: &Step, d: &[Declaration]) -> bool {
    outcome(step) == Outcome::Ok && kind_with(step, d) == Kind::Write && step.a_change()
}

/// Le dernier refus ou échec que rien n'a rattrapé ensuite : ni le même outil réussi, ni une
/// écriture réussie dont l'état observé a changé ([`is_proof`]). Une note (`remember`), une
/// lecture, une écriture sans empreinte ou sans effet ne rattrapent rien : « l'outil a dit
/// ok » n'est pas un fait.
///
/// C'est la mémoire du **run**, pas du tour : un refus au tour N reste vrai au tour N+1 tant
/// que personne ne l'a corrigé.
pub fn unresolved(steps: &[Step]) -> Option<&Step> {
    unresolved_with(steps, &[])
}

/// [`unresolved`], avec les déclarations de l'hôte : un outil déclaré `Write` rattrape
/// comme une écriture, un outil déclaré `Read` ou `Note` ne rattrape rien.
pub fn unresolved_with<'a>(steps: &'a [Step], d: &[Declaration]) -> Option<&'a Step> {
    steps.iter().enumerate().rev().find_map(|(i, etape)| {
        if outcome(etape) == Outcome::Ok {
            return None;
        }
        let rattrape = steps[i + 1..].iter().any(|apres| {
            outcome(apres) == Outcome::Ok && (apres.tool == etape.tool || is_proof_with(apres, d))
        });
        (!rattrape).then_some(etape)
    })
}

/// Les écritures refusées ou en échec que **le même outil** n'a jamais reprises avec succès —
/// même si une autre écriture a, depuis, prouvé autre chose. Un récit qui nomme cette action
/// (« j'ai installé » après `install_module` refusé) ne s'appuie pas sur une écriture sans
/// rapport.
fn non_repris<'a>(steps: &'a [Step], d: Decls) -> Vec<&'a Step> {
    steps
        .iter()
        .enumerate()
        .filter(|(i, etape)| {
            outcome(etape) != Outcome::Ok
                && !matches!(kind_with(etape, d), Kind::Read | Kind::Note)
                && !steps[i + 1..]
                    .iter()
                    .any(|apres| outcome(apres) == Outcome::Ok && apres.tool == etape.tool)
        })
        .map(|(_, etape)| etape)
        .collect()
}

/// Le verdict d'un run, d'après ses étapes. `completed` : le run s'est arrêté sur `Done`.
pub fn verdict(steps: &[Step], completed: bool) -> Verdict {
    verdict_with(steps, completed, &[])
}

/// [`verdict`], avec les déclarations de l'hôte.
pub fn verdict_with(steps: &[Step], completed: bool, d: &[Declaration]) -> Verdict {
    if unresolved_with(steps, d).is_some() {
        return Verdict::Failed;
    }
    if completed && steps.iter().any(|etape| is_proof_with(etape, d)) {
        Verdict::Proven
    } else {
        Verdict::Unverified
    }
}

/// L'en-tête de tout rappel : le modèle doit savoir que ce n'est pas l'utilisateur qui parle.
pub const RAPPEL: &str = "[RAPPEL AUTOMATIQUE — PAS UN MESSAGE DE L'UTILISATEUR]";

/// Ce que le récit affirme.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Claim {
    /// « j'ai créé », « c'est fait », "I added".
    Action,
    /// « j'ai noté », « j'ai enregistré » : une écriture, qu'une note peut couvrir.
    Noted,
    /// « je me souviens », « d'après ma mémoire », "I remember".
    Memory,
}

/// Une affirmation que les étapes ne prouvent pas.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Unproven {
    pub claim: Claim,
    /// La phrase d'origine, telle que le modèle l'a écrite.
    pub phrase: String,
    /// Ce qui manque, en une proposition (« aucune écriture réussie… »).
    pub manque: String,
    /// L'écriture refusée ou en échec que le récit contredit, quand il y en a une.
    pub etape: Option<Step>,
}

impl Unproven {
    /// Le texte du rappel à pousser au modèle.
    pub fn rappel(&self) -> String {
        format!(
            "{RAPPEL} Ta réponse affirme « {} », mais {}. Soit tu appelles maintenant l'outil qui le fait réellement, soit tu corriges ta phrase pour dire ce qui a vraiment été fait et ce qui reste.",
            self.phrase, self.manque
        )
    }
}

/// Une affirmation trouvée : sa nature, sa phrase, et le verbe qui la porte (`None` pour une
/// affirmation générique : « c'est fait », "done").
struct Trouve {
    claim: Claim,
    verbe: Option<String>,
}

/// Les affirmations du texte, chacune avec la phrase d'origine.
pub fn claims(said: &str) -> Vec<(Claim, String)> {
    claims_with(said, &[])
}

/// [`claims`], avec les déclarations de l'hôte : un verbe déclaré (« déployé ») est une
/// affirmation d'action même s'il n'est pas dans le vocabulaire par défaut.
pub fn claims_with(said: &str, d: &[Declaration]) -> Vec<(Claim, String)> {
    detections(said, d)
        .into_iter()
        .map(|(t, phrase, _)| (t.claim, phrase))
        .collect()
}

/// Chaque affirmation, avec sa phrase d'origine et les mots jugés (pour retrouver un objet
/// déclaré : « j'ai mis le module en place »).
fn detections(said: &str, d: Decls) -> Vec<(Trouve, String, Vec<String>)> {
    phrases(said)
        .into_iter()
        .filter_map(|p| {
            let mots = mots_a_juger(&p)?;
            claim_of(&mots, d).map(|t| (t, p.texte.trim().to_string(), mots))
        })
        .collect()
}

/// L'écriture en plan que nomme le verbe d'une affirmation.
///
/// 1. Les outils dont la déclaration contient ce verbe, et ceux que la table par défaut relie
///    à ce verbe (« installé » ⇒ `install_*`).
/// 2. Si le verbe ne désigne aucun outil (« mis »), les outils déclarés dont un **objet** est
///    nommé dans la phrase (« le module » ⇒ `install_module` s'il déclare `module`).
fn vise_par<'a>(verbe: &str, mots: &[String], en_plan: &[&'a Step], d: Decls) -> Option<&'a Step> {
    let declares: Vec<&Declaration> = d
        .iter()
        .filter(|x| x.effect != Effect::Read && x.designe(verbe))
        .collect();
    let jetons = outils_de(verbe);
    let par_verbe = en_plan.iter().rev().find(|etape| {
        declares
            .iter()
            .any(|x| x.tool.eq_ignore_ascii_case(&etape.tool))
            || jetons.is_some_and(|j| outil_fait(&etape.tool, j))
    });
    if par_verbe.is_some() || !declares.is_empty() || jetons.is_some() {
        return par_verbe.copied();
    }
    en_plan
        .iter()
        .rev()
        .find(|etape| {
            declaration(&etape.tool, d).is_some_and(|x| mots.iter().any(|m| x.designe(m)))
        })
        .copied()
}

/// La première affirmation du texte que les étapes ne prouvent pas.
///
/// - Une affirmation de mémoire exige une lecture réussie.
/// - Une affirmation d'action exige une écriture **observée** ([`is_proof`]), ou une écriture
///   réussie sans empreinte s'il n'y a aucun refus en suspens.
/// - Une action qui **nomme** une écriture refusée et jamais reprise par le même outil
///   (« j'ai installé » après `install_module` refusé) n'est pas prouvée, quelle que soit
///   l'écriture qui a réussi à côté ; une affirmation générique (« c'est fait ») non plus,
///   tant qu'une telle écriture reste en plan.
pub fn unproven(said: &str, steps: &[Step]) -> Option<Unproven> {
    unproven_with(said, steps, &[])
}

/// [`unproven`], avec les déclarations de l'hôte ([`Declaration`]) : l'effet déclaré d'un
/// outil remplace celui que son nom laisse deviner, et ses mots relient le récit à lui.
pub fn unproven_with(said: &str, steps: &[Step], d: &[Declaration]) -> Option<Unproven> {
    let suspens = unresolved_with(steps, d).is_some();
    let reussie = |k: Kind| {
        steps
            .iter()
            .any(|etape| outcome(etape) == Outcome::Ok && kind_with(etape, d) == k)
    };
    let ecriture = steps.iter().any(|etape| {
        is_proof_with(etape, d)
            || (!suspens
                && outcome(etape) == Outcome::Ok
                && kind_with(etape, d) == Kind::Write
                && etape.before.is_empty())
    });
    let note = reussie(Kind::Note);
    let lecture = reussie(Kind::Read);
    let en_plan = non_repris(steps, d);

    detections(said, d).into_iter().find_map(|(t, phrase, mots)| {
        let (manque, etape) = match t.claim {
            Claim::Memory => {
                if lecture {
                    return None;
                }
                (
                    "aucun `recall` ni aucune lecture du graphe réussie dans ce run ne le prouve"
                        .to_string(),
                    None,
                )
            }
            Claim::Action | Claim::Noted => {
                let vise = match (&t.verbe, t.claim) {
                    (Some(verbe), _) => vise_par(verbe, &mots, &en_plan, d),
                    (None, Claim::Action) => en_plan.last().copied(),
                    (None, _) => None,
                };
                if let Some(etape) = vise {
                    let pourquoi = etape
                        .motif
                        .clone()
                        .or_else(|| etape.failure.clone())
                        .unwrap_or_else(|| etape.resume.clone());
                    (
                        format!(
                            "`{}` n'a pas abouti dans ce run ({pourquoi}) et n'a jamais été repris avec succès — une autre écriture ne le remplace pas",
                            etape.tool
                        ),
                        Some(etape.clone()),
                    )
                } else if ecriture || (t.claim == Claim::Noted && note) {
                    return None;
                } else {
                    (
                        "aucune écriture réussie dans ce run ne le prouve (un outil refusé, en échec ou sans effet ne compte pas)"
                            .to_string(),
                        None,
                    )
                }
            }
        };
        Some(Unproven {
            claim: t.claim,
            phrase,
            manque,
            etape,
        })
    })
}

/// Confronte le récit aux étapes. Rend le texte d'un rappel si une affirmation n'est pas
/// prouvée (voir [`unproven`]), `None` sinon.
pub fn claim_check(said: &str, steps: &[Step]) -> Option<String> {
    claim_check_with(said, steps, &[])
}

/// [`claim_check`], avec les déclarations de l'hôte.
pub fn claim_check_with(said: &str, steps: &[Step], d: &[Declaration]) -> Option<String> {
    unproven_with(said, steps, d).map(|u| u.rappel())
}

// ---------------------------------------------------------------------------------------
// Découpage

/// Une phrase : le texte d'origine (pour le citer), le texte à juger (sans les citations de la
/// parole d'autrui), et « se termine-t-elle par un point d'interrogation ? ».
struct Phrase {
    texte: String,
    a_juger: String,
    question: bool,
}

/// Les caractères entre guillemets appariés (« », “ ”, " ") : une citation ne coupe pas la
/// phrase, et ce qu'elle contient n'est pas la parole du modèle.
fn masque_citations(chars: &[char]) -> Vec<bool> {
    let mut masque = vec![false; chars.len()];
    let mut pile: Vec<(usize, char)> = Vec::new();
    let mut droit: Option<usize> = None;
    for (i, &c) in chars.iter().enumerate() {
        let span = match c {
            '«' => {
                pile.push((i, '»'));
                None
            }
            '“' => {
                pile.push((i, '”'));
                None
            }
            '»' | '”' => pile.iter().rposition(|&(_, f)| f == c).map(|pos| {
                let debut = pile[pos].0;
                pile.truncate(pos);
                debut
            }),
            '"' => match droit.take() {
                Some(debut) => Some(debut),
                None => {
                    droit = Some(i);
                    None
                }
            },
            _ => None,
        };
        if let Some(debut) = span {
            masque[debut..=i].iter_mut().for_each(|m| *m = true);
        }
    }
    masque
}

/// Les phrases du texte.
fn phrases(texte: &str) -> Vec<Phrase> {
    let chars: Vec<char> = texte.chars().collect();
    let masque = masque_citations(&chars);
    let mut sortie: Vec<Phrase> = Vec::new();
    let mut courante: Vec<(char, bool)> = Vec::new();
    let fermer = |courante: &mut Vec<(char, bool)>, question: bool| -> Option<Phrase> {
        let morceau = std::mem::take(courante);
        let texte: String = morceau.iter().map(|(c, _)| *c).collect();
        if texte.trim().is_empty() {
            return None;
        }
        // Une citation n'est retirée que si la phrase a commencé avant elle : un récit
        // entièrement entre guillemets reste le récit du modèle.
        let mut parle = false;
        let a_juger = morceau
            .iter()
            .map(|&(c, cite)| {
                if cite && parle {
                    ' '
                } else {
                    if !cite && c.is_alphanumeric() {
                        parle = true;
                    }
                    c
                }
            })
            .collect();
        Some(Phrase {
            texte,
            a_juger,
            question,
        })
    };
    for (i, &c) in chars.iter().enumerate() {
        let fin = !masque[i] && matches!(c, '.' | '!' | '?' | '\n' | ';' | '…');
        if !fin {
            courante.push((c, masque[i]));
            continue;
        }
        match fermer(&mut courante, c == '?') {
            Some(phrase) => sortie.push(phrase),
            None => {
                // « Fait ?! » : le point d'interrogation suit une phrase déjà coupée.
                if c == '?'
                    && let Some(derniere) = sortie.last_mut()
                {
                    derniere.question = true;
                }
            }
        }
    }
    if let Some(phrase) = fermer(&mut courante, false) {
        sortie.push(phrase);
    }
    sortie
}

/// Minuscules, sans accents (précomposés ou combinants, NFD compris), apostrophes unifiées :
/// « CRÉÉ », « créé », « cre\u{301}e\u{301} » et « cree » se lisent pareil.
fn normaliser(texte: &str) -> String {
    let mut sortie = String::with_capacity(texte.len());
    for c in texte.chars().flat_map(char::to_lowercase) {
        match c {
            '\u{300}'..='\u{36f}' => {}
            'à' | 'á' | 'â' | 'ã' | 'ä' | 'å' => sortie.push('a'),
            'ç' => sortie.push('c'),
            'è' | 'é' | 'ê' | 'ë' => sortie.push('e'),
            'ì' | 'í' | 'î' | 'ï' => sortie.push('i'),
            'ñ' => sortie.push('n'),
            'ò' | 'ó' | 'ô' | 'õ' | 'ö' => sortie.push('o'),
            'ù' | 'ú' | 'û' | 'ü' => sortie.push('u'),
            'ý' | 'ÿ' => sortie.push('y'),
            'œ' => sortie.push_str("oe"),
            'æ' => sortie.push_str("ae"),
            '’' | '‘' | '`' | 'ʼ' => sortie.push('\''),
            _ => sortie.push(c),
        }
    }
    sortie
}

/// Une limite de proposition : la portée d'une condition s'arrête là.
fn separateur(c: char) -> bool {
    matches!(c, ',' | ':' | ';' | '—' | '–' | '(' | ')')
}

/// Les mots d'un texte déjà normalisé ; l'élision reste collée au mot qu'elle précède
/// (`j'ai` ⇒ `j'`, `ai`), et chaque limite de proposition devient le jeton `,`.
fn mots(phrase: &str) -> Vec<String> {
    let mut sortie: Vec<String> = Vec::new();
    let mut mot = String::new();
    for c in phrase.chars() {
        if c == '\'' {
            if !mot.is_empty() {
                mot.push('\'');
                sortie.push(std::mem::take(&mut mot));
            }
        } else if c.is_alphanumeric() {
            mot.push(c);
        } else {
            if !mot.is_empty() {
                sortie.push(std::mem::take(&mut mot));
            }
            if separateur(c) && sortie.last().is_some_and(|m| m != ",") {
                sortie.push(",".to_string());
            }
        }
    }
    if !mot.is_empty() {
        sortie.push(mot);
    }
    sortie
}

const INTERROGATIFS: &[&str] = &[
    "pourquoi", "comment", "quel", "quelle", "quels", "quelles", "combien", "did", "do", "does",
    "is", "are", "was", "were", "can", "could", "would", "should", "will", "shall", "have", "has",
    "may", "what", "how", "why", "where", "which", "who", "anything", "any",
];
const PRONOMS_INVERSES: &[&str] = &[
    "tu", "vous", "il", "elle", "on", "je", "nous", "ils", "elles",
];

/// Une proposition interrogative : « est-ce », une inversion (« veux-tu », « a-t-il »), ou un
/// mot interrogatif en tête. L'intonation seule (« c'est fait ? ») ne compte que pour la
/// dernière proposition de la phrase.
fn interrogative(proposition: &str) -> bool {
    if proposition.contains("est-ce") {
        return true;
    }
    let chars: Vec<char> = proposition.chars().collect();
    for (i, &c) in chars.iter().enumerate() {
        if c != '-' || i == 0 || !chars[i - 1].is_alphabetic() {
            continue;
        }
        let suite: String = chars[i + 1..]
            .iter()
            .take_while(|c| c.is_alphabetic())
            .collect();
        if PRONOMS_INVERSES.contains(&suite.as_str()) {
            return true;
        }
    }
    mots(proposition)
        .first()
        .is_some_and(|m| INTERROGATIFS.contains(&m.as_str()))
}

/// Les mots à juger d'une phrase. Dans une question, la dernière proposition est la question,
/// et toute proposition à partir de la première interrogative aussi ; ce qui précède reste une
/// affirmation (« J'ai créé X, veux-tu… ? », "Done — anything else?").
fn mots_a_juger(phrase: &Phrase) -> Option<Vec<String>> {
    let texte = normaliser(&phrase.a_juger);
    if !phrase.question {
        return Some(mots(&texte));
    }
    let propositions: Vec<&str> = texte.split(separateur).collect();
    let dernieres = propositions.len().saturating_sub(1);
    let gardees: Vec<&str> = propositions[..dernieres]
        .iter()
        .take_while(|p| !interrogative(p))
        .copied()
        .collect();
    if gardees.iter().all(|p| p.trim().is_empty()) {
        return None;
    }
    Some(mots(&gardees.join(",")))
}

// ---------------------------------------------------------------------------------------
// Motifs (tous écrits sans accents : le texte est normalisé avant d'être jugé)

const ADVERBES_FR: &[&str] = &[
    "bien",
    "deja",
    "aussi",
    "egalement",
    "maintenant",
    "enfin",
    "donc",
    "tout",
    "alors",
    "ensuite",
    "juste",
    "finalement",
    "correctement",
    "bel",
    "et",
    "desormais",
];
const NEGATIONS_FR: &[&str] = &["pas", "jamais", "rien", "point", "plus", "guere", "ni"];
/// `des` et `fois` ne sont des conditions que suivis de `que` (« dès que », « une fois que ») :
/// « une fois de plus » n'en est pas une, « des modules » non plus.
const CONDITIONS_FR: &[&str] = &["si", "quand", "lorsque", "lorsqu'", "des", "fois"];
const PRONOMS_FR: &[&str] = &[
    "l'", "les", "le", "la", "lui", "leur", "y", "en", "vous", "te", "t'", "m'", "me", "nous",
];

const PARTICIPES_FR: &[&str] = &[
    "cre", "ajout", "rajout", "install", "supprim", "reli", "li", "modifi", "retir", "activ",
    "desactiv", "effac", "renomm", "corrig", "connect", "branch", "configur", "deplac", "chang",
    "remplac",
];
const PARTICIPES_NOTE_FR: &[&str] = &["not", "enregistr", "memoris", "sauvegard", "consign"];
const IRREGULIERS_FR: &[&str] = &["mis", "mise", "mises", "ecrit", "ecrite", "ecrits"];
const IRREGULIERS_NOTE_FR: &[&str] = &["inscrit", "inscrite", "inscrits", "retenu", "retenue"];

const ADVERBES_EN: &[&str] = &[
    "have",
    "ve",
    "just",
    "already",
    "also",
    "now",
    "successfully",
    "went",
    "gone",
    "ahead",
    "and",
    "finally",
];
/// Ni `after` ni `before` : « After checking, I created… » affirme bel et bien.
const CONDITIONS_EN: &[&str] = &["if", "once", "when", "whether", "unless", "until"];
const VERBES_EN: &[&str] = &[
    "created",
    "added",
    "installed",
    "removed",
    "deleted",
    "linked",
    "connected",
    "updated",
    "wrote",
    "written",
    "enabled",
    "disabled",
    "renamed",
    "fixed",
    "configured",
    "registered",
    "replaced",
    "moved",
    "changed",
];
const VERBES_NOTE_EN: &[&str] = &["saved", "stored", "recorded", "noted", "memorized"];
/// Après « I did » : "I did create it".
const BASES_EN: &[&str] = &[
    "create",
    "add",
    "install",
    "remove",
    "delete",
    "link",
    "connect",
    "update",
    "write",
    "enable",
    "disable",
    "rename",
    "fix",
    "configure",
    "register",
    "replace",
    "move",
    "change",
];
const BASES_NOTE_EN: &[&str] = &["save", "store", "record", "note", "memorize"];
const MODAUX_EN: &[&str] = &[
    "will", "would", "should", "could", "might", "may", "must", "can", "ll", "d", "to",
];

/// `cre` + `e`, `ee`, `es`, `ees` (le texte est sans accents). Sans accents, la coupe est
/// ambiguë (« cree » = `cre` + `e` ou `cr` + `ee`) : toutes les coupes sont essayées.
fn participe(mot: &str, radicaux: &[&str]) -> bool {
    ["ees", "es", "ee", "e"]
        .iter()
        .filter_map(|suffixe| mot.strip_suffix(suffixe))
        .any(|r| radicaux.contains(&r))
}

/// Le début de la proposition qui contient le mot `i`.
fn debut_proposition(mots: &[String], i: usize) -> usize {
    mots[..i]
        .iter()
        .rposition(|m| m == ",")
        .map_or(0, |p| p + 1)
}

/// Une condition avant le mot `i`, **dans sa proposition** : « Si besoin, dis-le moi : j'ai
/// créé » affirme.
fn condition_avant(mots: &[String], i: usize, conditions: &[&str]) -> bool {
    (debut_proposition(mots, i)..i).any(|k| {
        let m = mots[k].as_str();
        let suite = mots.get(k + 1).map(String::as_str);
        match m {
            "des" | "fois" => matches!(suite, Some("que" | "qu'")),
            "s'" => matches!(suite, Some("il" | "ils")),
            _ => conditions.contains(&m),
        }
    })
}

fn est_negation(mot: &str) -> bool {
    matches!(
        mot,
        "ne" | "pas" | "jamais" | "rien" | "aucun" | "aucune" | "guere" | "not" | "never" | "no"
    ) || mot.ends_with("n'")
}

/// Une négation dans la proposition, avant le mot `i`.
fn nie_avant(mots: &[String], i: usize) -> bool {
    mots[debut_proposition(mots, i)..i]
        .iter()
        .any(|m| est_negation(m))
}

/// Pour les tournures sans sujet ou passives, plus ambiguës : ni négation, ni condition, ni
/// subordonnée (« vérifie que le module est installé », "I think that it has been created"),
/// ni modal ("will have been created") dans la proposition.
fn proposition_reservee(mots: &[String], i: usize) -> bool {
    let debut = debut_proposition(mots, i);
    nie_avant(mots, i)
        || condition_avant(mots, i, CONDITIONS_FR)
        || condition_avant(mots, i, CONDITIONS_EN)
        || mots[debut..i].iter().any(|m| {
            matches!(m.as_str(), "que" | "qu'" | "that") || MODAUX_EN.contains(&m.as_str())
        })
}

fn action(verbe: &str) -> Option<Trouve> {
    Some(Trouve {
        claim: Claim::Action,
        verbe: Some(verbe.to_string()),
    })
}

fn generique(claim: Claim) -> Option<Trouve> {
    Some(Trouve { claim, verbe: None })
}

fn participe_fr(mot: &str, d: Decls) -> Option<Trouve> {
    if let Some(trouve) = declare(mot, d) {
        return Some(trouve);
    }
    if participe(mot, PARTICIPES_NOTE_FR) || IRREGULIERS_NOTE_FR.contains(&mot) {
        return Some(Trouve {
            claim: Claim::Noted,
            verbe: Some(mot.to_string()),
        });
    }
    if participe(mot, PARTICIPES_FR) || IRREGULIERS_FR.contains(&mot) {
        return action(mot);
    }
    None
}

fn verbe_en(mot: &str, d: Decls) -> Option<Trouve> {
    if let Some(trouve) = declare(mot, d) {
        return Some(trouve);
    }
    if VERBES_NOTE_EN.contains(&mot) {
        return Some(Trouve {
            claim: Claim::Noted,
            verbe: Some(mot.to_string()),
        });
    }
    VERBES_EN.contains(&mot).then(|| action(mot)).flatten()
}

fn claim_of(mots: &[String], d: Decls) -> Option<Trouve> {
    action_fr(mots, d)
        .or_else(|| fait_fr(mots, d))
        .or_else(|| passif_fr(mots, d))
        .or_else(|| memoire_fr(mots))
        .or_else(|| action_en(mots, d))
        .or_else(|| fait_en(mots))
        .or_else(|| passif_en(mots, d))
        .or_else(|| sans_sujet(mots, d))
        .or_else(|| memoire_en(mots))
}

/// « j'ai créé », « je l'ai ajoutée », « nous avons relié » ; pas « je n'ai pas créé », pas
/// « si j'ai créé ».
fn action_fr(mots: &[String], d: Decls) -> Option<Trouve> {
    for (i, mot) in mots.iter().enumerate() {
        let sujet = match mot.as_str() {
            "ai" => ["j'", "je"],
            "avons" => ["nous", "nous"],
            _ => continue,
        };
        // Vers l'arrière : pronoms et négation jusqu'au sujet.
        let mut j = i;
        let mut nie = false;
        let mut sujet_trouve = None;
        while j > 0 {
            j -= 1;
            let m = mots[j].as_str();
            if sujet.contains(&m) {
                sujet_trouve = Some(j);
                break;
            }
            if m == "n'" || m == "ne" {
                nie = true;
            } else if !PRONOMS_FR.contains(&m) {
                break;
            }
        }
        let Some(debut) = sujet_trouve else { continue };
        if nie || condition_avant(mots, debut, CONDITIONS_FR) {
            continue;
        }
        // Vers l'avant : adverbes, puis le participe.
        for suivant in &mots[i + 1..] {
            let m = suivant.as_str();
            if ADVERBES_FR.contains(&m) {
                continue;
            }
            if NEGATIONS_FR.contains(&m) {
                break;
            }
            if let Some(trouve) = participe_fr(m, d) {
                return Some(trouve);
            }
            break;
        }
    }
    None
}

/// « c'est fait », « c'est noté », « voilà qui est fait », « c'est chose faite », « c'est en
/// place », et sans apostrophe « cest fait », « c est fait ».
fn fait_fr(mots: &[String], d: Decls) -> Option<Trouve> {
    for (i, mot) in mots.iter().enumerate() {
        let tete = match mot.as_str() {
            "cest" => i,
            "est" if i > 0 && matches!(mots[i - 1].as_str(), "c'" | "c" | "qui") => i - 1,
            _ => continue,
        };
        if condition_avant(mots, tete, CONDITIONS_FR) {
            continue;
        }
        let mut k = i + 1;
        while k < mots.len() && (ADVERBES_FR.contains(&mots[k].as_str()) || mots[k] == "chose") {
            k += 1;
        }
        let Some(m) = mots.get(k).map(String::as_str) else {
            continue;
        };
        let trouve = match m {
            "fait" | "faite" | "termine" | "terminee" | "regle" | "reglee" => {
                generique(Claim::Action)
            }
            "en" if mots.get(k + 1).is_some_and(|s| s == "place") => generique(Claim::Action),
            "installe" | "installee" | "cree" | "creee" | "ajoute" | "ajoutee" | "supprime"
            | "supprimee" => action(m),
            "note" | "notee" | "enregistre" | "enregistree" | "memorise" | "retenu" => {
                generique(Claim::Noted)
            }
            // « c'est déployé » : un verbe que l'hôte a déclaré.
            _ => declare(m, d),
        };
        if trouve.is_some() {
            return trouve;
        }
    }
    None
}

/// « Le module a été créé », « les fiches ont bien été ajoutées », « le module est maintenant
/// installé » ; pas « n'a pas été créé », pas « sera créé », pas « le module est déjà
/// installé » (un état, pas une action), pas « c'est lié à… » (une relation).
fn passif_fr(mots: &[String], d: Decls) -> Option<Trouve> {
    for (i, mot) in mots.iter().enumerate() {
        let ete = match mot.as_str() {
            "a" | "ont" => true,
            "est" | "sont" => false,
            _ => continue,
        };
        if i > 0 && matches!(mots[i - 1].as_str(), "c'" | "c" | "qui" | "y" | "n'" | "ne") {
            continue;
        }
        if proposition_reservee(mots, i) {
            continue;
        }
        let mut deja = false;
        let mut k = i + 1;
        let mut sauter = |k: &mut usize| {
            while *k < mots.len() && ADVERBES_FR.contains(&mots[*k].as_str()) {
                deja |= mots[*k] == "deja";
                *k += 1;
            }
        };
        sauter(&mut k);
        if ete {
            if mots.get(k).is_none_or(|m| m != "ete") {
                continue;
            }
            k += 1;
            sauter(&mut k);
        }
        if deja {
            continue;
        }
        let Some(p) = mots.get(k) else { continue };
        let suite = mots.get(k + 1).map(String::as_str);
        if !ete {
            let relation = participe(p, &["li", "reli", "connect", "branch"])
                && matches!(suite, Some("a" | "au" | "aux" | "avec"));
            let defaut = suite == Some("par") && mots.get(k + 2).is_some_and(|m| m == "defaut");
            if relation || defaut {
                continue;
            }
        }
        if let Some(trouve) = participe_fr(p, d) {
            return Some(trouve);
        }
    }
    None
}

/// « Module créé avec succès », "Successfully created the module", "created successfully".
fn sans_sujet(mots: &[String], d: Decls) -> Option<Trouve> {
    for (i, mot) in mots.iter().enumerate() {
        let m = mot.as_str();
        let trouve = if mots.get(i + 1).is_some_and(|s| s == "avec")
            && mots.get(i + 2).is_some_and(|s| s == "succes")
        {
            participe_fr(m, d)
        } else if m == "successfully" {
            mots.get(i + 1).and_then(|s| verbe_en(s, d))
        } else if mots.get(i + 1).is_some_and(|s| s == "successfully") {
            verbe_en(m, d)
        } else {
            None
        };
        if trouve.is_some() && !proposition_reservee(mots, i) {
            return trouve;
        }
    }
    None
}

/// « je me souviens », « je m'en souviens », « je me rappelle », « d'après ma mémoire », et
/// les paraphrases d'une conversation passée : « tu m'avais dit », « d'après nos échanges
/// précédents » ; pas « je ne me souviens pas », pas « tu m'as dit » (le fil courant). Un
/// « si je me souviens bien » reste une affirmation.
fn memoire_fr(mots: &[String]) -> Option<Trouve> {
    let memoire = generique(Claim::Memory);
    for (i, mot) in mots.iter().enumerate() {
        let m = mot.as_str();
        if m == "souviens" || m == "rappelle" {
            let mut j = i;
            let mut nie = false;
            let mut sujet = false;
            while j > 0 {
                j -= 1;
                match mots[j].as_str() {
                    "je" | "j'" => {
                        sujet = true;
                        break;
                    }
                    "n'" | "ne" => nie = true,
                    "me" | "m'" | "en" | "le" | "l'" | "la" | "les" | "bien" => {}
                    _ => break,
                }
            }
            let apres = mots.get(i + 1).map(String::as_str);
            let nie = nie || apres.is_some_and(|a| NEGATIONS_FR.contains(&a));
            if sujet && !nie {
                return memoire;
            }
        }
        if m == "memoire" && i >= 2 {
            let det = mots[i - 1].as_str();
            let prep = mots[i - 2].as_str();
            let nie = i >= 3 && NEGATIONS_FR.contains(&mots[i - 3].as_str());
            if matches!(det, "ma" | "mes")
                && matches!(prep, "d'apres" | "apres" | "selon" | "dans")
                && !nie
            {
                return memoire;
            }
        }
        // « De mémoire, tu préfères… » : seulement en tête de phrase.
        if m == "memoire" && i == 1 && mots[0] == "de" {
            return memoire;
        }
        // « Tu m'avais dit », « vous nous aviez indiqué » : le plus-que-parfait renvoie à une
        // conversation passée, que seul un `recall` peut rapporter.
        if matches!(m, "avais" | "aviez") && !nie_avant(mots, i) {
            let mut j = i;
            let mut sujet = false;
            while j > 0 {
                j -= 1;
                match mots[j].as_str() {
                    "tu" | "vous" => {
                        sujet = true;
                        break;
                    }
                    "m'" | "me" | "nous" | "l'" | "le" | "en" => {}
                    _ => break,
                }
            }
            let dit = mots[i + 1..]
                .iter()
                .map(String::as_str)
                .find(|m| !matches!(*m, "deja" | "bien"))
                .is_some_and(|m| {
                    matches!(
                        m,
                        "dit"
                            | "indique"
                            | "confie"
                            | "explique"
                            | "precise"
                            | "raconte"
                            | "parle"
                            | "ecrit"
                            | "mentionne"
                            | "demande"
                    )
                });
            if sujet && dit {
                return memoire;
            }
        }
        // « nos échanges précédents », « notre dernière conversation ».
        if matches!(m, "nos" | "notre") && !nie_avant(mots, i) {
            let suite: Vec<&str> = mots[i + 1..].iter().take(3).map(String::as_str).collect();
            let fil = suite.iter().any(|s| {
                ["echange", "conversation", "discussion", "session"]
                    .iter()
                    .any(|n| s.starts_with(n))
            });
            let passe = suite.iter().any(|s| {
                ["precedent", "passe", "anterieur", "dernier", "derniere"]
                    .iter()
                    .any(|q| s.starts_with(q))
            });
            if fil && passe {
                return memoire;
            }
        }
    }
    None
}

/// "I created", "I've added", "we have removed", "I did create it" ; pas "I haven't created",
/// pas "if I added".
fn action_en(mots: &[String], d: Decls) -> Option<Trouve> {
    for (i, mot) in mots.iter().enumerate() {
        if !matches!(mot.as_str(), "i" | "i'" | "we" | "we'") {
            continue;
        }
        if condition_avant(mots, i, CONDITIONS_EN) {
            continue;
        }
        let mut did = false;
        for suivant in &mots[i + 1..] {
            let m = suivant.as_str();
            if m == "did" {
                did = true;
                continue;
            }
            if ADVERBES_EN.contains(&m) {
                continue;
            }
            if did {
                if BASES_NOTE_EN.contains(&m) {
                    return Some(Trouve {
                        claim: Claim::Noted,
                        verbe: Some(m.to_string()),
                    });
                }
                if BASES_EN.contains(&m) {
                    return action(m);
                }
                if let Some(trouve) = declare(m, d) {
                    return Some(trouve);
                }
                break;
            }
            if let Some(trouve) = verbe_en(m, d) {
                return Some(trouve);
            }
            break;
        }
    }
    None
}

/// "Done.", "It's done", "all done", "that is done" ; pas "not done", pas "once done".
fn fait_en(mots: &[String]) -> Option<Trouve> {
    if mots.first().is_some_and(|m| m == "done") {
        return generique(Claim::Action);
    }
    for (i, mot) in mots.iter().enumerate() {
        if mot != "done" || i == 0 {
            continue;
        }
        let mut j = i;
        while j > 0 && matches!(mots[j - 1].as_str(), "all" | "now" | "already") {
            j -= 1;
        }
        let avant = &mots[..j];
        let tete = match avant {
            [.., a, b] if (a == "it'" || a == "that'") && b == "s" => Some(j - 2),
            [.., a, b] if (a == "it" || a == "that") && b == "is" => Some(j - 2),
            _ if j < i && mots[j] == "all" => Some(j),
            _ => None,
        };
        if let Some(t) = tete
            && !condition_avant(mots, t, CONDITIONS_EN)
        {
            return generique(Claim::Action);
        }
    }
    None
}

/// "The module has been created", "it's been added", "the rows were deleted", "the module is
/// now installed" ; pas "has not been created", pas "will have been created", pas "is
/// already installed" (un état), pas "is linked to" (une relation).
fn passif_en(mots: &[String], d: Decls) -> Option<Trouve> {
    for (i, mot) in mots.iter().enumerate() {
        let precedent = i.checked_sub(1).map(|p| mots[p].as_str());
        let etat = match mot.as_str() {
            "been" if matches!(precedent, Some("has" | "have" | "ve" | "s")) => false,
            "was" | "were" => false,
            "is" | "are" => true,
            "s" if precedent.is_some_and(|p| p.ends_with('\'')) => true,
            _ => continue,
        };
        if proposition_reservee(mots, i) {
            continue;
        }
        let mut k = i + 1;
        let mut deja = false;
        while k < mots.len()
            && matches!(
                mots[k].as_str(),
                "now"
                    | "just"
                    | "successfully"
                    | "also"
                    | "correctly"
                    | "properly"
                    | "finally"
                    | "already"
            )
        {
            deja |= mots[k] == "already";
            k += 1;
        }
        if deja {
            continue;
        }
        let Some(v) = mots.get(k) else { continue };
        let suite = mots.get(k + 1).map(String::as_str);
        if etat {
            let relation = matches!(v.as_str(), "linked" | "connected")
                && matches!(suite, Some("to" | "with"));
            if relation || suite == Some("by") {
                continue;
            }
        }
        if let Some(trouve) = verbe_en(v, d) {
            return Some(trouve);
        }
    }
    None
}

/// "I remember", "I recall", "from memory", et les renvois à une conversation passée : "as you
/// told me before", "our previous conversation" ; pas "I don't remember", pas "you never told
/// me before".
fn memoire_en(mots: &[String]) -> Option<Trouve> {
    let memoire = generique(Claim::Memory);
    for (i, mot) in mots.iter().enumerate() {
        let m = mot.as_str();
        if m == "i" {
            for suivant in &mots[i + 1..] {
                match suivant.as_str() {
                    "still" | "clearly" | "do" | "also" | "definitely" | "vaguely" => continue,
                    "remember" | "recall" => return memoire,
                    _ => break,
                }
            }
        }
        if m == "memory" && i >= 1 {
            let avant = mots[i - 1].as_str();
            if avant == "from" || (avant == "my" && (i < 2 || mots[i - 2] != "in")) {
                return memoire;
            }
        }
        if (m == "you" || m == "you'") && !nie_avant(mots, i) {
            let mut k = i + 1;
            while k < mots.len() && matches!(mots[k].as_str(), "had" | "d" | "already") {
                k += 1;
            }
            let dit = mots
                .get(k)
                .is_some_and(|v| matches!(v.as_str(), "told" | "mentioned" | "said"));
            let fin = mots[k.min(mots.len())..]
                .iter()
                .position(|m| m == ",")
                .map_or(mots.len(), |p| k + p);
            let passe = mots[k.min(fin)..fin]
                .iter()
                .any(|m| matches!(m.as_str(), "before" | "earlier" | "previously" | "last"));
            if dit && passe {
                return memoire;
            }
        }
        if m == "our" && !nie_avant(mots, i) {
            let suite: Vec<&str> = mots[i + 1..].iter().take(2).map(String::as_str).collect();
            let passe = suite
                .first()
                .is_some_and(|s| matches!(*s, "previous" | "past" | "earlier" | "last"));
            let fil = suite.get(1).is_some_and(|s| {
                ["conversation", "chat", "discussion", "session"]
                    .iter()
                    .any(|n| s.starts_with(n))
            });
            if passe && fil {
                return memoire;
            }
        }
    }
    None
}

// ---------------------------------------------------------------------------------------
// Du verbe à l'outil

/// Chaque action nommée par un récit : ses radicaux français (sans accents), ses préfixes
/// anglais, et les jetons d'un nom d'outil qui la font.
const ACTIONS: &[(&[&str], &[&str], &[&str])] = &[
    (&["install"], &["install"], &["install"]),
    (&["cre"], &["creat"], &["create"]),
    (&["ajout", "rajout"], &["add"], &["add"]),
    (
        &["supprim", "effac"],
        &["delet"],
        &["delete", "remove", "drop"],
    ),
    (&["retir"], &["remov"], &["remove", "delete"]),
    (
        &["reli", "li", "connect", "branch"],
        &["link", "connect"],
        &["link", "connect"],
    ),
    (
        &["modifi", "chang"],
        &["updat", "chang", "modif"],
        &["update", "edit", "modify", "set", "change"],
    ),
    (&["activ"], &["enabl", "activat"], &["enable", "activate"]),
    (
        &["desactiv"],
        &["disabl", "deactivat"],
        &["disable", "deactivate"],
    ),
    (&["renomm"], &["renam"], &["rename"]),
    (&["configur"], &["configur"], &["configure", "config"]),
    (&["deplac"], &["mov"], &["move"]),
    (&["remplac"], &["replac"], &["replace"]),
    (&["corrig"], &["fix"], &["fix"]),
    (&[], &["wrot", "writ"], &["write"]),
];

/// Les jetons d'outil de l'action que porte `verbe` (« installé » ⇒ `install`).
fn outils_de(verbe: &str) -> Option<&'static [&'static str]> {
    let ecrit = matches!(verbe, "ecrit" | "ecrite" | "ecrits");
    ACTIONS
        .iter()
        .find(|(fr, en, outils)| {
            participe(verbe, fr)
                || en.iter().any(|p| verbe.starts_with(p))
                || (ecrit && outils.contains(&"write"))
        })
        .map(|(_, _, outils)| *outils)
}

/// L'outil fait-il cette action ? `install_module`, `installModule`, `module-install` ⇒
/// `install`.
fn outil_fait(outil: &str, jetons: &[&str]) -> bool {
    let mut morceaux: Vec<String> = Vec::new();
    let mut courant = String::new();
    let mut precedent_minuscule = false;
    for c in outil.chars() {
        if (!c.is_alphanumeric() || (c.is_uppercase() && precedent_minuscule))
            && !courant.is_empty()
        {
            morceaux.push(std::mem::take(&mut courant));
        }
        precedent_minuscule = c.is_lowercase();
        if c.is_alphanumeric() {
            courant.extend(c.to_lowercase());
        }
    }
    if !courant.is_empty() {
        morceaux.push(courant);
    }
    morceaux.iter().any(|morceau| {
        jetons
            .iter()
            .any(|j| morceau == j || (j.len() >= 4 && morceau.starts_with(j)))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ecriture_ok() -> Step {
        Step::tool("row_create", 3).state("a", "b")
    }

    // ------------------------------------------------------------------ verdict

    #[test]
    fn verdict_sans_aucune_etape_est_non_verifie() {
        assert_eq!(verdict(&[], true), Verdict::Unverified);
    }

    #[test]
    fn verdict_ecriture_observee_et_run_termine_est_prouve() {
        assert_eq!(verdict(&[ecriture_ok()], true), Verdict::Proven);
    }

    #[test]
    fn verdict_run_non_termine_n_est_jamais_prouve() {
        assert_eq!(verdict(&[ecriture_ok()], false), Verdict::Unverified);
    }

    #[test]
    fn verdict_une_note_remember_n_est_pas_une_preuve() {
        let note = Step::tool("remember", 1).state("a", "b");
        assert_eq!(kind(&note), Kind::Note);
        assert_eq!(verdict(&[note], true), Verdict::Unverified);
    }

    #[test]
    fn verdict_lecture_seule_est_non_verifie() {
        let lu = Step::tool("graph_query", 1);
        assert_eq!(verdict(&[lu], true), Verdict::Unverified);
    }

    #[test]
    fn verdict_ecriture_sans_empreinte_n_est_pas_prouvee() {
        // L'outil dit « ok » sans observer l'état : c'est sa parole, pas un fait.
        assert_eq!(
            verdict(&[Step::tool("row_create", 1)], true),
            Verdict::Unverified
        );
    }

    #[test]
    fn verdict_ecriture_sans_effet_n_est_pas_prouvee() {
        let rien = Step::tool("row_create", 1).state("a", "a");
        assert_eq!(kind(&rien), Kind::NoEffect);
        assert_eq!(verdict(&[rien], true), Verdict::Unverified);
    }

    #[test]
    fn verdict_refus_non_rattrape_est_un_echec() {
        let refus = Step::tool("install_module", 1).refused("même version");
        assert_eq!(verdict(&[refus], true), Verdict::Failed);
    }

    #[test]
    fn verdict_refus_rattrape_par_une_ecriture_est_prouve() {
        let refus = Step::tool("install_module", 1).refused("même version");
        assert_eq!(verdict(&[refus, ecriture_ok()], true), Verdict::Proven);
    }

    #[test]
    fn un_refus_n_est_pas_rattrape_par_une_note_ni_une_lecture() {
        let etapes = [
            Step::tool("install_module", 1).refused("non"),
            Step::tool("remember", 1).state("a", "b"),
            Step::tool("recall", 1),
        ];
        assert_eq!(
            unresolved(&etapes).map(|e| e.tool.as_str()),
            Some("install_module")
        );
    }

    #[test]
    fn un_echec_est_rattrape_par_le_meme_outil_reussi() {
        let etapes = [
            Step::tool("graph_query", 1).failed("timeout"),
            Step::tool("graph_query", 1),
        ];
        assert!(unresolved(&etapes).is_none());
    }

    // ------------------------------------------------------------------ claim_check

    #[test]
    fn c_est_fait_sans_ecriture_rappelle() {
        let refus = Step::tool("install_module", 1).refused("même version");
        let rappel = claim_check("C'est fait !", &[refus]).expect("un rappel");
        assert!(rappel.starts_with(RAPPEL));
        assert!(rappel.contains("C'est fait"));
    }

    #[test]
    fn c_est_fait_apres_ecriture_reussie_ne_rappelle_pas() {
        assert_eq!(claim_check("C'est fait.", &[ecriture_ok()]), None);
    }

    #[test]
    fn j_ai_cree_sans_etape_rappelle() {
        for texte in [
            "J'ai créé la fiche client.",
            "J’ai bien ajouté le module.",
            "Je l'ai supprimée.",
            "J'ai relié les deux entités.",
            "J'ai installé conversation 1.2",
            "Nous avons mis à jour la table.",
        ] {
            assert!(
                claim_check(texte, &[]).is_some(),
                "aucun rappel pour {texte:?}"
            );
        }
    }

    #[test]
    fn j_ai_note_est_couvert_par_remember_mais_pas_j_ai_cree() {
        let note = [Step::tool("remember", 1)];
        assert_eq!(claim_check("J'ai noté ta préférence.", &note), None);
        assert_eq!(claim_check("C'est enregistré.", &note), None);
        assert!(claim_check("J'ai créé la fiche.", &note).is_some());
    }

    #[test]
    fn une_ecriture_refusee_ne_couvre_pas_l_affirmation() {
        let refus = [Step::tool("row_create", 1).refused("champ manquant")];
        assert!(claim_check("J'ai ajouté la ligne.", &refus).is_some());
        let echec = [Step::tool("row_create", 1).failed("réseau")];
        assert!(claim_check("J'ai ajouté la ligne.", &echec).is_some());
    }

    #[test]
    fn une_ecriture_sans_effet_ne_couvre_pas_l_affirmation() {
        let rien = [Step::tool("row_create", 1).state("x", "x")];
        assert!(claim_check("J'ai ajouté la ligne.", &rien).is_some());
    }

    #[test]
    fn anglais_action_sans_ecriture_rappelle() {
        for texte in [
            "I created the entity.",
            "I've added it.",
            "Done!",
            "It's done.",
            "All done.",
        ] {
            assert!(
                claim_check(texte, &[]).is_some(),
                "aucun rappel pour {texte:?}"
            );
        }
    }

    #[test]
    fn je_me_souviens_sans_recall_rappelle() {
        let ecrit = [ecriture_ok()];
        for texte in [
            "Je me souviens que tu préfères le thé.",
            "Je m'en souviens : c'était mardi.",
            "D'après ma mémoire, le module est déjà installé.",
            "Si je me souviens bien, tu habites Lyon.",
            "I remember you like tea.",
        ] {
            let rappel =
                claim_check(texte, &ecrit).unwrap_or_else(|| panic!("rien pour {texte:?}"));
            assert!(rappel.contains("recall"));
        }
    }

    #[test]
    fn je_me_souviens_apres_recall_ou_lecture_ne_rappelle_pas() {
        assert_eq!(
            claim_check(
                "Je me souviens que tu préfères le thé.",
                &[Step::tool("recall", 1)]
            ),
            None
        );
        assert_eq!(
            claim_check("I remember that.", &[Step::tool("graph_query", 1)]),
            None
        );
    }

    #[test]
    fn un_recall_en_echec_ne_couvre_pas_le_souvenir() {
        let rate = [Step::tool("recall", 1).failed("magasin fermé")];
        assert!(claim_check("Je me souviens de ça.", &rate).is_some());
    }

    #[test]
    fn negation_ne_rappelle_pas() {
        for texte in [
            "Je n'ai pas créé la fiche : l'outil a refusé.",
            "Je ne l'ai pas encore ajoutée.",
            "Je n'ai rien supprimé.",
            "Ce n'est pas fait.",
            "C'est pas fait, désolé.",
            "Je ne me souviens pas de ça.",
            "Je ne m'en souviens plus.",
            "Ce n'est pas dans ma mémoire.",
            "I haven't created it.",
            "I didn't add anything.",
            "I have not removed it.",
            "It's not done.",
            "I don't remember.",
        ] {
            assert_eq!(claim_check(texte, &[]), None, "faux positif sur {texte:?}");
        }
    }

    #[test]
    fn conditionnel_et_futur_ne_rappellent_pas() {
        for texte in [
            "Si j'ai créé un doublon, dis-le moi.",
            "Quand c'est fait, je te préviens.",
            "J'aurais créé la fiche si l'outil avait accepté.",
            "Je vais créer la fiche.",
            "Je peux ajouter le module.",
            "If I created a duplicate, tell me.",
            "Once done, I will tell you.",
            "I will add it.",
            "I'll remember that.",
        ] {
            assert_eq!(claim_check(texte, &[]), None, "faux positif sur {texte:?}");
        }
    }

    #[test]
    fn question_ne_rappelle_pas() {
        for texte in [
            "Est-ce que j'ai créé la fiche ?",
            "C'est fait ?",
            "Did I add it?",
            "Je me souviens de quoi ?",
        ] {
            assert_eq!(claim_check(texte, &[]), None, "faux positif sur {texte:?}");
        }
    }

    #[test]
    fn texte_neutre_ne_rappelle_pas() {
        for texte in [
            "Bonjour ! Que veux-tu faire ?",
            "Voici la liste des modules installés.",
            "C'est lié à ton autre fiche.",
            "",
        ] {
            assert_eq!(claim_check(texte, &[]), None, "faux positif sur {texte:?}");
        }
    }

    #[test]
    fn la_phrase_fautive_est_citee_dans_le_rappel() {
        let rappel =
            claim_check("Voilà. J'ai créé la fiche « Dupont ». Autre chose ?", &[]).unwrap();
        assert!(
            rappel.contains("« J'ai créé la fiche « Dupont » »"),
            "{rappel}"
        );
    }

    // ------------------------------------------------------------------ W10#2 rattrapage

    fn affirme(texte: &str) -> bool {
        claims(texte)
            .iter()
            .any(|(c, _)| matches!(c, Claim::Action | Claim::Noted))
    }

    fn ne_dit_rien(textes: &[&str]) {
        for texte in textes {
            assert!(
                claims(texte).is_empty(),
                "faux positif sur {texte:?} : {:?}",
                claims(texte)
            );
        }
    }

    fn dit_une_action(textes: &[&str]) {
        for texte in textes {
            assert!(affirme(texte), "manqué : {texte:?}");
        }
    }

    #[test]
    fn rattrapage_par_ecriture_sans_empreinte_ou_sans_effet_refuse() {
        let refus = Step::tool("install_module", 1).refused("non");
        for apres in [
            Step::tool("row_create", 1),
            Step::tool("row_create", 1).state("a", "a"),
        ] {
            let etapes = [refus.clone(), apres];
            assert_eq!(
                unresolved(&etapes).map(|e| e.tool.as_str()),
                Some("install_module")
            );
        }
    }

    #[test]
    fn rattrapage_par_le_meme_outil_ou_une_ecriture_observee() {
        let refus = Step::tool("install_module", 1).refused("non");
        // Le même outil réussi, même sans empreinte.
        assert!(unresolved(&[refus.clone(), Step::tool("install_module", 1)]).is_none());
        // Une écriture dont l'état observé a changé.
        assert!(unresolved(&[refus, ecriture_ok()]).is_none());
    }

    #[test]
    fn un_recit_qui_nomme_l_action_refusee_n_est_pas_couvert_par_une_autre_ecriture() {
        let etapes = [
            Step::tool("install_module", 1).refused("même version"),
            ecriture_ok(),
        ];
        for texte in [
            "J'ai installé le module stock.",
            "Le module a été installé.",
            "I installed the module.",
            "C'est fait.",
        ] {
            let u = unproven(texte, &etapes).unwrap_or_else(|| panic!("rien pour {texte:?}"));
            assert!(u.manque.contains("`install_module`"), "{}", u.manque);
            assert_eq!(u.etape.map(|e| e.tool), Some("install_module".into()));
        }
    }

    #[test]
    fn un_recit_qui_dit_l_autre_ecriture_est_couvert() {
        let etapes = [
            Step::tool("install_module", 1).refused("même version"),
            ecriture_ok(),
        ];
        assert_eq!(claim_check("J'ai ajouté la ligne.", &etapes), None);
        assert_eq!(claim_check("I added the row.", &etapes), None);
        // Le même outil repris : l'action nommée est prouvée.
        let repris = [
            Step::tool("install_module", 1).refused("même version"),
            Step::tool("install_module", 1).state("v1", "v2"),
        ];
        assert_eq!(claim_check("J'ai installé le module.", &repris), None);
    }

    #[test]
    fn une_ecriture_sans_empreinte_ne_couvre_le_recit_que_sans_refus_en_suspens() {
        let seule = [Step::tool("row_create", 1)];
        assert_eq!(claim_check("J'ai ajouté la ligne.", &seule), None);
        let apres_refus = [
            Step::tool("install_module", 1).refused("non"),
            Step::tool("row_create", 1),
        ];
        assert!(claim_check("J'ai ajouté la ligne.", &apres_refus).is_some());
    }

    #[test]
    fn outil_fait_lit_les_noms_d_outil() {
        let install = outils_de("installe").unwrap();
        assert!(outil_fait("install_module", install));
        assert!(outil_fait("installModule", install));
        assert!(outil_fait("module-install", install));
        assert!(!outil_fait("row_create", install));
        assert!(outil_fait("entity_create", outils_de("created").unwrap()));
        assert_eq!(outils_de("mis"), None);
    }

    // ------------------------------------------------------------------ W10#3 question finale

    #[test]
    fn affirmation_suivie_d_une_question_est_une_affirmation() {
        dit_une_action(&[
            "J'ai créé la fiche, veux-tu que je l'ouvre ?",
            "C'est fait : autre chose ?",
            "Done — anything else?",
            "I created the entity, do you want me to open it?",
            "J'ai ajouté la ligne ; tu veux la voir ?",
        ]);
    }

    #[test]
    fn une_vraie_question_reste_une_question() {
        ne_dit_rien(&[
            "Veux-tu que je crée la fiche ?",
            "Est-ce que j'ai créé la fiche, ou faut-il recommencer ?",
            "Did I add it, or should I retry?",
            "Tu veux que je vérifie si c'est fait ?",
            "Est-ce que, comme prévu, j'ai créé la fiche ?",
            "Le module a-t-il été créé ?",
            "Has the module been created?",
        ]);
    }

    // ------------------------------------------------------------------ W10#4 portée des conditions

    #[test]
    fn une_condition_ne_porte_que_sur_sa_proposition() {
        dit_une_action(&[
            "After checking the schema, I created the entity.",
            "Before answering, I added the row.",
            "Une fois de plus, j'ai créé la fiche.",
            "Si besoin, dis-le moi : j'ai créé la fiche.",
            "If needed, tell me: I created the row.",
        ]);
    }

    #[test]
    fn une_condition_dans_la_proposition_annule_toujours() {
        ne_dit_rien(&[
            "Si j'ai créé un doublon, dis-le moi.",
            "Une fois que j'ai créé le module, tu pourras l'utiliser.",
            "Dès que j'ai créé la fiche, je te préviens.",
            "If I created a duplicate, tell me.",
            "Once done, I will tell you.",
            "Quand c'est fait, je te préviens.",
            "S'il est installé, tu le verras.",
        ]);
    }

    // ------------------------------------------------------------------ W10#5 passif, sans sujet, NFD

    #[test]
    fn passif_sans_sujet_et_nfd_sont_des_affirmations() {
        dit_une_action(&[
            "Module créé avec succès.",
            "Le module est installé.",
            "Le module est maintenant installé.",
            "Le module a été créé.",
            "La fiche a bien été créée.",
            "Les deux fiches ont été reliées.",
            "Voilà, c'est en place.",
            "The module has been created.",
            "It's been added.",
            "The rows were deleted.",
            "The module is now installed.",
            "Successfully created the module.",
            "Created successfully.",
            "I did create it.",
            "I've gone ahead and created the module.",
            // NFD : accents combinants.
            "J'ai cre\u{301}e\u{301} la fiche.",
            "La fiche a e\u{301}te\u{301} cre\u{301}e\u{301}e.",
        ]);
    }

    #[test]
    fn passif_nie_futur_question_ou_cite_n_affirme_pas() {
        ne_dit_rien(&[
            "Le module n'a pas été créé.",
            "Le module n'est pas installé.",
            "Le module sera créé.",
            "Le module va être installé.",
            "Le module est déjà installé.",
            "Le module est installé par défaut.",
            "C'est lié à ton autre fiche.",
            "La fiche est liée à Dupont.",
            "Vérifie que le module est installé.",
            "Tu m'as dit que le module est installé.",
            "Tu m'as écrit : « le module a été créé ».",
            "Tu as écrit « j'ai créé la fiche ».",
            "Si le module a été créé, préviens-moi.",
            "The module has not been created.",
            "The module hasn't been created yet.",
            "The module will be created.",
            "It will have been created by then.",
            "The module is already installed.",
            "The entity is linked to the order.",
            "If the module has been created, tell me.",
            "I think that it has been created.",
            "I didn't successfully create it.",
            "Je n'ai pas cre\u{301}e\u{301} la fiche.",
            "Voici la liste des modules installés.",
        ]);
    }

    #[test]
    fn un_recit_entierement_entre_guillemets_reste_le_recit() {
        dit_une_action(&["« C'est fait. »", "\"I created the module.\""]);
    }

    // ------------------------------------------------------------------ W12 MOY 7 variantes

    #[test]
    fn sans_apostrophe_ni_accents() {
        dit_une_action(&[
            "cest fait",
            "Cest fait !",
            "c est fait",
            "j'ai cree le module",
            "J'ai ajoute la ligne.",
            "j'ai supprime la fiche",
            "le module a ete installe",
            "J'AI CRÉÉ LE MODULE.",
        ]);
        ne_dit_rien(&[
            "c est pas fait",
            "cest pas encore fait",
            "je n'ai pas cree le module",
            "le module n'a pas ete cree",
            "je vais creer le module",
            "cest fait ?",
        ]);
    }

    fn memoire(texte: &str) -> bool {
        claims(texte).iter().any(|(c, _)| *c == Claim::Memory)
    }

    #[test]
    fn paraphrases_de_memoire() {
        for texte in [
            "D'après nos échanges précédents, tu préfères le thé vert.",
            "Tu m'avais dit que tu préfères le thé vert.",
            "Vous m'aviez indiqué une préférence pour le thé.",
            "Lors de notre dernière conversation, tu as choisi le thé.",
            "As you told me before, you prefer green tea.",
            "You mentioned earlier that you like tea.",
            "In our previous conversation you chose tea.",
        ] {
            assert!(memoire(texte), "manqué : {texte:?}");
        }
        for texte in [
            "Tu m'as dit que tu préfères le thé vert.",
            "Tu ne m'avais pas dit ça.",
            "Tu m'avais dit ça ?",
            "Je ne trouve rien dans nos échanges précédents.",
            "You never told me before.",
            "As you told me, you prefer tea.",
            "Tu as écrit « tu m'avais dit que j'aimais le thé ».",
        ] {
            assert!(
                !memoire(texte),
                "faux positif : {texte:?} : {:?}",
                claims(texte)
            );
        }
    }

    #[test]
    fn une_paraphrase_de_memoire_sans_lecture_rappelle_et_avec_lecture_non() {
        let texte = "Tu m'avais dit que tu préfères le thé.";
        assert!(
            claim_check(texte, &[ecriture_ok()])
                .unwrap()
                .contains("recall")
        );
        assert_eq!(claim_check(texte, &[Step::tool("recall", 1)]), None);
    }

    // ------------------------------------------------------------ déclarations

    /// L'hôte qui déclare `deploy_app` (mots : déployer, deploy, application).
    fn deploiement() -> Vec<Declaration> {
        vec![
            Declaration::writes("deploy_app").words(["Déployer", "deploy", "application"]),
            Declaration::reads("inspect_app"),
        ]
    }

    #[test]
    fn declaration_un_outil_declare_se_comporte_comme_install_module() {
        let d = deploiement();
        let etapes = [
            Step::tool("deploy_app", 1).refused("quota atteint"),
            ecriture_ok(),
        ];
        for texte in [
            "J'ai déployé l'application.",
            "L'application a été déployée.",
            "C'est déployé.",
            "I deployed the app.",
            "I did deploy it.",
            "C'est fait.",
        ] {
            let u =
                unproven_with(texte, &etapes, &d).unwrap_or_else(|| panic!("rien pour {texte:?}"));
            assert!(
                u.manque.contains("`deploy_app`"),
                "{texte:?} : {}",
                u.manque
            );
            assert_eq!(u.etape.map(|e| e.tool), Some("deploy_app".into()));
            let rappel = claim_check_with(texte, &etapes, &d).unwrap();
            assert!(rappel.starts_with(RAPPEL) && rappel.contains("quota atteint"));
        }
        // Une autre écriture reste couverte, et le même outil repris prouve l'action.
        assert_eq!(claim_check_with("J'ai ajouté la ligne.", &etapes, &d), None);
        let repris = [
            Step::tool("deploy_app", 1).refused("quota atteint"),
            Step::tool("deploy_app", 1).state("v1", "v2"),
        ];
        assert_eq!(
            claim_check_with("J'ai déployé l'application.", &repris, &d),
            None
        );
    }

    #[test]
    fn declaration_absente_le_verbe_inconnu_n_est_pas_controle() {
        // Le trou constaté : sans déclaration, « déployé » n'est pas un verbe d'action, et
        // `deploy_app` refusé ne contredit rien.
        let etapes = [
            Step::tool("deploy_app", 1).refused("quota atteint"),
            ecriture_ok(),
        ];
        assert!(claims("J'ai déployé l'application.").is_empty());
        assert_eq!(claim_check("J'ai déployé l'application.", &etapes), None);
        // Et des déclarations vides valent l'absence de déclaration.
        assert_eq!(
            claim_check_with("J'ai installé le module.", &etapes, &[]),
            claim_check("J'ai installé le module.", &etapes)
        );
    }

    #[test]
    fn declaration_un_objet_declare_designe_l_outil_quand_le_verbe_ne_le_fait_pas() {
        let d = deploiement();
        let etapes = [
            Step::tool("deploy_app", 1).refused("quota atteint"),
            ecriture_ok(),
        ];
        // « mis » ne désigne aucun outil : l'objet « application » désigne `deploy_app`.
        let u = unproven_with("J'ai mis l'application en ligne.", &etapes, &d).unwrap();
        assert_eq!(u.etape.map(|e| e.tool), Some("deploy_app".into()));
        // Un verbe qui désigne un autre outil l'emporte sur l'objet.
        assert_eq!(
            claim_check_with("J'ai ajouté une ligne pour l'application.", &etapes, &d),
            None
        );
    }

    #[test]
    fn declaration_l_effet_declare_remplace_celui_que_le_nom_laisse_deviner() {
        let d = deploiement();
        // `inspect_app` n'a pas un nom de lecture : sans déclaration, c'est une écriture.
        let lecture = Step::tool("inspect_app", 1).state("a", "b");
        assert_eq!(kind(&lecture), Kind::Write);
        assert_eq!(kind_with(&lecture, &d), Kind::Read);
        assert!(is_proof(&lecture) && !is_proof_with(&lecture, &d));
        assert_eq!(
            verdict(std::slice::from_ref(&lecture), true),
            Verdict::Proven
        );
        assert_eq!(
            verdict_with(std::slice::from_ref(&lecture), true, &d),
            Verdict::Unverified
        );
        // Une lecture déclarée prouve la mémoire, pas l'action.
        assert_eq!(
            claim_check_with(
                "Je me souviens de ton choix.",
                std::slice::from_ref(&lecture),
                &d
            ),
            None
        );
        assert!(
            claim_check_with("J'ai créé la fiche.", std::slice::from_ref(&lecture), &d).is_some()
        );
        // Elle ne rattrape pas un refus.
        let refus = Step::tool("deploy_app", 1).refused("quota atteint");
        assert!(unresolved(&[refus.clone(), lecture.clone()]).is_none());
        assert_eq!(
            unresolved_with(&[refus.clone(), lecture], &d).map(|e| e.tool.clone()),
            Some("deploy_app".into())
        );
        // Une écriture déclarée dont le nom ressemble à une lecture (`get_` …) prouve.
        let d = [Declaration::writes("get_or_create")];
        let ecrit = Step::tool("get_or_create", 1).state("0", "1");
        assert_eq!(kind(&ecrit), Kind::Read);
        assert_eq!(kind_with(&ecrit, &d), Kind::Write);
        // Une note déclarée couvre « j'ai noté » et n'est pas une preuve.
        let d = [Declaration::notes("jot").words(["consigner"])];
        let note = Step::tool("jot", 1).state("0", "1");
        assert_eq!(kind_with(&note, &d), Kind::Note);
        assert_eq!(
            claim_check_with("J'ai noté ta préférence.", std::slice::from_ref(&note), &d),
            None
        );
        assert_eq!(verdict_with(&[note], true, &d), Verdict::Unverified);
    }

    #[test]
    fn declaration_les_mots_se_lisent_sans_accents_ni_infinitif() {
        let d = deploiement();
        for texte in [
            "J'ai déployé.",
            "J'ai DÉPLOYÉ l'app.",
            "Nous avons déployées les deux.",
            "I've deployed it.",
            "Successfully deployed.",
        ] {
            assert_eq!(
                claims_with(texte, &d).first().map(|(c, _)| *c),
                Some(Claim::Action),
                "{texte:?}"
            );
        }
        // Ni un futur, ni une négation, ni une question.
        for texte in [
            "Je vais déployer l'application.",
            "Je n'ai pas déployé l'application.",
            "Veux-tu que je déploie ?",
        ] {
            assert!(claims_with(texte, &d).is_empty(), "{texte:?}");
        }
    }

    #[test]
    fn declaration_se_serialise_en_snake_case() {
        let d = Declaration::writes("deploy_app").words(["deploy"]);
        let json = serde_json::to_string(&d).unwrap();
        assert_eq!(
            json,
            r#"{"tool":"deploy_app","effect":"write","words":["deploy"]}"#
        );
        assert_eq!(serde_json::from_str::<Declaration>(&json).unwrap(), d);
        let nu: Declaration = serde_json::from_str(r#"{"tool":"recall","effect":"read"}"#).unwrap();
        assert!(nu.words.is_empty());
    }
}
