//! **Configuration d'exposition** : CORS, domaines servis, délégation de proxy.
//!
//! Ces trois réglages vont ensemble, et c'est pour ça qu'ils vivent dans le même
//! fichier (`config/server.yaml`) : ils décrivent **par où** l'application est
//! atteinte et **qui** a le droit de l'atteindre depuis un navigateur.
//!
//! # Aucun défaut permissif
//!
//! * Pas de configuration ⇒ **aucun changement de comportement** : pas de
//!   couche CORS, pas de garde de domaine. Le framework n'invente rien.
//! * Une fois configurés, les réglages sont **fermés** : une origine absente de
//!   la liste n'obtient pas d'en-tête CORS, un hôte absent de la liste est
//!   **refusé**.
//!
//! # La délégation à Caddy
//!
//! Le framework **décrit** la configuration du proxy, il ne le pilote pas :
//! [`CaddyConfig`] rend un `Caddyfile` ([`ServerSecurityConfig::caddyfile`]), que
//! l'exploitant relit et applique. C'est une délégation, pas une automatisation
//! silencieuse — et c'est aussi ce qui dit à l'application quand faire confiance
//! aux en-têtes `X-Forwarded-*` (`trust_forwarded_host`), puisque c'est Caddy qui
//! les pose.

use std::collections::BTreeMap;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

/// Ce que le framework applique à l'entrée HTTP.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServerSecurityConfig {
    /// Politique CORS. Absente par défaut : aucune couche n'est posée.
    #[serde(default)]
    pub cors: CorsConfig,

    /// Domaines servis, et locataire associé.
    #[serde(default)]
    pub domains: DomainConfig,

    /// Délégation au proxy.
    #[serde(default)]
    pub proxy: ProxyConfig,

    /// **Délégation DNS** : la zone que le serveur autoritaire doit publier.
    ///
    /// Absente par défaut : le framework ne touche pas au DNS sans qu'on le lui
    /// demande — et il ne touche jamais qu'aux enregistrements **A/AAAA/CNAME**
    /// des sites déclarés.
    #[serde(default)]
    pub dns: Option<crate::dns::PdnsConfig>,
}

impl ServerSecurityConfig {
    /// Lit `config/server.yaml` (ou tout autre fichier du même schéma).
    pub fn from_file(path: &str) -> Result<Self> {
        let raw = std::fs::read_to_string(path)
            .with_context(|| format!("lecture de la configuration serveur : {path}"))?;
        Self::from_yaml(&raw).with_context(|| format!("configuration serveur invalide : {path}"))
    }

    /// Lit la configuration depuis un texte YAML.
    pub fn from_yaml(yaml: &str) -> Result<Self> {
        serde_yaml::from_str(yaml).context("YAML invalide")
    }

    /// Le `Caddyfile` correspondant à cette configuration.
    ///
    /// Rend `None` si la délégation n'est pas activée : le framework ne produit
    /// pas de configuration de proxy que personne n'a demandée.
    pub fn caddyfile(&self) -> Option<String> {
        if !self.proxy.caddy.enabled {
            return None;
        }
        Some(self.proxy.caddy.render(&self.domains, &self.cors))
    }
}

// ---------------------------------------------------------------------------
// CORS
// ---------------------------------------------------------------------------

/// Politique CORS.
///
/// **Le défaut programmatique est celui du YAML.** Une version antérieure
/// dérivait `Default`, ce qui donnait des listes de méthodes et d'en-têtes
/// **vides** en Rust alors que le fichier YAML appliquait les listes usuelles :
/// deux défauts pour un même type, donc deux comportements selon la façon de
/// configurer. C'est exactement le genre d'écart qui fait qu'une configuration
/// « marche en YAML » et pas par programme.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CorsConfig {
    /// Poser la couche CORS ? **Faux par défaut** : sans configuration, le
    /// framework n'ajoute aucun en-tête.
    #[serde(default)]
    pub enabled: bool,

    /// Origines autorisées (`https://app.exemple.fr`, ou `*`).
    ///
    /// Une origine absente de cette liste n'obtient **aucun** en-tête CORS — le
    /// navigateur bloque. `*` avec `credentials: true` est **refusé** : c'est une
    /// combinaison que les navigateurs rejettent, et l'accepter ici ferait croire
    /// à une configuration qui ne marche pas.
    #[serde(default)]
    pub origins: Vec<String>,

    /// Méthodes autorisées.
    #[serde(default = "default_cors_methods")]
    pub methods: Vec<String>,

    /// En-têtes de requête autorisés.
    #[serde(default = "default_cors_headers")]
    pub headers: Vec<String>,

    /// En-têtes de réponse exposés au navigateur.
    #[serde(default)]
    pub expose_headers: Vec<String>,

    /// Autoriser l'envoi des identifiants (cookies, `Authorization`).
    #[serde(default)]
    pub credentials: bool,

    /// Durée de mise en cache du pré-vol, en secondes.
    #[serde(default)]
    pub max_age_secs: Option<u64>,
}

