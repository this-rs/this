//! # Hôte de déploiement piloté par configuration
//!
//! **Une structure qu'on déploie, et sur laquelle on rattache des projets qui
//! existent déjà** — sans les réécrire, et sans écrire de Rust pour les
//! rattacher.
//!
//! Là où une application écrit du Rust (`ServerBuilder`, `Module`,
//! `EntityDescriptor`), l'hôte lit un **fichier de déploiement** et monte ce
//! qu'il décrit :
//!
//! ```yaml
//! bind: 127.0.0.1:8080
//!
//! attachments:
//!   - name: facturation
//!     store:
//!       kind: obrain
//!       path: /srv/facturation/data/app.obrain
//!       read_only: true          # défaut : on ne touche pas à une base en service
//!     exposure: /srv/facturation/config/server.yaml
//!     policies: /srv/facturation/config/links.yaml
//!     authorization:
//!       mode: report_only        # on observe avant d'armer
//!     identity:
//!       principal: "account:{user_id}"
//!       scope: "org:{tenant_id}"
//! ```
//!
//! # Ce que l'hôte publie, et ce qu'il ne fait pas
//!
//! `GET /` rend le **plan** de la structure : ce qui est rattaché, où, sur quoi,
//! sous quel régime d'autorisation — et ce qui **n'est pas** rattaché. Le plan
//! ne contient **aucun secret** (jamais le contenu d'une politique, d'une clé ou
//! d'un jeton).
//!
//! Trois choses sont volontairement **absentes**, et le plan les nomme :
//!
//! 1. **Pas de reverse proxy.** `config/server.yaml → proxy.caddy` *décrit* le
//!    proxy et rend un `Caddyfile` que l'exploitant applique ; l'hôte ne
//!    proxifie pas vers un projet existant. C'est le sens de « rattacher » ici :
//!    on rattache une **base** et une **identité**, pas du trafic.
//! 2. **Pas de routes CRUD d'entités sans Rust.** Elles exigent un
//!    [`EntityDescriptor`](crate::server::EntityDescriptor) (donc un service de
//!    données) ; l'hôte n'en invente pas depuis la configuration.
//! 3. **Pas d'ouverture de store dans la bibliothèque.** Le framework ne câble
//!    jamais un store depuis un chemin de configuration : ouvrir une base est
//!    une décision d'application. **L'hôte EST cette application** — il assume
//!    la décision, et la publie dans son plan.
//!
//! # Fail-closed, jusque dans la configuration
//!
//! Une configuration est refusée **au chargement**, pas à la première requête :
//! `kind` de store inconnu, `mount` sans `/`, deux attachements sur le même
//! `mount`, fichier d'exposition ou de politiques illisible, placeholders
//! d'identité inconnus. Et `read_only` vaut **`true` par défaut** : rattacher
//! une base en service ne l'ouvre pas en écriture sans qu'on l'ait écrit.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result, bail};
use axum::Router;
use axum::routing::get;
use serde::{Deserialize, Serialize};

use crate::config::LinksConfig;
use crate::core::auth::AuthContext;
use crate::core::module::Module;
use crate::server::builder::ServerBuilder;
use crate::server::entity_registry::EntityRegistry;
use crate::server::exposure::RestExposure;
use crate::server::host::ExposureSummary;
use crate::storage::InMemoryLinkService;

/// Les `mount` que l'hôte se réserve.
///
/// Un attachement qui les prendrait écraserait la structure elle-même : son
/// plan, ou sa sonde de vie.
pub const RESERVED_MOUNTS: &[&str] = &["/", "/health", "/healthz"];

/// Les placeholders d'identité acceptés dans un mappage.
///
/// Ce sont les seuls noms que [`AuthContext`] porte ; tout autre nom est refusé
/// au chargement, pour qu'une faute de frappe ne produise pas un mappage
/// silencieusement vide (donc un refus à l'exécution, sans explication).
pub const IDENTITY_PLACEHOLDERS: &[&str] = &[
    "user_id",
    "tenant_id",
    "role",
    "service_name",
    "admin_id",
    "resource_id",
    "resource_type",
];

/// Le fichier de déploiement.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeploymentConfig {
    /// Adresse d'écoute (`IP:port`). Défaut : `127.0.0.1:8080` — jamais `0.0.0.0`
    /// par défaut : une structure qui écoute partout doit le dire.
    #[serde(default = "default_bind")]
    pub bind: String,

    /// Les projets rattachés.
    pub attachments: Vec<Attachment>,
}

fn default_bind() -> String {
    "127.0.0.1:8080".to_string()
}

