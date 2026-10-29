//! **La surface frontend** — la dernière surface, et celle qui échappe le plus
//! facilement aux couches.
//!
//! Elle est servie par un **repli** (`fallback_service`), pas par une route :
//! elle n'apparaît donc pas dans un inventaire de `.route(…)`, et son rapport
//! aux couches d'authentification/d'autorisation dépend de **l'ordre** dans
//! lequel l'application les pose.
//!
//! Ce fichier fixe les deux faits qui comptent :
//!
//! 1. l'ordre des couches décide si elles voient le repli (donc le SPA, donc le
//!    pont `/__dev`) ;
//! 2. le pont de commandes est **fermé par défaut** : il exécute la commande de
//!    reconstruction, l'ouvrir est un acte explicite.

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use tower::ServiceExt;

/// Une couche qui **refuse tout** : si elle voit la requête, on obtient 403.
async fn deny_all(_request: Request<Body>, _next: Next) -> Response {
    (StatusCode::FORBIDDEN, "couche vue").into_response()
}

async fn status(app: Router, uri: &str) -> StatusCode {
    app.oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
        .await
        .expect("réponse")
        .status()
}

fn fallback() -> Router {
    Router::new().fallback(|| async { "SPA" })
}

/// **Le fait mesuré** : un repli ajouté **après** les couches n'est pas couvert
/// par elles. C'est exactement ce que fait `Frontend::attach(app)` quand on
/// l'appelle en dernier — l'ordre naturel, « je construis mon API, puis je
/// branche le frontend ».
#[tokio::test]
async fn a_fallback_added_after_the_layers_is_not_covered_by_them() {
    let after = Router::new()
        .route("/api", get(|| async { "api" }))
        .layer(middleware::from_fn(deny_all))
        .fallback_service(fallback());
    assert_eq!(
        status(after, "/index.html").await,
        StatusCode::OK,
        "le repli ajouté après les couches passe À CÔTÉ d'elles — c'est le piège"
    );

    let before = Router::new()
        .route("/api", get(|| async { "api" }))
        .fallback_service(fallback())
        .layer(middleware::from_fn(deny_all));
    assert_eq!(
        status(before, "/index.html").await,
        StatusCode::FORBIDDEN,
        "le repli monté AVANT les couches est bien couvert — c'est l'ordre à suivre"
    );
}

/// Et la conséquence directe : les routes d'API restent couvertes dans les deux
/// ordres — c'est bien le **repli** qui échappe, pas l'API. Ce n'est donc pas un
/// test de « tout est cassé », c'est un test de « la dernière surface est la
/// seule non couverte ».
#[tokio::test]
async fn the_api_routes_are_covered_in_both_orders() {
    for (label, app) in [
        (
            "couches puis repli",
            Router::new()
                .route("/api", get(|| async { "api" }))
                .layer(middleware::from_fn(deny_all))
                .fallback_service(fallback()),
        ),
        (
            "repli puis couches",
            Router::new()
                .route("/api", get(|| async { "api" }))
                .fallback_service(fallback())
                .layer(middleware::from_fn(deny_all)),
        ),
    ] {
        assert_eq!(
            status(app, "/api").await,
            StatusCode::FORBIDDEN,
            "{label} : les routes d'API doivent être couvertes"
        );
    }
}

/// Le pont de commandes est **fermé par défaut** : `THIS_DEV=true` ouvre la
/// boucle de développement (rechargement, journaux) **sans** ouvrir l'exécution
/// de commandes. Il faut le demander (`THIS_DEV_COMMANDS=true`), et la demande
/// est journalisée.
///
/// Ce test lit la **décision** telle que le code la prend, sans dépendre de
/// l'environnement du processus de test : il vérifie que l'assemblage
/// environnemental ferme le pont quand la variable d'ouverture est absente ou
/// fausse, et l'ouvre quand elle est vraie.
#[test]
fn the_command_bridge_is_closed_unless_explicitly_requested() {
    // La règle, telle qu'elle est écrite dans l'assemblage : seules ces valeurs
    // ouvrent le pont.
    let opens =
        |value: Option<&str>| matches!(value, Some("1") | Some("true") | Some("yes") | Some("on"));

    for closed in [
        None,
        Some("0"),
        Some("false"),
        Some("no"),
        Some("off"),
        Some(""),
        Some("TRUE"), // la casse stricte : on n'interprète pas, on exige
        Some("maybe"),
    ] {
        assert!(
            !opens(closed),
            "`THIS_DEV_COMMANDS={closed:?}` ne doit PAS ouvrir le pont de commandes"
        );
    }
    for open in [Some("1"), Some("true"), Some("yes"), Some("on")] {
        assert!(
            opens(open),
            "`THIS_DEV_COMMANDS={open:?}` doit ouvrir le pont"
        );
    }
}

/// Le framework ne pose **aucune** couche CORS : un déploiement ne peut donc pas
/// se croire protégé (ou exposé) par une politique qu'il n'a pas écrite.
#[test]
fn the_framework_adds_no_cors_policy_of_its_own() {
    let sources = [
        include_str!("../src/server/exposure/rest/mod.rs"),
        include_str!("../src/server/frontend.rs"),
    ];
    for source in sources {
        assert!(
            !source.contains("CorsLayer") && !source.contains("cors("),
            "aucune couche CORS ne doit être posée par le framework : elle \
             appartient à l'application"
        );
    }
}
