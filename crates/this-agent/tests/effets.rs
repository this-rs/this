//! La règle d'évidence, côté [`Effect`] : seul [`Effect::Write`] prouve qu'une écriture a
//! modifié l'état. [`Effect::Exec`] et [`Effect::Network`] agissent hors de nos empreintes :
//! un binaire lancé ou un « 200 » HTTP ne blanchit jamais un récit « c'est fait ».

use this_agent::Step;
use this_agent::evidence::{self, Declaration, Effect, Kind, Verdict, kind_with};

/// Un outil réseau déclaré qui répond « ok » et dont l'empreinte a même bougé.
fn reseau_ok() -> (Vec<Declaration>, Step) {
    let d = vec![Declaration::networks("web_search").words(["chercher", "search"])];
    let step = Step::tool("web_search", 1).state("a", "b");
    (d, step)
}

/// Un outil d'exécution déclaré qui répond « ok » et dont l'empreinte a même bougé.
fn exec_ok() -> (Vec<Declaration>, Step) {
    let d = vec![Declaration::execs("run_script")];
    let step = Step::tool("run_script", 1).state("a", "b");
    (d, step)
}

// ------------------------------------------------------------------ genre / kind

#[test]
fn chaque_effet_a_son_genre_et_jamais_une_ecriture_par_defaut() {
    let d = [
        Declaration::writes("ecrire"),
        Declaration::reads("lire"),
        Declaration::notes("noter"),
        Declaration::execs("executer"),
        Declaration::networks("sortir"),
    ];
    let pas = |outil: &str| Step::tool(outil, 1).state("a", "b");
    assert_eq!(kind_with(&pas("ecrire"), &d), Kind::Write);
    assert_eq!(kind_with(&pas("lire"), &d), Kind::Read);
    assert_eq!(kind_with(&pas("noter"), &d), Kind::Note);
    assert_eq!(kind_with(&pas("executer"), &d), Kind::Exec);
    assert_eq!(kind_with(&pas("sortir"), &d), Kind::Network);
}

#[test]
fn exec_et_network_ne_sont_pas_des_ecritures_meme_sans_empreinte() {
    // Même sans empreinte (before vide), Exec et Network restent hors de la preuve :
    // ils ne retombent pas sur l'écriture par défaut.
    let d = [
        Declaration::execs("run_script"),
        Declaration::networks("web_search"),
    ];
    let exec = Step::tool("run_script", 1);
    let net = Step::tool("web_search", 1);
    assert_eq!(kind_with(&exec, &d), Kind::Exec);
    assert_eq!(kind_with(&net, &d), Kind::Network);
}

// ------------------------------------------------------------------ la règle d'évidence

#[test]
fn un_outil_reseau_qui_repond_200_ne_prouve_pas_c_est_fait() {
    let (d, net) = reseau_ok();
    assert!(!evidence::is_proof_with(&net, &d));
    assert_eq!(
        evidence::verdict_with(std::slice::from_ref(&net), true, &d),
        Verdict::Unverified
    );
    assert!(evidence::claim_check_with("C'est fait.", std::slice::from_ref(&net), &d).is_some());
    assert!(
        evidence::claim_check_with("J'ai créé la fiche.", std::slice::from_ref(&net), &d).is_some()
    );
}

#[test]
fn un_outil_exec_ne_prouve_pas_c_est_fait() {
    let (d, exec) = exec_ok();
    assert!(!evidence::is_proof_with(&exec, &d));
    assert_eq!(
        evidence::verdict_with(std::slice::from_ref(&exec), true, &d),
        Verdict::Unverified
    );
    assert!(evidence::claim_check_with("C'est fait.", std::slice::from_ref(&exec), &d).is_some());
}

#[test]
fn une_ecriture_dont_l_empreinte_a_change_prouve() {
    let d = [Declaration::writes("row_create")];
    let write = Step::tool("row_create", 1).state("a", "b");
    assert!(evidence::is_proof_with(&write, &d));
    assert_eq!(
        evidence::verdict_with(std::slice::from_ref(&write), true, &d),
        Verdict::Proven
    );
    assert_eq!(
        evidence::claim_check_with("C'est fait.", std::slice::from_ref(&write), &d),
        None
    );
}

#[test]
fn une_ecriture_dont_l_empreinte_n_a_pas_change_ne_prouve_pas() {
    let d = [Declaration::writes("row_create")];
    let write = Step::tool("row_create", 1).state("a", "a");
    assert!(!evidence::is_proof_with(&write, &d));
    assert_eq!(
        evidence::verdict_with(std::slice::from_ref(&write), true, &d),
        Verdict::Unverified
    );
    assert!(evidence::claim_check_with("C'est fait.", std::slice::from_ref(&write), &d).is_some());
}

#[test]
fn lecture_et_note_ne_prouvent_rien() {
    let d = [Declaration::reads("recall"), Declaration::notes("remember")];
    let read = Step::tool("recall", 1).state("a", "b");
    let note = Step::tool("remember", 1).state("a", "b");
    assert!(!evidence::is_proof_with(&read, &d));
    assert!(!evidence::is_proof_with(&note, &d));
    assert_eq!(
        evidence::verdict_with(&[read, note], true, &d),
        Verdict::Unverified
    );
}

#[test]
fn exec_et_network_ne_comptent_pas_pour_une_ecriture_tentee() {
    // Le passif (« la fiche est créée ») ne se lit que si le run contient une écriture
    // tentée ; une sortie réseau n'en est pas une.
    let (d, net) = reseau_ok();
    assert!(!evidence::write_attempted(&[net], &d));
}

// ------------------------------------------------------------------ déclarations sûres

#[test]
fn une_ecriture_nommee_ne_se_declare_pas_exec_ni_network() {
    // `install_module` est une écriture d'après la table par défaut : la déclarer Exec ou
    // Network affaiblirait la preuve (un refus ne serait plus rattrapable).
    for d in [
        Declaration::execs("install_module"),
        Declaration::networks("install_module"),
    ] {
        assert!(evidence::rejet(&d).is_some(), "{d:?}");
    }
    // Un nom inconnu, lui, se déclare librement Exec ou Network.
    for d in [
        Declaration::execs("run_program"),
        Declaration::networks("web_search"),
    ] {
        assert_eq!(evidence::rejet(&d), None, "{d:?}");
    }
}

#[test]
fn l_effet_se_serialise_en_snake_case() {
    assert_eq!(serde_json::to_string(&Effect::Exec).unwrap(), r#""exec""#);
    assert_eq!(
        serde_json::to_string(&Effect::Network).unwrap(),
        r#""network""#
    );
}
