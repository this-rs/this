//! Tests de **sécurité** — IAM, autorisation, identité.
//!
//! Chaque test nomme l'attaque qu'il modélise. Il ne vérifie pas qu'une
//! fonction fait ce que dit sa documentation : il vérifie qu'un **attaquant**
//! n'obtient pas ce qu'il cherche. Trois catégories :
//!
//! 1. **escalade** — obtenir plus de droits que la politique ne le prévoit
//!    (composition, joker, décision forgée, ambiguïté) ;
//! 2. **contournement** — atteindre une route sans passer par sa politique
//!    (traversée de chemin, casse, méthode détournée) ;
//! 3. **capture / fuite** — faire lire la décision d'un autre (collision de clé,
//!    espace voisin, locataire voisin, rejeu après invalidation).
//!
//! Les limites du modèle de confiance sont **testées comme telles** : elles
//! s'appellent `documented_limit_*` et affirment ce qui est possible, pour que
//! personne ne le découvre en production.

#![cfg(feature = "obrain")]

use std::sync::Arc;

use axum::Router;
use axum::body::Body;
use axum::extract::Extension;
use axum::http::{Method, Request, StatusCode};
use axum::routing::{delete, get};
use tower::ServiceExt;

use this::config::{EntityAuthConfig, EntityConfig, LinksConfig};
use this::core::auth::{AuthContext, AuthResolverRegistry};
use this::core::identity::{EntityKey, IdentityRegistry, Qualifier};
use this::core::link::LinkDefinition;
use this::server::RestExposure;
use this::server::builder::ServerBuilder;
use this::server::entity_registry::{EntityDescriptor, EntityRegistry};
use this::server::middleware::authorization::{Authorization, Decision, Requirement};
use this::storage::GraphBackend;
use this::storage::ObrainStore;
use this::storage::iam::{
    ACL_DIGEST_PROP, ACL_EPOCH_PROP, ACL_KEY_PROP, ACL_PERMS_PROP, ACL_PRINCIPAL_PROP,
    ACL_ROLES_PROP, ACL_SCOPE_PROP, Iam, PendingDecision, RoleGrant,
};

// ---------------------------------------------------------------------------
// Outils
// ---------------------------------------------------------------------------

fn store() -> Arc<ObrainStore> {
    ObrainStore::new_in_memory()
}

fn write_flat_node(store: &ObrainStore, label: &str, props: &[(&str, &str)]) {
    let session = store.db().session();
    let pairs: Vec<(&str, obrain_common::types::Value)> = props
        .iter()
        .map(|(k, v)| (*k, obrain_common::types::Value::from(*v)))
        .collect();
    session.create_node_with_props(&[label], pairs);
}

/// Le nombre de nœuds portant une clé d'ACL, **dans l'espace actif**.
fn acl_nodes(store: &ObrainStore, key: &str) -> usize {
    <ObrainStore as GraphBackend>::find_nodes_by_property(
        store,
        ACL_KEY_PROP,
        &obrain_common::types::Value::from(key),
    )
    .len()
}

/// **Forge** une décision directement dans le store, comme le ferait un
/// attaquant qui peut écrire (voir les `documented_limit_*`).
fn forge_decision(store: &ObrainStore, key: &str, principal: &str, scope: &str, perms: &[&str]) {
    let perms_json = serde_json::to_string(&perms).expect("json");
    let session = store.db().session();
    let node = session.create_node_with_props(
        &["Acl"],
        [
            (ACL_KEY_PROP, obrain_common::types::Value::from(key)),
            (
                ACL_PRINCIPAL_PROP,
                obrain_common::types::Value::from(principal),
            ),
            (ACL_SCOPE_PROP, obrain_common::types::Value::from(scope)),
            (
                ACL_PERMS_PROP,
                obrain_common::types::Value::from(perms_json.as_str()),
            ),
            (ACL_ROLES_PROP, obrain_common::types::Value::from("[]")),
            (ACL_DIGEST_PROP, obrain_common::types::Value::from("forgé")),
            (ACL_EPOCH_PROP, obrain_common::types::Value::Int64(99)),
        ],
    );
    let _ = node;
    store.commit_wal().expect("commit");
}

fn grant(role: &str, perms: &[&str]) -> RoleGrant {
    RoleGrant {
        role: role.to_string(),
        permissions: perms.iter().map(|s| s.to_string()).collect(),
    }
}

