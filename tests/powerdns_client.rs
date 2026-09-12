//! **Le client PowerDNS**, vérifié contre un serveur **factice**.
//!
//! On ne teste pas PowerDNS : on teste **ce que this-rs lui demande**. C'est ce
//! qui compte ici — la méthode, le chemin, l'en-tête d'authentification, et la
//! forme exacte des changements (`REPLACE` pour créer ou modifier, `DELETE`
//! seulement quand le plan en contient).
//!
//! Et ce qui compte tout autant : **un refus du serveur est une erreur**, jamais
//! un succès silencieux. Un framework qui croit avoir publié un domaine et ne
//! l'a pas fait est pire qu'un framework qui échoue.

#![cfg(feature = "powerdns")]

use std::sync::{Arc, Mutex};

use axum::Router;
use axum::body::Body;
use axum::extract::State;
use axum::http::{Request, StatusCode};
use axum::response::IntoResponse;
use axum::routing::{get, post, put};
use serde_json::Value;

use this::dns::powerdns::{PdnsClient, PdnsConfig};
use this::dns::{DnsRecord, ensure_dot, plan};

/// Ce que le serveur factice a reçu.
#[derive(Default)]
struct Recorder {
    calls: Vec<(String, String, Option<String>)>, // (méthode, chemin, clé d'API)
    bodies: Vec<Value>,
    /// La zone existe-t-elle ? (404 quand faux)
    zone_exists: bool,
    /// Répondre en erreur sur PATCH ?
    refuse_patch: bool,
}

struct Stub {
    state: Arc<Mutex<Recorder>>,
    base: String,
}

/// Un PowerDNS factice, qui **enregistre** ce qu'on lui envoie.
async fn stub(recorder: Recorder) -> Stub {
    let state = Arc::new(Mutex::new(recorder));

    async fn zone(
        State(state): State<Arc<Mutex<Recorder>>>,
        request: Request<Body>,
    ) -> axum::response::Response {
        let key = request
            .headers()
            .get("x-api-key")
            .and_then(|v| v.to_str().ok())
            .map(str::to_string);
        state.lock().unwrap().calls.push((
            "GET".to_string(),
            request.uri().path().to_string(),
            key,
        ));
        if !state.lock().unwrap().zone_exists {
            return (StatusCode::NOT_FOUND, "").into_response();
        }
        axum::Json(serde_json::json!({
            "rrsets": [
                { "name": "app.exemple.fr.", "type": "A", "ttl": 300,
                  "records": [ { "content": "203.0.113.99", "disabled": false } ] },
                { "name": "exemple.fr.", "type": "MX", "ttl": 3600,
                  "records": [ { "content": "10 mail.exemple.fr.", "disabled": false } ] }
            ]
        }))
        .into_response()
    }

    async fn change(
        State(state): State<Arc<Mutex<Recorder>>>,
        request: Request<Body>,
    ) -> axum::response::Response {
        let key = request
            .headers()
            .get("x-api-key")
            .and_then(|v| v.to_str().ok())
            .map(str::to_string);
        let path = request.uri().path().to_string();
        let body = axum::body::to_bytes(request.into_body(), 64 * 1024)
            .await
            .unwrap_or_default();
        {
            let mut recorder = state.lock().unwrap();
            recorder.calls.push(("PATCH".to_string(), path, key));
            if let Ok(json) = serde_json::from_slice::<Value>(&body) {
                recorder.bodies.push(json);
            }
            if recorder.refuse_patch {
                return (StatusCode::UNPROCESSABLE_ENTITY, "refusé").into_response();
            }
        }
        StatusCode::NO_CONTENT.into_response()
    }

    async fn rectify() -> StatusCode {
        StatusCode::OK
    }

    async fn create_zone(
        State(state): State<Arc<Mutex<Recorder>>>,
        request: Request<Body>,
    ) -> axum::response::Response {
        let body = axum::body::to_bytes(request.into_body(), 64 * 1024)
            .await
            .unwrap_or_default();
        let mut recorder = state.lock().unwrap();
        recorder
            .calls
            .push(("POST".to_string(), "/zones".to_string(), None));
        if let Ok(json) = serde_json::from_slice::<Value>(&body) {
            recorder.bodies.push(json);
        }
        (StatusCode::CREATED, "").into_response()
    }

    let app = Router::new()
        .route(
            "/api/v1/servers/localhost/zones/exemple.fr.",
            get(zone).patch(change).put(rectify_router),
        )
        .route(
            "/api/v1/servers/localhost/zones/exemple.fr./rectify",
            put(rectify),
        )
        .route("/api/v1/servers/localhost/zones", post(create_zone))
        .with_state(Arc::clone(&state));

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("écoute");
    let base = format!("http://{}", listener.local_addr().expect("adresse"));
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });

    Stub { state, base }
}

