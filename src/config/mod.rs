//! Configuration loading and management

pub mod auth;
pub mod events;
pub mod server;
pub mod sinks;

use crate::core::LinkDefinition;
use anyhow::Result;
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

pub use auth::AuthConfig;
pub use events::*;
pub use server::{
    CaddyConfig, CaddyTls, CorsConfig, DomainConfig, ProxyConfig, ServerSecurityConfig,
};
pub use sinks::*;

/// Authorization configuration for an entity
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EntityAuthConfig {
    /// Policy for listing entities (GET /{entities})
    #[serde(default = "default_auth_policy")]
    pub list: String,

    /// Policy for getting a single entity (GET /{entities}/{id})
    #[serde(default = "default_auth_policy")]
    pub get: String,

    /// Policy for creating an entity (POST /{entities})
    #[serde(default = "default_auth_policy")]
    pub create: String,

    /// Policy for updating an entity (PUT /{entities}/{id})
    #[serde(default = "default_auth_policy")]
    pub update: String,

    /// Policy for deleting an entity (DELETE /{entities}/{id})
    #[serde(default = "default_auth_policy")]
    pub delete: String,

    /// Policy for listing links (GET /{entities}/{id}/{link_route})
    #[serde(default = "default_auth_policy")]
    pub list_links: String,

    /// Policy for creating links (POST /{entities}/{id}/{link_type}/{target_type}/{target_id})
    #[serde(default = "default_auth_policy")]
    pub create_link: String,

    /// Policy for deleting links (DELETE /{entities}/{id}/{link_type}/{target_type}/{target_id})
    #[serde(default = "default_auth_policy")]
    pub delete_link: String,

    /// Policy for updating a link (PUT /{entities}/{id}/{link_route}/{target_id})
    ///
    /// Ajouté en auditant la chaîne : le routeur de liens sert un `PUT` à quatre
    /// segments (`update_link`), et aucune opération ne lui correspondait — la
    /// requête était donc **abstenue**, donc non jugée par les politiques.
    /// `#[serde(default)]` : une configuration écrite avant ce champ continue de
    /// se lire, avec la politique par défaut.
    #[serde(default = "default_auth_policy")]
    pub update_link: String,
}

/// Same serialized form (the config types do not all implement `PartialEq`).
fn same<T: Serialize>(a: &T, b: &T) -> bool {
    serde_json::to_value(a).ok() == serde_json::to_value(b).ok()
}

/// A descriptor policy in the grammar the legacy middleware reads (`AuthPolicy::parse_policy`):
/// `all:a,b`, `any:a,b`, `not:a`, `role:x`. `deny` has no legacy word: it becomes
/// `not:public` (always false). A composite nested inside another composite cannot be
/// written in the flat legacy form: the canonical form is kept, which the legacy parser
/// does not know and therefore REFUSES (fail-closed).
pub fn legacy_policy(p: &crate::descriptor::PolicyExpr) -> String {
    use crate::descriptor::PolicyExpr as P;
    let flat = |v: &[P]| {
        v.iter()
            .all(|e| !matches!(e, P::And(_) | P::Or(_) | P::Not(_)))
    };
    match p {
        P::Deny => "not:public".into(),
        P::HasRole(r) if r.len() == 1 => format!("role:{}", r[0]),
        P::HasRole(r) => format!(
            "any:{}",
            r.iter()
                .map(|x| format!("role:{x}"))
                .collect::<Vec<_>>()
                .join(",")
        ),
        P::And(v) if flat(v) => format!(
            "all:{}",
            v.iter().map(legacy_policy).collect::<Vec<_>>().join(",")
        ),
        P::Or(v) if flat(v) => format!(
            "any:{}",
            v.iter().map(legacy_policy).collect::<Vec<_>>().join(",")
        ),
        P::Not(e) if flat(std::slice::from_ref(e)) => format!("not:{}", legacy_policy(e)),
        other => other.to_string(),
    }
}

fn default_auth_policy() -> String {
    "authenticated".to_string()
}

impl Default for EntityAuthConfig {
    fn default() -> Self {
        Self {
            list: default_auth_policy(),
            get: default_auth_policy(),
            create: default_auth_policy(),
            update: default_auth_policy(),
            delete: default_auth_policy(),
            list_links: default_auth_policy(),
            create_link: default_auth_policy(),
            delete_link: default_auth_policy(),
            update_link: default_auth_policy(),
        }
    }
}

/// Configuration for an entity type
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EntityConfig {
    /// Singular form (e.g., "user", "company")
    pub singular: String,

    /// Plural form (e.g., "users", "companies")
    pub plural: String,

    /// Authorization configuration
    #[serde(default)]
    pub auth: EntityAuthConfig,
}

/// Validation rule for a link type
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValidationRule {
    /// Source entity type
    pub source: String,

    /// Allowed target entity types
    pub targets: Vec<String>,
}

