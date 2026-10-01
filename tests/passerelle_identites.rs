#![cfg(all(feature = "obrain", feature = "wami"))]
//! CONTRAT du chantier STS-PERSISTANT — écrit AVANT l'implémentation.
//!
//! La passerelle devient digne d'un service public sur deux axes :
//!
//! **A. Utilisateurs persistés** : les identités vivent dans un store obrain
//! dédié (`--identites <store>`), nœuds `User {username, hash_argon2,
//! tenant_id, roles, created_at}`, mots de passe hachés argon2id. Le flag
//! `--user` reste un SEED de bootstrap : au premier démarrage l'utilisateur
//! est créé AVEC hash dans le store — plus jamais de clair persistant — puis
//! le store fait foi.
//!
//! **B. Anonyme sur public** : une requête SANS jeton sur un store
//! `tenant=public` répond 200 en LECTURE SEULE (GET uniquement). Toute
//! écriture anonyme, tout store à tenant, tout store inconnu : 401 — on ne
//! renseigne pas un anonyme sur l'existence des stores.
//!
//! # La matrice (chaque ligne est un assert HTTP réel)
//!
//! | requête                                    | attendu |
//! |--------------------------------------------|---------|
//! | GET  /stores/publie/graph  sans jeton      | 200     |
//! | GET  /stores/prive/graph   sans jeton      | 401     |
//! | PUT  /stores/publie/graph  sans jeton      | 401     |
//! | POST /stores/publie/links  sans jeton      | 401     |
//! | GET  /stores/inconnu/graph sans jeton      | 401     |
//! | POST /auth/token bon mot de passe          | 200 + cookie `this_token` (`HttpOnly`, `SameSite=Lax`) |
//! | GET  /stores/prive/graph   cookie de session | 200   |
//! | GET  /stores/prive/graph   cookie forgé     | 401    |
//! | POST /auth/token mauvais mot de passe      | 401     |
//! | GET  /stores/prive/graph   avec jeton      | 200     |
//! | GET  /stores/publie/graph  avec jeton      | 200     |
//! | POST /auth/users sans jeton                | 403     |
//! | POST /auth/users avec jeton admin          | 201     |
//! | redémarrage SANS --user → login            | 200     |
//!
//! Et les invariants hors HTTP : le clair n'apparaît JAMAIS dans les octets
//! du store d'identités (le hash argon2id, si) ; les utilisateurs survivent
//! au redémarrage du gateway d'ESSAI (kill du process de test — jamais du
//! service systemd de production).
//!
//! Gateway d'essai : 127.0.0.1:8145 (le service de production écoute sur
//! :8144 et n'est pas touché). Stores de test dans un tempdir, tués avec lui.

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use this::core::LinkService;
use this::core::link::LinkEntity;
use this::storage::ObrainLinkService;
use this::storage::obrain::ObrainStore;
use uuid::Uuid;

const ADRESSE: &str = "127.0.0.1:8145";
const TENANT: &str = "3f1c2a84-5b6d-4e7f-8a90-1b2c3d4e5f60";
const MDP_ADMIN: &str = "sesame-tres-secret-9482";
const MDP_CAROLE: &str = "mdp-carole-tres-secret-123";

fn base() -> String {
    format!("http://{ADRESSE}")
}

/// Le gateway d'essai : tué au drop, même si un assert panique avant la fin.
struct GardienGateway(Child);

