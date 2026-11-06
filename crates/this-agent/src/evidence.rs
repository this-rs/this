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
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// Le run est terminé et au moins une écriture réussie a changé l'état observé.
    Proven,
    /// Rien ne contredit le run, mais rien ne le prouve : ce n'est pas un échec, c'est
    /// « pas d'avis ».
    Unverified,
    /// Un refus ou un échec n'a été suivi d'aucune reprise réussie.
    Failed,
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
    let tool = nom(step);
    if est_note(&tool) {
        Kind::Note
    } else if est_lecture(&tool) {
        Kind::Read
    } else if !step.before.is_empty() && !step.a_change() {
        Kind::NoEffect
    } else {
        Kind::Write
    }
}

/// Une étape est-elle une preuve au sens de PO : une écriture réussie dont l'état a bougé.
pub fn is_proof(step: &Step) -> bool {
    outcome(step) == Outcome::Ok && kind(step) == Kind::Write && step.a_change()
}

/// Le dernier refus ou échec que rien n'a rattrapé ensuite : ni le même outil réussi, ni une
/// autre écriture réussie. Une note (`remember`) ne rattrape rien.
///
/// C'est la mémoire du **run**, pas du tour : un refus au tour N reste vrai au tour N+1 tant
/// que personne ne l'a corrigé.
pub fn unresolved(steps: &[Step]) -> Option<&Step> {
    steps.iter().enumerate().rev().find_map(|(i, etape)| {
        if outcome(etape) == Outcome::Ok {
            return None;
        }
        let rattrape = steps[i + 1..].iter().any(|apres| {
            outcome(apres) == Outcome::Ok
                && (apres.tool == etape.tool || kind(apres) == Kind::Write)
        });
        (!rattrape).then_some(etape)
    })
}