/// Complete configuration for the links system
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LinksConfig {
    /// List of entity configurations
    pub entities: Vec<EntityConfig>,

    /// List of link definitions
    pub links: Vec<LinkDefinition>,

    /// Optional validation rules (link_type -> rules)
    #[serde(default)]
    pub validation_rules: Option<HashMap<String, Vec<ValidationRule>>>,

    /// Optional event flow configuration (backend, flows, consumers)
    #[serde(default)]
    pub events: Option<EventsConfig>,

    /// Optional sink configurations (notification destinations)
    #[serde(default)]
    pub sinks: Option<Vec<SinkConfig>>,

    /// Optional authentication & authorization configuration
    #[serde(default)]
    pub auth: Option<AuthConfig>,
}

impl LinksConfig {
    /// Load configuration from a YAML file
    pub fn from_yaml_file(path: &str) -> Result<Self> {
        let content = std::fs::read_to_string(path)?;
        let config: Self = serde_yaml::from_str(&content)?;
        Ok(config)
    }

    /// Load configuration from a YAML string
    pub fn from_yaml_str(yaml: &str) -> Result<Self> {
        let config: Self = serde_yaml::from_str(yaml)?;
        Ok(config)
    }

    /// Merge multiple configurations into one
    ///
    /// Rules:
    /// - Entities: Combined from all configs, duplicates (by singular name) use last definition
    /// - Links: Combined from all configs, duplicates (by link_type+source+target) use last definition
    /// - Validation rules: Merged by link_type, rules combined for each link type
    pub fn merge(configs: Vec<LinksConfig>) -> Self {
        if configs.is_empty() {
            return Self {
                entities: vec![],
                links: vec![],
                validation_rules: None,
                events: None,
                sinks: None,
                auth: None,
            };
        }

        if configs.len() == 1 {
            return configs.into_iter().next().unwrap();
        }

        let mut entities_map: IndexMap<String, EntityConfig> = IndexMap::new();
        let mut links_map: IndexMap<(String, String, String), LinkDefinition> = IndexMap::new();
        let mut validation_rules_map: HashMap<String, Vec<ValidationRule>> = HashMap::new();

        // Merge entities (last one wins for duplicates)
        for config in &configs {
            for entity in &config.entities {
                entities_map.insert(entity.singular.clone(), entity.clone());
            }
        }

        // Merge links (last one wins for duplicates)
        for config in &configs {
            for link in &config.links {
                let key = (
                    link.link_type.clone(),
                    link.source_type.clone(),
                    link.target_type.clone(),
                );
                links_map.insert(key, link.clone());
            }
        }

        // Merge validation rules (combine rules for same link_type)
        for config in &configs {
            if let Some(rules) = &config.validation_rules {
                for (link_type, link_rules) in rules {
                    validation_rules_map
                        .entry(link_type.clone())
                        .or_default()
                        .extend(link_rules.clone());
                }
            }
        }

        // Merge events: backend last-wins, flows are concatenated (with duplicate warning)
        let mut merged_events: Option<EventsConfig> = None;
        for config in &configs {
            if let Some(events) = &config.events {
                if let Some(ref mut existing) = merged_events {
                    // Backend: last-wins (consistent with entities/links merge behavior)
                    existing.backend = events.backend.clone();
                    existing.flows.extend(events.flows.clone());
                    existing.consumers.extend(events.consumers.clone());
                } else {
                    merged_events = Some(events.clone());
                }
            }
        }

        // Detect duplicate flow names and warn
        if let Some(ref events) = merged_events {
            let mut seen_names = std::collections::HashSet::new();
            for flow in &events.flows {
                if !seen_names.insert(&flow.name) {
                    tracing::warn!(
                        flow_name = %flow.name,
                        "config merge: duplicate flow name detected — \
                         later definition will shadow earlier one at runtime"
                    );
                }
            }
        }

        // Merge auth: last-wins (only one auth config should be active)
        let mut merged_auth: Option<AuthConfig> = None;
        for config in &configs {
            if config.auth.is_some() {
                merged_auth = config.auth.clone();
            }
        }

        // Merge sinks: deduplicate by name (last wins), preserving insertion order
        let mut sinks_map: IndexMap<String, SinkConfig> = IndexMap::new();
        for config in &configs {
            if let Some(sinks) = &config.sinks {
                for sink in sinks {
                    sinks_map.insert(sink.name.clone(), sink.clone());
                }
            }
        }
        let merged_sinks = if sinks_map.is_empty() {
            None
        } else {
            Some(sinks_map.into_values().collect())
        };

        // Convert back to vectors
        let entities: Vec<EntityConfig> = entities_map.into_values().collect();
        let links: Vec<LinkDefinition> = links_map.into_values().collect();
        let validation_rules = if validation_rules_map.is_empty() {
            None
        } else {
            Some(validation_rules_map)
        };

        Self {
            entities,
            links,
            validation_rules,
            events: merged_events,
            sinks: merged_sinks,
            auth: merged_auth,
        }
    }

