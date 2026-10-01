#![cfg(all(feature = "obrain", feature = "wami", feature = "proxy"))]
//! CONTRAT du **rattachement déclaratif** — un projet qui existe déjà, joint
//! par la passerelle, sous sa garde.
//!
//! Ce que ce fichier verrouille, et pourquoi chaque ligne compte :
//!
//! | requête                                          | attendu |
//! |--------------------------------------------------|---------|
//! | `GET /services/viz/` sans jeton                   | 401 — **gardé par défaut** |
//! | `GET /services/viz/` avec le jeton du tenant      | 200 — le proxy transmet |
//! | l'amont reçoit `x-this-subject` / `x-this-tenant` | l'identité **vérifiée** |
//! | le client FORGE `x-this-tenant: …`                | la valeur vérifiée gagne |
//! | `GET /services/viz/api/query` avec jeton          | 403 — **refus déclaré** |
//! | `GET /services/viz/api/querylog` avec jeton       | 200 — la règle ne refuse pas plus large |
//! | `POST` sur un service `anonymous: true`           | 401 — jamais d'écriture anonyme |
//! | déclaration invalide (clé inconnue, mount réservé…)| refus de DÉMARRER |
//!
//! Le point le plus important est l'en-tête d'identité : un rattachement qui
//! transmettrait celui du client laisserait n'importe qui choisir son tenant.
//!
//! **Ports** : le gateway d'essai écoute sur 8146, et l'amont d'essai sur un
//! port **éphémère** (le noyau en choisit un) — deux tests du même binaire
//! tournent en parallèle, et un port fixe les ferait échouer l'un par l'autre.
//! Le service de production (:8144) et la matrice d'identités (:8145) ne sont
//! pas touchés.

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use axum::http::{HeaderMap, Method, Uri};
use this::core::LinkService;
use this::core::link::LinkEntity;
use this::storage::ObrainLinkService;
use this::storage::obrain::ObrainStore;
use uuid::Uuid;

const GW: &str = "127.0.0.1:8146";
const TENANT: &str = "7d2e1f30-4a5b-4c6d-8e9f-0a1b2c3d4e5f";
const AUTRE_TENANT: &str = "11111111-2222-3333-4444-555555555555";
const MDP: &str = "mot-de-passe-de-test-4821";

fn base() -> String {
    format!("http://{GW}")
}

/// Le gateway d'essai : tué au drop, même si un assert panique avant la fin.
struct Gardien(Child);

impl Drop for Gardien {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn lance_gateway(args: &[String]) -> Gardien {
    Gardien(
        Command::new(env!("CARGO_BIN_EXE_this-gateway"))
            .args(args)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("lancement du gateway d'essai"),
    )
}

/// **L'amont d'essai** : le « projet qui existe déjà ». Il rend ce qu'il a
/// reçu — méthode, chemin, requête et **ses en-têtes `x-this-*`** — pour qu'un
/// test puisse prouver ce qui lui arrive vraiment.
async fn amont(headers: HeaderMap, methode: Method, uri: Uri) -> axum::Json<serde_json::Value> {
    let mut vus = serde_json::Map::new();
    for (nom, valeur) in headers.iter() {
        if nom.as_str().starts_with("x-this-") {
            vus.insert(
                nom.to_string(),
                serde_json::json!(valeur.to_str().unwrap_or("").to_string()),
            );
        }
    }
    axum::Json(serde_json::json!({
        "vus": vus,
        "methode": methode.as_str(),
        "chemin": uri.path(),
        "requete": uri.query(),
    }))
}

/// Lance l'amont sur un port éphémère et rend son URL.
async fn lance_amont() -> String {
    let app = axum::Router::new().fallback(amont);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind de l'amont d'essai");
    let adresse = listener.local_addr().expect("adresse de l'amont");
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    format!("http://{adresse}")
}

async fn attendre_pret(client: &reqwest::Client) {
    let debut = Instant::now();
    loop {
        if let Ok(r) = client.get(format!("{}/health", base())).send().await
            && r.status() == 200
        {
            return;
        }
        assert!(
            debut.elapsed() < Duration::from_secs(30),
            "le gateway d'essai n'est pas devenu prêt en 30 s"
        );
        tokio::time::sleep(Duration::from_millis(150)).await;
    }
}

/// Crée un store obrain de test avec un nœud témoin, puis relâche le verrou.
async fn cree_store_de_test(chemin: &Path) {
    let store = ObrainStore::open(chemin).expect("création du store de test");
    let liens = ObrainLinkService::new(store);
    liens
        .create(LinkEntity::new(
            "temoin",
            Uuid::new_v4(),
            Uuid::new_v4(),
            None,
        ))
        .await
        .expect("écriture du nœud témoin");
}

/// Écrit une déclaration complète, avec les services qu'on veut tester.
fn ecrit_declaration(racine: &Path, amont: &str, services: &str) -> PathBuf {
    let chemin = racine.join("deploy.yaml");
    let contenu = format!(
        "addr: {GW}\n\
         keys: {cles}\n\
         identites: {identites}\n\
         issuer: test-rattachement\n\
         \n\
         stores:\n\
         \x20 - name: prive\n\
         \x20   path: {prive}\n\
         \x20   tenant: {TENANT}\n\
         \n\
         services:\n{services}",
        cles = racine.join("cles").display(),
        identites = racine.join("identites.obrain").display(),
        prive = racine.join("prive.obrain").display(),
    )
    .replace("UPSTREAM", amont);
    std::fs::write(&chemin, contenu).expect("écriture de la déclaration");
    chemin
}

async fn terrain(amont: &str, services: &str) -> (tempfile::TempDir, Vec<String>) {
    let racine = tempfile::tempdir().expect("tempdir");
    cree_store_de_test(&racine.path().join("prive.obrain")).await;
    let declaration = ecrit_declaration(racine.path(), amont, services);
    let args = vec![
        "--config".to_string(),
        declaration.display().to_string(),
        "--user".to_string(),
        format!("admin:{MDP}:{TENANT}:admin"),
    ];
    (racine, args)
}

async fn jeton_du_tenant(client: &reqwest::Client) -> String {
    let reponse: serde_json::Value = client
        .post(format!("{}/auth/token", base()))
        .json(&serde_json::json!({"username": "admin", "password": MDP}))
        .send()
        .await
        .expect("POST /auth/token")
        .json()
        .await
        .expect("paire de jetons");
    reponse["access_token"]
        .as_str()
        .expect("access_token")
        .to_string()
}

/// **Le rattachement complet** : gardé, borné, et transmettant l'identité
/// vérifiée. Une seule séquence par port — le gateway est arrêté entre les deux
/// phases (déclaration gardée, puis déclaration `anonymous: true`).
#[tokio::test]
async fn le_rattachement_est_garde_borne_et_transmet_l_identite_verifiee() {
    let amont = lance_amont().await;
    let client = reqwest::Client::new();

    // ── Phase A : un service gardé, avec un refus déclaré ──
    {
        let (_racine, args) = terrain(
            &amont,
            "\x20 - name: viz\n\x20   upstream: UPSTREAM\n\x20   deny: [\"/api/query\"]\n",
        )
        .await;
        let _gw = lance_gateway(&args);
        attendre_pret(&client).await;

        // 1. Sans jeton : refusé. Un rattachement n'ouvre rien par défaut.
        let r = client
            .get(format!("{}/services/viz/", base()))
            .send()
            .await
            .unwrap();
        assert_eq!(
            r.status(),
            401,
            "un service rattaché est gardé par défaut — sinon rattacher, c'est exposer"
        );

        // 2. Avec le jeton du tenant : transmis à l'amont.
        let jeton = jeton_du_tenant(&client).await;
        let r = client
            .get(format!("{}/services/viz/tableau?onglet=relief", base()))
            .bearer_auth(&jeton)
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), 200, "jeton valide = transmis à l'amont");
        let vu: serde_json::Value = r.json().await.unwrap();
        assert_eq!(vu["chemin"], "/tableau", "l'amont voit le chemin DÉPRÉFIXÉ");
        assert_eq!(vu["requete"], "onglet=relief", "la requête est préservée");
        assert_eq!(
            vu["vus"]["x-this-tenant"], TENANT,
            "l'amont reçoit le tenant VÉRIFIÉ — c'est ce qui rend le rattachement utile"
        );

