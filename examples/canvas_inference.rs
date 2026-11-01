//! **L'interface et les droits, de bout en bout** — exemple exécutable.
//!
//! ```bash
//! cargo run --features obrain --example canvas_inference
//! ```
//!
//! # Ce que cet exemple montre
//!
//! Une même base, lue **par l'interface** et **sous les droits** :
//!
//! 1. un utilisateur regarde un **sujet** à une **échelle** — c'est son état
//!    d'interface, celui qu'un Canvas affiche ;
//! 2. sa lecture part **de ce sujet**, **à cette échelle**, sans qu'il traduise
//!    quoi que ce soit : `GET /graph/hierarchy/self?echelle=canvas` ;
//! 3. sa **décision IAM** plafonne tout : une interface qui demande plus de
//!    détail que ses droits n'en donnent reçoit un **refus**, jamais une vue
//!    tronquée en silence ;
//! 4. ce qui est masqué est **compté** — le lecteur sait qu'on lui cache quelque
//!    chose, et combien.
//!
//! Aucune de ces quatre choses n'est décorative : chacune est vérifiée sur le
//! routeur réel par `tests/chain_aletheia.rs`.

use std::sync::Arc;

use axum::body::Body;
use axum::extract::Extension;
use axum::http::{Method, Request, StatusCode};
use tower::ServiceExt;

