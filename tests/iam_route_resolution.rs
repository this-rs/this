//! Résolution des routes par le point d'application d'autorisation.
//!
//! Ce fichier verrouille *quelle* politique s'applique à *quelle* requête :
//! une erreur ici ouvre ou ferme la mauvaise porte.
//!
//! # Le contrat verrouillé
//!
//! | segments | méthode | opération |
//! |---|---|---|
//! | `[plural]` | GET | `list` |
//! | `[plural]` | POST | `create` |
//! | `[plural, id]` | GET | `get` |
//! | `[plural, id]` | PUT, PATCH | `update` |
//! | `[plural, id]` | DELETE | `delete` |
//! | `[plural, id, route]` | GET | `list_links` |
//! | `[plural, id, type, target, target_id]` | POST | `create_link` |
//! | `[plural, id, type, target, target_id]` | DELETE | `delete_link` |
//!
//! Toute autre forme, et tout pluriel non enregistré, ⇒ `Requirement::Abstain`
//! avec une raison **non vide**. Une abstention n'est jamais un refus : elle
//! laisse la couche d'authentification globale seule juge — mais elle ne doit
//! jamais être une **invention** de politique pour une route inconnue.
//!
//! Ce fichier ne dépend pas de la feature `obrain` : la résolution est purement
//! structurelle (segments + méthode + registre + configuration).

use std::collections::HashSet;
use std::sync::Arc;

use axum::http::{Method, StatusCode};
use this::config::{EntityAuthConfig, EntityConfig, LinksConfig};
use this::core::auth::AuthResolverRegistry;
use this::core::link::LinkDefinition;
use this::server::entity_registry::{EntityDescriptor, EntityRegistry};
use this::server::middleware::authorization::{Authorization, PolicyOrigin, Requirement};

// ── Montage ─────────────────────────────────────────────────────────────────

/// Descripteur minimal : la résolution ne lit que `entity_type()` et `plural()`.
struct TestEntity {
    entity_type: &'static str,
    plural: &'static str,
}

impl EntityDescriptor for TestEntity {
    fn entity_type(&self) -> &str {
        self.entity_type
    }

    fn plural(&self) -> &str {
        self.plural
    }

    fn build_routes(&self) -> axum::Router {
        // Les routes ne sont pas nécessaires : la résolution part du chemin
        // observé par le middleware, pas du routeur.
        axum::Router::new()
    }
}

fn registry(registered: &[(&'static str, &'static str)]) -> EntityRegistry {
    let mut registry = EntityRegistry::new();
    for (entity_type, plural) in registered {
        registry.register(Box::new(TestEntity {
            entity_type,
            plural,
        }));
    }
    registry
}

fn links_config(entities: Vec<EntityConfig>) -> LinksConfig {
    LinksConfig {
        entities,
        links: Vec::<LinkDefinition>::new(),
        validation_rules: None,
        events: None,
        sinks: None,
        auth: None,
    }
}

fn authorization(
    entities: Vec<EntityConfig>,
    registered: &[(&'static str, &'static str)],
) -> Authorization {
    Authorization::new(
        Arc::new(links_config(entities)),
        &registry(registered),
        Arc::new(AuthResolverRegistry::new()),
    )
}

fn entity_config(singular: &str, plural: &str, auth: EntityAuthConfig) -> EntityConfig {
    EntityConfig {
        singular: singular.to_string(),
        plural: plural.to_string(),
        auth,
    }
}

/// Huit politiques **toutes distinctes** : si la résolution confond deux
/// opérations, la comparaison le voit — au lieu de tomber sur deux valeurs
/// égales et de laisser passer la confusion.
fn distinct_per_operation(prefix: &str) -> EntityAuthConfig {
    EntityAuthConfig {
        list: format!("{prefix}:list"),
        get: format!("{prefix}:get"),
        create: format!("{prefix}:create"),
        update: format!("{prefix}:update"),
        delete: format!("{prefix}:delete"),
        list_links: format!("{prefix}:list_links"),
        create_link: format!("{prefix}:create_link"),
        delete_link: format!("{prefix}:delete_link"),
    }
}

/// Les huit opérations, chacune dans la forme de route qui la produit.
fn shapes_for(plural: &str) -> Vec<(Method, String, &'static str)> {
    vec![
        (Method::GET, format!("/{plural}"), "list"),
        (Method::POST, format!("/{plural}"), "create"),
        (Method::GET, format!("/{plural}/42"), "get"),
        (Method::PUT, format!("/{plural}/42"), "update"),
        (Method::PATCH, format!("/{plural}/42"), "update"),
        (Method::DELETE, format!("/{plural}/42"), "delete"),
        (Method::GET, format!("/{plural}/42/invoices"), "list_links"),
        (
            Method::POST,
            format!("/{plural}/42/items/product/7"),
            "create_link",
        ),
        (
            Method::DELETE,
            format!("/{plural}/42/items/product/7"),
            "delete_link",
        ),
    ]
}