/// Un projet existant rattaché à la structure.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Attachment {
    /// Nom court, utilisé dans le plan et dans le `mount` par défaut.
    pub name: String,

    /// Préfixe sous lequel les surfaces de ce projet sont montées.
    /// Défaut : `/attached/<name>`.
    #[serde(default)]
    pub mount: Option<String>,

    /// La base du projet.
    pub store: StoreConfig,

    /// `config/server.yaml` du projet : CORS, domaines servis, locataire désigné
    /// par un domaine, délégation Caddy, zone PowerDNS.
    #[serde(default)]
    pub exposure: Option<PathBuf>,

    /// `links.yaml` du projet : définitions de liens et politiques `auth:`.
    #[serde(default)]
    pub policies: Option<PathBuf>,

    /// `auth.yaml` (WAMI) du projet : authentification JWT Ed25519.
    #[serde(default)]
    pub auth: Option<PathBuf>,

    /// Régime d'autorisation.
    #[serde(default)]
    pub authorization: AuthzConfig,

    /// Mappage d'identité vers **les noms du projet**.
    #[serde(default)]
    pub identity: Option<IdentityMapping>,
}

impl Attachment {
    /// Le préfixe effectivement monté.
    pub fn mount(&self) -> String {
        match &self.mount {
            Some(mount) => mount.clone(),
            None => format!("/attached/{}", self.name),
        }
    }
}

/// Où vit la base du projet rattaché.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum StoreConfig {
    /// Base en mémoire : rien à installer, rien à nettoyer. Elle ne survit pas au
    /// processus — utile pour valider une structure avant de la brancher.
    Memory,

    /// Base obrain existante.
    Obrain {
        /// Le répertoire de la base.
        path: PathBuf,

        /// Ouverture **en lecture seule** (défaut).
        ///
        /// `true` : mutations refusées au niveau moteur, verrou partagé, aucun
        /// bail d'écriture — on peut publier une base qu'un autre processus
        /// détient. `false` doit être **écrit** : l'hôte peut alors écrire dans
        /// une base en service, et prend le bail d'écriture (fail-closed : si un
        /// autre écrivain le détient, le démarrage échoue).
        #[serde(default = "default_read_only")]
        read_only: bool,
    },
}

fn default_read_only() -> bool {
    true
}

impl StoreConfig {
    /// Le nom du type de store, tel qu'il apparaît dans le plan.
    pub fn kind(&self) -> &'static str {
        match self {
            StoreConfig::Memory => "memory",
            StoreConfig::Obrain { .. } => "obrain",
        }
    }

    /// Le chemin de la base, quand il y en a un.
    pub fn path(&self) -> Option<&Path> {
        match self {
            StoreConfig::Memory => None,
            StoreConfig::Obrain { path, .. } => Some(path),
        }
    }

    /// La base est-elle ouverte en lecture seule ?
    ///
    /// Une base en mémoire n'est jamais « en lecture seule » : elle est neuve.
    pub fn read_only(&self) -> bool {
        match self {
            StoreConfig::Memory => false,
            StoreConfig::Obrain { read_only, .. } => *read_only,
        }
    }
}

/// Comment les politiques déclarées sont appliquées.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct AuthzConfig {
    /// Le régime. Défaut : `enforce` — le défaut sûr, celui du framework.
    #[serde(default)]
    pub mode: AuthzMode,
}

/// Les trois régimes, et ce qu'ils coûtent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum AuthzMode {
    /// Les politiques sont **appliquées** : un refus arrête la requête.
    #[default]
    Enforce,

    /// Les refus sont **calculés, journalisés et publiés** (`x-this-rs-authz`),
    /// mais rien n'est bloqué. C'est le mode d'adoption d'un projet qui tourne.
    ReportOnly,

    /// Les politiques sont **décoratives** : lues, jamais appliquées.
    Off,
}

impl AuthzMode {
    /// Le nom publié dans le plan.
    pub fn as_str(&self) -> &'static str {
        match self {
            AuthzMode::Enforce => "enforce",
            AuthzMode::ReportOnly => "report_only",
            AuthzMode::Off => "off",
        }
    }
}

/// Le mappage d'identité : les noms du projet, écrits comme des gabarits.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IdentityMapping {
    /// Gabarit du **principal**, ex. `account:{user_id}`.
    pub principal: String,

    /// Gabarit de la **portée**, ex. `org:{tenant_id}`.
    pub scope: String,
}

impl DeploymentConfig {
    /// Lit le fichier de déploiement.
    pub fn from_file(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let raw = std::fs::read_to_string(path)
            .with_context(|| format!("lecture du déploiement : {}", path.display()))?;
        let config: Self = serde_yaml::from_str(&raw)
            .with_context(|| format!("déploiement invalide : {}", path.display()))?;
        config
            .validate()
            .with_context(|| format!("déploiement refusé : {}", path.display()))?;
        Ok(config)
    }

