//! # Settings sections declared by modules
//!
//! A module declares a section of settings with **closed** values: a toggle, or a choice
//! among declared options. The host validates the declaration, validates every write,
//! and persists behind the same [`Store`] trait as the app settings.
//!
//! - a value outside the choices is refused, and the refusal lists the expected values;
//! - a value of the wrong kind (a toggle given a choice) is refused with the expected kind;
//! - a section of a module that is not loaded is not shown, and cannot be written;
//! - reading is tolerant, like [`crate::Settings::load`]: a stored value that is no longer
//!   valid (the module removed the option) falls back to the declared default.
//!
//! Storage: one key per module, `this.app.module.<module>.settings`, holding
//! `{ "<section>": { "<field>": <value> } }`.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::settings::Store;

/// The storage key of a module's settings. Stable: changing it loses what people chose.
pub fn store_key(module: &str) -> String {
    format!("this.app.module.{module}.settings")
}

/// One option of a choice.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Choice {
    pub value: String,
    pub label: String,
}

impl Choice {
    pub fn new(value: impl Into<String>, label: impl Into<String>) -> Self {
        Self {
            value: value.into(),
            label: label.into(),
        }
    }
}

/// The kind of a setting, with its default.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum FieldKind {
    Toggle {
        default: bool,
    },
    Choice {
        options: Vec<Choice>,
        default: String,
    },
}

/// One setting of a section.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Field {
    pub key: String,
    pub label: String,
    #[serde(flatten)]
    pub kind: FieldKind,
}

impl Field {
    pub fn toggle(key: impl Into<String>, label: impl Into<String>, default: bool) -> Self {
        Self {
            key: key.into(),
            label: label.into(),
            kind: FieldKind::Toggle { default },
        }
    }

    pub fn choice(
        key: impl Into<String>,
        label: impl Into<String>,
        options: Vec<Choice>,
        default: impl Into<String>,
    ) -> Self {
        Self {
            key: key.into(),
            label: label.into(),
            kind: FieldKind::Choice {
                options,
                default: default.into(),
            },
        }
    }

    pub fn default_value(&self) -> SettingValue {
        match &self.kind {
            FieldKind::Toggle { default } => SettingValue::Toggle(*default),
            FieldKind::Choice { default, .. } => SettingValue::Choice(default.clone()),
        }
    }

    /// Checks a value against this field.
    pub fn check(&self, value: &SettingValue) -> Result<(), Expected> {
        match (&self.kind, value) {
            (FieldKind::Toggle { .. }, SettingValue::Toggle(_)) => Ok(()),
            (FieldKind::Choice { options, .. }, SettingValue::Choice(v)) => {
                if options.iter().any(|o| &o.value == v) {
                    Ok(())
                } else {
                    Err(self.expected())
                }
            }
            _ => Err(self.expected()),
        }
    }

    /// What this field accepts.
    pub fn expected(&self) -> Expected {
        match &self.kind {
            FieldKind::Toggle { .. } => Expected::Toggle,
            FieldKind::Choice { options, .. } => {
                Expected::OneOf(options.iter().map(|o| o.value.clone()).collect())
            }
        }
    }
}

/// What a field accepts, as told to the caller of a refused write.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "values", rename_all = "snake_case")]
pub enum Expected {
    Toggle,
    OneOf(Vec<String>),
}

/// A setting value: closed, so nothing else can be stored.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum SettingValue {
    Toggle(bool),
    Choice(String),
}

impl SettingValue {
    fn to_json(&self) -> Value {
        match self {
            SettingValue::Toggle(b) => Value::Bool(*b),
            SettingValue::Choice(s) => Value::String(s.clone()),
        }
    }

    fn from_json(v: &Value) -> Option<SettingValue> {
        match v {
            Value::Bool(b) => Some(SettingValue::Toggle(*b)),
            Value::String(s) => Some(SettingValue::Choice(s.clone())),
            _ => None,
        }
    }
}

/// A section of settings declared by a module.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Section {
    pub module: String,
    pub id: String,
    pub title: String,
    pub fields: Vec<Field>,
}

impl Section {
    pub fn new(
        module: impl Into<String>,
        id: impl Into<String>,
        title: impl Into<String>,
        fields: Vec<Field>,
    ) -> Self {
        Self {
            module: module.into(),
            id: id.into(),
            title: title.into(),
            fields,
        }
    }

