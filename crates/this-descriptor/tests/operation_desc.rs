//! `OperationDesc` : les constructeurs chaînés et leur forme JSON (les champs vides sont omis).

use serde_json::json;
use this_descriptor::OperationDesc;

#[test]
fn a_new_operation_has_only_its_name() {
    let op = OperationDesc::new("archiver");
    assert_eq!(op.name, "archiver");
    assert_eq!(
        (
            &op.label,
            &op.entity,
            &op.input,
            &op.output,
            &op.description
        ),
        (&None, &None, &None, &None, &None)
    );
    assert_eq!(
        serde_json::to_value(&op).unwrap(),
        json!({"name": "archiver"})
    );
}

#[test]
fn each_builder_sets_exactly_its_own_field() {
    let op = OperationDesc::new("archiver")
        .label("Archiver")
        .entity("tache")
        .input(json!({"id": "uuid"}))
        .output(json!({"archived": "bool"}));
    assert_eq!(op.label.as_deref(), Some("Archiver"));
    assert_eq!(op.entity.as_deref(), Some("tache"));
    assert_eq!(op.input, Some(json!({"id": "uuid"})));
    assert_eq!(op.output, Some(json!({"archived": "bool"})));
    assert_eq!(op.description, None);
    assert_eq!(
        serde_json::to_value(&op).unwrap(),
        json!({
            "name": "archiver", "label": "Archiver", "entity": "tache",
            "input": {"id": "uuid"}, "output": {"archived": "bool"}
        })
    );
}

#[test]
fn an_operation_round_trips_through_json() {
    let op = OperationDesc::new("x").label("L").input(json!([1, 2]));
    let back: OperationDesc = serde_json::from_value(serde_json::to_value(&op).unwrap()).unwrap();
    assert_eq!(back, op);
    let minimal: OperationDesc = serde_json::from_value(json!({"name": "y"})).unwrap();
    assert_eq!(minimal, OperationDesc::new("y"));
}