/// Une route `PUT` vide pour ne pas casser le routeur du factice.
async fn rectify_router() -> StatusCode {
    StatusCode::METHOD_NOT_ALLOWED
}

fn config(base: &str) -> PdnsConfig {
    PdnsConfig {
        api_url: base.to_string(),
        api_key_env: "PDNS_API_KEY_TEST".to_string(),
        server_id: "localhost".to_string(),
        zone: "exemple.fr".to_string(),
        ttl: 300,
        target: "203.0.113.10".to_string(),
        record_type: "A".to_string(),
        prune: false,
        rectify: true,
    }
}

fn set_key(value: &str) {
    // Un seul test à la fois touche l'environnement : `serial` n'est pas
    // nécessaire ici, les clés sont posées avant chaque appel.
    unsafe { std::env::set_var("PDNS_API_KEY_TEST", value) };
}

#[tokio::test]
async fn the_client_reads_the_zone_and_sends_the_key() {
    set_key("cle-de-test");
    let stub = stub(Recorder {
        zone_exists: true,
        ..Default::default()
    })
    .await;

    let client = PdnsClient::new(config(&stub.base)).expect("client");
    let records = client.current_records().await.expect("lecture");

    assert_eq!(records.len(), 2, "deux enregistrements : {records:?}");
    assert!(records.iter().any(|r| r.record_type == "MX"));
    assert!(client.zone_exists().await.expect("existence"));

    let (method, path, key) = stub.state.lock().unwrap().calls[0].clone();
    assert_eq!(method, "GET");
    assert_eq!(
        path, "/api/v1/servers/localhost/zones/exemple.fr.",
        "le chemin porte le nom de zone QUALIFIÉ"
    );
    assert_eq!(
        key.as_deref(),
        Some("cle-de-test"),
        "la clé part en en-tête"
    );
}

#[tokio::test]
async fn an_absent_zone_is_reported_as_absent_not_as_an_error() {
    set_key("cle-de-test");
    let stub = stub(Recorder::default()).await;
    let client = PdnsClient::new(config(&stub.base)).expect("client");
    assert!(!client.zone_exists().await.expect("existence"));
}

/// **L'appel d'écriture**, tel qu'il doit être : un `REPLACE` par enregistrement
/// créé ou modifié, et **aucun** `DELETE` sans `prune`.
#[tokio::test]
async fn applying_a_plan_sends_replace_only_and_never_deletes_without_prune() {
    set_key("cle-de-test");
    let stub = stub(Recorder {
        zone_exists: true,
        ..Default::default()
    })
    .await;
    let client = PdnsClient::new(config(&stub.base)).expect("client");

    let desired = vec![DnsRecord {
        name: ensure_dot("app.exemple.fr"),
        record_type: "A".to_string(),
        content: "203.0.113.10".to_string(),
        ttl: 300,
    }];
    let current = client.current_records().await.expect("lecture");
    let dns_plan = plan("exemple.fr", &desired, &current, false);
    assert_eq!(
        dns_plan.updates.len(),
        1,
        "le contenu a changé : {dns_plan:?}"
    );
    assert!(dns_plan.deletes.is_empty());

    let applied = client.apply(&dns_plan).await.expect("application");
    assert_eq!(applied.replaced, 1);
    assert_eq!(applied.deleted, 0);
    assert!(
        applied.rectified,
        "la zone est rectifiée après un changement"
    );

    let bodies = stub.state.lock().unwrap().bodies.clone();
    let rrsets = bodies[0]["rrsets"].as_array().expect("rrsets");
    assert_eq!(rrsets.len(), 1);
    assert_eq!(rrsets[0]["changetype"], "REPLACE");
    assert_eq!(rrsets[0]["type"], "A");
    assert_eq!(rrsets[0]["name"], "app.exemple.fr.");
    assert_eq!(rrsets[0]["records"][0]["content"], "203.0.113.10");
}