impl Drop for GardienGateway {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn lance_gateway(args: &[String]) -> GardienGateway {
    let child = Command::new(env!("CARGO_BIN_EXE_this-gateway"))
        .args(args)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("lancement du gateway d'essai");
    GardienGateway(child)
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

/// Balaye récursivement les fichiers d'un store et rend `true` si `aiguille`
/// apparaît dans les octets d'au moins un fichier.
fn octets_contiennent(racine: &Path, aiguille: &[u8]) -> bool {
    fn visite(chemin: &Path, aiguille: &[u8], trouve: &mut bool) {
        if *trouve {
            return;
        }
        if chemin.is_dir() {
            if let Ok(entrees) = std::fs::read_dir(chemin) {
                for e in entrees.flatten() {
                    visite(&e.path(), aiguille, trouve);
                }
            }
        } else if let Ok(octets) = std::fs::read(chemin)
            && octets
                .windows(aiguille.len())
                .any(|fenetre| fenetre == aiguille)
        {
            *trouve = true;
        }
    }
    let mut trouve = false;
    visite(racine, aiguille, &mut trouve);
    trouve
}

async fn jeton(client: &reqwest::Client, username: &str, password: &str) -> reqwest::Response {
    client
        .post(format!("{}/auth/token", base()))
        .json(&serde_json::json!({"username": username, "password": password}))
        .send()
        .await
        .expect("POST /auth/token")
}

struct Terrain {
    _racine: tempfile::TempDir,
    identites: PathBuf,
    args_communs: Vec<String>,
}

async fn prepare_terrain() -> Terrain {
    let racine = tempfile::tempdir().expect("tempdir");
    let publie = racine.path().join("publie-test.obrain");
    let prive = racine.path().join("prive-test.obrain");
    let identites = racine.path().join("identites-test.obrain");
    let cles = racine.path().join("cles");

    cree_store_de_test(&publie).await;
    cree_store_de_test(&prive).await;

    let args_communs = vec![
        "--addr".into(),
        ADRESSE.into(),
        "--keys".into(),
        cles.display().to_string(),
        "--identites".into(),
        identites.display().to_string(),
        "--store".into(),
        format!("publie={},tenant=public", publie.display()),
        "--store".into(),
        format!("prive={},tenant={TENANT}", prive.display()),
    ];
    Terrain {
        _racine: racine,
        identites,
        args_communs,
    }
}

/// LA matrice — un seul test séquentiel : le port 8145 n'est pas partageable.
#[tokio::test]
async fn matrice_identites_persistees_et_anonyme_public() {
    let terrain = prepare_terrain().await;
    let client = reqwest::Client::new();

    // ── Premier démarrage : --user est un SEED (créé haché dans le store) ──
    let mut args = terrain.args_communs.clone();
    args.push("--user".into());
    args.push(format!("admin:{MDP_ADMIN}:{TENANT}:admin"));
    let gw = lance_gateway(&args);
    attendre_pret(&client).await;

    // B. Anonyme : GET sur store public → 200, tout le reste → 401.
    let r = client
        .get(format!("{}/stores/publie/graph", base()))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200, "anonyme + GET + store public = 200");

    let r = client
        .get(format!("{}/stores/prive/graph", base()))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 401, "anonyme + store à tenant = 401");