    /// Lit un déploiement depuis un texte YAML (et le valide).
    pub fn from_yaml(yaml: &str) -> Result<Self> {
        let config: Self = serde_yaml::from_str(yaml).context("YAML invalide")?;
        config.validate()?;
        Ok(config)
    }

    /// Valide la structure **avant** d'ouvrir quoi que ce soit.
    ///
    /// Aucune E/S de base ici : seulement les fichiers déclarés (existence et
    /// lisibilité), pour qu'une faute de frappe se voie au chargement.
    pub fn validate(&self) -> Result<()> {
        self.bind.parse::<std::net::SocketAddr>().map_err(|e| {
            anyhow::anyhow!(
                "`bind: {}` n'est pas une adresse `IP:port` ({e}) — ex. `127.0.0.1:8080`",
                self.bind
            )
        })?;

        if self.attachments.is_empty() {
            bail!(
                "aucun attachement : un hôte de déploiement sans projet rattaché ne sert rien. \
                 Déclarez au moins un `attachments: - name: …`"
            );
        }

        let mut names: BTreeSet<&str> = BTreeSet::new();
        let mut mounts: BTreeSet<String> = BTreeSet::new();
        let mut bases: BTreeSet<&Path> = BTreeSet::new();

        for attachment in &self.attachments {
            let name = attachment.name.as_str();
            if name.is_empty() {
                bail!("un attachement a un `name` vide");
            }
            if !name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
            {
                bail!(
                    "attachement `{name}` : le nom n'accepte que `[a-zA-Z0-9_-]` — il sert de \
                     segment de chemin"
                );
            }
            if !names.insert(name) {
                bail!("deux attachements portent le nom `{name}`");
            }

            let mount = attachment.mount();
            if !mount.starts_with('/') {
                bail!("attachement `{name}` : `mount: {mount}` doit commencer par `/`");
            }
            if mount.len() > 1 && mount.ends_with('/') {
                bail!("attachement `{name}` : `mount: {mount}` ne doit pas finir par `/`");
            }
            if RESERVED_MOUNTS.contains(&mount.as_str()) {
                bail!(
                    "attachement `{name}` : `mount: {mount}` est réservé à l'hôte \
                     (plan et sonde de vie) — choisissez un préfixe"
                );
            }
            if !mounts.insert(mount.clone()) {
                bail!("deux attachements sont montés sur `{mount}`");
            }

            if let StoreConfig::Obrain { path, .. } = &attachment.store {
                if path.as_os_str().is_empty() {
                    bail!("attachement `{name}` : `store.path` est vide");
                }
                if !bases.insert(path) {
                    bail!(
                        "attachement `{name}` : la base `{}` est déjà rattachée par un autre \
                         attachement — deux hôtes sur une même base écrite se refusent le bail \
                         d'écriture (ouvrez-la en `read_only: true`)",
                        path.display()
                    );
                }
            }

            for (label, file) in [
                ("exposure", &attachment.exposure),
                ("policies", &attachment.policies),
                ("auth", &attachment.auth),
            ] {
                if let Some(file) = file
                    && !file.is_file()
                {
                    bail!(
                        "attachement `{name}` : `{label}: {}` est introuvable — l'hôte \
                         refuse de démarrer avec un fichier déclaré et absent",
                        file.display()
                    );
                }
            }

            // L'exposition du projet est **lue** au chargement : un `server.yaml`
            // illisible doit arrêter le démarrage ici, pas à la construction de
            // l'hôte après que d'autres bases ont été ouvertes.
            if let Some(file) = &attachment.exposure {
                crate::config::ServerSecurityConfig::from_file(&file.to_string_lossy())
                    .with_context(|| {
                        format!(
                            "attachement `{name}` : `exposure: {}` est illisible",
                            file.display()
                        )
                    })?;
            }

            if attachment.auth.is_some() && !cfg!(feature = "wami") {
                bail!(
                    "attachement `{name}` : `auth:` déclaré, mais ce binaire a été construit \
                     sans la feature `wami` — reconstruisez avec `--features wami`"
                );
            }

            if let Some(identity) = &attachment.identity {
                for (role, template) in [
                    ("principal", &identity.principal),
                    ("scope", &identity.scope),
                ] {
                    if template.is_empty() {
                        bail!("attachement `{name}` : `identity.{role}` est vide");
                    }
                    check_placeholders(name, role, template)?;
                }
            }
        }

        Ok(())
    }

