//! **Application** des politiques d'autorisation — le point qui manquait.
//!
//! # Le constat qui justifie ce module
//!
//! this-rs *déclarait* des politiques sans jamais les appliquer :
//! - `EntityAuthConfig` (`list`/`get`/`create`/`update`/`delete`/`list_links`/
//!   `create_link`/`delete_link`) n'était lu **nulle part** hors de la
//!   configuration ;
//! - `AppState::get_link_auth_policy` calculait bien une politique par
//!   opération de lien, mais n'avait **aucun appelant de production** ;
//! - le moteur de politiques, lui, était **complet** : `AuthPolicy` (`Public`,
//!   `Authenticated`, `Owner`, `HasRole`, `ServiceOnly`, `AdminOnly`, `And`,
//!   `Or`, `Not`, `Custom`, `Resolver`), `AuthPolicy::parse_policy`,
//!   `AuthPolicy::check_with_resolver` — et déjà **fail-closed** sur un
//!   résolveur inconnu.
//!
//! Il ne manquait donc que la **résolution** (route → opération → politique) et
//! le **refus**. C'est ce module, et rien d'autre.
//!
//! # Deux décisions qui n'iront pas de soi
//!
//! **1. L'application est EXPLICITE, jamais par défaut.** La politique par
//! défaut d'une entité est `"authenticated"`, et elle n'a jamais été appliquée :
//! l'appliquer d'office **casserait** tout déploiement existant qui comptait sur
//! l'accès libre. C'est un correctif de sécurité, donc il s'arme sciemment
//! ([`ServerBuilder::with_authorization`](crate::server::builder::ServerBuilder::with_authorization)),
//! il ne s'impose pas.
//!
//! **2. Fail-closed, et jamais d'invention.**
//! - politique déclarée **illisible** → **refus** (jamais « on laisse passer ») ;
//! - entité **enregistrée** mais absente de la configuration → la politique par
//!   défaut du framework est appliquée, et la décision le **dit**
//!   (`origin: default`) au lieu de faire croire à une déclaration ;
//! - route **inconnue** (route personnalisée, `/health`, `/graph`, `/auth/*`)
//!   → **abstention** : la couche d'authentification globale reste seule
//!   juge, le framework ne fabrique pas de politique pour ce qu'il ne connaît pas.

use std::sync::Arc;

use axum::extract::{Request, State};
use axum::http::{Method, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use serde::Serialize;
use serde_json::json;

use crate::config::{EntityAuthConfig, LinksConfig};
use crate::core::auth::{AuthContext, AuthPolicy, AuthResolverRegistry};
use crate::server::entity_registry::EntityRegistry;

/// D'où vient la politique appliquée.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PolicyOrigin {
    /// Déclarée dans la configuration de l'entité.
    Declared,
    /// Politique par défaut du framework (entité sans entrée de configuration).
    Default,
    /// Politique évaluée par la **décision IAM matérialisée** (`perm:…`) —
    /// donc par une lecture indexée, sans traversée du graphe des rôles.
    Iam,
}

/// Ce qu'une route exige.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Requirement {
    /// Une politique s'applique : c'est l'opération et l'entité concernées.
    Policy {
        /// Type d'entité (ou `link` pour une route de lien).
        entity_type: String,
        /// Opération (`list`, `get`, `create`, `update`, `delete`, `list_links`,
        /// `create_link`, `delete_link`).
        operation: String,
        /// La politique telle qu'écrite dans la configuration.
        policy: String,
        /// Sa provenance.
        origin: PolicyOrigin,
    },
    /// Aucune politique : route non reconnue comme une route d'entité.
    Abstain {
        /// Pourquoi (publié : une abstention doit être explicable).
        reason: String,
    },
}

/// Le verdict.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "verdict", rename_all = "snake_case")]
pub enum Decision {
    /// Autorise, et dit sous quelle politique.
    Allow {
        /// Portée (entité · opération).
        scope: String,
        /// Politique appliquée.
        policy: String,
        /// Provenance.
        origin: PolicyOrigin,
    },
    /// Refuse, et dit pourquoi.
    Deny {
        /// Portée (entité · opération).
        scope: String,
        /// Politique appliquée.
        policy: String,
        /// Provenance.
        origin: PolicyOrigin,
        /// Raison lisible (le contexte ne satisfaisait pas la politique, ou elle
        /// était illisible).
        reason: String,
    },
    /// Le framework ne se prononce pas.
    Abstain {
        /// Raison.
        reason: String,
    },
}

impl Decision {
    /// La décision autorise-t-elle ?
    pub fn is_allow(&self) -> bool {
        matches!(self, Decision::Allow { .. })
    }
}

/// Le point d'application des politiques.
///
/// Construit à partir de ce que le serveur sait déjà : la configuration fusionnée
/// (qui porte les politiques), le registre d'entités (qui porte les **routes**
/// réellement montées) et les résolveurs nommés.
/// Le préfixe d'une politique adossée à l'IAM matérialisé.
pub const PERM_PREFIX: &str = "perm:";