    pub fn field(&self, key: &str) -> Option<&Field> {
        self.fields.iter().find(|f| f.key == key)
    }

    /// Checks the declaration: identifiers present, keys unique, choices non-empty and
    /// unique, defaults among the choices.
    pub fn validate(&self) -> Result<(), SettingsError> {
        if self.module.trim().is_empty() || self.id.trim().is_empty() {
            return Err(SettingsError::EmptyIdentifier);
        }
        let mut keys = BTreeSet::new();
        for f in &self.fields {
            if f.key.trim().is_empty() {
                return Err(SettingsError::EmptyIdentifier);
            }
            if !keys.insert(f.key.as_str()) {
                return Err(SettingsError::DuplicateField { key: f.key.clone() });
            }
            if let FieldKind::Choice { options, default } = &f.kind {
                if options.is_empty() {
                    return Err(SettingsError::NoChoices { key: f.key.clone() });
                }
                let mut values = BTreeSet::new();
                for o in options {
                    if !values.insert(o.value.as_str()) {
                        return Err(SettingsError::DuplicateChoice {
                            key: f.key.clone(),
                            value: o.value.clone(),
                        });
                    }
                }
                if !values.contains(default.as_str()) {
                    return Err(SettingsError::DefaultNotAChoice {
                        key: f.key.clone(),
                        default: default.clone(),
                        expected: f.expected(),
                    });
                }
            }
        }
        Ok(())
    }
}

/// Why a declaration or a write was refused.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SettingsError {
    EmptyIdentifier,
    DuplicateField {
        key: String,
    },
    NoChoices {
        key: String,
    },
    DuplicateChoice {
        key: String,
        value: String,
    },
    DefaultNotAChoice {
        key: String,
        default: String,
        expected: Expected,
    },
    /// The same module declared the same section twice.
    DuplicateSection {
        module: String,
        section: String,
    },
    UnknownSection {
        module: String,
        section: String,
    },
    UnknownField {
        module: String,
        section: String,
        key: String,
    },
    /// The module is not loaded: its section is not shown and cannot be written.
    ModuleNotLoaded {
        module: String,
    },
    /// The value is not one the field accepts.
    InvalidValue {
        key: String,
        given: SettingValue,
        expected: Expected,
    },
    /// The store holds something that is not JSON, or writing failed to serialise.
    Storage {
        message: String,
    },
}

impl SettingsError {
    /// A sentence for the person.
    pub fn message(&self) -> String {
        match self {
            SettingsError::InvalidValue {
                key,
                given,
                expected,
            } => {
                let given = match given {
                    SettingValue::Toggle(b) => b.to_string(),
                    SettingValue::Choice(s) => format!("« {s} »"),
                };
                match expected {
                    Expected::Toggle => format!("« {key} » attend oui ou non, pas {given}."),
                    Expected::OneOf(values) => {
                        format!(
                            "« {key} » attend l'une de : {} ; reçu {given}.",
                            values.join(", ")
                        )
                    }
                }
            }
            SettingsError::ModuleNotLoaded { module } => {
                format!(
                    "Les réglages de « {module} » ne sont pas disponibles : le module n'est pas chargé."
                )
            }
            other => format!("Réglage refusé : {other:?}"),
        }
    }
}

/// The sections the modules declared.
#[derive(Clone, Debug, Default)]
pub struct Registry {
    sections: BTreeMap<(String, String), Section>,
}

