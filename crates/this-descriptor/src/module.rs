//! Module descriptor, dependencies and the host-wide index.

use crate::custom::Custom;
use crate::entity::EntityDesc;
use crate::error::{ContractError, ErrorCode, ErrorDesc};
use crate::framework::codes;
use crate::i18n::I18nDecl;
use crate::link::LinkDesc;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

pub const DESCRIPTOR_FORMAT: u32 = 1;

fn default_format() -> u32 {
    DESCRIPTOR_FORMAT
}

/// Everything a module declares, once.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModuleDescriptor {
    /// `DESCRIPTOR_FORMAT`; a higher value is refused (`this.module.format_unsupported`).
    #[serde(default = "default_format")]
    pub format: u32,
    /// `^[a-z][a-z0-9_]*$`, unique in a host. Also the namespace of its codes and keys.
    pub name: String,
    /// semver.
    pub version: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub requires: Vec<ModuleReq>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub entities: Vec<EntityDesc>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub links: Vec<LinkDesc>,
    /// Business error codes of the module (`<name>.…`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub errors: Vec<ErrorDesc>,
    /// Declared message keys and default locale (texts live in `MessageBundle`s).
    #[serde(default)]
    pub i18n: I18nDecl,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub custom: Vec<Custom>,
}

/// Dependency on another module.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModuleReq {
    pub name: String,
    /// `semver::VersionReq`, e.g. `^1.2`.
    pub version: String,
    /// Loaded if present (and then version-checked), never required.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub optional: bool,
}

impl ModuleDescriptor {
    pub fn new(name: impl Into<String>, version: impl Into<String>) -> Self {
        Self {
            format: DESCRIPTOR_FORMAT,
            name: name.into(),
            version: version.into(),
            requires: vec![],
            entities: vec![],
            links: vec![],
            errors: vec![],
            i18n: I18nDecl::default(),
            custom: vec![],
        }
    }

    pub fn entity(&self, name: &str) -> Option<&EntityDesc> {
        self.entities.iter().find(|e| e.name == name)
    }

    /// Fill derived defaults (plurals, link routes, rule merging).
    pub fn normalize(&mut self) {
        self.entities.iter_mut().for_each(EntityDesc::normalize);
        self.links.iter_mut().for_each(LinkDesc::normalize);
    }

    fn parsed(mut self) -> Result<Self, ContractError> {
        if self.format > DESCRIPTOR_FORMAT {
            return Err(ContractError::framework(codes::MODULE_FORMAT_UNSUPPORTED)
                .with_param("format", self.format));
        }
        self.normalize();
        Ok(self)
    }

    /// Parse (no `check()`: call it with the registry of the consumer).
    pub fn from_json(s: &str) -> Result<Self, ContractError> {
        serde_json::from_str::<Self>(s)
            .map_err(|e| {
                ContractError::framework(codes::DESCRIPTOR_PARSE)
                    .with_param("reason", e.to_string())
            })?
            .parsed()
    }

    #[cfg(feature = "yaml")]
    pub fn from_yaml(s: &str) -> Result<Self, ContractError> {
        serde_yaml::from_str::<Self>(s)
            .map_err(|e| {
                ContractError::framework(codes::DESCRIPTOR_PARSE)
                    .with_param("reason", e.to_string())
            })?
            .parsed()
    }

    pub fn to_json_pretty(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_default()
    }

    #[cfg(feature = "yaml")]
    pub fn to_yaml(&self) -> String {
        serde_yaml::to_string(self).unwrap_or_default()
    }

    pub fn semver(&self) -> Option<semver::Version> {
        semver::Version::parse(&self.version).ok()
    }

