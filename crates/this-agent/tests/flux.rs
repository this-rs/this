//! Le contrat des flux : franchissement, réarmement, pertes comptées, cycle de vie,
//! refus motivés. Aucune I/O : le temps et les événements sont fournis par le test.

use std::collections::BTreeMap;

use this_agent::flux::{
    Diffuseur, Event, Filtre, Livraison, Proprietaire, Refus, Souscription, Valeur, catalogue,
    depuis_appareil,
};
use this_agent::surface::Status;

fn module(nom: &str) -> Proprietaire {
    Proprietaire {
        module: nom.into(),
        run: None,
    }
}

fn batterie(at_ms: u64, niveau: f64) -> Event {
    let mut champs = BTreeMap::new();
    champs.insert("niveau".into(), Valeur::Nombre(niveau));
    champs.insert("en_charge".into(), Valeur::Booleen(false));
    Event {
        flux: "battery.level".into(),
        at_ms,
        champs,
    }
}

fn avec_filtre(filtre: Filtre) -> Souscription {
    Souscription {
        filtre: Some(filtre),
        ..Souscription::default()
    }
}

fn evenements(sortie: Vec<Livraison>) -> Vec<Event> {
    sortie
        .into_iter()
        .filter_map(|l| match l {
            Livraison::Evenement(e) => Some(e),
            Livraison::Perdus { .. } => None,
        })
        .collect()
}

fn niveaux(sortie: &[Event]) -> Vec<f64> {
    sortie
        .iter()
        .map(|e| match e.champs["niveau"] {
            Valeur::Nombre(x) => x,
            _ => panic!("niveau attendu"),
        })
        .collect()
}

// ------------------------------------------------------------ franchissement

#[test]
fn below_ne_se_declenche_qu_une_fois_en_franchissant_vers_le_bas() {
    let mut d = Diffuseur::new();
    let id = d
        .abonner(
            module("veilleur"),
            "battery.level",
            avec_filtre(Filtre::Below {
                champ: "niveau".into(),
                seuil: 0.20,
            }),
        )
        .unwrap();

    // Au-dessus, puis le franchissement, puis 500 échantillons restant dessous.
    d.pousser(batterie(0, 0.50)).unwrap();
    d.pousser(batterie(1, 0.25)).unwrap();
    let reveil = d.pousser(batterie(2, 0.19)).unwrap();
    assert_eq!(reveil, vec![id], "le franchissement réveille");
    for t in 3..503 {
        let reveil = d.pousser(batterie(t, 0.19)).unwrap();
        assert!(reveil.is_empty(), "rester sous le seuil ne réveille pas");
    }
    let livres = evenements(d.lire(id).unwrap());
    assert_eq!(niveaux(&livres), vec![0.19], "un seul événement livré");
}

#[test]
fn below_se_rearme_apres_etre_remonte_au_dessus() {
    let mut d = Diffuseur::new();
    let id = d
        .abonner(
            module("veilleur"),
            "battery.level",
            avec_filtre(Filtre::Below {
                champ: "niveau".into(),
                seuil: 0.20,
            }),
        )
        .unwrap();

    for niveau in [0.50, 0.19, 0.18, 0.25, 0.10] {
        d.pousser(batterie(0, niveau)).unwrap();
    }
    // Deux franchissements vers le bas : 0,19 puis 0,10. Pas 0,18 (déjà dessous).
    let livres = evenements(d.lire(id).unwrap());
    assert_eq!(niveaux(&livres), vec![0.19, 0.10]);
}

#[test]
fn premier_echantillon_deja_du_bon_cote_se_declenche() {
    let mut d = Diffuseur::new();
    let id = d
        .abonner(
            module("veilleur"),
            "battery.level",
            avec_filtre(Filtre::Below {
                champ: "niveau".into(),
                seuil: 0.20,
            }),
        )
        .unwrap();
    d.pousser(batterie(0, 0.05)).unwrap();
    assert_eq!(niveaux(&evenements(d.lire(id).unwrap())), vec![0.05]);
}