    /// Merge configurations **refusing** conflicts instead of last-wins.
    ///
    /// Two configurations may repeat an IDENTICAL declaration (same entity, same link,
    /// same sink): that is not a conflict. Every DIFFERING redeclaration is reported, all
    /// of them (not only the first), each as `this.module.config_conflict {what, key}`:
    /// - `entity` (by singular) and `plural` (two entities on the same plural);
    /// - `link` (by `link_type`, `source_type`, `target_type`) and `link_route` (two links
    ///   with the same route name on the same entity);
    /// - `sink` (by name), `flow` (by name), `events.backend`, `auth`.
    ///
    /// Validation rules and consumers are additive (no identity to conflict on). On
    /// success the result equals [`LinksConfig::merge`].
    pub fn merge_strict(
        configs: Vec<LinksConfig>,
    ) -> std::result::Result<Self, Vec<crate::descriptor::ContractError>> {
        use crate::descriptor::ContractError;
        use crate::descriptor::framework::codes;
        let conflict = |what: &str, key: String| {
            ContractError::framework(codes::MODULE_CONFIG_CONFLICT)
                .with_param("what", what)
                .with_param("key", key)
        };
        let mut errors = Vec::new();

        let mut entities: IndexMap<&str, &EntityConfig> = IndexMap::new();
        let mut plurals: HashMap<&str, &str> = HashMap::new();
        let mut links: IndexMap<(&str, &str, &str), &LinkDefinition> = IndexMap::new();
        let mut routes: HashMap<(&str, &str), (&str, &str, &str)> = HashMap::new();
        let mut sinks: HashMap<&str, &SinkConfig> = HashMap::new();
        let mut flows: HashMap<&str, &FlowConfig> = HashMap::new();
        let mut backend: Option<&BackendConfig> = None;
        let mut auth: Option<&AuthConfig> = None;

        for config in &configs {
            for e in &config.entities {
                match entities.get(e.singular.as_str()) {
                    Some(prev) if !same(*prev, e) => {
                        errors.push(conflict("entity", e.singular.clone()))
                    }
                    Some(_) => {}
                    None => {
                        entities.insert(&e.singular, e);
                    }
                }
                match plurals.get(e.plural.as_str()) {
                    Some(owner) if *owner != e.singular => errors.push(conflict(
                        "plural",
                        format!("{} ({owner}, {})", e.plural, e.singular),
                    )),
                    Some(_) => {}
                    None => {
                        plurals.insert(&e.plural, &e.singular);
                    }
                }
            }
            for l in &config.links {
                let key = (
                    l.link_type.as_str(),
                    l.source_type.as_str(),
                    l.target_type.as_str(),
                );
                match links.get(&key) {
                    Some(prev) if !same(*prev, l) => {
                        errors.push(conflict("link", format!("{}:{}->{}", key.0, key.1, key.2)))
                    }
                    Some(_) => continue,
                    None => {
                        links.insert(key, l);
                    }
                }
                for (entity, route) in [
                    (l.source_type.as_str(), l.forward_route_name.as_str()),
                    (l.target_type.as_str(), l.reverse_route_name.as_str()),
                ] {
                    match routes.get(&(entity, route)) {
                        Some(owner) if *owner != key => {
                            errors.push(conflict("link_route", format!("{entity}/{route}")))
                        }
                        Some(_) => {}
                        None => {
                            routes.insert((entity, route), key);
                        }
                    }
                }
            }
            for s in config.sinks.iter().flatten() {
                match sinks.get(s.name.as_str()) {
                    Some(prev) if !same(*prev, s) => errors.push(conflict("sink", s.name.clone())),
                    Some(_) => {}
                    None => {
                        sinks.insert(&s.name, s);
                    }
                }
            }
            if let Some(ev) = &config.events {
                match backend {
                    Some(prev) if !same(prev, &ev.backend) => {
                        errors.push(conflict("events.backend", String::new()))
                    }
                    Some(_) => {}
                    None => backend = Some(&ev.backend),
                }
                for f in &ev.flows {
                    match flows.get(f.name.as_str()) {
                        Some(prev) if !same(*prev, f) => {
                            errors.push(conflict("flow", f.name.clone()))
                        }
                        Some(_) => {}
                        None => {
                            flows.insert(&f.name, f);
                        }
                    }
                }
            }
            if let Some(a) = &config.auth {
                match auth {
                    Some(prev) if !same(prev, a) => errors.push(conflict("auth", String::new())),
                    Some(_) => {}
                    None => auth = Some(a),
                }
            }
        }
        if !errors.is_empty() {
            return Err(errors);
        }
        // No conflict: the legacy merge is now order-independent for every keyed item,
        // except identical flows that it would concatenate twice: dedup them.
        let mut merged = Self::merge(configs);
        if let Some(ev) = merged.events.as_mut() {
            let mut seen = std::collections::HashSet::new();
            ev.flows.retain(|f| seen.insert(f.name.clone()));
        }
        Ok(merged)
    }

