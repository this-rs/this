//! # Le modèle d'affichage d'un module, dérivé de son descripteur typé
//!
//! C'est ce qui rend l'interface **transposable** : n'importe quel module de this-rs produit
//! le même [`Module`], et les composants du kit n'ont plus besoin de connaître ni le module,
//! ni la forme JSON du descripteur — ils lisent des [`Kind`] typés.
//!
//! Avant, l'application réinterprétait le JSON du descripteur (`{"enum": …}`, `{"ref": …}`)
//! avec des `Value` : une faute de frappe dans une clé produisait un champ silencieusement
//! « autre ». Ici, le compilateur vérifie la correspondance avec `FieldType`.

use serde_json::Value;
use this_descriptor::{FieldDesc, FieldType, ModuleDescriptor};

/// La nature d'un champ, du point de vue d'une interface.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Kind {
    Text,
    Bool,
    Int,
    Decimal,
    Date,
    DateTime,
    Uuid,
    Json,
    Enum(Vec<String>),
    Ref(String),
    List(Box<Kind>),
    /// Type fourni par un module (`Custom`) : l'interface le traite comme du texte.
    Other,
}

impl Kind {
    pub fn of(ty: &FieldType) -> Self {
        match ty {
            FieldType::String | FieldType::Text => Kind::Text,
            FieldType::Bool => Kind::Bool,
            FieldType::I32 | FieldType::I64 | FieldType::U32 | FieldType::U64 => Kind::Int,
            FieldType::F32 | FieldType::F64 | FieldType::Decimal { .. } => Kind::Decimal,
            FieldType::Date => Kind::Date,
            FieldType::DateTime => Kind::DateTime,
            FieldType::Uuid => Kind::Uuid,
            FieldType::Json => Kind::Json,
            FieldType::Enum { variants, .. } => Kind::Enum(variants.clone()),
            FieldType::Ref { entity } => Kind::Ref(entity.clone()),
            FieldType::List(inner) => Kind::List(Box::new(Kind::of(inner))),
            FieldType::Custom(_) => Kind::Other,
            // le descripteur est `non_exhaustive` : un type ajouté demain tombe ici plutôt
            // que de casser la compilation de tous les fronts
            _ => Kind::Other,
        }
    }

    /// Juste à droite en colonne, et accepte un `<input type=number>`.
    pub fn is_numeric(&self) -> bool {
        matches!(self, Kind::Int | Kind::Decimal)
    }

    pub fn as_ref_entity(&self) -> Option<&str> {
        match self {
            Kind::Ref(entity) => Some(entity),
            _ => None,
        }
    }

    pub fn enum_variants(&self) -> &[String] {
        match self {
            Kind::Enum(variants) => variants,
            _ => &[],
        }
    }

    /// `<input type=…>` correspondant.
    pub fn input_type(&self) -> &'static str {
        match self {
            Kind::Int | Kind::Decimal => "number",
            Kind::Date => "date",
            Kind::DateTime => "datetime-local",
            _ => "text",
        }
    }

    pub fn step(&self) -> &'static str {
        match self {
            Kind::Decimal => "0.01",
            Kind::Int => "1",
            _ => "any",
        }
    }
}

/// Un champ tel qu'un écran le lit.
#[derive(Clone, Debug, PartialEq)]
pub struct Field {
    pub name: String,
    pub kind: Kind,
    pub optional: bool,
    pub read_only: bool,
    pub default: Option<Value>,
}