#[derive(Clone)]
pub struct Authorization {
    config: Arc<LinksConfig>,
    entities: Arc<Vec<(String, String)>>,
    resolvers: Arc<AuthResolverRegistry>,
    /// Le plan IAM, quand un store obrain est disponible.
    ///
    /// C'est ce qui permet à une politique `perm:<permission>` d'être tranchée
    /// par la **décision matérialisée** (une lecture indexée) au lieu d'être
    /// recalculée. Sans plan, `perm:` **refuse** — jamais d'approbation par
    /// défaut.
    #[cfg(feature = "obrain")]
    iam: Option<crate::storage::iam::Iam>,
}

impl Authorization {
    /// Construit le point d'application.
    pub fn new(
        config: Arc<LinksConfig>,
        registry: &EntityRegistry,
        resolvers: Arc<AuthResolverRegistry>,
    ) -> Self {
        Self {
            config,
            entities: Arc::new(registry.entity_plurals()),
            resolvers,
            #[cfg(feature = "obrain")]
            iam: None,
        }
    }

    /// Branche le **plan IAM** : les politiques `perm:<permission>` seront
    /// tranchées par la décision matérialisée du principal.
    #[cfg(feature = "obrain")]
    pub fn with_iam(mut self, store: &Arc<crate::storage::ObrainStore>) -> Self {
        self.iam = Some(store.iam());
        self
    }

    /// `(principal, portée)` d'un contexte, pour l'IAM.
    ///
    /// **L'IAM décide, pas la variante de contexte** : un administrateur sans
    /// décision matérialisée est refusé comme les autres. Un contexte anonyme
    /// n'a pas de principal — donc pas de décision, donc refus.
    #[cfg(feature = "obrain")]
    fn iam_identity(context: &AuthContext) -> Option<(String, String)> {
        match context {
            AuthContext::User {
                user_id, tenant_id, ..
            }
            | AuthContext::Owner {
                user_id, tenant_id, ..
            } => Some((user_id.to_string(), tenant_id.to_string())),
            AuthContext::Service {
                service_name,
                tenant_id,
            } => Some((
                service_name.clone(),
                tenant_id
                    .map(|t| t.to_string())
                    .unwrap_or_else(|| "global".to_string()),
            )),
            AuthContext::Admin { admin_id } => Some((admin_id.to_string(), "global".to_string())),
            AuthContext::Anonymous => None,
        }
    }

    /// **L'évaluation d'une politique**, avec `perm:` en plus du vocabulaire
    /// existant.
    ///
    /// Les préfixes de composition (`all:`, `any:`, `not:`) sont traités **de la
    /// même façon que [`AuthPolicy::parse_policy`]** : `all:` = ET, `any:` = OU,
    /// `not:` = négation. Le test
    /// `the_pre_pass_agrees_with_the_framework_parser` vérifie cette égalité
    /// pour tout le vocabulaire qui n'utilise pas `perm:` — le pré-passage ne
    /// doit **rien** changer aux politiques existantes.
    fn evaluate(&self, policy: &str, context: &AuthContext, used_iam: &mut bool) -> bool {
        let policy = policy.trim();

        for (prefix, conjunction) in [("all:", true), ("any:", false)] {
            if let Some(rest) = policy.strip_prefix(prefix) {
                let parts: Vec<&str> = rest.split(',').map(str::trim).collect();
                return if conjunction {
                    parts.iter().all(|p| self.evaluate(p, context, used_iam))
                } else {
                    parts.iter().any(|p| self.evaluate(p, context, used_iam))
                };
            }
        }
        if let Some(inner) = policy.strip_prefix("not:") {
            return !self.evaluate(inner, context, used_iam);
        }

        if let Some(permission) = policy.strip_prefix(PERM_PREFIX) {
            *used_iam = true;
            return self.check_iam(permission.trim(), context);
        }

        AuthPolicy::parse_policy(policy).check_with_resolver(context, Some(&self.resolvers), None)
    }

    /// La décision **matérialisée** autorise-t-elle cette permission ?
    ///
    /// Une seule recherche indexée (`find_nodes_by_property` sur `acl_key`),
    /// aucune traversée. Fail-closed : sans plan IAM, sans principal, ou sans
    /// décision, c'est **non**.
    #[cfg(feature = "obrain")]
    fn check_iam(&self, permission: &str, context: &AuthContext) -> bool {
        let Some(iam) = &self.iam else {
            tracing::warn!(
                permission,
                "politique `perm:` sans plan IAM branché — refus (fail-closed)"
            );
            return false;
        };
        let Some((principal, scope)) = Self::iam_identity(context) else {
            tracing::debug!(permission, "`perm:` sans principal (anonyme) — refus");
            return false;
        };
        match iam.decision(&principal, &scope) {
            Ok(Some(stamp)) => {
                let allowed = stamp.allows(permission);
                tracing::debug!(
                    principal,
                    scope,
                    permission,
                    epoch = stamp.epoch,
                    allowed,
                    "décision IAM matérialisée consultée"
                );
                allowed
            }
            Ok(None) => {
                tracing::debug!(principal, scope, "aucune décision matérialisée — refus");
                false
            }
            Err(e) => {
                tracing::warn!(error = %e, "lecture de la décision IAM échouée — refus");
                false
            }
        }
    }

