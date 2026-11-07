//! Les pannes d'un provider arrivent au harnais telles quelles : chaque famille que
//! `this-llm-providers` nomme doit être un code que ce harnais connaît, et la forme que
//! rend une coquille (`{ code, status, message }`) doit se relire en un `Failure`.
//!
//! Sans ce test, un code renommé d'un côté retomberait silencieusement en « injoignable ».

use serde_json::json;
use this_harness::{Code, Failure};
use this_llm_providers::ErrorKind;

#[test]
fn chaque_famille_de_panne_d_un_provider_est_un_code_du_harnais() {
    let familles = [
        ErrorKind::Unreachable,
        ErrorKind::Http(429),
        ErrorKind::Truncated,
        ErrorKind::Unreadable,
        ErrorKind::Timeout,
        ErrorKind::Provider,
    ];
    for famille in familles {
        let relu: Result<Code, _> = serde_json::from_value(json!(famille.code()));
        assert!(
            relu.is_ok(),
            "`{}` n'est pas un code du harnais : une panne de cette famille serait mal rangée",
            famille.code()
        );
    }
}

#[test]
fn une_panne_de_la_coquille_se_relit_en_failure_avec_sa_famille() {
    let recu = r#"{"code":"http","status":429,"message":"trop de requêtes"}"#;
    let panne: Failure = serde_json::from_str(recu).expect("la forme de la coquille se relit");
    assert_eq!(panne.code, Code::Http);
    assert_eq!(panne.status, Some(429));
    assert_eq!(panne.message, "trop de requêtes");

    // Sans statut, la coquille écrit `null` : ça se relit aussi.
    let sans_statut = r#"{"code":"timeout","status":null,"message":"silence"}"#;
    let panne: Failure = serde_json::from_str(sans_statut).expect("un statut nul se relit");
    assert_eq!(panne.code, Code::Timeout);
    assert_eq!(panne.status, None);
}
