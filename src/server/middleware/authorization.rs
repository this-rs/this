//! **Application** des politiques d'autorisation — le point qui manquait.
//!
//! # Le constat qui justifie ce module
//!
//! this-rs *déclarait* des politiques sans jamais les appliquer :
//! - `EntityAuthConfig` (`list`/`get`/`create`/`update`/`delete`/`list_links`/
//!   `create_link`/`delete_link`) n'était lu **nulle part** hors de la
//!   configuration ;
//! - `LinkAuthConfig` — la politique **par type de lien** — était déclaré et
//!   lu par personne : `AppState::get_link_auth_policy` calculait bien une
//!   politique par opération de lien, mais n'avait **aucun appelant de
//!   production**. Elle est désormais appliquée ici, **avant** la politique de
//!   l'entité source quand le chemin nomme une route de lien déclarée
//!   (`declared_link_policy`) : « lire les factures d'une commande » n'est pas
//!   « lire les commandes » ;
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
use axum::response::Response;
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
    /// Déclarée dans le **descripteur** du module (`EntityDesc.permissions` /
    /// `LinkDesc.permissions`) : tranchée par l'évaluateur unique
    /// (`engine::policy`), comme sur GraphQL, gRPC et WebSocket.
    Descriptor,
}

/// Ce qu'une route exige.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Requirement {
    /// Une politique s'applique : c'est l'opération et l'entité concernées.
    Policy {
        /// Type d'entité **source** (le pluriel de la route décide, pas le type
        /// d'arête : une route de lien est gouvernée par la politique du lien
        /// déclarée pour l'entité source, sinon par celle de l'entité source).
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

/// Les politiques exactes du vocabulaire du framework.
const KNOWN_EXACT: &[&str] = &[
    "public",
    "authenticated",
    "owner",
    "service_only",
    "admin_only",
    "owner_or_service",
];

/// Les préfixes acceptés par le vocabulaire du framework.
const KNOWN_PREFIXES: &[&str] = &[
    "all:",
    "any:",
    "not:",
    "role:",
    "owner_or_role:",
    "resolver:",
    "owner_or_resolver:",
    PERM_PREFIX,
];

/// Ce texte fait-il partie du vocabulaire **connu** ?
///
/// # Pourquoi cette fonction existe
///
/// [`AuthPolicy::parse_policy`] se replie sur `AuthPolicy::Authenticated` pour
/// **tout** texte qu'il ne reconnaît pas (`_ => AuthPolicy::Authenticated`). Ce
/// repli est un **fail-open** : une faute de frappe dans une configuration
/// `auth:` (`"n'importe quoi"`, `"PUBLIC"`, `""`) était jugée comme
/// « authenticated » — donc un simple utilisateur authentifié passait là où l'on
/// croyait exiger un administrateur.
///
/// On refuse donc explicitement ce qui n'est pas du vocabulaire, au lieu de
/// laisser le parseur deviner.
pub fn is_known_policy(policy: &str) -> bool {
    KNOWN_EXACT.contains(&policy) || KNOWN_PREFIXES.iter().any(|p| policy.starts_with(p))
}

/// Le verdict d'une politique adossée à l'IAM.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum IamVerdict {
    /// Le plan a été consulté et la décision accorde.
    #[cfg_attr(not(feature = "obrain"), allow(dead_code))]
    Allowed,
    /// Le plan a été consulté et la décision refuse.
    #[cfg_attr(not(feature = "obrain"), allow(dead_code))]
    Denied,
    /// **Rien à consulter** : aucun plan branché, ou aucun principal dans le
    /// contexte. Ce n'est ni un accord ni un refus : c'est une politique **non
    /// tranchable**, et elle doit refuser — sans jamais pouvoir être retournée
    /// en autorisation par une négation.
    Unevaluable,
}

/// Le mappage `contexte → (principal, portée)` — la couture pour les **projets
/// existants**.
///
/// Le mappage par défaut suppose le modèle de this-rs : `User`/`Owner` →
/// `(user_id, tenant_id)`, `Service` → `(service_name, tenant_id)`, `Admin` →
/// `(admin_id, "global")`. Un projet plus ancien a souvent un autre modèle (un
/// `sub` et une organisation, un compte et un espace de travail, une clé d'API
/// et un projet). Plutôt que de lui imposer une réécriture, il fournit sa
/// fonction — et l'IAM natif s'applique à **ses** identités.
pub type PrincipalMapping =
    dyn Fn(&AuthContext) -> Option<(String, String)> + Send + Sync + 'static;