fn the_eight(auth: &EntityAuthConfig) -> Vec<(&'static str, String)> {
    vec![
        ("list", auth.list.clone()),
        ("get", auth.get.clone()),
        ("create", auth.create.clone()),
        ("update", auth.update.clone()),
        ("delete", auth.delete.clone()),
        ("list_links", auth.list_links.clone()),
        ("create_link", auth.create_link.clone()),
        ("delete_link", auth.delete_link.clone()),
    ]
}

fn policy_for<'a>(auth: &'a EntityAuthConfig, operation: &str) -> &'a String {
    match operation {
        "list" => &auth.list,
        "get" => &auth.get,
        "create" => &auth.create,
        "update" => &auth.update,
        "delete" => &auth.delete,
        "list_links" => &auth.list_links,
        "create_link" => &auth.create_link,
        "delete_link" => &auth.delete_link,
        other => panic!("opération hors contrat : {other}"),
    }
}

/// Décompose un `Requirement::Policy` — et **refuse l'abstention**, qui serait
/// ici un trou : aucune politique ne serait appliquée.
fn resolved(req: &Requirement, context: &str) -> (String, String, String, PolicyOrigin) {
    match req {
        Requirement::Policy {
            entity_type,
            operation,
            policy,
            origin,
        } => (
            entity_type.clone(),
            operation.clone(),
            policy.clone(),
            *origin,
        ),
        Requirement::Abstain { reason } => {
            panic!(
                "{context} : abstention inattendue alors qu'une politique est attendue — {reason}"
            )
        }
    }
}

/// Décompose un `Requirement::Abstain` en exigeant une raison **non vide**, et
/// **refuse qu'une politique ait été inventée** pour une route inconnue.
fn abstention_reason(req: &Requirement, context: &str) -> String {
    match req {
        Requirement::Abstain { reason } => {
            assert!(
                !reason.trim().is_empty(),
                "{context} : une abstention non expliquée n'est pas exploitable"
            );
            reason.clone()
        }
        Requirement::Policy { .. } => {
            panic!("{context} : une politique a été inventée pour une route inconnue — {req:?}")
        }
    }
}

// ── 1. Les huit opérations, et LA politique de chacune ──────────────────────

/// **Le test central.** Chaque forme rend l'opération du contrat *et* la
/// politique déclarée pour **cette** opération — pas celle d'à côté.
#[test]
fn each_route_shape_resolves_to_the_policy_declared_for_its_own_operation() {
    let auth = distinct_per_operation("order");
    let eight = the_eight(&auth);
    let distinct: HashSet<&String> = eight.iter().map(|(_, p)| p).collect();
    assert_eq!(
        distinct.len(),
        8,
        "montage de test invalide : les huit politiques doivent être distinctes"
    );

    let authz = authorization(
        vec![entity_config("order", "orders", auth.clone())],
        &[("order", "orders")],
    );

    for (method, path, operation) in shapes_for("orders") {
        let context = format!("{method} {path}");
        let (entity_type, resolved_operation, policy, origin) =
            resolved(&authz.requirement(&method, &path), &context);

        assert_eq!(entity_type, "order", "{context} : mauvaise entité");
        assert_eq!(
            resolved_operation, operation,
            "{context} : mauvaise opération"
        );
        assert_eq!(
            &policy,
            policy_for(&auth, operation),
            "{context} : ce n'est PAS la politique déclarée pour `{operation}`"
        );
        assert_eq!(
            origin,
            PolicyOrigin::Declared,
            "{context} : une entrée de configuration existe, l'origine doit le dire"
        );
    }
}

/// La porte qui compte : aucune forme ne doit rendre la politique d'une
/// **autre** opération. Vérifié en creux, contre les huit politiques déclarées.
#[test]
fn no_shape_ever_resolves_to_another_operations_policy() {
    let auth = distinct_per_operation("order");
    let authz = authorization(
        vec![entity_config("order", "orders", auth.clone())],
        &[("order", "orders")],
    );

    for (method, path, operation) in shapes_for("orders") {
        let context = format!("{method} {path}");
        let (_, _, policy, _) = resolved(&authz.requirement(&method, &path), &context);

        for (other_operation, other_policy) in the_eight(&auth) {
            if other_operation != operation {
                assert_ne!(
                    policy, other_policy,
                    "{context} : `{operation}` a reçu la politique de `{other_operation}`"
                );
            }
        }
    }
}

/// PUT et PATCH sont **la même** opération : une seule politique les couvre.
#[test]
fn put_and_patch_share_the_update_policy() {
    let auth = distinct_per_operation("order");
    let authz = authorization(
        vec![entity_config("order", "orders", auth.clone())],
        &[("order", "orders")],
    );

    let put = resolved(&authz.requirement(&Method::PUT, "/orders/42"), "PUT");
    let patch = resolved(&authz.requirement(&Method::PATCH, "/orders/42"), "PATCH");

    assert_eq!(
        put, patch,
        "PUT et PATCH doivent rendre le même Requirement"
    );
    assert_eq!(put.1, "update");
    assert_eq!(put.2, auth.update);
}

// ── 2. Segments : barres, préfixes, longueurs ───────────────────────────────