impl Registry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Declares a section, after validating it. A second declaration of the same section
    /// by the same module is refused (sections are scoped by module, so two modules never
    /// collide).
    pub fn declare(&mut self, section: Section) -> Result<(), SettingsError> {
        section.validate()?;
        let key = (section.module.clone(), section.id.clone());
        if self.sections.contains_key(&key) {
            return Err(SettingsError::DuplicateSection {
                module: section.module,
                section: section.id,
            });
        }
        self.sections.insert(key, section);
        Ok(())
    }

    /// Removes every section of a module (it was uninstalled).
    pub fn forget_module(&mut self, module: &str) -> usize {
        let before = self.sections.len();
        self.sections.retain(|(m, _), _| m != module);
        before - self.sections.len()
    }

    /// The sections to show: those of loaded modules, by module then section id.
    pub fn visible<'a, I: IntoIterator<Item = &'a str>>(&self, loaded: I) -> Vec<&Section> {
        let loaded: BTreeSet<&str> = loaded.into_iter().collect();
        self.sections
            .values()
            .filter(|s| loaded.contains(s.module.as_str()))
            .collect()
    }

    pub fn section(&self, module: &str, id: &str) -> Option<&Section> {
        self.sections.get(&(module.to_string(), id.to_string()))
    }

    fn field(&self, module: &str, section: &str, key: &str) -> Result<&Field, SettingsError> {
        let s = self
            .section(module, section)
            .ok_or_else(|| SettingsError::UnknownSection {
                module: module.to_string(),
                section: section.to_string(),
            })?;
        s.field(key).ok_or_else(|| SettingsError::UnknownField {
            module: module.to_string(),
            section: section.to_string(),
            key: key.to_string(),
        })
    }

    /// Reads a value: the stored one when it is still valid, the default otherwise.
    pub fn get(
        &self,
        store: &dyn Store,
        module: &str,
        section: &str,
        key: &str,
    ) -> Result<SettingValue, SettingsError> {
        let field = self.field(module, section, key)?;
        let stored = read_module(store, module)
            .ok()
            .and_then(|all| all.get(section).and_then(|s| s.get(key)).cloned())
            .and_then(|v| SettingValue::from_json(&v))
            .filter(|v| field.check(v).is_ok());
        Ok(stored.unwrap_or_else(|| field.default_value()))
    }

    /// Validates and persists a value. `loaded` names the loaded modules.
    pub fn set<'a, I: IntoIterator<Item = &'a str>>(
        &self,
        store: &mut dyn Store,
        loaded: I,
        module: &str,
        section: &str,
        key: &str,
        value: SettingValue,
    ) -> Result<(), SettingsError> {
        if !loaded.into_iter().any(|m| m == module) {
            return Err(SettingsError::ModuleNotLoaded {
                module: module.to_string(),
            });
        }
        let field = self.field(module, section, key)?;
        field
            .check(&value)
            .map_err(|expected| SettingsError::InvalidValue {
                key: key.to_string(),
                given: value.clone(),
                expected,
            })?;
        // a corrupt stored object is replaced rather than blocking every write
        let mut all = read_module(store, module).unwrap_or_default();
        let entry = all
            .entry(section.to_string())
            .or_insert_with(|| Value::Object(serde_json::Map::new()));
        if !entry.is_object() {
            *entry = Value::Object(serde_json::Map::new());
        }
        if let Some(obj) = entry.as_object_mut() {
            obj.insert(key.to_string(), value.to_json());
        }
        let raw = serde_json::to_string(&all).map_err(|e| SettingsError::Storage {
            message: e.to_string(),
        })?;
        store.write(&store_key(module), raw);
        Ok(())
    }

    /// Every field of a section with its current value, for the settings panel.
    pub fn values(
        &self,
        store: &dyn Store,
        module: &str,
        section: &str,
    ) -> Result<Vec<(&Field, SettingValue)>, SettingsError> {
        let s = self
            .section(module, section)
            .ok_or_else(|| SettingsError::UnknownSection {
                module: module.to_string(),
                section: section.to_string(),
            })?;
        s.fields
            .iter()
            .map(|f| self.get(store, module, section, &f.key).map(|v| (f, v)))
            .collect()
    }
}