        // 3. Un en-tête d'identité FORGÉ par le client ne passe pas.
        let r = client
            .get(format!("{}/services/viz/", base()))
            .bearer_auth(&jeton)
            .header("x-this-tenant", AUTRE_TENANT)
            .header("x-this-subject", "moi-le-patron")
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), 200);
        let vu: serde_json::Value = r.json().await.unwrap();
        assert_eq!(
            vu["vus"]["x-this-tenant"], TENANT,
            "l'identité forgée est ÉCRASÉE par la vérification : {vu}"
        );
        assert_ne!(
            vu["vus"]["x-this-subject"], "moi-le-patron",
            "un client ne s'attribue pas son propre sujet : {vu}"
        );

        // 4. Les refus déclarés, et leur portée exacte.
        let r = client
            .get(format!("{}/services/viz/api/query", base()))
            .bearer_auth(&jeton)
            .send()
            .await
            .unwrap();
        assert_eq!(
            r.status(),
            403,
            "`/api/query` est refusé par la déclaration"
        );
        let corps: serde_json::Value = r.json().await.unwrap();
        assert_eq!(corps["path"], "/api/query", "le refus dit ce qu'il refuse");

        let r = client
            .get(format!("{}/services/viz/api/querylog", base()))
            .bearer_auth(&jeton)
            .send()
            .await
            .unwrap();
        assert_eq!(
            r.status(),
            200,
            "une règle qui refuserait `/api/querylog` refuserait plus large que ce qu'elle écrit"
        );

        let r = client
            .get(format!("{}/services/viz/api/query/plan", base()))
            .bearer_auth(&jeton)
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), 403, "la règle couvre ses sous-chemins");
    }
    // Le gateway d'essai est arrêté ici (Drop du gardien) : le port est libéré.

    // ── Phase B : le même service, déclaré `anonymous: true` ──
    {
        let (_racine, args) = terrain(
            &amont,
            "\x20 - name: viz\n\x20   upstream: UPSTREAM\n\x20   anonymous: true\n",
        )
        .await;
        let _gw = lance_gateway(&args);
        attendre_pret(&client).await;

        let r = client
            .get(format!("{}/services/viz/", base()))
            .send()
            .await
            .unwrap();
        assert_eq!(
            r.status(),
            200,
            "déclaré anonyme : la lecture passe sans jeton"
        );
        let vu: serde_json::Value = r.json().await.unwrap();
        assert!(
            vu["vus"].get("x-this-tenant").is_none(),
            "un anonyme n'a pas d'identité à transmettre : {vu}"
        );

        let r = client
            .post(format!("{}/services/viz/quelque-chose", base()))
            .send()
            .await
            .unwrap();
        assert_eq!(
            r.status(),
            401,
            "jamais d'écriture anonyme, même sur un service déclaré ouvert"
        );
    }
}

/// **Une déclaration refusée ne démarre pas** — et le dit.
#[test]
fn une_declaration_invalide_refuse_le_demarrage() {
    let racine = tempfile::tempdir().expect("tempdir");

    let cas: Vec<(&str, &str)> = vec![
        (
            "clé inconnue (faute de frappe)",
            "addr: 127.0.0.1:8146\nkeys: /tmp/x\nservices:\n  - name: v\n    upstream: http://127.0.0.1:8147\n    anonyme: true\n",
        ),
        (
            "upstream sans http",
            "keys: /tmp/x\nservices:\n  - name: v\n    upstream: 127.0.0.1:8147\n",
        ),
        (
            "mount réservé à la passerelle",
            "keys: /tmp/x\nservices:\n  - name: v\n    upstream: http://127.0.0.1:8147\n    mount: /auth/pirate\n",
        ),
        (
            "anonyme ET tenant (contradictoire)",
            "keys: /tmp/x\nservices:\n  - name: v\n    upstream: http://127.0.0.1:8147\n    anonymous: true\n    tenant: 7d2e1f30-4a5b-4c6d-8e9f-0a1b2c3d4e5f\n",
        ),
        (
            "tenant illisible",
            "keys: /tmp/x\nstores:\n  - name: s\n    path: /tmp/s.obrain\n    tenant: pas-un-uuid\n",
        ),
        (
            "refus sans slash",
            "keys: /tmp/x\nservices:\n  - name: v\n    upstream: http://127.0.0.1:8147\n    deny: [\"api/query\"]\n",
        ),
        (
            "deux services sur le même mount",
            "keys: /tmp/x\nservices:\n  - name: a\n    upstream: http://127.0.0.1:8147\n    mount: /services/x\n  - name: b\n    upstream: http://127.0.0.1:8148\n    mount: /services/x\n",
        ),
    ];

    for (etiquette, contenu) in cas {
        let chemin = racine.path().join("deploy.yaml");
        std::fs::write(&chemin, contenu).expect("écriture");
        let sortie = Command::new(env!("CARGO_BIN_EXE_this-gateway"))
            .args(["--config", &chemin.display().to_string()])
            .output()
            .expect("le binaire démarre");
        assert!(
            !sortie.status.success(),
            "`{etiquette}` doit refuser le démarrage"
        );
        let stderr = String::from_utf8_lossy(&sortie.stderr);
        assert!(
            !stderr.trim().is_empty(),
            "`{etiquette}` : le refus doit être explicable sur la sortie d'erreur"
        );
    }
}