/// Un chemin vide n'a pas de pluriel : abstention, expliquée.
#[test]
fn empty_paths_abstain_for_the_same_reason() {
    let authz = authorization(
        vec![entity_config(
            "order",
            "orders",
            EntityAuthConfig::default(),
        )],
        &[("order", "orders")],
    );

    let reasons: Vec<String> = ["", "/", "//", "///"]
        .iter()
        .map(|path| {
            abstention_reason(
                &authz.requirement(&Method::GET, path),
                &format!("GET `{path}`"),
            )
        })
        .collect();

    // Même cause ⇒ même explication : une abstention doit être diagnosticable.
    assert!(
        reasons.windows(2).all(|pair| pair[0] == pair[1]),
        "les chemins vides doivent tous s'abstenir pour la même raison : {reasons:?}"
    );
}

/// Les segments vides sont **ignorés** : barres superflues et absence de barre
/// initiale mènent à la même opération que la forme canonique.
#[test]
fn superfluous_slashes_and_a_missing_leading_slash_reach_the_same_operation() {
    let auth = distinct_per_operation("order");
    let authz = authorization(
        vec![entity_config("order", "orders", auth.clone())],
        &[("order", "orders")],
    );

    let canonical = authz.requirement(&Method::GET, "/orders");
    assert_eq!(resolved(&canonical, "canonique").1, "list");

    for path in [
        "/orders/",
        "/orders//",
        "//orders",
        "///orders///",
        "orders",
    ] {
        let context = format!("GET `{path}`");
        let requirement = authz.requirement(&Method::GET, path);
        assert_eq!(
            requirement, canonical,
            "{context} : doit être la même résolution que `GET /orders`"
        );
    }

    // …y compris pour la forme à identifiant.
    let canonical_get = authz.requirement(&Method::GET, "/orders/42");
    for path in ["/orders/42/", "/orders//42", "orders/42"] {
        assert_eq!(
            authz.requirement(&Method::GET, path),
            canonical_get,
            "GET `{path}` : doit être la même résolution que `GET /orders/42`"
        );
    }
}

/// Les formes hors contrat s'abstiennent : jamais de politique extrapolée.
#[test]
fn shapes_outside_the_contract_abstain() {
    let authz = authorization(
        vec![entity_config(
            "order",
            "orders",
            distinct_per_operation("order"),
        )],
        &[("order", "orders")],
    );

    let cases = [
        // Un pluriel seul n'accepte que GET et POST.
        (Method::PUT, "/orders"),
        (Method::PATCH, "/orders"),
        (Method::DELETE, "/orders"),
        // 4 segments : aucune opération du contrat.
        (Method::GET, "/orders/42/a/b"),
        (Method::POST, "/orders/42/a/b"),
        (Method::DELETE, "/orders/42/a/b"),
        // 5 segments : réservés à POST et DELETE (liens).
        (Method::GET, "/orders/42/a/b/c"),
        (Method::PUT, "/orders/42/a/b/c"),
        (Method::PATCH, "/orders/42/a/b/c"),
        // 6 segments : au-delà de toute forme connue.
        (Method::GET, "/orders/42/a/b/c/d"),
        (Method::POST, "/orders/42/a/b/c/d"),
        (Method::DELETE, "/orders/42/a/b/c/d"),
    ];

    for (method, path) in cases {
        abstention_reason(
            &authz.requirement(&method, path),
            &format!("{method} {path}"),
        );
    }
}

/// **Limite assumée, verrouillée ici** : la forme à 3 segments est un *motif*.
/// Une route personnalisée greffée sous une entité hérite donc de `list_links`
/// — mais seulement en GET : en POST, la forme est inconnue et le framework
/// s'abstient plutôt que de deviner.
#[test]
fn any_third_segment_is_treated_as_a_link_route_for_get_only() {
    let authz = authorization(
        vec![entity_config(
            "order",
            "orders",
            distinct_per_operation("order"),
        )],
        &[("order", "orders")],
    );

    for path in ["/orders/42/invoices", "/orders/42/voisins", "/orders/42/x"] {
        let context = format!("GET `{path}`");
        let (_, operation, _, _) = resolved(&authz.requirement(&Method::GET, path), &context);
        assert_eq!(
            operation, "list_links",
            "{context} : la 3e segment est un motif"
        );
    }

    for path in ["/orders/42/voisins", "/orders/42/x"] {
        abstention_reason(
            &authz.requirement(&Method::POST, path),
            &format!("POST `{path}`"),
        );
    }
}