    /// The links configuration DERIVED from a module descriptor (entities with their
    /// plural and policies, links with their routes and policies). The descriptor is the
    /// owner; nothing else is invented. Host-level keys (`events`, `sinks`, `auth`,
    /// `validation_rules`) are absent: they belong to the host (see
    /// [`LinksConfig::overlay_descriptor`]).
    pub fn from_descriptor(d: &crate::descriptor::ModuleDescriptor) -> Self {
        Self {
            entities: vec![],
            links: vec![],
            validation_rules: None,
            events: None,
            sinks: None,
            auth: None,
        }
        .overlay_descriptor(d)
    }

    /// Merge a descriptor INTO this configuration, by key: an entity (by singular) or a
    /// link (by type, source, target) of the descriptor replaces the YAML one, everything
    /// else (other entities and links, `events`, `sinks`, `auth`, `validation_rules`) is
    /// kept untouched. This is what keeps a `links.yaml` written by hand (or by an older
    /// CLI) from losing keys when its entities become descriptor-owned.
    pub fn overlay_descriptor(mut self, d: &crate::descriptor::ModuleDescriptor) -> Self {
        use crate::descriptor::Operation as Op;
        for e in &d.entities {
            let p = |op: Op| legacy_policy(&e.permissions.effective(&op));
            let cfg = EntityConfig {
                singular: e.name.clone(),
                plural: e.plural.clone(),
                auth: EntityAuthConfig {
                    list: p(Op::List),
                    get: p(Op::Get),
                    create: p(Op::Create),
                    update: p(Op::Update),
                    delete: p(Op::Delete),
                    list_links: p(Op::ListLinks),
                    create_link: p(Op::CreateLink),
                    delete_link: p(Op::DeleteLink),
                    update_link: p(Op::UpdateLink),
                },
            };
            match self.entities.iter_mut().find(|x| x.singular == e.name) {
                Some(slot) => *slot = cfg,
                None => self.entities.push(cfg),
            }
        }
        for l in &d.links {
            let scope = d.entity(&l.source);
            let auth = (!l.permissions.is_empty()).then(|| {
                let p = |op: Op| legacy_policy(&l.effective_policy(scope, &op));
                crate::core::link::LinkAuthConfig {
                    list: p(Op::ListLinks),
                    get: p(Op::GetLink),
                    create: p(Op::CreateLink),
                    update: p(Op::UpdateLink),
                    delete: p(Op::DeleteLink),
                }
            });
            let def = LinkDefinition {
                link_type: l.link_type.clone(),
                source_type: l.source.clone(),
                target_type: l.target.clone(),
                forward_route_name: l.forward_route.clone(),
                reverse_route_name: l.reverse_route.clone(),
                description: l.description.clone(),
                required_fields: {
                    let req: Vec<String> = l
                        .fields
                        .iter()
                        .filter(|f| f.required_on(crate::descriptor::Applies::Create))
                        .map(|f| f.name.clone())
                        .collect();
                    (!req.is_empty()).then_some(req)
                },
                auth,
            };
            match self.links.iter_mut().find(|x| {
                x.link_type == def.link_type
                    && x.source_type == def.source_type
                    && x.target_type == def.target_type
            }) {
                Some(slot) => *slot = def,
                None => self.links.push(def),
            }
        }
        self
    }

    /// Validate if a link combination is allowed
    ///
    /// If no validation rules are defined, all combinations are allowed (permissive mode)
    pub fn is_valid_link(&self, link_type: &str, source_type: &str, target_type: &str) -> bool {
        // If no validation rules, accept everything
        let Some(rules) = &self.validation_rules else {
            return true;
        };

        // Check if there are rules for this link type
        let Some(link_rules) = rules.get(link_type) else {
            return true; // No rules for this link type, accept
        };

        // Check if the combination is in the rules
        link_rules.iter().any(|rule| {
            rule.source == source_type && rule.targets.contains(&target_type.to_string())
        })
    }

    /// Find a link definition
    pub fn find_link_definition(
        &self,
        link_type: &str,
        source_type: &str,
        target_type: &str,
    ) -> Option<&LinkDefinition> {
        self.links.iter().find(|def| {
            def.link_type == link_type
                && def.source_type == source_type
                && def.target_type == target_type
        })
    }

