//! L'hôte de déploiement : une structure qu'on déploie, sur laquelle on
//! rattache des projets qui existent déjà.
//!
//! Ces tests vérifient ce qu'un **opérateur** observe : le plan publié, les
//! sondes de vie, l'absence de route inventée, et — le plus important — que
//! `--dry-run` **n'ouvre aucune base** : on relit une structure avant de la
//! déployer, on ne touche pas aux bases en service pour le faire.

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use this::deploy::DeploymentConfig;
use tower::ServiceExt;

/// Envoie une requête au routeur et rend le statut et le corps (JSON quand il y
/// en a un).
async fn request(router: &Router, method: &str, uri: &str) -> (StatusCode, serde_json::Value) {
    let response = router
        .clone()
        .oneshot(
            Request::builder()
                .method(method)
                .uri(uri)
                .body(Body::empty())
                .expect("requête construite"),
        )
        .await
        .expect("le routeur répond");
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), 1 << 20)
        .await
        .expect("corps lu");
    let json = serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
    (status, json)
}

fn memory_deployment(name: &str) -> String {
    format!("bind: 127.0.0.1:0\nattachments:\n  - name: {name}\n    store: {{ kind: memory }}\n")
}

#[tokio::test]
async fn the_host_publishes_the_structure_it_serves() {
    let config = DeploymentConfig::from_yaml(&memory_deployment("facturation")).expect("valide");
    let host = config.build().expect("structure construite");

    let (status, plan) = request(&host.router, "GET", "/").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(plan["structure"], "this-rs host");
    assert_eq!(
        plan["inspected"], true,
        "en service, les bases ont été ouvertes : le plan doit le dire"
    );
    assert_eq!(plan["attachments"][0]["name"], "facturation");
    assert_eq!(plan["attachments"][0]["mount"], "/attached/facturation");
    assert_eq!(plan["attachments"][0]["store"]["kind"], "memory");
    assert_eq!(plan["attachments"][0]["authorization"]["mode"], "enforce");

    // Ce que la structure **ne** rattache **pas** est publié, pas caché.
    let not_attached = plan["not_attached"]
        .as_array()
        .expect("une liste de limites");
    assert!(
        not_attached
            .iter()
            .any(|l| l.as_str().is_some_and(|l| l.contains("reverse proxy"))),
        "l'hôte ne proxifie pas, et il le dit : {not_attached:?}"
    );
}

#[tokio::test]
async fn the_host_is_alive_and_so_are_its_attachments() {
    let config = DeploymentConfig::from_yaml(&memory_deployment("crm")).expect("valide");
    let host = config.build().expect("structure construite");

    let (status, body) = request(&host.router, "GET", "/healthz").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["status"], "ok");
    assert_eq!(body["attachments"], 1);

    let (status, _) = request(&host.router, "GET", "/attached/crm/healthz").await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = request(&host.router, "GET", "/attached/crm/health").await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn the_host_invents_no_route_outside_a_declared_mount() {
    let config = DeploymentConfig::from_yaml(&memory_deployment("crm")).expect("valide");
    let host = config.build().expect("structure construite");

    for uri in ["/nope", "/graph", "/attached/inconnu/healthz"] {
        let (status, _) = request(&host.router, "GET", uri).await;
        assert_eq!(
            status,
            StatusCode::NOT_FOUND,
            "`{uri}` n'est déclaré nulle part : l'hôte ne l'invente pas"
        );
    }
}