    /// Le plan de la structure, **sans ouvrir aucune base**.
    ///
    /// C'est ce que rend `--dry-run`, et ce qu'un opérateur relit avant de
    /// déployer. `inspected: false` dit exactement ce que le document est : une
    /// lecture de la déclaration, pas de l'état des bases.
    pub fn plan(&self) -> Plan {
        self.plan_with(false, &[])
    }

    fn plan_with(&self, inspected: bool, exposure: &[Option<ExposureSummary>]) -> Plan {
        let mut notes = vec![
            "le plan ne contient aucun secret : ni politique, ni clé, ni jeton".to_string(),
            "l'hôte ne proxifie pas : `config/server.yaml → proxy.caddy` produit un `Caddyfile` \
             que l'exploitant applique"
                .to_string(),
        ];
        if !cfg!(feature = "obrain") {
            notes.push(
                "binaire construit sans la feature `obrain` : seul le store `memory` peut être \
                 rattaché"
                    .to_string(),
            );
        }

        let attachments = self
            .attachments
            .iter()
            .enumerate()
            .map(|(i, attachment)| {
                let mut warnings = Vec::new();
                let graph_guarded = graph_policy_armed(attachment);
                match attachment.authorization.mode {
                    // Une abstention n'est plus un passage (voir
                    // `Middleware::authorization`) : en mode `enforce`, une route
                    // que le résolveur ne reconnaît pas est **refusée**. Les
                    // lectures de graphe en font partie tant qu'aucune
                    // `graph.policy` n'est déclarée — et c'est exactement le
                    // genre de 403 qui ne s'explique pas tout seul.
                    AuthzMode::Enforce if !graph_guarded => warnings.push(
                        "mode `enforce` sans `graph.policy` : les lectures de graphe \
                         s'abstiennent, et une abstention est un refus — `/graph*` répond 403. \
                         Déclarez `graph.policy` dans le `server.yaml` du projet (ses décisions \
                         IAM doivent alors être matérialisées dans sa base), ou attachez en \
                         `report_only` (observer) ou `off` (relief descriptif et public)."
                            .to_string(),
                    ),
                    AuthzMode::Enforce => {}
                    AuthzMode::ReportOnly => warnings.push(
                        "mode observation : les refus sont calculés et publiés, jamais appliqués"
                            .to_string(),
                    ),
                    AuthzMode::Off => warnings.push(
                        "autorisation désarmée : les politiques `auth:` déclarées sont \
                         décoratives, et les lectures de graphe redeviennent descriptives et \
                         publiques"
                            .to_string(),
                    ),
                }

                if attachment.store.read_only() {
                    warnings.push(
                        "base ouverte en lecture seule : aucune décision IAM ne peut y être \
                         matérialisée"
                            .to_string(),
                    );
                } else if attachment.store.path().is_some() {
                    warnings.push(
                        "base ouverte en ÉCRITURE : le bail d'écriture est armé, un second \
                         écrivain sera refusé"
                            .to_string(),
                    );
                }
                if attachment.identity.is_none() {
                    warnings.push(
                        "aucun mappage d'identité : le modèle natif de this-rs s'applique \
                         (`User`/`Owner` → `(user_id, tenant_id)`)"
                            .to_string(),
                    );
                }
                if attachment.policies.is_none() {
                    warnings.push(
                        "aucun fichier de politiques : les surfaces rattachées ne sont jugées \
                         que par la couche d'authentification"
                            .to_string(),
                    );
                }

                AttachmentPlan {
                    name: attachment.name.clone(),
                    mount: attachment.mount(),
                    store: StorePlan {
                        kind: attachment.store.kind().to_string(),
                        path: attachment.store.path().map(|p| p.display().to_string()),
                        read_only: attachment.store.read_only(),
                    },
                    authorization: AuthzPlan {
                        mode: attachment.authorization.mode.as_str().to_string(),
                        policies: attachment
                            .policies
                            .as_ref()
                            .map(|p| p.display().to_string()),
                    },
                    identity: match &attachment.identity {
                        None => IdentityPlan::Native,
                        Some(mapping) => IdentityPlan::Mapped {
                            principal: mapping.principal.clone(),
                            scope: mapping.scope.clone(),
                        },
                    },
                    exposure: exposure.get(i).cloned().flatten(),
                    surfaces: surfaces_of(attachment),
                    warnings,
                }
            })
            .collect();

        let mut not_attached = vec![
            "routes CRUD d'entités : elles exigent un descripteur Rust \
             (`EntityDescriptor`, donc un service de données) — l'hôte n'en invente pas depuis \
             la configuration"
                .to_string(),
            "reverse proxy vers un projet existant : l'hôte ne transporte pas de trafic, \
             il rattache une base et une identité"
                .to_string(),
        ];
        if self
            .attachments
            .iter()
            .any(|a| matches!(a.store, StoreConfig::Obrain { .. }))
            && !cfg!(feature = "obrain")
        {
            not_attached.push(
                "surfaces de graphe (`/graph`, `/graph/identity`, `/graph/hierarchy/{id}`) : \
                 elles exigent la feature `obrain`"
                    .to_string(),
            );
        }

        Plan {
            structure: "this-rs host",
            version: env!("CARGO_PKG_VERSION"),
            bind: self.bind.clone(),
            inspected,
            attachments,
            not_attached,
            notes,
        }
    }
}