/// Le nom d'identité **tel que le mappage par défaut le produit** : les UUID en
/// texte. S'en écarter, c'est mesurer une décision qui n'existe pas.
fn identity_of(value: u128) -> String {
    uuid::Uuid::from_u128(value).to_string()
}

fn user(tenant: u128, id: u128) -> AuthContext {
    AuthContext::User {
        user_id: uuid::Uuid::from_u128(id),
        tenant_id: uuid::Uuid::from_u128(tenant),
        roles: vec![],
    }
}

struct Probe;

impl EntityDescriptor for Probe {
    fn entity_type(&self) -> &str {
        "order"
    }
    fn plural(&self) -> &str {
        "orders"
    }
    fn build_routes(&self) -> Router {
        Router::new()
            .route("/orders", get(|| async { "[]" }))
            .route("/orders/{id}", delete(|| async { "supprimé" }))
    }
}

fn authz_with(policy: &str, store: &Arc<ObrainStore>) -> Authorization {
    let auth = EntityAuthConfig {
        list: policy.to_string(),
        delete: policy.to_string(),
        ..Default::default()
    };
    let mut registry = EntityRegistry::new();
    registry.register(Box::new(Probe));
    let config = LinksConfig {
        entities: vec![EntityConfig {
            singular: "order".to_string(),
            plural: "orders".to_string(),
            auth,
        }],
        links: Vec::<LinkDefinition>::new(),
        validation_rules: None,
        events: None,
        sinks: None,
        auth: None,
    };
    Authorization::new(
        Arc::new(config),
        &registry,
        Arc::new(AuthResolverRegistry::new()),
    )
    .with_iam(store)
}