    /// Create a default configuration for testing
    pub fn default_config() -> Self {
        Self {
            entities: vec![
                EntityConfig {
                    singular: "user".to_string(),
                    plural: "users".to_string(),
                    auth: EntityAuthConfig::default(),
                },
                EntityConfig {
                    singular: "company".to_string(),
                    plural: "companies".to_string(),
                    auth: EntityAuthConfig::default(),
                },
                EntityConfig {
                    singular: "car".to_string(),
                    plural: "cars".to_string(),
                    auth: EntityAuthConfig::default(),
                },
            ],
            links: vec![
                LinkDefinition {
                    link_type: "owner".to_string(),
                    source_type: "user".to_string(),
                    target_type: "car".to_string(),
                    forward_route_name: "cars-owned".to_string(),
                    reverse_route_name: "users-owners".to_string(),
                    description: Some("User owns a car".to_string()),
                    required_fields: None,
                    auth: None,
                },
                LinkDefinition {
                    link_type: "driver".to_string(),
                    source_type: "user".to_string(),
                    target_type: "car".to_string(),
                    forward_route_name: "cars-driven".to_string(),
                    reverse_route_name: "users-drivers".to_string(),
                    description: Some("User drives a car".to_string()),
                    required_fields: None,
                    auth: None,
                },
                LinkDefinition {
                    link_type: "worker".to_string(),
                    source_type: "user".to_string(),
                    target_type: "company".to_string(),
                    forward_route_name: "companies-work".to_string(),
                    reverse_route_name: "users-workers".to_string(),
                    description: Some("User works at a company".to_string()),
                    required_fields: Some(vec!["role".to_string()]),
                    auth: None,
                },
            ],
            validation_rules: None,
            events: None,
            sinks: None,
            auth: None,
        }
    }

    /// Get the effective auth config (or default if absent)
    pub fn auth_config(&self) -> AuthConfig {
        self.auth.clone().unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = LinksConfig::default_config();

        assert_eq!(config.entities.len(), 3);
        assert_eq!(config.links.len(), 3);
    }

    #[test]
    fn test_yaml_serialization() {
        let config = LinksConfig::default_config();
        let yaml = serde_yaml::to_string(&config).unwrap();

        // Should be able to parse it back
        let parsed = LinksConfig::from_yaml_str(&yaml).unwrap();
        assert_eq!(parsed.entities.len(), config.entities.len());
        assert_eq!(parsed.links.len(), config.links.len());
    }

    #[test]
    fn test_link_auth_config_parsing() {
        let yaml = r#"
entities:
  - singular: order
    plural: orders
  - singular: invoice
    plural: invoices

links:
  - link_type: has_invoice
    source_type: order
    target_type: invoice
    forward_route_name: invoices
    reverse_route_name: order
    auth:
      list: authenticated
      create: service_only
      delete: admin_only
"#;

        let config = LinksConfig::from_yaml_str(yaml).unwrap();
        assert_eq!(config.links.len(), 1);

        let link_def = &config.links[0];
        assert!(link_def.auth.is_some());

        let auth = link_def.auth.as_ref().unwrap();
        assert_eq!(auth.list, "authenticated");
        assert_eq!(auth.create, "service_only");
        assert_eq!(auth.delete, "admin_only");
    }

    #[test]
    fn test_link_without_auth_config() {
        let yaml = r#"
entities:
  - singular: invoice
    plural: invoices
  - singular: payment
    plural: payments

links:
  - link_type: payment
    source_type: invoice
    target_type: payment
    forward_route_name: payments
    reverse_route_name: invoice
"#;

        let config = LinksConfig::from_yaml_str(yaml).unwrap();
        assert_eq!(config.links.len(), 1);

        let link_def = &config.links[0];
        assert!(link_def.auth.is_none());
    }

    #[test]
    fn test_mixed_link_auth_configs() {
        let yaml = r#"
entities:
  - singular: order
    plural: orders
  - singular: invoice
    plural: invoices
  - singular: payment
    plural: payments

links:
  - link_type: has_invoice
    source_type: order
    target_type: invoice
    forward_route_name: invoices
    reverse_route_name: order
    auth:
      list: authenticated
      create: service_only
      delete: admin_only
  
  - link_type: payment
    source_type: invoice
    target_type: payment
    forward_route_name: payments
    reverse_route_name: invoice
"#;

        let config = LinksConfig::from_yaml_str(yaml).unwrap();
        assert_eq!(config.links.len(), 2);

        // First link has auth
        assert!(config.links[0].auth.is_some());
        let auth1 = config.links[0].auth.as_ref().unwrap();
        assert_eq!(auth1.create, "service_only");

        // Second link has no auth
        assert!(config.links[1].auth.is_none());
    }

    #[test]
    fn test_merge_empty() {
        let merged = LinksConfig::merge(vec![]);
        assert_eq!(merged.entities.len(), 0);
        assert_eq!(merged.links.len(), 0);
    }

    #[test]
    fn test_merge_single() {
        let config = LinksConfig::default_config();
        let merged = LinksConfig::merge(vec![config.clone()]);
        assert_eq!(merged.entities.len(), config.entities.len());
        assert_eq!(merged.links.len(), config.links.len());
    }