/// Comment l'application des politiques se comporte.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthorizationMode {
    /// Refus effectif : la requête est arrêtée en 403.
    Enforced,
    /// **Observation** : on calcule la décision, on la **journalise**, mais on
    /// laisse passer, et la réponse porte l'en-tête `x-this-rs-authz`.
    ///
    /// C'est le mode d'adoption d'un projet existant : on voit exactement ce qui
    /// serait refusé, sur le trafic réel, avant d'armer. Passer directement de
    /// « rien n'est appliqué » à « tout est appliqué » casse une production.
    ReportOnly,
}

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
    /// Mode d'application (défaut : `Enforced`).
    mode: AuthorizationMode,
    /// Mappage `contexte → (principal, portée)`, s'il est fourni par
    /// l'application. `None` = le mappage par défaut de this-rs.
    mapping: Option<Arc<PrincipalMapping>>,
    /// Politique des **lectures de graphe** (`/graph…`), quand l'application en
    /// déclare une.
    ///
    /// Ces routes ne sont pas des routes d'entité : sans ce réglage elles
    /// **s'abstiennent**. En déclarer une est ce qui permet à un moteur
    /// d'inférence de lire la base **en respectant le protocole IAM**.
    graph_policy: Option<String>,
    /// Les chemins **déclarés publics** : les seuls où une abstention laisse
    /// passer.
    ///
    /// Une entrée terminée par `/` est un préfixe (`/auth/` couvre
    /// `/auth/token`), sinon la correspondance est exacte. Défaut : `/health`,
    /// `/healthz`, `/auth/` — la liveness et l'émission de jetons, rien
    /// d'autre. Partout ailleurs, **l'abstention est un refus** : une route que
    /// le résolveur ne reconnaît pas ne traverse plus un service armé (c'était
    /// la faille des chemins lien/nested à 4 segments, qui passaient sans
    /// garde).
    public_paths: Arc<Vec<String>>,
    /// Le catalogue vivant de l'hôte : quand il porte des descripteurs, une
    /// politique déclarée dans le descripteur décide AVANT la table YAML
    /// (`core::auth::surface::PolicyIndex`, même résolution que les autres
    /// surfaces). `None` = résolution YAML seule (historique).
    catalog: Option<Arc<crate::server::catalog::LiveCatalog>>,
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
            mode: AuthorizationMode::Enforced,
            mapping: None,
            graph_policy: None,
            public_paths: Arc::new(vec![
                "/health".to_string(),
                "/healthz".to_string(),
                "/auth/".to_string(),
            ]),
            catalog: None,
        }
    }

    /// Branche le **catalogue vivant** de l'hôte : les politiques déclarées dans
    /// les descripteurs (`EntityDesc.permissions`, `LinkDesc.permissions`)
    /// s'appliquent alors à REST avec la même préséance et le même évaluateur
    /// que GraphQL, gRPC et WebSocket (`core::auth::surface`). Sans lui, une
    /// politique déclarée seulement dans le descripteur protégeait les autres
    /// surfaces mais pas REST.
    pub fn with_catalog(mut self, catalog: Arc<crate::server::catalog::LiveCatalog>) -> Self {
        self.catalog = Some(catalog);
        self
    }

    /// Les politiques résolues **par le descripteur** pour cette requête, quand
    /// c'est lui qui décide (préséance de `PolicyIndex` : `LinkDesc`, puis
    /// `auth:` YAML du lien, puis `EntityDesc`, puis `auth:` YAML de l'entité).
    /// `None` = la table YAML (ou le défaut) décide : chemin historique inchangé.
    fn descriptor_policies(
        &self,
        segments: &[&str],
        scope_entity: &str,
        operation: &str,
    ) -> Option<Vec<crate::core::auth::surface::ResolvedPolicy>> {
        use crate::core::auth::surface::{PolicyIndex, PolicySource};
        use crate::descriptor::Operation;
        let catalog = self.catalog.as_ref()?.load();
        catalog.descriptors.modules().next()?;
        let op: Operation = operation.parse().ok()?;
        let index = PolicyIndex::new(catalog.descriptors.clone(), catalog.links_config.clone());
        let is_link = matches!(
            op,
            Operation::ListLinks
                | Operation::GetLink
                | Operation::CreateLink
                | Operation::UpdateLink
                | Operation::DeleteLink
        );
        // Same route → link resolution as the link router (and `declared_link_policy`).
        let definition = segments.get(2).filter(|_| is_link).and_then(|route| {
            catalog.links_config.links.iter().find(|d| {
                (d.source_type == scope_entity && d.forward_route_name == *route)
                    || (d.target_type == scope_entity && d.reverse_route_name == *route)
            })
        });
        let resolved = match definition {
            Some(d) => index.link_policies(
                Some(&d.link_type),
                Some(&d.source_type),
                Some(&d.target_type),
                Some(scope_entity),
                &op,
            ),
            None => vec![index.entity_policy(scope_entity, &op)],
        };
        resolved
            .iter()
            .any(|r| {
                matches!(
                    r.source,
                    PolicySource::Descriptor | PolicySource::LinkDescriptor
                )
            })
            .then_some(resolved)
    }

    /// Évalue des politiques issues du descripteur avec l'évaluateur unique.
    fn evaluate_descriptor(
        &self,
        resolved: &[crate::core::auth::surface::ResolvedPolicy],
        context: &AuthContext,
        used_iam: &mut bool,
    ) -> Result<(), String> {
        use crate::core::auth::descriptor_policy::{PermissionFn, RegistryEnv};
        use std::sync::atomic::{AtomicBool, Ordering};
        let consulted = AtomicBool::new(false);
        let perm = |p: &str| -> Option<bool> {
            let mut c = false;
            let verdict = match self.check_iam(p, context, &mut c) {
                IamVerdict::Allowed => Some(true),
                IamVerdict::Denied => Some(false),
                IamVerdict::Unevaluable => None,
            };
            if c {
                consulted.store(true, Ordering::Relaxed);
            }
            verdict
        };
        let perm_ref: &PermissionFn<'_> = &perm;
        let env = RegistryEnv::new(Some(&self.resolvers), context).with_permission(perm_ref);
        let mut outcome = Ok(());
        for r in resolved {
            if let Err(e) = crate::core::auth::surface::decide(r, context, None, &env) {
                outcome = Err(e
                    .params
                    .get("reason")
                    .and_then(|v| v.as_str())
                    .map(str::to_string)
                    .unwrap_or_else(|| e.message.clone()));
                break;
            }
        }
        *used_iam = consulted.load(Ordering::Relaxed);
        outcome
    }

    /// Le mode d'application.
    pub fn mode(&self) -> AuthorizationMode {
        self.mode
    }

    /// **Mode observation** : décide et journalise, mais laisse passer.
    ///
    /// À utiliser pour adopter ce module sur un projet qui existe déjà : on
    /// regarde le trafic réel, on corrige les politiques qui refuseraient à
    /// tort, puis on arme.
    pub fn report_only(mut self) -> Self {
        self.mode = AuthorizationMode::ReportOnly;
        self
    }

    /// **Fournit le mappage d'identité** de l'application, qui remplace celui de
    /// this-rs pour toutes les politiques `perm:`.
    ///
    /// Rend l'IAM natif pour un modèle d'authentification qui n'est pas celui de
    /// this-rs : l'application dit qui est le principal et dans quelle portée,
    /// et les décisions matérialisées s'appliquent à **ses** identités.
    ///
    /// ```rust,ignore
    /// let authz = Authorization::new(config, &registry, resolvers)
    ///     .with_principal_mapping(|ctx| match ctx {
    ///         // Modèle existant : le sujet du jeton et son organisation.
    ///         AuthContext::User { user_id, tenant_id, .. } => {
    ///             Some((format!("account:{user_id}"), format!("org:{tenant_id}")))
    ///         }
    ///         _ => None,
    ///     });
    /// ```
    pub fn with_principal_mapping<F>(mut self, mapping: F) -> Self
    where
        F: Fn(&AuthContext) -> Option<(String, String)> + Send + Sync + 'static,
    {
        self.mapping = Some(Arc::new(mapping));
        self
    }

    /// Comme [`Authorization::with_principal_mapping`], avec un mappage déjà
    /// partagé (utile quand l'hôte le porte et le passe au routeur).
    pub fn with_principal_mapping_shared(mut self, mapping: Arc<PrincipalMapping>) -> Self {
        self.mapping = Some(mapping);
        self
    }

    /// **Déclare la politique des lectures de graphe** (`/graph…`).
    ///
    /// Sans elle, les lectures de graphe s'abstiennent (descriptives, publiques
    /// par nature). Avec elle, elles sont **jugées** comme n'importe quelle
    /// route : un moteur d'inférence doit présenter un principal qui a la
    /// permission, et une politique `perm:` la tranche par la décision
    /// matérialisée.
    pub fn with_graph_policy(mut self, policy: impl Into<String>) -> Self {
        let policy = policy.into();
        self.graph_policy = (!policy.trim().is_empty()).then_some(policy);
        self
    }

    /// La politique des lectures de graphe, si elle est déclarée.
    pub fn graph_policy(&self) -> Option<&str> {
        self.graph_policy.as_deref()
    }

    /// **Remplace** la liste des chemins publics — les seuls où une abstention
    /// laisse passer.
    ///
    /// Le remplacement est TOTAL : ce qui n'y figure plus est refusé. Une
    /// entrée terminée par `/` est un préfixe, sinon la correspondance est
    /// exacte. La liste vide est légitime : plus aucun chemin ne vit
    /// d'abstention.
    pub fn with_public_paths(mut self, paths: Vec<String>) -> Self {
        self.public_paths = Arc::new(paths);
        self
    }

    /// Ce chemin est-il déclaré public ?
    pub fn is_public_path(&self, path: &str) -> bool {
        self.public_paths.iter().any(|entry| {
            match entry.strip_suffix('/') {
                // Préfixe : `/auth/` couvre `/auth/token` ET `/auth` lui-même
                // (le chemin nu ne doit pas être plus fermé que ses enfants).
                Some(stem) => path == stem || path.starts_with(entry.as_str()),
                None => path == entry,
            }
        })
    }

    /// Un mappage d'identité est-il fourni par l'application ?
    pub fn has_principal_mapping(&self) -> bool {
        self.mapping.is_some()
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
    fn iam_identity(&self, context: &AuthContext) -> Option<(String, String)> {
        if let Some(mapping) = &self.mapping {
            return mapping(context);
        }
        Self::default_iam_identity(context)
    }

    /// Le mappage **par défaut** : le modèle d'identité de this-rs.
    ///
    /// **L'IAM décide, pas la variante de contexte** : un administrateur sans
    /// décision matérialisée est refusé comme les autres. Un contexte anonyme
    /// n'a pas de principal — donc pas de décision, donc refus.
    /// **Le mappage par défaut, partagé.**
    ///
    /// Il est `pub(crate)` parce que le périmètre de lecture
    /// (`crate::graph::scope`) doit répondre à la même question — *qui est cet
    /// appelant* — que le middleware d'autorisation. Deux mappages divergents
    /// donneraient deux autorisations différentes pour une seule personne.
    #[cfg(feature = "obrain")]
    pub(crate) fn default_iam_identity(context: &AuthContext) -> Option<(String, String)> {
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
    fn evaluate(&self, policy: &str, context: &AuthContext, used_iam: &mut bool) -> Option<bool> {
        // Pas de `trim()` ici : `parse_policy` n'élague pas, et une divergence
        // dans le sens permissif (« public » entouré d'espaces ouvrait la route
        // aux anonymes) est exactement ce qu'il ne faut pas.
        // Les parties d'une composition sont, elles, élaguées — comme le fait
        // `parse_policy` après découpage.
        for (prefix, conjunction) in [("all:", true), ("any:", false)] {
            if let Some(rest) = policy.strip_prefix(prefix) {
                let parts: Vec<&str> = rest.split(',').map(str::trim).collect();
                let mut unevaluable = false;
                for part in parts {
                    match self.evaluate(part, context, used_iam) {
                        // Court-circuit : un « et » faux, un « ou » vrai.
                        Some(true) if !conjunction => return Some(true),
                        Some(false) if conjunction => return Some(false),
                        Some(_) => {}
                        None => unevaluable = true,
                    }
                }
                return if unevaluable { None } else { Some(conjunction) };
            }
        }
        if let Some(inner) = policy.strip_prefix("not:") {
            // **Une négation ne peut pas transformer « non tranchable » en
            // approbation.** Sans cette règle, `not:perm:blacklisted` sans plan
            // IAM autorisait tout le monde, anonyme compris : une liste de
            // refus devenait une liste d'admission.
            return self
                .evaluate(inner, context, used_iam)
                .map(|verdict| !verdict);
        }
        if let Some(permission) = policy.strip_prefix(PERM_PREFIX) {
            return match self.check_iam(permission.trim(), context, used_iam) {
                IamVerdict::Allowed => Some(true),
                IamVerdict::Denied => Some(false),
                IamVerdict::Unevaluable => None,
            };
        }
        if !is_known_policy(policy) {
            // Fail-closed sur le vocabulaire (voir `is_known_policy`).
            tracing::warn!(
                policy,
                "politique hors vocabulaire — refus (le repli du parseur sur \
                 `authenticated` serait un fail-open)"
            );
            return Some(false);
        }
        Some(AuthPolicy::parse_policy(policy).check_with_resolver(
            context,
            Some(&self.resolvers),
            None,
        ))
    }

    /// La décision **matérialisée** autorise-t-elle cette permission ?
    ///
    /// Une seule recherche indexée (`find_nodes_by_property` sur `acl_key`),
    /// aucune traversée. Fail-closed : sans plan IAM, sans principal, ou sans
    /// décision, c'est **non**.
    #[cfg(feature = "obrain")]
    fn check_iam(
        &self,
        permission: &str,
        context: &AuthContext,
        consulted: &mut bool,
    ) -> IamVerdict {
        let Some(iam) = &self.iam else {
            tracing::warn!(
                permission,
                "politique `perm:` sans plan IAM branché — non tranchable (refus)"
            );
            return IamVerdict::Unevaluable;
        };
        let Some((principal, scope)) = self.iam_identity(context) else {
            tracing::debug!(
                permission,
                "`perm:` sans principal (anonyme) — non tranchable"
            );
            return IamVerdict::Unevaluable;
        };
        // À partir d'ici, le plan est RÉELLEMENT consulté : la provenance peut
        // l'affirmer.
        *consulted = true;
        // **Chemin chaud** : `Iam::allows` répond à la seule question posée
        // (une recherche indexée, une lecture de propriété, un parcours de
        // chaîne) au lieu de construire une décision complète — rôles, portée,
        // principal et empreinte SHA-256 compris — que personne n'utilise ici.
        // La sémantique est identique, et un test différentiel le vérifie cas
        // par cas.
        match iam.allows(&principal, &scope, permission) {
            Ok(allowed) => {
                tracing::debug!(
                    principal,
                    scope,
                    permission,
                    allowed,
                    "décision IAM consultée (chemin chaud)"
                );
                if allowed {
                    IamVerdict::Allowed
                } else {
                    IamVerdict::Denied
                }
            }
            Err(e) => {
                tracing::warn!(error = %e, "lecture de la décision IAM échouée — refus");
                IamVerdict::Denied
            }
        }
    }

    /// Sans plan IAM, la compilation par défaut doit quand même refuser.
    #[cfg(not(feature = "obrain"))]
    fn check_iam(
        &self,
        _permission: &str,
        _context: &AuthContext,
        _consulted: &mut bool,
    ) -> IamVerdict {
        tracing::warn!("politique `perm:` sans support IAM (feature `obrain`) — non tranchable");
        IamVerdict::Unevaluable
    }

    /// La politique déclarée pour une entité et une opération.
    ///
    /// `None` = l'entité n'a **pas** d'entrée de configuration ; l'appelant
    /// décide alors d'appliquer la politique par défaut du framework — en le
    /// disant ([`PolicyOrigin::Default`]).
    fn declared_policy(&self, entity_type: &str, plural: &str, operation: &str) -> Option<String> {
        // Jointure par **type d'entité**, puis par **pluriel de la route** : une
        // entrée qui nomme le pluriel monté ne doit pas être ignorée (elle
        // faisait retomber la politique sur le défaut du framework — plus faible
        // que déclaré, sans autre trace que `origin: default`).
        let entity = self
            .config
            .entities
            .iter()
            .find(|e| e.singular == entity_type)
            .or_else(|| self.config.entities.iter().find(|e| e.plural == plural))?;
        let auth: &EntityAuthConfig = &entity.auth;
        let policy = match operation {
            "list" => &auth.list,
            "get" => &auth.get,
            "create" => &auth.create,
            "update" => &auth.update,
            "delete" => &auth.delete,
            "list_links" => &auth.list_links,
            "create_link" => &auth.create_link,
            "update_link" => &auth.update_link,
            "delete_link" => &auth.delete_link,
            _ => return None,
        };
        Some(policy.clone())
    }

    /// Les segments utiles d'un chemin (`/pages/42` → `["pages", "42"]`).
    fn segments(path: &str) -> Vec<&str> {
        path.split('/').filter(|s| !s.is_empty()).collect()
    }

    /// La politique déclarée pour le **type de lien** que ce chemin emprunte.
    ///
    /// # Pourquoi ce chemin existe
    ///
    /// `LinkAuthConfig` (`core/link.rs`) permet de déclarer une politique **par
    /// type de lien** — plus précise que celle de l'entité source : « lire les
    /// factures d'une commande » n'est pas « lire les commandes ». Il était
    /// déclaré, documenté, testé… et lu par **aucun** appelant de production :
    /// la décision se prenait sur la forme du chemin, avec l'`EntityAuthConfig`
    /// de l'entité source. Une politique déclarée et jamais appliquée est pire
    /// qu'une politique absente : elle donne l'assurance d'un réglage qui n'a
    /// pas eu lieu.
    ///
    /// La résolution est celle du **routeur** lui-même (`LinkRouteRegistry`) :
    /// un lien se désigne par le couple `(type d'entité, nom de route)`, dans un
    /// sens ou dans l'autre. On ne devine donc rien : on cherche la définition
    /// déclarée dont le **nom de route est celui du chemin**, du côté où le
    /// chemin se trouve (source en marche avant, cible en marche arrière).
    ///
    /// `None` = aucune définition ne porte d'`auth` pour cette route : le
    /// comportement d'avant est **inchangé** (politique de l'entité, puis défaut
    /// du framework). C'est ce qui rend ce correctif sans effet de bord sur les
    /// configurations existantes.
    fn declared_link_policy(
        &self,
        segments: &[&str],
        scope_entity: &str,
        operation: &str,
    ) -> Option<String> {
        // Seules les opérations dont le 3ᵉ segment est un **nom de route** :
        // `POST /{entité}/{id}/{cible}` (3 segments) crée une entité liée, son
        // 3ᵉ segment est un pluriel d'entité, pas une route de lien.
        if !matches!(
            operation,
            "list_links" | "create_link" | "update_link" | "delete_link" | "get_link"
        ) {
            return None;
        }
        let route = *segments.get(2)?;
        let definition = self.config.links.iter().find(|def| {
            def.auth.is_some()
                && ((def.source_type == scope_entity && def.forward_route_name == route)
                    || (def.target_type == scope_entity && def.reverse_route_name == route))
        })?;
        definition
            .auth
            .as_ref()?
            .policy_for(operation)
            .map(str::to_string)
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

        // **Routes du framework `/_this/*`** (export des descripteurs de module, JSON
        // Schema, OpenAPI — `exposure/descriptor.rs`). Ce ne sont pas des routes
        // d'entité : sans cette règle elles s'abstenaient, et l'abstention étant un
        // refus, l'export était injoignable pour TOUT appelant sur un serveur armé.
        // Les descripteurs décrivent aussi les politiques : on ne les ouvre pas à
        // l'anonyme — lecture `authenticated` ; toute autre méthode s'abstient (refus).
        if *plural == crate::server::exposure::descriptor::PREFIX.trim_start_matches('/') {
            return match *method {
                Method::GET | Method::HEAD => Requirement::Policy {
                    entity_type: "Descriptor".to_string(),
                    operation: "read".to_string(),
                    policy: "authenticated".to_string(),
                    origin: PolicyOrigin::Default,
                },
                _ => Requirement::Abstain {
                    reason: "`/_this/*` est en lecture seule".to_string(),
                },
            };
        }

        // **Les lectures de graphe**, d'abord : elles ne sont pas des routes
        // d'entité, et quand l'application déclare une politique pour elles,
        // c'est elle qui décide. C'est ainsi qu'un moteur d'inférence lit la
        // base **sous IAM** : il présente un principal, la politique tranche.
        if plural == &"graph"
            && let Some(policy) = &self.graph_policy
        {
            return Requirement::Policy {
                entity_type: "Graph".to_string(),
                operation: "read".to_string(),
                policy: policy.clone(),
                origin: PolicyOrigin::Declared,
            };
        }

        // **Les lectures de graphe SANS politique déclarée.** Elles ne sont pas
        // des routes d'entité : rien ne s'y rattache — et depuis que
        // l'abstention est un refus, ce « rien » refuse. Le motif le **dit** :
        // la raison générique (« `graph` n'est le pluriel d'aucune entité
        // enregistrée ») enverrait chercher une entité manquante là où il
        // manque une **politique**, et un opérateur qui cherche au mauvais
        // endroit finit par désarmer la couche pour faire taire le symptôme.
        if plural == &"graph" {
            return Requirement::Abstain {
                reason: "lectures de graphe (`/graph…`) : aucune `graph.policy` déclarée — \
                         l'abstention est un refus. Pour les ouvrir délibérément, déclarez \
                         `security.graph.policy` (`public`, `authenticated`, \
                         `perm:graph:read`, `role:…`) ; pour les ouvrir sans garde, \
                         désarmez l'autorisation."
                    .to_string(),
            };
        }

        let Some((entity_type, _)) = self.entities.iter().find(|(_, p)| p == plural) else {
            return Requirement::Abstain {
                reason: format!("`{plural}` n'est le pluriel d'aucune entité enregistrée"),
            };
        };
        // La portée peut être **déplacée** : `POST /{entité}/{id}/{cible}` crée
        // une entité liée, et c'est la politique de la CIBLE qui s'applique.
        let mut scope_entity = entity_type.clone();
        let mut scope_plural = plural.to_string();

        // **Chaque forme que le routeur sert doit être jugée.** Le routeur de
        // liens monte `/{entité}/{id}/{route}` (3), `/{src}/{id}/{route}/{cible}`
        // (4), `/{src}/{id}/{route}/{cible}/{id}` (5) et un repli qui sert les
        // chemins plus profonds : s'en tenir aux formes « canoniques » laissait
        // passer des **créations** sans politique (audit de la chaîne).
        let operation = match (method, segments.len()) {
            (&Method::GET | &Method::HEAD, 1) => "list",
            (&Method::POST, 1) => "create",
            (&Method::GET | &Method::HEAD, 2) => "get",
            (&Method::PUT | &Method::PATCH, 2) => "update",
            (&Method::DELETE, 2) => "delete",
            // Toute lecture sous une entité est une navigation de liens — y
            // compris les chemins profonds servis par le repli (`>= 3`).
            (&Method::GET | &Method::HEAD, n) if n >= 3 => "list_links",
            // `POST /{entité}/{id}/{cible}` **crée une entité liée** : c'est la
            // politique de création de la CIBLE qui s'applique, pas celle de la
            // source. Absente des formes reconnues, cette route créait sans
            // politique.
            (&Method::POST, 3) => {
                let target = segments[2];
                let Some((target_type, _)) = self.entities.iter().find(|(_, p)| p == target) else {
                    return Requirement::Abstain {
                        reason: format!("`{target}` n'est le pluriel d'aucune entité enregistrée"),
                    };
                };
                // La décision portera le nom de la cible : un opérateur doit voir
                // `Invoice · create`, pas `Order · create`.
                scope_entity = target_type.clone();
                scope_plural = target.to_string();
                "create"
            }
            (&Method::POST, n) if n >= 4 => "create_link",
            (&Method::PUT | &Method::PATCH, 4) => "update_link",
            (&Method::DELETE, n) if n >= 4 => "delete_link",
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

        // **Le descripteur d'abord** (quand il décide : voir
        // `descriptor_policies`). Même préséance que GraphQL / gRPC / WebSocket.
        if let Some(resolved) = self.descriptor_policies(&segments, &scope_entity, operation) {
            return Requirement::Policy {
                entity_type: scope_entity.clone(),
                operation: operation.to_string(),
                policy: describe_resolved(&resolved),
                origin: PolicyOrigin::Descriptor,
            };
        }

        // **Le type de lien passe avant l'entité.** Une politique déclarée sur le
        // lien (« lire les factures d'une commande ») est plus précise que celle
        // de l'entité source (« lire les commandes ») : quand elle existe, c'est
        // elle qui décide. Sinon, rien ne change — c'est le `match` ci-dessous.
        if let Some(policy) = self.declared_link_policy(&segments, &scope_entity, operation) {
            return Requirement::Policy {
                entity_type: scope_entity.clone(),
                operation: operation.to_string(),
                policy,
                origin: PolicyOrigin::Declared,
            };
        }

        match self.declared_policy(&scope_entity, &scope_plural, operation) {
            Some(policy) => Requirement::Policy {
                entity_type: scope_entity.clone(),
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
                    "update_link" => default_auth.update_link,
                    _ => default_auth.delete_link,
                };
                Requirement::Policy {
                    entity_type: scope_entity.clone(),
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

        // Politique du descripteur : l'évaluateur unique (`engine::policy`), pas
        // la grammaire plate. Re-résolue ici sur le même instantané logique ; si
        // le catalogue a changé entre-temps au point de ne plus la porter, refus.
        if origin == PolicyOrigin::Descriptor {
            let segments = Self::segments(path);
            let mut used_iam = false;
            let verdict = match self.descriptor_policies(&segments, &entity_type, &operation) {
                Some(resolved) => self.evaluate_descriptor(&resolved, context, &mut used_iam),
                None => Err("catalogue republié pendant la décision — refus".to_string()),
            };
            let origin = if used_iam { PolicyOrigin::Iam } else { origin };
            return match verdict {
                Ok(()) => Decision::Allow {
                    scope,
                    policy,
                    origin,
                },
                Err(reason) => Decision::Deny {
                    scope,
                    policy,
                    origin,
                    reason: format!("le contexte ({}) : {reason}", describe(context)),
                },
            };
        }

        // Fail-closed : une politique illisible ne doit JAMAIS laisser passer.
        // `parse_policy` retombe sur `Resolver("<texte>")`, que
        // `check_with_resolver` refuse déjà quand le résolveur est inconnu.
        let mut used_iam = false;
        let verdict = self.evaluate(&policy, context, &mut used_iam);
        // Une politique tranchée par l'IAM matérialisé le DIT : la provenance
        // d'une décision ne se devine pas.
        let origin = if used_iam { PolicyOrigin::Iam } else { origin };

        match verdict {
            Some(true) => Decision::Allow {
                scope,
                policy,
                origin,
            },
            Some(false) => Decision::Deny {
                scope,
                policy,
                origin,
                reason: format!(
                    "le contexte ({}) ne satisfait pas la politique",
                    describe(context)
                ),
            },
            None => Decision::Deny {
                scope,
                policy,
                origin,
                reason: "politique non tranchable (IAM non consultable) — refus fail-closed"
                    .to_string(),
            },
        }
    }
}

/// Le texte d'une (ou de plusieurs, toutes requises) politique(s) résolue(s).
fn describe_resolved(resolved: &[crate::core::auth::surface::ResolvedPolicy]) -> String {
    let texts: Vec<String> = resolved
        .iter()
        .map(|r| match &r.policy {
            Ok(e) => e.to_string(),
            Err(raw) => raw.clone(),
        })
        .collect();
    match texts.as_slice() {
        [one] => one.clone(),
        many => format!("all({})", many.join(", ")),
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
        Decision::Allow { .. } => next.run(request).await,
        // **L'abstention n'est plus un passage.** Elle l'était : toute route
        // que le résolveur ne reconnaissait pas (chemins lien/nested à
        // 4 segments d'un pluriel non enregistré, formes non reconnues)
        // traversait le service ARMÉ sans décision — la faille 4-segments.
        // Désormais : seuls les chemins **déclarés publics** passent ; partout
        // ailleurs, refus par défaut. Le mode observation observe (la requête
        // passe, la réponse le dit) — c'est le chemin d'adoption.
        Decision::Abstain { reason } => {
            if authz.is_public_path(&path) {
                return next.run(request).await;
            }
            if authz.mode() == AuthorizationMode::ReportOnly {
                tracing::warn!(
                    %method,
                    %path,
                    %reason,
                    "autorisation : abstention OBSERVÉE — mode observation, la requête passe \
                     (hors observation, ce chemin sans garde serait refusé)"
                );
                let mut response = next.run(request).await;
                response.headers_mut().insert(
                    header_name("x-this-rs-authz"),
                    header_value("observed-deny-abstain"),
                );
                return response;
            }
            tracing::debug!(
                %method,
                %path,
                %reason,
                "abstention refusée — chemin sans garde explicite (refus par défaut)"
            );
            let mut response = forbidden_response(
                &format!("{method} {path}"),
                json!({
                    "error": "forbidden",
                    "verdict": "deny_abstain",
                    "reason": reason,
                }),
            );
            response
                .headers_mut()
                .insert(header_name("x-this-rs-authz"), header_value("deny-abstain"));
            response
        }
        Decision::Deny {
            scope,
            policy,
            origin,
            reason,
        } => {
            // **Mode observation** : on décide, on le dit dans les journaux et
            // dans la réponse, mais la requête passe. C'est ainsi qu'un projet
            // qui existe déjà adopte ce module sans casser sa production : il
            // voit, sur le trafic réel, ce qui serait refusé — puis il arme.
            if authz.mode() == AuthorizationMode::ReportOnly {
                tracing::warn!(
                    %method,
                    %path,
                    %scope,
                    %policy,
                    origin = ?origin,
                    %reason,
                    "autorisation : refus OBSERVÉ — mode observation, la requête passe \
                     (armer avec `with_authorization()` après avoir lu ces journaux)"
                );
                let mut response = next.run(request).await;
                let headers = response.headers_mut();
                headers.insert(
                    header_name("x-this-rs-authz"),
                    header_value("observed-deny"),
                );
                headers.insert(header_name("x-this-rs-authz-policy"), header_value(&policy));
                headers.insert(header_name("x-this-rs-authz-scope"), header_value(&scope));
                return response;
            }

            tracing::debug!(%method, %path, %scope, %policy, "autorisation refusée");
            forbidden_response(
                &format!("{method} {path}"),
                json!({
                    "error": "forbidden",
                    "scope": scope,
                    "policy": policy,
                    "origin": origin,
                    "reason": reason,
                }),
            )
        }
    }
}

/// 403 of the authorization layer: the contract envelope (`this.auth.forbidden`, localized by
/// `localize_errors`) PLUS the legacy keys (`error`, `scope`, `policy`, `origin`, `reason`,
/// `verdict`), unchanged.
fn forbidden_response(operation: &str, legacy: serde_json::Value) -> Response {
    let extras = match legacy {
        serde_json::Value::Object(m) => m,
        _ => serde_json::Map::new(),
    };
    crate::core::errors::error_response_with(
        crate::core::errors::forbidden(operation),
        Some(StatusCode::FORBIDDEN),
        extras,
    )
}

/// Un nom d'en-tête constant (connu, donc infaillible).
fn header_name(name: &'static str) -> axum::http::HeaderName {
    axum::http::HeaderName::from_static(name)
}

/// Une valeur d'en-tête sûre : une portée ou une politique exotique ne doit pas
/// faire échouer la réponse — on la remplace alors par une forme lisible.
fn header_value(value: &str) -> axum::http::HeaderValue {
    axum::http::HeaderValue::from_str(value)
        .unwrap_or_else(|_| axum::http::HeaderValue::from_static("non-ascii"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::EntityConfig;
    use crate::core::link::{LinkAuthConfig, LinkDefinition};
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
        EntityAuthConfig {
            // « admin » n'est PAS du vocabulaire : c'était un fail-open (le parseur
            // retombait sur `authenticated`, donc un simple utilisateur passait).
            // Le vrai mot est `admin_only`.
            delete: "admin_only".to_string(),
            list: "public".to_string(),
            ..Default::default()
        }
    }

    // ── Politiques par TYPE DE LIEN ──────────────────────────────────────
    //
    // `LinkAuthConfig` était déclaré, documenté, testé… et lu par aucun
    // appelant de production. Ces tests tiennent l'application : une politique
    // de lien décide **avant** celle de l'entité, et seulement sur SA route.

    fn authz_avec_liens(
        entities: Vec<EntityConfig>,
        plurals: &[(&'static str, &'static str)],
        links: Vec<LinkDefinition>,
    ) -> Authorization {
        let config = LinksConfig {
            entities,
            links,
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

    /// Une définition de lien : `source` navigue vers `target` par `route`.
    fn lien(
        source: &str,
        target: &str,
        route: &str,
        route_inverse: &str,
        auth: Option<LinkAuthConfig>,
    ) -> LinkDefinition {
        LinkDefinition {
            link_type: format!("{source}-{target}"),
            source_type: source.to_string(),
            target_type: target.to_string(),
            forward_route_name: route.to_string(),
            reverse_route_name: route_inverse.to_string(),
            description: None,
            required_fields: None,
            auth,
        }
    }

    /// Le cas qui justifie tout ce bloc : l'entité dit `public`, le lien dit
    /// `admin_only`. C'est le lien qui doit décider — il est plus précis.
    #[test]
    fn une_politique_de_type_de_lien_decide_avant_celle_de_l_entite() {
        let auth = EntityAuthConfig {
            list_links: "public".to_string(),
            ..Default::default()
        };
        let a = authz_avec_liens(
            vec![entity("order", "orders", auth)],
            &[("order", "orders")],
            vec![lien(
                "order",
                "invoice",
                "invoices",
                "order",
                Some(LinkAuthConfig {
                    list: "admin_only".to_string(),
                    ..Default::default()
                }),
            )],
        );

        assert_eq!(
            a.requirement(&Method::GET, "/orders/42/invoices"),
            Requirement::Policy {
                entity_type: "order".to_string(),
                operation: "list_links".to_string(),
                policy: "admin_only".to_string(),
                origin: PolicyOrigin::Declared,
            }
        );
    }

    /// Le même lien, emprunté dans l'autre sens, est jugé par la **même**
    /// déclaration : la route inverse est la même arête vue de la cible.
    #[test]
    fn la_route_inverse_du_meme_lien_est_jugee_par_sa_declaration() {
        let a = authz_avec_liens(
            vec![
                entity("order", "orders", EntityAuthConfig::default()),
                entity("invoice", "invoices", EntityAuthConfig::default()),
            ],
            &[("order", "orders"), ("invoice", "invoices")],
            vec![lien(
                "order",
                "invoice",
                "invoices",
                "order",
                Some(LinkAuthConfig {
                    list: "service_only".to_string(),
                    ..Default::default()
                }),
            )],
        );

        assert_eq!(
            a.requirement(&Method::GET, "/invoices/7/order"),
            Requirement::Policy {
                entity_type: "invoice".to_string(),
                operation: "list_links".to_string(),
                policy: "service_only".to_string(),
                origin: PolicyOrigin::Declared,
            }
        );
    }

    /// Créer un lien se lit sur `auth.create` du **type de lien**, pas sur
    /// `create_link` de l'entité source.
    #[test]
    fn creer_un_lien_suit_la_politique_de_creation_du_type_de_lien() {
        let auth = EntityAuthConfig {
            create_link: "public".to_string(),
            ..Default::default()
        };
        let a = authz_avec_liens(
            vec![entity("order", "orders", auth)],
            &[("order", "orders")],
            vec![lien(
                "order",
                "invoice",
                "invoices",
                "order",
                Some(LinkAuthConfig {
                    create: "admin_only".to_string(),
                    ..Default::default()
                }),
            )],
        );

        assert_eq!(
            a.requirement(&Method::POST, "/orders/42/invoices/invoice/7"),
            Requirement::Policy {
                entity_type: "order".to_string(),
                operation: "create_link".to_string(),
                policy: "admin_only".to_string(),
                origin: PolicyOrigin::Declared,
            }
        );
    }

    /// **Sans `auth` sur le lien, rien ne change** — c'est ce qui rend ce
    /// correctif sans effet de bord sur les configurations existantes.
    #[test]
    fn sans_auth_sur_le_lien_la_politique_de_l_entite_reste_seule() {
        let auth = EntityAuthConfig {
            list_links: "admin_only".to_string(),
            ..Default::default()
        };
        let a = authz_avec_liens(
            vec![entity("order", "orders", auth)],
            &[("order", "orders")],
            vec![lien("order", "invoice", "invoices", "order", None)],
        );

        assert_eq!(
            a.requirement(&Method::GET, "/orders/42/invoices"),
            Requirement::Policy {
                entity_type: "order".to_string(),
                operation: "list_links".to_string(),
                policy: "admin_only".to_string(),
                origin: PolicyOrigin::Declared,
            }
        );
    }

    /// Une politique de lien ne déborde **pas** sur les autres routes de la même
    /// entité : sa portée est sa route, et rien d'autre.
    #[test]
    fn une_politique_de_lien_ne_deborde_pas_sur_les_autres_routes() {
        let auth = EntityAuthConfig {
            list: "public".to_string(),
            ..Default::default()
        };
        let lien_verrouille = || {
            vec![lien(
                "order",
                "invoice",
                "invoices",
                "order",
                Some(LinkAuthConfig {
                    list: "admin_only".to_string(),
                    ..Default::default()
                }),
            )]
        };
        let a = authz_avec_liens(
            vec![entity("order", "orders", auth)],
            &[("order", "orders")],
            lien_verrouille(),
        );

        // La liste des commandes garde SA politique.
        assert_eq!(
            a.requirement(&Method::GET, "/orders"),
            Requirement::Policy {
                entity_type: "order".to_string(),
                operation: "list".to_string(),
                policy: "public".to_string(),
                origin: PolicyOrigin::Declared,
            }
        );
        // Une navigation par une AUTRE route de la même entité aussi.
        assert_eq!(
            a.requirement(&Method::GET, "/orders/42/vehicules"),
            Requirement::Policy {
                entity_type: "order".to_string(),
                operation: "list_links".to_string(),
                policy: "authenticated".to_string(),
                origin: PolicyOrigin::Declared,
            }
        );
    }

    /// Un lien déclaré pour **une autre entité source** ne décide pas ici : la
    /// résolution est celle du routeur — le couple (type d'entité, nom de route),
    /// jamais le nom de route seul.
    #[test]
    fn un_lien_d_une_autre_entite_source_ne_decide_pas() {
        let a = authz_avec_liens(
            vec![entity("order", "orders", EntityAuthConfig::default())],
            &[("order", "orders")],
            vec![lien(
                "customer",
                "invoice",
                "invoices",
                "customer",
                Some(LinkAuthConfig {
                    list: "admin_only".to_string(),
                    ..Default::default()
                }),
            )],
        );

        assert_eq!(
            a.requirement(&Method::GET, "/orders/42/invoices"),
            Requirement::Policy {
                entity_type: "order".to_string(),
                operation: "list_links".to_string(),
                policy: "authenticated".to_string(),
                origin: PolicyOrigin::Declared,
            }
        );
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
        // **Correction d'audit** : un chemin profond sous une entité connue
        // n'est PLUS abstenu. Le routeur de liens monte un repli qui **sert**
        // ces chemins (`build_link_routes().fallback`) : s'abstenir y laissait
        // une lecture non jugée. Toute lecture sous une entité est une
        // navigation de liens, donc `list_links`.
        assert!(matches!(
            a.requirement(&Method::GET, "/orders/42/a/b/c/d"),
            Requirement::Policy { ref operation, .. } if operation == "list_links"
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
        let auth = EntityAuthConfig {
            delete: "role:admin".to_string(),
            ..Default::default()
        };
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
        let auth = EntityAuthConfig {
            // Ni un mot-clé connu, ni un résolveur enregistré.
            create: "resolver:inexistant".to_string(),
            ..Default::default()
        };
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
        let auth = EntityAuthConfig {
            update: "owner".to_string(),
            ..Default::default()
        };
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
            let auth = EntityAuthConfig {
                delete: "perm:orders:write".to_string(),
                list: "any:perm:orders:read,role:auditor".to_string(),
                ..Default::default()
            };
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
            let auth = EntityAuthConfig {
                delete: "perm:orders:write".to_string(),
                ..Default::default()
            };
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
                    // Le vocabulaire est **clos** : dès qu'une partie n'est pas
                    // un mot connu (partie vide d'une virgule double, mot
                    // inventé), le pré-passage REFUSE là où `parse_policy`
                    // retombait sur `authenticated`. Divergence VOULUE : on
                    // l'affirme au lieu de la subir.
                    assert_eq!(
                        mine,
                        Some(theirs),
                        "divergence sur `{policy}` avec {}",
                        describe(context)
                    );
                    assert!(!used_iam, "`{policy}` ne doit pas consulter l'IAM");
                }
            }
        }
    }

    // ── Adoption d'un projet existant : observation + mappage d'identité ──

    /// Un mappage d'identité **d'un autre modèle** : le projet nomme ses
    /// principaux `account:<id>` et ses portées `org:<id>`.
    #[cfg(feature = "obrain")]
    fn legacy_mapping() -> impl Fn(&AuthContext) -> Option<(String, String)> + Send + Sync + 'static
    {
        |ctx: &AuthContext| match ctx {
            AuthContext::User {
                user_id, tenant_id, ..
            } => Some((format!("account:{user_id}"), format!("org:{tenant_id}"))),
            _ => None,
        }
    }

    #[cfg(feature = "obrain")]
    #[test]
    fn a_custom_mapping_is_what_makes_the_iam_native_to_another_model() {
        use crate::storage::ObrainStore;
        use crate::storage::iam::RoleGrant;

        let store = ObrainStore::new_in_memory();
        let iam = store.iam();
        let tenant = uuid::Uuid::from_u128(7);
        let user = uuid::Uuid::from_u128(42);
        let ctx = AuthContext::User {
            user_id: user,
            tenant_id: tenant,
            roles: vec![],
        };

        // La décision est matérialisée sous les noms **du projet**, pas sous
        // ceux de this-rs : c'est lui qui dit qui est le principal.
        iam.materialize(
            "account:00000000-0000-0000-0000-00000000002a",
            "org:00000000-0000-0000-0000-000000000007",
            &[RoleGrant {
                role: "reader".into(),
                permissions: vec!["orders:read".into()],
            }],
        )
        .expect("matérialisation");

        let auth = EntityAuthConfig {
            list: "perm:orders:read".to_string(),
            ..Default::default()
        };
        let config = LinksConfig {
            entities: vec![entity("order", "orders", auth)],
            links: Vec::<LinkDefinition>::new(),
            validation_rules: None,
            events: None,
            sinks: None,
            auth: None,
        };
        let base = || {
            Authorization::new(
                Arc::new(config.clone()),
                &registry(&[("order", "orders")]),
                Arc::new(AuthResolverRegistry::new()),
            )
            .with_iam(&store)
        };

        // Sans mappage : le modèle par défaut cherche `(user_id, tenant_id)` et
        // ne trouve AUCUNE décision — refus.
        assert!(
            !base().decide(&Method::GET, "/orders", &ctx).is_allow(),
            "le mappage par défaut ne doit pas trouver une décision nommée autrement"
        );

        // Avec le mappage du projet : la décision matérialisée est trouvée, et
        // la politique est tranchée — l'IAM est **natif** pour ses identités.
        let authz = base().with_principal_mapping(legacy_mapping());
        assert!(authz.has_principal_mapping());
        match authz.decide(&Method::GET, "/orders", &ctx) {
            Decision::Allow { origin, .. } => assert_eq!(origin, PolicyOrigin::Iam),
            other => panic!("le mappage du projet doit rendre la décision lisible : {other:?}"),
        }
    }

    #[cfg(feature = "obrain")]
    #[test]
    fn a_mapping_that_declines_a_context_keeps_the_policy_unevaluable() {
        let store = crate::storage::ObrainStore::new_in_memory();
        let auth = EntityAuthConfig {
            list: "perm:orders:read".to_string(),
            ..Default::default()
        };
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
        )
        .with_iam(&store)
        // Le projet ne reconnaît que les utilisateurs : ce contexte-là n'a pas
        // d'identité chez lui.
        .with_principal_mapping(|_| None);

        match authz.decide(&Method::GET, "/orders", &AuthContext::Anonymous) {
            Decision::Deny { reason, .. } => assert!(
                reason.contains("non tranchable"),
                "un contexte sans identité doit rester non tranchable : {reason}"
            ),
            other => panic!("attendu un refus : {other:?}"),
        }
    }

    #[test]
    fn the_default_mode_is_enforced_and_report_only_is_explicit() {
        let authz = authz(
            vec![entity("order", "orders", EntityAuthConfig::default())],
            &[("order", "orders")],
        );
        assert_eq!(
            authz.mode(),
            AuthorizationMode::Enforced,
            "le défaut doit rester le refus effectif"
        );
        assert_eq!(authz.report_only().mode(), AuthorizationMode::ReportOnly);
    }

    /// **Le test qui porte l'adoption d'un projet existant** : en observation,
    /// un refus est calculé, journalisé et **publié**, mais la requête passe.
    #[cfg(feature = "obrain")]
    #[tokio::test]
    async fn report_only_publishes_the_denial_without_blocking() {
        use axum::routing::delete;

        let store = crate::storage::ObrainStore::new_in_memory();
        let auth = EntityAuthConfig {
            delete: "admin_only".to_string(),
            ..Default::default()
        };
        let config = LinksConfig {
            entities: vec![entity("order", "orders", auth)],
            links: Vec::<LinkDefinition>::new(),
            validation_rules: None,
            events: None,
            sinks: None,
            auth: None,
        };
        let base = || {
            Authorization::new(
                Arc::new(config.clone()),
                &registry(&[("order", "orders")]),
                Arc::new(AuthResolverRegistry::new()),
            )
            .with_iam(&store)
        };

        let app = |authz: Authorization| {
            axum::Router::new()
                .route("/orders/{id}", delete(|| async { "supprime" }))
                .layer(axum::Extension(AuthContext::Anonymous))
                .layer(axum::middleware::from_fn_with_state(
                    authz,
                    authorization_middleware,
                ))
        };

        // Armé : refus effectif.
        assert_eq!(
            call(app(base()), Method::DELETE, "/orders/1").await,
            StatusCode::FORBIDDEN,
            "en mode armé, un anonyme n'efface pas"
        );

        // Observation : la requête PASSE, et la réponse dit ce qui serait refusé.
        use axum::body::Body;
        use tower::ServiceExt;
        let response = app(base().report_only())
            .oneshot(
                axum::http::Request::builder()
                    .method(Method::DELETE)
                    .uri("/orders/1")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .expect("réponse");
        assert_eq!(
            response.status(),
            StatusCode::OK,
            "en observation, rien n'est bloqué — c'est le principe"
        );
        let headers = response.headers();
        assert_eq!(
            headers.get("x-this-rs-authz").map(|v| v.to_str().unwrap()),
            Some("observed-deny"),
            "la réponse doit publier le refus observé"
        );
        assert_eq!(
            headers
                .get("x-this-rs-authz-policy")
                .map(|v| v.to_str().unwrap()),
            Some("admin_only")
        );
        assert!(
            headers.get("x-this-rs-authz-scope").is_some(),
            "la portée concernée doit être lisible"
        );
    }

    #[tokio::test]
    async fn the_middleware_refuses_and_allows_on_real_requests() {
        use axum::routing::{delete, get};

        let auth = EntityAuthConfig {
            list: "public".to_string(),
            delete: "admin".to_string(),
            ..Default::default()
        };
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

        let auth = EntityAuthConfig {
            delete: "public".to_string(),
            ..Default::default()
        };
        let a = authz(
            vec![entity("order", "orders", auth)],
            &[("order", "orders")],
        );

        // Aucune couche ne pose d'AuthContext : fail-closed ⇒ anonyme.
        let strict = EntityAuthConfig {
            delete: "authenticated".to_string(),
            ..Default::default()
        };
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
    fn descriptor_export_requires_authentication() {
        let a = authz(vec![], &[]);
        assert_eq!(
            a.requirement(&Method::GET, "/_this/descriptors/shop"),
            Requirement::Policy {
                entity_type: "Descriptor".to_string(),
                operation: "read".to_string(),
                policy: "authenticated".to_string(),
                origin: PolicyOrigin::Default,
            }
        );
        assert!(
            !a.decide(&Method::GET, "/_this/openapi.json", &AuthContext::Anonymous)
                .is_allow()
        );
        let user = AuthContext::User {
            user_id: uuid::Uuid::new_v4(),
            tenant_id: uuid::Uuid::new_v4(),
            roles: vec![],
        };
        assert!(
            a.decide(&Method::GET, "/_this/descriptors", &user)
                .is_allow()
        );
        // Read-only: any write shape abstains (= refused when armed).
        assert!(matches!(
            a.requirement(&Method::POST, "/_this/descriptors"),
            Requirement::Abstain { .. }
        ));
        // `_thisx` is not the framework prefix.
        assert!(matches!(
            a.requirement(&Method::GET, "/_thisx/descriptors"),
            Requirement::Abstain { .. }
        ));
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

// ══════════════════════════════════════════════════════════════════════════════
// Périmètre dédié : **le vocabulaire des politiques et son évaluation**.
//
// Ce module est un enfant de `authorization`, donc il atteint délibérément les
// éléments privés (`evaluate`, `check_iam`, `iam_identity`, `describe`) : ce sont
// des chemins de décision, pas des détails d'implémentation.
// ══════════════════════════════════════════════════════════════════════════════
#[cfg(test)]
mod policy_perimeter {
    use super::*;
    use crate::config::EntityConfig;
    use crate::core::link::LinkDefinition;
    use crate::server::entity_registry::EntityDescriptor;
    use serde_json::json;

    // ── Montage ──────────────────────────────────────────────────────────

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

    fn entity(singular: &str, plural: &str, auth: EntityAuthConfig) -> EntityConfig {
        EntityConfig {
            singular: singular.to_string(),
            plural: plural.to_string(),
            auth,
        }
    }

    fn config_of(entities: Vec<EntityConfig>) -> LinksConfig {
        LinksConfig {
            entities,
            links: Vec::<LinkDefinition>::new(),
            validation_rules: None,
            events: None,
            sinks: None,
            auth: None,
        }
    }

    fn authz_with(config: LinksConfig, plurals: &[(&'static str, &'static str)]) -> Authorization {
        Authorization::new(
            Arc::new(config),
            &registry(plurals),
            Arc::new(AuthResolverRegistry::new()),
        )
    }

    /// La **même** politique sur les huit opérations : les tests de vocabulaire
    /// n'ont pas à dépendre de la table route → opération.
    fn policy_everywhere(policy: &str) -> EntityAuthConfig {
        EntityAuthConfig {
            list: policy.to_string(),
            get: policy.to_string(),
            create: policy.to_string(),
            update: policy.to_string(),
            delete: policy.to_string(),
            list_links: policy.to_string(),
            create_link: policy.to_string(),
            delete_link: policy.to_string(),
            ..Default::default()
        }
    }

    fn authz_policy(policy: &str) -> Authorization {
        authz_with(
            config_of(vec![entity("order", "orders", policy_everywhere(policy))]),
            &[("order", "orders")],
        )
    }

    // ── Contextes ────────────────────────────────────────────────────────
    //
    // Identifiants fixes : les matrices comparent des verdicts, et une
    // comparaison ne doit pas dépendre du hasard d'un `new_v4()`.

    const USER: u128 = 0x1111_1111_1111_1111_1111_1111_1111_1111;
    #[cfg(feature = "obrain")]
    const OTHER_USER: u128 = 0x5555_5555_5555_5555_5555_5555_5555_5555;
    const TENANT: u128 = 0x2222_2222_2222_2222_2222_2222_2222_2222;
    const ADMIN: u128 = 0x3333_3333_3333_3333_3333_3333_3333_3333;
    const RESOURCE: u128 = 0x4444_4444_4444_4444_4444_4444_4444_4444;

    fn uid(v: u128) -> uuid::Uuid {
        uuid::Uuid::from_u128(v)
    }

    fn user_ctx(roles: &[&str]) -> AuthContext {
        AuthContext::User {
            user_id: uid(USER),
            tenant_id: uid(TENANT),
            roles: roles.iter().map(|r| r.to_string()).collect(),
        }
    }

    fn owner_ctx() -> AuthContext {
        AuthContext::Owner {
            user_id: uid(USER),
            tenant_id: uid(TENANT),
            resource_id: uid(RESOURCE),
            resource_type: "order".into(),
        }
    }

    fn service_ctx(tenant: Option<uuid::Uuid>) -> AuthContext {
        AuthContext::Service {
            service_name: "svc".into(),
            tenant_id: tenant,
        }
    }

    fn admin_ctx() -> AuthContext {
        AuthContext::Admin {
            admin_id: uid(ADMIN),
        }
    }

    /// **Les sept contextes** du vocabulaire, dans un ordre stable — c'est
    /// l'index utilisé par les matrices de verdicts.
    fn all_contexts() -> Vec<(&'static str, AuthContext)> {
        vec![
            ("anonyme", AuthContext::Anonymous),
            ("utilisateur", user_ctx(&[])),
            ("utilisateur admin", user_ctx(&["admin"])),
            ("propriétaire", owner_ctx()),
            ("service en locataire", service_ctx(Some(uid(TENANT)))),
            ("service global", service_ctx(None)),
            ("administrateur", admin_ctx()),
        ]
    }

    // ── 1. Le vocabulaire × tous les contextes ───────────────────────────
    //
    // Table de verdicts explicite : chaque ligne est une politique du
    // vocabulaire, chaque colonne un contexte. C'est le filet qui attrape une
    // dérive silencieuse de la sémantique d'un mot-clé.

    const ANON: usize = 0;
    const USER_: usize = 1;
    const USER_ADMIN: usize = 2;
    const OWNER: usize = 3;
    const SERVICE_TENANT: usize = 4;
    const SERVICE_GLOBAL: usize = 5;
    const ADMIN_: usize = 6;

    #[test]
    fn the_vocabulary_decides_the_same_way_for_every_context_kind() {
        let authz = authz_policy("public");
        // [anonyme, utilisateur, utilisateur admin, propriétaire,
        //  service en locataire, service global, administrateur]
        let cases: &[(&str, [bool; 7])] = &[
            ("public", [true, true, true, true, true, true, true]),
            ("authenticated", [false, true, true, true, true, true, true]),
            ("owner", [false, false, false, true, false, false, false]),
            (
                "admin_only",
                [false, false, false, false, false, false, true],
            ),
            (
                "service_only",
                [false, false, false, false, true, true, false],
            ),
            (
                "owner_or_service",
                [false, false, false, true, true, true, false],
            ),
            (
                "role:admin",
                [false, false, true, false, false, false, false],
            ),
            (
                "role:reader",
                [false, false, false, false, false, false, false],
            ),
            ("role:", [false, false, false, false, false, false, false]),
            (
                "owner_or_role:admin",
                [false, false, true, true, false, false, false],
            ),
            (
                "owner_or_role:",
                [false, false, false, true, false, false, false],
            ),
            ("resolver:inconnu", [false; 7]),
            (
                "owner_or_resolver:inconnu",
                [false, false, false, true, false, false, false],
            ),
            (
                "all:authenticated,role:admin",
                [false, false, true, false, false, false, false],
            ),
            (
                "all:public,authenticated",
                [false, true, true, true, true, true, true],
            ),
            ("all:owner,service_only", [false; 7]),
            (
                "any:owner,role:admin",
                [false, false, true, true, false, false, false],
            ),
            (
                "any:service_only,admin_only",
                [false, false, false, false, true, true, true],
            ),
            ("any:public,not:authenticated", [true; 7]),
            ("not:public", [false; 7]),
            (
                "not:authenticated",
                [true, false, false, false, false, false, false],
            ),
            ("not:not:public", [true; 7]),
            (
                // `any:owner` n'est PAS un OU : avec une seule branche, le
                // framework rend la branche elle-même — donc ici `all:` exige
                // réellement `owner`.
                "all:any:owner,role:admin,authenticated",
                [false; 7],
            ),
            (
                "any:all:owner,role:admin,service_only",
                [false, false, true, true, true, true, false],
            ),
        ];

        for (policy, expected) in cases {
            for (i, (name, ctx)) in all_contexts().iter().enumerate() {
                let mut used_iam = false;
                let got = authz.evaluate(policy, ctx, &mut used_iam);
                assert_eq!(
                    got,
                    Some(expected[i]),
                    "`{policy}` pour {name} : verdict inattendu (colonne {i})"
                );
                assert!(
                    !used_iam,
                    "`{policy}` ne contient pas `perm:` — l'IAM ne doit pas être consulté"
                );
            }
        }
    }

    /// L'index des colonnes de la table ci-dessus doit rester celui de
    /// [`all_contexts`] : sinon la table comparerait des colonnes décalées et
    /// resterait verte en disant n'importe quoi.
    #[test]
    fn the_verdict_matrix_columns_match_the_context_list() {
        let contexts = all_contexts();
        assert_eq!(contexts.len(), 7);
        assert!(matches!(contexts[ANON].1, AuthContext::Anonymous));
        assert!(matches!(contexts[USER_].1, AuthContext::User { .. }));
        assert!(matches!(contexts[USER_ADMIN].1, AuthContext::User { .. }));
        assert!(matches!(contexts[OWNER].1, AuthContext::Owner { .. }));
        assert!(matches!(
            contexts[SERVICE_TENANT].1,
            AuthContext::Service {
                tenant_id: Some(_),
                ..
            }
        ));
        assert!(matches!(
            contexts[SERVICE_GLOBAL].1,
            AuthContext::Service {
                tenant_id: None,
                ..
            }
        ));
        assert!(matches!(contexts[ADMIN_].1, AuthContext::Admin { .. }));
        // les rôles distinguent les deux utilisateurs
        assert!(contexts[USER_].1.roles().is_empty());
        assert_eq!(contexts[USER_ADMIN].1.roles().len(), 1);
        assert_eq!(contexts[USER_ADMIN].1.roles()[0], "admin");
    }

    // ── 2. Fail-closed ───────────────────────────────────────────────────

    /// Un résolveur absent refuse **pour tous les contextes**, y compris
    /// l'administrateur et le service : la variante de contexte ne rachète pas
    /// une politique illisible.
    #[test]
    fn an_unreadable_resolver_denies_for_every_context_kind() {
        let authz = authz_policy("resolver:absent_du_registre");
        for (name, ctx) in all_contexts() {
            let decision = authz.decide(&Method::GET, "/orders", &ctx);
            match decision {
                Decision::Deny {
                    scope,
                    policy,
                    reason,
                    ..
                } => {
                    assert_eq!(scope, "order · list");
                    assert_eq!(policy, "resolver:absent_du_registre");
                    assert!(
                        !reason.trim().is_empty(),
                        "{name} : un refus doit être explicable"
                    );
                }
                other => panic!("{name} : un résolveur absent doit refuser, obtenu {other:?}"),
            }
        }
    }

    /// **Défaut constaté.** L'en-tête de `decide` annonce que `parse_policy`
    /// retombe sur `Resolver(<texte>)` : c'est faux, la branche finale est
    /// `_ => AuthPolicy::Authenticated`. Un texte hors vocabulaire (faute de
    /// frappe, politique vide) **autorise donc tout contexte authentifié** au
    /// lieu de refuser.
    #[test]
    fn a_policy_word_outside_the_vocabulary_denies_instead_of_authenticating() {
        let authz = authz_policy("");
        for policy in ["", "foobar:x", "PUBLIC", "Public", "admin", "role"] {
            for (name, ctx) in all_contexts() {
                let mut used_iam = false;
                let allowed = authz.evaluate(policy, &ctx, &mut used_iam);
                assert_eq!(
                    allowed,
                    Some(false),
                    "hors vocabulaire : refus explicite attendu (jamais `None`, qui dirait « non tranchable »)"
                );
                assert!(
                    allowed == Some(false),
                    "`{policy}` est hors vocabulaire : il doit refuser pour {name}, jamais approuver"
                );
            }
        }
    }

    /// **Corrigé** : `evaluate` **n'élague plus** la politique entière avant de
    /// la parser — `parse_policy` ne l'élague pas, et une divergence dans le sens
    /// permissif est exactement ce qu'il ne faut pas. Un mot-clé entouré
    /// d'espaces n'appartient donc plus au vocabulaire : il est **refusé**.
    #[test]
    fn a_padded_policy_word_does_not_change_the_frame() {
        let authz = authz_policy("public");
        for (name, ctx) in all_contexts() {
            let mut used_iam = false;
            assert_eq!(
                authz.evaluate(" public ", &ctx, &mut used_iam),
                Some(false),
                "un mot-clé entouré d'espaces n'est pas du vocabulaire : refus pour {name}"
            );
            assert!(
                !used_iam,
                "le vocabulaire ne doit jamais consulter l'IAM ({name})"
            );
        }
    }

    /// Le rôle littéral : `role:admin,editor` n'est **pas** une liste de rôles —
    /// Toutes les parties d'une politique appartiennent-elles au vocabulaire ?
    ///
    /// Le pré-passage descend les compositions (`all:`, `any:`, `not:`) : une
    /// partie **vide** (virgule double) ou un mot inventé n'en fait pas partie.
    fn every_part_is_known(policy: &str) -> bool {
        fn known(part: &str) -> bool {
            let part = part.trim();
            if part.is_empty() {
                return false;
            }
            for prefix in ["all:", "any:"] {
                if let Some(rest) = part.strip_prefix(prefix) {
                    return rest.split(',').all(known);
                }
            }
            if let Some(inner) = part.strip_prefix("not:") {
                return known(inner);
            }
            is_known_policy(part)
        }
        known(policy)
    }

    /// c'est le rôle nommé « admin,editor », que personne ne porte.
    #[test]
    fn a_role_policy_is_a_literal_name_not_a_comma_list() {
        let policy = "role:admin,editor";
        let authz = authz_policy(policy);
        for (name, ctx) in [
            ("utilisateur admin", user_ctx(&["admin"])),
            ("utilisateur éditeur", user_ctx(&["editor"])),
            ("utilisateur des deux", user_ctx(&["admin", "editor"])),
        ] {
            let mut used_iam = false;
            assert_eq!(
                authz.evaluate(policy, &ctx, &mut used_iam),
                Some(false),
                "`{policy}` n'est pas une liste de rôles (le framework la lit comme un rôle littéral) : {name} doit être refusé"
            );
        }
    }

    #[test]
    fn a_role_policy_denies_an_admin_context_that_carries_no_role() {
        let authz = authz_policy("role:admin");
        // Un contexte `Admin` n'a pas de rôle : `role:` ne le concerne pas.
        assert!(
            !authz
                .decide(&Method::GET, "/orders", &admin_ctx())
                .is_allow()
        );
        assert!(
            authz
                .decide(&Method::GET, "/orders", &user_ctx(&["admin"]))
                .is_allow()
        );
    }

    // ── 4. Test différentiel étendu vs le parseur du framework ───────────
    //
    // `evaluate` est un pré-passage : pour toute politique SANS `perm:`, il doit
    // rendre exactement ce que rend `AuthPolicy::parse_policy` —
    // sinon il change en silence le sens des politiques déjà déployées.

    #[test]
    fn the_pre_pass_agrees_with_the_framework_on_a_wide_matrix() {
        let authz = authz_policy("");
        let resolvers = AuthResolverRegistry::new();
        let contexts = all_contexts();

        let policies = [
            // vocabulaire simple
            "public",
            "authenticated",
            "owner",
            "admin_only",
            "service_only",
            "owner_or_service",
            // rôles et résolveurs
            "role:admin",
            "role:reader",
            "role:",
            "role:admin,role:editor",
            "owner_or_role:admin",
            "resolver:inconnu",
            "resolver:absent",
            "owner_or_resolver:inconnu",
            // compositions
            "all:authenticated",
            "any:owner",
            // `all:`, `any:`, `not:` **seuls** sont hors vocabulaire après
            // découpage : le pré-passage les refuse explicitement (fail-closed
            // sur le vocabulaire) là où le parseur retombait sur
            // `authenticated`. Cette divergence est VOULUE, elle est testée par
            // `a_policy_word_outside_the_vocabulary_denies_instead_of_authenticating`.
            "all:authenticated,role:admin",
            "any:owner,role:admin",
            "all: owner , role:admin",
            "any:owner,,role:admin",
            "all:any:owner,role:admin,authenticated",
            "any:all:authenticated,owner,admin_only",
            "all:authenticated,any:owner,role:admin",
            // négations
            "not:public",
            "not:authenticated",
            "not:not:public",
            "not:any:public,role:admin",
            "any:public,not:authenticated",
            // hors vocabulaire
            "foobar:x",
            "FOOBAR:x",
            "",
            "   ",
        ];
        assert!(
            policies.len() >= 25,
            "la matrice doit couvrir au moins 25 politiques"
        );

        for policy in policies {
            for (name, ctx) in &contexts {
                let mut used_iam = false;
                let mine = authz.evaluate(policy, ctx, &mut used_iam);
                let theirs = AuthPolicy::parse_policy(policy).check_with_resolver(
                    ctx,
                    Some(&resolvers),
                    None,
                );
                // Le vocabulaire est **clos** : dès qu'une partie n'est pas un
                // mot connu (partie vide d'une virgule double, mot inventé), le
                // pré-passage REFUSE là où `parse_policy` retombait sur
                // `authenticated`. Divergence VOULUE — on l'affirme au lieu de
                // la subir.
                if every_part_is_known(policy) {
                    assert_eq!(
                        mine,
                        Some(theirs),
                        "divergence du pré-passage sur `{policy}` pour {name}"
                    );
                } else {
                    // Une partie hors vocabulaire est refusée **part par part** :
                    // dans une disjonction, une autre branche peut légitimement
                    // accorder. L'invariant qui compte — et le seul qui protège —
                    // est que le pré-passage ne soit **jamais plus permissif** que
                    // le framework.
                    assert!(
                        mine != Some(true) || theirs,
                        "le pré-passage est PLUS PERMISSIF que le framework sur \
                         `{policy}` ({name}) : c'est le fail-open à interdire"
                    );
                }
                assert!(
                    !used_iam,
                    "`{policy}` n'utilise pas `perm:` — l'IAM ne doit pas être consulté"
                );
            }
        }
    }

    /// Le quantificateur de la matrice : elle doit **distinguer** les contextes,
    /// sinon elle serait verte en comparant deux fois la même chose.
    #[test]
    fn the_differential_matrix_actually_discriminates() {
        let authz = authz_policy("");
        let contexts = all_contexts();
        let verdicts_of = |policy: &str| -> Vec<bool> {
            contexts
                .iter()
                .map(|(_, ctx)| {
                    let mut used = false;
                    // `None` (non tranchable) se lit comme un refus : c'est la
                    // seule lecture sûre.
                    authz.evaluate(policy, ctx, &mut used) == Some(true)
                })
                .collect()
        };
        let public = verdicts_of("public");
        let owner = verdicts_of("owner");
        let admin_only = verdicts_of("admin_only");
        assert!(public.iter().all(|v| *v));
        assert_eq!(owner, vec![false, false, false, true, false, false, false]);
        assert_eq!(
            admin_only,
            vec![false, false, false, false, false, false, true]
        );
    }

    // ── 5. Requirement : les huit opérations et l'abstention ─────────────

    fn distinct_policies() -> EntityAuthConfig {
        EntityAuthConfig {
            list: "public".to_string(),
            get: "authenticated".to_string(),
            create: "role:creator".to_string(),
            update: "owner".to_string(),
            delete: "admin_only".to_string(),
            list_links: "service_only".to_string(),
            create_link: "owner_or_service".to_string(),
            delete_link: "any:owner,role:admin".to_string(),
            update_link: "owner_or_service".to_string(),
        }
    }

    #[test]
    fn every_route_shape_renders_its_operation_policy_and_scope() {
        let authz = authz_with(
            config_of(vec![entity("order", "orders", distinct_policies())]),
            &[("order", "orders")],
        );

        let cases = [
            (Method::GET, "/orders", "list", "public"),
            (Method::POST, "/orders", "create", "role:creator"),
            (Method::GET, "/orders/7", "get", "authenticated"),
            (Method::PUT, "/orders/7", "update", "owner"),
            (Method::PATCH, "/orders/7", "update", "owner"),
            (Method::DELETE, "/orders/7", "delete", "admin_only"),
            (
                Method::GET,
                "/orders/7/invoices",
                "list_links",
                "service_only",
            ),
            (
                Method::POST,
                "/orders/7/items/product/9",
                "create_link",
                "owner_or_service",
            ),
            (
                Method::DELETE,
                "/orders/7/items/product/9",
                "delete_link",
                "any:owner,role:admin",
            ),
            // **Correction d'audit** : la forme à QUATRE segments d'écriture
            // (que le routeur de liens sert) n'était rattachée à aucune
            // opération — elle était donc servie sans politique.
            (
                Method::POST,
                "/orders/7/items/9",
                "create_link",
                "owner_or_service",
            ),
            (
                Method::PUT,
                "/orders/7/items/9",
                "update_link",
                "owner_or_service",
            ),
        ];

        let mut seen: Vec<&str> = Vec::new();
        for (method, path, operation, policy) in cases {
            match authz.requirement(&method, path) {
                Requirement::Policy {
                    entity_type,
                    operation: got_operation,
                    policy: got_policy,
                    origin,
                } => {
                    assert_eq!(got_operation, operation, "{method} {path}");
                    assert_eq!(got_policy, policy, "{method} {path}");
                    assert_eq!(origin, PolicyOrigin::Declared, "{method} {path}");
                    // Une route de lien garde l'entité **source** (la doc du
                    // variant annonce `link` : c'est l'implémentation qui fait foi).
                    assert_eq!(entity_type, "order", "{method} {path}");
                    // La portée publiée est exactement la forme résolue.
                    match authz.decide(&method, path, &AuthContext::Anonymous) {
                        Decision::Allow { scope, .. } | Decision::Deny { scope, .. } => {
                            assert_eq!(scope, format!("order · {operation}"), "{method} {path}");
                        }
                        Decision::Abstain { reason } => panic!(
                            "{method} {path} : abstention inattendue sur une forme connue ({reason})"
                        ),
                    }
                    if !seen.contains(&operation) {
                        seen.push(operation);
                    }
                }
                other => panic!("{method} {path} : attendu une politique, obtenu {other:?}"),
            }
        }

        seen.sort_unstable();
        assert_eq!(
            seen,
            [
                "create",
                "create_link",
                "delete",
                "delete_link",
                "get",
                "list",
                "list_links",
                "update",
                "update_link"
            ],
            "les neuf opérations doivent être rendues"
        );
    }

    #[test]
    fn every_abstention_carries_an_actionable_reason() {
        let authz = authz_policy("public");
        let cases: [(Method, &str, Option<&str>); 10] = [
            (Method::GET, "", Some("vide")),
            (Method::GET, "/", Some("vide")),
            (Method::GET, "/health", Some("health")),
            (Method::GET, "/graph/identity", Some("graph")),
            (Method::GET, "/inconnus", Some("inconnus")),
            (Method::PUT, "/orders", Some("PUT")),
            (Method::DELETE, "/orders", Some("DELETE")),
            // `HEAD` n'est PAS ici : il est gouverné comme `GET`, sinon une
            // requête HEAD atteindrait la route par le handler GET sans passer
            // par la politique (voir le test dédié dans `iam_route_resolution`).
            (Method::OPTIONS, "/orders/1", Some("OPTIONS")),
            // **Correction d'audit** : ces deux formes étaient listées ici comme
            // abstentions. Le routeur les SERT (repli des liens, et
            // `/{src}/{id}/{route}/{cible}`) : elles sont maintenant jugées —
            // `list_links` pour toute lecture profonde, `create_link` pour une
            // écriture à quatre segments. Elles ont migré vers le test de
            // couverture, et il ne reste ici que de vraies abstentions.
            (Method::PATCH, "/orders/1/2/3/4/5", Some("6 segments")),
            // 4 segments en `PUT` est désormais jugé (`update_link`) : il ne
            // reste ici qu'une forme que le routeur NE sert PAS.
            (Method::PUT, "/orders/1/2", Some("3 segments")),
        ];

        for (method, path, fragment) in cases {
            match authz.requirement(&method, path) {
                Requirement::Abstain { reason } => {
                    assert!(
                        !reason.trim().is_empty(),
                        "{method} {path} : une abstention doit être explicable"
                    );
                    if let Some(fragment) = fragment {
                        assert!(
                            reason.contains(fragment),
                            "{method} {path} : la raison `{reason}` devrait nommer `{fragment}`"
                        );
                    }
                    match authz.decide(&method, path, &AuthContext::Anonymous) {
                        Decision::Abstain { reason: carried } => assert_eq!(
                            carried, reason,
                            "{method} {path} : la décision doit reprendre la raison telle quelle"
                        ),
                        other => {
                            panic!("{method} {path} : attendu une abstention, obtenu {other:?}")
                        }
                    }
                }
                other => panic!("{method} {path} : attendu une abstention, obtenu {other:?}"),
            }
        }
    }

    /// **Correction d'audit** : `POST /{entité}/{id}/{cible}` **crée une entité
    /// liée**. La forme n'était rattachée à aucune opération : la route créait
    /// donc sans qu'aucune politique ne soit consultée. Elle est jugée par la
    /// politique de création de la **cible** — c'est une création d'invoice, pas
    /// une opération sur l'order qui la porte.
    #[test]
    fn creating_a_linked_entity_is_judged_by_the_target_entity_policy() {
        let invoice = EntityAuthConfig {
            create: "admin_only".to_string(),
            ..Default::default()
        };
        let authz = authz_with(
            config_of(vec![
                entity("order", "orders", EntityAuthConfig::default()),
                entity("invoice", "invoices", invoice),
            ]),
            &[("order", "orders"), ("invoice", "invoices")],
        );

        match authz.requirement(&Method::POST, "/orders/7/invoices") {
            Requirement::Policy {
                entity_type,
                operation,
                policy,
                ..
            } => {
                assert_eq!(
                    entity_type, "invoice",
                    "la portée est la CIBLE, pas la source"
                );
                assert_eq!(operation, "create");
                assert_eq!(policy, "admin_only");
            }
            other => panic!("la création d'une entité liée doit être jugée : {other:?}"),
        }

        // Et l'application effective le fait : un anonyme ne crée pas.
        assert!(
            !authz
                .decide(&Method::POST, "/orders/7/invoices", &AuthContext::Anonymous)
                .is_allow()
        );

        // Une cible NON enregistrée reste non tranchable : pas de politique
        // inventée pour un type que le serveur ne connaît pas.
        assert!(matches!(
            authz.requirement(&Method::POST, "/orders/7/inconnues"),
            Requirement::Abstain { .. }
        ));
    }

    // ── 6. Provenance : Declared vs Default ──────────────────────────────

    /// La provenance dépend de la **présence** d'une entrée de configuration,
    /// jamais de la valeur de la politique : une entité configurée dont la
    /// politique vaut le défaut du framework reste `Declared`.
    #[test]
    fn the_origin_follows_the_presence_of_a_config_entry_not_the_value() {
        let configured = authz_with(
            config_of(vec![entity("order", "orders", EntityAuthConfig::default())]),
            &[("order", "orders")],
        );
        let unconfigured = authz_with(config_of(Vec::new()), &[("order", "orders")]);

        for (authz, expected) in [
            (&configured, PolicyOrigin::Declared),
            (&unconfigured, PolicyOrigin::Default),
        ] {
            match authz.requirement(&Method::GET, "/orders") {
                Requirement::Policy { policy, origin, .. } => {
                    assert_eq!(policy, "authenticated");
                    assert_eq!(origin, expected);
                }
                other => panic!("attendu une politique, obtenu {other:?}"),
            }
        }

        // Le défaut s'applique aux huit opérations, pas seulement à `list`.
        let cases = [
            (Method::POST, "/orders"),
            (Method::GET, "/orders/1"),
            (Method::PUT, "/orders/1"),
            (Method::PATCH, "/orders/1"),
            (Method::DELETE, "/orders/1"),
            (Method::GET, "/orders/1/invoices"),
            (Method::POST, "/orders/1/items/product/2"),
            (Method::DELETE, "/orders/1/items/product/2"),
        ];
        for (method, path) in cases {
            match unconfigured.requirement(&method, path) {
                Requirement::Policy { policy, origin, .. } => {
                    assert_eq!(origin, PolicyOrigin::Default, "{method} {path}");
                    assert_eq!(policy, "authenticated", "{method} {path}");
                }
                other => panic!("{method} {path} : attendu une politique, obtenu {other:?}"),
            }
        }

        // …et pour un contexte anonyme le défaut refuse, en portant la provenance.
        match unconfigured.decide(&Method::GET, "/orders", &AuthContext::Anonymous) {
            Decision::Deny { origin, policy, .. } => {
                assert_eq!(origin, PolicyOrigin::Default);
                assert_eq!(policy, "authenticated");
            }
            other => panic!("attendu un refus, obtenu {other:?}"),
        }
    }

    /// Le registre d'entités est l'autorité sur les **routes** : la
    /// configuration seule ne fabrique pas de politique.
    #[test]
    fn the_registry_is_the_authority_on_routes_not_the_config() {
        // Déclarée en configuration, absente du registre : aucune route, donc
        // abstention — la configuration ne suffit pas à s'arroger une politique.
        let authz = authz_with(
            config_of(vec![
                entity("order", "orders", policy_everywhere("public")),
                entity("invoice", "invoices", policy_everywhere("admin_only")),
            ]),
            &[("order", "orders")],
        );
        assert!(matches!(
            authz.requirement(&Method::GET, "/invoices"),
            Requirement::Abstain { .. }
        ));

        // Le pluriel retenu est celui du **registre** : `EntityConfig::plural`
        // diverge ici volontairement.
        let mismatched = authz_with(
            config_of(vec![entity(
                "order",
                "commandes",
                policy_everywhere("public"),
            )]),
            &[("order", "orders")],
        );
        assert!(matches!(
            mismatched.requirement(&Method::GET, "/commandes"),
            Requirement::Abstain { .. }
        ));
        match mismatched.requirement(&Method::GET, "/orders") {
            Requirement::Policy { policy, origin, .. } => {
                assert_eq!(policy, "public");
                assert_eq!(
                    origin,
                    PolicyOrigin::Declared,
                    "la configuration est retrouvée par le SINGULIER de l'entité"
                );
            }
            other => panic!("attendu une politique, obtenu {other:?}"),
        }
    }

    // ── 7. `describe` : lisible, sans fuite d'identifiant ────────────────

    #[test]
    fn every_context_is_described_without_leaking_its_identifiers() {
        let cases = [
            (AuthContext::Anonymous, "anonyme"),
            (user_ctx(&[]), "utilisateur authentifié"),
            (owner_ctx(), "propriétaire de la ressource"),
            (service_ctx(None), "service"),
            (admin_ctx(), "administrateur"),
        ];
        for (ctx, expected) in &cases {
            let got = describe(ctx);
            assert_eq!(got, *expected);
            assert!(!got.is_empty());
        }

        // La raison d'un refus porte la description du contexte, jamais son id.
        let authz = authz_policy("owner");
        match authz.decide(&Method::GET, "/orders", &user_ctx(&[])) {
            Decision::Deny { reason, .. } => {
                assert!(
                    reason.contains("utilisateur authentifié"),
                    "le refus doit dire quel contexte a échoué : {reason}"
                );
                assert!(
                    !reason.contains(&uid(USER).to_string())
                        && !reason.contains(&uid(TENANT).to_string()),
                    "la raison ne doit pas divulguer d'identifiant : {reason}"
                );
            }
            other => panic!("attendu un refus, obtenu {other:?}"),
        }
    }

    // ── 8. Sérialisation publiée ─────────────────────────────────────────

    #[test]
    fn a_decision_serialises_under_its_verdict_discriminant() {
        let allow = Decision::Allow {
            scope: "order · list".to_string(),
            policy: "public".to_string(),
            origin: PolicyOrigin::Declared,
        };
        let value = serde_json::to_value(&allow).expect("sérialisation");
        assert_eq!(value["verdict"].as_str(), Some("allow"));
        assert_eq!(value["origin"].as_str(), Some("declared"));
        assert_eq!(value["scope"].as_str(), Some("order · list"));
        assert_eq!(value["policy"].as_str(), Some("public"));
        assert!(value.get("reason").is_none());

        let deny = Decision::Deny {
            scope: "order · delete".to_string(),
            policy: "role:admin".to_string(),
            origin: PolicyOrigin::Default,
            reason: "le contexte (anonyme) ne satisfait pas la politique".to_string(),
        };
        let value = serde_json::to_value(&deny).expect("sérialisation");
        assert_eq!(value["verdict"].as_str(), Some("deny"));
        assert_eq!(value["origin"].as_str(), Some("default"));
        assert!(matches!(value["reason"].as_str(), Some(r) if !r.trim().is_empty()));

        let abstain = Decision::Abstain {
            reason: "chemin vide".to_string(),
        };
        let value = serde_json::to_value(&abstain).expect("sérialisation");
        assert_eq!(value["verdict"].as_str(), Some("abstain"));
        assert_eq!(value["reason"].as_str(), Some("chemin vide"));
        assert!(
            value.get("scope").is_none() && value.get("policy").is_none(),
            "une abstention ne porte ni portée ni politique"
        );

        // Les trois provenances ont trois jetons distincts — un tableau de bord
        // qui les agrège en dépend.
        assert_eq!(
            serde_json::to_value(PolicyOrigin::Declared).unwrap(),
            json!("declared")
        );
        assert_eq!(
            serde_json::to_value(PolicyOrigin::Default).unwrap(),
            json!("default")
        );
        assert_eq!(
            serde_json::to_value(PolicyOrigin::Iam).unwrap(),
            json!("iam")
        );
    }

    #[test]
    fn a_requirement_serialises_under_its_kind_discriminant() {
        let policy = Requirement::Policy {
            entity_type: "order".to_string(),
            operation: "list_links".to_string(),
            policy: "service_only".to_string(),
            origin: PolicyOrigin::Iam,
        };
        let value = serde_json::to_value(&policy).expect("sérialisation");
        assert_eq!(value["kind"].as_str(), Some("policy"));
        assert_eq!(value["origin"].as_str(), Some("iam"));
        assert_eq!(value["entity_type"].as_str(), Some("order"));
        assert_eq!(value["operation"].as_str(), Some("list_links"));
        assert_eq!(value["policy"].as_str(), Some("service_only"));

        let abstain = Requirement::Abstain {
            reason: "`inconnus` n'est le pluriel d'aucune entité enregistrée".to_string(),
        };
        let value = serde_json::to_value(&abstain).expect("sérialisation");
        assert_eq!(value["kind"].as_str(), Some("abstain"));
        assert!(matches!(value["reason"].as_str(), Some(r) if !r.is_empty()));
        assert!(
            value.get("policy").is_none(),
            "une abstention ne publie pas de politique"
        );
    }

    /// `is_allow` n'est vrai que pour `Allow` — une abstention n'est **pas** une
    /// autorisation : hors chemins déclarés publics, le middleware la refuse
    /// (voir `tests/abstention_refusee.rs`).
    #[test]
    fn is_allow_is_true_only_for_allow() {
        assert!(
            Decision::Allow {
                scope: "order · list".into(),
                policy: "public".into(),
                origin: PolicyOrigin::Declared,
            }
            .is_allow()
        );
        assert!(
            !Decision::Deny {
                scope: "order · list".into(),
                policy: "public".into(),
                origin: PolicyOrigin::Declared,
                reason: "non".into(),
            }
            .is_allow()
        );
        assert!(
            !Decision::Abstain {
                reason: "route inconnue".into()
            }
            .is_allow(),
            "l'abstention n'est pas une autorisation"
        );
    }

    // ── 9. L'IAM matérialisé (feature `obrain`) ──────────────────────────

    #[cfg(feature = "obrain")]
    mod iam_plan {
        use super::*;
        use crate::storage::ObrainStore;
        use crate::storage::iam::RoleGrant;

        fn store() -> Arc<ObrainStore> {
            ObrainStore::new_in_memory()
        }

        fn authz_with_plan(store: &Arc<ObrainStore>, policy: &str) -> Authorization {
            authz_policy(policy).with_iam(store)
        }

        fn materialize(store: &Arc<ObrainStore>, principal: &str, scope: &str, perms: &[&str]) {
            let grants = [RoleGrant {
                role: "r".to_string(),
                permissions: perms.iter().map(|p| p.to_string()).collect(),
            }];
            store
                .iam()
                .materialize(principal, scope, &grants)
                .expect("matérialisation");
        }

        /// **(principal, portée)** — c'est la clé de lecture de la décision, donc
        /// une erreur ici est une erreur de cloisonnement.
        #[test]
        fn the_identity_of_each_context_is_its_principal_and_its_scope() {
            let cases: [(AuthContext, Option<(String, String)>); 6] = [
                (AuthContext::Anonymous, None),
                (
                    user_ctx(&[]),
                    Some((uid(USER).to_string(), uid(TENANT).to_string())),
                ),
                (
                    owner_ctx(),
                    Some((uid(USER).to_string(), uid(TENANT).to_string())),
                ),
                (
                    service_ctx(Some(uid(TENANT))),
                    Some(("svc".to_string(), uid(TENANT).to_string())),
                ),
                (
                    service_ctx(None),
                    Some(("svc".to_string(), "global".to_string())),
                ),
                (
                    admin_ctx(),
                    Some((uid(ADMIN).to_string(), "global".to_string())),
                ),
            ];
            for (ctx, expected) in cases {
                assert_eq!(
                    Authorization::default_iam_identity(&ctx),
                    expected,
                    "identité IAM de {}",
                    describe(&ctx)
                );
            }
        }

        /// Une décision matérialisée répond à `perm:` pour **chaque** sorte de
        /// principal, dans exactement la portée que le contexte désigne.
        #[test]
        fn a_materialised_decision_answers_every_principal_kind_in_its_scope() {
            // Un store par cas : `User` et `Owner` désignent le MÊME couple
            // (principal, portée), donc un store partagé confondrait les deux.
            let cases: Vec<(&str, AuthContext, String, String)> = vec![
                (
                    "utilisateur",
                    user_ctx(&[]),
                    uid(USER).to_string(),
                    uid(TENANT).to_string(),
                ),
                (
                    "propriétaire",
                    owner_ctx(),
                    uid(USER).to_string(),
                    uid(TENANT).to_string(),
                ),
                (
                    "service en locataire",
                    service_ctx(Some(uid(TENANT))),
                    "svc".to_string(),
                    uid(TENANT).to_string(),
                ),
                (
                    "service global",
                    service_ctx(None),
                    "svc".to_string(),
                    "global".to_string(),
                ),
                (
                    "administrateur",
                    admin_ctx(),
                    uid(ADMIN).to_string(),
                    "global".to_string(),
                ),
            ];

            for (name, ctx, principal, scope) in &cases {
                let store = store();
                let authz = authz_with_plan(&store, "perm:orders:read");
                let before = authz.decide(&Method::GET, "/orders", ctx);
                assert!(
                    !before.is_allow(),
                    "{name} : sans décision matérialisée, refus attendu, obtenu {before:?}"
                );
                materialize(&store, principal, scope, &["orders:read"]);
                match authz.decide(&Method::GET, "/orders", ctx) {
                    Decision::Allow { origin, .. } => {
                        assert_eq!(origin, PolicyOrigin::Iam, "{name}");
                    }
                    other => panic!("{name} : autorisation attendue, obtenue {other:?}"),
                }
            }
        }

        /// Le cloisonnement : la décision d'un principal ne vaut pas pour un
        /// autre, même dans le même locataire.
        #[test]
        fn a_decision_for_one_principal_does_not_authorise_another() {
            let store = store();
            materialize(
                &store,
                &uid(USER).to_string(),
                &uid(TENANT).to_string(),
                &["x"],
            );
            let authz = authz_with_plan(&store, "perm:x");

            assert!(
                authz
                    .decide(&Method::GET, "/orders", &user_ctx(&[]))
                    .is_allow()
            );
            let neighbour = AuthContext::User {
                user_id: uid(OTHER_USER),
                tenant_id: uid(TENANT),
                roles: vec![],
            };
            assert!(
                !authz.decide(&Method::GET, "/orders", &neighbour).is_allow(),
                "aucune décision pour cet utilisateur : refus"
            );
            // et un autre locataire non plus (même principal)
            let other_tenant = AuthContext::User {
                user_id: uid(USER),
                tenant_id: uid(OTHER_USER),
                roles: vec![],
            };
            assert!(
                !authz
                    .decide(&Method::GET, "/orders", &other_tenant)
                    .is_allow(),
                "la portée isole les locataires"
            );
        }

        /// Une décision **absente** refuse ; une décision **invalidée** refuse
        /// aussi, mais l'époque continue d'avancer (rematérialisation sur le même
        /// nœud) — c'est ce qui rend un cache sûr.
        #[test]
        fn an_absent_decision_denies_and_an_invalidated_one_keeps_the_epoch_moving() {
            let store = store();
            let principal = uid(USER).to_string();
            let scope = uid(TENANT).to_string();

            // absente
            assert!(store.iam().decision(&principal, &scope).unwrap().is_none());
            let authz = authz_with_plan(&store, "perm:orders:write");
            assert!(
                !authz
                    .decide(&Method::DELETE, "/orders/1", &user_ctx(&[]))
                    .is_allow(),
                "aucune décision matérialisée : refus"
            );

            // présente
            materialize(&store, &principal, &scope, &["orders:write"]);
            assert!(
                authz
                    .decide(&Method::DELETE, "/orders/1", &user_ctx(&[]))
                    .is_allow()
            );
            let e1 = store
                .iam()
                .decision(&principal, &scope)
                .unwrap()
                .expect("décision")
                .epoch;

            // invalidée
            let e2 = store
                .iam()
                .invalidate(&principal, &scope)
                .expect("invalidation");
            assert_eq!(e2, e1 + 1, "l'époque est monotone");
            let decision = authz.decide(&Method::DELETE, "/orders/1", &user_ctx(&[]));
            assert!(
                !decision.is_allow(),
                "une décision invalidée ne doit plus autoriser : {decision:?}"
            );
            assert!(
                matches!(
                    decision,
                    Decision::Deny {
                        origin: PolicyOrigin::Iam,
                        ..
                    }
                ),
                "le nœud de décision existe toujours, et c'est bien l'IAM qui a tranché"
            );

            // rematérialisée : même nœud, époque strictement croissante
            materialize(&store, &principal, &scope, &["orders:write"]);
            let e3 = store
                .iam()
                .decision(&principal, &scope)
                .unwrap()
                .expect("décision")
                .epoch;
            assert_eq!(e3, e2 + 1, "l'époque ne repart pas à 1");
            assert!(
                authz
                    .decide(&Method::DELETE, "/orders/1", &user_ctx(&[]))
                    .is_allow()
            );
        }

        /// La permission est comparée **exactement** ; seul `*` couvre le reste.
        #[test]
        fn a_permission_is_matched_exactly_and_only_a_star_covers_the_rest() {
            let store = store();
            let principal = uid(USER).to_string();
            let scope = uid(TENANT).to_string();
            materialize(&store, &principal, &scope, &["orders:read"]);

            let allowed = |permission: &str| -> bool {
                let authz = authz_with_plan(&store, &format!("perm:{permission}"));
                authz
                    .decide(&Method::GET, "/orders", &user_ctx(&[]))
                    .is_allow()
            };
            assert!(allowed("orders:read"));
            assert!(!allowed("orders:write"), "permission non accordée");
            assert!(!allowed("orders:rea"), "aucune correspondance partielle");
            assert!(!allowed("orders:"), "un préfixe n'est pas un joker");
            assert!(
                !allowed("ORDERS:READ"),
                "la comparaison est sensible à la casse"
            );
            assert!(!allowed("*"), "demander `*` n'est pas détenir `*`");

            // Le joker accordé couvre toute permission demandée.
            materialize(&store, &principal, &scope, &["*"]);
            assert!(allowed("orders:write"));
            assert!(allowed("n'importe:quoi"));
        }

        /// `perm:` composé : `all:` (ET), `any:` (OU), et `not:` (liste de refus)
        /// quand le plan **est** consultable.
        #[test]
        fn perm_composes_with_all_any_and_not() {
            let store = store();
            materialize(
                &store,
                &uid(USER).to_string(),
                &uid(TENANT).to_string(),
                &["x"],
            );

            // all: — les deux conditions sont nécessaires
            let all = authz_with_plan(&store, "all:perm:x,authenticated");
            assert!(
                all.decide(&Method::GET, "/orders", &user_ctx(&[]))
                    .is_allow()
            );
            assert!(
                !all.decide(&Method::GET, "/orders", &AuthContext::Anonymous)
                    .is_allow(),
                "un anonyme n'a pas de décision matérialisée"
            );
            let all_with_role = authz_with_plan(&store, "all:perm:x,role:admin");
            assert!(
                !all_with_role
                    .decide(&Method::GET, "/orders", &user_ctx(&[]))
                    .is_allow(),
                "le rôle manque"
            );

            // any: — une seule branche suffit
            let any = authz_with_plan(&store, "any:perm:x,role:auditor");
            assert!(
                any.decide(&Method::GET, "/orders", &user_ctx(&[]))
                    .is_allow()
            );
            assert!(
                any.decide(&Method::GET, "/orders", &user_ctx(&["auditor"]))
                    .is_allow()
            );
            let any_missing = authz_with_plan(&store, "any:perm:y,role:auditor");
            assert!(
                !any_missing
                    .decide(&Method::GET, "/orders", &user_ctx(&[]))
                    .is_allow(),
                "ni la permission ni le rôle : refus"
            );

            // not: — refus de qui détient la permission, et c'est le sens d'une
            // liste de refus : qui ne la détient pas passe.
            let deny_list = authz_with_plan(&store, "not:perm:x");
            assert!(
                !deny_list
                    .decide(&Method::GET, "/orders", &user_ctx(&[]))
                    .is_allow(),
                "le porteur de la permission est refusé"
            );
            let neighbour = AuthContext::User {
                user_id: uid(OTHER_USER),
                tenant_id: uid(TENANT),
                roles: vec![],
            };
            match deny_list.decide(&Method::GET, "/orders", &neighbour) {
                Decision::Allow { origin, .. } => assert_eq!(
                    origin,
                    PolicyOrigin::Iam,
                    "l'IAM a été consulté (et n'a rien trouvé pour ce principal)"
                ),
                other => panic!("attendu une autorisation, obtenue {other:?}"),
            }
            // Le cas du contexte SANS principal (anonyme) est un défaut : voir
            // `a_deny_list_over_the_plan_does_not_absolve_a_context_without_a_principal`.
        }

        /// Une permission vide (`perm:`) est une configuration fautive : elle
        /// refuse quand la décision est finie.
        #[test]
        fn an_empty_permission_policy_denies_a_finite_decision() {
            let store = store();
            materialize(
                &store,
                &uid(USER).to_string(),
                &uid(TENANT).to_string(),
                &["orders:read", "orders:write"],
            );
            let authz = authz_with_plan(&store, "perm:");
            let decision = authz.decide(&Method::GET, "/orders", &user_ctx(&[]));
            assert!(
                !decision.is_allow(),
                "`perm:` (permission vide) doit refuser : {decision:?}"
            );
        }

        /// **Corrigé** : le joker ne couvre **plus** la permission vide —
        /// `AclStamp::allows` refuse un nom vide, donc un `perm:` sans nom
        /// (faute de frappe) refuse même quand la décision porte `*`.
        #[test]
        fn an_empty_permission_policy_is_not_authorised_by_a_wildcard_decision() {
            let store = store();
            materialize(
                &store,
                &uid(USER).to_string(),
                &uid(TENANT).to_string(),
                &["*"],
            );
            let authz = authz_with_plan(&store, "perm:");
            let decision = authz.decide(&Method::GET, "/orders", &user_ctx(&[]));
            assert!(
                !decision.is_allow(),
                "`perm:` sans permission demandée ne doit pas être approuvé : {decision:?}"
            );
        }

        /// **Corrigé** : une politique `perm:` **non tranchable** (ici : contexte
        /// sans principal) ne peut plus être retournée en autorisation par une
        /// négation. L'évaluation est tri-état : `None` (non tranchable) reste
        /// `None` sous `not:`, et `None` refuse.
        #[test]
        fn a_deny_list_over_the_plan_does_not_absolve_a_context_without_a_principal() {
            let store = store();
            materialize(
                &store,
                &uid(USER).to_string(),
                &uid(TENANT).to_string(),
                &["x"],
            );
            let deny_list = authz_with_plan(&store, "not:perm:x");
            let decision = deny_list.decide(&Method::GET, "/orders", &AuthContext::Anonymous);
            assert!(
                !decision.is_allow(),
                "`not:perm:` ne doit pas absoudre un contexte sans principal : {decision:?}"
            );
        }

        /// La provenance suit la **consultation réelle**, pas la présence du
        /// préfixe : `any:`/`all:` court-circuitent selon l'ordre des branches.
        #[test]
        fn the_iam_origin_follows_the_actual_consultation_of_the_plan() {
            let store = store();
            materialize(
                &store,
                &uid(USER).to_string(),
                &uid(TENANT).to_string(),
                &["x"],
            );

            // `perm:` en tête : consulté, et il refuse (l'utilisateur n'a pas `y`)
            // — puis le rôle sauve. La provenance doit dire `iam`.
            let role_after = authz_with_plan(&store, "any:perm:y,role:auditor");
            match role_after.decide(&Method::GET, "/orders", &user_ctx(&["auditor"])) {
                Decision::Allow { origin, .. } => assert_eq!(origin, PolicyOrigin::Iam),
                other => panic!("le rôle `auditor` doit sauver : {other:?}"),
            }

            // `role:` en tête et satisfait : `any` court-circuite, l'IAM n'est
            // jamais consulté — la provenance reste celle de la déclaration.
            let role_first = authz_with_plan(&store, "any:role:auditor,perm:x");
            match role_first.decide(&Method::GET, "/orders", &user_ctx(&["auditor"])) {
                Decision::Allow { origin, .. } => assert_eq!(
                    origin,
                    PolicyOrigin::Declared,
                    "l'IAM n'a pas été consulté : la provenance ne doit pas le prétendre"
                ),
                other => panic!("attendu une autorisation, obtenue {other:?}"),
            }
            // …et le même `any:` consulte bien l'IAM pour qui n'a pas le rôle.
            match role_first.decide(&Method::GET, "/orders", &user_ctx(&[])) {
                Decision::Allow { origin, .. } => assert_eq!(origin, PolicyOrigin::Iam),
                other => panic!("la décision matérialisée doit sauver : {other:?}"),
            }
        }

        /// **Corrigé** : `role:admin` est du vocabulaire, et un utilisateur qui
        /// porte le rôle le satisfait — sans consulter l'IAM (la provenance reste
        /// celle de la déclaration).
        #[test]
        fn a_role_policy_is_satisfied_without_consulting_the_iam() {
            let store = store();
            materialize(
                &store,
                &uid(USER).to_string(),
                &uid(TENANT).to_string(),
                &["x"],
            );
            let authz = authz_with_plan(&store, "role:admin");
            match authz.decide(&Method::GET, "/orders", &user_ctx(&["admin"])) {
                Decision::Allow { origin, policy, .. } => {
                    assert_eq!(origin, PolicyOrigin::Declared);
                    assert_eq!(policy, "role:admin");
                }
                other => panic!("attendu une autorisation, obtenue {other:?}"),
            }
        }
    }
}

#[cfg(test)]
mod descriptor_tests {
    //! REST reads the descriptor's permissions with the precedence of every other
    //! surface (`core::auth::surface::PolicyIndex`).
    use super::*;
    use crate::config::EntityConfig;
    use crate::core::link::{LinkAuthConfig, LinkDefinition};
    use crate::descriptor::{
        DescriptorSet, EntityDesc, LinkDesc, ModuleDescriptor, Operation, PolicyExpr,
    };
    use crate::server::host::ServerHost;
    use crate::storage::InMemoryLinkService;
    use std::collections::HashMap;

    struct Stub(&'static str, &'static str);
    impl crate::server::entity_registry::EntityDescriptor for Stub {
        fn entity_type(&self) -> &str {
            self.0
        }
        fn plural(&self) -> &str {
            self.1
        }
        fn build_routes(&self) -> axum::Router {
            axum::Router::new()
        }
    }

    fn user(roles: &[&str]) -> AuthContext {
        AuthContext::User {
            user_id: uuid::Uuid::from_u128(1),
            tenant_id: uuid::Uuid::from_u128(2),
            roles: roles.iter().map(|r| r.to_string()).collect(),
        }
    }

    /// YAML: `order` all `authenticated`; links `has_invoice` (YAML `create` =
    /// `role:clerk`) and `has_note` (no YAML auth). Descriptor: `order.create` =
    /// `role:admin`, `order.update` = `all(authenticated, not(role:banned))`,
    /// `order.list_links` = `role:auditor`, `has_invoice.list_links` = `role:billing`.
    fn authz() -> Authorization {
        let link =
            |lt: &str, target: &str, route: &str, auth: Option<LinkAuthConfig>| LinkDefinition {
                link_type: lt.into(),
                source_type: "order".into(),
                target_type: target.into(),
                forward_route_name: route.into(),
                reverse_route_name: "order".into(),
                description: None,
                required_fields: None,
                auth,
            };
        let config = LinksConfig {
            entities: vec![EntityConfig {
                singular: "order".into(),
                plural: "orders".into(),
                auth: EntityAuthConfig::default(),
            }],
            links: vec![
                link(
                    "has_invoice",
                    "invoice",
                    "invoices",
                    Some(LinkAuthConfig {
                        create: "role:clerk".into(),
                        ..Default::default()
                    }),
                ),
                link("has_note", "note", "notes", None),
            ],
            validation_rules: None,
            events: None,
            sinks: None,
            auth: None,
        };
        let mut m = ModuleDescriptor::new("shop", "1.0.0");
        let mut order = EntityDesc::new("order");
        let ops = &mut order.permissions.ops;
        ops.insert(Operation::Create, PolicyExpr::HasRole(vec!["admin".into()]));
        ops.insert(
            Operation::Update,
            PolicyExpr::parse("all(authenticated, not(role:banned))").unwrap(),
        );
        ops.insert(
            Operation::ListLinks,
            PolicyExpr::HasRole(vec!["auditor".into()]),
        );
        m.entities.push(order);
        let mut l = LinkDesc::new("has_invoice", "order", "invoice");
        l.permissions.ops.insert(
            Operation::ListLinks,
            PolicyExpr::HasRole(vec!["billing".into()]),
        );
        m.links.push(l);
        let mut set = DescriptorSet::new();
        set.insert(m).unwrap();

        let mut registry = EntityRegistry::new();
        registry.register(Box::new(Stub("order", "orders")));
        let host = ServerHost::from_builder_components(
            Arc::new(InMemoryLinkService::new()),
            config,
            registry,
            HashMap::new(),
            HashMap::new(),
        )
        .unwrap()
        .with_descriptors(set);
        Authorization::new(
            host.config.clone(),
            &host.entity_registry,
            host.resolver_registry.clone(),
        )
        .with_catalog(host.catalog.clone())
    }

    fn allowed(a: &Authorization, m: Method, path: &str, ctx: &AuthContext) -> bool {
        matches!(a.decide(&m, path, ctx), Decision::Allow { .. })
    }

    fn origin(a: &Authorization, m: Method, path: &str) -> PolicyOrigin {
        match a.requirement(&m, path) {
            Requirement::Policy { origin, .. } => origin,
            other => panic!("{path}: {other:?}"),
        }
    }

    #[test]
    fn rest_applies_descriptor_permissions() {
        let a = authz();
        let id = uuid::Uuid::nil();
        // Entity: descriptor explicit beats the YAML `authenticated`.
        assert_eq!(
            origin(&a, Method::POST, "/orders"),
            PolicyOrigin::Descriptor
        );
        assert!(!allowed(&a, Method::POST, "/orders", &user(&[])));
        assert!(allowed(&a, Method::POST, "/orders", &user(&["admin"])));
        // Function form: evaluated by the engine, not refused as "unknown vocabulary".
        let one = format!("/orders/{id}");
        assert!(allowed(&a, Method::PUT, &one, &user(&[])));
        assert!(!allowed(&a, Method::PUT, &one, &user(&["banned"])));
        assert!(!allowed(&a, Method::PUT, &one, &AuthContext::Anonymous));
        // Not declared in the descriptor: YAML decides, as before.
        assert_eq!(origin(&a, Method::GET, "/orders"), PolicyOrigin::Declared);
    }

    #[test]
    fn link_policy_precedence_matches_rest() {
        let a = authz();
        let id = uuid::Uuid::nil();
        let t = uuid::Uuid::from_u128(5);
        // 1. LinkDesc explicit (`has_invoice.list_links` = billing) beats the entity's
        //    descriptor (`order.list_links` = auditor).
        let invoices = format!("/orders/{id}/invoices");
        assert_eq!(origin(&a, Method::GET, &invoices), PolicyOrigin::Descriptor);
        assert!(allowed(&a, Method::GET, &invoices, &user(&["billing"])));
        assert!(!allowed(&a, Method::GET, &invoices, &user(&["auditor"])));
        // 2. YAML link table (`has_invoice.create` = clerk) beats the entity descriptor.
        let create = format!("/orders/{id}/invoices/invoice/{t}");
        assert_eq!(origin(&a, Method::POST, &create), PolicyOrigin::Declared);
        assert!(allowed(&a, Method::POST, &create, &user(&["clerk"])));
        assert!(!allowed(&a, Method::POST, &create, &user(&["admin"])));
        // 3. Neither on the link: the entity descriptor (`order.list_links` = auditor).
        let notes = format!("/orders/{id}/notes");
        assert_eq!(origin(&a, Method::GET, &notes), PolicyOrigin::Descriptor);
        assert!(allowed(&a, Method::GET, &notes, &user(&["auditor"])));
        assert!(!allowed(&a, Method::GET, &notes, &user(&["billing"])));
        // Same resolution as the non-REST guard.
        let index = crate::core::auth::surface::PolicyIndex::new(
            a.catalog.as_ref().unwrap().load().descriptors.clone(),
            a.catalog.as_ref().unwrap().load().links_config.clone(),
        );
        let p = index.link_policies(
            Some("has_note"),
            Some("order"),
            Some("note"),
            Some("order"),
            &Operation::ListLinks,
        );
        assert_eq!(p[0].policy, Ok(PolicyExpr::HasRole(vec!["auditor".into()])));
    }
}