    let r = client
        .put(format!("{}/stores/publie/graph", base()))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 401, "anonyme + écriture, même sur public = 401");

    let r = client
        .post(format!("{}/stores/publie/links", base()))
        .json(&serde_json::json!({}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 401, "anonyme + POST, même sur public = 401");

    let r = client
        .get(format!("{}/stores/inconnu/graph", base()))
        .send()
        .await
        .unwrap();
    assert_eq!(
        r.status(),
        401,
        "un anonyme n'apprend pas l'existence des stores (401, pas 404)"
    );

    // A. Le login vient du store persisté (seed --user haché au bootstrap).
    let r = jeton(&client, "admin", MDP_ADMIN).await;
    assert_eq!(r.status(), 200, "login du seed --user = 200");
    // Le login pose AUSSI un cookie de session : sans lui, aucune surface
    // rattachée n'est atteignable depuis un navigateur.
    let cookie_session = r
        .headers()
        .get(reqwest::header::SET_COOKIE)
        .expect("POST /auth/token pose un cookie de session")
        .to_str()
        .expect("cookie lisible")
        .to_string();
    assert!(
        cookie_session.starts_with("this_token="),
        "{cookie_session}"
    );
    assert!(cookie_session.contains("HttpOnly"), "{cookie_session}");
    assert!(cookie_session.contains("SameSite=Lax"), "{cookie_session}");
    assert!(
        !cookie_session.contains("Secure"),
        "requête en clair : `Secure` ferait perdre la session au premier clic"
    );
    let jeton_cookie = cookie_session
        .split(';')
        .next()
        .and_then(|paire| paire.split_once('='))
        .map(|(_, valeur)| valeur.to_string())
        .expect("valeur du cookie");
    let paire: serde_json::Value = r.json().await.unwrap();
    let acces = paire["access_token"]
        .as_str()
        .expect("access_token")
        .to_string();

    let r = jeton(&client, "admin", "mauvais-mot-de-passe").await;
    assert_eq!(r.status(), 401, "mauvais mot de passe = 401");

    let r = jeton(&client, "fantome", MDP_ADMIN).await;
    assert_eq!(r.status(), 401, "utilisateur inconnu = 401");

    // L'authentifié du bon tenant lit son store privé, et le public.
    let r = client
        .get(format!("{}/stores/prive/graph", base()))
        .bearer_auth(&acces)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200, "jeton du tenant propriétaire = 200");

    let r = client
        .get(format!("{}/stores/publie/graph", base()))
        .bearer_auth(&acces)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200, "l'authentifié garde l'accès au public");

    // ── Le cookie ouvre la porte comme le jeton — et ne l'ouvre pas à un forgé ──
    let r = client
        .get(format!("{}/stores/prive/graph", base()))
        .header(
            reqwest::header::COOKIE,
            format!("this_token={jeton_cookie}"),
        )
        .send()
        .await
        .unwrap();
    assert_eq!(
        r.status(),
        200,
        "le cookie de session porte l'identité (c'est ce qui rend une page utilisable)"
    );

    let r = client
        .get(format!("{}/stores/prive/graph", base()))
        .header(
            reqwest::header::COOKIE,
            "this_token=eyJhbGciOiJFZERTQSJ9.forge.signature",
        )
        .send()
        .await
        .unwrap();
    assert_eq!(
        r.status(),
        401,
        "un cookie forgé est VÉRIFIÉ comme un jeton — jamais cru sur parole"
    );

    let r = client
        .get(format!("{}/stores/prive/graph", base()))
        .header(reqwest::header::COOKIE, "this_token=")
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 401, "un cookie vide n'est pas un cookie");

    // Administration HTTP : sans jeton → refus ; admin → création.
    let corps_carole = serde_json::json!({
        "username": "carole",
        "password": MDP_CAROLE,
        "tenant_id": TENANT,
        "roles": ["lecteur"],
    });
    let r = client
        .post(format!("{}/auth/users", base()))
        .json(&corps_carole)
        .send()
        .await
        .unwrap();
    assert_eq!(
        r.status(),
        403,
        "création d'utilisateur sans jeton admin = 403"
    );

    let r = client
        .post(format!("{}/auth/users", base()))
        .bearer_auth(&acces)
        .json(&corps_carole)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 201, "création d'utilisateur par un admin = 201");

    let r = jeton(&client, "carole", MDP_CAROLE).await;
    assert_eq!(r.status(), 200, "l'utilisateur créé à chaud se connecte");

    let r = client
        .get(format!("{}/auth/users", base()))
        .bearer_auth(&acces)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    let liste = r.text().await.unwrap();
    assert!(liste.contains("carole"), "la liste montre carole");
    assert!(
        !liste.contains("argon2") && !liste.contains(MDP_CAROLE),
        "la liste ne divulgue ni hash ni clair"
    );

    // ── Kill du gateway d'ESSAI (jamais du service de production). ──
    drop(gw);

    // Le clair n'est JAMAIS persisté ; le hash argon2id, si.
    assert!(
        !octets_contiennent(&terrain.identites, MDP_ADMIN.as_bytes()),
        "le mot de passe du seed ne doit pas apparaître dans le store"
    );
    assert!(
        !octets_contiennent(&terrain.identites, MDP_CAROLE.as_bytes()),
        "le mot de passe créé à chaud ne doit pas apparaître dans le store"
    );
    assert!(
        octets_contiennent(&terrain.identites, b"$argon2id$"),
        "le store contient des hashes argon2id"
    );

    // ── Redémarrage SANS --user : le store fait foi, tout le monde survit. ──
    let gw = lance_gateway(&terrain.args_communs);
    attendre_pret(&client).await;

    let r = jeton(&client, "admin", MDP_ADMIN).await;
    assert_eq!(r.status(), 200, "admin survit au redémarrage sans --user");

    let r = jeton(&client, "carole", MDP_CAROLE).await;
    assert_eq!(r.status(), 200, "carole survit au redémarrage sans --user");

    let r = jeton(&client, "admin", "mauvais-mot-de-passe").await;
    assert_eq!(r.status(), 401, "le refus survit aussi");

    drop(gw); // le gardien tue le gateway d'essai
}