#[test]
fn above_suit_le_meme_principe_vers_le_haut() {
    let mut d = Diffuseur::new();
    let id = d
        .abonner(
            module("veilleur"),
            "battery.level",
            avec_filtre(Filtre::Above {
                champ: "niveau".into(),
                seuil: 0.80,
            }),
        )
        .unwrap();
    for niveau in [0.70, 0.85, 0.90, 0.79, 0.81] {
        d.pousser(batterie(0, niveau)).unwrap();
    }
    assert_eq!(niveaux(&evenements(d.lire(id).unwrap())), vec![0.85, 0.81]);
}

#[test]
fn equals_et_exit_suivent_une_zone() {
    let zone = |at_ms: u64, z: &str| {
        let mut champs = BTreeMap::new();
        champs.insert("lat".into(), Valeur::Nombre(48.8));
        champs.insert("lon".into(), Valeur::Nombre(2.3));
        champs.insert("zone".into(), Valeur::Texte(z.into()));
        Event {
            flux: "location.position".into(),
            at_ms,
            champs,
        }
    };
    let mut d = Diffuseur::new();
    let entree = d
        .abonner(
            module("maison"),
            "location.position",
            avec_filtre(Filtre::Equals {
                champ: "zone".into(),
                valeur: Valeur::Texte("maison".into()),
            }),
        )
        .unwrap();
    let sortie = d
        .abonner(
            module("maison"),
            "location.position",
            avec_filtre(Filtre::Exit {
                champ: "zone".into(),
                valeur: Valeur::Texte("maison".into()),
            }),
        )
        .unwrap();

    d.pousser(zone(0, "dehors")).unwrap();
    d.pousser(zone(1, "maison")).unwrap();
    d.pousser(zone(2, "maison")).unwrap();
    d.pousser(zone(3, "dehors")).unwrap();

    let entrees = evenements(d.lire(entree).unwrap());
    let sorties = evenements(d.lire(sortie).unwrap());
    assert_eq!(entrees.len(), 1, "une entrée, pas une par position");
    assert_eq!(sorties.len(), 1, "une sortie");
    assert_eq!(sorties[0].at_ms, 3);
}

#[test]
fn changed_ne_se_declenche_pas_au_premier_echantillon() {
    let mut d = Diffuseur::new();
    let id = d
        .abonner(
            module("veilleur"),
            "battery.level",
            avec_filtre(Filtre::Changed {
                champ: "en_charge".into(),
            }),
        )
        .unwrap();
    d.pousser(batterie(0, 0.5)).unwrap();
    assert!(d.lire(id).unwrap().is_empty());
}

// ----------------------------------------------------------------- cadence

#[test]
fn throttled_ne_laisse_passer_qu_un_evenement_par_seconde() {
    let mut d = Diffuseur::new();
    let id = d
        .abonner(
            module("pouls"),
            "battery.level",
            Souscription {
                espacement_ms: Some(1000),
                ..Souscription::default()
            },
        )
        .unwrap();
    for at in [0, 100, 999, 1000, 1500, 2001] {
        d.pousser(batterie(at, 0.5)).unwrap();
    }
    let livres = evenements(d.lire(id).unwrap());
    let instants: Vec<u64> = livres.iter().map(|e| e.at_ms).collect();
    assert_eq!(instants, vec![0, 1000, 2001]);
}

#[test]
fn sans_filtre_on_recoit_tout() {
    let mut d = Diffuseur::new();
    let id = d
        .abonner(module("tout"), "battery.level", Souscription::default())
        .unwrap();
    for at in 0..10 {
        d.pousser(batterie(at, 0.5)).unwrap();
    }
    assert_eq!(evenements(d.lire(id).unwrap()).len(), 10);
}

// ------------------------------------------------------------------- pertes

#[test]
fn la_perte_remonte_en_tete_quand_la_file_est_pleine() {
    let mut d = Diffuseur::new();
    let id = d
        .abonner(
            module("lent"),
            "battery.level",
            Souscription {
                capacite: Some(2),
                ..Souscription::default()
            },
        )
        .unwrap();
    for (at, niveau) in [(0, 0.1), (1, 0.2), (2, 0.3)] {
        d.pousser(batterie(at, niveau)).unwrap();
    }
    let sortie = d.lire(id).unwrap();
    assert_eq!(
        sortie[0],
        Livraison::Perdus {
            topic: "battery.level".into(),
            count: 1
        },
        "la perte est dite avant les événements"
    );
    // La file garde les plus récents : on a évincé le plus ancien.
    assert_eq!(niveaux(&evenements(sortie)), vec![0.2, 0.3]);
    // Le compteur se remet à zéro à la lecture suivante.
    assert!(d.lire(id).unwrap().is_empty());
}

