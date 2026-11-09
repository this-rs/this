//! Le manifeste se sérialise en JSON exactement comme le `ModuleDescriptor` le relit :
//! on vérifie chaque constructeur par la valeur JSON qu'il produit.

use serde_json::{Value, json, to_value};
use this_module_sdk::*;

#[test]
fn field_types_use_snake_case_names() {
    let cases: Vec<(FieldType, Value)> = vec![
        (FieldType::String, json!("string")),
        (FieldType::Text, json!("text")),
        (FieldType::Bool, json!("bool")),
        (FieldType::I32, json!("i32")),
        (FieldType::I64, json!("i64")),
        (FieldType::U32, json!("u32")),
        (FieldType::U64, json!("u64")),
        (FieldType::F32, json!("f32")),
        (FieldType::F64, json!("f64")),
        (FieldType::Uuid, json!("uuid")),
        (FieldType::DateTime, json!("date_time")),
        (FieldType::Date, json!("date")),
        (FieldType::Json, json!("json")),
        (
            FieldType::Enum {
                name: "Etat".into(),
                variants: vec!["ouvert".into(), "clos".into()],
            },
            json!({"enum": {"name": "Etat", "variants": ["ouvert", "clos"]}}),
        ),
        (
            FieldType::List(Box::new(FieldType::Uuid)),
            json!({"list": "uuid"}),
        ),
        (
            FieldType::Ref {
                entity: "projet".into(),
            },
            json!({"ref": {"entity": "projet"}}),
        ),
    ];
    for (ty, expected) in cases {
        assert_eq!(to_value(&ty).unwrap(), expected, "{ty:?}");
    }
}

#[test]
fn validators_serialize_their_rule() {
    let cases: Vec<(Validator, Value)> = vec![
        (Validator::required(), json!({"rule": "required"})),
        (Validator::positive(), json!({"rule": "positive"})),
        (Validator::email(), json!({"rule": "email"})),
        (
            Validator::string_length(2, 40),
            json!({"rule": {"string_length": {"min": 2, "max": 40}}}),
        ),
        (Validator::min_value(-5), json!({"rule": {"min_value": -5}})),
        (Validator::max_value(99), json!({"rule": {"max_value": 99}})),
    ];
    for (v, expected) in cases {
        assert_eq!(to_value(&v).unwrap(), expected);
    }
}

#[test]
fn transforms_serialize_their_rule() {
    assert_eq!(
        to_value(Transform::trim()).unwrap(),
        json!({"rule": "trim"})
    );
    assert_eq!(
        to_value(Transform::uppercase()).unwrap(),
        json!({"rule": "uppercase"})
    );
    assert_eq!(
        to_value(Transform::lowercase()).unwrap(),
        json!({"rule": "lowercase"})
    );
}

#[test]
fn a_bare_field_is_required_and_omits_every_empty_attribute() {
    assert_eq!(
        to_value(Field::string("titre")).unwrap(),
        json!({"name": "titre", "ty": "string"})
    );
    assert_eq!(
        to_value(Field::bool("fait")).unwrap(),
        json!({"name": "fait", "ty": "bool"})
    );
    assert_eq!(
        to_value(Field::int("rang")).unwrap(),
        json!({"name": "rang", "ty": "i32"})
    );
    assert_eq!(
        to_value(Field::new("quand", FieldType::DateTime)).unwrap(),
        json!({"name": "quand", "ty": "date_time"})
    );
}

#[test]
fn a_field_carries_its_optional_default_validators_and_transforms_in_order() {
    let f = Field::string("email")
        .optional()
        .default(json!("a@b.co"))
        .validator(Validator::required())
        .validator(Validator::email())
        .transform(Transform::trim())
        .transform(Transform::lowercase());
    assert!(f.optional);
    assert_eq!(f.default, Some(json!("a@b.co")));
    assert_eq!(
        to_value(&f).unwrap(),
        json!({
            "name": "email", "ty": "string", "optional": true, "default": "a@b.co",
            "validators": [{"rule": "required"}, {"rule": "email"}],
            "transforms": [{"rule": "trim"}, {"rule": "lowercase"}]
        })
    );
}

#[test]
fn a_new_entity_grants_authenticated_on_the_five_crud_operations() {
    let e = Entity::new("tache", "taches");
    assert_eq!(
        (e.name.as_str(), e.plural.as_str(), e.kind),
        ("tache", "taches", "data")
    );
    assert!(e.fields.is_empty());
    assert_eq!(
        to_value(&e).unwrap(),
        json!({
            "name": "tache", "plural": "taches", "kind": "data", "fields": [],
            "permissions": {
                "create": "authenticated", "delete": "authenticated", "get": "authenticated",
                "list": "authenticated", "update": "authenticated"
            }
        })
    );
}

#[test]
fn entity_permission_replaces_one_policy_and_can_add_a_custom_operation() {
    let e = Entity::new("tache", "taches")
        .field(Field::string("titre"))
        .field(Field::bool("fait"))
        .permission("delete", "admin_only")
        .permission("archive", "owner");
    assert_eq!(e.permissions["delete"], "admin_only");
    assert_eq!(e.permissions["archive"], "owner");
    assert_eq!(e.permissions["list"], "authenticated");
    assert_eq!(e.permissions.len(), 6);
    let v = to_value(&e).unwrap();
    assert_eq!(
        v["fields"],
        json!([{"name": "titre", "ty": "string"}, {"name": "fait", "ty": "bool"}])
    );
}

#[test]
fn an_operation_declaration_defaults_to_empty_schemas() {
    let op = OperationDecl::new("archiver", "Archiver une tâche");
    assert_eq!(
        to_value(&op).unwrap(),
        json!({"name": "archiver", "label": "Archiver une tâche", "input": {}, "output": {}})
    );
    let op = op
        .input(json!({"id": "uuid"}))
        .output(json!({"archived": "bool"}));
    assert_eq!(op.input, json!({"id": "uuid"}));
    assert_eq!(op.output, json!({"archived": "bool"}));
}

#[test]
fn a_new_manifest_is_format_one_with_nothing_declared() {
    let m = Manifest::new("vide", "0.0.1");
    assert_eq!(m.format, 1);
    assert_eq!(
        to_value(&m).unwrap(),
        json!({"format": 1, "name": "vide", "version": "0.0.1", "entities": []}),
        "operations, views et actions vides sont omises"
    );
}

#[test]
fn a_full_manifest_serializes_every_declared_part() {
    let m = Manifest::new("taches", "1.2.3")
        .entity(Entity::new("tache", "taches").field(Field::string("titre")))
        .operation(OperationDecl::new("archiver", "Archiver"))
        .view(json!({"id": "liste", "kind": "list"}))
        .view(json!({"id": "detail", "kind": "detail"}))
        .action(json!({"id": "nouvelle"}));
    assert_eq!(m.entities.len(), 1);
    assert_eq!(m.operations.len(), 1);
    assert_eq!(m.views.len(), 2);
    assert_eq!(m.actions.len(), 1);
    let v = to_value(&m).unwrap();
    assert_eq!(v["operations"][0]["name"], "archiver");
    assert_eq!(
        v["views"],
        json!([{"id": "liste", "kind": "list"}, {"id": "detail", "kind": "detail"}])
    );
    assert_eq!(v["actions"], json!([{"id": "nouvelle"}]));
    assert_eq!(v["entities"][0]["fields"][0]["name"], "titre");
}