    #[test]
    fn test_merge_disjoint_configs() {
        let config1 = LinksConfig {
            entities: vec![EntityConfig {
                singular: "order".to_string(),
                plural: "orders".to_string(),
                auth: EntityAuthConfig::default(),
            }],
            links: vec![],
            validation_rules: None,
            events: None,
            sinks: None,
            auth: None,
        };

        let config2 = LinksConfig {
            entities: vec![EntityConfig {
                singular: "invoice".to_string(),
                plural: "invoices".to_string(),
                auth: EntityAuthConfig::default(),
            }],
            links: vec![],
            validation_rules: None,
            events: None,
            sinks: None,
            auth: None,
        };

        let merged = LinksConfig::merge(vec![config1, config2]);
        assert_eq!(merged.entities.len(), 2);
    }

    #[test]
    fn test_merge_overlapping_entities() {
        let auth1 = EntityAuthConfig {
            list: "public".to_string(),
            ..Default::default()
        };

        let config1 = LinksConfig {
            entities: vec![EntityConfig {
                singular: "user".to_string(),
                plural: "users".to_string(),
                auth: auth1,
            }],
            links: vec![],
            validation_rules: None,
            events: None,
            sinks: None,
            auth: None,
        };

        let auth2 = EntityAuthConfig {
            list: "authenticated".to_string(),
            ..Default::default()
        };

        let config2 = LinksConfig {
            entities: vec![EntityConfig {
                singular: "user".to_string(),
                plural: "users".to_string(),
                auth: auth2,
            }],
            links: vec![],
            validation_rules: None,
            events: None,
            sinks: None,
            auth: None,
        };

        let merged = LinksConfig::merge(vec![config1, config2]);

        // Should have only 1 entity (last wins)
        assert_eq!(merged.entities.len(), 1);
        assert_eq!(merged.entities[0].auth.list, "authenticated");
    }

    #[test]
    fn test_merge_validation_rules() {
        let mut rules1 = HashMap::new();
        rules1.insert(
            "works_at".to_string(),
            vec![ValidationRule {
                source: "user".to_string(),
                targets: vec!["company".to_string()],
            }],
        );

        let config1 = LinksConfig {
            entities: vec![],
            links: vec![],
            validation_rules: Some(rules1),
            events: None,
            sinks: None,
            auth: None,
        };

        let mut rules2 = HashMap::new();
        rules2.insert(
            "works_at".to_string(),
            vec![ValidationRule {
                source: "user".to_string(),
                targets: vec!["project".to_string()],
            }],
        );

        let config2 = LinksConfig {
            entities: vec![],
            links: vec![],
            validation_rules: Some(rules2),
            events: None,
            sinks: None,
            auth: None,
        };

        let merged = LinksConfig::merge(vec![config1, config2]);

        // Validation rules should be combined
        assert!(merged.validation_rules.is_some());
        let rules = merged.validation_rules.unwrap();
        assert_eq!(rules["works_at"].len(), 2);
    }

    #[test]
    fn test_find_link_definition_found() {
        let config = LinksConfig::default_config();

        let def = config.find_link_definition("owner", "user", "car");
        assert!(def.is_some(), "should find owner link from user to car");
        let def = def.expect("link definition should exist");
        assert_eq!(def.link_type, "owner");
        assert_eq!(def.source_type, "user");
        assert_eq!(def.target_type, "car");
    }

    #[test]
    fn test_find_link_definition_not_found() {
        let config = LinksConfig::default_config();

        let def = config.find_link_definition("nonexistent", "user", "car");
        assert!(def.is_none(), "should not find a nonexistent link type");

        // Wrong source type
        let def = config.find_link_definition("owner", "company", "car");
        assert!(def.is_none(), "should not find link with wrong source type");
    }

    #[test]
    fn test_is_valid_link_source_type_mismatch() {
        let mut rules = HashMap::new();
        rules.insert(
            "owner".to_string(),
            vec![ValidationRule {
                source: "user".to_string(),
                targets: vec!["car".to_string()],
            }],
        );

        let config = LinksConfig {
            entities: vec![],
            links: vec![],
            validation_rules: Some(rules),
            events: None,
            sinks: None,
            auth: None,
        };

        // Correct combination
        assert!(config.is_valid_link("owner", "user", "car"));

        // Source type mismatch
        assert!(
            !config.is_valid_link("owner", "company", "car"),
            "should reject mismatched source type"
        );

        // Target type mismatch
        assert!(
            !config.is_valid_link("owner", "user", "truck"),
            "should reject mismatched target type"
        );
    }

    #[test]
    fn test_is_valid_link_empty_targets() {
        let mut rules = HashMap::new();
        rules.insert(
            "membership".to_string(),
            vec![ValidationRule {
                source: "user".to_string(),
                targets: vec![], // empty targets list
            }],
        );

        let config = LinksConfig {
            entities: vec![],
            links: vec![],
            validation_rules: Some(rules),
            events: None,
            sinks: None,
            auth: None,
        };

        // With empty targets, no target type can match
        assert!(
            !config.is_valid_link("membership", "user", "group"),
            "should reject when targets list is empty"
        );
    }

