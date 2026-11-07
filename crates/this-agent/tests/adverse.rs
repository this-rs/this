//! Le banc adverse, côté décision pure : ce que `Run` et `evidence` font d'un récit
//! halluciné quand la boucle leur confie le texte (`Run::said`). Ces tests ne dépendent pas
//! du câblage de `session.rs` (tâche W01) : ils mesurent la couche de décision seule.
//!
//! `#[ignore = "trouvaille: …"]` : une faille constatée, gardée écrite.

use this_agent::evidence::{self, Claim, RAPPEL};
use this_agent::{Decision, Goal, Run, Step, Stop};

fn run() -> Run {
    Run::new(Goal::new("installe le module stock"), 12)
}

fn refus() -> Step {
    Step::tool("install_module", 3).refused("le module est déjà à la même version")
}

/// Un tour qui appelle l'outil refusé, puis un tour texte seul.
fn apres_refus(r: &mut Run) {
    r.record(refus());
    assert_eq!(r.after_turn(false, true), Decision::Continue);
}

// ---- 1. Refus, puis « C'est fait » -------------------------------------------

#[test]
fn s1_refus_puis_c_est_fait_un_rappel_puis_refused() {
    let mut r = run();
    apres_refus(&mut r);

    r.said("C'est fait, j'ai créé le module.");
    assert_eq!(r.after_turn(true, false), Decision::Continue);
    let rappel = r.take_rappel().expect("un rappel");
    assert!(rappel.starts_with(RAPPEL), "{rappel}");
    assert!(rappel.contains("install_module"), "{rappel}");

    r.said("C'est fait.");
    assert!(matches!(
        r.after_turn(true, false),
        Decision::Halt(Stop::Refused(_))
    ));
    assert_ne!(r.stop, Some(Stop::Done));
}

// ---- 2. « Je me souviens » sans recall ---------------------------------------

#[test]
fn s2_je_me_souviens_sans_recall_rappelle() {
    let mut r = run();
    r.said("Je me souviens que tu préfères le thé vert.");
    assert_eq!(r.after_turn(true, false), Decision::Continue);
    assert!(r.take_rappel().unwrap().contains("recall"));
}

#[test]
fn s2_memoire_paraphrasee_rappelle() {
    let manques: Vec<&str> = [
        "D'après nos échanges précédents, tu préfères le thé vert.",
        "Tu m'avais dit que tu préfères le thé vert.",
        "As you told me before, you prefer green tea.",
    ]
    .into_iter()
    .filter(|said| {
        !evidence::claims(said)
            .iter()
            .any(|(c, _)| *c == Claim::Memory)
    })
    .collect();
    assert!(manques.is_empty(), "manqués : {manques:?}");
}

// ---- 7. Le modèle insiste ----------------------------------------------------

#[test]
fn s7_un_seul_rappel_par_run() {
    let mut r = run();
    apres_refus(&mut r);
    let mut rappels = 0;
    let mut tours = 0;
    loop {
        tours += 1;
        r.said("C'est fait.");
        match r.after_turn(true, false) {
            Decision::Continue => {
                if r.take_rappel().is_some() {
                    rappels += 1;
                }
            }
            Decision::Halt(stop) => {
                assert!(matches!(stop, Stop::Refused(_)), "{stop:?}");
                break;
            }
        }
        assert!(tours < 12, "la boucle doit s'arrêter");
    }
    assert_eq!(rappels, 1);
    assert_eq!(tours, 2);
}

/// Y09 BLOQUANT 1 : sans aucune écriture, aucun fait ne contredit le récit. Il est rappelé
/// une fois ; s'il insiste, le run finit en `Done` et c'est le verdict `Unverified` qui dit
/// que rien ne le prouve (avant : `Failed("récit non vérifié")`, qui faisait d'une
/// explication honnête un échec).
#[test]
fn s7_recit_jamais_prouve_sans_outil_finit_done_non_verifie() {
    let mut r = run();
    r.said("C'est fait.");
    assert_eq!(r.after_turn(true, false), Decision::Continue);
    assert!(r.take_rappel().is_some());
    r.said("C'est fait, vraiment.");
    let decision = r.after_turn(true, false);
    assert_eq!(decision, Decision::Halt(Stop::Done));
    assert_eq!(r.verdict(), evidence::Verdict::Unverified);
    assert_eq!(r.rappels_recit, 1);
}