impl Default for CorsConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            origins: Vec::new(),
            methods: default_cors_methods(),
            headers: default_cors_headers(),
            expose_headers: Vec::new(),
            credentials: false,
            max_age_secs: None,
        }
    }
}

fn default_cors_methods() -> Vec<String> {
    ["GET", "POST", "PUT", "PATCH", "DELETE", "OPTIONS"]
        .iter()
        .map(|s| (*s).to_string())
        .collect()
}

fn default_cors_headers() -> Vec<String> {
    ["authorization", "content-type", "x-tenant-id"]
        .iter()
        .map(|s| (*s).to_string())
        .collect()
}

impl CorsConfig {
    /// La configuration est-elle utilisable ? (fail-closed sur les incohérences)
    pub fn validate(&self) -> Result<()> {
        if !self.enabled {
            return Ok(());
        }
        if self.origins.is_empty() {
            bail!(
                "CORS activé sans origine : aucune requête de navigateur ne serait autorisée. \
                 Listez vos origines (`origins: [\"https://app.exemple.fr\"]`) ou désactivez CORS."
            );
        }
        if self.credentials && self.origins.iter().any(|o| o == "*") {
            bail!(
                "CORS : `origins: [\"*\"]` avec `credentials: true` est refusé par les navigateurs. \
                 Listez les origines explicitement."
            );
        }
        if self.methods.iter().any(|m| m == "*") {
            bail!("CORS : `methods: [\"*\"]` n'est pas supporté — listez les méthodes.");
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Domaines
// ---------------------------------------------------------------------------

/// Domaines servis, et ce qu'ils désignent.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DomainConfig {
    /// Hôtes servis. **Vide = pas de garde** (le framework ne devine pas vos
    /// domaines). Non vide = tout autre `Host` est **refusé**.
    ///
    /// Un `*.exemple.fr` autorise exactement un niveau de sous-domaine.
    #[serde(default)]
    pub allowed: Vec<String>,

    /// **La liste explicite des sites**, pour la délégation.
    ///
    /// C'est **la même liste** qui alimente le `Caddyfile` et les
    /// enregistrements DNS : une déclaration, deux délégations — pas deux
    /// inventaires à tenir en accord.
    ///
    /// `allowed` est un **filtre** : il accepte des motifs (`*.exemple.fr`). Un
    /// `Caddyfile`, lui, énumère des **sites** : un motif ne dit pas quels
    /// domaines existent. Sans cette liste, la délégation écrit les motifs
    /// (Caddy sait faire un bloc joker, mais on ne voit pas ce qui est servi) ;
    /// avec elle, chaque domaine a son bloc, et l'exploitant relit ce qu'il
    /// applique.
    #[serde(default)]
    pub sites: Vec<String>,

    /// Faire confiance à `X-Forwarded-Host` (posé par le proxy).
    ///
    /// À n'activer **que** derrière un proxy de confiance : sinon un client
    /// choisit lui-même l'hôte que le framework croit servir.
    #[serde(default)]
    pub trust_forwarded_host: bool,

    /// Table explicite domaine → locataire (UUID).
    #[serde(default)]
    pub tenants: BTreeMap<String, String>,

    /// Base de sous-domaine : `<locataire>.exemple.fr` désigne le locataire.
    ///
    /// Le premier label est alors lu comme un UUID de locataire.
    #[serde(default)]
    pub subdomain_base: Option<String>,

    /// Locataire par défaut quand l'hôte ne désigne personne.
    #[serde(default)]
    pub default_tenant: Option<String>,
}

/// **Un motif d'hôte correspond-il ?** — logique **partagée**.
///
/// Le garde de domaine et la répartition des sites répondent à la même question
/// de la même façon : deux implémentations divergentes laisseraient passer un
/// hôte que l'une refuse, ou serviraient le site d'un autre.
///
/// `*.exemple.fr` couvre **exactement un** niveau de sous-domaine.
pub fn host_matches(pattern: &str, host: &str) -> bool {
    let pattern = normalize_host(pattern);
    let host = normalize_host(host);
    match pattern.strip_prefix("*.") {
        Some(suffix) => host.strip_suffix(suffix).is_some_and(|prefix| {
            prefix.ends_with('.') && !prefix[..prefix.len() - 1].contains('.')
        }),
        None => host == pattern,
    }
}

/// `Exemple.FR:443` → `exemple.fr` (minuscules, sans port ni point final).
pub fn normalize_host(host: &str) -> String {
    let host = host.trim().trim_end_matches('.');
    let host = host.split(':').next().unwrap_or(host);
    host.to_ascii_lowercase()
}

impl DomainConfig {
    /// Une garde est-elle active ?
    pub fn guards(&self) -> bool {
        !self.allowed.is_empty()
    }

    /// L'hôte est-il autorisé ? (faux quand aucune liste n'est configurée)
    pub fn allows(&self, host: &str) -> bool {
        if !self.guards() {
            return true;
        }
        self.allowed
            .iter()
            .any(|pattern| host_matches(pattern, host))
    }

    /// Le locataire désigné par cet hôte, s'il y en a un.
    ///
    /// L'ordre : la **table explicite** d'abord, puis le **sous-domaine**, puis
    /// le locataire par défaut.
    pub fn tenant_of(&self, host: &str) -> Option<uuid::Uuid> {
        let host = normalize_host(host);
        if let Some(tenant) = self.tenants.get(&host) {
            return uuid::Uuid::parse_str(tenant).ok();
        }
        if let Some(base) = &self.subdomain_base {
            let base = normalize_host(base);
            if let Some(label) = host.strip_suffix(&base).and_then(|p| p.strip_suffix('.'))
                && !label.is_empty()
                && !label.contains('.')
                && let Ok(tenant) = uuid::Uuid::parse_str(label)
            {
                return Some(tenant);
            }
        }
        self.default_tenant
            .as_ref()
            .and_then(|t| uuid::Uuid::parse_str(t).ok())
    }

    /// Valide la configuration (fail-closed : une valeur illisible est refusée
    /// au démarrage, pas au premier appel).
    pub fn validate(&self) -> Result<()> {
        for (host, tenant) in &self.tenants {
            if uuid::Uuid::parse_str(tenant).is_err() {
                bail!("domains.tenants : `{host}` → `{tenant}` n'est pas un UUID de locataire");
            }
        }
        for value in [&self.default_tenant] {
            if let Some(tenant) = value
                && uuid::Uuid::parse_str(tenant).is_err()
            {
                bail!("domains.default_tenant : `{tenant}` n'est pas un UUID");
            }
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Proxy / Caddy
// ---------------------------------------------------------------------------

/// Délégation au proxy.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProxyConfig {
    /// Délégation à Caddy.
    #[serde(default)]
    pub caddy: CaddyConfig,
}

/// Ce que le framework écrit pour Caddy — et que l'exploitant applique.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CaddyConfig {
    /// Produire un `Caddyfile` ? Faux par défaut.
    #[serde(default)]
    pub enabled: bool,

    /// L'adresse de l'application, telle que Caddy doit la joindre.
    #[serde(default = "default_upstream")]
    pub upstream: String,

    /// Adresse de contact ACME (Let's Encrypt).
    #[serde(default)]
    pub email: Option<String>,

    /// Comment Caddy obtient ses certificats.
    #[serde(default)]
    pub tls: CaddyTls,

    /// Poser les en-têtes de sécurité usuels.
    #[serde(default = "default_true")]
    pub security_headers: bool,

    /// **Upstream par hôte**, quand les domaines ne vont pas tous au même
    /// endroit.
    ///
    /// Cas réel : une application sert son API et sa SPA sur un port, mais son
    /// serveur **git** sur un autre. Déléguer « le domaine » proprement, c'est
    /// router `git.<domaine>` vers le serveur git — pas tout vers l'API.
    ///
    /// Un hôte absent de cette table utilise [`CaddyConfig::upstream`].
    #[serde(default)]
    pub upstreams: std::collections::BTreeMap<String, String>,

    /// Directives ajoutées telles quelles dans chaque bloc de site.
    ///
    /// L'échappatoire : un besoin que ce schéma ne couvre pas se dit ici, plutôt
    /// que de forcer le framework à tout prévoir.
    #[serde(default)]
    pub extra_directives: Vec<String>,
}

impl Default for CaddyConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            upstream: default_upstream(),
            email: None,
            tls: CaddyTls::default(),
            security_headers: true,
            upstreams: std::collections::BTreeMap::new(),
            extra_directives: Vec::new(),
        }
    }
}