/// **Comportement constaté** : les segments vides sont supprimés, donc
/// `/orders//x` ne compte que **deux** segments utiles et se résout en `get`
/// (avec `x` pour identifiant) — et non en `list_links` comme le laisserait
/// croire une lecture littérale de la table (`[plural, id, route]` avec `id`
/// vide). La conséquence est bénigne (une politique *plus stricte* est
/// appliquée, et jamais une politique de lien fabriquée), mais elle doit être
/// connue : le nombre de segments se compte **après** suppression des vides.
#[test]
fn an_empty_identifier_makes_the_path_collapse_to_the_two_segment_shape() {
    let authz = authorization(
        vec![entity_config(
            "order",
            "orders",
            distinct_per_operation("order"),
        )],
        &[("order", "orders")],
    );

    let with_empty_id = authz.requirement(&Method::GET, "/orders//x");
    let canonical = authz.requirement(&Method::GET, "/orders/x");
    assert_eq!(
        with_empty_id, canonical,
        "`/orders//x` et `/orders/x` doivent être indiscernables"
    );
    assert_eq!(resolved(&with_empty_id, "GET /orders//x").1, "get");

    // En revanche, trois segments **utiles** restent trois segments utiles.
    let three = authz.requirement(&Method::GET, "/orders//x/y");
    assert_eq!(resolved(&three, "GET /orders//x/y").1, "list_links");

    // Et une barre finale après l'identifiant vide ne fabrique pas de forme.
    assert_eq!(
        resolved(
            &authz.requirement(&Method::GET, "/orders/42//"),
            "GET /orders/42//"
        )
        .1,
        "get"
    );
}

// ── 3. Correspondance exacte du pluriel, et origine de la politique ─────────

/// Le pluriel est comparé **exactement** : ni le singulier, ni un pluriel
/// approchant ne sont reconnus — et l'abstention nomme le segment fautif.
#[test]
fn the_plural_must_match_exactly() {
    let authz = authorization(
        vec![entity_config(
            "order",
            "orders",
            distinct_per_operation("order"),
        )],
        &[("order", "orders")],
    );

    for path in ["/order", "/orders2", "/ord", "/orders-x", "/xorders"] {
        let reason = abstention_reason(
            &authz.requirement(&Method::GET, path),
            &format!("GET `{path}`"),
        );
        assert!(
            reason.contains(path.trim_start_matches('/')),
            "GET `{path}` : l'abstention doit nommer le segment fautif — {reason}"
        );
    }

    // La comparaison est sensible à la casse : `/Orders` n'est pas `/orders`.
    for path in ["/Orders", "/ORDERS"] {
        abstention_reason(
            &authz.requirement(&Method::GET, path),
            &format!("GET `{path}`"),
        );
    }
}

/// Un pluriel **enregistré** mais absent de la configuration : la politique par
/// défaut du framework s'applique, pour les **huit** opérations, et la décision
/// dit qu'elle n'est pas déclarée.
#[test]
fn a_registered_plural_without_a_config_entry_gets_the_framework_default() {
    let authz = authorization(Vec::new(), &[("order", "orders")]);
    let default = EntityAuthConfig::default();

    // La valeur par défaut est verrouillée ici : elle n'est pas un détail.
    assert_eq!(default.list, "authenticated");

    for (method, path, operation) in shapes_for("orders") {
        let context = format!("{method} {path}");
        let (entity_type, resolved_operation, policy, origin) =
            resolved(&authz.requirement(&method, &path), &context);

        assert_eq!(entity_type, "order", "{context}");
        assert_eq!(resolved_operation, operation, "{context}");
        assert_eq!(
            &policy,
            policy_for(&default, operation),
            "{context} : politique par défaut du framework"
        );
        assert_eq!(
            origin,
            PolicyOrigin::Default,
            "{context} : sans entrée de configuration, l'origine doit être `default`"
        );
    }
}

/// La distinction `Declared`/`Default` porte sur la **présence** d'une entrée de
/// configuration, pas sur la valeur de la politique : une entrée qui recopie la
/// valeur par défaut reste `Declared`.
#[test]
fn the_origin_tracks_the_presence_of_a_config_entry_not_the_policy_value() {
    let default = EntityAuthConfig::default();

    let declared = authorization(
        vec![entity_config(
            "order",
            "orders",
            EntityAuthConfig::default(),
        )],
        &[("order", "orders")],
    );
    let undeclared = authorization(Vec::new(), &[("order", "orders")]);

    for (method, path, operation) in shapes_for("orders") {
        let context = format!("{method} {path}");
        let with_entry = resolved(&declared.requirement(&method, &path), &context);
        let without_entry = resolved(&undeclared.requirement(&method, &path), &context);

        assert_eq!(
            with_entry.2, without_entry.2,
            "{context} : les deux politiques ont la MÊME valeur…"
        );
        assert_eq!(&with_entry.2, policy_for(&default, operation), "{context}");
        assert_eq!(with_entry.1, operation, "{context}");
        assert_eq!(without_entry.1, operation, "{context}");
        assert_eq!(
            with_entry.3,
            PolicyOrigin::Declared,
            "{context} : …mais l'une est déclarée"
        );
        assert_eq!(
            without_entry.3,
            PolicyOrigin::Default,
            "{context} : …et l'autre non"
        );
    }
}

// ── 4. Méthodes inattendues ─────────────────────────────────────────────────