/// Le `server.yaml` du projet soumet-il les lectures de graphe à une politique ?
///
/// C'est la question qui décide si `/graph*` est **jugé** ou **abstenu** — et
/// depuis que l'abstention est un refus, elle décide si la surface répond 200 ou
/// 403. Autant la poser au chargement plutôt que de laisser un opérateur
/// découvrir un 403 inexplicable.
fn graph_policy_armed(attachment: &Attachment) -> bool {
    attachment.exposure.as_ref().is_some_and(|path| {
        crate::config::ServerSecurityConfig::from_file(&path.to_string_lossy())
            .map(|config| config.graph.armed())
            .unwrap_or(false)
    })
}

/// Vérifie que chaque `{placeholder}` d'un gabarit est connu.
fn check_placeholders(attachment: &str, role: &str, template: &str) -> Result<()> {
    let mut rest = template;
    while let Some(start) = rest.find('{') {
        let after = &rest[start + 1..];
        let Some(end) = after.find('}') else {
            bail!(
                "attachement `{attachment}` : `identity.{role}: {template}` a une accolade \
                 ouvrante sans fermante"
            );
        };
        let key = &after[..end];
        if !IDENTITY_PLACEHOLDERS.contains(&key) {
            bail!(
                "attachement `{attachment}` : `identity.{role}` utilise `{{{key}}}`, qui n'existe \
                 pas — placeholders connus : {}",
                IDENTITY_PLACEHOLDERS.join(", ")
            );
        }
        rest = &after[end + 1..];
    }
    Ok(())
}

/// Remplace les placeholders d'un gabarit par les valeurs du contexte.
///
/// Rend `None` dès qu'une valeur **manque** pour ce contexte : le mappage
/// n'invente rien, et une identité absente rend la politique non tranchable —
/// donc refusée (fail-closed).
pub fn expand_identity(template: &str, context: &AuthContext) -> Option<String> {
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(start) = rest.find('{') {
        out.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        let end = after.find('}')?;
        out.push_str(&placeholder_value(&after[..end], context)?);
        rest = &after[end + 1..];
    }
    out.push_str(rest);
    Some(out)
}

fn placeholder_value(key: &str, context: &AuthContext) -> Option<String> {
    match (key, context) {
        ("user_id", AuthContext::User { user_id, .. } | AuthContext::Owner { user_id, .. }) => {
            Some(user_id.to_string())
        }
        ("tenant_id", context) => context.tenant_id().map(|t| t.to_string()),
        ("role", AuthContext::User { roles, .. }) => roles.first().cloned(),
        ("service_name", AuthContext::Service { service_name, .. }) => Some(service_name.clone()),
        ("admin_id", AuthContext::Admin { admin_id }) => Some(admin_id.to_string()),
        ("resource_id", AuthContext::Owner { resource_id, .. }) => Some(resource_id.to_string()),
        ("resource_type", AuthContext::Owner { resource_type, .. }) => Some(resource_type.clone()),
        _ => None,
    }
}

/// Les chemins qu'un attachement monte réellement.
fn surfaces_of(attachment: &Attachment) -> Vec<String> {
    let mount = attachment.mount();
    let mut surfaces = vec![format!("{mount}/health"), format!("{mount}/healthz")];
    if matches!(attachment.store, StoreConfig::Obrain { .. }) && cfg!(feature = "obrain") {
        for route in [
            "/graph",
            "/graph/procedures",
            "/graph/spaces",
            "/graph/identity",
            "/graph/identity/{id}",
            "/graph/hierarchy/{id}",
        ] {
            surfaces.push(format!("{mount}{route}"));
        }
    }
    surfaces
}

// ---------------------------------------------------------------------------
// Construction
// ---------------------------------------------------------------------------

/// La structure construite : le routeur, et le plan qui le décrit.
pub struct Host {
    /// Le routeur complet (plan, sondes de vie, attachements montés).
    pub router: Router,
    /// Le plan, `inspected: true` : les bases ont été ouvertes, et l'exposition
    /// effective de chaque attachement y figure.
    pub plan: Plan,
}