/// **Le plan écrit des artefacts, et ne déploie rien.**
///
/// Chaque fichier porte son état (« préparé, non déployé ») : un générateur qui
/// laisserait croire qu'il a installé quelque chose serait pire que pas de
/// générateur du tout.
#[test]
fn le_plan_ecrit_les_artefacts_et_ne_deploie_rien() {
    let racine = tempfile::tempdir().expect("tempdir");
    let declaration = racine.path().join("deploy.yaml");
    let contenu = "addr: 192.168.1.103:8144\n\
             keys: /home/triviere/.config/this-gateway/cles\n\
             identites: /home/triviere/.local/share/this-gateway/identites.obrain\n\
             issuer: obrain-n3\n\
             \n\
             stores:\n\
             \x20 - name: linux\n\
             \x20   path: /tmp/linux-full3.obrain\n\
             \x20   tenant: public\n\
             \n\
             services:\n\
             \x20 - name: viz\n\
             \x20   upstream: http://192.168.1.103:8432\n\
             \x20   deny: [\"/api/query\"]\n\
             \n\
             expose:\n\
             \x20 acme_email: quelqu-un@exemple.fr\n\
             \x20 gateway: 192.168.1.103:8144\n\
             \x20 sites:\n\
             \x20   - host: ffs.space\n\
             \x20     aliases: [www.ffs.space]\n\
             \x20     service: viz\n\
             \x20   - host: viz.ffs.space\n\
             \x20     service: viz\n\
             \x20     mode: forward_auth\n\
             \x20     tunnel: true\n\
             \x20     tunnel_host: triviere@192.168.1.103\n\
             \x20 dns:\n\
             \x20   target: 82.65.152.87\n\
             \x20   record_type: A\n\
             \x20   ttl: 300\n";
    std::fs::write(&declaration, contenu).expect("écriture de la déclaration");

    let sortie = Command::new(env!("CARGO_BIN_EXE_this-gateway"))
        .args([
            "plan",
            "--config",
            &declaration.display().to_string(),
            "--out",
            &racine.path().join("plan").display().to_string(),
            "--bin",
            "/opt/this-rs/this-gateway",
        ])
        .output()
        .expect("plan");
    assert!(
        sortie.status.success(),
        "plan doit réussir : {}",
        String::from_utf8_lossy(&sortie.stderr)
    );

    let plan = racine.path().join("plan");
    let lire = |relatif: &str| -> String {
        std::fs::read_to_string(plan.join(relatif))
            .unwrap_or_else(|e| panic!("artefact manquant `{relatif}` : {e}"))
    };

    // ── L'unité systemd : la déclaration remplace la ligne de commande ──
    let unite = lire("this-gateway.service");
    assert!(
        unite.contains("PRÉPARÉ, NON DÉPLOYÉ"),
        "chaque artefact dit son état"
    );
    assert!(
        unite.contains(&format!("--config {}", declaration.display())),
        "l'unité sert la déclaration, pas une ligne de commande : {unite}"
    );
    assert!(
        !unite.contains("--store"),
        "aucun store n'est écrit en dur : la déclaration fait foi"
    );
    assert!(
        unite.contains("LimitNOFILE"),
        "la limite de descripteurs se pose dans l'unité, pas en production"
    );
    assert!(
        !unite.contains("MDP") && !unite.contains("motdepasse"),
        "un mot de passe ne doit jamais entrer dans une unité"
    );

    // ── Les blocs Caddy : par la passerelle, ou gardés par la porte ──
    let via_gateway = lire("caddy/ffs.space.caddy");
    assert!(
        via_gateway.contains("reverse_proxy 192.168.1.103:8144"),
        "tout passe par la passerelle : {via_gateway}"
    );
    assert!(
        via_gateway.contains("www.ffs.space"),
        "les alias sont dans le même bloc"
    );

    let par_la_porte = lire("caddy/viz.ffs.space.caddy");
    assert!(
        par_la_porte.contains("forward_auth 192.168.1.103:8144"),
        "la façade interroge la porte : {par_la_porte}"
    );
    assert!(par_la_porte.contains("uri /auth/verify"));
    assert!(
        par_la_porte.contains("copy_headers x-this-subject x-this-tenant x-this-roles"),
        "l'identité vérifiée est recopiée vers le projet"
    );
    assert!(
        par_la_porte.contains("reverse_proxy http://192.168.1.103:8432"),
        "le mode `forward_auth` joint le projet en direct"
    );
    assert!(
        par_la_porte.contains("respond @interdit 403"),
        "les refus déclarés sont aussi dans la façade (défense en profondeur)"
    );

    // ── Le tunnel : vers la passerelle, pas vers chaque projet ──
    let tunnel = lire("systemd/this-rs-tunnel-viz-ffs-space.service");
    assert!(tunnel.contains("triviere@192.168.1.103"));
    assert!(
        tunnel.contains("127.0.0.1:8144:192.168.1.103:8144"),
        "le tunnel joint la passerelle : un seul chemin à connaître"
    );
    assert!(tunnel.contains("ExitOnForwardFailure=yes"));

    // ── Le DNS : un plan, jamais une écriture ──
    let dns = lire("dns.txt");
    assert!(
        dns.contains("ffs.space."),
        "nom qualifié avec son point : {dns}"
    );
    assert!(dns.contains("82.65.152.87"));
    assert!(dns.contains("A\t300") || dns.contains("300\tA"));
    assert!(
        dns.contains("jamais MX"),
        "le plan dit ce qu'il ne touchera jamais"
    );

    // ── Le runbook : l'état, les gestes, la vérification, le retrait ──
    let runbook = lire("RUNBOOK.md");
    assert!(runbook.contains("PRÉPARÉ, RIEN DE DÉPLOYÉ"));
    assert!(
        runbook.contains("## Retrait"),
        "un déploiement sans retrait n'en est pas un"
    );
    for attendu in ["401", "200", "403", "/auth/verify", "x-this"] {
        assert!(
            runbook.contains(attendu),
            "le runbook donne la vérification (`{attendu}`)"
        );
    }

    // ── Le plan en JSON se dit non déployé ──
    let json = Command::new(env!("CARGO_BIN_EXE_this-gateway"))
        .args([
            "plan",
            "--config",
            &declaration.display().to_string(),
            "--json",
        ])
        .output()
        .expect("plan --json");
    let plan_json: serde_json::Value =
        serde_json::from_slice(&json.stdout).expect("le plan est du JSON");
    assert_eq!(plan_json["deployed"], false);
    assert_eq!(plan_json["services"][0]["name"], "viz");
    assert_eq!(plan_json["services"][0]["guard"], "authenticated");
    assert_eq!(plan_json["services"][0]["deny"][0], "/api/query");
}

