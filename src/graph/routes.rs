//! Publication REST du relief d'obrain.
//!
//! Trois points d'entrée, montés par `RestExposure` **uniquement** quand un
//! `ObrainStore` est attaché au `ServerHost` :
//!
//! | Route | Contenu |
//! |---|---|
//! | `GET /graph` | le document complet ([`Relief`]) — `?counts=true` ajoute les compteurs de simplexes (balayage complet, opt-in) |
//! | `GET /graph/procedures` | la surface d'appel : procédures introspectées + familles déclarées + pièges |
//! | `GET /graph/spaces` | l'espace actif et les espaces connus |
//!
//! Aucune de ces routes n'écrit, et aucune ne rend d'erreur pour un fait
//! simplement indisponible : c'est le contrat du document (cf. `relief.rs`).

use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use serde::{Deserialize, Serialize};

use crate::graph::canvas::CanvasState;
use crate::graph::hierarchy;
use crate::storage::permission::{ReadScope, Scale};
use serde_json::json;
use uuid::Uuid;

use crate::core::identity::{IdentityRegistry, MotiveSpec};
use crate::graph::relief::{FamilySpec, ProcedureSpec, Relief, ReliefOptions, Trap};
use crate::storage::ObrainStore;
use crate::storage::obrain_identity::IdentityReport;

/// État partagé des routes de relief.
#[derive(Clone)]
pub struct GraphState {
    /// Le store obrain dont on publie le relief.
    pub store: Arc<ObrainStore>,
    /// Comment les entités sont nommées, **quel que soit le moteur**.
    ///
    /// C'est ce registre qui décide si une base écrite par un autre outil
    /// (propriétés plates, sans `_uuid`) est lisible ou non.
    pub identity: Arc<IdentityRegistry>,
    /// **Qui lit, et ce qu'il a le droit de voir.**
    ///
    /// Absent : lecture non restreinte (le relief descriptif d'avant). Présent :
    /// chaque lecture demande au résolveur le périmètre du **demandeur**, donc
    /// ses permissions IAM décident des labels, des types de relations et des
    /// échelles visibles. C'est ce qui rend la granularité **effective par
    /// requête**.
    pub scope: Option<Arc<dyn ScopeResolver>>,
    /// **L'état d'interface de l'appelant**, s'il en a un.
    ///
    /// Présent : `?echelle=canvas` prend l'échelle que l'utilisateur a sous les
    /// yeux, et `self` prend son sujet. Absent : ces deux formes sont refusées
    /// avec un message clair — jamais interprétées comme « aucun sujet ».
    pub canvas: Option<Arc<dyn CanvasState>>,
}

/// **Résout le périmètre de lecture d'une requête.**
///
/// Le framework en fournit une implémentation adossée à l'IAM
/// (`IamScopeResolver`) ; une application peut la remplacer — c'est une couture,
/// pas une politique en dur.
pub trait ScopeResolver: Send + Sync {
    /// Le périmètre du demandeur, d'après son contexte d'authentification.
    ///
    /// **Sans contexte, rien n'est accordé** : un appelant non identifié ne voit
    /// aucune donnée, et les compteurs de masquage le disent.
    fn scope_for(&self, context: Option<&crate::core::auth::AuthContext>) -> ReadScope;
}

impl GraphState {
    /// L'état minimal : le store et l'identité, **sans** restriction.
    pub fn new(store: Arc<ObrainStore>, identity: Arc<IdentityRegistry>) -> Self {
        Self {
            store,
            identity,
            scope: None,
            canvas: None,
        }
    }

    /// **Relie l'état d'interface de l'appelant** aux lectures de graphe.
    pub fn with_canvas(mut self, canvas: Arc<dyn CanvasState>) -> Self {
        self.canvas = Some(canvas);
        self
    }

    /// **Restreint les lectures au périmètre du demandeur.**
    pub fn with_scope_resolver(mut self, resolver: Arc<dyn ScopeResolver>) -> Self {
        self.scope = Some(resolver);
        self
    }
}

/// Construit les routes du relief.
///
/// Fusionnées telles quelles dans le routeur REST — elles n'installent aucun
/// fallback, donc `Router::merge` reste sûr.
pub fn graph_routes(state: GraphState) -> Router {
    Router::new()
        .route("/graph", get(relief_handler))
        .route("/graph/procedures", get(procedures_handler))
        .route("/graph/spaces", get(spaces_handler))
        .route("/graph/identity", get(identity_handler))
        .route("/graph/identity/{id}", get(identity_get_handler))
        // **La vue multi-échelle** : descendre d'un groupe à ses sous-groupes,
        // niveau par niveau, sans jamais charger tout le graphe.
        .route("/graph/hierarchy/{id}", get(hierarchy_handler))
        .with_state(state)
}

/// Paramètres de `GET /graph`.
#[derive(Debug, Default, Deserialize)]
struct ReliefQuery {
    /// Calculer les compteurs de simplexes — **balayage complet**, opt-in.
    #[serde(default)]
    counts: bool,
}

/// `GET /graph` — le document de relief.
async fn relief_handler(
    State(state): State<GraphState>,
    Query(q): Query<ReliefQuery>,
    context: Option<axum::Extension<crate::core::auth::AuthContext>>,
) -> Response {
    let opts = ReliefOptions {
        include_counts: q.counts,
    };
    let scope = resolve_scope(&state, context.as_ref());
    match describe_scoped(state.store, opts, scope).await {
        Ok(relief) => Json(relief).into_response(),
        Err(e) => join_error(e),
    }
}

