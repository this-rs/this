//! Relais d'un événement inconnu (#314) : ce qui est GARANTI, et seulement cela. Le payload passe
//! par `serde_json::Value` ; une valeur représentable par `Value` ressort égale. Les cas au-delà
//! (grands entiers, ordre des clés, `1e400`) dépendent des features `serde_json` unifiées dans le
//! binaire (`preserve_order`, `arbitrary_precision`) : ils sont documentés et mesurés dans
//! `docs/harnais-contrat-evenements.md` §3, PAS figés ici.

use this_harness::EventWire;

#[test]
fn une_valeur_representable_survit_au_relais() {
    let brut = r#"{"type":"futur","n":42,"neg":-7,"f":0.5,"s":"é\n\"x\"","b":true,"z":null,"l":[1,{"k":"v"}],"o":{"a":{"b":[]}}}"#;
    let relaye = serde_json::to_string(&serde_json::from_str::<EventWire>(brut).unwrap()).unwrap();
    let avant: serde_json::Value = serde_json::from_str(brut).unwrap();
    let apres: serde_json::Value = serde_json::from_str(&relaye).unwrap();
    assert_eq!(
        apres, avant,
        "une valeur représentable ressort égale en Value"
    );
}

/// Garde-fou de la décision (§3) : le manifeste de `this-harness` n'active pas DIRECTEMENT la
/// feature `serde_json/raw_value`. Ce test ne voit pas une activation TRANSITIVE (une autre
/// crate du binaire, comme `this-mesh`, peut l'activer par unification Cargo) : il empêche
/// seulement ce crate de l'imposer à tous ses hôtes.
#[test]
fn le_manifeste_de_this_harness_n_active_pas_directement_raw_value() {
    let chemin = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
    let manifeste = std::fs::read_to_string(&chemin)
        .unwrap_or_else(|e| panic!("{} illisible : {e}", chemin.display()));
    assert!(
        !manifeste.contains("raw_value"),
        "this-harness ne doit pas activer serde_json/raw_value (voir le contrat d'événements §3)"
    );
}
