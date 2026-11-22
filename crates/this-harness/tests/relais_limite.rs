//! Relais d'un événement inconnu (#314), contrat réel :
//! - `EventWire::from_json_str` + `to_json_exact` rendent les OCTETS reçus d'un inconnu, sans passer
//!   par `Value` : un nombre hors de `f64` ne fait échouer ni la lecture, ni le relais ;
//! - la vue `Value` (`UnknownEvent::raw`) n'est que pour l'inspection, et vaut `None` hors de sa
//!   plage ;
//! - le chemin `serde` générique (`from_str::<EventWire>`) passe par `Value` : il rejette ce qui
//!   n'y tient pas, et ne prétend pas au relais exact.
//!
//! Aucune feature `serde_json` n'est activée ici : le contrat tient quelles que soient les
//! features unifiées dans le binaire.

use this_harness::EventWire;

const EXACT: &str = r#"{"z":1,"type":"budget_projete","n":18446744073709551616,"d":0.10,"e":1E2,"x":1e400,"neg":-0,"o":{"b":[2,1],"a":"é\n"}}"#;

#[test]
fn le_relais_exact_rend_les_octets_recus() {
    let lu = EventWire::from_json_str(EXACT).expect("un inconnu, même hors de Value, se lit");
    assert_eq!(
        lu.to_json_exact().unwrap(),
        EXACT,
        "octets, ordre des clés et nombres compris"
    );
}

#[test]
fn les_espaces_qui_entourent_l_objet_sont_retires_seulement() {
    let lu = EventWire::from_json_str(&format!("  \n\t{EXACT} \r\n")).unwrap();
    assert_eq!(lu.to_json_exact().unwrap(), EXACT);
}

#[test]
fn la_vue_value_n_existe_que_dans_sa_plage() {
    let EventWire::Unknown(hors) = EventWire::from_json_str(EXACT).unwrap() else {
        panic!("inconnu attendu")
    };
    assert_eq!(hors.type_name, "budget_projete");
    assert!(
        hors.raw.is_none(),
        "1e400 n'entre pas dans Value : pas de vue, mais pas d'erreur"
    );

    let dans = r#"{"type":"futur","n":42,"f":0.5,"l":[1,{"k":"v"}]}"#;
    let EventWire::Unknown(ok) = EventWire::from_json_str(dans).unwrap() else {
        panic!("inconnu attendu")
    };
    assert_eq!(ok.raw.as_ref().unwrap()["n"], 42);
}

#[test]
fn un_type_duplique_ou_non_textuel_est_refuse() {
    assert!(EventWire::from_json_str(r#"{"type":"futur","type":"autre"}"#).is_err());
    assert!(EventWire::from_json_str(r#"{"type":1}"#).is_err());
    assert!(EventWire::from_json_str(r#"{"sans":"type"}"#).is_err());
}

#[test]
fn une_sequence_ou_un_scalaire_ne_sont_pas_des_evenements() {
    for refuse in [r#"["futur"]"#, "42", r#""texte""#, "null"] {
        assert!(EventWire::from_json_str(refuse).is_err(), "{refuse}");
    }
}

#[test]
fn un_type_connu_passe_par_le_decodage_strict() {
    let brut = r#"{"type":"stopped"}"#;
    // Un type connu au payload incomplet est une erreur, jamais un inconnu.
    assert!(EventWire::from_json_str(r#"{"type":"stopped","stop":}"#).is_err());
    let lu = EventWire::from_json_str(brut);
    if let Ok(connu @ EventWire::Known(_)) = &lu {
        assert_eq!(
            connu.to_json_exact().unwrap(),
            serde_json::to_string(connu.known().unwrap()).unwrap()
        );
    }
}

#[test]
fn le_chemin_serde_generique_passe_par_value_et_le_dit() {
    // Hors de Value : refusé nommément, pas relayé déformé en silence.
    assert!(serde_json::from_str::<EventWire>(EXACT).is_err());
    // Dans Value : lu, avec sa vue.
    let dans = r#"{"type":"futur","n":42}"#;
    let EventWire::Unknown(u) = serde_json::from_str::<EventWire>(dans).unwrap() else {
        panic!("inconnu attendu")
    };
    assert!(u.raw.is_some());
}

/// Garde-fou : le manifeste de `this-harness` n'active pas DIRECTEMENT la feature `raw_value`.
/// Il ne voit pas une activation TRANSITIVE par une autre crate du binaire.
#[test]
fn le_manifeste_de_this_harness_n_active_pas_directement_raw_value() {
    let chemin = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
    let manifeste = std::fs::read_to_string(&chemin)
        .unwrap_or_else(|e| panic!("{} illisible : {e}", chemin.display()));
    assert!(
        !manifeste.contains("raw_value"),
        "this-harness ne doit pas activer serde_json/raw_value directement"
    );
}