fn default_upstream() -> String {
    "127.0.0.1:3000".to_string()
}

fn default_true() -> bool {
    true
}

/// Obtention des certificats.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CaddyTls {
    /// ACME (Let's Encrypt) — le défaut de Caddy, rendu explicite.
    #[default]
    Acme,
    /// Autorité interne de Caddy : pour un réseau privé.
    Internal,
    /// Pas de TLS ici : Caddy sert en clair (derrière un autre terminateur).
    Off,
}

impl CaddyConfig {
    /// Le `Caddyfile`, à partir des domaines et de la politique CORS.
    ///
    /// Ce que le framework écrit **décrit** ce qu'il applique : les hôtes sont
    /// ceux de `domains.allowed`, et le CORS y est posé **par l'application**
    /// (le framework le fait), pas par Caddy — sauf si l'application n'a pas
    /// activé CORS, auquel cas le `Caddyfile` le dit en commentaire plutôt que
    /// de laisser croire que c'est réglé.
    pub fn render(&self, domains: &DomainConfig, cors: &CorsConfig) -> String {
        let mut out = String::new();

        out.push_str("# Caddyfile — délégué par this-rs\n");
        out.push_str("#\n");
        out.push_str("# Ce fichier DÉCRIT ce que this-rs applique : les domaines servis et\n");
        out.push_str("# l'upstream. Relisez-le et appliquez-le (`caddy reload`). this-rs ne\n");
        out.push_str("# pilote pas Caddy : la délégation est un texte, pas une automatisation.\n");
        if domains.trust_forwarded_host {
            out.push_str(
                "#\n# `domains.trust_forwarded_host: true` : l'application fait confiance à\n",
            );
            out.push_str("# `X-Forwarded-Host`. C'est cohérent ici parce que Caddy le pose et\n");
            out.push_str("# remplace celui du client.\n");
        }
        out.push('\n');

        if let Some(email) = &self.email {
            out.push_str("{\n");
            out.push_str(&format!("\temail {email}\n"));
            out.push_str("}\n\n");
        }

        // Les sites à écrire : la liste explicite d'abord — `allowed` est un
        // filtre, pas un inventaire de domaines.
        let sites = if !domains.sites.is_empty() {
            domains.sites.clone()
        } else if domains.allowed.is_empty() {
            vec![":443".to_string()]
        } else {
            domains.allowed.clone()
        };

        for site in sites {
            out.push_str(&format!("{site} {{\n"));
            if let Some(email) = &self.email {
                let _ = email;
            }
            match self.tls {
                CaddyTls::Acme => {}
                CaddyTls::Internal => out.push_str("\ttls internal\n"),
                CaddyTls::Off => out.push_str("\ttls off\n"),
            }
            // L'upstream de CE site : le sien s'il en a un, sinon le commun.
            let upstream = self.upstreams.get(&site).unwrap_or(&self.upstream);
            out.push_str(&format!("\treverse_proxy {upstream}\n"));

            if self.security_headers {
                out.push_str("\theader {\n");
                out.push_str(
                    "\t\tStrict-Transport-Security \"max-age=31536000; includeSubDomains\"\n",
                );
                out.push_str("\t\tX-Content-Type-Options \"nosniff\"\n");
                out.push_str("\t\tReferrer-Policy \"strict-origin-when-cross-origin\"\n");
                out.push_str("\t}\n");
            }

            if cors.enabled {
                out.push_str("\t# CORS : posé par this-rs (voir config/server.yaml), pas ici —\n");
                out.push_str(
                    "\t# une seule couche doit décider, sinon les en-têtes se contredisent.\n",
                );
            } else {
                out.push_str(
                    "\t# CORS : NON configuré côté application — aucune origine de navigateur\n",
                );
                out.push_str(
                    "\t# n'est autorisée. Activez-le dans config/server.yaml si besoin.\n",
                );
            }

            for directive in &self.extra_directives {
                out.push_str(&format!("\t{directive}\n"));
            }
            out.push_str("}\n\n");
        }

        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_configuration_changes_nothing() {
        let config = ServerSecurityConfig::default();
        assert!(!config.cors.enabled, "CORS n'est pas posé par défaut");
        assert!(
            !config.domains.guards(),
            "aucune garde de domaine par défaut"
        );
        assert!(config.caddyfile().is_none(), "aucune délégation par défaut");
        // Et sans garde, tout hôte est accepté — le comportement d'avant.
        assert!(config.domains.allows("n-importe-quoi.example"));
    }

    #[test]
    fn the_domain_list_is_closed_once_configured() {
        let domains = DomainConfig {
            allowed: vec!["app.exemple.fr".into(), "*.interne.exemple.fr".into()],
            ..Default::default()
        };
        assert!(domains.allows("app.exemple.fr"));
        assert!(
            domains.allows("APP.Exemple.FR:8443"),
            "casse et port ignorés"
        );
        assert!(domains.allows("a.interne.exemple.fr"), "un niveau de joker");
        assert!(
            !domains.allows("interne.exemple.fr"),
            "le joker exige un label"
        );
        assert!(!domains.allows("a.b.interne.exemple.fr"), "et un seul");
        assert!(!domains.allows("evil.fr"));
        assert!(
            !domains.allows("app.exemple.fr.evil.fr"),
            "pas de suffixe trompeur"
        );
    }

    #[test]
    fn a_domain_can_designate_its_tenant() {
        let tenant = uuid::Uuid::from_u128(7);
        let domains = DomainConfig {
            tenants: [("app.exemple.fr".to_string(), tenant.to_string())]
                .into_iter()
                .collect(),
            subdomain_base: Some("multi.exemple.fr".into()),
            default_tenant: Some(uuid::Uuid::from_u128(9).to_string()),
            ..Default::default()
        };

        assert_eq!(domains.tenant_of("app.exemple.fr"), Some(tenant));
        let via_sub = uuid::Uuid::from_u128(3);
        assert_eq!(
            domains.tenant_of(&format!("{}.multi.exemple.fr", via_sub)),
            Some(via_sub),
            "le sous-domaine porte le locataire"
        );
        assert_eq!(
            domains.tenant_of("autre.exemple.fr"),
            Some(uuid::Uuid::from_u128(9)),
            "sinon, le locataire par défaut"
        );
        assert_eq!(
            domains.tenant_of("pas-un-uuid.multi.exemple.fr"),
            Some(uuid::Uuid::from_u128(9)),
            "un label illisible ne devient pas un locataire : on retombe sur le défaut"
        );
    }

    #[test]
    fn a_broken_tenant_is_refused_at_startup_not_at_first_call() {
        let domains = DomainConfig {
            tenants: [("app.exemple.fr".to_string(), "pas-un-uuid".to_string())]
                .into_iter()
                .collect(),
            ..Default::default()
        };
        let err = domains.validate().expect_err("doit refuser");
        assert!(err.to_string().contains("UUID"), "{err}");
    }

    /// **Le défaut du code et celui du YAML sont le même.** Sinon une
    /// configuration « marche en YAML » et pas par programme, ou l'inverse.
    #[test]
    fn the_programmatic_default_matches_the_yaml_default() {
        let from_code = CorsConfig::default();
        let from_yaml: CorsConfig = serde_yaml::from_str("enabled: true").expect("yaml");
        assert_eq!(from_code.methods, from_yaml.methods);
        assert_eq!(from_code.headers, from_yaml.headers);
        assert!(
            !from_code.methods.is_empty(),
            "des méthodes, pas une liste vide"
        );
        assert!(!from_code.headers.is_empty());
    }

    #[test]
    fn cors_refuses_an_empty_or_impossible_configuration() {
        let enabled_but_empty = CorsConfig {
            enabled: true,
            ..Default::default()
        };
        assert!(enabled_but_empty.validate().is_err(), "aucune origine");

        let star_with_credentials = CorsConfig {
            enabled: true,
            origins: vec!["*".into()],
            credentials: true,
            ..Default::default()
        };
        let err = star_with_credentials
            .validate()
            .expect_err("combinaison refusée par les navigateurs");
        assert!(err.to_string().contains("credentials"), "{err}");

        let ok = CorsConfig {
            enabled: true,
            origins: vec!["https://app.exemple.fr".into()],
            credentials: true,
            ..Default::default()
        };
        assert!(ok.validate().is_ok());
    }

    #[test]
    fn the_yaml_schema_is_readable_and_unknown_keys_are_refused() {
        let yaml = r#"
cors:
  enabled: true
  origins: ["https://app.exemple.fr"]
  credentials: true
  max_age_secs: 600

domains:
  allowed: ["app.exemple.fr"]
  trust_forwarded_host: true
  tenants:
    "app.exemple.fr": "00000000-0000-0000-0000-000000000007"

proxy:
  caddy:
    enabled: true
    upstream: "127.0.0.1:3000"
    email: "ops@exemple.fr"
    tls: internal
"#;
        let config = ServerSecurityConfig::from_yaml(yaml).expect("configuration lisible");
        assert!(config.cors.enabled);
        assert_eq!(config.cors.max_age_secs, Some(600));
        assert_eq!(config.domains.allowed, vec!["app.exemple.fr"]);
        assert!(config.domains.trust_forwarded_host);

        // Une clé inconnue est une faute de frappe, pas un réglage ignoré.
        let typo = ServerSecurityConfig::from_yaml("cors:\n  origin: [\"*\"]\n");
        assert!(
            typo.is_err(),
            "un réglage mal orthographié doit être refusé, sinon il est ignoré en silence"
        );
    }

    #[test]
    fn the_caddyfile_describes_what_the_framework_applies() {
        let config = ServerSecurityConfig {
            cors: CorsConfig {
                enabled: true,
                origins: vec!["https://app.exemple.fr".into()],
                ..Default::default()
            },
            domains: DomainConfig {
                allowed: vec!["app.exemple.fr".into()],
                trust_forwarded_host: true,
                ..Default::default()
            },
            proxy: ProxyConfig {
                caddy: CaddyConfig {
                    enabled: true,
                    upstream: "127.0.0.1:8080".into(),
                    email: Some("ops@exemple.fr".into()),
                    tls: CaddyTls::Internal,
                    security_headers: true,
                    upstreams: std::collections::BTreeMap::new(),
                    extra_directives: vec!["encode gzip".into()],
                },
            },
            ..Default::default()
        };

        let caddyfile = config.caddyfile().expect("délégation demandée");
        assert!(caddyfile.contains("email ops@exemple.fr"), "{caddyfile}");
        assert!(caddyfile.contains("app.exemple.fr {"), "{caddyfile}");
        assert!(
            caddyfile.contains("reverse_proxy 127.0.0.1:8080"),
            "l'upstream doit être celui de la config : {caddyfile}"
        );
        assert!(caddyfile.contains("tls internal"), "{caddyfile}");
        assert!(
            caddyfile.contains("Strict-Transport-Security"),
            "les en-têtes de sécurité sont posés par défaut : {caddyfile}"
        );
        assert!(caddyfile.contains("encode gzip"), "échappatoire honorée");
        assert!(
            caddyfile.contains("posé par this-rs"),
            "le fichier doit dire QUI pose le CORS : {caddyfile}"
        );
        assert!(
            caddyfile.contains("DÉCRIT"),
            "le fichier doit dire que c'est une description, pas un pilotage : {caddyfile}"
        );
    }
}
