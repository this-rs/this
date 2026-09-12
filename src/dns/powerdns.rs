//! **Client PowerDNS** — l'API HTTP du serveur autoritaire.
//!
//! # Ce que ce module fait, et ce qu'il ne fait pas
//!
//! Il **lit** l'état d'une zone, **applique** un [`DnsPlan`] et demande la
//! rectification. Il ne décide de rien : le plan vient de [`super::plan`], qui
//! est une fonction pure, relisible, et qui ne touche jamais aux enregistrements
//! hors périmètre (MX, TXT, NS…).
//!
//! # La clé d'API ne vient pas du fichier
//!
//! `config/server.yaml` se versionne ; une clé d'API non. Le schéma porte donc le
//! **nom** de la variable d'environnement (`api_key_env`), et la valeur est lue à
//! l'exécution. La clé n'est **jamais** journalisée ni incluse dans un message
//! d'erreur — un secret dans une trace est un secret publié.

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use super::{DnsPlan, DnsRecord, ensure_dot};

/// Configuration de la délégation DNS (`dns:` dans `config/server.yaml`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PdnsConfig {
    /// URL de base de l'API (`http://127.0.0.1:8081`).
    pub api_url: String,

    /// **Nom** de la variable d'environnement qui porte la clé d'API.
    ///
    /// Jamais la clé elle-même : ce fichier se versionne.
    #[serde(default = "default_key_env")]
    pub api_key_env: String,

    /// Identifiant du serveur dans l'API (`localhost` par défaut).
    #[serde(default = "default_server_id")]
    pub server_id: String,

    /// La zone gérée, avec ou sans point final (`exemple.fr`).
    pub zone: String,

    /// Durée de vie posée sur les enregistrements créés.
    #[serde(default = "default_ttl")]
    pub ttl: u32,

    /// La cible des enregistrements dérivés des sites.
    pub target: String,

    /// Le type des enregistrements dérivés (`A`, `AAAA` ou `CNAME`).
    #[serde(default = "default_record_type")]
    pub record_type: String,

    /// Supprimer les enregistrements **gérés** qui ne sont plus déclarés ?
    ///
    /// `false` par défaut : un enregistrement oublié est **signalé**, pas
    /// effacé. Un framework ne doit pas pouvoir vider une zone par omission.
    #[serde(default)]
    pub prune: bool,

    /// Rectifier la zone après un changement (obligatoire pour une zone
    /// autoritaire dont on sert les NS).
    #[serde(default = "default_true")]
    pub rectify: bool,
}

fn default_key_env() -> String {
    "PDNS_API_KEY".to_string()
}

fn default_server_id() -> String {
    "localhost".to_string()
}

fn default_ttl() -> u32 {
    300
}

fn default_record_type() -> String {
    "A".to_string()
}

fn default_true() -> bool {
    true
}

impl PdnsConfig {
    /// La clé d'API, lue dans l'environnement.
    ///
    /// Refus **explicite** si la variable est absente ou vide : mieux vaut un
    /// démarrage qui échoue qu'un appel d'API sans authentification.
    pub fn api_key(&self) -> Result<String> {
        match std::env::var(&self.api_key_env) {
            Ok(key) if !key.trim().is_empty() => Ok(key),
            _ => bail!(
                "clé d'API PowerDNS absente : la variable d'environnement `{}` est vide ou \
                 non définie (elle n'est JAMAIS lue depuis le fichier de configuration, qui \
                 se versionne)",
                self.api_key_env
            ),
        }
    }

    /// L'identifiant de zone tel que l'API le veut (nom pleinement qualifié).
    pub fn zone_id(&self) -> String {
        ensure_dot(&self.zone)
    }

    /// `http://127.0.0.1:8081/api/v1/servers/localhost/zones/exemple.fr.`
    fn zone_url(&self) -> String {
        format!(
            "{}/api/v1/servers/{}/zones/{}",
            self.api_url.trim_end_matches('/'),
            self.server_id,
            self.zone_id()
        )
    }

    fn zones_url(&self) -> String {
        format!(
            "{}/api/v1/servers/{}/zones",
            self.api_url.trim_end_matches('/'),
            self.server_id
        )
    }
}

#[cfg(feature = "powerdns")]
/// Ce qu'un `apply` a réellement changé.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Applied {
    /// Enregistrements créés ou remplacés.
    pub replaced: usize,
    /// Enregistrements supprimés.
    pub deleted: usize,
    /// La zone a-t-elle été rectifiée ?
    pub rectified: bool,
}

#[cfg(feature = "powerdns")]
/// Le client.
#[derive(Clone)]
pub struct PdnsClient {
    http: reqwest::Client,
    config: PdnsConfig,
    api_key: std::sync::Arc<String>,
}

#[cfg(feature = "powerdns")]
impl PdnsClient {
    /// Construit le client — la clé est lue **maintenant**, pas à chaque appel.
    pub fn new(config: PdnsConfig) -> Result<Self> {
        let api_key = config.api_key()?;
        Ok(Self {
            http: reqwest::Client::new(),
            config,
            api_key: std::sync::Arc::new(api_key),
        })
    }

    /// La configuration utilisée.
    pub fn config(&self) -> &PdnsConfig {
        &self.config
    }