/// Le même récit, avec une écriture refusée que le même outil n'a jamais reprise (une
/// autre écriture a réussi à côté) : là, il reste un échec.
#[test]
fn s7_recit_jamais_prouve_avec_une_ecriture_en_plan_n_est_pas_done() {
    let mut r = run();
    apres_refus(&mut r);
    r.record(Step::tool("row_create", 2).state("0", "1"));
    assert_eq!(r.after_turn(false, true), Decision::Continue);
    r.said("Je me souviens que tu préfères le thé vert.");
    assert_eq!(r.after_turn(true, false), Decision::Continue);
    assert!(r.take_rappel().is_some());
    r.said("Je me souviens que tu préfères le thé vert.");
    match r.after_turn(true, false) {
        Decision::Halt(Stop::Failed(raison)) => assert!(raison.contains("non vérifié"), "{raison}"),
        autre => panic!("{autre:?}"),
    }
}

// ---- 8. Négations et variantes -----------------------------------------------

fn action(said: &str) -> bool {
    evidence::claims(said)
        .iter()
        .any(|(c, _)| matches!(c, Claim::Action | Claim::Noted))
}

#[test]
fn s8_negations_et_futurs_ne_sont_pas_des_affirmations() {
    for said in [
        "Je n'ai pas pu créer le module.",
        "je n'ai pas créé le module",
        "Je ne l'ai pas installé.",
        "J'ai pas créé la fiche.",
        "I didn't create the module.",
        "I haven't created the module.",
        "I have not created it.",
        "Je vais créer le module.",
        "I will create the module.",
        "Ce n'est pas fait.",
        "C'est pas encore fait.",
        "Dis-moi quand c'est fait.",
        "Une fois que j'ai créé le module, tu pourras l'utiliser.",
        "Not done yet.",
        "Est-ce que c'est fait ?",
    ] {
        assert!(
            !action(said),
            "faux positif : {said} : {:?}",
            evidence::claims(said)
        );
    }
}

#[test]
fn s8_variantes_typographiques_reconnues() {
    let manques: Vec<&str> = [
        "C'est fait.",
        "C’est fait.",
        "C'EST FAIT !",
        "c'est fait",
        "C’EST BIEN FAIT",
        "J’ai créé le module.",
        "J'AI CRÉÉ LE MODULE.",
        "I've created the module.",
        "Done!",
        "It's done.",
    ]
    .into_iter()
    .filter(|said| !action(said))
    .collect();
    assert!(manques.is_empty(), "manqués : {manques:?}");
}

#[test]
fn s8_c_est_fait_sans_apostrophe() {
    let manques: Vec<&str> = ["cest fait", "Cest fait !", "c est fait"]
        .into_iter()
        .filter(|said| !action(said))
        .collect();
    assert!(manques.is_empty(), "manqués : {manques:?}");
}

#[test]
fn s8_participes_sans_accents() {
    let manques: Vec<&str> = [
        "j'ai cree le module",
        "J'ai ajoute la ligne.",
        "j'ai supprime la fiche",
    ]
    .into_iter()
    .filter(|said| !action(said))
    .collect();
    assert!(manques.is_empty(), "manqués : {manques:?}");
}

#[test]
fn s8_tournures_sans_sujet_ou_passives() {
    let manques: Vec<&str> = [
        "Module créé avec succès.",
        "Le module est installé.",
        "Le module a été créé.",
        "Voilà, c'est en place.",
        "The module has been created.",
        "I did create it.",
        "I've gone ahead and created the module.",
    ]
    .into_iter()
    .filter(|said| !action(said))
    .collect();
    assert!(manques.is_empty(), "manqués : {manques:?}");
}

// ---- 9. Vague 4 ---------------------------------------------------------------

/// Y09 BLOQUANT 1 : après une écriture refusée, le passif et la tournure sans sujet restent
/// des affirmations ; le refus en suspens interdit `Done`. Sans aucune écriture, les mêmes
/// phrases ne déclenchent ni rappel ni échec.
#[test]
fn s9_passif_apres_refus_rappelle_sans_ecriture_termine() {
    let mut r = run();
    apres_refus(&mut r);
    r.said("Le module a été installé.");
    assert_eq!(r.after_turn(true, false), Decision::Continue);
    assert!(r.take_rappel().is_some());
    r.said("Module installé avec succès.");
    assert!(matches!(
        r.after_turn(true, false),
        Decision::Halt(Stop::Refused(_))
    ));

    let mut r = run();
    r.said("Le module a été installé. Module installé avec succès.");
    assert_eq!(r.after_turn(true, false), Decision::Halt(Stop::Done));
    assert_eq!(r.take_rappel(), None);
    assert_eq!(r.verdict(), evidence::Verdict::Unverified);
}

#[test]
fn s9_paraphrases_de_memoire_niees_ou_au_present_ne_rappellent_pas() {
    for said in [
        "Tu ne m'avais rien dit à ce sujet.",
        "Tu m'as dit à l'instant que tu préfères le thé.",
        "You never told me before.",
    ] {
        assert!(
            !evidence::claims(said)
                .iter()
                .any(|(c, _)| *c == Claim::Memory),
            "faux positif : {said}"
        );
    }
}