/// Avec `prune`, la suppression part — et **seulement** pour un enregistrement
/// géré (jamais le MX).
#[tokio::test]
async fn pruning_deletes_only_managed_records() {
    set_key("cle-de-test");
    let stub = stub(Recorder {
        zone_exists: true,
        ..Default::default()
    })
    .await;
    let client = PdnsClient::new(config(&stub.base)).expect("client");

    // Rien de déclaré : tout ce qui est géré et dans la zone est à supprimer.
    let current = client.current_records().await.expect("lecture");
    let dns_plan = plan("exemple.fr", &[], &current, true);
    assert_eq!(
        dns_plan.deletes.len(),
        1,
        "seul l'A est géré : {dns_plan:?}"
    );
    assert!(
        dns_plan.untouched.iter().any(|r| r.record_type == "MX"),
        "le MX n'est jamais dans le plan de suppression"
    );

    let applied = client.apply(&dns_plan).await.expect("application");
    assert_eq!(applied.deleted, 1);

    let bodies = stub.state.lock().unwrap().bodies.clone();
    let rrsets = bodies[0]["rrsets"].as_array().expect("rrsets");
    assert_eq!(rrsets[0]["changetype"], "DELETE");
    assert!(
        !serde_json::to_string(&bodies)
            .unwrap()
            .contains("mail.exemple.fr"),
        "le MX ne doit apparaître dans AUCUN changement envoyé"
    );
}

/// **Un refus du serveur est une erreur.** Un framework qui croit avoir publié un
/// domaine et ne l'a pas fait est pire qu'un framework qui échoue.
#[tokio::test]
async fn a_refused_change_is_an_error_never_a_silent_success() {
    set_key("cle-de-test");
    let stub = stub(Recorder {
        zone_exists: true,
        refuse_patch: true,
        ..Default::default()
    })
    .await;
    let client = PdnsClient::new(config(&stub.base)).expect("client");

    let desired = vec![DnsRecord {
        name: ensure_dot("app.exemple.fr"),
        record_type: "A".to_string(),
        content: "203.0.113.10".to_string(),
        ttl: 300,
    }];
    let dns_plan = plan("exemple.fr", &desired, &[], false);
    let error = client
        .apply(&dns_plan)
        .await
        .expect_err("un refus doit remonter");
    let message = format!("{error:#}");
    assert!(
        message.contains("422"),
        "le statut doit être nommé : {message}"
    );
    assert!(
        !message.contains("cle-de-test"),
        "la clé d'API ne doit JAMAIS apparaître dans une erreur : {message}"
    );
}

/// Une clé absente **arrête** au lieu d'appeler l'API sans authentification.
#[test]
fn a_missing_api_key_is_refused_before_any_call() {
    unsafe { std::env::remove_var("PDNS_API_KEY_ABSENTE") };
    let mut config = config("http://127.0.0.1:1");
    config.api_key_env = "PDNS_API_KEY_ABSENTE".to_string();

    let error = match PdnsClient::new(config) {
        Ok(_) => panic!("une clé absente doit arrêter avant tout appel"),
        Err(e) => e,
    };
    let message = format!("{error:#}");
    assert!(
        message.contains("PDNS_API_KEY_ABSENTE"),
        "l'erreur doit nommer la variable attendue : {message}"
    );
}