/// **Une exposition incomplète refuse la génération** : un Caddyfile à moitié
/// décrit est une panne qui attend son heure.
#[test]
fn une_exposition_incomplete_refuse_la_generation() {
    let racine = tempfile::tempdir().expect("tempdir");
    let cas: Vec<(&str, &str)> = vec![
        (
            "expose sans acme_email",
            "keys: /tmp/x\nservices:\n  - name: v\n    upstream: http://127.0.0.1:8147\nexpose:\n  acme_email: \"\"\n  gateway: 1.2.3.4:8144\n  sites:\n    - host: a.fr\n",
        ),
        (
            "site sans site",
            "keys: /tmp/x\nservices:\n  - name: v\n    upstream: http://127.0.0.1:8147\nexpose:\n  acme_email: a@b.fr\n  gateway: 1.2.3.4:8144\n  sites: []\n",
        ),
        (
            "site pointant un service inexistant",
            "keys: /tmp/x\nservices:\n  - name: v\n    upstream: http://127.0.0.1:8147\nexpose:\n  acme_email: a@b.fr\n  gateway: 1.2.3.4:8144\n  sites:\n    - host: a.fr\n      service: inconnu\n",
        ),
        (
            "tunnel sans cible",
            "keys: /tmp/x\nservices:\n  - name: v\n    upstream: http://127.0.0.1:8147\nexpose:\n  acme_email: a@b.fr\n  gateway: 1.2.3.4:8144\n  sites:\n    - host: a.fr\n      tunnel: true\n",
        ),
        (
            "mode inconnu",
            "keys: /tmp/x\nservices:\n  - name: v\n    upstream: http://127.0.0.1:8147\nexpose:\n  acme_email: a@b.fr\n  gateway: 1.2.3.4:8144\n  sites:\n    - host: a.fr\n      mode: direct\n",
        ),
        (
            "hôte avec un schéma",
            "keys: /tmp/x\nservices:\n  - name: v\n    upstream: http://127.0.0.1:8147\nexpose:\n  acme_email: a@b.fr\n  gateway: 1.2.3.4:8144\n  sites:\n    - host: https://a.fr\n",
        ),
        (
            "DNS hors types gérés",
            "keys: /tmp/x\nservices:\n  - name: v\n    upstream: http://127.0.0.1:8147\nexpose:\n  acme_email: a@b.fr\n  gateway: 1.2.3.4:8144\n  sites:\n    - host: a.fr\n  dns:\n    target: 1.2.3.4\n    record_type: TXT\n",
        ),
    ];

    for (etiquette, contenu) in cas {
        let chemin = racine.path().join("deploy.yaml");
        std::fs::write(&chemin, contenu).expect("écriture");
        let sortie = Command::new(env!("CARGO_BIN_EXE_this-gateway"))
            .args(["plan", "--config", &chemin.display().to_string()])
            .output()
            .expect("plan");
        assert!(
            !sortie.status.success(),
            "`{etiquette}` doit refuser la génération"
        );
        let stderr = String::from_utf8_lossy(&sortie.stderr);
        assert!(
            !stderr.trim().is_empty(),
            "`{etiquette}` : refus explicable"
        );
    }
}

/// **Le prévol voit le port déjà tenu** — un constat faux vaut moins que pas de
/// constat : une première version annonçait « port libre » devant un service en
/// marche, à cause d'un motif `awk` qui ne matchait rien.
#[tokio::test]
async fn le_prevol_voit_un_port_deja_tenu() {
    let ecouteur = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("écouteur d'essai");
    let adresse = ecouteur.local_addr().expect("adresse");

    let racine = tempfile::tempdir().expect("tempdir");
    let declaration = racine.path().join("deploy.yaml");
    std::fs::write(
        &declaration,
        format!(
            "addr: {adresse}\nkeys: {}\nidentites: {}\n",
            "/tmp/cles-essai", "/tmp/identites-essai"
        ),
    )
    .expect("écriture");

    let sortie = Command::new(env!("CARGO_BIN_EXE_this-gateway"))
        .args([
            "prevol",
            "--config",
            &declaration.display().to_string(),
            "--bin",
            "/bin/sh",
        ])
        .output()
        .expect("prevol");

    let stdout = String::from_utf8_lossy(&sortie.stdout);
    let ligne_port = stdout
        .lines()
        .find(|l| l.starts_with("port"))
        .unwrap_or_default()
        .to_string();
    assert!(
        ligne_port.contains("MANQUE") && ligne_port.contains("tenu par autre chose"),
        "le prévol doit voir l'écouteur : {ligne_port}"
    );
    assert!(
        !sortie.status.success(),
        "un MANQUE doit sortir en erreur — sinon un prévol vert ne veut rien dire"
    );
    let _ = ecouteur;
}

/// **Le prévol dit OK quand tout est vrai** : clés, store d'identités, base,
/// binaire — et un port libre.
#[test]
fn le_prevol_dit_ok_quand_tout_est_vrai() {
    let racine = tempfile::tempdir().expect("tempdir");
    let cles = racine.path().join("cles");
    let identites = racine.path().join("identites.obrain");
    let base = racine.path().join("base.obrain");
    std::fs::create_dir_all(&cles).expect("clés");
    std::fs::create_dir_all(&identites).expect("identités");
    std::fs::create_dir_all(&base).expect("base");
    std::fs::write(cles.join("gateway.key.pem"), "cle-de-test").expect("clé privée");
    std::fs::write(cles.join("gateway.pub.pem"), "cle-de-test").expect("clé publique");

    let declaration = racine.path().join("deploy.yaml");
    std::fs::write(
        &declaration,
        format!(
            "addr: 127.0.0.1:58999\nkeys: {}\nidentites: {}\nstores:\n  - name: essai\n    path: {}\n    tenant: public\n",
            cles.display(),
            identites.display(),
            base.display()
        ),
    )
    .expect("écriture");

    let sortie = Command::new(env!("CARGO_BIN_EXE_this-gateway"))
        .args([
            "prevol",
            "--config",
            &declaration.display().to_string(),
            "--bin",
            "/bin/sh",
        ])
        .output()
        .expect("prevol");

    let stdout = String::from_utf8_lossy(&sortie.stdout);
    assert!(
        sortie.status.success(),
        "tout est vrai : le prévol doit passer\n{stdout}\n{}",
        String::from_utf8_lossy(&sortie.stderr)
    );
    assert!(
        stdout.contains("les jetons émis survivent au redémarrage"),
        "le prévol dit ce que les clés présentes garantissent : {stdout}"
    );
    assert!(
        !stdout.contains("MANQUE"),
        "aucun constat ne doit manquer : {stdout}"
    );
}

