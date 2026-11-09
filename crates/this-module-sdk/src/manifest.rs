//! Le manifeste : miroir typé du `ModuleDescriptor` de `this-descriptor`.
//!
//! Il ne dépend pas de ce crate (trop lourd pour le gabarit de la forge) mais produit le même
//! JSON : les tests de l'exemple `taches` le font relire par le vrai `ModuleDescriptor` et
//! passer `check(Stored)`.

use serde::Serialize;
use serde_json::Value;
use std::collections::BTreeMap;

/// Le type d'un champ (mêmes noms que `this_descriptor::FieldType`, en snake_case).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FieldType {
    String,
    Text,
    Bool,
    I32,
    I64,
    U32,
    U64,
    F32,
    F64,
    Uuid,
    DateTime,
    Date,
    Json,
    Enum { name: String, variants: Vec<String> },
    List(Box<FieldType>),
    Ref { entity: String },
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
enum ValidatorSpec {
    Required,
    Positive,
    StringLength { min: usize, max: usize },
    MinValue(i64),
    MaxValue(i64),
    Email,
}

/// Règle déclarative de validation, vérifiée aussi par l'hôte avant le module.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Validator {
    rule: ValidatorSpec,
}

impl Validator {
    pub fn required() -> Self {
        Self {
            rule: ValidatorSpec::Required,
        }
    }
    pub fn positive() -> Self {
        Self {
            rule: ValidatorSpec::Positive,
        }
    }
    /// Longueur en OCTETS (comme le framework).
    pub fn string_length(min: usize, max: usize) -> Self {
        Self {
            rule: ValidatorSpec::StringLength { min, max },
        }
    }
    pub fn min_value(v: i64) -> Self {
        Self {
            rule: ValidatorSpec::MinValue(v),
        }
    }
    pub fn max_value(v: i64) -> Self {
        Self {
            rule: ValidatorSpec::MaxValue(v),
        }
    }
    pub fn email() -> Self {
        Self {
            rule: ValidatorSpec::Email,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
enum TransformSpec {
    Trim,
    Uppercase,
    Lowercase,
}

/// Transformation d'entrée, appliquée avant les validateurs.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Transform {
    rule: TransformSpec,
}

impl Transform {
    pub fn trim() -> Self {
        Self {
            rule: TransformSpec::Trim,
        }
    }
    pub fn uppercase() -> Self {
        Self {
            rule: TransformSpec::Uppercase,
        }
    }
    pub fn lowercase() -> Self {
        Self {
            rule: TransformSpec::Lowercase,
        }
    }
}

/// Un champ déclaré. Requis à la création sauf `.optional()` ou `.default(..)`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Field {
    pub name: String,
    pub ty: FieldType,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub optional: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default: Option<Value>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub validators: Vec<Validator>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub transforms: Vec<Transform>,
}

impl Field {
    pub fn new(name: impl Into<String>, ty: FieldType) -> Self {
        Self {
            name: name.into(),
            ty,
            optional: false,
            default: None,
            validators: vec![],
            transforms: vec![],
        }
    }
    pub fn string(name: impl Into<String>) -> Self {
        Self::new(name, FieldType::String)
    }
    pub fn bool(name: impl Into<String>) -> Self {
        Self::new(name, FieldType::Bool)
    }
    /// Entier signé 32 bits (`Int`).
    pub fn int(name: impl Into<String>) -> Self {
        Self::new(name, FieldType::I32)
    }
    pub fn optional(mut self) -> Self {
        self.optional = true;
        self
    }
    pub fn default(mut self, v: Value) -> Self {
        self.default = Some(v);
        self
    }
    pub fn validator(mut self, v: Validator) -> Self {
        self.validators.push(v);
        self
    }
    pub fn transform(mut self, t: Transform) -> Self {
        self.transforms.push(t);
        self
    }
}

/// Une entité. `Entity::new` accorde `authenticated` aux cinq opérations CRUD (un module
/// stocké doit les déclarer toutes, explicitement) ; `.permission` les change.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Entity {
    pub name: String,
    /// Le pluriel est donné, jamais deviné : il devient la route (`/taches`).
    pub plural: String,
    pub kind: &'static str,
    pub fields: Vec<Field>,
    pub permissions: BTreeMap<String, String>,
}

impl Entity {
    pub fn new(name: impl Into<String>, plural: impl Into<String>) -> Self {
        let permissions = ["list", "get", "create", "update", "delete"]
            .iter()
            .map(|o| (o.to_string(), "authenticated".to_string()))
            .collect();
        Self {
            name: name.into(),
            plural: plural.into(),
            kind: "data",
            fields: vec![],
            permissions,
        }
    }
    pub fn field(mut self, f: Field) -> Self {
        self.fields.push(f);
        self
    }
    /// `op` : `list|get|create|update|delete` ; `policy` : `authenticated`, `owner`, `admin_only`…
    pub fn permission(mut self, op: impl Into<String>, policy: impl Into<String>) -> Self {
        self.permissions.insert(op.into(), policy.into());
        self
    }
}

/// Une opération nommée (§9), appelée par `Op::Call` avec `Request::name`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct OperationDecl {
    pub name: String,
    pub label: String,
    pub input: Value,
    pub output: Value,
}

impl OperationDecl {
    pub fn new(name: impl Into<String>, label: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            label: label.into(),
            input: Value::Object(Default::default()),
            output: Value::Object(Default::default()),
        }
    }
    pub fn input(mut self, schema: Value) -> Self {
        self.input = schema;
        self
    }
    pub fn output(mut self, schema: Value) -> Self {
        self.output = schema;
        self
    }
}

/// Le descripteur du module.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Manifest {
    pub format: u32,
    /// `^[a-z][a-z0-9_]*$`.
    pub name: String,
    /// semver.
    pub version: String,
    pub entities: Vec<Entity>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub operations: Vec<OperationDecl>,
    /// Vues (`ViewDesc`) en JSON brut : le contrat de vue est celui de `this-descriptor::views`.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub views: Vec<Value>,
    /// Actions (`Action`) en JSON brut.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub actions: Vec<Value>,
}

impl Manifest {
    pub fn new(name: impl Into<String>, version: impl Into<String>) -> Self {
        Self {
            format: 1,
            name: name.into(),
            version: version.into(),
            entities: vec![],
            operations: vec![],
            views: vec![],
            actions: vec![],
        }
    }
    pub fn entity(mut self, e: Entity) -> Self {
        self.entities.push(e);
        self
    }
    pub fn operation(mut self, o: OperationDecl) -> Self {
        self.operations.push(o);
        self
    }
    pub fn view(mut self, v: Value) -> Self {
        self.views.push(v);
        self
    }
    pub fn action(mut self, a: Value) -> Self {
        self.actions.push(a);
        self
    }
}