/// `GET /graph/procedures` — la surface d'appel.
///
/// Sépare explicitement ce qui est **introspecté** (le registre du moteur) de
/// ce qui est **déclaré** (les familles de dispatch qui n'y figurent pas), et
/// joint les pièges connus : un client qui découvre la surface par cette route
/// n'a pas besoin d'aller les chercher ailleurs.
async fn procedures_handler(State(state): State<GraphState>) -> Response {
    match describe(state.store, ReliefOptions::default()).await {
        Ok(relief) => Json(CallSurface {
            procedures: relief.procedures,
            families: relief.families,
            traps: relief.traps,
            notes: relief.notes,
        })
        .into_response(),
        Err(e) => join_error(e),
    }
}

/// `GET /graph/spaces` — espace actif et espaces connus.
async fn spaces_handler(State(state): State<GraphState>) -> Response {
    match describe(state.store, ReliefOptions::default()).await {
        Ok(relief) => Json(relief.space).into_response(),
        Err(e) => join_error(e),
    }
}

/// La surface d'appel, telle que publiée par `GET /graph/procedures`.
#[derive(Debug, Serialize)]
struct CallSurface {
    /// Procédures du registre — introspectées.
    procedures: Vec<ProcedureSpec>,
    /// Familles hors registre — déclarées.
    families: Vec<FamilySpec>,
    /// Limites connues, adossées à une preuve.
    traps: Vec<Trap>,
    /// Notes de lecture.
    notes: Vec<String>,
}

/// Décrit le relief hors du thread async.
///
/// `Relief::describe` interroge le moteur de façon synchrone — et avec
/// `include_counts`, il **balaye la zone simplex entière**. Le faire sur le
/// réacteur bloquerait les autres requêtes ; on le pousse donc sur le pool
/// bloquant.
async fn describe(
    store: Arc<ObrainStore>,
    opts: ReliefOptions,
) -> Result<Relief, tokio::task::JoinError> {
    tokio::task::spawn_blocking(move || Relief::describe(&store, opts)).await
}

/// Le même calcul, **sous un périmètre de lecture** — donc sur le chemin d'une
/// requête identifiée.
async fn describe_scoped(
    store: Arc<ObrainStore>,
    opts: ReliefOptions,
    scope: Arc<ReadScope>,
) -> Result<Relief, tokio::task::JoinError> {
    tokio::task::spawn_blocking(move || Relief::describe_scoped(&store, opts, &scope)).await
}

/// `Relief::describe` ne rend jamais d'erreur métier : la seule issue d'échec
/// est la mort de la tâche bloquante (panique). On la rapporte telle quelle.
fn join_error(e: tokio::task::JoinError) -> Response {
    envelope(
        StatusCode::INTERNAL_SERVER_ERROR,
        json!({ "error": format!("relief introspection task failed: {e}") }),
    )
}