impl Field {
    pub fn of(field: &FieldDesc) -> Self {
        Self {
            name: field.name.clone(),
            kind: Kind::of(&field.ty),
            optional: field.optional,
            read_only: field.read_only,
            default: field.default.clone(),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Entity {
    pub name: String,
    pub fields: Vec<Field>,
}

impl Entity {
    pub fn field(&self, name: &str) -> Option<&Field> {
        self.fields.iter().find(|f| f.name == name)
    }

    /// Tous les champs obligatoires sont saisissables à la main.
    pub fn creatable_by_hand(&self) -> bool {
        self.fields.iter().all(|f| f.optional || !f.read_only)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Link {
    pub link_type: String,
    pub source: String,
    pub target: String,
}

/// Le modèle d'un module : ce dont une interface a besoin, et rien de plus.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Module {
    pub name: String,
    pub version: String,
    pub entities: Vec<Entity>,
    pub links: Vec<Link>,
    pub errors: usize,
    /// Langues **livrées** par le module (bundles présents).
    pub locales: Vec<String>,
}

impl Module {
    pub fn of(descriptor: &ModuleDescriptor, locales: Vec<String>) -> Self {
        Self {
            name: descriptor.name.clone(),
            version: descriptor.version.clone(),
            entities: descriptor
                .entities
                .iter()
                .map(|entity| Entity {
                    name: entity.name.clone(),
                    fields: entity.fields.iter().map(Field::of).collect(),
                })
                .collect(),
            links: descriptor
                .links
                .iter()
                .map(|link| Link {
                    link_type: link.link_type.clone(),
                    source: link.source.clone(),
                    target: link.target.clone(),
                })
                .collect(),
            errors: descriptor.errors.len(),
            locales,
        }
    }

    pub fn entity(&self, name: &str) -> Option<&Entity> {
        self.entities.iter().find(|e| e.name == name)
    }

    pub fn entity_names(&self) -> Vec<String> {
        self.entities.iter().map(|e| e.name.clone()).collect()
    }
}

// ---- Formats ---------------------------------------------------------------
//
// Le français groupe par trois avec une espace fine insécable et sépare les décimales par
// une virgule. Ces deux fonctions sont la seule définition de ce format dans le kit.

/// Espace fine insécable entre les groupes de trois chiffres.
const THIN: char = '\u{202f}';

fn grouped(int: &str) -> String {
    let mut out = String::new();
    for (i, c) in int.chars().enumerate() {
        if i > 0 && (int.len() - i).is_multiple_of(3) {
            out.push(THIN);
        }
        out.push(c);
    }
    out
}

/// `1234.5` → `1 234,50 €`.
pub fn money_f(x: f64) -> String {
    let s = format!("{:.2}", x.abs());
    let (int, dec) = s.split_once('.').unwrap_or((s.as_str(), "00"));
    format!(
        "{}{},{} €",
        if x < 0.0 { "-" } else { "" },
        grouped(int),
        dec
    )
}

/// `1234` → `1 234`.
pub fn count(x: f64) -> String {
    grouped(&format!("{:.0}", x.abs()))
}

/// `1234` → `1 234` pour un entier déjà arrondi.
pub fn count_usize(x: usize) -> String {
    count(x as f64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use this_descriptor::{EntityDesc, FieldDesc, ModuleDescriptor};

    fn field(name: &str, ty: FieldType) -> FieldDesc {
        FieldDesc::new(name, ty)
    }

    #[test]
    fn kinds_are_read_from_the_typed_descriptor() {
        assert_eq!(Kind::of(&FieldType::String), Kind::Text);
        assert_eq!(Kind::of(&FieldType::I32), Kind::Int);
        assert_eq!(Kind::of(&FieldType::Decimal { scale: 2 }), Kind::Decimal);
        assert_eq!(
            Kind::of(&FieldType::Enum {
                name: "state".into(),
                variants: vec!["draft".into(), "paid".into()]
            }),
            Kind::Enum(vec!["draft".into(), "paid".into()])
        );
        assert_eq!(
            Kind::of(&FieldType::Ref {
                entity: "customer".into()
            }),
            Kind::Ref("customer".into())
        );
        assert_eq!(
            Kind::of(&FieldType::List(Box::new(FieldType::Uuid))),
            Kind::List(Box::new(Kind::Uuid))
        );
    }

    #[test]
    fn a_module_exposes_entities_links_and_locales() {
        let mut descriptor = ModuleDescriptor::new("shop", "0.1.0");
        let mut customer = EntityDesc::new("customer");
        customer.fields = vec![
            field("name", FieldType::String),
            field("city", FieldType::String),
        ];
        descriptor.entities = vec![customer];

        let module = Module::of(&descriptor, vec!["fr".into(), "en".into()]);
        assert_eq!(module.name, "shop");
        assert_eq!(module.locales, vec!["fr", "en"]);
        assert_eq!(module.entity_names(), vec!["customer"]);
        let entity = module.entity("customer").unwrap();
        assert_eq!(entity.field("name").unwrap().kind, Kind::Text);
        assert!(entity.field("fantome").is_none());
        assert!(entity.creatable_by_hand());
    }

    #[test]
    fn a_required_read_only_field_is_not_creatable_by_hand() {
        let mut descriptor = ModuleDescriptor::new("shop", "0.1.0");
        let mut order = EntityDesc::new("order");
        order.fields = vec![
            field("number", FieldType::String).read_only(),
            field("total", FieldType::Decimal { scale: 2 }).read_only(),
        ];
        descriptor.entities = vec![order];

        let module = Module::of(&descriptor, vec![]);
        assert!(!module.entity("order").unwrap().creatable_by_hand());
    }

    #[test]
    fn amounts_are_formatted_french() {
        assert_eq!(money_f(1234.5), "1\u{202f}234,50 €");
        assert_eq!(money_f(-89.9), "-89,90 €");
        assert_eq!(money_f(0.0), "0,00 €");
        assert_eq!(count(1234.0), "1\u{202f}234");
        assert_eq!(count(89.0), "89");
    }

    #[test]
    fn input_types_follow_the_kind() {
        assert_eq!(Kind::Int.input_type(), "number");
        assert_eq!(Kind::Decimal.step(), "0.01");
        assert_eq!(Kind::Date.input_type(), "date");
        assert_eq!(Kind::Text.input_type(), "text");
        assert!(Kind::Ref("x".into()).as_ref_entity() == Some("x"));
    }
}
