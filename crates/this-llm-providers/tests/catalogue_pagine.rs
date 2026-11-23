//! `/models` was read ONE page at a time: an engine that paginates (Anthropic's `has_more` +
//! `last_id`, Google's `nextPageToken`, a `next` link) showed its first page and no more, and
//! the settings screen announced a catalogue that was a fraction of the real one.
//!
//! The walk is checked against a real local server, with the abuses that make it unsafe:
//! a cursor the engine ignores, a link to another origin, and a later page that fails.

#![cfg(feature = "http")]

use std::sync::{Arc, Mutex};

use serde_json::{Value, json};
use this_llm::{CredentialRef, Preset};
use this_llm_providers::{Hub, MemoryStore, ProviderClient, ProviderConfig, ProviderId, Secrets};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

const KEY: &str = "nvapi-TESTKEY-do-not-leak-0123456789";

/// What the engine received: the request line and whether it carried the key.
#[derive(Clone, Debug)]
struct Seen {
    line: String,
    with_key: bool,
}

type Answer = Arc<dyn Fn(&str) -> (u16, Value) + Send + Sync>;

/// A server that answers `GET …` from `answer(request line)` and records every request.
async fn serve(listener: TcpListener, answer: Answer, seen: Arc<Mutex<Vec<Seen>>>) {
    loop {
        let Ok((mut socket, _)) = listener.accept().await else {
            return;
        };
        let answer = answer.clone();
        let seen = seen.clone();
        tokio::spawn(async move {
            let mut raw = vec![0u8; 8192];
            let read = socket.read(&mut raw).await.unwrap_or(0);
            let request = String::from_utf8_lossy(&raw[..read]).to_string();
            let line = request.lines().next().unwrap_or_default().to_string();
            seen.lock().unwrap().push(Seen {
                line: line.clone(),
                with_key: request.contains(KEY),
            });
            let (status, body) = answer(&line);
            let body = body.to_string();
            let response = format!(
                "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = socket.write_all(response.as_bytes()).await;
            let _ = socket.shutdown().await;
        });
    }
}

async fn engine(answer: Answer) -> (ProviderClient, Arc<Mutex<Vec<Seen>>>, String) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let seen = Arc::new(Mutex::new(Vec::new()));
    tokio::spawn(serve(listener, answer, seen.clone()));
    let id = ProviderId::new("local").unwrap();
    let mut config = ProviderConfig::new(
        id.clone(),
        "Catalogue paginé",
        Preset::OpenAiCompatible,
        CredentialRef::Vault("k".to_string()),
    );
    config.base_url = format!("{origin}/v1");
    struct Day;
    impl this_llm_providers::Clock for Day {
        fn day(&self) -> u64 {
            20_000
        }
    }
    let hub = Hub::open(
        MemoryStore::default(),
        Secrets::with_vault(|_| Some(KEY.to_string())),
        Day,
    )
    .unwrap();
    hub.upsert(config).unwrap();
    (hub.client(&id).expect("client"), seen, origin)
}

fn ids(models: &[this_llm_providers::ModelInfo]) -> Vec<&str> {
    models.iter().map(|m| m.id.as_str()).collect()
}

fn page(ids: &[&str]) -> Vec<Value> {
    ids.iter().map(|id| json!({ "id": id })).collect()
}

/// Anthropic's dialect: `has_more` + `last_id`, the cursor goes back as `after_id`.
#[tokio::test]
async fn le_curseur_after_id_amene_toutes_les_pages() {
    let (client, seen, _) = engine(Arc::new(|line| {
        if line.contains("after_id=b") {
            (
                200,
                json!({"data": page(&["c"]), "has_more": false, "last_id": "c"}),
            )
        } else if line.contains("after_id=") {
            (500, json!({}))
        } else {
            (
                200,
                json!({"data": page(&["a", "b"]), "has_more": true, "last_id": "b"}),
            )
        }
    }))
    .await;
    let models = client.list_models().await.expect("liste");
    assert_eq!(ids(&models), ["a", "b", "c"]);
    assert_eq!(
        seen.lock().unwrap().len(),
        2,
        "deux pages, pas de requête de trop"
    );
}

/// Google's dialect: the token comes back as `pageToken`.
#[tokio::test]
async fn le_jeton_de_page_est_renvoye_sous_le_nom_du_dialecte() {
    let (client, seen, _) = engine(Arc::new(|line| {
        if line.contains("pageToken=t2") {
            (200, json!({"models": page(&["z"])}))
        } else {
            (200, json!({"models": page(&["y"]), "nextPageToken": "t2"}))
        }
    }))
    .await;
    let models = client.list_models().await.expect("liste");
    assert_eq!(ids(&models), ["y", "z"]);
    assert!(seen.lock().unwrap()[1].line.contains("pageToken=t2"));
}