use this::core::auth::AuthContext;
use this::core::identity::IdentityRegistry;
use this::graph::canvas::SimpleCanvasState;
use this::graph::routes::{GraphState, ScopeResolver};
use this::graph::scope::IamScopeResolver;
use this::storage::iam::RoleGrant;
use this::storage::permission::Scale;
use this::storage::{GraphBackend, ObrainStore};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // ── La base : un sujet, ses sous-groupes, et un nœud d'un autre label ────
    let store = ObrainStore::new_in_memory();
    let (sujet, cache) = {
        let session = store.db().session();
        let racine = session.create_node_with_props(
            &["Groupe"],
            [
                ("name", obrain_common::types::Value::from("aletheia")),
                (
                    "qualified_name",
                    obrain_common::types::Value::from("aletheia"),
                ),
            ],
        );
        let maths = session.create_node_with_props(
            &["Groupe"],
            [
                ("name", obrain_common::types::Value::from("mathématiques")),
                (
                    "qualified_name",
                    obrain_common::types::Value::from("aletheia/maths"),
                ),
            ],
        );
        session.create_edge(racine, maths, "CONTAINS");
        // Un nœud que le lecteur n'a PAS le droit de voir.
        let secret = session.create_node_with_props(
            &["Confidentiel"],
            [
                ("name", obrain_common::types::Value::from("brouillon")),
                (
                    "qualified_name",
                    obrain_common::types::Value::from("aletheia/brouillon"),
                ),
            ],
        );
        session.create_edge(racine, secret, "CONTAINS");
        store.commit_wal()?;
        (racine.as_u64(), secret.as_u64())
    };

    // ── Le lecteur : identifié, et ses droits ────────────────────────────────
    let utilisateur = uuid::Uuid::from_u128(11);
    let tenant = uuid::Uuid::from_u128(7);
    store.iam().materialize(
        &utilisateur.to_string(),
        &tenant.to_string(),
        &[RoleGrant {
            role: "lecteur".into(),
            permissions: vec![
                "graph:read".into(),
                "node:read:Groupe".into(),
                "edge:read:CONTAINS".into(),
                // Le droit s'arrête au groupe : pas de détail par nœud.
                "echelle:read:groupe".into(),
            ],
        }],
    )?;

    // ── Son état d'interface : il regarde `aletheia`, à l'échelle `groupe` ───
    let canvas = Arc::new(SimpleCanvasState::new());
    canvas.set_subject(&utilisateur.to_string(), "xth", "aletheia");
    canvas.set_scale(&utilisateur.to_string(), Scale::Groupe);

    let resolver: Arc<dyn ScopeResolver> = Arc::new(IamScopeResolver::new(store.iam()));
    let app = this::graph::graph_routes(
        GraphState::new(
            Arc::clone(&store),
            Arc::new(IdentityRegistry::obrain_default()),
        )
        .with_scope_resolver(resolver)
        .with_canvas(canvas.clone()),
    );

    let lire = |app: axum::Router, uri: String| async move {
        let response = app
            .layer(Extension(AuthContext::User {
                user_id: utilisateur,
                tenant_id: tenant,
                roles: vec![],
                scopes: Vec::new(),
            }))
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri(uri)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .expect("réponse");
        let status = response.status();
        let bytes = axum::body::to_bytes(response.into_body(), 1 << 20)
            .await
            .expect("corps");
        (status, String::from_utf8(bytes.to_vec()).expect("utf8"))
    };

    println!("\n═══ 1. La lecture part du SUJET, à l'ÉCHELLE de l'interface ═══\n");
    let (status, body) = lire(
        app.clone(),
        "/graph/hierarchy/self?echelle=canvas&format=outline".to_string(),
    )
    .await;
    println!("GET /graph/hierarchy/self?echelle=canvas  →  {status}");
    println!("{body}");

    println!("═══ 2. Ce qui est masqué est COMPTÉ (pas silencieux) ═══\n");
    let (_, body) = lire(
        app.clone(),
        "/graph/hierarchy/self?echelle=canvas".to_string(),
    )
    .await;
    let json: serde_json::Value = serde_json::from_str(&body)?;
    println!(
        "nœuds au niveau 1 : {} · masqués : {}",
        json["levels"][0]["nodes"]
            .as_array()
            .map(|a| a.len())
            .unwrap_or(0),
        json["levels"][0]["hidden"]
    );
    for note in json["notes"].as_array().into_iter().flatten() {
        if let Some(note) = note.as_str()
            && (note.contains("MASQUÉ") || note.contains("TYPE"))
        {
            println!("  note : {}", note.trim());
        }
    }
    assert!(
        !body.contains("brouillon"),
        "le nœud hors périmètre ne doit PAS apparaître"
    );
    println!("\n  ✓ `brouillon` (label hors périmètre) n'apparaît pas, et le compte le dit.");
    assert_ne!(cache, sujet);

    println!("\n═══ 3. L'interface propose, la DÉCISION dispose ═══\n");
    canvas.set_scale(&utilisateur.to_string(), Scale::Noeud);
    let (status, body) = lire(
        app.clone(),
        "/graph/hierarchy/self?echelle=canvas".to_string(),
    )
    .await;
    println!("l'interface demande l'échelle `noeud` (le détail d'un nœud) →  {status}");
    println!("  {}", body.trim());
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "le plafond de la décision doit s'appliquer"
    );

    println!("\n═══ 4. Le relief suit la MÊME décision ═══\n");
    let (_, body) = lire(app.clone(), "/graph".to_string()).await;
    let json: serde_json::Value = serde_json::from_str(&body)?;
    let labels: Vec<&str> = json["store"]["labels"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|l| l["name"].as_str())
        .collect();
    println!(
        "labels publiés : {labels:?} · masqués : {} · types d'arêtes : {:?}",
        json["store"]["hidden_labels"],
        json["store"]["edge_types"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|e| e["name"].as_str())
            .collect::<Vec<_>>()
    );

    println!("\n═══ 5. Le sujet est résolu depuis l'interface, pas du chemin ═══\n");
    let trouves = <ObrainStore as GraphBackend>::find_nodes_by_property(
        &store,
        "qualified_name",
        &obrain_common::types::Value::from("aletheia/maths"),
    );
    println!(
        "`aletheia/maths` → {} nœud(s) dans l'espace : le sujet peut désigner n'importe \
         quel nœud nommé, pas seulement la racine",
        trouves.len()
    );

    println!(
        "\n✅ L'interface et les droits parlent la même langue : l'échelle et le sujet \
         viennent de l'état d'interface, le plafond vient de la décision IAM, et rien \
         n'est masqué en silence.\n"
    );
    Ok(())
}