/// HEAD, OPTIONS, une méthode inconnue : abstention — jamais une invention, et
/// jamais non plus une politique de complaisance.
#[test]
fn unexpected_methods_abstain_instead_of_being_invented() {
    let authz = authorization(
        vec![entity_config(
            "order",
            "orders",
            distinct_per_operation("order"),
        )],
        &[("order", "orders")],
    );

    let brew = Method::from_bytes(b"BREW").expect("méthode d'extension valide");
    let lowercase_brew = Method::from_bytes(b"brew").expect("méthode d'extension valide");
    // `HEAD` n'est PLUS ici : il est gouverné comme `GET` (voir le test
    // `head_is_governed_like_get_because_axum_serves_it_through_the_get_route`).
    let methods = [
        Method::OPTIONS,
        Method::TRACE,
        Method::CONNECT,
        brew,
        lowercase_brew,
    ];
    let paths = ["/orders", "/orders/42", "/orders/42/items/product/7"];

    for method in &methods {
        for path in paths {
            let context = format!("{method} {path}");
            let reason = abstention_reason(&authz.requirement(method, path), &context);
            assert!(
                reason.contains(&method.to_string()),
                "{context} : l'abstention doit nommer la méthode fautive — {reason}"
            );
        }
    }
}

/// **Propriété de sécurité** : `HEAD` est gouverné **exactement comme `GET`**.
///
/// Axum sert une requête `HEAD` par le **handler `GET`** : si le résolveur
/// s'abstenait sur `HEAD`, une requête `HEAD` atteindrait la route d'entité sans
/// que la politique de l'entité soit appliquée — une porte latérale à la
/// politique. Le résolveur traite donc `HEAD` comme `GET`, sur toutes les formes.
#[test]
fn head_is_governed_like_get_because_axum_serves_it_through_the_get_route() {
    let authz = authorization(
        vec![entity_config(
            "order",
            "orders",
            distinct_per_operation("order"),
        )],
        &[("order", "orders")],
    );

    for path in ["/orders", "/orders/42", "/orders/42/items"] {
        let get = authz.requirement(&Method::GET, path);
        let head = authz.requirement(&Method::HEAD, path);
        assert_eq!(
            head, get,
            "HEAD {path} doit être jugé comme GET {path} — sinon la route serait              atteignable sans politique"
        );
        assert!(
            matches!(head, Requirement::Policy { .. }),
            "HEAD {path} ne doit pas s'abstenir : {head:?}"
        );
    }

    // Et `HEAD` n'invente pas d'opération là où `GET` s'abstient. On compare la
    // **forme** : la raison d'une abstention nomme la méthode fautive, donc les
    // deux `Abstain` ne sont pas textuellement égaux — et c'est voulu.
    for path in ["/orders/42/a/b/c/d", "/inconnus"] {
        let head = authz.requirement(&Method::HEAD, path);
        let get = authz.requirement(&Method::GET, path);
        assert!(
            matches!(head, Requirement::Abstain { .. })
                && matches!(get, Requirement::Abstain { .. }),
            "HEAD {path} doit s'abstenir comme GET : {head:?} / {get:?}"
        );
    }
}

// ── 5. Identifiants : le nombre de segments, et rien d'autre ────────────────

/// **Propriété** : la résolution ne dépend que du **nombre de segments**. Un
/// identifiant n'est jamais interprété — uuid, nombre, chaîne vide, très longue,
/// `%2F` non décodé, traversée de chemin : tout mène à la même opération, et le
/// `Requirement` ne porte d'ailleurs aucune trace de l'identifiant.
#[test]
fn the_resolution_only_depends_on_the_segment_count() {
    let auth = distinct_per_operation("order");
    let authz = authorization(
        vec![entity_config("order", "orders", auth.clone())],
        &[("order", "orders")],
    );

    let long = "a".repeat(4096);
    let identifiers = [
        "42",
        "0",
        "00000000-0000-4000-8000-000000000000",
        "%2F",
        "%2e%2e%2f",
        "a%2Fb",
        "..",
        "-",
        " ",
        "тридцать",
        "id with spaces?",
        long.as_str(),
    ];

    let baseline_get = authz.requirement(&Method::GET, "/orders/42");
    let baseline_delete = authz.requirement(&Method::DELETE, "/orders/42");

    for id in identifiers {
        let context = format!("GET /orders/{id}");
        let requirement = authz.requirement(&Method::GET, &format!("/orders/{id}"));
        assert_eq!(
            requirement, baseline_get,
            "{context} : l'identifiant ne doit pas changer la résolution"
        );
        assert_eq!(resolved(&requirement, &context).1, "get");

        // …et l'opération suit la méthode, toujours pour deux segments.
        assert_eq!(
            resolved(
                &authz.requirement(&Method::DELETE, &format!("/orders/{id}")),
                &context
            )
            .1,
            "delete"
        );
        assert_eq!(
            authz.requirement(&Method::DELETE, &format!("/orders/{id}")),
            baseline_delete
        );
    }

    // Cinq segments : l'opération de lien ne dépend pas non plus des
    // identifiants qui la composent.
    let baseline_link = authz.requirement(&Method::POST, "/orders/42/items/product/7");
    for path in [
        "/orders/00000000-0000-4000-8000-000000000000/items/product/7",
        "/orders/42/items/product/00000000-0000-4000-8000-000000000000",
        "/orders/42/a%2Fb/c%2Fd/%2F",
        "/orders/42/x/y/z",
    ] {
        let context = format!("POST {path}");
        assert_eq!(
            resolved(&authz.requirement(&Method::POST, path), &context).1,
            "create_link",
            "{context} : cinq segments en POST ⇒ `create_link`"
        );
    }
    assert_eq!(resolved(&baseline_link, "baseline").1, "create_link");
}