/// Error response of the relief routes: the contract envelope (code, kind, message_key,
/// fields; localized by `localize_errors`) chosen by `status`, PLUS the legacy keys of `body`
/// (`error`, `reason`, `hint`...), unchanged. A 500 never carries the internal text
/// (logged instead).
fn envelope(status: StatusCode, body: serde_json::Value) -> Response {
    use crate::core::errors as ce;
    let text = body
        .get("reason")
        .or_else(|| body.get("error"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let mut extras = body.as_object().cloned().unwrap_or_default();
    let err = match status.as_u16() {
        401 => ce::unauthenticated(),
        403 => ce::forbidden(&text),
        404 => ce::not_found("graph", &text),
        409 => ce::conflict("graph"),
        501 => crate::descriptor::ContractError::framework(
            crate::descriptor::framework::codes::NOT_IMPLEMENTED,
        )
        .with_param("what", &text),
        500 => {
            tracing::error!(detail = %text, "graph relief: internal failure");
            extras.clear();
            ce::internal()
        }
        _ => ce::bad_request(&text),
    };
    ce::error_response_with(err, Some(status), extras)
}

// ── Identité canonique ────────────────────────────────────────────────

/// Bornes du comptage exposé par HTTP.
///
/// Le balayage complet d'un store réel se compte en **minutes** (mesuré :
/// 1 m 53 s sur 1,55 M de nœuds). Il n'est donc **jamais** servi : la route
/// borne, et publie `truncated` quand la borne mord.
const IDENTITY_COUNT_MAX_PER_LABEL: usize = 2_000;
const IDENTITY_COUNT_MAX_TOTAL: usize = 20_000;

/// Paramètres de `GET /graph/identity`.
#[derive(Debug, Default, Deserialize)]
struct IdentityQuery {
    /// Compter les entités — **balayage borné**, opt-in.
    #[serde(default)]
    counts: bool,
}

/// Ce que la route publie : une **déclaration**, et des compteurs sur demande.
#[derive(Debug, Serialize)]
struct IdentityDoc {
    /// Identifiant de schéma du document.
    schema: String,
    /// Schéma d'identité en vigueur.
    scheme: String,
    /// Espace du store au moment de la réponse.
    space: Option<String>,
    /// Repli en vigueur (voir `Fallback`).
    fallback: crate::core::identity::Fallback,
    /// La chaîne de motifs, dans l'ordre, avec les propriétés examinées.
    motives: Vec<MotiveSpec>,
    /// Labels vus par le moteur (`all_labels()`).
    labels: Vec<String>,
    /// Pourquoi la liste de labels est vide, le cas échéant.
    labels_error: Option<String>,
    /// Entités actuellement indexées (index en mémoire).
    index_len: usize,
    /// Espace de l'index.
    index_space: Option<String>,
    /// Le rapport de comptage — présent seulement si `?counts=true`.
    counts: Option<IdentityReport>,
    /// Bornes appliquées au comptage (publiées : le client doit pouvoir juger).
    count_bounds: CountBounds,
    /// Limites connues, adossées à une preuve.
    traps: Vec<Trap>,
    /// Notes de lecture.
    notes: Vec<String>,
}

/// Les bornes du comptage, publiées.
#[derive(Debug, Serialize)]
struct CountBounds {
    /// Plafond par label.
    max_per_label: usize,
    /// Plafond global.
    max_total: usize,
    /// Le balayage complet est-il possible ? (non : il se compte en minutes)
    full_scan_served: bool,
}

/// `GET /graph/identity` — la déclaration d'identité, et les compteurs sur demande.
async fn identity_handler(
    State(state): State<GraphState>,
    Query(q): Query<IdentityQuery>,
) -> Response {
    let store = state.store.clone();
    let registry = state.identity.clone();
    let with_counts = q.counts;

    let doc = tokio::task::spawn_blocking(move || {
        let labels = crate::storage::obrain_identity::discover_labels(&store);
        // Une liste vide ne se lit PAS « aucune entité » : l'API du moteur ne
        // permet pas de distinguer « store vide » de « backend non
        // introspectable » (le trait rend `Vec::new()` par défaut). On publie
        // l'ambiguïté au lieu de trancher à tort.
        let labels_error = labels.is_empty().then(|| {
            "aucun label rendu par le moteur : store vide OU introspection indisponible — les \
             deux cas sont INDISCERNABLES par l'API actuelle"
                .to_string()
        });

        let counts = with_counts.then(|| {
            store.rebuild_identity_index_capped(
                &registry,
                IDENTITY_COUNT_MAX_PER_LABEL,
                IDENTITY_COUNT_MAX_TOTAL,
            )
        });

        let mut notes = vec![
            "L'index d'identité est en mémoire et REPART DE ZÉRO à chaque ouverture du store, \
             ainsi qu'à chaque bascule d'espace (`use_graph`/`reset_graph`) : les deux espaces \
             allouent leurs identifiants internes à partir de 1 et ne peuvent pas partager un index."
                .to_string(),
            "Une identité dérivée est un uuid de VERSION 8 : elle est reconnaissable, et ne peut \
             pas être confondue avec un uuid v4 écrit par la convention `_uuid`/`_data`."
                .to_string(),
        ];
        if with_counts {
            notes.push(format!(
                "Comptage borné à {IDENTITY_COUNT_MAX_PER_LABEL} nœuds par label et \
                 {IDENTITY_COUNT_MAX_TOTAL} au total. Le balayage complet d'une base réelle se \
                 compte en minutes : utiliser l'outil `adopt_obrain_base`, pas cette route."
            ));
        } else {
            notes.push(
                "Comptage non demandé : `counts` est absent, ce qui ne signifie PAS « aucune \
                 entité ». Ajouter `?counts=true` pour un comptage borné."
                    .to_string(),
            );
        }

        IdentityDoc {
            schema: "this-rs/graph-identity/1".to_string(),
            scheme: crate::core::identity::ENTITY_ID_SCHEME.to_string(),
            space: store.space(),
            fallback: registry.fallback(),
            motives: registry.motives(),
            labels,
            labels_error,
            index_len: store.identity_len(),
            index_space: store.identity_space(),
            counts,
            count_bounds: CountBounds {
                max_per_label: IDENTITY_COUNT_MAX_PER_LABEL,
                max_total: IDENTITY_COUNT_MAX_TOTAL,
                full_scan_served: false,
            },
            traps: identity_traps(),
            notes,
        }
    })
    .await;

    match doc {
        Ok(doc) => Json(doc).into_response(),
        Err(e) => join_error(e),
    }
}

/// **Le sujet de l'appelant**, résolu en identifiant de nœud.
///
/// Quatre refus explicites, jamais un défaut silencieux : pas d'état d'interface
/// branché, pas d'identification, pas de sujet déclaré, ou un sujet ambigu
/// (plusieurs nœuds portent ce nom) ou introuvable. Une lecture qui partirait
/// « de nulle part » serait pire qu'un refus : elle montrerait autre chose que ce
/// que l'utilisateur regarde.
fn resolve_subject(
    state: &GraphState,
    context: Option<&axum::Extension<crate::core::auth::AuthContext>>,
) -> Result<String, Box<Response>> {
    let refus = |status: StatusCode, corps: serde_json::Value| -> Box<Response> {
        Box::new(envelope(status, corps))
    };

    let Some(canvas) = &state.canvas else {
        return Err(refus(
            StatusCode::NOT_IMPLEMENTED,
            json!({
                "error": "canvas_absent",
                "reason": "cette instance ne publie pas d'état d'interface : désignez le nœud \
                           par son identifiant numérique",
            }),
        ));
    };
    let Some(portee) = canvas_portee(context) else {
        return Err(refus(
            StatusCode::UNAUTHORIZED,
            json!({
                "error": "portee_inconnue",
                "reason": "le sujet est une ressource PERSONNELLE : identifiez-vous",
            }),
        ));
    };
    let Some((store_name, qn)) = canvas.subject_of(&portee) else {
        return Err(refus(
            StatusCode::NOT_FOUND,
            json!({
                "error": "sujet_non_declare",
                "reason": "votre état d'interface ne déclare pas de sujet",
            }),
        ));
    };

    // Un sujet qui nomme un espace différent de celui qu'on sert : on le dit,
    // plutôt que de chercher dans le mauvais.
    if let (false, Some(servi)) = (store_name.is_empty(), state.store.space())
        && store_name != servi
    {
        return Err(refus(
            StatusCode::CONFLICT,
            json!({
                "error": "espace_divergent",
                "sujet": store_name,
                "servi": servi,
                "reason": "le sujet de votre interface appartient à un autre espace que celui-ci",
            }),
        ));
    }

    let trouves = state.store.active_store().find_nodes_by_property(
        "qualified_name",
        &obrain_common::types::Value::from(qn.as_str()),
    );
    match trouves.as_slice() {
        [] => Err(refus(
            StatusCode::NOT_FOUND,
            json!({
                "error": "sujet_introuvable",
                "sujet": qn,
                "reason": "aucun nœud ne porte ce `qualified_name` dans cet espace",
            }),
        )),
        [unique] => Ok(unique.as_u64().to_string()),
        plusieurs => Err(refus(
            StatusCode::CONFLICT,
            json!({
                "error": "sujet_ambigu",
                "sujet": qn,
                "candidats": plusieurs.len(),
                "reason": "plusieurs nœuds portent ce `qualified_name` : désignez-le par son \
                           identifiant plutôt que par un nom ambigu",
            }),
        )),
    }
}

/// **La portée de l'état d'interface** : celle de l'appelant, jamais du chemin.
///
/// Même règle que `/notifications/*` : l'état d'interface est une ressource
/// **personnelle**, et l'identifiant du chemin n'est jamais cru sur parole. Ici il
/// n'y a même pas d'identifiant : la portée vient du contexte d'authentification.
fn canvas_portee(
    context: Option<&axum::Extension<crate::core::auth::AuthContext>>,
) -> Option<String> {
    match context.map(|axum::Extension(ctx)| ctx)? {
        crate::core::auth::AuthContext::User { user_id, .. }
        | crate::core::auth::AuthContext::Owner { user_id, .. } => Some(user_id.to_string()),
        crate::core::auth::AuthContext::Admin { admin_id } => Some(admin_id.to_string()),
        crate::core::auth::AuthContext::Service { service_name, .. } => Some(service_name.clone()),
        crate::core::auth::AuthContext::Anonymous => None,
    }
}

/// **Le périmètre du demandeur**, ou « tout » quand aucun résolveur n'est branché.
fn resolve_scope(
    state: &GraphState,
    context: Option<&axum::Extension<crate::core::auth::AuthContext>>,
) -> Arc<ReadScope> {
    match &state.scope {
        Some(resolver) => Arc::new(resolver.scope_for(context.map(|axum::Extension(ctx)| ctx))),
        // Sans résolveur, la lecture reste **descriptive** — le comportement
        // d'avant, et le défaut que le framework ne change pas sans qu'on le lui
        // demande.
        None => Arc::new(ReadScope::all()),
    }
}

/// Paramètres de `GET /graph/hierarchy/{id}`.
#[derive(Debug, Deserialize)]
struct HierarchyQuery {
    /// Profondeur à descendre (bornée par la spécification).
    depth: Option<usize>,
    /// Plafond de nœuds **par niveau**.
    limit: Option<usize>,
    /// **L'échelle de descente**, dans le vocabulaire de l'interface :
    /// `cosmos`, `monde`, `groupe`, `noeud`.
    ///
    /// Une échelle est une **intention** (jusqu'où je veux voir), une profondeur
    /// un **nombre** : donner l'échelle évite au client de traduire, et c'est ce
    /// que l'état d'interface d'un Canvas exprime déjà.
    echelle: Option<String>,
    /// `outline` : rendre la vue en **texte indenté**, pour un moteur
    /// d'inférence, plutôt qu'en JSON.
    format: Option<String>,
}

/// **`GET /graph/hierarchy/{id}`** — les sous-groupes, jusqu'à la profondeur
/// demandée.
///
/// Chaque niveau est borné, chaque troncature est **publiée** (`truncated`,
/// `total`), chaque cycle est **signalé** au lieu d'être parcouru. Parcours par
/// **adjacence** : le coût suit ce qui est montré, pas la taille de la base.
///
/// `?format=outline` rend la même information en texte indenté, avec l'adresse
/// à demander pour continuer — la forme qu'un modèle lit le mieux.
async fn hierarchy_handler(
    State(state): State<GraphState>,
    Path(id): Path<String>,
    Query(query): Query<HierarchyQuery>,
    context: Option<axum::Extension<crate::core::auth::AuthContext>>,
) -> Response {
    // **`self` : le sujet de l'appelant.** Ce qu'il regarde à l'écran devient le
    // point de départ de la descente — l'interface n'a plus à traduire.
    let id = if id == "self" {
        match resolve_subject(&state, context.as_ref()) {
            Ok(resolved) => resolved,
            Err(response) => return *response,
        }
    } else {
        id
    };

    let Ok(raw) = id.parse::<u64>() else {
        return envelope(
            StatusCode::BAD_REQUEST,
            json!({
                "error": format!("identifiant de nœud illisible : {id:?}"),
                "hint": "un nœud se désigne par son identifiant numérique (voir /graph), \
                         ou par `self` (le sujet de votre état d'interface)"
            }),
        );
    };

    let scope = resolve_scope(&state, context.as_ref());

    let mut spec = hierarchy::HierarchySpec::default();

    // **L'échelle d'abord** : elle porte l'intention, `depth`/`limit` la
    // précisent. Une échelle inconnue est refusée — jamais ramenée à un défaut
    // silencieux, qui ferait croire qu'on a obéi.
    // **`?echelle=canvas`** : l'échelle que l'utilisateur a sous les yeux.
    let echelle_demandee: Option<Scale> = match query.echelle.as_deref() {
        Some("canvas") => {
            let Some(canvas) = &state.canvas else {
                return envelope(
                    StatusCode::NOT_IMPLEMENTED,
                    json!({
                        "error": "canvas_absent",
                        "reason": "cette instance ne publie pas d'état d'interface : demandez une \
                                   échelle explicite (cosmos, monde, groupe, noeud)",
                    }),
                );
            };
            let Some(portee) = canvas_portee(context.as_ref()) else {
                return envelope(
                    StatusCode::UNAUTHORIZED,
                    json!({
                        "error": "portee_inconnue",
                        "reason": "l'échelle du canvas est une ressource PERSONNELLE : identifiez-vous",
                    }),
                );
            };
            match canvas.scale_of(&portee) {
                Some(scale) => Some(scale),
                None => {
                    return envelope(
                        StatusCode::NOT_FOUND,
                        json!({
                            "error": "echelle_non_declaree",
                            "reason": "votre état d'interface ne déclare pas d'échelle : \
                                       l'interface n'a rien dit, on n'invente pas",
                        }),
                    );
                }
            }
        }
        Some(raw) => match Scale::parse(raw) {
            Some(scale) => Some(scale),
            None => {
                return envelope(
                    StatusCode::BAD_REQUEST,
                    json!({
                        "error": format!("échelle inconnue : {raw:?}"),
                        "echelles": Scale::ALL.iter().map(|s| json!({
                            "echelle": s.as_str(),
                            "montre": s.meaning(),
                        })).chain(std::iter::once(json!({
                            "echelle": "canvas",
                            "montre": "l'échelle de VOTRE état d'interface",
                        }))).collect::<Vec<_>>(),
                    }),
                );
            }
        },
        None => None,
    };

    if let Some(scale) = echelle_demandee {
        // **Le plafond du demandeur** : l'interface propose, la décision dispose.
        if !scope.allows_scale(scale) {
            return envelope(
                StatusCode::FORBIDDEN,
                json!({
                    "error": "echelle_refusee",
                    "echelle": scale.as_str(),
                    "finest": scope.finest_scale().map(|s| s.as_str()),
                    "reason": "votre décision IAM ne couvre pas cette échelle : voyez \
                               `echelle:read:*` dans iam.vocabulary",
                }),
            );
        }
        let (depth, limit) = scale.descent();
        spec.max_depth = depth;
        spec.per_level_limit = limit;
    }

    if let Some(depth) = query.depth {
        // Borné : une profondeur absurde est ramenée à la borne, jamais
        // obéie telle quelle.
        spec.max_depth = depth.min(32);
    }
    if let Some(limit) = query.limit {
        spec.per_level_limit = limit.clamp(1, 1000);
    }

    let store = state.store.clone();
    let vue = tokio::task::spawn_blocking(move || {
        let node = obrain_common::types::NodeId::new(raw);
        // **La descente porte le périmètre** : ce qui est masqué est compté.
        hierarchy::describe_scoped(&store, node, spec, &scope)
    })
    .await;

    match vue {
        Ok(vue) => {
            if query.format.as_deref() == Some("outline") {
                (StatusCode::OK, vue.outline()).into_response()
            } else {
                Json(vue).into_response()
            }
        }
        Err(e) => envelope(
            StatusCode::INTERNAL_SERVER_ERROR,
            json!({ "error": format!("vue multi-échelle indisponible : {e}") }),
        ),
    }
}

async fn identity_get_handler(State(state): State<GraphState>, Path(id): Path<String>) -> Response {
    let Ok(uuid) = Uuid::parse_str(&id) else {
        return envelope(
            StatusCode::BAD_REQUEST,
            json!({
                "error": format!("identifiant illisible : {id:?}"),
                "hint": "un identifiant est un uuid ; les identités dérivées sont en version 8                          (voir GET /graph/identity)"
            }),
        );
    };

    let store = state.store.clone();
    let registry = state.identity.clone();
    let resolved =
        tokio::task::spawn_blocking(move || store.resolve_identity(&registry, &uuid)).await;

    match resolved {
        Ok(Ok(Some(entity))) => Json(json!({
            "id": entity.id.to_string(),
            "scheme": crate::core::identity::ENTITY_ID_SCHEME,
            "provenance": entity.origin,
            "motive": entity.motive,
            "label": entity.label,
            "entity_type": entity.entity_type,
            "name": entity.name,
            "key": entity.key,
            // Les octets hachés, publiés : un tiers recalcule et vérifie.
            "canonical_bytes_hex": entity.key.canonical_hex(),
            "data": entity.data,
        }))
        .into_response(),
        Ok(Ok(None)) => envelope(
            StatusCode::NOT_FOUND,
            json!({
                "error": "identité inconnue de l'index",
                "hint": "l'index est en mémoire et se reconstruit à la demande : appeler                          GET /graph/identity?counts=true (borné) ou l'outil adopt_obrain_base                          pour un balayage complet hors requête"
            }),
        ),
        Ok(Err(e)) => envelope(
            StatusCode::UNPROCESSABLE_ENTITY,
            json!({ "error": e.to_string() }),
        ),
        Err(e) => join_error(e),
    }
}

/// Les limites connues de l'identité, chacune adossée à une preuve.
fn identity_traps() -> Vec<Trap> {
    vec![
        Trap {
            id: "derived-ids-are-permanent".to_string(),
            statement: "Un identifiant dérivé est une clé PERMANENTE : il est fonction pure de                         (espace, label, nom de propriété, valeur). Renommer un `qualified_name`                         change l'identité — et donc les liens, les références et les empreintes                         de journal qui pointaient dessus."
                .to_string(),
            evidence: "this/src/core/identity.rs — ENTITY_ID_SCHEME, vecteurs d'or gelés".to_string(),
        },
        Trap {
            id: "index-is-in-memory-and-scoped".to_string(),
            statement: "L'index d'identité vit en mémoire, repart de zéro à chaque ouverture et                         est purgé à chaque bascule d'espace."
                .to_string(),
            evidence: "this/src/storage/obrain.rs — invalidate_caches, IdentityIndex".to_string(),
        },
        Trap {
            id: "labels-introspection-is-ambiguous".to_string(),
            statement: "`all_labels()` ne distingue pas « store vide » de « backend non                         introspectable » : une liste vide n'est jamais « aucune entité »."
                .to_string(),
            evidence: "obrain-core/src/graph/traits.rs:552 (défaut `Vec::new()`) ;                        obrain-substrate/src/store.rs (override)"
                .to_string(),
        },
        Trap {
            id: "content-digest-is-not-a-primary-key".to_string(),
            statement: "Le repli par digest de contenu est une clé de DÉDUPLICATION à la lecture,                         pas une clé primaire : il est désactivé par défaut, et sa liste de                         propriétés volatiles exclues est publiée par les motifs."
                .to_string(),
            evidence: "this/src/core/identity.rs — ContentDigestProjection, DEFAULT_VOLATILE"
                .to_string(),
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use tower::ServiceExt;

    fn app() -> Router {
        graph_routes(GraphState {
            store: ObrainStore::new_in_memory(),
            identity: Arc::new(crate::core::identity::IdentityRegistry::obrain_default()),
            scope: None,
            canvas: None,
        })
    }

    async fn body_json(response: Response) -> serde_json::Value {
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("corps");
        serde_json::from_slice(&bytes).expect("json")
    }

    #[tokio::test]
    async fn get_graph_publishes_the_relief() {
        let response = app()
            .oneshot(
                Request::builder()
                    .uri("/graph")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let json = body_json(response).await;
        assert_eq!(json["schema"], crate::graph::relief::RELIEF_SCHEMA);
        assert!(
            json["procedures"].as_array().is_some_and(|p| !p.is_empty()),
            "le relief doit publier les procédures introspectées du registre"
        );
        assert!(
            json["families"].as_array().is_some_and(|f| !f.is_empty()),
            "les familles déclarées doivent être publiées"
        );
        assert!(json["traps"].as_array().is_some_and(|t| !t.is_empty()));
        // Ce store EST substrat-backed (T17 : une config mémoire ouvre un
        // substrat temporaire) — mais ses passes simpliciales ne sont pas
        // configurées, et le document doit le distinguer.
        assert_eq!(json["engine"]["substrate"], true);
        assert_eq!(json["engine"]["simplex"]["configured"], false);
    }

    #[tokio::test]
    async fn counts_are_opt_in_and_explained_when_impossible() {
        // Sans `?counts=true`, aucun compteur n'est calculé.
        let response = app()
            .oneshot(
                Request::builder()
                    .uri("/graph")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let json = body_json(response).await;
        assert!(json["engine"]["simplex"]["counts"].is_null());

        // Avec `?counts=true` sur un store en mémoire, le compteur reste absent
        // mais la RAISON est publiée (jamais un 0 ambigu).
        let response = app()
            .oneshot(
                Request::builder()
                    .uri("/graph?counts=true")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let json = body_json(response).await;
        assert!(json["engine"]["simplex"]["counts"].is_null());
        assert!(
            json["engine"]["simplex"]["counts_error"]
                .as_str()
                .is_some_and(|e| !e.is_empty()),
            "l'absence de compteur doit être expliquée"
        );
    }

    #[tokio::test]
    async fn procedures_route_separates_introspected_from_declared() {
        let response = app()
            .oneshot(
                Request::builder()
                    .uri("/graph/procedures")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let json = body_json(response).await;

        let procedures = json["procedures"].as_array().expect("procédures");
        assert!(procedures.iter().all(|p| p["origin"] == "introspected"));
        let families = json["families"].as_array().expect("familles");
        assert!(families.iter().all(|f| f["origin"] == "declared"));
    }

    #[tokio::test]
    async fn spaces_route_reports_the_default_graph() {
        let response = app()
            .oneshot(
                Request::builder()
                    .uri("/graph/spaces")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let json = body_json(response).await;
        assert!(json["current"].is_null(), "aucun espace actif par défaut");
        assert!(json["graphs"].as_array().is_some_and(|g| g.is_empty()));
    }

    // ── Auto-câblage par RestExposure ────────────────────────────────────
    //
    // Les tests ci-dessus vérifient le groupe de routes ; ceux-ci vérifient
    // qu'il est bien MONTÉ — et monté seulement quand il y a un store. C'est
    // la différence entre « la route existe » et « la route est servie ».

    #[tokio::test]
    async fn rest_exposure_auto_wires_graph_routes() {
        use crate::server::RestExposure;
        use crate::server::host::ServerHost;

        // **Couche d'autorisation désarmée, exprès** — sinon ce test ne prouve
        // plus ce qu'il annonce. Depuis que l'abstention est un refus
        // (2026-09-13), une route de graphe **sans `graph.policy` déclarée**
        // rend 403, et une route **absente** rend 403 pour la même raison : le
        // statut ne distingue plus « montée » de « non montée ». Le contrat de
        // refus est verrouillé juste en dessous
        // (`graph_reads_are_refused_when_no_policy_is_declared`) et au niveau de
        // la couche par `tests/abstention_refusee.rs`.
        let mut host =
            ServerHost::minimal_for_test().with_graph_store(ObrainStore::new_in_memory());
        host.authorize = false;
        let router = RestExposure::build_router(Arc::new(host), vec![]).expect("routeur REST");

        let response = router
            .oneshot(
                Request::builder()
                    .uri("/graph")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let json = body_json(response).await;
        assert_eq!(json["schema"], crate::graph::relief::RELIEF_SCHEMA);
    }

    /// Le contrat de refus, **à l'endroit où la route est montée**.
    ///
    /// Un store est attaché, les routes existent — mais aucune `graph.policy`
    /// n'est déclarée : la couche s'abstient, et une abstention est un refus.
    /// Un appelant identifié n'y change rien (ce n'est pas une affaire
    /// d'identité : la route n'a pas de garde). Le remède est de **déclarer**
    /// la politique (`security.graph.policy`), jamais d'espérer un passage
    /// silencieux — c'est exactement ce que le plan de déploiement réclame.
    #[tokio::test]
    async fn graph_reads_are_refused_when_no_policy_is_declared() {
        use crate::server::RestExposure;
        use crate::server::host::ServerHost;

        let host =
            Arc::new(ServerHost::minimal_for_test().with_graph_store(ObrainStore::new_in_memory()));
        assert!(host.authorize, "l'autorisation est armée par défaut");
        let router = RestExposure::build_router(host, vec![]).expect("routeur REST");

        for uri in ["/graph", "/graph/identity", "/graph/spaces"] {
            let response = router
                .clone()
                .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
                .await
                .unwrap();
            assert_eq!(
                response.status(),
                StatusCode::FORBIDDEN,
                "`{uri}` sans politique déclarée : l'abstention refuse"
            );
            let json = body_json(response).await;
            assert_eq!(
                json["verdict"], "deny_abstain",
                "le refus se nomme, il ne se devine pas : {json}"
            );
            assert!(
                json["reason"]
                    .as_str()
                    .is_some_and(|r| r.contains("graph.policy")),
                "et il NOMME le remède : un motif qui parle d'« entité enregistrée » \
                 enverrait chercher au mauvais endroit : {json}"
            );
        }
    }

    #[tokio::test]
    async fn graph_routes_are_absent_without_a_store() {
        use crate::server::RestExposure;
        use crate::server::host::ServerHost;

        let host = Arc::new(ServerHost::minimal_for_test());
        let router = RestExposure::build_router(host, vec![]).expect("routeur REST");

        let response = router
            .oneshot(
                Request::builder()
                    .uri("/graph")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        // Le routeur REST répond 400 sur un chemin inconnu (son handler de
        // liens attend un paramètre de chemin) — peu importe le code exact :
        // ce qui compte est qu'AUCUN relief ne soit servi.
        assert_ne!(
            response.status(),
            StatusCode::OK,
            "sans handle obrain, pas de relief partiel"
        );
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("corps");
        assert!(
            !String::from_utf8_lossy(&body).contains(crate::graph::relief::RELIEF_SCHEMA),
            "aucune route /graph ne doit exister sans store attaché"
        );
    }

    // ── Identité canonique ───────────────────────────────────────────────

    /// Un store avec **un nœud à propriétés plates** (comme une base écrite par
    /// un autre outil), indexé par identité dérivée.
    fn app_with_flat_node() -> (Router, String) {
        let store = ObrainStore::new_in_memory();
        {
            let session = store.session();
            session.create_node_with_props(
                &["Page"],
                [(
                    "qualified_name",
                    obrain_common::types::Value::from("entity:mp_hodge"),
                )],
            );
        }
        let registry = crate::core::identity::IdentityRegistry::obrain_default();
        let report = store.rebuild_identity_index(&registry);
        assert_eq!(report.total_resolved, 1, "le nœud plat doit être résolu");

        // L'identifiant se recalcule depuis la clé canonique — c'est justement
        // ce qu'un tiers peut refaire.
        let key = crate::core::identity::EntityKey::new(
            None,
            "Page",
            crate::core::identity::Qualifier::Natural {
                property: "qualified_name".into(),
                value: "entity:mp_hodge".into(),
            },
        )
        .expect("clé");
        let id = key.uuid().to_string();

        let router = graph_routes(GraphState {
            store,
            identity: Arc::new(registry),
            scope: None,
            canvas: None,
        });
        (router, id)
    }

    #[tokio::test]
    async fn identity_document_declares_the_chain_without_counting() {
        let response = app()
            .oneshot(
                Request::builder()
                    .uri("/graph/identity")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let json = body_json(response).await;
        assert_eq!(json["schema"], "this-rs/graph-identity/1");
        assert_eq!(
            json["scheme"],
            crate::core::identity::ENTITY_ID_SCHEME,
            "le schéma d'identité doit être publié tel quel"
        );
        assert_eq!(json["motives"][0]["motive"], "convention:_uuid/_data");
        assert_eq!(json["motives"][0]["rank"], 0);
        assert!(
            json["counts"].is_null(),
            "sans ?counts=true, aucun comptage — et surtout pas un zéro"
        );
        assert_eq!(json["count_bounds"]["full_scan_served"], false);
        assert!(json["traps"].as_array().is_some_and(|t| !t.is_empty()));
        // Le store est vide : l'ambiguïté doit être publiée, pas un zéro muet.
        assert!(
            json["labels_error"]
                .as_str()
                .is_some_and(|e| e.contains("INDISCERNABLES")),
            "store vide : l'ambiguïté doit être dite"
        );
    }

    #[tokio::test]
    async fn identity_counts_are_bounded_and_opt_in() {
        let (router, _) = app_with_flat_node();
        let response = router
            .oneshot(
                Request::builder()
                    .uri("/graph/identity?counts=true")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let json = body_json(response).await;
        let counts = &json["counts"];
        assert_eq!(counts["total_resolved"], 1);
        assert_eq!(counts["labels"][0]["by_derivation"], 1);
        assert_eq!(counts["labels"][0]["by_convention"], 0);
        assert_eq!(counts["truncated"], false);
        assert_eq!(
            json["count_bounds"]["max_per_label"],
            IDENTITY_COUNT_MAX_PER_LABEL
        );
        assert_eq!(json["index_len"], 1);
    }

    /// La pièce maîtresse : la réponse porte de quoi **recalculer** l'identifiant.
    #[tokio::test]
    async fn identity_lookup_publishes_the_bytes_a_third_party_can_recheck() {
        let (router, id) = app_with_flat_node();
        let response = router
            .oneshot(
                Request::builder()
                    .uri(format!("/graph/identity/{id}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let json = body_json(response).await;
        assert_eq!(json["id"], id);
        assert_eq!(json["provenance"], "derived");
        assert_eq!(json["motive"], "derived:flat-property");
        assert_eq!(json["label"], "Page");
        assert_eq!(json["key"]["qualifier"]["kind"], "natural");
        assert_eq!(json["key"]["qualifier"]["property"], "qualified_name");

        // Vérification indépendante : on recalcule l'id depuis les octets publiés.
        let hex = json["canonical_bytes_hex"]
            .as_str()
            .expect("octets publiés");
        assert!(!hex.is_empty() && hex.len() % 2 == 0);
        let key = crate::core::identity::EntityKey::new(
            None,
            "Page",
            crate::core::identity::Qualifier::Natural {
                property: "qualified_name".into(),
                value: "entity:mp_hodge".into(),
            },
        )
        .expect("clé");
        assert_eq!(
            key.canonical_hex(),
            hex,
            "les octets publiés doivent être EXACTEMENT ceux qui sont hachés"
        );
        assert_eq!(key.uuid().to_string(), id);
    }

    #[tokio::test]
    async fn identity_lookup_explains_a_miss_and_refuses_garbage() {
        let (router, _) = app_with_flat_node();

        // Inconnue de l'index : 404 AVEC la marche à suivre.
        let unknown = Uuid::new_v4().to_string();
        let response = router
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/graph/identity/{unknown}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        let json = body_json(response).await;
        assert!(
            json["hint"]
                .as_str()
                .is_some_and(|h| h.contains("counts=true")),
            "un 404 doit dire comment peupler l'index"
        );

        // Identifiant illisible : 400, pas 404 (ce n'est pas la même chose).
        let response = router
            .oneshot(
                Request::builder()
                    .uri("/graph/identity/pas-un-uuid")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn rest_exposure_auto_wires_the_identity_routes() {
        use crate::server::RestExposure;
        use crate::server::host::ServerHost;

        let host =
            Arc::new(ServerHost::minimal_for_test().with_graph_store(ObrainStore::new_in_memory()));
        let router = RestExposure::build_router(host, vec![]).expect("routeur REST");

        let response = router
            .oneshot(
                Request::builder()
                    .uri("/graph/identity")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            response.status(),
            StatusCode::FORBIDDEN,
            "sans `graph.policy`, la couche s'abstient et l'abstention refuse : \
             voir `graph_reads_are_refused_when_no_policy_is_declared`"
        );
        let json = body_json(response).await;
        assert_eq!(json["verdict"], "deny_abstain");
    }

    /// **La route est servie dès qu'on déclare la politique** — le remède est
    /// réel, pas théorique : même hôte, même store, une politique de lecture de
    /// graphe déclarée, et le document sort.
    #[tokio::test]
    async fn declaring_a_graph_policy_serves_the_identity_contract() {
        use crate::core::auth::AuthContext;
        use crate::server::RestExposure;
        use crate::server::host::ServerHost;

        let mut host =
            ServerHost::minimal_for_test().with_graph_store(ObrainStore::new_in_memory());
        // `authorize` reste armé : c'est bien la DÉCLARATION qui ouvre la route.
        let mut security = (*host.security).clone();
        security.graph.policy = Some("authenticated".to_string());
        host.security = Arc::new(security);

        let router = RestExposure::build_router(Arc::new(host), vec![]).expect("routeur REST");
        let router = router.layer(axum::Extension(AuthContext::User {
            user_id: uuid::Uuid::from_u128(1),
            tenant_id: uuid::Uuid::from_u128(9),
            roles: vec![],
        }));

        let response = router
            .oneshot(
                Request::builder()
                    .uri("/graph/identity")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            response.status(),
            StatusCode::OK,
            "politique déclarée + appelant identifié : la route répond"
        );
        let json = body_json(response).await;
        assert_eq!(json["schema"], "this-rs/graph-identity/1");
    }
}
