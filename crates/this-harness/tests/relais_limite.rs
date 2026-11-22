//! Relais d'un événement inconnu (#314), contrat réel :
//! - `EventWire::from_json_str` lit le texte en entier par `serde_json::Value` (refus nommé de ce
//!   que `Value` refuse : nombre hors de `f64`, surrogate UTF-16 isolé, profondeur excessive) ;
//! - `to_json_exact` rend les OCTETS reçus d'un inconnu, sans les espaces qui entourent l'objet ;
//! - le `Deserialize` générique passe par `Value` et refuse les mêmes cas.
//!
//! Aucune feature `serde_json` n'est activée : le contrat tient quelles que soient les features
//! unifiées dans le binaire.

use this_harness::EventWire;

/// Tout ce que `Value` accepte, avec ce qui est facile à perdre : ordre des clés, espaces intérieurs,
/// nombres au-delà de `u64`, décimaux, exposant majuscule, `-0`, échappements.
const EXACT: &str = r#"{"z":1,"type":"budget_projete","n":18446744073709551616,"d":0.10,"e":1E2,"neg":-0,"o":{"b":[2,1],"a":"é\né"}}"#;

#[test]
fn le_relais_exact_rend_les_octets_recus() {
    let lu = EventWire::from_json_str(EXACT).expect("ce que Value accepte se lit");
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
fn la_vue_value_est_celle_du_texte() {
    let EventWire::Unknown(u) = EventWire::from_json_str(EXACT).unwrap() else {
        panic!("inconnu attendu")
    };
    assert_eq!(u.type_name, "budget_projete");
    assert_eq!(
        u.raw,
        serde_json::from_str::<serde_json::Value>(EXACT).unwrap()
    );
    assert_eq!(u.texte(), EXACT);
}

#[test]
fn ce_que_value_refuse_est_refuse_nommement() {
    // Nombre hors de f64 : refusé, jamais gardé ni arrondi en silence.
    assert!(EventWire::from_json_str(r#"{"type":"futur","x":1e400}"#).is_err());
    // Surrogate UTF-16 isolé : refusé par Value, donc refusé ici (un autre lecteur le refuserait).
    assert!(EventWire::from_json_str(r#"{"type":"futur","x":"\ud800"}"#).is_err());
    // Profondeur au-delà de la limite de serde_json.
    let mut profond = String::from(r#"{"type":"futur","x":"#);
    profond.push_str(&"[".repeat(200));
    profond.push_str(&"]".repeat(200));
    profond.push('}');
    assert!(EventWire::from_json_str(&profond).is_err());
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
fn un_type_connu_au_payload_incomplet_est_une_erreur() {
    assert!(EventWire::from_json_str(r#"{"type":"stopped","stop":}"#).is_err());
}

#[test]
fn le_chemin_serde_generique_refuse_les_memes_cas() {
    assert!(serde_json::from_str::<EventWire>(r#"{"type":"futur","x":1e400}"#).is_err());
    let dans = r#"{"type":"futur","n":42}"#;
    let EventWire::Unknown(u) = serde_json::from_str::<EventWire>(dans).unwrap() else {
        panic!("inconnu attendu")
    };
    assert_eq!(u.raw["n"], 42);
}

/// Garde-fou : le manifeste de `this-harness` n'active pas DIRECTEMENT `raw_value`. Il ne voit pas
/// une activation TRANSITIVE par une autre crate du binaire.
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

/// Une clé dupliquée AUTRE que `type` : `serde_json::Value` garde la dernière valeur. Le texte reçu,
/// lui, est relayé tel quel (les deux occurrences) : la vue `raw` et le texte ne disent pas la même
/// chose. C'est documenté, et ce test le fige.
#[test]
fn une_cle_dupliquee_autre_que_type_est_relayee_telle_quelle_et_vue_en_value_la_dernier_gagne() {
    let brut = r#"{"type":"futur","a":1,"a":2}"#;
    let lu = EventWire::from_json_str(brut).expect("une clé dupliquée non-type est acceptée");
    let EventWire::Unknown(u) = &lu else {
        panic!("inconnu attendu")
    };
    assert_eq!(
        lu.to_json_exact().unwrap(),
        brut,
        "le texte reçu, les deux occurrences"
    );
    assert_eq!(u.raw["a"], 2, "la vue Value garde la dernière valeur");
}