// ── 6. Query string ─────────────────────────────────────────────────────────

/// **C'est au routeur de séparer la query string, pas au résolveur.** Le
/// résolveur ne voit que le chemin que le middleware lui passe
/// (`request.uri().path()`, qui exclut la query) ; si on lui donne une chaîne
/// brute contenant `?`, elle devient un **segment**.
///
/// Conséquences verrouillées ici : sur un pluriel seul, la query fait échouer la
/// reconnaissance (⇒ abstention, donc aucune politique de complaisance) ; sur
/// une forme à deux segments, la query se retrouve dans l'identifiant et
/// l'opération reste correcte — l'identifiant n'étant jamais interprété.
#[test]
fn a_query_string_is_not_stripped_by_the_resolver() {
    let auth = distinct_per_operation("order");
    let authz = authorization(
        vec![entity_config("order", "orders", auth.clone())],
        &[("order", "orders")],
    );

    // Pluriel seul : le `?` adhère au segment, la reconnaissance échoue.
    abstention_reason(
        &authz.requirement(&Method::GET, "/orders?counts=true"),
        "GET /orders?counts=true",
    );

    // Deux segments : même résolution qu'avec ou sans query, car la résolution
    // repose sur le NOMBRE de segments.
    let with_query = authz.requirement(&Method::GET, "/orders/42?counts=true");
    assert_eq!(
        with_query,
        authz.requirement(&Method::GET, "/orders/42"),
        "la query ne doit pas changer l'opération d'une forme à deux segments"
    );
    let (_, operation, policy, origin) = resolved(&with_query, "GET /orders/42?counts=true");
    assert_eq!(operation, "get");
    assert_eq!(&policy, &auth.get);
    assert_eq!(origin, PolicyOrigin::Declared);
}

// ── 7. Déterminisme ─────────────────────────────────────────────────────────

/// **Déterminisme** : mêmes entrées ⇒ même `Requirement`. Un résolveur qui
/// oscille entre deux politiques selon l'ordre d'un `HashMap` (registre,
/// configuration) serait une porte qui s'ouvre au hasard.
#[test]
fn the_same_request_always_resolves_to_the_same_requirement() {
    let authz = authorization(
        vec![
            entity_config("order", "orders", distinct_per_operation("order")),
            entity_config("invoice", "invoices", distinct_per_operation("invoice")),
        ],
        &[
            ("order", "orders"),
            ("invoice", "invoices"),
            ("payment", "payments"),
        ],
    );

    let mut requests: Vec<(Method, String)> = ["orders", "invoices", "payments"]
        .iter()
        .flat_map(|plural| shapes_for(plural))
        .map(|(method, path, _)| (method, path))
        .collect();
    requests.extend([
        (Method::GET, "/inconnu".to_string()),
        (Method::HEAD, "/orders".to_string()),
        (Method::GET, String::new()),
        (Method::GET, "/orders/42/a/b/c/d".to_string()),
    ]);

    for (method, path) in &requests {
        let context = format!("{method} {path}");
        let first = authz.requirement(method, path);
        for iteration in 0..100 {
            assert_eq!(
                authz.requirement(method, path),
                first,
                "{context} : résolution instable à l'itération {iteration}"
            );
        }
    }
}

// ── 8. Sérialisation ────────────────────────────────────────────────────────

/// Le discriminant `kind` et les champs `entity_type`, `operation`, `policy`,
/// `origin` sont un **contrat** : un journal ou un refus HTTP qui les renomme
/// rend la décision inexploitable.
#[test]
fn a_policy_requirement_serialises_with_its_discriminant_and_fields() {
    let authz = authorization(
        vec![entity_config(
            "order",
            "orders",
            distinct_per_operation("order"),
        )],
        &[("order", "orders")],
    );

    let value = serde_json::to_value(authz.requirement(&Method::GET, "/orders")).expect("JSON");
    assert_eq!(
        value,
        serde_json::json!({
            "kind": "policy",
            "entity_type": "order",
            "operation": "list",
            "policy": "order:list",
            "origin": "declared",
        })
    );

    // `origin` suit la provenance, y compris quand aucune entrée n'existe.
    let undeclared = authorization(Vec::new(), &[("order", "orders")]);
    let value =
        serde_json::to_value(undeclared.requirement(&Method::GET, "/orders")).expect("JSON");
    assert_eq!(value["origin"], serde_json::json!("default"));
    assert_eq!(value["kind"], serde_json::json!("policy"));

    // Et l'origine est bien embarquée dans le `Requirement`, pas seulement dans
    // la décision.
    assert_eq!(
        resolved(&undeclared.requirement(&Method::GET, "/orders"), "défaut").3,
        PolicyOrigin::Default
    );
}

