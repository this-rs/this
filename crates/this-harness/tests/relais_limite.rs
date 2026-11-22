//! Relais d'un événement inconnu (#314) : le contrat réel d'`EventJson`, enveloppe distincte.
//!
//! - `EventJson::parse` + `to_json` : les OCTETS reçus d'un inconnu, espaces compris.
//! - Ce que `serde_json::Value` refuse est refusé nommément (jamais relayé déformé).
//! - La vue `Value` (`to_wire`) est AVEC PERTE et n'est que pour l'inspection.
//! - `UnknownEvent` / `EventWire` sont inchangés : littéraux externes toujours constructibles.
//!
//! Aucune feature `serde_json` n'est activée ici : chaque test tient quelles que soient les
//! features unifiées dans le binaire (mesuré sous aucune / `arbitrary_precision` /
//! `preserve_order`+`raw_value` / les trois).

use serde_json::Value;
use this_harness::{Event, EventJson, EventWire, UnknownEvent};

/// Tout ce qui est facile à perdre : espaces autour et dedans, ordre des clés, clé dupliquée
/// hors `type`, entier au-delà de u64, décimal long, exposant majuscule, `-0`, échappements.
const EXACT: &str = " \n{\"z\":1, \"type\":\"budget_projete\",\"n\":18446744073709551617,\"d\":0.1234567890123456789,\"e\":1E2,\"z\":-0,\"o\":{\"b\":[2,1],\"a\":\"é\\n\\u00e9\"}}\t";

/// `Value` garde-t-il la précision d'un grand entier dans CE binaire (`arbitrary_precision`) ?
fn value_garde_les_grands_entiers() -> bool {
    serde_json::from_str::<Value>("18446744073709551617")
        .unwrap()
        .to_string()
        .as_str()
        == "18446744073709551617"
}

#[test]
fn le_relais_exact_rend_les_octets_recus_espaces_compris() {
    let lu = EventJson::parse(EXACT).expect("ce que Value accepte se lit");
    assert_eq!(lu.unknown_type(), Some("budget_projete"));
    assert!(lu.known().is_none());
    assert_eq!(
        lu.to_json().unwrap(),
        EXACT,
        "octets, ordre des clés, nombres, espaces"
    );
    assert_eq!(
        EventJson::parse(&lu.to_json().unwrap()).unwrap(),
        lu,
        "idempotent"
    );
}

#[test]
fn la_vue_value_est_avec_perte_et_le_dit() {
    let lu = EventJson::parse(EXACT).unwrap();
    let EventWire::Unknown(vue) = lu.to_wire().unwrap() else {
        panic!("inconnu attendu")
    };
    assert_eq!(vue.type_name, "budget_projete");
    let n = vue.raw["n"].to_string();
    if value_garde_les_grands_entiers() {
        assert_eq!(
            n, "18446744073709551617",
            "arbitrary_precision : la vue garde la valeur"
        );
    } else {
        assert_ne!(
            n, "18446744073709551617",
            "sans arbitrary_precision : arrondi ({n})"
        );
        assert!(vue.raw["n"].is_f64(), "passé en flottant : {n}");
    }
    // Quelle que soit la vue, le relais exact, lui, ne bouge pas.
    assert!(lu.to_json().unwrap().contains("18446744073709551617"));
}

#[test]
fn ce_que_value_refuse_est_refuse_nommement() {
    // Surrogate UTF-16 isolé : un autre lecteur serde le refuserait, donc refusé ici.
    assert!(EventJson::parse(r#"{"type":"futur","x":"\ud800"}"#).is_err());
    // Profondeur au-delà de la limite de serde_json.
    let mut profond = String::from(r#"{"type":"futur","x":"#);
    profond.push_str(&"[".repeat(200));
    profond.push_str(&"]".repeat(200));
    profond.push('}');
    assert!(EventJson::parse(&profond).is_err());
    // Zéro initial, caractères après l'objet.
    assert!(EventJson::parse(r#"{"type":"futur","x":01}"#).is_err());
    assert!(EventJson::parse(r#"{"type":"futur"} trailing"#).is_err());
    // Hors de f64 : refusé, SAUF si `arbitrary_precision` est unifiée — alors accepté et relayé
    // exactement. Jamais arrondi en silence.
    let hors = r#"{"type":"futur","x":1e400}"#;
    // Refusé : jamais arrondi. Accepté (`arbitrary_precision`) : relayé tel quel.
    if let Ok(lu) = EventJson::parse(hors) {
        assert_eq!(lu.to_json().unwrap(), hors, "accepté : relayé tel quel");
    }
}

#[test]
fn un_type_absent_duplique_ou_non_textuel_est_refuse() {
    for brut in [
        r#"{"type":"futur","type":"autre"}"#,
        r#"{"type":"futur","type":"futur"}"#,
        r#"{"type":1}"#,
        r#"{"type":null}"#,
        r#"{"sans":"type"}"#,
    ] {
        assert!(EventJson::parse(brut).is_err(), "{brut}");
    }
}

#[test]
fn une_sequence_ou_un_scalaire_ne_sont_pas_des_evenements() {
    for brut in [r#"["futur"]"#, r#"  ["futur"]"#, "42", r#""texte""#, "null"] {
        assert!(EventJson::parse(brut).is_err(), "{brut}");
    }
}

#[test]
fn un_type_connu_est_decode_strictement_et_reserialise() {
    // Payload incomplet mais JSON VALIDE : erreur nommant le champ, jamais un inconnu.
    let err = EventJson::parse(r#"{"type":"delta"}"#).unwrap_err();
    assert!(err.to_string().contains("text"), "{err}");
    let lu = EventJson::parse(r#"{"type":"delta","text":"salut"}"#).unwrap();
    assert!(matches!(lu.known(), Some(Event::Delta { .. })));
    assert!(lu.unknown_type().is_none());
    assert_eq!(
        lu.to_json().unwrap(),
        serde_json::to_string(lu.known().unwrap()).unwrap()
    );
}

/// Limite assumée : un type CONNU auquel une version future ajoute un champ perd ce champ au
/// relais (`Event` ne le connaît pas). Le relais exact ne couvre que les types inconnus.
#[test]
fn un_champ_futur_d_un_type_connu_est_perdu_au_relais() {
    if let Ok(lu) = EventJson::parse(r#"{"type":"delta","text":"x","futur":1}"#) {
        assert!(!lu.to_json().unwrap().contains("futur"));
    }
}

/// `UnknownEvent` est inchangé : un littéral externe compile toujours (pas de rupture de source).
#[test]
fn unknown_event_reste_constructible_par_un_litteral_externe() {
    let u = UnknownEvent {
        type_name: "futur".into(),
        raw: serde_json::json!({"type":"futur","n":1}),
    };
    assert_eq!(EventWire::Unknown(u.clone()), EventWire::Unknown(u));
}

/// Garde-fou : `this-harness` n'active pas DIRECTEMENT `raw_value`. Il ne voit pas une activation
/// TRANSITIVE par une autre crate du binaire.
#[test]
fn le_manifeste_de_this_harness_n_active_pas_directement_raw_value() {
    let chemin = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
    let manifeste = std::fs::read_to_string(&chemin)
        .unwrap_or_else(|e| panic!("{} illisible : {e}", chemin.display()));
    assert!(!manifeste.contains("raw_value"));
}