#[test]
fn un_evenement_non_voulu_n_est_pas_une_perte() {
    let mut d = Diffuseur::new();
    let id = d
        .abonner(
            module("seuil"),
            "battery.level",
            Souscription {
                filtre: Some(Filtre::Below {
                    champ: "niveau".into(),
                    seuil: 0.20,
                }),
                capacite: Some(1),
                ..Souscription::default()
            },
        )
        .unwrap();
    for niveau in [0.9, 0.8, 0.7, 0.6] {
        d.pousser(batterie(0, niveau)).unwrap();
    }
    assert!(
        d.lire(id).unwrap().is_empty(),
        "rien de voulu, donc rien de perdu"
    );
}

// --------------------------------------------------------------- cycle de vie

#[test]
fn liberer_un_module_coupe_ses_abonnements_et_les_reveils() {
    let mut d = Diffuseur::new();
    let id = d
        .abonner(module("parti"), "battery.level", Souscription::default())
        .unwrap();
    assert_eq!(d.abonnements_actifs(), 1);
    assert_eq!(d.liberer_module("parti"), 1);
    assert_eq!(d.abonnements_actifs(), 0);
    assert!(d.pousser(batterie(0, 0.5)).unwrap().is_empty());
    assert!(d.lire(id).is_none(), "un abonnement libéré n'existe plus");
}

#[test]
fn la_fin_d_un_run_libere_ses_seuls_abonnements() {
    let mut d = Diffuseur::new();
    d.abonner(
        Proprietaire {
            module: "m".into(),
            run: Some("r1".into()),
        },
        "battery.level",
        Souscription::default(),
    )
    .unwrap();
    d.abonner(module("m"), "battery.level", Souscription::default())
        .unwrap();
    assert_eq!(d.terminer_run("r1"), 1);
    assert_eq!(d.abonnements_actifs(), 1, "l'abonnement du module reste");
}

// ---------------------------------------------------------------- refus

#[test]
fn un_id_inconnu_est_refuse_avec_un_motif() {
    let mut d = Diffuseur::new();
    let refus = d
        .abonner(module("m"), "battery.lvl", Souscription::default())
        .unwrap_err();
    assert_eq!(refus, Refus::FluxInconnu("battery.lvl".into()));
    assert!(refus.to_string().contains("battery.lvl"));
}

#[test]
fn un_filtre_sur_un_champ_inconnu_ou_mal_type_est_refuse() {
    let mut d = Diffuseur::new();
    let inconnu = d
        .abonner(
            module("m"),
            "battery.level",
            avec_filtre(Filtre::Below {
                champ: "pourcent".into(),
                seuil: 0.2,
            }),
        )
        .unwrap_err();
    assert!(matches!(inconnu, Refus::ChampInconnu { .. }));

    let mauvais_genre = d
        .abonner(
            module("m"),
            "battery.level",
            avec_filtre(Filtre::Below {
                champ: "en_charge".into(),
                seuil: 0.2,
            }),
        )
        .unwrap_err();
    assert!(matches!(mauvais_genre, Refus::TypeIncompatible { .. }));
}

#[test]
fn un_evenement_mal_forme_est_refuse_sans_toucher_aux_abonnements() {
    let mut d = Diffuseur::new();
    let id = d
        .abonner(module("m"), "battery.level", Souscription::default())
        .unwrap();

    let mut manquant = batterie(0, 0.5);
    manquant.champs.remove("en_charge");
    assert!(matches!(
        d.pousser(manquant),
        Err(Refus::ChampManquant { .. })
    ));

    // Un NaN ne doit jamais atteindre une comparaison de seuil.
    assert!(matches!(
        d.pousser(batterie(1, f64::NAN)),
        Err(Refus::ValeurNonFinie { .. })
    ));

    assert!(d.lire(id).unwrap().is_empty(), "rien n'a été livré");
}