/// L'abstention se sérialise avec son propre discriminant et **sa raison**.
#[test]
fn an_abstention_serialises_with_its_reason() {
    let authz = authorization(Vec::new(), &[("order", "orders")]);

    let value = serde_json::to_value(authz.requirement(&Method::GET, "/health")).expect("JSON");
    let object = value.as_object().expect("objet JSON");

    assert_eq!(object.get("kind"), Some(&serde_json::json!("abstain")));
    let reason = object
        .get("reason")
        .and_then(|r| r.as_str())
        .expect("une abstention sérialisée porte sa raison");
    assert!(!reason.trim().is_empty(), "raison vide : {value}");
    assert_eq!(
        object.len(),
        2,
        "abstention : `kind` et `reason`, rien d'autre"
    );
}

// ── 9. Deux entités, deux politiques : le test qui attrape un mélange ───────

/// Deux entités enregistrées avec des politiques **disjointes** : chaque pluriel
/// rend **sa** politique, pour les huit opérations. Un mélange entre entités
/// ferait passer ici une politique qui n'appartient pas à l'entité visée.
#[test]
fn two_entities_never_exchange_their_policies() {
    let order = distinct_per_operation("order");
    let invoice = distinct_per_operation("invoice");

    // Montage : les deux jeux doivent être étrangers l'un à l'autre.
    let eight_order = the_eight(&order);
    let eight_invoice = the_eight(&invoice);
    let order_policies: HashSet<&String> = eight_order.iter().map(|(_, p)| p).collect();
    let invoice_policies: HashSet<&String> = eight_invoice.iter().map(|(_, p)| p).collect();
    assert!(
        order_policies.is_disjoint(&invoice_policies),
        "montage de test invalide : les politiques des deux entités doivent être distinctes"
    );

    let authz = authorization(
        vec![
            entity_config("order", "orders", order.clone()),
            entity_config("invoice", "invoices", invoice.clone()),
        ],
        &[("order", "orders"), ("invoice", "invoices")],
    );

    for (entity_type, plural, auth) in [
        ("order", "orders", &order),
        ("invoice", "invoices", &invoice),
    ] {
        // Garde-fou : le pluriel de l'autre entité ne doit pas être reconnu ici.
        for (method, path, operation) in shapes_for(plural) {
            let context = format!("{method} {path}");
            let (resolved_entity, resolved_operation, policy, origin) =
                resolved(&authz.requirement(&method, &path), &context);

            assert_eq!(resolved_entity, entity_type, "{context} : mauvaise entité");
            assert_eq!(resolved_operation, operation, "{context}");
            assert_eq!(
                &policy,
                policy_for(auth, operation),
                "{context} : politique de `{entity_type}` attendue"
            );
            assert_eq!(origin, PolicyOrigin::Declared, "{context}");
        }
    }
}

/// **Ambiguïté silencieuse, documentée ici** : deux entités enregistrées sous le
/// même pluriel — le registre les accepte, car il est indexé par `entity_type` —
/// et la résolution choisit la première par ordre de `entity_type`. Le choix est
/// déterministe, mais il n'est **pas** signalé : c'est la politique d'une entité
/// qui s'applique à la route de l'autre. En production, deux routeurs montés sur
/// le même chemin font échouer le montage axum ; le cas n'est donc atteignable
/// que par un montage inhabituel, mais il ne doit pas être découvert par hasard.
#[test]
fn a_plural_claimed_by_two_entities_is_resolved_deterministically_and_silently() {
    let order = distinct_per_operation("order");
    let invoice = distinct_per_operation("invoice");
    let authz = authorization(
        vec![
            entity_config("order", "things", order.clone()),
            entity_config("invoice", "things", invoice.clone()),
        ],
        &[("order", "things"), ("invoice", "things")],
    );

    // L'ambiguïté est réelle : le registre porte bien deux entités pour `things`.
    let registered = registry(&[("order", "things"), ("invoice", "things")]);
    let plurals = registered.entity_plurals();
    let claiming: Vec<&String> = plurals
        .iter()
        .filter(|(_, plural)| plural == "things")
        .map(|(entity_type, _)| entity_type)
        .collect();
    assert_eq!(
        claiming.len(),
        2,
        "le registre doit porter les deux entités"
    );

    let first = resolved(&authz.requirement(&Method::GET, "/things"), "GET /things");
    assert_eq!(
        first.0, "invoice",
        "le premier `entity_type` dans l'ordre du registre gagne : {first:?}"
    );
    assert_eq!(
        first.2, invoice.list,
        "c'est donc la politique d'`invoice` qui s'applique à `/things`"
    );

    for _ in 0..100 {
        assert_eq!(
            resolved(&authz.requirement(&Method::GET, "/things"), "GET /things"),
            first,
            "le choix doit au moins être stable"
        );
    }
}