fn read_module(store: &dyn Store, module: &str) -> Result<BTreeMap<String, Value>, SettingsError> {
    match store.read(&store_key(module)) {
        None => Ok(BTreeMap::new()),
        Some(raw) => serde_json::from_str(&raw).map_err(|e| SettingsError::Storage {
            message: e.to_string(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::MemoryStore;

    fn sync_section() -> Section {
        Section::new(
            "sync",
            "general",
            "Synchronisation",
            vec![
                Field::toggle("auto", "Synchroniser automatiquement", true),
                Field::choice(
                    "interval",
                    "Fréquence",
                    vec![
                        Choice::new("1m", "Chaque minute"),
                        Choice::new("1h", "Chaque heure"),
                    ],
                    "1h",
                ),
            ],
        )
    }

    fn registry() -> Registry {
        let mut r = Registry::new();
        r.declare(sync_section()).unwrap();
        r
    }

    #[test]
    fn defaults_are_returned_when_nothing_is_stored() {
        let r = registry();
        let store = MemoryStore::new();
        assert_eq!(
            r.get(&store, "sync", "general", "auto"),
            Ok(SettingValue::Toggle(true))
        );
        assert_eq!(
            r.get(&store, "sync", "general", "interval"),
            Ok(SettingValue::Choice("1h".into()))
        );
    }

    #[test]
    fn a_valid_value_is_persisted_and_read_back() {
        let r = registry();
        let mut store = MemoryStore::new();
        r.set(
            &mut store,
            ["sync"],
            "sync",
            "general",
            "interval",
            SettingValue::Choice("1m".into()),
        )
        .unwrap();
        r.set(
            &mut store,
            ["sync"],
            "sync",
            "general",
            "auto",
            SettingValue::Toggle(false),
        )
        .unwrap();
        assert_eq!(
            r.get(&store, "sync", "general", "interval"),
            Ok(SettingValue::Choice("1m".into()))
        );
        assert_eq!(
            r.get(&store, "sync", "general", "auto"),
            Ok(SettingValue::Toggle(false))
        );
        let raw = store.raw().get(&store_key("sync")).expect("written");
        // Comparer les VALEURS, pas les octets. L'ordre des clés dépend de
        // `serde_json/preserve_order`, qu'une autre crate du graphe de dépendances peut
        // allumer — le job de couverture le fait, pas un `cargo test -p this-app`. Une
        // assertion sur la chaîne passait donc ici et échouait en CI : le pire des tests,
        // celui qui apprend à ne pas croire la CI.
        let ecrit: serde_json::Value = serde_json::from_str(raw).expect("JSON valide");
        let attendu: serde_json::Value =
            serde_json::from_str(r#"{"general":{"auto":false,"interval":"1m"}}"#)
                .expect("attendu valide");
        assert_eq!(ecrit, attendu);
    }

    #[test]
    fn a_value_outside_the_choices_is_refused_with_the_expected_values() {
        let r = registry();
        let mut store = MemoryStore::new();
        let err = r
            .set(
                &mut store,
                ["sync"],
                "sync",
                "general",
                "interval",
                SettingValue::Choice("1s".into()),
            )
            .unwrap_err();
        assert_eq!(
            err,
            SettingsError::InvalidValue {
                key: "interval".into(),
                given: SettingValue::Choice("1s".into()),
                expected: Expected::OneOf(vec!["1m".into(), "1h".into()]),
            }
        );
        assert!(err.message().contains("1m, 1h"), "{}", err.message());
        assert!(store.raw().is_empty(), "nothing written");
    }

    #[test]
    fn a_value_of_the_wrong_kind_is_refused() {
        let r = registry();
        let mut store = MemoryStore::new();
        let err = r
            .set(
                &mut store,
                ["sync"],
                "sync",
                "general",
                "auto",
                SettingValue::Choice("yes".into()),
            )
            .unwrap_err();
        assert!(matches!(
            err,
            SettingsError::InvalidValue {
                expected: Expected::Toggle,
                ..
            }
        ));
    }

    #[test]
    fn a_section_of_a_module_not_loaded_is_not_shown_nor_writable() {
        let mut r = registry();
        r.declare(Section::new(
            "billing",
            "general",
            "Facturation",
            vec![Field::toggle("pdf", "PDF", false)],
        ))
        .unwrap();
        let shown: Vec<_> = r
            .visible(["billing"])
            .iter()
            .map(|s| s.module.as_str())
            .collect();
        assert_eq!(shown, ["billing"]);
        let mut store = MemoryStore::new();
        let err = r
            .set(
                &mut store,
                ["billing"],
                "sync",
                "general",
                "auto",
                SettingValue::Toggle(false),
            )
            .unwrap_err();
        assert_eq!(
            err,
            SettingsError::ModuleNotLoaded {
                module: "sync".into()
            }
        );
    }

    #[test]
    fn a_stored_value_that_is_no_longer_valid_falls_back_to_the_default() {
        let r = registry();
        let mut store = MemoryStore::new();
        store.write(
            &store_key("sync"),
            r#"{"general":{"interval":"5m","auto":"maybe"}}"#.into(),
        );
        assert_eq!(
            r.get(&store, "sync", "general", "interval"),
            Ok(SettingValue::Choice("1h".into()))
        );
        assert_eq!(
            r.get(&store, "sync", "general", "auto"),
            Ok(SettingValue::Toggle(true))
        );
        store.write(&store_key("sync"), "{pas du json".into());
        assert_eq!(
            r.get(&store, "sync", "general", "auto"),
            Ok(SettingValue::Toggle(true))
        );
        // and a write repairs it
        r.set(
            &mut store,
            ["sync"],
            "sync",
            "general",
            "auto",
            SettingValue::Toggle(false),
        )
        .unwrap();
        assert_eq!(
            r.get(&store, "sync", "general", "auto"),
            Ok(SettingValue::Toggle(false))
        );
    }

    #[test]
    fn invalid_declarations_are_refused() {
        let mut r = Registry::new();
        let dup = Section::new(
            "m",
            "s",
            "S",
            vec![
                Field::toggle("a", "A", true),
                Field::toggle("a", "A", false),
            ],
        );
        assert_eq!(
            r.declare(dup),
            Err(SettingsError::DuplicateField { key: "a".into() })
        );
        let empty = Section::new("m", "s", "S", vec![Field::choice("c", "C", vec![], "x")]);
        assert_eq!(
            r.declare(empty),
            Err(SettingsError::NoChoices { key: "c".into() })
        );
        let bad_default = Section::new(
            "m",
            "s",
            "S",
            vec![Field::choice("c", "C", vec![Choice::new("a", "A")], "z")],
        );
        assert!(matches!(
            r.declare(bad_default),
            Err(SettingsError::DefaultNotAChoice { .. })
        ));
        let dup_choice = Section::new(
            "m",
            "s",
            "S",
            vec![Field::choice(
                "c",
                "C",
                vec![Choice::new("a", "A"), Choice::new("a", "B")],
                "a",
            )],
        );
        assert!(matches!(
            r.declare(dup_choice),
            Err(SettingsError::DuplicateChoice { .. })
        ));
        r.declare(Section::new("m", "s", "S", vec![])).unwrap();
        assert_eq!(
            r.declare(Section::new("m", "s", "S", vec![])),
            Err(SettingsError::DuplicateSection {
                module: "m".into(),
                section: "s".into()
            })
        );
        // another module may use the same section id
        r.declare(Section::new("n", "s", "S", vec![])).unwrap();
    }

    #[test]
    fn unknown_sections_and_fields_are_named() {
        let r = registry();
        let store = MemoryStore::new();
        assert!(matches!(
            r.get(&store, "sync", "nope", "auto"),
            Err(SettingsError::UnknownSection { .. })
        ));
        assert!(matches!(
            r.get(&store, "sync", "general", "nope"),
            Err(SettingsError::UnknownField { .. })
        ));
    }

    #[test]
    fn values_lists_every_field_with_its_current_value() {
        let r = registry();
        let mut store = MemoryStore::new();
        r.set(
            &mut store,
            ["sync"],
            "sync",
            "general",
            "auto",
            SettingValue::Toggle(false),
        )
        .unwrap();
        let v = r.values(&store, "sync", "general").unwrap();
        let flat: Vec<_> = v.iter().map(|(f, v)| (f.key.as_str(), v.clone())).collect();
        assert_eq!(
            flat,
            [
                ("auto", SettingValue::Toggle(false)),
                ("interval", SettingValue::Choice("1h".into()))
            ]
        );
    }

    #[test]
    fn the_declaration_wire_shape_is_stable() {
        let raw = r#"{ "module": "sync", "id": "general", "title": "Sync", "fields": [
            { "key": "auto", "label": "Auto", "kind": "toggle", "default": true },
            { "key": "interval", "label": "Fréquence", "kind": "choice", "default": "1h",
              "options": [ { "value": "1h", "label": "Chaque heure" } ] } ] }"#;
        let s: Section = serde_json::from_str(raw).expect("section json");
        assert_eq!(s.validate(), Ok(()));
        assert_eq!(s.fields[1].expected(), Expected::OneOf(vec!["1h".into()]));
    }

    #[test]
    fn forgetting_a_module_removes_its_sections() {
        let mut r = registry();
        assert_eq!(r.forget_module("sync"), 1);
        assert!(r.visible(["sync"]).is_empty());
    }
}