#[test]
fn une_capacite_nulle_est_refusee() {
    let mut d = Diffuseur::new();
    let refus = d
        .abonner(
            module("m"),
            "battery.level",
            Souscription {
                capacite: Some(0),
                ..Souscription::default()
            },
        )
        .unwrap_err();
    assert_eq!(refus, Refus::CapaciteNulle);
}

// ------------------------------------------------------------- catalogue

#[test]
fn le_catalogue_ne_declare_rien_de_live_et_ses_ids_sont_uniques() {
    let flux = catalogue();
    assert!(flux.iter().all(|s| s.status == Status::Missing));
    let mut ids: Vec<&str> = flux.iter().map(|s| s.id.as_str()).collect();
    ids.sort_unstable();
    let avant = ids.len();
    ids.dedup();
    assert_eq!(avant, ids.len(), "identifiants en double");
    for attendu in [
        "battery.level",
        "network.state",
        "gesture.tap",
        "app.foreground",
    ] {
        assert!(ids.contains(&attendu), "{attendu} manque au catalogue");
    }
}

// ------------------------------------------------------------- pont natif

#[test]
fn un_echantillon_de_batterie_du_pont_devient_un_evenement_du_catalogue() {
    let ev = depuis_appareil(
        &serde_json::json!({"kind":"battery","level":0.42,"state":"unplugged"}),
        1_000,
    )
    .unwrap();
    assert_eq!(ev.flux, "battery.level");
    assert_eq!(ev.at_ms, 1_000);
    assert_eq!(ev.champs["niveau"], Valeur::Nombre(0.42));
    assert_eq!(ev.champs["en_charge"], Valeur::Booleen(false));
    let charge = depuis_appareil(
        &serde_json::json!({"kind":"battery","level":0.9,"state":"charging"}),
        0,
    )
    .unwrap();
    assert_eq!(charge.champs["en_charge"], Valeur::Booleen(true));
}

#[test]
fn un_niveau_inconnu_n_est_pas_un_zero() {
    // Simulateur : `level` vaut -1. Un zéro inventé franchirait tous les seuils bas.
    assert!(
        depuis_appareil(
            &serde_json::json!({"kind":"battery","level":-1.0,"state":"unknown"}),
            0
        )
        .is_none()
    );
}

#[test]
fn le_pont_alimente_le_diffuseur_et_le_seuil_franchit_une_fois() {
    let mut d = Diffuseur::new();
    let bas = d
        .abonner(
            module("m"),
            "battery.level",
            Souscription {
                filtre: Some(Filtre::Below {
                    champ: "niveau".into(),
                    seuil: 0.50,
                }),
                ..Souscription::default()
            },
        )
        .unwrap();
    let orient = d
        .abonner(module("m"), "orientation", Souscription::default())
        .unwrap();
    for (at, niveau) in [
        (0, 0.55),
        (1, 0.51),
        (2, 0.49),
        (3, 0.48),
        (4, 0.52),
        (5, 0.47),
    ] {
        let ev = depuis_appareil(
            &serde_json::json!({"kind":"battery","level":niveau,"state":"unplugged"}),
            at,
        )
        .unwrap();
        d.pousser(ev).unwrap();
    }
    let niveaux: Vec<u64> = d
        .lire(bas)
        .unwrap()
        .into_iter()
        .filter_map(|l| match l {
            Livraison::Evenement(e) => Some(e.at_ms),
            Livraison::Perdus { .. } => None,
        })
        .collect();
    // Deux franchissements vers le bas (0.49 puis 0.47), pas quatre échantillons sous le seuil.
    assert_eq!(niveaux, vec![2, 5]);

    let ev = depuis_appareil(
        &serde_json::json!({"kind":"orientation","angle":"paysage_gauche"}),
        9,
    )
    .unwrap();
    assert_eq!(d.pousser(ev).unwrap(), vec![orient]);
    // Un angle hors des quatre choix est refusé, jamais transmis.
    let faux = depuis_appareil(
        &serde_json::json!({"kind":"orientation","angle":"face_up"}),
        10,
    )
    .unwrap();
    assert!(matches!(d.pousser(faux), Err(Refus::HorsChoix { .. })));
}