    /// Compose a compiled module's descriptor from its two sources, each facet having ONE
    /// owner (no equality check between copies: a redeclaration is an error):
    /// - `described` (the macros, `Described::entity_desc()`): name, plural, kind, fields,
    ///   indexed fields, field rules;
    /// - `self` (descriptor/module.yaml): module identity, requires, links, errors, i18n,
    ///   and per entity: permissions, i18n labels, capabilities, reserved tags.
    ///
    /// An entity of the YAML that redeclares `fields` (or a different plural) is refused
    /// with `this.descriptor.declared_twice`. A described entity absent from the YAML is
    /// added as is; a YAML entity without a described counterpart is kept (stored
    /// modules, which have no macro).
    pub fn compose(mut self, described: &[&EntityDesc]) -> Result<Self, ContractError> {
        let twice = |what: String| {
            ContractError::framework(codes::DESCRIPTOR_DECLARED_TWICE).with_param("what", what)
        };
        for d in described {
            match self.entities.iter_mut().find(|e| e.name == d.name) {
                None => self.entities.push((*d).clone()),
                Some(y) => {
                    if !y.fields.is_empty() || !y.indexed.is_empty() {
                        return Err(twice(format!(
                            "fields of `{}` (owned by the macro)",
                            d.name
                        )));
                    }
                    if !y.plural.is_empty() && y.plural != d.plural {
                        return Err(twice(format!(
                            "plural of `{}` ({} vs {})",
                            d.name, y.plural, d.plural
                        )));
                    }
                    y.plural = d.plural.clone();
                    y.kind = d.kind;
                    y.fields = d.fields.clone();
                    y.indexed = d.indexed.clone();
                }
            }
        }
        self.normalize();
        Ok(self)
    }

    /// [`Self::compose`] then `descriptor.lock`: the compiled module receives the STABLE proto
    /// tags (`/grpc/proto`, `descriptor export --format proto`) instead of the declaration
    /// order. A lock that contradicts the composed descriptor (moved or reused tag) is a start
    /// error, as `this descriptor check` refuses it at build time.
    #[cfg(feature = "yaml")]
    pub fn compose_locked(
        self,
        described: &[&EntityDesc],
        lock_yaml: &str,
    ) -> Result<Self, ContractError> {
        let lock = crate::lock::DescriptorLock::parse(lock_yaml)?;
        let mut composed = self.compose(described)?;
        let issues = lock.check(&composed);
        if !issues.is_empty() {
            return Err(
                ContractError::framework(codes::DESCRIPTOR_INVALID).with_param(
                    "issues",
                    issues
                        .iter()
                        .map(ToString::to_string)
                        .collect::<Vec<_>>()
                        .join("; "),
                ),
            );
        }
        lock.apply(&mut composed);
        Ok(composed)
    }
}

/// Precomputed view of an entity, built once by [`DescriptorSet::insert`]: field index,
/// presence sets, compiled patterns. The validation engine runs on it.
#[derive(Debug, Clone)]
pub struct CompiledEntity {
    pub module: String,
    pub desc: EntityDesc,
    /// Field name -> index in `desc.fields`.
    pub field_index: HashMap<String, usize>,
    #[cfg(feature = "pattern")]
    pub patterns: HashMap<String, regex::Regex>,
}

impl CompiledEntity {
    pub fn new(module: &str, desc: EntityDesc) -> Result<Self, ContractError> {
        let field_index = desc
            .fields
            .iter()
            .enumerate()
            .map(|(i, f)| (f.name.clone(), i))
            .collect();
        #[cfg(feature = "pattern")]
        let patterns = {
            let mut m = HashMap::new();
            for f in &desc.fields {
                for r in &f.validators {
                    if let crate::field::ValidatorSpec::Pattern(p) = &r.rule {
                        let re = regex::Regex::new(p).map_err(|e| {
                            ContractError::framework(codes::DESCRIPTOR_INVALID)
                                .with_param("issues", format!("{}.{}: {e}", desc.name, f.name))
                        })?;
                        m.insert(p.clone(), re);
                    }
                }
            }
            m
        };
        Ok(Self {
            module: module.to_string(),
            desc,
            field_index,
            #[cfg(feature = "pattern")]
            patterns,
        })
    }

    pub fn field(&self, name: &str) -> Option<&crate::field::FieldDesc> {
        self.field_index.get(name).map(|&i| &self.desc.fields[i])
    }
}

/// Index of every active descriptor of a host. Immutable once built: a change produces a
/// NEW set (cheap: descriptors are `Arc`) published atomically by the host catalog, so
/// readers never hold a lock across an `.await`.
#[derive(Debug, Clone, Default)]
pub struct DescriptorSet {
    modules: BTreeMap<String, Arc<ModuleDescriptor>>,
    entities: HashMap<String, Arc<CompiledEntity>>,
    plurals: HashMap<String, String>,
    errors: HashMap<ErrorCode, (String, ErrorDesc)>,
    generation: u64,
}