/// L'administration hors ligne : `this-gateway user add/list/passwd`.
/// Le store d'identités est ouvert en exclusif : ces commandes s'exécutent
/// gateway éteint (ou sur un autre chemin) — c'est documenté, pas caché.
#[test]
fn cli_user_add_list_passwd() {
    let racine = tempfile::tempdir().expect("tempdir");
    let identites = racine.path().join("identites-cli.obrain");
    let bin = env!("CARGO_BIN_EXE_this-gateway");

    let ajout = Command::new(bin)
        .args([
            "user",
            "add",
            &format!("bob:mdp-bob-tres-secret-456:{TENANT}:lecteur"),
            "--identites",
            &identites.display().to_string(),
        ])
        .output()
        .expect("user add");
    assert!(
        ajout.status.success(),
        "user add doit réussir : {}",
        String::from_utf8_lossy(&ajout.stderr)
    );

    let liste = Command::new(bin)
        .args([
            "user",
            "list",
            "--identites",
            &identites.display().to_string(),
        ])
        .output()
        .expect("user list");
    assert!(liste.status.success());
    let sortie = String::from_utf8_lossy(&liste.stdout).to_string();
    assert!(sortie.contains("bob"), "user list montre bob : {sortie}");
    assert!(
        sortie.contains("$argon2id$"),
        "user list montre le hash argon2id (préfixe PHC) : {sortie}"
    );
    assert!(
        !sortie.contains("mdp-bob-tres-secret-456"),
        "user list ne montre jamais un clair"
    );

    let passwd = Command::new(bin)
        .args([
            "user",
            "passwd",
            "bob:nouveau-mdp-tres-secret-789",
            "--identites",
            &identites.display().to_string(),
        ])
        .output()
        .expect("user passwd");
    assert!(
        passwd.status.success(),
        "user passwd doit réussir : {}",
        String::from_utf8_lossy(&passwd.stderr)
    );

    let liste2 = Command::new(bin)
        .args([
            "user",
            "list",
            "--identites",
            &identites.display().to_string(),
        ])
        .output()
        .expect("user list après passwd");
    let sortie2 = String::from_utf8_lossy(&liste2.stdout).to_string();
    assert!(sortie2.contains("$argon2id$"));
    assert_ne!(
        sortie, sortie2,
        "le hash change quand le mot de passe change"
    );
    assert!(
        !octets_contiennent(&identites, b"nouveau-mdp-tres-secret-789"),
        "le nouveau clair n'est pas persisté non plus"
    );
}