/// Le verdict d'un run, d'après ses étapes. `completed` : le run s'est arrêté sur `Done`.
pub fn verdict(steps: &[Step], completed: bool) -> Verdict {
    if unresolved(steps).is_some() {
        return Verdict::Failed;
    }
    if completed && steps.iter().any(is_proof) {
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

/// Les affirmations du texte, chacune avec la phrase d'origine.
pub fn claims(said: &str) -> Vec<(Claim, String)> {
    let mut trouvees = Vec::new();
    for (phrase, question) in phrases(said) {
        if question {
            continue;
        }
        let mots = mots(&phrase.to_lowercase());
        if let Some(claim) = claim_of(&mots) {
            trouvees.push((claim, phrase.trim().to_string()));
        }
    }
    trouvees
}

/// Confronte le récit aux étapes. Rend le texte d'un rappel si une affirmation n'a aucune
/// étape **réussie** correspondante, `None` sinon.
pub fn claim_check(said: &str, steps: &[Step]) -> Option<String> {
    let ok = |accepte: &dyn Fn(Kind) -> bool| {
        steps
            .iter()
            .any(|etape| outcome(etape) == Outcome::Ok && accepte(kind(etape)))
    };
    let (claim, phrase) = claims(said).into_iter().find(|(claim, _)| match claim {
        Claim::Action => !ok(&|k| k == Kind::Write),
        Claim::Noted => !ok(&|k| k == Kind::Write || k == Kind::Note),
        Claim::Memory => !ok(&|k| k == Kind::Read),
    })?;
    let manque = match claim {
        Claim::Action | Claim::Noted => {
            "aucune écriture réussie dans ce run ne le prouve (un outil refusé, en échec ou sans effet ne compte pas)"
        }
        Claim::Memory => {
            "aucun `recall` ni aucune lecture du graphe réussie dans ce run ne le prouve"
        }
    };
    Some(format!(
        "{RAPPEL} Ta réponse affirme « {phrase} », mais {manque}. Soit tu appelles maintenant l'outil qui le fait réellement, soit tu corriges ta phrase pour dire ce qui a vraiment été fait et ce qui reste."
    ))
}

// ---------------------------------------------------------------------------------------
// Découpage

/// Les phrases du texte, avec « est-ce une question ? ».
fn phrases(texte: &str) -> Vec<(String, bool)> {
    let mut sortie = Vec::new();
    let mut courante = String::new();
    for c in texte.chars() {
        match c {
            '.' | '!' | '?' | '\n' | ';' | '…' => {
                if !courante.trim().is_empty() {
                    sortie.push((std::mem::take(&mut courante), c == '?'));
                } else {
                    courante.clear();
                    // « Fait ?! » : le point d'interrogation suit une phrase déjà coupée.
                    if c == '?'
                        && let Some(derniere) = sortie.last_mut()
                    {
                        derniere.1 = true;
                    }
                }
            }
            _ => courante.push(c),
        }
    }
    if !courante.trim().is_empty() {
        sortie.push((courante, false));
    }
    sortie
}

/// Les mots d'une phrase en minuscules ; l'élision reste collée au mot qu'elle précède
/// (`j'ai` ⇒ `j'`, `ai`).
fn mots(phrase: &str) -> Vec<String> {
    let mut sortie = Vec::new();
    let mut mot = String::new();
    for c in phrase.chars() {
        let c = if c == '’' || c == '`' || c == 'ʼ' {
            '\''
        } else {
            c
        };
        if c == '\'' {
            if !mot.is_empty() {
                mot.push('\'');
                sortie.push(std::mem::take(&mut mot));
            }
        } else if c.is_alphanumeric() {
            mot.push(c);
        } else if !mot.is_empty() {
            sortie.push(std::mem::take(&mut mot));
        }
    }
    if !mot.is_empty() {
        sortie.push(mot);
    }
    sortie
}

// ---------------------------------------------------------------------------------------
// Motifs

const ADVERBES_FR: &[&str] = &[
    "bien",
    "déjà",
    "aussi",
    "également",
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
];
const NEGATIONS_FR: &[&str] = &["pas", "jamais", "rien", "point", "plus", "guère", "ni"];
const CONDITIONS_FR: &[&str] = &["si", "quand", "lorsque", "lorsqu'", "dès", "fois"];
const PRONOMS_FR: &[&str] = &[
    "l'", "les", "le", "la", "lui", "leur", "y", "en", "vous", "te", "t'", "m'", "me", "nous",
];

const PARTICIPES_FR: &[&str] = &[
    "cré",
    "ajout",
    "rajout",
    "install",
    "supprim",
    "reli",
    "li",
    "modifi",
    "retir",
    "activ",
    "désactiv",
    "effac",
    "renomm",
    "corrig",
    "connect",
    "branch",
    "configur",
    "déplac",
    "chang",
    "remplac",
];
const PARTICIPES_NOTE_FR: &[&str] = &["not", "enregistr", "mémoris", "sauvegard", "consign"];
const IRREGULIERS_FR: &[&str] = &["mis", "mise", "mises", "écrit", "écrite", "écrits"];
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
    "ahead",
    "and",
    "finally",
];
const CONDITIONS_EN: &[&str] = &[
    "if", "once", "when", "whether", "unless", "after", "until", "before",
];
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

/// `cré` + `é`, `ée`, `és`, `ées`.
fn participe(mot: &str, radicaux: &[&str]) -> bool {
    let racine = mot
        .strip_suffix("ées")
        .or_else(|| mot.strip_suffix("és"))
        .or_else(|| mot.strip_suffix("ée"))
        .or_else(|| mot.strip_suffix('é'));
    racine.is_some_and(|r| radicaux.contains(&r))
}

fn condition_avant(mots: &[String], i: usize, conditions: &[&str]) -> bool {
    mots[..i].iter().any(|m| conditions.contains(&m.as_str()))
}

fn claim_of(mots: &[String]) -> Option<Claim> {
    action_fr(mots)
        .or_else(|| fait_fr(mots))
        .or_else(|| memoire_fr(mots))
        .or_else(|| action_en(mots))
        .or_else(|| fait_en(mots))
        .or_else(|| memoire_en(mots))
}

/// « j'ai créé », « je l'ai ajoutée », « nous avons relié » ; pas « je n'ai pas créé », pas
/// « si j'ai créé ».
fn action_fr(mots: &[String]) -> Option<Claim> {
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
            if participe(m, PARTICIPES_NOTE_FR) || IRREGULIERS_NOTE_FR.contains(&m) {
                return Some(Claim::Noted);
            }
            if participe(m, PARTICIPES_FR) || IRREGULIERS_FR.contains(&m) {
                return Some(Claim::Action);
            }
            break;
        }
    }
    None
}