async fn call_with(
    app: Router,
    context: Option<AuthContext>,
    method: Method,
    uri: &str,
) -> (StatusCode, axum::http::HeaderMap) {
    let app = match context {
        Some(ctx) => app.layer(Extension(ctx)),
        None => app,
    };
    let response = app
        .oneshot(
            Request::builder()
                .method(method)
                .uri(uri)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .expect("réponse");
    (response.status(), response.headers().clone())
}

// ===========================================================================
// 1. ESCALADE PAR LA COMPOSITION DES POLITIQUES
// ===========================================================================

/// **Piège de configuration** : une négation dans une disjonction n'est pas une
/// restriction — c'est l'inverse. `any:not:admin_only` autorise *tout le monde
/// sauf les administrateurs*. Un auteur qui écrit ça croit restreindre.
#[test]
fn a_negation_inside_a_disjunction_is_a_trap_not_a_restriction() {
    let store = store();
    let authz = authz_with("any:not:admin_only", &store);

    match authz.decide(&Method::GET, "/orders", &AuthContext::Anonymous) {
        Decision::Allow { .. } => {}
        other => panic!("`any:not:admin_only` doit AUTORISER un anonyme (le piège) : {other:?}"),
    }
}

/// **Piège de configuration** : `all:public,not:authenticated` n'autorise que
/// les **anonymes** — l'exact contraire de ce que la lecture rapide suggère.
#[test]
fn a_conjunction_of_public_and_not_authenticated_serves_only_anonymous() {
    let store = store();
    let authz = authz_with("all:public,not:authenticated", &store);

    assert!(
        authz
            .decide(&Method::GET, "/orders", &AuthContext::Anonymous)
            .is_allow(),
        "un anonyme passe (c'est le sens littéral de la politique)"
    );
    assert!(
        !authz
            .decide(&Method::GET, "/orders", &user(1, 1))
            .is_allow(),
        "et un utilisateur authentifié est REFUSÉ — le piège"
    );
}

/// **Piège de configuration** : dès qu'une branche `public` figure dans une
/// disjonction, la politique `perm:` devient décorative.
#[test]
fn a_public_branch_makes_a_perm_policy_decorative() {
    let store = store();
    let authz = authz_with("any:perm:orders:write,public", &store);

    assert!(
        authz
            .decide(&Method::DELETE, "/orders/1", &AuthContext::Anonymous)
            .is_allow(),
        "la branche `public` court-circuite : la vérification IAM n'a plus d'effet"
    );
}

/// Le vocabulaire est **clos** : ni la casse, ni les espaces, ni un mot
/// approchant ne doivent ouvrir une route.
#[test]
fn near_miss_vocabulary_never_opens_a_route() {
    let store = store();
    for policy in [
        "PUBLIC",
        "Public",
        "public ",
        " public",
        "authenticated\r\n",
        "admin",
        "owner ",
        "",
        "\tpublic",
    ] {
        let authz = authz_with(policy, &store);
        assert!(
            !authz
                .decide(&Method::GET, "/orders", &AuthContext::Anonymous)
                .is_allow(),
            "`{policy:?}` ne doit pas ouvrir la route à un anonyme"
        );
    }
}

/// Un résolveur nommé mais **absent** refuse tout le monde, y compris un
/// administrateur : pas de repli silencieux.
#[test]
fn an_absent_resolver_denies_everyone_including_administrators() {
    let store = store();
    let authz = authz_with("resolver:personne", &store);
    for ctx in [
        AuthContext::Anonymous,
        user(1, 1),
        AuthContext::Admin {
            admin_id: uuid::Uuid::from_u128(9),
        },
    ] {
        assert!(
            !authz.decide(&Method::GET, "/orders", &ctx).is_allow(),
            "un résolveur absent doit refuser : {ctx:?}"
        );
    }
}

// ===========================================================================
// 2. ESCALADE PAR LA DÉCISION IAM
// ===========================================================================

/// Un joker (`*`) ne doit **jamais** accorder une permission vide : un `perm:`
/// sans nom est une faute, et une faute refuse.
#[test]
fn a_wildcard_never_grants_an_empty_permission() {
    let store = store();
    let iam = store.iam();
    iam.materialize("u", "t", &[grant("root", &["*"])])
        .expect("matérialisation");

    let decision = iam.decision("u", "t").expect("lecture").expect("décision");
    assert!(decision.allows("orders:write"));
    assert!(
        !decision.allows(""),
        "le joker ne couvre PAS le nom vide (fail-closed)"
    );
}

/// Le joker IAM ne fait pas sauter les politiques qui ne le consultent pas :
/// une décision `*` ne satisfait pas `role:admin`.
#[test]
fn a_wildcard_decision_does_not_satisfy_a_role_policy() {
    let store = store();
    store
        .iam()
        .materialize("u", "t", &[grant("root", &["*"])])
        .expect("matérialisation");

    // `role:admin` ne consulte jamais l'IAM.
    let authz = authz_with("role:admin", &store);
    let ctx = AuthContext::User {
        user_id: uuid::Uuid::from_u128(1),
        tenant_id: uuid::Uuid::from_u128(1),
        roles: vec!["editor".into()],
    };
    assert!(
        !authz.decide(&Method::GET, "/orders", &ctx).is_allow(),
        "un joker IAM n'est pas un rôle : l'autorisation reste refusée"
    );
}

/// La correspondance des permissions est **exacte** : ni préfixe, ni extension,
/// ni insensibilité à la casse.
#[test]
fn permissions_match_exactly_never_by_prefix_or_case() {
    let store = store();
    let iam = store.iam();
    iam.materialize("u", "t", &[grant("r", &["orders:read"])])
        .expect("matérialisation");
    let decision = iam.decision("u", "t").unwrap().unwrap();

    for p in [
        "orders",
        "orders:read:all",
        "orders:rea",
        "Orders:Read",
        " orders:read",
        "orders:read ",
    ] {
        assert!(
            !decision.allows(p),
            "`{p}` ne doit pas être accordé par `orders:read`"
        );
    }
}

/// Une décision **forgée** par qui écrit dans le store accorde bien ce qu'elle
/// veut : c'est la **frontière de confiance**, affirmée ici pour qu'elle soit
/// connue (voir la documentation de `Iam`).
#[test]
fn documented_limit_whoever_writes_the_store_grants_permissions() {
    let store = store();
    let key = Iam::acl_key("t", "u");
    forge_decision(&store, &key, "u", "t", &["*"]);

    let decision = store
        .iam()
        .decision("u", "t")
        .expect("lecture")
        .expect("décision forgée lue");
    assert!(
        decision.allows("tout:ce:quon:veut"),
        "un nœud de décision écrit à la main est cru — la protection est le bail \
         d'écriture et la maîtrise du store, pas une signature"
    );
}

/// **Durcissement** : deux nœuds portant la même clé d'ACL rendent la décision
/// ambiguë — elle doit être **refusée**, jamais résolue par l'ordre de l'index.
#[test]
fn a_duplicated_acl_key_is_refused_instead_of_resolved_by_index_order() {
    let store = store();
    let iam = store.iam();
    iam.materialize("u", "t", &[grant("reader", &["orders:read"])])
        .expect("matérialisation");
    let key = Iam::acl_key("t", "u");
    assert_eq!(acl_nodes(&store, &key), 1);

    // Un second nœud, forgé, portant la MÊME clé mais des permissions `*`.
    forge_decision(&store, &key, "u", "t", &["*"]);
    assert_eq!(acl_nodes(&store, &key), 2, "deux nœuds portent la clé");

    let outcome = store.iam().decision("u", "t");
    match outcome {
        Err(e) => assert!(
            e.to_string().contains("AMBIGU"),
            "l'ambiguïté doit être nommée : {e}"
        ),
        Ok(_) => panic!("une clé dupliquée doit rendre la décision AMBIGUË (donc refusée)"),
    }
}

/// Une décision **invalidée** ne peut pas être rejouée : la décision servie ne
/// permet plus rien, et un stamp mémorisé est détectablement périmé.
#[test]
fn an_invalidated_decision_cannot_be_replayed() {
    let store = store();
    let iam = store.iam();
    let stamp = iam
        .materialize("u", "t", &[grant("writer", &["orders:write"])])
        .expect("matérialisation");
    assert!(stamp.allows("orders:write"));

    let epoch = iam.invalidate("u", "t").expect("invalidation");
    assert!(stamp.is_stale(epoch), "le stamp mémorisé doit être périmé");

    let after = iam.decision("u", "t").expect("lecture").expect("nœud");
    assert!(
        !after.allows("orders:write"),
        "après invalidation, la décision ne permet plus rien"
    );
}

/// **Capture par collision de clé** : `("a/b","c")` et `("a","b/c")` ne doivent
/// pas partager de nœud (le séparateur `/` de la v1 collisionnait).
#[test]
fn two_identity_pairs_never_share_a_decision_node() {
    let store = store();
    let iam = store.iam();
    iam.materialize("c", "a/b", &[grant("r1", &["p1"])])
        .expect("première");
    iam.materialize("b/c", "a", &[grant("r2", &["p2"])])
        .expect("seconde");

    assert_ne!(Iam::acl_key("a/b", "c"), Iam::acl_key("a", "b/c"));
    assert_eq!(acl_nodes(&store, &Iam::acl_key("a/b", "c")), 1);
    assert_eq!(acl_nodes(&store, &Iam::acl_key("a", "b/c")), 1);

    let first = iam.decision("c", "a/b").unwrap().unwrap();
    let second = iam.decision("b/c", "a").unwrap().unwrap();
    assert!(first.allows("p1") && !first.allows("p2"), "{first:?}");
    assert!(second.allows("p2") && !second.allows("p1"), "{second:?}");
}

/// **Capture par le caractère séparateur** : un nom contenant le US (0x1F)
/// permettrait à un principal d'en usurper un autre (`("a\u{1f}b","c")` et
/// `("a","b\u{1f}c")` donnent la même clé). Ces identités sont **refusées**.
#[test]
fn an_identity_containing_the_separator_is_refused() {
    let store = store();
    let iam = store.iam();

    let forged_scope = "a\u{1f}b";
    let err = iam
        .materialize("c", forged_scope, &[grant("r", &["*"])])
        .expect_err("une portée contenant le séparateur doit être refusée");
    assert!(
        err.to_string().contains("contrôle"),
        "le refus doit nommer la cause : {err}"
    );

    // Et l'attaque par le principal échoue aussi.
    assert!(
        iam.materialize("b\u{1f}c", "a", &[grant("r", &["*"])])
            .is_err(),
        "un principal contenant le séparateur doit être refusé"
    );

    // Aucun nœud n'a été créé : la collision est impossible, pas seulement
    // improbable.
    assert_eq!(acl_nodes(&store, &Iam::acl_key("a", "b/c")), 0);
    let first = store
        .iam()
        .decision("b\u{1f}c", "a")
        .expect_err("la lecture aussi est refusée")
        .to_string();
    let second = store
        .iam()
        .decision("b\u{1f}c", "a")
        .expect_err("stable")
        .to_string();
    assert_eq!(first, second, "le refus est déterministe");
}

/// Une identité **vide** est refusée : elle se confondrait avec « pas de
/// principal », donc avec un refus… ou avec tout le monde, selon le lecteur.
#[test]
fn an_empty_identity_is_refused() {
    let store = store();
    let iam = store.iam();
    assert!(iam.materialize("", "t", &[grant("r", &["*"])]).is_err());
    assert!(iam.materialize("u", "", &[grant("r", &["*"])]).is_err());
    assert!(
        iam.materialize_batch(&[PendingDecision {
            principal: "",
            scope: "t",
            roles: &[grant("r", &["*"])],
        }])
        .is_err()
    );
}

// ===========================================================================
// 3. FUITE ENTRE LOCATAIRES ET ENTRE ESPACES
// ===========================================================================

/// Le cloisonnement par **locataire** : deux locataires, même identifiant
/// d'utilisateur — et un joker chez l'un ne donne rien chez l'autre.
#[test]
fn a_wildcard_in_one_tenant_grants_nothing_in_another() {
    let store = store();
    let iam = store.iam();
    iam.materialize(&identity_of(1), &identity_of(10), &[grant("root", &["*"])])
        .expect("locataire 10");

    assert!(
        iam.decision(&identity_of(1), &identity_of(10))
            .unwrap()
            .expect("décision")
            .allows("orders:write"),
        "le locataire 10 a bien le droit"
    );
    assert_eq!(
        iam.decision(&identity_of(1), &identity_of(11))
            .expect("lecture"),
        None,
        "même identifiant, autre locataire : AUCUNE décision"
    );

    // Et par la route : le contexte du locataire 11 est refusé.
    let authz = authz_with("perm:orders:write", &store);
    let allowed = authz.decide(&Method::DELETE, "/orders/1", &user(10, 1));
    let denied = authz.decide(&Method::DELETE, "/orders/1", &user(11, 1));
    assert!(allowed.is_allow(), "locataire 10 autorisé");
    assert!(!denied.is_allow(), "locataire 11 refusé");
}

/// Le cloisonnement par **espace** : une décision écrite dans un espace ne doit
/// pas être lue dans un autre. (Défaut réellement trouvé : les recherches
/// IAM passaient par l'index **racine**.)
#[test]
fn a_decision_written_in_one_space_is_invisible_in_another() {
    let store = store();
    let iam = store.iam();

    // Des permissions DIFFÉRENTES dans chaque espace : c'est ce qui rend la
    // fuite visible. Deux contenus identiques ne prouveraient rien.
    iam.materialize("u", "t", &[grant("r", &["default:only"])])
        .expect("dans l'espace par défaut");

    if store.create_space("tenant-b").is_ok() {
        store.use_graph("tenant-b");
        assert_eq!(
            store.iam().decision("u", "t").expect("lecture"),
            None,
            "la décision de l'espace par défaut ne doit pas être servie ici"
        );

        store
            .iam()
            .materialize("u", "t", &[grant("r", &["tenantb:only"])])
            .expect("dans tenant-b");
        let in_b = store.iam().decision("u", "t").unwrap().unwrap();
        assert!(
            in_b.allows("tenantb:only") && !in_b.allows("default:only"),
            "{in_b:?}"
        );

        // Retour à l'espace par défaut : c'est SA décision qui doit être servie,
        // pas celle de tenant-b.
        store.reset_graph();
        let back = store.iam().decision("u", "t").unwrap().unwrap();
        assert!(
            back.allows("default:only"),
            "l'espace par défaut doit retrouver sa décision : {back:?}"
        );
        assert!(
            !back.allows("tenantb:only"),
            "et SURTOUT pas celle de l'autre espace : {back:?}"
        );
    }
}

// ===========================================================================
// 4. CONTOURNEMENT PAR LA ROUTE
// ===========================================================================

/// Une **traversée de chemin** ne doit pas être jugée comme une autre opération
/// que celle que le routeur servira.
#[tokio::test]
async fn a_path_traversal_is_not_judged_as_a_different_operation() {
    let store = store();
    let authz = authz_with("perm:orders:write", &store);

    // `/orders/1/../..` : après suppression des segments vides, la forme fait
    // 5 segments ⇒ `create_link`. C'est un DÉCALAGE : le résolveur juge une
    // opération de lien pour un chemin que le routeur interprétera autrement.
    let requirement = authz.requirement(&Method::POST, "/orders/1/../../admin");
    match requirement {
        Requirement::Policy { operation, .. } => assert_eq!(
            operation, "create_link",
            "le résolveur suit le NOMBRE de segments : une traversée est jugée \
             comme une autre opération"
        ),
        Requirement::Abstain { .. } => {}
    }

    // Le point qui compte : rien de tout cela n'atteint une route réelle sans
    // politique, et le routeur ne sert pas le chemin traversé.
    let host = Arc::new(
        ServerBuilder::new()
            .register_module(ProbeModule)
            .expect("module")
            .with_obrain_store(Arc::clone(&store))
            .build_host()
            .expect("hôte"),
    );
    let app = RestExposure::build_router(host, vec![]).expect("routeur");
    let (status, _) = call_with(app, Some(user(1, 1)), Method::POST, "/orders/1/../../admin").await;
    assert_ne!(
        status,
        StatusCode::OK,
        "une traversée ne doit pas atteindre une ressource"
    );
}

struct ProbeModule;

impl this::core::module::Module for ProbeModule {
    fn name(&self) -> &str {
        "probe"
    }
    fn entity_types(&self) -> Vec<&str> {
        vec!["order"]
    }
    fn links_config(&self) -> anyhow::Result<LinksConfig> {
        let auth = EntityAuthConfig {
            list: "perm:orders:read".to_string(),
            delete: "perm:orders:write".to_string(),
            ..Default::default()
        };
        Ok(LinksConfig {
            entities: vec![EntityConfig {
                singular: "order".to_string(),
                plural: "orders".to_string(),
                auth,
            }],
            links: Vec::<LinkDefinition>::new(),
            validation_rules: None,
            events: None,
            sinks: None,
            auth: None,
        })
    }
    fn register_entities(&self, registry: &mut EntityRegistry) {
        registry.register(Box::new(Probe));
    }
}

/// Un **en-tête de substitution de méthode** ne doit pas changer la méthode
/// jugée : c'est la requête réelle qui compte.
#[tokio::test]
async fn a_method_override_header_does_not_change_the_judged_method() {
    let store = store();
    // Le porteur n'a QUE la lecture.
    store
        .iam()
        .materialize(
            &identity_of(1),
            &identity_of(1),
            &[grant("reader", &["orders:read"])],
        )
        .expect("matérialisation");

    let host = Arc::new(
        ServerBuilder::new()
            .register_module(ProbeModule)
            .expect("module")
            .with_obrain_store(Arc::clone(&store))
            .build_host()
            .expect("hôte"),
    );
    let app = RestExposure::build_router(host, vec![]).expect("routeur");

    // GET autorisé…
    let (status, _) = call_with(app.clone(), Some(user(1, 1)), Method::GET, "/orders").await;
    assert_eq!(status, StatusCode::OK);

    // …et un GET portant `X-HTTP-Method-Override: DELETE` reste un GET : il ne
    // peut pas emprunter la politique de suppression.
    let response = app
        .layer(Extension(user(1, 1)))
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/orders")
                .header("x-http-method-override", "DELETE")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .expect("réponse");
    assert_eq!(
        response.status(),
        StatusCode::OK,
        "l'en-tête ne doit pas transformer la requête"
    );
    assert!(
        !response.headers().contains_key("x-this-rs-authz"),
        "aucun refus observé : c'est bien la lecture qui a été jugée"
    );
}

/// `HEAD` est jugé comme `GET` — axum le sert par le handler `GET`, donc
/// s'abstenir ouvrirait une porte latérale.
#[test]
fn head_is_never_a_side_door_to_the_get_route() {
    let store = store();
    let authz = authz_with("perm:orders:read", &store);
    for path in ["/orders", "/orders/1", "/orders/1/links"] {
        let get = authz.requirement(&Method::GET, path);
        let head = authz.requirement(&Method::HEAD, path);
        assert!(
            matches!(head, Requirement::Policy { .. }),
            "HEAD {path} doit porter une politique, pas s'abstenir : {head:?}"
        );
        match (get, head) {
            (
                Requirement::Policy { operation: a, .. },
                Requirement::Policy { operation: b, .. },
            ) => assert_eq!(a, b, "HEAD et GET doivent juger la même opération"),
            (a, b) => panic!("divergence inattendue sur {path} : {a:?} vs {b:?}"),
        }
    }
}

/// Les identités du **mode observation** ne doivent pas être plus permissives
/// que celles du mode armé : observer change le comportement, pas la décision.
#[test]
fn observation_mode_never_changes_the_decision() {
    let store = store();
    let policy = "admin_only";

    let enforced = authz_with(policy, &store);
    let observed = authz_with(policy, &store).report_only();

    for ctx in [AuthContext::Anonymous, user(1, 1)] {
        let a = enforced.decide(&Method::DELETE, "/orders/1", &ctx);
        let b = observed.decide(&Method::DELETE, "/orders/1", &ctx);
        assert_eq!(
            a.is_allow(),
            b.is_allow(),
            "la décision doit être identique dans les deux modes ({ctx:?})"
        );
    }
}

// ===========================================================================
// 5. IDENTITÉ : COLLISIONS ET USURPATION
// ===========================================================================

/// L'encodage d'identité préfixe les longueurs : `("ab","c")` et `("a","bc")` ne
/// doivent pas produire l'identifiant l'un de l'autre (le défaut classique de la
/// concaténation).
#[test]
fn length_ambiguous_names_do_not_collide() {
    let key = |label: &str, prop: &str, value: &str| {
        EntityKey::new(
            None,
            label,
            Qualifier::Natural {
                property: prop.into(),
                value: value.into(),
            },
        )
        .expect("clé")
        .uuid()
    };

    assert_ne!(
        key("AB", "p", "v"),
        key("A", "Bp", "v"),
        "label et propriété ne doivent pas pouvoir se décaler"
    );
    assert_ne!(
        key("A", "p", "bc"),
        key("A", "pb", "c"),
        "propriété et valeur ne doivent pas pouvoir se décaler"
    );
    assert_ne!(
        key("A", "p", "x"),
        key("A", "p", "x\u{0}"),
        "un octet nul ajouté change l'identité"
    );
}

/// **Piège réel de l'Unicode** : deux écritures du même nom (« é » précomposé et
/// « e + accent combinant ») donnent **deux identités**. Ce n'est pas une faille
/// — c'est une propriété de l'encodage, et elle doit être connue : une base où
/// le même nom est saisi de deux façons porte deux entités.
#[test]
fn documented_limit_unicode_normalisation_forms_are_two_identities() {
    let key = |value: &str| {
        EntityKey::new(
            None,
            "Page",
            Qualifier::Natural {
                property: "qualified_name".into(),
                value: value.into(),
            },
        )
        .expect("clé")
        .uuid()
    };
    let precomposed = "caf\u{e9}"; // U+00E9
    let decomposed = "cafe\u{301}"; // e + U+0301

    assert_ne!(
        key(precomposed),
        key(decomposed),
        "l'encodage est en octets : deux formes Unicode normalisées différemment \
         sont deux identités — à normaliser en amont si l'on veut qu'elles n'en fassent qu'une"
    );
}

/// Un `_uuid` illisible ne doit **jamais** être re-cléé : donner un identifiant
/// dérivé créerait une seconde identité pour la même entité.
#[test]
fn a_malformed_uuid_is_never_rekeyed_even_under_attack() {
    let store = store();
    write_flat_node(
        &store,
        "Page",
        &[
            ("_uuid", "'; DROP TABLE --"),
            ("qualified_name", "entity:attaque"),
        ],
    );

    let registry = IdentityRegistry::obrain_default();
    let report = store.rebuild_identity_index(&registry);

    assert_eq!(
        report.total_resolved, 0,
        "un `_uuid` illisible doit rester non résolu : {report:?}"
    );
    assert_eq!(report.labels[0].malformed.len(), 1);
    assert!(
        report.labels[0].malformed[0].contains("DROP"),
        "la valeur fautive doit être rapportée, pas interprétée"
    );
}

/// Les nœuds **internes** du framework ne sont pas des entités : un attaquant ne
/// doit pas pouvoir se faire passer pour un par un label réservé.
#[test]
fn reserved_labels_cannot_be_used_to_become_an_entity() {
    let store = store();
    write_flat_node(&store, "_link:admin", &[("qualified_name", "entity:moi")]);
    write_flat_node(&store, "_uuid:Page", &[("qualified_name", "entity:moi")]);

    let registry = IdentityRegistry::obrain_default();
    let report = store.rebuild_identity_index(&registry);
    assert_eq!(
        report.total_resolved, 0,
        "aucun label réservé ne doit produire d'identité : {report:?}"
    );
}