    #[test]
    fn test_yaml_backward_compatible_without_events() {
        // Old-style YAML without events/sinks should still parse
        let yaml = r#"
entities:
  - singular: user
    plural: users
links:
  - link_type: follows
    source_type: user
    target_type: user
    forward_route_name: following
    reverse_route_name: followers
"#;

        let config = LinksConfig::from_yaml_str(yaml).unwrap();
        assert_eq!(config.entities.len(), 1);
        assert_eq!(config.links.len(), 1);
        assert!(config.events.is_none());
        assert!(config.sinks.is_none());
    }

    #[test]
    fn test_yaml_with_events_and_sinks() {
        let yaml = r#"
entities:
  - singular: user
    plural: users
  - singular: capture
    plural: captures

links:
  - link_type: follows
    source_type: user
    target_type: user
    forward_route_name: following
    reverse_route_name: followers
  - link_type: likes
    source_type: user
    target_type: capture
    forward_route_name: liked-captures
    reverse_route_name: likers
  - link_type: owns
    source_type: user
    target_type: capture
    forward_route_name: captures
    reverse_route_name: owner

events:
  backend:
    type: memory
  flows:
    - name: notify-new-follower
      trigger:
        kind: link.created
        link_type: follows
      pipeline:
        - resolve:
            from: source_id
            as: follower
        - map:
            template:
              type: follow
              message: "{{ follower.name }} started following you"
        - deliver:
            sinks: [push-notification, in-app-notification]
    - name: notify-like
      trigger:
        kind: link.created
        link_type: likes
      pipeline:
        - resolve:
            from: target_id
            via: owns
            direction: reverse
            as: owner
        - filter:
            condition: "source_id != owner.id"
        - batch:
            key: target_id
            window: 5m
        - deliver:
            sink: push-notification
  consumers:
    - name: mobile-feed
      seek: last_acknowledged

sinks:
  - name: push-notification
    type: push
    config:
      provider: expo
  - name: in-app-notification
    type: in_app
    config:
      ttl: 30d
"#;

        let config = LinksConfig::from_yaml_str(yaml).unwrap();

        // Entities and links
        assert_eq!(config.entities.len(), 2);
        assert_eq!(config.links.len(), 3);

        // Events
        assert!(config.events.is_some());
        let events = config.events.as_ref().unwrap();
        assert_eq!(events.backend.backend_type, "memory");
        assert_eq!(events.flows.len(), 2);
        assert_eq!(events.flows[0].name, "notify-new-follower");
        assert_eq!(events.flows[1].name, "notify-like");
        assert_eq!(events.consumers.len(), 1);
        assert_eq!(events.consumers[0].name, "mobile-feed");

        // Sinks
        assert!(config.sinks.is_some());
        let sinks = config.sinks.as_ref().unwrap();
        assert_eq!(sinks.len(), 2);
        assert_eq!(sinks[0].name, "push-notification");
        assert_eq!(sinks[0].sink_type, SinkType::Push);
        assert_eq!(sinks[1].name, "in-app-notification");
        assert_eq!(sinks[1].sink_type, SinkType::InApp);
    }

    #[test]
    fn test_merge_configs_with_events() {
        let config1 = LinksConfig {
            entities: vec![EntityConfig {
                singular: "user".to_string(),
                plural: "users".to_string(),
                auth: EntityAuthConfig::default(),
            }],
            links: vec![],
            validation_rules: None,
            events: Some(EventsConfig {
                backend: BackendConfig::default(),
                flows: vec![FlowConfig {
                    name: "flow-a".to_string(),
                    description: None,
                    trigger: TriggerConfig {
                        kind: "link.created".to_string(),
                        link_type: Some("follows".to_string()),
                        entity_type: None,
                    },
                    pipeline: vec![],
                }],
                consumers: vec![],
            }),
            sinks: Some(vec![SinkConfig {
                name: "push".to_string(),
                sink_type: SinkType::Push,
                config: HashMap::new(),
            }]),
            auth: None,
        };

        let config2 = LinksConfig {
            entities: vec![],
            links: vec![],
            validation_rules: None,
            events: Some(EventsConfig {
                backend: BackendConfig::default(),
                flows: vec![FlowConfig {
                    name: "flow-b".to_string(),
                    description: None,
                    trigger: TriggerConfig {
                        kind: "entity.created".to_string(),
                        link_type: None,
                        entity_type: Some("user".to_string()),
                    },
                    pipeline: vec![],
                }],
                consumers: vec![ConsumerConfig {
                    name: "mobile".to_string(),
                    seek: SeekMode::LastAcknowledged,
                }],
            }),
            sinks: Some(vec![SinkConfig {
                name: "in-app".to_string(),
                sink_type: SinkType::InApp,
                config: HashMap::new(),
            }]),
            auth: None,
        };

        let merged = LinksConfig::merge(vec![config1, config2]);

        // Events should be merged
        let events = merged.events.unwrap();
        assert_eq!(events.flows.len(), 2);
        assert_eq!(events.flows[0].name, "flow-a");
        assert_eq!(events.flows[1].name, "flow-b");
        assert_eq!(events.consumers.len(), 1);

        // Sinks should be merged (deduplicated by name)
        let sinks = merged.sinks.unwrap();
        assert_eq!(sinks.len(), 2);
    }
}