    /// Sans plan IAM, la compilation par défaut doit quand même refuser.
    #[cfg(not(feature = "obrain"))]
    fn check_iam(&self, _permission: &str, _context: &AuthContext) -> bool {
        tracing::warn!("politique `perm:` sans support IAM (feature `obrain`) — refus");
        false
    }

    /// La politique déclarée pour une entité et une opération.
    ///
    /// `None` = l'entité n'a **pas** d'entrée de configuration ; l'appelant
    /// décide alors d'appliquer la politique par défaut du framework — en le
    /// disant ([`PolicyOrigin::Default`]).
    fn declared_policy(&self, entity_type: &str, operation: &str) -> Option<String> {
        let entity = self
            .config
            .entities
            .iter()
            .find(|e| e.singular == entity_type)?;
        let auth: &EntityAuthConfig = &entity.auth;
        let policy = match operation {
            "list" => &auth.list,
            "get" => &auth.get,
            "create" => &auth.create,
            "update" => &auth.update,
            "delete" => &auth.delete,
            "list_links" => &auth.list_links,
            "create_link" => &auth.create_link,
            "delete_link" => &auth.delete_link,
            _ => return None,
        };
        Some(policy.clone())
    }

    /// Les segments utiles d'un chemin (`/pages/42` → `["pages", "42"]`).
    fn segments(path: &str) -> Vec<&str> {
        path.split('/').filter(|s| !s.is_empty()).collect()
    }

    /// **La résolution** : route → (entité, opération).
    ///
    /// Les formes reconnues sont celles que this-rs génère lui-même :
    ///
    /// | segments | méthode | opération |
    /// |---|---|---|
    /// | `[plural]` | GET | `list` |
    /// | `[plural]` | POST | `create` |
    /// | `[plural, id]` | GET | `get` |
    /// | `[plural, id]` | PUT/PATCH | `update` |
    /// | `[plural, id]` | DELETE | `delete` |
    /// | `[plural, id, route]` | GET | `list_links` |
    /// | `[plural, id, type, target, target_id]` | POST | `create_link` |
    /// | `[plural, id, type, target, target_id]` | DELETE | `delete_link` |
    ///
    /// **Limite assumée** : la forme à 3 segments est un *motif*. Une route
    /// personnalisée greffée sous une entité (`/pages/{id}/voisins`) est donc
    /// traitée comme `list_links`. C'est dit ici, et la décision porte la portée
    /// exacte (`Page · list_links`) pour qu'un opérateur voie ce qui a été
    /// appliqué au lieu de le deviner.
    pub fn requirement(&self, method: &Method, path: &str) -> Requirement {
        let segments = Self::segments(path);
        let Some(plural) = segments.first() else {
            return Requirement::Abstain {
                reason: "chemin vide".to_string(),
            };
        };

        let Some((entity_type, _)) = self.entities.iter().find(|(_, p)| p == plural) else {
            return Requirement::Abstain {
                reason: format!("`{plural}` n'est le pluriel d'aucune entité enregistrée"),
            };
        };

        let operation = match (method, segments.len()) {
            (&Method::GET, 1) => "list",
            (&Method::POST, 1) => "create",
            (&Method::GET, 2) => "get",
            (&Method::PUT | &Method::PATCH, 2) => "update",
            (&Method::DELETE, 2) => "delete",
            (&Method::GET, 3) => "list_links",
            (&Method::POST, 5) => "create_link",
            (&Method::DELETE, 5) => "delete_link",
            _ => {
                return Requirement::Abstain {
                    reason: format!(
                        "forme non reconnue pour `{plural}` : {} segments avec {}",
                        segments.len(),
                        method
                    ),
                };
            }
        };

        match self.declared_policy(entity_type, operation) {
            Some(policy) => Requirement::Policy {
                entity_type: entity_type.clone(),
                operation: operation.to_string(),
                policy,
                origin: PolicyOrigin::Declared,
            },
            None => {
                let default_auth = EntityAuthConfig::default();
                let policy = match operation {
                    "list" => default_auth.list,
                    "get" => default_auth.get,
                    "create" => default_auth.create,
                    "update" => default_auth.update,
                    "delete" => default_auth.delete,
                    "list_links" => default_auth.list_links,
                    "create_link" => default_auth.create_link,
                    _ => default_auth.delete_link,
                };
                Requirement::Policy {
                    entity_type: entity_type.clone(),
                    operation: operation.to_string(),
                    policy,
                    origin: PolicyOrigin::Default,
                }
            }
        }
    }