impl DescriptorSet {
    pub fn new() -> Self {
        Self::default()
    }

    /// Insert a module. Refused (and the set unchanged): duplicate module, entity, plural or
    /// error code — the end of the silent last-wins.
    pub fn insert(&mut self, d: ModuleDescriptor) -> Result<(), ContractError> {
        if self.modules.contains_key(&d.name) {
            return Err(ContractError::framework(codes::MODULE_DUPLICATE_MODULE)
                .with_param("module", &d.name));
        }
        let mut compiled = Vec::new();
        for e in &d.entities {
            if let Some(prev) = self.entities.get(&e.name) {
                return Err(ContractError::framework(codes::MODULE_DUPLICATE_ENTITY)
                    .with_param("entity", &e.name)
                    .with_param("modules", format!("{}, {}", prev.module, d.name)));
            }
            if self.plurals.contains_key(&e.plural) {
                return Err(ContractError::framework(codes::MODULE_DUPLICATE_PLURAL)
                    .with_param("plural", &e.plural));
            }
            compiled.push(Arc::new(CompiledEntity::new(&d.name, e.clone())?));
        }
        for err in &d.errors {
            if self.errors.contains_key(&err.code) {
                return Err(ContractError::framework(codes::MODULE_DUPLICATE_ERROR_CODE)
                    .with_param("code", &err.code.0));
            }
        }
        for c in compiled {
            self.plurals
                .insert(c.desc.plural.clone(), c.desc.name.clone());
            self.entities.insert(c.desc.name.clone(), c);
        }
        for err in &d.errors {
            self.errors
                .insert(err.code.clone(), (d.name.clone(), err.clone()));
        }
        self.modules.insert(d.name.clone(), Arc::new(d));
        self.generation += 1;
        Ok(())
    }

    pub fn remove(&mut self, module: &str) -> Option<Arc<ModuleDescriptor>> {
        let d = self.modules.remove(module)?;
        for e in &d.entities {
            self.entities.remove(&e.name);
            self.plurals.remove(&e.plural);
        }
        self.errors.retain(|_, (m, _)| m != module);
        self.generation += 1;
        Some(d)
    }

