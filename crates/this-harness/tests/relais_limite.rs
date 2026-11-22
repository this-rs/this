//! Limite CONNUE du relais d'un événement inconnu (#314), fixée pour qu'elle ne change pas en
//! silence. Le payload passe par `serde_json::Value` : le sens JSON est gardé, pas les octets.
//! Voir `docs/harnais-contrat-evenements.md` §3. Quand la conservation exacte sera livrée (sans
//! activer `serde_json/raw_value` dans tout le binaire), ce test changera — exprès.

use this_harness::EventWire;

fn relayer(brut: &str) -> Result<String, String> {
    serde_json::from_str::<EventWire>(brut)
        .map(|w| serde_json::to_string(&w).unwrap())
        .map_err(|e| e.to_string())
}

#[test]
fn un_inconnu_relaye_garde_le_sens_json_pas_les_octets() {
    assert_eq!(
        relayer(r#"{"type":"futur","n":18446744073709551616}"#).unwrap(),
        r#"{"n":1.8446744073709552e+19,"type":"futur"}"#,
        "au-delà de u64 : arrondi en flottant"
    );
    assert_eq!(
        relayer(r#"{"type":"futur","d":0.10,"e":1E2}"#).unwrap(),
        r#"{"d":0.1,"e":100.0,"type":"futur"}"#,
        "décimaux réécrits"
    );
    assert_eq!(
        relayer(r#"{"z":1,"type":"futur","a":2}"#).unwrap(),
        r#"{"a":2,"type":"futur","z":1}"#,
        "clés triées"
    );
    assert!(
        relayer(r#"{"type":"futur","x":1e400}"#).is_err(),
        "hors plage f64 : refusé à la lecture"
    );
}

/// La conservation exacte NE DOIT PAS passer par `serde_json/raw_value` : Cargo unifie la feature
/// dans tout le binaire, et elle change alors la lecture de `Value` partout (un objet dont la
/// première clé est `$serde_json::private::RawValue` est relu comme le JSON qu'elle porte —
/// mesuré sur `ToolCall.arguments`). Le test porte sur le MANIFESTE, pas sur le comportement :
/// sous `--workspace`, `this-mesh` active déjà la feature, et une sonde d'exécution y mentirait.
#[test]
fn this_harness_n_active_pas_raw_value() {
    let chemin = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
    let manifeste = std::fs::read_to_string(&chemin)
        .unwrap_or_else(|e| panic!("{} illisible : {e}", chemin.display()));
    assert!(
        !manifeste.contains("raw_value"),
        "this-harness ne doit pas activer serde_json/raw_value (voir le contrat d'événements §3)"
    );
}