/// « c'est fait », « c'est noté », « voilà qui est fait », « c'est chose faite ».
fn fait_fr(mots: &[String]) -> Option<Claim> {
    for (i, mot) in mots.iter().enumerate() {
        if mot != "est" || i == 0 {
            continue;
        }
        let avant = mots[i - 1].as_str();
        if avant != "c'" && avant != "qui" {
            continue;
        }
        if condition_avant(mots, i - 1, CONDITIONS_FR) {
            continue;
        }
        for suivant in &mots[i + 1..] {
            let m = suivant.as_str();
            if ADVERBES_FR.contains(&m) || m == "chose" {
                continue;
            }
            return match m {
                "fait" | "faite" | "installé" | "installée" | "créé" | "créée" | "ajouté"
                | "ajoutée" | "supprimé" | "supprimée" => Some(Claim::Action),
                "noté" | "notée" | "enregistré" | "enregistrée" | "mémorisé" | "retenu" => {
                    Some(Claim::Noted)
                }
                _ => None,
            };
        }
    }
    None
}

/// « je me souviens », « je m'en souviens », « je me rappelle », « d'après ma mémoire » ;
/// pas « je ne me souviens pas ». Un « si je me souviens bien » reste une affirmation.
fn memoire_fr(mots: &[String]) -> Option<Claim> {
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
                return Some(Claim::Memory);
            }
        }
        if m == "mémoire" && i >= 2 {
            let det = mots[i - 1].as_str();
            let prep = mots[i - 2].as_str();
            let nie = i >= 3 && NEGATIONS_FR.contains(&mots[i - 3].as_str());
            if matches!(det, "ma" | "mes")
                && matches!(prep, "d'après" | "après" | "selon" | "dans")
                && !nie
            {
                return Some(Claim::Memory);
            }
        }
        // « De mémoire, tu préfères… » : seulement en tête de phrase.
        if m == "mémoire" && i == 1 && mots[0] == "de" {
            return Some(Claim::Memory);
        }
    }
    None
}

/// "I created", "I've added", "we have removed" ; pas "I haven't created", pas "if I added".
fn action_en(mots: &[String]) -> Option<Claim> {
    for (i, mot) in mots.iter().enumerate() {
        if !matches!(mot.as_str(), "i" | "i'" | "we" | "we'") {
            continue;
        }
        if condition_avant(mots, i, CONDITIONS_EN) {
            continue;
        }
        for suivant in &mots[i + 1..] {
            let m = suivant.as_str();
            if ADVERBES_EN.contains(&m) {
                continue;
            }
            if VERBES_NOTE_EN.contains(&m) {
                return Some(Claim::Noted);
            }
            if VERBES_EN.contains(&m) {
                return Some(Claim::Action);
            }
            break;
        }
    }
    None
}

/// "Done.", "It's done", "all done", "that is done" ; pas "not done", pas "once done".
fn fait_en(mots: &[String]) -> Option<Claim> {
    if mots.first().is_some_and(|m| m == "done") {
        return Some(Claim::Action);
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
            return Some(Claim::Action);
        }
    }
    None
}

/// "I remember", "I recall", "from memory" ; pas "I don't remember".
fn memoire_en(mots: &[String]) -> Option<Claim> {
    for (i, mot) in mots.iter().enumerate() {
        let m = mot.as_str();
        if m == "i" {
            for suivant in &mots[i + 1..] {
                match suivant.as_str() {
                    "still" | "clearly" | "do" | "also" | "definitely" | "vaguely" => continue,
                    "remember" | "recall" => return Some(Claim::Memory),
                    _ => break,
                }
            }
        }
        if m == "memory" && i >= 1 {
            let avant = mots[i - 1].as_str();
            if avant == "from" || (avant == "my" && (i < 2 || mots[i - 2] != "in")) {
                return Some(Claim::Memory);
            }
        }
    }
    None
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
}