#[cfg(test)]
mod strict_merge_tests {
    use super::*;
    use crate::descriptor::{EntityDesc, LinkDesc, ModuleDescriptor, Operation, PolicyExpr};

    fn yaml(s: &str) -> LinksConfig {
        LinksConfig::from_yaml_str(s).unwrap()
    }

    #[test]
    fn merge_strict_reports_conflicts() {
        let a = yaml(
            r#"
entities:
  - { singular: order, plural: orders }
links:
  - { link_type: has, source_type: order, target_type: invoice,
      forward_route_name: invoices, reverse_route_name: order }
sinks:
  - { name: inbox, type: in_app }
"#,
        );
        // Identical redeclarations are not conflicts.
        let merged = LinksConfig::merge_strict(vec![a.clone(), a.clone()]).unwrap();
        assert_eq!(merged.entities.len(), 1);
        assert_eq!(merged.links.len(), 1);

        let b = yaml(
            r#"
entities:
  - { singular: order, plural: commandes }
  - { singular: command, plural: orders }
links:
  - { link_type: has, source_type: order, target_type: invoice,
      forward_route_name: bills, reverse_route_name: order }
  - { link_type: owns, source_type: order, target_type: payment,
      forward_route_name: invoices, reverse_route_name: owner }
sinks:
  - { name: inbox, type: webhook }
"#,
        );
        let errs = LinksConfig::merge_strict(vec![a.clone(), b]).unwrap_err();
        let what: Vec<String> = errs
            .iter()
            .map(|e| {
                assert_eq!(e.code.as_str(), "this.module.config_conflict");
                e.params["what"].as_str().unwrap().to_string()
            })
            .collect();
        for w in ["entity", "plural", "link", "link_route", "sink"] {
            assert!(what.iter().any(|x| x == w), "missing {w} in {what:?}");
        }
        // The legacy merge still silently keeps the last one (unchanged behaviour).
        let legacy = LinksConfig::merge(vec![a.clone(), a]);
        assert_eq!(legacy.entities.len(), 1);
    }

    #[test]
    fn from_descriptor_preserves_host_level_keys() {
        let host = yaml(
            r#"
entities:
  - { singular: invoice, plural: invoices, auth: { list: public } }
  - { singular: user, plural: users }
links: []
sinks:
  - { name: inbox, type: in_app }
validation_rules:
  owner: [ { source: user, targets: [invoice] } ]
"#,
        );
        let mut d = ModuleDescriptor::new("billing", "1.0.0");
        let mut inv = EntityDesc::new("invoice");
        inv.permissions
            .ops
            .insert(Operation::Delete, PolicyExpr::AdminOnly);
        inv.permissions
            .ops
            .insert(Operation::List, PolicyExpr::Deny);
        d.entities.push(inv);
        let mut l = LinkDesc::new("owner", "user", "invoice");
        l.permissions.ops.insert(
            Operation::CreateLink,
            PolicyExpr::HasRole(vec!["a".into(), "b".into()]),
        );
        d.links.push(l);
        d.normalize();

        let merged = host.overlay_descriptor(&d);
        assert_eq!(merged.sinks.as_ref().unwrap().len(), 1, "sinks kept");
        assert!(merged.validation_rules.is_some(), "validation_rules kept");
        assert_eq!(
            merged.entities.len(),
            2,
            "entity replaced by key, user kept"
        );
        let invoice = merged
            .entities
            .iter()
            .find(|e| e.singular == "invoice")
            .unwrap();
        assert_eq!(invoice.plural, "invoices");
        assert_eq!(invoice.auth.delete, "admin_only");
        assert_eq!(
            invoice.auth.list, "not:public",
            "deny in the legacy grammar"
        );
        assert_eq!(invoice.auth.get, "authenticated", "descriptor default");
        let link = &merged.links[0];
        assert_eq!(link.forward_route_name, "invoices-owners");
        assert_eq!(link.auth.as_ref().unwrap().create, "any:role:a,role:b");

        let alone = LinksConfig::from_descriptor(&d);
        assert!(alone.sinks.is_none() && alone.auth.is_none());
        assert_eq!(alone.entities.len(), 1);
    }
}