/// Un moteur qui IGNORE le curseur répond toujours la même page avec `has_more`: la marche
/// s'arrête à la première page qui n'apporte rien, sans doublons, au lieu de boucler.
#[tokio::test]
async fn un_moteur_qui_ignore_le_curseur_ne_fait_pas_boucler_la_marche() {
    let (client, seen, _) = engine(Arc::new(|_| {
        (
            200,
            json!({"data": page(&["a", "b"]), "has_more": true, "last_id": "b"}),
        )
    }))
    .await;
    let models = client.list_models().await.expect("liste");
    assert_eq!(ids(&models), ["a", "b"], "sans doublon");
    assert_eq!(
        seen.lock().unwrap().len(),
        2,
        "la 2e page n'apporte rien : on s'arrête"
    );
}

/// Un lien `next` vers un AUTRE hôte n'est pas suivi : la clé d'API n'y part pas.
#[tokio::test]
async fn un_lien_vers_un_autre_hote_n_est_pas_suivi_et_la_cle_n_y_va_pas() {
    let foreign = Arc::new(Mutex::new(Vec::new()));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let foreign_origin = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(serve(
        listener,
        Arc::new(|_| (200, json!({"data": page(&["pirate"])}))),
        foreign.clone(),
    ));
    let link = format!("{foreign_origin}/steal");
    let (client, _, _) = engine(Arc::new(move |_| {
        (200, json!({"data": page(&["a"]), "next": link}))
    }))
    .await;
    let models = client.list_models().await.expect("liste");
    assert_eq!(ids(&models), ["a"]);
    assert!(
        foreign.lock().unwrap().is_empty(),
        "l'autre hôte n'a reçu AUCUNE requête (donc pas la clé)"
    );
}

/// Un lien `next` sur la MÊME origine est suivi, avec la clé.
#[tokio::test]
async fn un_lien_de_la_meme_origine_est_suivi_avec_la_cle() {
    let origin_cell: Arc<Mutex<String>> = Arc::new(Mutex::new(String::new()));
    let cell = origin_cell.clone();
    let (client, seen, origin) = engine(Arc::new(move |line| {
        if line.contains("p=2") {
            (200, json!({"data": page(&["b"])}))
        } else {
            let origin = cell.lock().unwrap().clone();
            (
                200,
                json!({"data": page(&["a"]), "next": format!("{origin}/v1/models?p=2")}),
            )
        }
    }))
    .await;
    *origin_cell.lock().unwrap() = origin;
    let models = client.list_models().await.expect("liste");
    assert_eq!(ids(&models), ["a", "b"]);
    assert!(
        seen.lock().unwrap().iter().all(|s| s.with_key),
        "la clé part à chaque page"
    );
}

/// Une page suivante qui échoue fait échouer la liste : un catalogue partiel passerait
/// pour un catalogue complet.
#[tokio::test]
async fn une_page_suivante_en_echec_fait_echouer_la_liste() {
    let (client, _, _) = engine(Arc::new(|line| {
        if line.contains("after_id=") {
            (503, json!({"error": "overloaded"}))
        } else {
            (
                200,
                json!({"data": page(&["a"]), "has_more": true, "last_id": "a"}),
            )
        }
    }))
    .await;
    assert!(client.list_models().await.is_err());
}

/// Un catalogue sans pagination (le cas courant) coûte UNE requête, comme avant.
#[tokio::test]
async fn un_catalogue_sans_pagination_coute_une_requete() {
    let (client, seen, _) = engine(Arc::new(|_| (200, json!({"data": page(&["a", "b"])})))).await;
    let models = client.list_models().await.expect("liste");
    assert_eq!(ids(&models), ["a", "b"]);
    assert_eq!(seen.lock().unwrap().len(), 1);
}

/// Le curseur CHANGE à chaque page mais les modèles sont toujours les mêmes : seul le garde
/// « une page qui n'apporte rien met fin à la marche » l'arrête (le garde « curseur répété »
/// ne le voit pas). Sans lui, la marche irait jusqu'à la borne de pages.
#[tokio::test]
async fn un_curseur_qui_change_sans_rien_apporter_met_fin_a_la_marche() {
    let (client, seen, _) = engine(Arc::new(|line| {
        let n: u32 = line
            .split("after_id=")
            .nth(1)
            .and_then(|rest| rest.split(['&', ' ']).next())
            .and_then(|n| n.parse().ok())
            .unwrap_or(0);
        (
            200,
            json!({"data": page(&["a", "b"]), "has_more": true, "last_id": (n + 1).to_string()}),
        )
    }))
    .await;
    let models = client.list_models().await.expect("liste");
    assert_eq!(ids(&models), ["a", "b"]);
    assert_eq!(
        seen.lock().unwrap().len(),
        2,
        "la 2e page n'apporte rien de neuf : on s'arrête là"
    );
}