impl DeploymentConfig {
    /// Construit la structure : ouvre les bases, monte les surfaces.
    ///
    /// Chaque attachement obtient son propre hôte (`ServerHost`) : sa propre
    /// exposition, ses propres politiques, son propre store. Rien n'est partagé
    /// implicitement entre deux projets rattachés.
    pub fn build(&self) -> Result<Host> {
        self.validate()?;

        let mut router = Router::new();
        let mut exposures: Vec<Option<ExposureSummary>> = Vec::new();

        for attachment in &self.attachments {
            let (attachment_router, exposure) = build_attachment(attachment)?;
            exposures.push(exposure);
            router = router.nest(&attachment.mount(), attachment_router);
        }

        let plan = self.plan_with(true, &exposures);
        let published = Arc::new(plan.clone());
        let count = self.attachments.len();
        let plan_route = get(move || {
            let plan = Arc::clone(&published);
            async move { axum::Json(plan.as_ref().clone()) }
        });
        let alive = get(move || async move {
            axum::Json(serde_json::json!({ "status": "ok", "attachments": count }))
        });

        // Les sondes de vie de l'hôte, montées **après** les attachements : leur
        // préfixe est réservé, donc rien ne peut les masquer.
        router = router
            .route("/", plan_route)
            .route("/health", alive.clone())
            .route("/healthz", alive);

        Ok(Host { router, plan })
    }
}

/// Construit le routeur d'un attachement, et l'exposition qu'il a effectivement
/// posée.
fn build_attachment(attachment: &Attachment) -> Result<(Router, Option<ExposureSummary>)> {
    let mut builder = ServerBuilder::new();

    match &attachment.store {
        StoreConfig::Memory => {
            builder = builder.with_link_service(InMemoryLinkService::new());
        }
        StoreConfig::Obrain { path, read_only } => {
            #[cfg(feature = "obrain")]
            {
                let store = if *read_only {
                    crate::storage::ObrainStore::open_read_only(path).with_context(|| {
                        format!(
                            "attachement `{}` : ouverture en lecture seule de {}",
                            attachment.name,
                            path.display()
                        )
                    })?
                } else {
                    crate::storage::ObrainStore::open(path).with_context(|| {
                        format!(
                            "attachement `{}` : ouverture en écriture de {}",
                            attachment.name,
                            path.display()
                        )
                    })?
                };
                tracing::info!(
                    attachment = attachment.name,
                    base = %path.display(),
                    read_only,
                    "base obrain rattachée"
                );
                builder = builder.with_obrain_store(store);
            }
            #[cfg(not(feature = "obrain"))]
            {
                let _ = (path, read_only);
                bail!(
                    "attachement `{}` : store `obrain` déclaré, mais ce binaire a été construit \
                     sans la feature `obrain` — reconstruisez avec `--features obrain`",
                    attachment.name
                );
            }
        }
    }

    if let Some(exposure) = &attachment.exposure {
        builder = builder.with_server_config_file(&exposure.to_string_lossy())?;
    }

    if let Some(policies) = &attachment.policies {
        builder = builder.register_module(ConfigOnlyModule {
            name: attachment.name.clone(),
            path: policies.clone(),
        })?;
    }

    #[cfg(feature = "wami")]
    if let Some(auth) = &attachment.auth {
        builder = builder.with_auth_config_file(&auth.to_string_lossy())?;
    }

    builder = match attachment.authorization.mode {
        AuthzMode::Enforce => builder.with_authorization(),
        AuthzMode::ReportOnly => builder.authorization_report_only(),
        AuthzMode::Off => builder.without_authorization(),
    };

    if let Some(mapping) = &attachment.identity {
        let principal = mapping.principal.clone();
        let scope = mapping.scope.clone();
        builder = builder.with_principal_mapping(move |context| {
            let principal = expand_identity(&principal, context)?;
            let scope = expand_identity(&scope, context)?;
            Some((principal, scope))
        });
    }

    let host = Arc::new(builder.build_host()?);
    let exposure = host.exposure();
    let router = RestExposure::build_router(host, Vec::new())?;
    Ok((router, Some(exposure)))
}

/// Un module **sans entité** : il ne sert qu'à porter une configuration.
///
/// C'est la pièce qui permet de rattacher un projet sans écrire de Rust : son
/// `links.yaml` (définitions de liens et politiques `auth:`) est lu tel quel,
/// alors qu'un `Module` est normalement écrit à la main pour porter des
/// descripteurs d'entités.
struct ConfigOnlyModule {
    name: String,
    path: PathBuf,
}