/// Routes inconnues : abstention, jamais invention. Un préfixe devant le pluriel
/// suffit à sortir du domaine connu (aucun support de préfixe global ici).
#[test]
fn unknown_routes_abstain_rather_than_inventing_a_policy() {
    let authz = authorization(
        vec![entity_config(
            "order",
            "orders",
            distinct_per_operation("order"),
        )],
        &[("order", "orders")],
    );

    for (method, path) in [
        (Method::GET, "/health"),
        (Method::GET, "/graph/identity"),
        (Method::GET, "/auth/login"),
        (Method::POST, "/auth/login"),
        (Method::GET, "/inconnus"),
        (Method::GET, "/orders2/42"),
        (Method::GET, "/api/orders"),
        (Method::GET, "/api/orders/42"),
    ] {
        abstention_reason(
            &authz.requirement(&method, path),
            &format!("{method} {path}"),
        );
    }
}

// ── Défauts constatés ───────────────────────────────────────────────────────

/// **Défaut constaté** — la configuration est indexée par `singular`, alors que
/// le reste du crate indexe par `plural` (`core::extractors`,
/// `links::handlers` cherchent `e.plural == <pluriel de la route>`).
///
/// Conséquence : une entrée de configuration dont le `plural` **est** celui de la
/// route montée, mais dont le `singular` ne coïncide pas avec
/// `EntityDescriptor::entity_type()`, est ignorée par la résolution. La
/// politique déclarée est alors remplacée par la politique **par défaut du
/// framework** (`authenticated`), c'est-à-dire par une politique **plus faible**
/// que celle qui est écrite dans la configuration (`admin_only` ici) : la porte
/// s'ouvre plus grand que ce qui est déclaré, et seule la provenance
/// (`origin: default`) le laisse deviner.
#[test]
fn a_config_entry_naming_the_route_plural_must_not_be_ignored() {
    let mut auth = EntityAuthConfig::default();
    auth.delete = "admin_only".to_string();

    // La configuration nomme le pluriel de la route : `orders`.
    let authz = authorization(
        vec![entity_config("orders", "orders", auth)],
        &[("order", "orders")],
    );

    let (entity_type, operation, policy, origin) = resolved(
        &authz.requirement(&Method::DELETE, "/orders/1"),
        "DELETE /orders/1",
    );

    assert_eq!(entity_type, "order");
    assert_eq!(operation, "delete");
    assert_eq!(
        policy, "admin_only",
        "la politique déclarée pour le pluriel `orders` doit s'appliquer"
    );
    assert_eq!(origin, PolicyOrigin::Declared);
}

/// Le miroir du défaut ci-dessus, **passant** : la résolution ne lit jamais le
/// champ `plural` de la configuration. Une entrée dont le `plural` désigne une
/// toute autre route s'applique quand même, dès que le `singular` coïncide avec
/// `entity_type()`. Déclaré ≠ appliqué à la route annoncée : le champ `plural`
/// de la configuration est décoratif pour l'autorisation.
#[test]
fn the_config_plural_field_does_not_participate_in_resolution() {
    let mut auth = EntityAuthConfig::default();
    auth.delete = "admin_only".to_string();

    // `plural` annonce `commandes`, mais la route montée est `/orders`.
    let authz = authorization(
        vec![entity_config("order", "commandes", auth)],
        &[("order", "orders")],
    );

    let (_, operation, policy, origin) = resolved(
        &authz.requirement(&Method::DELETE, "/orders/1"),
        "DELETE /orders/1",
    );
    assert_eq!(operation, "delete");
    assert_eq!(policy, "admin_only");
    assert_eq!(
        origin,
        PolicyOrigin::Declared,
        "le `plural` de la configuration n'entre pas dans la résolution"
    );

    // Et le pluriel annoncé par la configuration n'est pas une route d'entité.
    abstention_reason(
        &authz.requirement(&Method::DELETE, "/commandes/1"),
        "DELETE /commandes/1",
    );
}

// ── Le contexte qui donne son sens à l'abstention sur HEAD ──────────────────

/// Pourquoi l'abstention sur HEAD (verrouillée plus haut) mérite d'être connue :
/// axum sert HEAD **par le handler GET** (`get(...)` enregistre GET et HEAD).
/// Une requête HEAD atteint donc la route d'entité alors que le résolveur, lui,
/// s'abstient — la politique déclarée pour cette route n'est pas appliquée à
/// cette requête, et seule la couche d'authentification globale reste juge.
#[tokio::test]
async fn axum_serves_head_requests_through_the_get_route() {
    use tower::ServiceExt;

    let app = axum::Router::new().route("/orders", axum::routing::get(|| async { "liste" }));

    for method in [Method::GET, Method::HEAD] {
        let response = app
            .clone()
            .oneshot(
                axum::http::Request::builder()
                    .method(method.clone())
                    .uri("/orders")
                    .body(axum::body::Body::empty())
                    .expect("requête"),
            )
            .await
            .expect("réponse");

        assert_eq!(
            response.status(),
            StatusCode::OK,
            "{method} /orders : axum doit servir HEAD par la route GET"
        );
    }
}