    /// Entity and the module that declares it.
    pub fn entity(&self, entity: &str) -> Option<(&Arc<ModuleDescriptor>, &EntityDesc)> {
        let c = self.entities.get(entity)?;
        Some((self.modules.get(&c.module)?, &c.desc))
    }
    pub fn compiled(&self, entity: &str) -> Option<&Arc<CompiledEntity>> {
        self.entities.get(entity)
    }
    /// Singular name of the entity whose plural is `plural`.
    pub fn entity_by_plural(&self, plural: &str) -> Option<&str> {
        self.plurals.get(plural).map(String::as_str)
    }
    pub fn error(&self, code: &ErrorCode) -> Option<&ErrorDesc> {
        self.errors.get(code).map(|(_, e)| e)
    }
    pub fn module(&self, name: &str) -> Option<&Arc<ModuleDescriptor>> {
        self.modules.get(name)
    }
    pub fn modules(&self) -> impl Iterator<Item = &Arc<ModuleDescriptor>> {
        self.modules.values()
    }
    pub fn is_empty(&self) -> bool {
        self.modules.is_empty()
    }
    /// Incremented on every change: key of the caches derived from the set (SDL, proto).
    pub fn generation(&self) -> u64 {
        self.generation
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entity::EntityDesc;

    #[test]
    fn descriptor_set_refuses_duplicates_and_keeps_state() {
        let mut a = ModuleDescriptor::new("billing", "1.0.0");
        a.entities.push(EntityDesc::new("invoice"));
        let mut b = ModuleDescriptor::new("other", "1.0.0");
        b.entities.push(EntityDesc::new("invoice"));
        let mut set = DescriptorSet::new();
        set.insert(a.clone()).unwrap();
        let g = set.generation();
        let e = set.insert(b).unwrap_err();
        assert_eq!(e.code.as_str(), codes::MODULE_DUPLICATE_ENTITY);
        assert_eq!(
            set.generation(),
            g,
            "refused insert leaves the set unchanged"
        );
        assert_eq!(
            set.insert(a).unwrap_err().code.as_str(),
            codes::MODULE_DUPLICATE_MODULE
        );
        assert_eq!(set.entity_by_plural("invoices"), Some("invoice"));
        assert!(set.remove("billing").is_some());
        assert!(set.entity("invoice").is_none());
    }

    #[test]
    fn compose_gives_each_facet_one_owner() {
        use crate::field::{FieldDesc, FieldType};
        use crate::perm::{Operation, PolicyExpr};
        let mut described = EntityDesc::new("invoice");
        described
            .fields
            .push(FieldDesc::new("amount", FieldType::F64));
        let mut yaml = ModuleDescriptor::new("billing", "1.0.0");
        let mut y = EntityDesc::new("invoice");
        y.plural.clear();
        y.permissions
            .ops
            .insert(Operation::Delete, PolicyExpr::AdminOnly);
        yaml.entities.push(y);
        let m = yaml.clone().compose(&[&described]).unwrap();
        assert_eq!(m.entities[0].fields.len(), 1);
        assert_eq!(m.entities[0].plural, "invoices");
        assert!(
            m.entities[0]
                .permissions
                .policy(&Operation::Delete)
                .is_some()
        );
        // Redeclaring fields in the YAML is an error, not an equality check.
        let mut dup = yaml;
        dup.entities[0]
            .fields
            .push(FieldDesc::new("amount", FieldType::F64));
        assert_eq!(
            dup.compose(&[&described]).unwrap_err().code.as_str(),
            "this.descriptor.declared_twice"
        );
    }

    #[cfg(feature = "yaml")]
    fn compiled_invoice() -> (ModuleDescriptor, EntityDesc) {
        use crate::field::{FieldDesc, FieldType};
        let mut described = EntityDesc::new("invoice");
        described
            .fields
            .push(FieldDesc::new("amount", FieldType::F64));
        described
            .fields
            .push(FieldDesc::new("label", FieldType::String));
        (ModuleDescriptor::new("billing", "1.0.0"), described)
    }

    #[cfg(feature = "yaml")]
    #[test]
    fn proto_tags_present_for_compiled_module_with_lock() {
        use crate::derive::proto::{TagMode, file_for};
        let (m, described) = compiled_invoice();
        // The lock keeps `label` on 20 although it is declared second: stable, not positional.
        let lock = "format: 1\nentities:\n  invoice:\n    fields:\n      amount: 16\n      label: 20\n    reserved: [17]\n";
        let composed = m.clone().compose_locked(&[&described], lock).unwrap();
        let e = &composed.entities[0];
        assert_eq!(e.field("amount").unwrap().proto_tag, Some(16));
        assert_eq!(e.field("label").unwrap().proto_tag, Some(20));
        assert_eq!(e.reserved_tags, vec![17]);
        let proto = file_for(&[e], "billing", TagMode::Strict).unwrap();
        assert!(proto.contains("label = 20;"), "{proto}");
        // Without the lock (plain compose) the tags stay absent: Strict refuses, no invented number.
        let bare = m.compose(&[&described]).unwrap();
        assert!(bare.entities[0].field("label").unwrap().proto_tag.is_none());
        assert!(file_for(&[&bare.entities[0]], "billing", TagMode::Strict).is_err());
    }

    #[cfg(feature = "yaml")]
    #[test]
    fn compose_locked_refuses_a_contradicting_lock() {
        let (m, mut described) = compiled_invoice();
        described.fields[1].proto_tag = Some(30);
        let lock =
            "format: 1\nentities:\n  invoice:\n    fields:\n      amount: 16\n      label: 20\n";
        let e = m.compose_locked(&[&described], lock).unwrap_err();
        assert_eq!(e.code.as_str(), codes::DESCRIPTOR_INVALID);
    }

    #[test]
    fn format_above_supported_is_refused() {
        let e = ModuleDescriptor::from_json(r#"{"format": 99, "name": "x", "version": "1.0.0"}"#)
            .unwrap_err();
        assert_eq!(e.code.as_str(), codes::MODULE_FORMAT_UNSUPPORTED);
    }
}