    /// **La décision.** Fail-closed : illisible ⇒ refus.
    pub fn decide(&self, method: &Method, path: &str, context: &AuthContext) -> Decision {
        let (entity_type, operation, policy, origin) = match self.requirement(method, path) {
            Requirement::Policy {
                entity_type,
                operation,
                policy,
                origin,
            } => (entity_type, operation, policy, origin),
            Requirement::Abstain { reason } => return Decision::Abstain { reason },
        };

        let scope = format!("{entity_type} · {operation}");
        // Fail-closed : une politique illisible ne doit JAMAIS laisser passer.
        // `parse_policy` retombe sur `Resolver("<texte>")`, que
        // `check_with_resolver` refuse déjà quand le résolveur est inconnu.
        let mut used_iam = false;
        let allowed = self.evaluate(&policy, context, &mut used_iam);
        // Une politique tranchée par l'IAM matérialisé le DIT : la provenance
        // d'une décision ne se devine pas.
        let origin = if used_iam { PolicyOrigin::Iam } else { origin };

        if allowed {
            Decision::Allow {
                scope,
                policy,
                origin,
            }
        } else {
            Decision::Deny {
                scope,
                policy,
                origin,
                reason: format!(
                    "le contexte ({}) ne satisfait pas la politique",
                    describe(context)
                ),
            }
        }
    }
}

/// Décrit un contexte sans divulguer d'identifiants inutiles.
fn describe(ctx: &AuthContext) -> &'static str {
    match ctx {
        AuthContext::User { .. } => "utilisateur authentifié",
        AuthContext::Owner { .. } => "propriétaire de la ressource",
        AuthContext::Service { .. } => "service",
        AuthContext::Admin { .. } => "administrateur",
        AuthContext::Anonymous => "anonyme",
    }
}