/// **Les deux défauts qu'une répétition avec un vrai Caddy a trouvés**, et que
/// la relecture n'avait pas vus :
///
/// 1. en mode `forward_auth`, aucune route ne menait au **login** — une garde
///    sans porte d'entrée n'est pas une garde, c'est une fermeture : le
///    navigateur n'obtenait jamais son cookie ;
/// 2. en mode `via_gateway`, la racine du site allait au **404 de la
///    passerelle** (`/` n'est pas une de ses routes) : il faut préfixer vers le
///    montage du service, sinon la page d'accueil d'un site exposé est un 404.
#[test]
fn les_blocs_de_facade_portent_le_login_et_le_prefixe_du_service() {
    let racine = tempfile::tempdir().expect("tempdir");
    let declaration = racine.path().join("deploy.yaml");
    std::fs::write(
        &declaration,
        "addr: 127.0.0.1:8298\n\
         keys: /tmp/cles\n\
         identites: /tmp/identites\n\
         services:\n\
         \x20 - name: viz\n\
         \x20   upstream: http://192.168.1.103:8432\n\
         \x20   deny: [\"/api/query\"]\n\
         expose:\n\
         \x20 acme_email: a@b.fr\n\
         \x20 gateway: 127.0.0.1:8298\n\
         \x20 sites:\n\
         \x20   - host: par-la-porte.invalid\n\
         \x20     service: viz\n\
         \x20     mode: forward_auth\n\
         \x20   - host: par-la-passerelle.invalid\n\
         \x20     service: viz\n",
    )
    .expect("écriture");

    let plan = racine.path().join("plan");
    let sortie = Command::new(env!("CARGO_BIN_EXE_this-gateway"))
        .args([
            "plan",
            "--config",
            &declaration.display().to_string(),
            "--out",
            &plan.display().to_string(),
        ])
        .output()
        .expect("plan");
    assert!(
        sortie.status.success(),
        "{}",
        String::from_utf8_lossy(&sortie.stderr)
    );

    let porte = std::fs::read_to_string(plan.join("caddy/par-la-porte.invalid.caddy"))
        .expect("bloc forward_auth");
    assert!(
        porte.contains("@passerelle path /auth/*"),
        "sans route de login, le navigateur n'obtient jamais de cookie : {porte}"
    );
    assert!(
        porte.contains("uri /auth/verify"),
        "la porte reste interrogée : {porte}"
    );
    assert!(
        porte.contains("route {"),
        "l'ordre est porteur de sens : il doit être imposé par `route` : {porte}"
    );
    assert!(
        porte.contains("respond @interdit 403"),
        "les refus déclarés sont reposés dans la façade : {porte}"
    );

    let passerelle = std::fs::read_to_string(plan.join("caddy/par-la-passerelle.invalid.caddy"))
        .expect("bloc via_gateway");
    assert!(
        passerelle.contains("rewrite * /services/viz{uri}"),
        "la racine du site doit être préfixée vers le montage du service, sinon c'est un 404 : {passerelle}"
    );
    assert!(
        passerelle.contains("@passerelle path /auth/*"),
        "le login doit rester servi par la passerelle : {passerelle}"
    );
}