#[test]
fn an_attached_project_brings_its_own_exposure_and_policies() {
    let dir = tempfile::tempdir().expect("répertoire temporaire");
    let policies = dir.path().join("links.yaml");
    std::fs::write(
        &policies,
        "entities:\n  - singular: invoice\n    plural: invoices\n    auth:\n      list: authenticated\n      delete: admin_only\nlinks: []\n",
    )
    .expect("écriture des politiques");
    let exposure = dir.path().join("server.yaml");
    std::fs::write(
        &exposure,
        "cors:\n  enabled: true\n  origins:\n    - https://app.example\ngraph:\n  policy: \"perm:graph:read\"\n",
    )
    .expect("écriture de l'exposition");

    let config = DeploymentConfig::from_yaml(&format!(
        "attachments:\n  - name: facturation\n    store: {{ kind: memory }}\n    policies: {}\n    exposure: {}\n",
        policies.display(),
        exposure.display()
    ))
    .expect("déploiement valide");

    let host = config.build().expect("structure construite");
    let attachment = &host.plan.attachments[0];

    // L'exposition du projet est **effectivement** posée, et publiée en faits
    // (comptes, jamais les listes).
    let exposure = attachment
        .exposure
        .as_ref()
        .expect("l'exposition est publiée");
    assert!(exposure.cors_enabled, "la CORS du projet est posée");
    assert_eq!(exposure.cors_origins, 1);
    assert!(!exposure.domains_guarded, "aucun domaine déclaré ici");

    // Ses politiques sont lues telles quelles, sans réécriture.
    assert_eq!(
        attachment.authorization.policies.as_deref(),
        Some(policies.to_string_lossy().as_ref())
    );
}