/// Middleware d'autorisation.
///
/// Doit être monté **après** le middleware d'authentification, dont il lit
/// l'`AuthContext` dans les extensions. Un contexte absent vaut `Anonymous`
/// (fail-closed) — et c'est journalisé, parce que c'est presque toujours le
/// signe que la couche d'authentification n'a pas été montée.
pub async fn authorization_middleware(
    State(authz): State<Authorization>,
    request: Request,
    next: Next,
) -> Response {
    let context = request
        .extensions()
        .get::<AuthContext>()
        .cloned()
        .unwrap_or_else(|| {
            tracing::warn!(
                path = %request.uri().path(),
                "autorisation armée sans contexte d'authentification : traité comme anonyme"
            );
            AuthContext::Anonymous
        });

    let method = request.method().clone();
    let path = request.uri().path().to_string();
    let decision = authz.decide(&method, &path, &context);

    match decision {
        Decision::Allow { .. } | Decision::Abstain { .. } => next.run(request).await,
        Decision::Deny {
            scope,
            policy,
            origin,
            reason,
        } => {
            tracing::debug!(%method, %path, %scope, %policy, "autorisation refusée");
            (
                StatusCode::FORBIDDEN,
                axum::Json(json!({
                    "error": "forbidden",
                    "scope": scope,
                    "policy": policy,
                    "origin": origin,
                    "reason": reason,
                })),
            )
                .into_response()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::EntityConfig;
    use crate::core::link::LinkDefinition;
    use crate::server::entity_registry::EntityDescriptor;

    struct Descriptor {
        entity_type: &'static str,
        plural: &'static str,
    }

    impl EntityDescriptor for Descriptor {
        fn entity_type(&self) -> &str {
            self.entity_type
        }
        fn plural(&self) -> &str {
            self.plural
        }
        fn build_routes(&self) -> axum::Router {
            axum::Router::new()
        }
    }

    fn registry(plurals: &[(&'static str, &'static str)]) -> EntityRegistry {
        let mut r = EntityRegistry::new();
        for (t, p) in plurals {
            r.register(Box::new(Descriptor {
                entity_type: t,
                plural: p,
            }));
        }
        r
    }

    fn authz(
        entities: Vec<EntityConfig>,
        plurals: &[(&'static str, &'static str)],
    ) -> Authorization {
        let config = LinksConfig {
            entities,
            links: Vec::<LinkDefinition>::new(),
            validation_rules: None,
            events: None,
            sinks: None,
            auth: None,
        };
        Authorization::new(
            Arc::new(config),
            &registry(plurals),
            Arc::new(AuthResolverRegistry::new()),
        )
    }

    fn entity(singular: &str, plural: &str, auth: EntityAuthConfig) -> EntityConfig {
        EntityConfig {
            singular: singular.to_string(),
            plural: plural.to_string(),
            auth,
        }
    }

    fn admin_only_delete() -> EntityAuthConfig {
        let mut a = EntityAuthConfig::default();
        a.delete = "admin".to_string();
        a.list = "public".to_string();
        a
    }

    // ── Résolution ───────────────────────────────────────────────────────

    #[test]
    fn route_shapes_resolve_to_the_declared_operation() {
        let a = authz(
            vec![entity("order", "orders", admin_only_delete())],
            &[("order", "orders")],
        );
        let cases = [
            (Method::GET, "/orders", "list"),
            (Method::POST, "/orders", "create"),
            (Method::GET, "/orders/42", "get"),
            (Method::PUT, "/orders/42", "update"),
            (Method::PATCH, "/orders/42", "update"),
            (Method::DELETE, "/orders/42", "delete"),
            (Method::GET, "/orders/42/invoices", "list_links"),
            (Method::POST, "/orders/42/items/product/7", "create_link"),
            (Method::DELETE, "/orders/42/items/product/7", "delete_link"),
        ];
        for (method, path, expected) in cases {
            match a.requirement(&method, path) {
                Requirement::Policy {
                    operation, origin, ..
                } => {
                    assert_eq!(operation, expected, "{method} {path}");
                    assert_eq!(origin, PolicyOrigin::Declared);
                }
                other => panic!("{method} {path} : attendu une politique, obtenu {other:?}"),
            }
        }
    }

    #[test]
    fn unrecognised_routes_abstain_instead_of_inventing_a_policy() {
        let a = authz(
            vec![entity("order", "orders", admin_only_delete())],
            &[("order", "orders")],
        );
        // Route d'un autre domaine : le framework ne fabrique rien.
        assert!(matches!(
            a.requirement(&Method::GET, "/health"),
            Requirement::Abstain { .. }
        ));
        assert!(matches!(
            a.requirement(&Method::GET, "/graph/identity"),
            Requirement::Abstain { .. }
        ));
        // Un pluriel non enregistré n'est pas une entité.
        assert!(matches!(
            a.requirement(&Method::GET, "/inconnus"),
            Requirement::Abstain { .. }
        ));
        // Forme non reconnue sous une entité connue.
        assert!(matches!(
            a.requirement(&Method::PUT, "/orders"),
            Requirement::Abstain { .. }
        ));
        assert!(matches!(
            a.requirement(&Method::GET, "/orders/42/a/b/c/d"),
            Requirement::Abstain { .. }
        ));
    }

    #[test]
    fn a_registered_entity_without_config_gets_the_default_and_says_so() {
        let a = authz(Vec::new(), &[("order", "orders")]);
        match a.requirement(&Method::GET, "/orders") {
            Requirement::Policy { policy, origin, .. } => {
                assert_eq!(policy, "authenticated", "politique par défaut du framework");
                assert_eq!(
                    origin,
                    PolicyOrigin::Default,
                    "la décision doit DIRE qu'elle n'est pas déclarée"
                );
            }
            other => panic!("attendu une politique par défaut, obtenu {other:?}"),
        }
    }

    // ── Décision ─────────────────────────────────────────────────────────

    #[test]
    fn public_allows_anonymous_and_authenticated_denies_it() {
        let a = authz(
            vec![entity("order", "orders", admin_only_delete())],
            &[("order", "orders")],
        );

        // `list: public`
        assert!(
            a.decide(&Method::GET, "/orders", &AuthContext::Anonymous)
                .is_allow()
        );

        // `get` vaut `authenticated` — valeur par défaut, mais **déclarée** :
        // la distinction `Declared`/`Default` porte sur la présence d'une entrée
        // de configuration pour l'entité, pas sur la valeur de la politique.
        match a.decide(&Method::GET, "/orders/42", &AuthContext::Anonymous) {
            Decision::Deny {
                scope,
                policy,
                origin,
                ..
            } => {
                assert_eq!(scope, "order · get");
                assert_eq!(policy, "authenticated");
                assert_eq!(origin, PolicyOrigin::Declared);
            }
            other => panic!("attendu un refus, obtenu {other:?}"),
        }
        // …et accepté pour un utilisateur authentifié.
        let user = AuthContext::User {
            user_id: uuid::Uuid::new_v4(),
            tenant_id: uuid::Uuid::new_v4(),
            roles: vec![],
        };
        assert!(a.decide(&Method::GET, "/orders/42", &user).is_allow());
    }

    #[test]
    fn role_policies_use_the_roles_of_the_context() {
        let mut auth = EntityAuthConfig::default();
        auth.delete = "role:admin".to_string();
        let a = authz(
            vec![entity("order", "orders", auth)],
            &[("order", "orders")],
        );

        let plain = AuthContext::User {
            user_id: uuid::Uuid::new_v4(),
            tenant_id: uuid::Uuid::new_v4(),
            roles: vec!["reader".into()],
        };
        let admin = AuthContext::User {
            user_id: uuid::Uuid::new_v4(),
            tenant_id: uuid::Uuid::new_v4(),
            roles: vec!["admin".into()],
        };
        assert!(!a.decide(&Method::DELETE, "/orders/1", &plain).is_allow());
        assert!(a.decide(&Method::DELETE, "/orders/1", &admin).is_allow());
    }

    #[test]
    fn admin_only_policy_is_satisfied_by_an_admin_context() {
        let a = authz(
            vec![entity("order", "orders", admin_only_delete())],
            &[("order", "orders")],
        );
        let admin = AuthContext::Admin {
            admin_id: uuid::Uuid::new_v4(),
        };
        assert!(a.decide(&Method::DELETE, "/orders/1", &admin).is_allow());
        assert!(
            !a.decide(&Method::DELETE, "/orders/1", &AuthContext::Anonymous)
                .is_allow()
        );
    }

    /// **La propriété de sécurité qui compte** : une politique illisible refuse.
    #[test]
    fn an_unreadable_policy_denies_instead_of_allowing() {
        let mut auth = EntityAuthConfig::default();
        // Ni un mot-clé connu, ni un résolveur enregistré.
        auth.create = "resolver:inexistant".to_string();
        let a = authz(
            vec![entity("order", "orders", auth)],
            &[("order", "orders")],
        );

        let user = AuthContext::User {
            user_id: uuid::Uuid::new_v4(),
            tenant_id: uuid::Uuid::new_v4(),
            roles: vec!["admin".into()],
        };
        let decision = a.decide(&Method::POST, "/orders", &user);
        assert!(
            !decision.is_allow(),
            "un résolveur inconnu ne doit JAMAIS laisser passer : {decision:?}"
        );
    }

    #[test]
    fn owner_policy_needs_an_owner_context() {
        let mut auth = EntityAuthConfig::default();
        auth.update = "owner".to_string();
        let a = authz(
            vec![entity("order", "orders", auth)],
            &[("order", "orders")],
        );

        let user = AuthContext::User {
            user_id: uuid::Uuid::new_v4(),
            tenant_id: uuid::Uuid::new_v4(),
            roles: vec![],
        };
        assert!(!a.decide(&Method::PUT, "/orders/1", &user).is_allow());

        let owner = AuthContext::Owner {
            user_id: uuid::Uuid::new_v4(),
            tenant_id: uuid::Uuid::new_v4(),
            resource_id: uuid::Uuid::new_v4(),
            resource_type: "order".into(),
        };
        assert!(a.decide(&Method::PUT, "/orders/1", &owner).is_allow());
    }

    // ── Middleware, de bout en bout ──────────────────────────────────────

    async fn call(app: axum::Router, method: Method, uri: &str) -> StatusCode {
        use tower::ServiceExt;
        let response = app
            .oneshot(
                axum::http::Request::builder()
                    .method(method)
                    .uri(uri)
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .expect("réponse");
        response.status()
    }

    // ── `perm:` : la décision IAM matérialisée ───────────────────────────

    #[cfg(feature = "obrain")]
    mod iam_backed {
        use super::*;
        use crate::storage::ObrainStore;
        use crate::storage::iam::RoleGrant;

        fn ctx(tenant: uuid::Uuid, user: uuid::Uuid) -> AuthContext {
            AuthContext::User {
                user_id: user,
                tenant_id: tenant,
                roles: vec![],
            }
        }

        fn authz_with_iam(store: &std::sync::Arc<ObrainStore>) -> Authorization {
            let mut auth = EntityAuthConfig::default();
            auth.delete = "perm:orders:write".to_string();
            auth.list = "any:perm:orders:read,role:auditor".to_string();
            let config = LinksConfig {
                entities: vec![entity("order", "orders", auth)],
                links: Vec::<LinkDefinition>::new(),
                validation_rules: None,
                events: None,
                sinks: None,
                auth: None,
            };
            Authorization::new(
                Arc::new(config),
                &registry(&[("order", "orders")]),
                Arc::new(AuthResolverRegistry::new()),
            )
            .with_iam(store)
        }

        fn materialize(
            store: &std::sync::Arc<ObrainStore>,
            tenant: uuid::Uuid,
            user: uuid::Uuid,
            perms: Vec<&str>,
        ) {
            let iam = store.iam();
            let grants = vec![RoleGrant {
                role: "r".into(),
                permissions: perms.into_iter().map(str::to_string).collect(),
            }];
            iam.materialize(&user.to_string(), &tenant.to_string(), &grants)
                .expect("matérialisation");
        }

        /// **La propriété centrale** : la politique est tranchée par la décision
        /// matérialisée, et la décision le DIT (`origin: iam`).
        #[test]
        fn a_perm_policy_is_answered_by_the_materialised_decision() {
            let store = ObrainStore::new_in_memory();
            let tenant = uuid::Uuid::new_v4();
            let user = uuid::Uuid::new_v4();
            materialize(&store, tenant, user, vec!["orders:read"]);
            let authz = authz_with_iam(&store);

            // `delete: perm:orders:write` — l'utilisateur n'a que `orders:read`.
            match authz.decide(&Method::DELETE, "/orders/1", &ctx(tenant, user)) {
                Decision::Deny {
                    scope,
                    policy,
                    origin,
                    ..
                } => {
                    assert_eq!(scope, "order · delete");
                    assert_eq!(policy, "perm:orders:write");
                    assert_eq!(
                        origin,
                        PolicyOrigin::Iam,
                        "la provenance doit dire que l'IAM a tranché"
                    );
                }
                other => panic!("attendu un refus, obtenu {other:?}"),
            }

            // On lui accorde la permission : la même requête passe.
            materialize(&store, tenant, user, vec!["orders:read", "orders:write"]);
            match authz.decide(&Method::DELETE, "/orders/1", &ctx(tenant, user)) {
                Decision::Allow { origin, .. } => assert_eq!(origin, PolicyOrigin::Iam),
                other => panic!("attendu une autorisation, obtenu {other:?}"),
            }
        }

        /// La portée : la décision d'un locataire ne vaut pas pour un autre.
        #[test]
        fn the_materialised_decision_is_scoped_to_the_tenant() {
            let store = ObrainStore::new_in_memory();
            let tenant_a = uuid::Uuid::new_v4();
            let tenant_b = uuid::Uuid::new_v4();
            let user = uuid::Uuid::new_v4();
            materialize(&store, tenant_a, user, vec!["orders:write"]);
            let authz = authz_with_iam(&store);

            assert!(
                authz
                    .decide(&Method::DELETE, "/orders/1", &ctx(tenant_a, user))
                    .is_allow()
            );
            assert!(
                !authz
                    .decide(&Method::DELETE, "/orders/1", &ctx(tenant_b, user))
                    .is_allow(),
                "aucune décision pour le locataire B : refus"
            );
        }

        #[test]
        fn a_wildcard_decision_grants_every_permission_policy() {
            let store = ObrainStore::new_in_memory();
            let tenant = uuid::Uuid::new_v4();
            let user = uuid::Uuid::new_v4();
            materialize(&store, tenant, user, vec!["*"]);
            let authz = authz_with_iam(&store);
            assert!(
                authz
                    .decide(&Method::DELETE, "/orders/1", &ctx(tenant, user))
                    .is_allow()
            );
        }

        /// `any:perm:…,role:…` — le pré-passage compose, et l'origine reste
        /// `iam` dès que l'IAM a été consulté.
        #[test]
        fn perm_composes_with_the_existing_vocabulary() {
            let store = ObrainStore::new_in_memory();
            let tenant = uuid::Uuid::new_v4();
            let user = uuid::Uuid::new_v4();
            // Aucune décision matérialisée : seul le rôle peut sauver.
            let authz = authz_with_iam(&store);

            let plain = ctx(tenant, user);
            assert!(!authz.decide(&Method::GET, "/orders", &plain).is_allow());

            let auditor = AuthContext::User {
                user_id: user,
                tenant_id: tenant,
                roles: vec!["auditor".into()],
            };
            match authz.decide(&Method::GET, "/orders", &auditor) {
                Decision::Allow { origin, .. } => assert_eq!(
                    origin,
                    PolicyOrigin::Iam,
                    "l'IAM a été consulté (et a refusé) avant que le rôle ne sauve"
                ),
                other => panic!("le rôle `auditor` doit sauver : {other:?}"),
            }
        }

        /// Fail-closed : sans plan IAM, `perm:` refuse — même pour un admin.
        #[test]
        fn perm_without_an_iam_plan_denies() {
            let mut auth = EntityAuthConfig::default();
            auth.delete = "perm:orders:write".to_string();
            let config = LinksConfig {
                entities: vec![entity("order", "orders", auth)],
                links: Vec::<LinkDefinition>::new(),
                validation_rules: None,
                events: None,
                sinks: None,
                auth: None,
            };
            let authz = Authorization::new(
                Arc::new(config),
                &registry(&[("order", "orders")]),
                Arc::new(AuthResolverRegistry::new()),
            );
            let admin = AuthContext::Admin {
                admin_id: uuid::Uuid::new_v4(),
            };
            assert!(
                !authz
                    .decide(&Method::DELETE, "/orders/1", &admin)
                    .is_allow(),
                "sans IAM branché, `perm:` refuse — jamais d'approbation par défaut"
            );
            // Et un anonyme aussi, pour la même raison.
            assert!(
                !authz
                    .decide(&Method::DELETE, "/orders/1", &AuthContext::Anonymous)
                    .is_allow()
            );
        }

        /// **Test différentiel** : pour tout le vocabulaire SANS `perm:`, le
        /// pré-passage doit rendre exactement ce que rend le parseur du
        /// framework. Sans ce test, mon pré-passage pourrait dériver en silence.
        #[test]
        fn the_pre_pass_agrees_with_the_framework_parser() {
            let store = ObrainStore::new_in_memory();
            let authz = authz_with_iam(&store);
            let tenant = uuid::Uuid::new_v4();
            let user = uuid::Uuid::new_v4();

            let contexts = vec![
                AuthContext::Anonymous,
                ctx(tenant, user),
                AuthContext::User {
                    user_id: user,
                    tenant_id: tenant,
                    roles: vec!["admin".into()],
                },
                AuthContext::Owner {
                    user_id: user,
                    tenant_id: tenant,
                    resource_id: uuid::Uuid::new_v4(),
                    resource_type: "order".into(),
                },
                AuthContext::Admin {
                    admin_id: uuid::Uuid::new_v4(),
                },
                AuthContext::Service {
                    service_name: "svc".into(),
                    tenant_id: Some(tenant),
                },
            ];
            let policies = [
                "public",
                "authenticated",
                "owner",
                "admin_only",
                "service_only",
                "owner_or_service",
                "role:admin",
                "any:owner,role:admin",
                "all:authenticated,role:admin",
                "not:public",
                "any:public,not:authenticated",
                "resolver:inexistant",
                "all:authenticated,any:owner,role:admin",
            ];
            let resolvers = AuthResolverRegistry::new();

            for policy in policies {
                let expected = AuthPolicy::parse_policy(policy).check_with_resolver(
                    &contexts[0],
                    Some(&resolvers),
                    None,
                );
                let _ = expected;
                for context in &contexts {
                    let mut used_iam = false;
                    let mine = authz.evaluate(policy, context, &mut used_iam);
                    let theirs = AuthPolicy::parse_policy(policy).check_with_resolver(
                        context,
                        Some(&resolvers),
                        None,
                    );
                    assert_eq!(
                        mine,
                        theirs,
                        "divergence sur `{policy}` avec {}",
                        describe(context)
                    );
                    assert!(!used_iam, "`{policy}` ne doit pas consulter l'IAM");
                }
            }
        }
    }

    #[tokio::test]
    async fn the_middleware_refuses_and_allows_on_real_requests() {
        use axum::routing::{delete, get};

        let mut auth = EntityAuthConfig::default();
        auth.list = "public".to_string();
        auth.delete = "admin".to_string();
        let a = authz(
            vec![entity("order", "orders", auth)],
            &[("order", "orders")],
        );

        // Un contexte anonyme est posé par une couche interne.
        let app = || {
            axum::Router::new()
                .route("/orders", get(|| async { "liste" }))
                .route("/orders/{id}", delete(|| async { "supprime" }))
                .layer(axum::Extension(AuthContext::Anonymous))
                .layer(axum::middleware::from_fn_with_state(
                    a.clone(),
                    authorization_middleware,
                ))
        };

        assert_eq!(
            call(app(), Method::GET, "/orders").await,
            StatusCode::OK,
            "`list: public` doit passer pour un anonyme"
        );
        assert_eq!(
            call(app(), Method::DELETE, "/orders/1").await,
            StatusCode::FORBIDDEN,
            "`delete: admin` doit refuser un anonyme"
        );
        // Une route inconnue n'est pas jugée par cette couche.
        assert_eq!(
            call(app(), Method::GET, "/health").await,
            StatusCode::NOT_FOUND,
            "abstention : c'est le routeur qui répond, pas l'autorisation"
        );
    }

    #[tokio::test]
    async fn an_absent_context_is_treated_as_anonymous() {
        use axum::routing::delete;

        let mut auth = EntityAuthConfig::default();
        auth.delete = "public".to_string();
        let a = authz(
            vec![entity("order", "orders", auth)],
            &[("order", "orders")],
        );

        // Aucune couche ne pose d'AuthContext : fail-closed ⇒ anonyme.
        let mut strict = EntityAuthConfig::default();
        strict.delete = "authenticated".to_string();
        let a2 = authz(
            vec![entity("order", "orders", strict)],
            &[("order", "orders")],
        );
        let _ = a;

        let app = axum::Router::new()
            .route("/orders/{id}", delete(|| async { "supprime" }))
            .layer(axum::middleware::from_fn_with_state(
                a2,
                authorization_middleware,
            ));
        assert_eq!(
            call(app, Method::DELETE, "/orders/1").await,
            StatusCode::FORBIDDEN,
            "sans contexte, on est anonyme — et `authenticated` refuse"
        );
    }

    #[test]
    fn abstention_is_never_a_refusal() {
        let a = authz(
            vec![entity("order", "orders", admin_only_delete())],
            &[("order", "orders")],
        );
        assert!(matches!(
            a.decide(&Method::GET, "/health", &AuthContext::Anonymous),
            Decision::Abstain { .. }
        ));
    }
}