impl Module for ConfigOnlyModule {
    fn name(&self) -> &str {
        &self.name
    }

    fn entity_types(&self) -> Vec<&str> {
        Vec::new()
    }

    fn links_config(&self) -> Result<LinksConfig> {
        LinksConfig::from_yaml_file(&self.path.to_string_lossy())
    }

    fn register_entities(&self, _registry: &mut EntityRegistry) {}
}

// ---------------------------------------------------------------------------
// Le plan publié
// ---------------------------------------------------------------------------

/// Le document publié par `GET /` (et par `--dry-run`).
///
/// Il est **sérialisé** seulement : un plan se publie, il ne se relit pas comme
/// une configuration — la déclaration, c'est [`DeploymentConfig`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Plan {
    /// Le type de structure.
    pub structure: &'static str,
    /// La version du binaire.
    pub version: &'static str,
    /// L'adresse d'écoute déclarée.
    pub bind: String,
    /// Les bases ont-elles été **ouvertes** pour produire ce plan ?
    pub inspected: bool,
    /// Les projets rattachés.
    pub attachments: Vec<AttachmentPlan>,
    /// Ce que la structure ne rattache **pas**, et pourquoi.
    pub not_attached: Vec<String>,
    /// Les propriétés de la structure elle-même.
    pub notes: Vec<String>,
}

/// Un projet rattaché, tel qu'il est publié.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AttachmentPlan {
    /// Nom du projet.
    pub name: String,
    /// Préfixe monté.
    pub mount: String,
    /// La base.
    pub store: StorePlan,
    /// Le régime d'autorisation.
    pub authorization: AuthzPlan,
    /// Le mappage d'identité.
    pub identity: IdentityPlan,
    /// Les chemins montés.
    pub surfaces: Vec<String>,
    /// Ce que l'application a **effectivement** posé à l'entrée HTTP — des
    /// comptes et des faits, jamais les listes (voir [`ExposureSummary`]).
    pub exposure: Option<ExposureSummary>,
    /// Ce qu'un opérateur doit savoir avant de déployer.
    pub warnings: Vec<String>,
}

/// La base d'un attachement.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StorePlan {
    /// `memory` ou `obrain`.
    pub kind: String,
    /// Le chemin, quand il y en a un.
    pub path: Option<String>,
    /// La base est-elle ouverte en lecture seule ?
    pub read_only: bool,
}

/// Le régime d'autorisation d'un attachement.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthzPlan {
    /// `enforce`, `report_only` ou `off`.
    pub mode: String,
    /// Le fichier de politiques, quand il y en a un.
    pub policies: Option<String>,
}