/// **La contrainte qui fixe l'ORDRE de la bascule : le store d'identités
/// s'ouvre en verrou EXCLUSIF.**
///
/// Mesuré ici, avec deux passerelles réelles et **un seul** store d'identités :
/// la seconde ne démarre pas — « already open by another process … exclusive
/// lock on substrate.meta refused ». C'est pourquoi `basculer.sh` arrête
/// l'ancienne unité **avant** de démarrer la nouvelle : l'ordre « démarrer,
/// attendre la santé, arrêter ensuite » (celui qui éviterait toute coupure)
/// échouerait à **chaque** bascule et déclencherait un retour arrière à chaque
/// fois. La coupure est réelle mais brève : la nouvelle répond sur `/health` en
/// 0,2 s (bases chaudes) à 3,3 s (froid) — mesuré, 4 bases déclarées.
///
/// Si un jour obrain permet la coexistence, ce test tombe — et l'ordre sans
/// coupure redevient possible. Il faudra alors le **reposer**, pas le supposer.
///
/// Ports 8147/8148 : `8145` appartient à la matrice, et un port ne se partage
/// pas plus qu'un store.
#[tokio::test]
async fn deux_passerelles_ne_partagent_pas_le_store_didentites() {
    let racine = tempfile::tempdir().expect("tempdir");
    let publie = racine.path().join("publie-verrou.obrain");
    let identites = racine.path().join("identites-verrou.obrain");
    let cles = racine.path().join("cles");
    cree_store_de_test(&publie).await;

    // Le store d'identités est créé par la CLI — chemin éprouvé, et il évite de
    // dépendre du seed `--user` pour que la première passerelle démarre.
    let creation = Command::new(env!("CARGO_BIN_EXE_this-gateway"))
        .args([
            "user",
            "add",
            &format!("bob:mdp-bob-tres-secret-456:{TENANT}:lecteur"),
            "--identites",
            &identites.display().to_string(),
        ])
        .output()
        .expect("user add");
    assert!(
        creation.status.success(),
        "le store d'identités doit se créer : {}",
        String::from_utf8_lossy(&creation.stderr)
    );

    let args = |addr: &str| -> Vec<String> {
        vec![
            "--addr".into(),
            addr.into(),
            "--keys".into(),
            cles.display().to_string(),
            "--identites".into(),
            identites.display().to_string(),
            "--store".into(),
            format!("publie={},tenant=public", publie.display()),
        ]
    };

    // 1. La première prend le verrou et sert.
    let _premiere = lance_gateway(&args("127.0.0.1:8147"));
    let client = reqwest::Client::new();
    let debut = Instant::now();
    loop {
        if let Ok(r) = client.get("http://127.0.0.1:8147/health").send().await
            && r.status() == 200
        {
            break;
        }
        assert!(
            debut.elapsed() < Duration::from_secs(30),
            "la première passerelle n'est pas devenue prête en 30 s"
        );
        tokio::time::sleep(Duration::from_millis(150)).await;
    }

    // 2. La seconde, MÊME store d'identités : elle doit mourir, et le DIRE.
    let mut seconde = Command::new(env!("CARGO_BIN_EXE_this-gateway"))
        .args(args("127.0.0.1:8148"))
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("lancement de la seconde");

    let fin = Instant::now();
    let statut = loop {
        match seconde.try_wait().expect("try_wait") {
            Some(statut) => break statut,
            None if fin.elapsed() < Duration::from_secs(20) => {
                tokio::time::sleep(Duration::from_millis(100)).await
            }
            None => {
                let _ = seconde.kill();
                let _ = seconde.wait();
                panic!(
                    "la seconde passerelle a démarré MALGRÉ le verrou : la coexistence est \
                     possible, donc l'ordre de la bascule doit être REPOSÉ (démarrer, \
                     attendre la santé, arrêter ensuite) — il n'a plus de raison d'être \
                     celui d'aujourd'hui"
                );
            }
        }
    };
    let sortie = seconde.wait_with_output().expect("sortie de la seconde");
    let err = String::from_utf8_lossy(&sortie.stderr).to_string();

    assert!(
        !statut.success(),
        "la seconde doit ÉCHOUER, pas servir à moitié : {err}"
    );
    assert!(
        err.contains("already open by another process") || err.contains("exclusive lock"),
        "l'échec doit NOMMER le verrou, sinon la bascule échouerait sans que rien ne dise \
         pourquoi : {err}"
    );
    assert!(
        err.contains("identites") || err.contains("identités"),
        "…et nommer le store d'identités : {err}"
    );

    // 3. La première n'a pas bougé — c'est ce qui rend le retour arrière possible.
    let r = client
        .get("http://127.0.0.1:8147/health")
        .send()
        .await
        .expect("santé de la première");
    assert_eq!(
        r.status(),
        200,
        "la première reste saine après le refus de la seconde"
    );
}