    /// La zone existe-t-elle ?
    pub async fn zone_exists(&self) -> Result<bool> {
        let response = self
            .http
            .get(self.config.zone_url())
            .header("X-API-Key", self.api_key.as_str())
            .send()
            .await
            .context("appel PowerDNS (lecture de zone)")?;

        match response.status().as_u16() {
            200 => Ok(true),
            404 => Ok(false),
            other => bail!("PowerDNS a répondu {other} à la lecture de la zone"),
        }
    }

    /// **L'état de la zone** : les enregistrements tels que le serveur les
    /// publie.
    pub async fn current_records(&self) -> Result<Vec<DnsRecord>> {
        let response = self
            .http
            .get(self.config.zone_url())
            .header("X-API-Key", self.api_key.as_str())
            .send()
            .await
            .context("appel PowerDNS (lecture de zone)")?;

        if response.status().as_u16() != 200 {
            bail!(
                "PowerDNS a répondu {} à la lecture de la zone {}",
                response.status(),
                self.config.zone_id()
            );
        }

        let zone: ZoneResponse = response.json().await.context("réponse de zone illisible")?;
        Ok(zone
            .rrsets
            .into_iter()
            .flat_map(|rrset| {
                let ttl = rrset.ttl;
                rrset.records.into_iter().map(move |record| DnsRecord {
                    name: rrset.name.clone(),
                    record_type: rrset.record_type.clone(),
                    content: record.content,
                    ttl,
                })
            })
            .collect())
    }

    /// **Applique un plan.** Un `REPLACE` couvre création et modification ; un
    /// `DELETE` ne part que si le plan en contient (donc jamais sans `prune`).
    pub async fn apply(&self, plan: &DnsPlan) -> Result<Applied> {
        let mut rrsets: Vec<RrsetChange> = Vec::new();

        for record in plan.creates.iter().chain(plan.updates.iter()) {
            rrsets.push(RrsetChange {
                name: record.name.clone(),
                record_type: record.record_type.clone(),
                ttl: record.ttl,
                changetype: "REPLACE".to_string(),
                records: vec![RecordContent {
                    content: record.content.clone(),
                    disabled: false,
                }],
            });
        }
        for record in &plan.deletes {
            rrsets.push(RrsetChange {
                name: record.name.clone(),
                record_type: record.record_type.clone(),
                ttl: record.ttl,
                changetype: "DELETE".to_string(),
                records: Vec::new(),
            });
        }

        if rrsets.is_empty() {
            return Ok(Applied::default());
        }

        let replaced = plan.creates.len() + plan.updates.len();
        let deleted = plan.deletes.len();

        let response = self
            .http
            .patch(self.config.zone_url())
            .header("X-API-Key", self.api_key.as_str())
            .json(&serde_json::json!({ "rrsets": rrsets }))
            .send()
            .await
            .context("appel PowerDNS (application des changements)")?;

        if !response.status().is_success() {
            // Le corps peut nommer l'erreur ; la clé, jamais.
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            bail!(
                "PowerDNS a refusé les changements ({status}) : {}",
                body.chars().take(400).collect::<String>()
            );
        }

        let mut rectified = false;
        if self.config.rectify {
            let response = self
                .http
                .put(format!("{}/rectify", self.config.zone_url()))
                .header("X-API-Key", self.api_key.as_str())
                .send()
                .await
                .context("appel PowerDNS (rectification)")?;
            if !response.status().is_success() {
                bail!(
                    "les changements ont été écrits mais la rectification a échoué ({}) — \
                     la zone peut ne pas être servie avant une rectification manuelle",
                    response.status()
                );
            }
            rectified = true;
        }

        Ok(Applied {
            replaced,
            deleted,
            rectified,
        })
    }

    /// **Crée la zone** si elle n'existe pas.
    ///
    /// Rend `true` si elle a été créée. Ne touche jamais une zone existante : la
    /// recréer effacerait son contenu, et un framework n'a pas à décider cela.
    pub async fn ensure_zone(&self, nameservers: &[String]) -> Result<bool> {
        if self.zone_exists().await? {
            return Ok(false);
        }
        if nameservers.is_empty() {
            bail!(
                "création de la zone {} refusée : aucun serveur de noms fourni (il doit \
                 correspondre à la délégation du registraire, sinon la zone ne sera pas servie)",
                self.config.zone_id()
            );
        }

        let response = self
            .http
            .post(self.config.zones_url())
            .header("X-API-Key", self.api_key.as_str())
            .json(&serde_json::json!({
                "name": self.config.zone_id(),
                "kind": "Native",
                "nameservers": nameservers,
            }))
            .send()
            .await
            .context("appel PowerDNS (création de zone)")?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            bail!(
                "PowerDNS a refusé la création de la zone ({status}) : {}",
                body.chars().take(400).collect::<String>()
            );
        }
        Ok(true)
    }
}

#[cfg(feature = "powerdns")]
// ── Les formes de l'API PowerDNS ──────────────────────────────────────────
#[derive(Debug, Deserialize)]
struct ZoneResponse {
    #[serde(default)]
    rrsets: Vec<RrsetResponse>,
}

#[derive(Debug, Deserialize)]
struct RrsetResponse {
    name: String,
    #[serde(rename = "type")]
    record_type: String,
    #[serde(default = "default_ttl")]
    ttl: u32,
    #[serde(default)]
    records: Vec<RecordContent>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
struct RecordContent {
    content: String,
    #[serde(default)]
    disabled: bool,
}

#[derive(Debug, Serialize)]
struct RrsetChange {
    name: String,
    #[serde(rename = "type")]
    record_type: String,
    ttl: u32,
    changetype: String,
    records: Vec<RecordContent>,
}