/// Le mappage d'identité d'un attachement.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum IdentityPlan {
    /// Le modèle natif de this-rs, sans mappage.
    Native,
    /// Un mappage vers les noms du projet.
    Mapped {
        /// Gabarit du principal.
        principal: String,
        /// Gabarit de la portée.
        scope: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    fn minimal() -> &'static str {
        r#"
bind: 127.0.0.1:0
attachments:
  - name: facturation
    store:
      kind: obrain
      path: /tmp/facturation.obrain
"#
    }

    #[test]
    fn a_minimal_deployment_validates_and_publishes_its_plan() {
        let config = DeploymentConfig::from_yaml(minimal()).expect("déploiement valide");
        assert_eq!(config.bind, "127.0.0.1:0");
        let plan = config.plan();
        assert!(!plan.inspected, "un plan sec n'a ouvert aucune base");
        assert_eq!(plan.attachments.len(), 1);
        let attachment = &plan.attachments[0];
        assert_eq!(attachment.mount, "/attached/facturation");
        assert_eq!(attachment.store.kind, "obrain");
        assert!(attachment.store.read_only, "lecture seule par défaut");
        assert_eq!(attachment.authorization.mode, "enforce");
        assert!(matches!(attachment.identity, IdentityPlan::Native));
    }

    #[test]
    fn the_plan_publishes_paths_never_the_contents_of_declared_files() {
        let dir = tempfile::tempdir().expect("répertoire temporaire");
        let policies = dir.path().join("links.yaml");
        std::fs::write(
            &policies,
            "entities:\n  - singular: invoice\n    plural: invoices\n    auth:\n      list: TOKEN-INTERDIT\nlinks: []\n",
        )
        .expect("écriture des politiques");

        let config = DeploymentConfig::from_yaml(&format!(
            "attachments:\n  - name: a\n    store: {{ kind: memory }}\n    policies: {}\n",
            policies.display()
        ))
        .expect("déploiement valide");
        let text = serde_json::to_string(&config.plan()).expect("plan sérialisé");

        assert!(
            !text.contains("TOKEN-INTERDIT"),
            "le plan publie le chemin d'un fichier déclaré, jamais son contenu"
        );
        assert!(
            text.contains("links.yaml"),
            "le chemin, lui, est publié — un opérateur doit savoir ce qui est lu"
        );
    }

    #[test]
    fn an_unknown_store_kind_is_refused_at_load() {
        let yaml = minimal().replace("kind: obrain", "kind: postgres");
        let error = DeploymentConfig::from_yaml(&yaml).unwrap_err().to_string();
        assert!(error.contains("YAML invalide"), "{error}");
    }

    #[test]
    fn write_access_must_be_written_explicitly() {
        let yaml = minimal().replace(
            "path: /tmp/facturation.obrain",
            "path: /tmp/facturation.obrain\n      read_only: false",
        );
        let config = DeploymentConfig::from_yaml(&yaml).unwrap();
        assert!(!config.attachments[0].store.read_only());
        let warnings = &config.plan().attachments[0].warnings;
        assert!(
            warnings.iter().any(|w| w.contains("bail d'écriture")),
            "une base ouverte en écriture doit être annoncée : {warnings:?}"
        );
    }

    #[test]
    fn a_reserved_or_duplicated_mount_is_refused() {
        let yaml = r#"
attachments:
  - name: a
    mount: /healthz
    store: { kind: memory }
"#;
        let error = DeploymentConfig::from_yaml(yaml).unwrap_err().to_string();
        assert!(error.contains("réservé"), "{error}");

        let yaml = r#"
attachments:
  - name: a
    mount: /x
    store: { kind: memory }
  - name: b
    mount: /x
    store: { kind: memory }
"#;
        let error = DeploymentConfig::from_yaml(yaml).unwrap_err().to_string();
        assert!(error.contains("deux attachements sont montés"), "{error}");
    }

    #[test]
    fn an_unknown_identity_placeholder_is_refused_at_load() {
        let yaml = r#"
attachments:
  - name: a
    store: { kind: memory }
    identity:
      principal: "account:{userid}"
      scope: "org:{tenant_id}"
"#;
        let error = DeploymentConfig::from_yaml(yaml).unwrap_err().to_string();
        assert!(error.contains("userid"), "{error}");
        assert!(
            error.contains("tenant_id"),
            "le message doit nommer les placeholders connus"
        );
    }

    #[test]
    fn a_declared_but_absent_file_is_refused() {
        let yaml = r#"
attachments:
  - name: a
    store: { kind: memory }
    policies: /nonexistent/links.yaml
"#;
        let error = DeploymentConfig::from_yaml(yaml).unwrap_err().to_string();
        assert!(error.contains("introuvable"), "{error}");
    }

    #[test]
    fn two_attachments_on_one_base_are_refused_before_the_lease_does_it() {
        let yaml = r#"
attachments:
  - name: a
    store: { kind: obrain, path: /tmp/partagee.obrain }
  - name: b
    store: { kind: obrain, path: /tmp/partagee.obrain }
"#;
        let error = DeploymentConfig::from_yaml(yaml).unwrap_err().to_string();
        assert!(error.contains("déjà rattachée"), "{error}");
    }

    #[test]
    fn a_vague_bind_is_refused() {
        let yaml = r#"
bind: 0.0.0.0
attachments:
  - name: a
    store: { kind: memory }
"#;
        let error = DeploymentConfig::from_yaml(yaml).unwrap_err().to_string();
        assert!(error.contains("IP:port"), "{error}");
    }

    #[test]
    fn an_unknown_key_in_the_deployment_is_refused() {
        let yaml = r#"
attachments:
  - name: a
    store: { kind: memory }
    authorisation: { mode: report_only }
"#;
        assert!(DeploymentConfig::from_yaml(yaml).is_err());
    }

    #[test]
    fn identity_templates_expand_only_what_the_context_carries() {
        let user = AuthContext::User {
            user_id: uuid::Uuid::nil(),
            tenant_id: uuid::Uuid::nil(),
            roles: vec!["operator".to_string()],
        };
        assert_eq!(
            expand_identity("account:{user_id}", &user).as_deref(),
            Some("account:00000000-0000-0000-0000-000000000000")
        );
        assert_eq!(
            expand_identity("role:{role}", &user).as_deref(),
            Some("role:operator")
        );
        // Un contexte qui ne porte pas la valeur ne fait pas deviner le mappage.
        assert_eq!(expand_identity("svc:{service_name}", &user), None);
        assert_eq!(
            expand_identity("role:{role}", &AuthContext::Anonymous),
            None
        );
    }
}