/// **Le port tenu par la passerelle sous son ANCIEN nom** : le prévol doit dire
/// « c'est un redémarrage », pas « tenu par autre chose ». Un faux `MANQUE`
/// bloquerait un déploiement qui est en réalité sans risque — le miroir exact du
/// faux `OK` (port annoncé libre devant un service en marche) que le tour
/// précédent avait corrigé. Les deux erreurs sont symétriques, les deux sont
/// testées.
#[tokio::test]
async fn le_prevol_reconnait_la_passerelle_sous_son_ancien_nom() {
    // Un exécutable **nommé** `this-gateway` qui tient un port : `ss` rend le
    // nom du binaire, c'est donc ce que le prévol voit.
    let atelier = tempfile::tempdir().expect("tempdir");
    let stub = atelier.path().join("this-gateway");
    let python = std::fs::canonicalize("/usr/bin/python3")
        .or_else(|_| std::fs::canonicalize("/usr/local/bin/python3"))
        .expect("un interpréteur python3 à copier");
    std::fs::copy(&python, &stub).expect("copie du binaire");

    let mut enfant = Command::new(&stub)
        .args([
            "-c",
            "import socket,time; s=socket.socket(); s.bind(('127.0.0.1',58998)); s.listen(1); time.sleep(60)",
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("le stub démarre");

    // Attendre que le port soit réellement tenu — et le **VÉRIFIER**.
    //
    // La version précédente sortait de la boucle au bout de 10 s **sans
    // regarder** si le stub avait pris le port : sur une machine chargée (suite
    // complète, plusieurs binaires de test en parallèle), le prévol voyait un
    // port LIBRE et le test accusait le prévol d'un défaut qui n'existait pas.
    // C'est arrivé une fois. Un échec doit nommer sa cause, sinon il apprend à
    // ignorer les rouges.
    let debut = Instant::now();
    let mut tenu = false;
    while Instant::now().duration_since(debut) < Duration::from_secs(20) {
        if std::net::TcpStream::connect("127.0.0.1:58998").is_ok() {
            tenu = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(
        tenu,
        "le stub `this-gateway` n'a pas pris le port 58998 en 20 s : c'est \
         l'ENVIRONNEMENT du test (machine chargée), pas un défaut du prévol"
    );

    let declaration = atelier.path().join("deploy.yaml");
    std::fs::write(
        &declaration,
        "addr: 127.0.0.1:58998\nkeys: /tmp/cles-essai\nidentites: /tmp/identites-essai\n",
    )
    .expect("écriture");

    let sortie = Command::new(env!("CARGO_BIN_EXE_this-gateway"))
        .args([
            "prevol",
            "--config",
            &declaration.display().to_string(),
            "--bin",
            "/bin/sh",
        ])
        .output()
        .expect("prevol");
    let _ = enfant.kill();
    let _ = enfant.wait();

    let stdout = String::from_utf8_lossy(&sortie.stdout);
    let ligne_port = stdout
        .lines()
        .find(|l| l.starts_with("port"))
        .unwrap_or_default()
        .to_string();
    assert!(
        ligne_port.contains("OK") && ligne_port.contains("REDÉMARRAGE"),
        "un port tenu par un processus nommé `this-gateway` est un redémarrage, pas un conflit : {ligne_port}"
    );
    assert!(
        !ligne_port.contains("MANQUE"),
        "un faux MANQUE bloquerait un déploiement sans risque : {ligne_port}"
    );
}

/// **Un flux traversant ne doit pas être tamponné.**
///
/// `reponse.bytes()` attendait la fin du corps : un projet qui parle SSE se
/// serait **figé** à travers la passerelle, sans erreur levée — le rattachement
/// aurait paru cassé, et rien ne l'aurait dit. Ce test lit la réponse en **TCP
/// brut** (donc incrémentalement, comme un navigateur) : le premier événement
/// doit arriver **avant** que l'amont n'ait fini d'écrire.
// `multi_thread` est **nécessaire** : ce test lit la réponse en TCP brut
// (bloquant), et sur un runtime mono-thread il affamerait l'amont qu'il a
// lui-même lancé — la passerelle attendrait une réponse que le test empêche de
// produire. Le premier jet de ce test échouait exactement là.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn un_flux_traversant_n_est_pas_tamponne() {
    use std::io::{BufRead, BufReader, Write};

    // Amont : deux événements SSE, séparés de 700 ms.
    let app = axum::Router::new().route(
        "/flux",
        axum::routing::get(|| async {
            let flux = futures_util::stream::unfold(0u8, |etape| async move {
                match etape {
                    0 => {
                        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                        Some((
                            Ok::<_, std::convert::Infallible>(axum::body::Bytes::from(
                                "data: premier\n\n",
                            )),
                            1u8,
                        ))
                    }
                    1 => {
                        tokio::time::sleep(std::time::Duration::from_millis(700)).await;
                        Some((Ok(axum::body::Bytes::from("data: second\n\n")), 2u8))
                    }
                    _ => None,
                }
            });
            (
                [("content-type", "text/event-stream")],
                axum::body::Body::from_stream(flux),
            )
        }),
    );
    let ecouteur = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("amont");
    let amont = format!("http://{}", ecouteur.local_addr().expect("adresse"));
    tokio::spawn(async move {
        let _ = axum::serve(ecouteur, app).await;
    });

    // **Un port à lui** (8148) : le port 8146 est celui de la matrice de
    // rattachement, et un second gateway sur le même port fait échouer l'un des
    // deux tests quand ils tournent en parallèle — ce qui est arrivé.
    const GW_FLUX: &str = "127.0.0.1:8148";
    let racine = tempfile::tempdir().expect("tempdir");
    let declaration = racine.path().join("deploy.yaml");
    std::fs::write(
        &declaration,
        format!(
            "addr: {GW_FLUX}\nkeys: {}\nidentites: {}\nservices:\n  - name: flux\n    upstream: {amont}\n",
            racine.path().join("cles").display(),
            racine.path().join("identites.obrain").display(),
        ),
    )
    .expect("écriture");
    let _gw = lance_gateway(&[
        "--config".to_string(),
        declaration.display().to_string(),
        "--user".to_string(),
        format!("admin:{MDP}:{TENANT}:admin"),
    ]);
    let client = reqwest::Client::new();

    let base_flux = format!("http://{GW_FLUX}");
    let debut_attente = Instant::now();
    loop {
        if let Ok(r) = client.get(format!("{base_flux}/health")).send().await
            && r.status() == 200
        {
            break;
        }
        assert!(
            debut_attente.elapsed() < Duration::from_secs(30),
            "le gateway d'essai du flux n'est pas devenu prêt"
        );
        tokio::time::sleep(Duration::from_millis(150)).await;
    }
    let jeton: serde_json::Value = client
        .post(format!("{base_flux}/auth/token"))
        .json(&serde_json::json!({"username": "admin", "password": MDP}))
        .send()
        .await
        .expect("login")
        .json()
        .await
        .expect("paire de jetons");
    let jeton = jeton["access_token"]
        .as_str()
        .expect("access_token")
        .to_string();

    // Lecture incrémentale : on note l'instant du PREMIER événement.
    let debut = Instant::now();
    let mut socket = std::net::TcpStream::connect(GW_FLUX).expect("connexion à la passerelle");
    socket
        .set_read_timeout(Some(Duration::from_secs(5)))
        .expect("délai de lecture");
    write!(
        socket,
        "GET /services/flux/flux HTTP/1.1\r\nHost: {GW_FLUX}\r\nAuthorization: Bearer {jeton}\r\nConnection: close\r\n\r\n"
    )
    .expect("requête écrite");

    let mut lecteur = BufReader::new(socket);
    let mut premiere_ligne = String::new();
    lecteur.read_line(&mut premiere_ligne).expect("statut");
    assert!(
        premiere_ligne.contains("200"),
        "le flux doit être accepté : {premiere_ligne}"
    );

    let mut vu_premier = None;
    let mut vu_second = None;
    let mut tampon = String::new();
    while lecteur.read_line(&mut tampon).is_ok() {
        if tampon.contains("premier") && vu_premier.is_none() {
            vu_premier = Some(debut.elapsed());
        }
        if tampon.contains("second") {
            vu_second = Some(debut.elapsed());
            break;
        }
        tampon.clear();
    }

    let premier = vu_premier.expect("le premier événement doit arriver");
    let second = vu_second.expect("le second événement doit arriver");
    assert!(
        premier < Duration::from_millis(650),
        "le premier événement est arrivé à {premier:?} : il a donc été TAMPONNÉ, \
         pas transmis (le second n'est écrit qu'à 750 ms)"
    );
    assert!(
        second >= Duration::from_millis(650),
        "le second événement vient bien de l'amont : {second:?}"
    );
}

/// **Le script de bascule** : ses invariants sont dans l'artefact, pas dans un
/// commentaire de commit. Le retour arrière doit être **écrit** — un runbook qui
/// le décrit sans l'exécuter laisse l'opérateur seul au pire moment.
///
/// (Son exécution a été répétée sur cette machine, les deux chemins : nominal et
/// retour arrière forcé par un port occupé. Ici, on vérifie ce que l'artefact
/// contient.)
#[test]
fn le_script_de_bascule_porte_le_retour_arriere_et_les_invariants() {
    let racine = tempfile::tempdir().expect("tempdir");
    let declaration = racine.path().join("deploy.yaml");
    std::fs::write(
        &declaration,
        "addr: 127.0.0.1:8144\n\
         keys: /home/triviere/.config/this-rs-gateway/cles\n\
         identites: /home/triviere/.local/share/this-rs-gateway/identites.obrain\n\
         services:\n\
         \x20 - name: viz\n\
         \x20   upstream: http://192.168.1.103:8432\n",
    )
    .expect("écriture");

    let plan = racine.path().join("plan");
    let sortie = Command::new(env!("CARGO_BIN_EXE_this-gateway"))
        .args([
            "plan",
            "--config",
            &declaration.display().to_string(),
            "--out",
            &plan.display().to_string(),
            // **`--bin` explicite** : le script refuse de démarrer si le binaire
            // déclaré n'est pas exécutable (« rien n'a été touché »). On lui donne
            // celui que ce test vient de construire, pour que le `--sec`
            // ci-dessous soit déterministe — et non tributaire d'un
            // `cargo build --release` antérieur sur la machine.
            "--bin",
            env!("CARGO_BIN_EXE_this-gateway"),
        ])
        .output()
        .expect("plan");
    assert!(
        sortie.status.success(),
        "{}",
        String::from_utf8_lossy(&sortie.stderr)
    );

    let script = std::fs::read_to_string(plan.join("basculer.sh")).expect("basculer.sh");
    assert!(
        script.contains("PRÉPARÉ, NON DÉPLOYÉ"),
        "l'artefact dit son état"
    );
    assert!(
        script.contains("UNITE=\"this-gateway.service\""),
        "le nouveau nom est dans le script : {script}"
    );
    assert!(
        script.contains("ANCIENNE=\"this-rs-gateway.service\""),
        "l'ancien nom aussi : c'est lui qu'on arrête, et qu'on relance si la nouvelle \
         ne devient pas saine"
    );
    assert!(
        script.contains("systemctl --user restart $ANCIENNE"),
        "le retour arrière doit être ÉCRIT : {script}"
    );

    // ── L'ORDRE : arrêter l'ancienne AVANT de démarrer la nouvelle ─────────
    //
    // Ce n'est pas une préférence de style : le store d'identités s'ouvre en
    // verrou **exclusif** — `deux_passerelles_ne_partagent_pas_le_store_didentites`
    // le mesure avec deux passerelles réelles. Démarrer la nouvelle en laissant
    // l'ancienne tourner échouerait à l'ouverture du store, la santé ne
    // viendrait jamais, et le script déclencherait un retour arrière à CHAQUE
    // bascule. Un script qui ne peut pas réussir n'est pas déployable.
    let ligne_arret = script
        .lines()
        .position(|l| l.contains("stop $ANCIENNE"))
        .expect("le script arrête l'ancienne unité");
    let ligne_demarrage = script
        .lines()
        .position(|l| l.contains("start $UNITE"))
        .expect("le script démarre la nouvelle");
    assert!(
        ligne_arret < ligne_demarrage,
        "l'ancienne doit être arrêtée AVANT que la nouvelle démarre (verrou exclusif du \
         store d'identités) — arrêt ligne {ligne_arret}, démarrage ligne {ligne_demarrage} : \
         {script}"
    );
    assert!(
        script.contains("verrou EXCLUSIF") || script.contains("verrou exclusif"),
        "l'artefact doit DIRE pourquoi l'ordre est celui-là : sans la raison, la prochaine \
         relecture le « corrigera » en ordre sans coupure — qui ne peut pas fonctionner — \
         et la bascule échouera sans que personne ne comprenne : {script}"
    );
    assert!(
        script.starts_with("#!/bin/sh\n"),
        "le shebang doit être la PREMIÈRE ligne, sinon `./basculer.sh` ne s'exécute pas : {script}"
    );
    assert!(
        script.contains("ÉCHEC"),
        "l'échec doit être nommé, pas silencieux"
    );
    assert!(
        script.contains("--sec"),
        "un mode à sec, pour relire ce que le script ferait"
    );
    assert!(
        script.contains("ne touche NI les clés"),
        "les invariants sont écrits DANS l'artefact"
    );
    // Les chemins qui portent des données sont nommés (pour être épargnés), et
    // rien dans le script ne les supprime.
    assert!(script.contains("this-rs-gateway/cles"));
    assert!(script.contains("identites.obrain"));
    for destructif in ["rm -rf", "rm -f \"$BIN", "mv \"$"] {
        assert!(
            !script.contains(destructif),
            "le script ne doit rien détruire (`{destructif}`) : {script}"
        );
    }

    // ── Le script est-il du SHELL VALIDE ? ────────────────────────────────
    //
    // Les assertions ci-dessus lisent le TEXTE ; aucune ne l'exécutait. Un
    // artefact qu'on ne peut pas lancer tel quel n'est pas déployable — le
    // shebang avait déjà été pris en défaut une fois, la **syntaxe** l'a été
    // une seconde : la commande d'installation portait une substitution
    // imbriquée mal échappée, et `sh -c` répondait « Unterminated quoted
    // string ». La bascule échouait donc à son PREMIER geste, en production.
    let analyse = Command::new("sh")
        .arg("-n")
        .arg(plan.join("basculer.sh"))
        .output()
        .expect("sh -n");
    assert!(
        analyse.status.success(),
        "`sh -n basculer.sh` doit passer — un script de bascule illisible par le shell \
         ne bascule rien : {}",
        String::from_utf8_lossy(&analyse.stderr)
    );

    // Et **chaque commande du mode à sec** doit être du shell valide, telle que
    // `faire` la ré-exécutera (`sh -c`) : c'est là que l'échappement se joue.
    let a_sec = Command::new("sh")
        .arg(plan.join("basculer.sh"))
        .arg("--sec")
        .output()
        .expect("sh basculer.sh --sec");
    assert!(
        a_sec.status.success(),
        "le mode à sec ne doit rien exécuter et sortir 0 : {}",
        String::from_utf8_lossy(&a_sec.stderr)
    );
    let sortie = String::from_utf8_lossy(&a_sec.stdout).to_string();
    let mut commandes = 0usize;
    for ligne in sortie.lines() {
        let Some(commande) = ligne.trim().strip_prefix("[à sec] ") else {
            continue;
        };
        commandes += 1;
        let valide = Command::new("sh")
            .arg("-n")
            .arg("-c")
            .arg(commande)
            .output()
            .expect("sh -n -c");
        assert!(
            valide.status.success(),
            "la commande que `faire` exécutera doit être du shell valide — \
             `{commande}` : {}",
            String::from_utf8_lossy(&valide.stderr)
        );
    }
    assert!(
        commandes >= 3,
        "le mode à sec doit montrer les gestes réels (installer, démarrer, arrêter \
         l'ancienne) : {commandes} commande(s) vue(s) dans\n{sortie}"
    );

    // ── Le chemin du binaire doit être ABSOLU, jamais un `%h` ─────────────
    //
    // `%h` n'est développé que par systemd. Dans `basculer.sh` — du shell — il
    // restait littéral : `[ -x "%h/projects/…" ]` échouait toujours, et la
    // bascule refusait au premier geste sur une machine où le binaire était
    // installé. Même conséquence pour le prévol, qui annonçait `binaire
    // MANQUE`. Le défaut ne se voyait qu'en REGARDANT le script, jamais en le
    // lisant : on le regarde donc ici, sans `--bin`.
    let plan_sans_bin = racine.path().join("plan-sans-bin");
    let sortie = Command::new(env!("CARGO_BIN_EXE_this-gateway"))
        .args([
            "plan",
            "--config",
            &declaration.display().to_string(),
            "--out",
            &plan_sans_bin.display().to_string(),
        ])
        .output()
        .expect("plan");
    assert!(
        sortie.status.success(),
        "{}",
        String::from_utf8_lossy(&sortie.stderr)
    );
    let script_defaut =
        std::fs::read_to_string(plan_sans_bin.join("basculer.sh")).expect("basculer.sh");
    let ligne_bin = script_defaut
        .lines()
        .find(|l| l.starts_with("BIN="))
        .expect("le script nomme son binaire");
    assert!(
        !ligne_bin.contains("%h"),
        "le binaire du script doit être un chemin ABSOLU : `%h` n'est pas développé \
         par le shell, donc la bascule refuserait toujours — {ligne_bin}"
    );
    assert!(
        ligne_bin.contains("/target/release/this-gateway"),
        "…et il doit désigner le binaire de la passerelle : {ligne_bin}"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(plan.join("basculer.sh"))
            .expect("métadonnées")
            .permissions()
            .mode();
        assert!(
            mode & 0o111 != 0,
            "le script doit être exécutable (mode {mode:o})"
        );
    }
}

/// **Les coins du contrat de rattachement** : ce qui arrive à un projet réel qui
/// répond mal, qui reçoit un `HEAD`, ou à qui on envoie un corps énorme.
///
/// Un proxy qui « corrige » les erreurs de l'amont ment sur ce qui se passe ; un
/// proxy qui accepte n'importe quelle taille est un amplificateur. Les deux sont
/// vérifiés ici sur un amont réel, à travers un gateway réel.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn les_coins_du_rattachement_sont_tenus() {
    const GW_COINS: &str = "127.0.0.1:8149";

    // Amont : 503 sur /casse, 200 sur /ok, et il rend la méthode qu'il a reçue.
    let app = axum::Router::new()
        .route(
            "/casse",
            axum::routing::get(|| async {
                (
                    axum::http::StatusCode::SERVICE_UNAVAILABLE,
                    "amont indisponible (c'est SON message)",
                )
            }),
        )
        .route(
            "/ok",
            axum::routing::any(|methode: axum::http::Method| async move {
                format!("methode={methode}")
            }),
        );
    let ecouteur = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("amont");
    let amont = format!("http://{}", ecouteur.local_addr().expect("adresse"));
    tokio::spawn(async move {
        let _ = axum::serve(ecouteur, app).await;
    });

    let racine = tempfile::tempdir().expect("tempdir");
    let declaration = racine.path().join("deploy.yaml");
    std::fs::write(
        &declaration,
        format!(
            "addr: {GW_COINS}\nkeys: {}\nidentites: {}\nservices:\n  - name: coins\n    upstream: {amont}\n",
            racine.path().join("cles").display(),
            racine.path().join("identites.obrain").display(),
        ),
    )
    .expect("écriture");
    let _gw = lance_gateway(&[
        "--config".to_string(),
        declaration.display().to_string(),
        "--user".to_string(),
        format!("admin:{MDP}:{TENANT}:admin"),
    ]);
    let client = reqwest::Client::new();
    let base_coins = format!("http://{GW_COINS}");
    let debut = Instant::now();
    loop {
        if let Ok(r) = client.get(format!("{base_coins}/health")).send().await
            && r.status() == 200
        {
            break;
        }
        assert!(
            debut.elapsed() < Duration::from_secs(30),
            "gateway non prêt"
        );
        tokio::time::sleep(Duration::from_millis(150)).await;
    }
    let jeton: serde_json::Value = client
        .post(format!("{base_coins}/auth/token"))
        .json(&serde_json::json!({"username": "admin", "password": MDP}))
        .send()
        .await
        .expect("login")
        .json()
        .await
        .expect("jetons");
    let jeton = jeton["access_token"].as_str().unwrap().to_string();

    // 1. Une erreur de l'amont traverse TELLE QUELLE : le proxy ne la réécrit pas.
    let r = client
        .get(format!("{base_coins}/services/coins/casse"))
        .bearer_auth(&jeton)
        .send()
        .await
        .unwrap();
    assert_eq!(
        r.status(),
        503,
        "un 503 de l'amont doit rester un 503 (pas un 502 « bad gateway » inventé)"
    );
    let corps = r.text().await.unwrap();
    assert!(
        corps.contains("SON message"),
        "le corps de l'amont traverse aussi : {corps}"
    );

    // 2. `HEAD` traverse comme méthode (et sans corps).
    let r = client
        .head(format!("{base_coins}/services/coins/ok"))
        .bearer_auth(&jeton)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200, "HEAD doit traverser");

    // 3. Un corps au-delà de la borne est REFUSÉ (8 Mio) — un proxy qui accepte
    //    n'importe quelle taille est un amplificateur.
    let enorme = "x".repeat(9 * 1024 * 1024);
    let r = client
        .post(format!("{base_coins}/services/coins/ok"))
        .bearer_auth(&jeton)
        .body(enorme)
        .send()
        .await
        .unwrap();
    assert_eq!(
        r.status(),
        413,
        "un corps de 9 Mio doit être refusé par la borne déclarée (8 Mio)"
    );
    let corps: serde_json::Value = r.json().await.unwrap();
    assert_eq!(corps["error"], "payload_too_large");
}