#[test]
fn dry_run_publishes_the_plan_without_opening_any_base() {
    let dir = tempfile::tempdir().expect("répertoire temporaire");
    let deploy = dir.path().join("deploy.yaml");
    let base = dir.path().join("inexistante.obrain");
    std::fs::write(
        &deploy,
        format!(
            "bind: 127.0.0.1:0\nattachments:\n  - name: facturation\n    store:\n      kind: obrain\n      path: {}\n",
            base.display()
        ),
    )
    .expect("écriture du déploiement");

    let output = std::process::Command::new(env!("CARGO_BIN_EXE_this-host"))
        .args(["--config", &deploy.to_string_lossy(), "--dry-run", "--json"])
        .output()
        .expect("le binaire démarre");

    assert!(
        output.status.success(),
        "--dry-run doit réussir sans base : {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let plan: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("le plan est du JSON");
    assert_eq!(
        plan["inspected"], false,
        "un plan sec ne prétend pas avoir inspecté les bases"
    );
    assert!(
        !base.exists(),
        "`--dry-run` ne doit pas créer la base : relire une structure ne touche à rien"
    );
    assert_eq!(plan["attachments"][0]["store"]["read_only"], true);
}

#[test]
fn the_binary_refuses_a_deployment_that_declares_an_absent_file() {
    let dir = tempfile::tempdir().expect("répertoire temporaire");
    let deploy = dir.path().join("deploy.yaml");
    std::fs::write(
        &deploy,
        "attachments:\n  - name: facturation\n    store: { kind: memory }\n    policies: /inexistant/links.yaml\n",
    )
    .expect("écriture du déploiement");

    let output = std::process::Command::new(env!("CARGO_BIN_EXE_this-host"))
        .args(["--config", &deploy.to_string_lossy(), "--dry-run"])
        .output()
        .expect("le binaire démarre");

    assert!(
        !output.status.success(),
        "un fichier déclaré et absent doit arrêter le démarrage"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("introuvable"),
        "message actionnable : {stderr}"
    );
}

#[cfg(feature = "obrain")]
mod obrain_attachment {
    use super::*;
    use this::storage::ObrainStore;

    /// Écrit une base « comme un autre outil » : un nœud à propriétés plates,
    /// sans convention `_uuid` — le cas d'un projet qui existe déjà.
    fn base_ecrite_par_un_autre_outil(dir: &std::path::Path) -> std::path::PathBuf {
        let base = dir.join("app.obrain");
        {
            let store = ObrainStore::open(&base).expect("base créée");
            let session = store.db().session();
            session.create_node_with_props(
                &["Page"],
                [(
                    "titre",
                    obrain_common::types::Value::from("Page écrite par un autre outil"),
                )],
            );
            store.commit_wal().expect("commit");
        }
        base
    }

    fn deploiement(base: &std::path::Path, mode: &str) -> String {
        format!(
            "bind: 127.0.0.1:0\nattachments:\n  - name: aletheia\n    store:\n      kind: obrain\n      path: {}\n      read_only: true\n    authorization:\n      mode: {mode}\n",
            base.display()
        )
    }

    /// **En mode `enforce`, le relief d'une base rattachée est refusé** tant que
    /// le projet n'a pas déclaré de `graph.policy` : une route que le résolveur
    /// ne reconnaît pas s'abstient, et une abstention est un refus. C'est
    /// fail-closed — et c'est exactement le 403 qu'un opérateur doit pouvoir
    /// comprendre sans lire le code : le plan l'annonce.
    #[tokio::test]
    async fn in_enforce_mode_an_undeclared_relief_is_refused_and_the_plan_says_why() {
        let dir = tempfile::tempdir().expect("répertoire temporaire");
        let base = base_ecrite_par_un_autre_outil(dir.path());

        let config = DeploymentConfig::from_yaml(&deploiement(&base, "enforce"))
            .expect("déploiement valide");
        let host = config.build().expect("structure construite");

        let (status, _) = request(&host.router, "GET", "/attached/aletheia/graph").await;
        assert_eq!(
            status,
            StatusCode::FORBIDDEN,
            "sans `graph.policy`, la lecture du graphe s'abstient — et l'abstention est un refus"
        );

        let warnings = &host.plan.attachments[0].warnings;
        assert!(
            warnings.iter().any(|w| w.contains("403")),
            "le plan doit expliquer le 403 et sa remède : {warnings:?}"
        );
    }

    /// **Rattacher un projet qui existe déjà pour le LIRE** : sa base est écrite
    /// par un autre outil, l'hôte l'ouvre en lecture seule et publie son relief.
    /// Le projet n'est pas réécrit, et sa base n'est pas modifiée.
    #[tokio::test]
    async fn an_existing_base_is_attached_read_only_and_its_relief_answers() {
        let dir = tempfile::tempdir().expect("répertoire temporaire");
        let base = base_ecrite_par_un_autre_outil(dir.path());

        // `off` : les lectures de graphe redeviennent ce qu'elles sont par
        // défaut dans le framework — descriptives et publiques. L'opérateur
        // l'écrit, et le plan le publie.
        let config =
            DeploymentConfig::from_yaml(&deploiement(&base, "off")).expect("déploiement valide");
        let host = config.build().expect("structure construite");

        let (status, relief) = request(&host.router, "GET", "/attached/aletheia/graph").await;
        assert_eq!(status, StatusCode::OK, "le relief de la base rattachée");
        assert!(
            relief["engine"].is_object() || relief["store"].is_object(),
            "le document de relief décrit le moteur et la base : {relief}"
        );

        // Le plan dit la vérité sur l'ouverture : lecture seule, donc aucune
        // décision IAM ne peut être matérialisée dans la base d'un autre.
        let attachment = &host.plan.attachments[0];
        assert!(attachment.store.read_only);
        assert!(
            attachment
                .warnings
                .iter()
                .any(|w| w.contains("matérialisée")),
            "la conséquence de la lecture seule est publiée : {:?}",
            attachment.warnings
        );
        assert!(
            attachment.warnings.iter().any(|w| w.contains("publiques")),
            "le relief est public : le plan le dit : {:?}",
            attachment.warnings
        );

        // Et l'hôte n'a rien écrit : le nœud de l'autre outil est toujours là,
        // et le compteur de nœuds de la base n'a pas bougé.
        let after = ObrainStore::open_read_only(&base).expect("relecture en lecture seule");
        assert_eq!(
            after.db().node_count(),
            1,
            "rattacher une base ne la modifie pas"
        );
    }
}
