//! JSON Schema (draft 2020-12) of write payloads and a minimal OpenAPI 3.1 document.
//!
//! The schema describes what [`engine::validate::apply`](crate::engine::validate::apply)
//! accepts, with three documented gaps:
//! - transforms run BEFORE validators on the server (`x-this-transforms` lists them): a
//!   client value that only becomes valid after `trim`/`uppercase` is refused here;
//! - `string_length` counts BYTES on the server, characters in JSON Schema
//!   (`x-this-length-unit: bytes`);
//! - `Custom` validators and `date_format` are named (`x-this-validators`,
//!   `x-this-date-format`), not expressed.

use super::pascal_case;
use crate::custom::ALLOW_UNKNOWN_FIELDS;
use crate::engine::validate::WriteOp;
use crate::entity::EntityDesc;
use crate::field::{Applies, FieldDesc, FieldType, TransformSpec, ValidatorSpec};
use crate::module::DescriptorSet;
use serde_json::{Map, Value, json};

/// Schema of a value of type `ty` (non-null).
pub fn type_schema(ty: &FieldType) -> Value {
    match ty {
        FieldType::String | FieldType::Text => json!({"type": "string"}),
        FieldType::Bool => json!({"type": "boolean"}),
        FieldType::I32 => json!({"type": "integer", "minimum": i32::MIN, "maximum": i32::MAX}),
        FieldType::I64 => json!({"type": "integer", "minimum": i64::MIN, "maximum": i64::MAX}),
        FieldType::U32 => json!({"type": "integer", "minimum": 0, "maximum": u32::MAX}),
        FieldType::U64 => json!({"type": "integer", "minimum": 0, "maximum": u64::MAX}),
        FieldType::F32 | FieldType::F64 => json!({"type": "number"}),
        FieldType::Decimal { scale } => json!({
            "anyOf": [{"type": "number"}, {"type": "string", "pattern": "^-?[0-9]+(\\.[0-9]+)?$"}],
            "x-this-scale": scale
        }),
        FieldType::Uuid => json!({"type": "string", "format": "uuid"}),
        FieldType::Ref { entity } => {
            json!({"type": "string", "format": "uuid", "x-this-ref": entity})
        }
        FieldType::DateTime => json!({"type": "string", "format": "date-time"}),
        FieldType::Date => json!({"type": "string", "format": "date"}),
        FieldType::Enum { name, variants } => {
            json!({"type": "string", "enum": variants, "x-this-enum": name})
        }
        FieldType::List(inner) => json!({"type": "array", "items": type_schema(inner)}),
        FieldType::Custom(c) => json!({"x-this-type": c.name}),
        // Json, and any future type: anything.
        _ => json!({}),
    }
}

fn applies(op: WriteOp) -> Applies {
    match op {
        WriteOp::Create => Applies::Create,
        WriteOp::Update => Applies::Update,
    }
}

/// Schema of one field for `op` (`None` = rules of every operation are ignored: read
/// schema).
fn field_schema(f: &FieldDesc, op: Option<Applies>) -> Value {
    let mut s = type_schema(&f.ty);
    if let (Some(op), Value::Object(m)) = (op, &mut s) {
        let mut customs = Vec::new();
        for r in f.validators.iter().filter(|r| r.on.covers(op)) {
            match &r.rule {
                ValidatorSpec::Positive => {
                    m.insert("exclusiveMinimum".into(), json!(0));
                }
                ValidatorSpec::StringLength { min, max } => {
                    m.insert("minLength".into(), json!(min));
                    m.insert("maxLength".into(), json!(max));
                    m.insert("x-this-length-unit".into(), json!("bytes"));
                }
                ValidatorSpec::MinValue(n) => {
                    m.insert("minimum".into(), Value::Number(n.clone()));
                }
                ValidatorSpec::MaxValue(n) => {
                    m.insert("maximum".into(), Value::Number(n.clone()));
                }
                ValidatorSpec::InList(values) => {
                    m.insert("enum".into(), Value::Array(values.clone()));
                }
                ValidatorSpec::Pattern(p) => {
                    m.insert("pattern".into(), json!(p));
                }
                ValidatorSpec::Email => {
                    m.insert("format".into(), json!("email"));
                }
                ValidatorSpec::Url => {
                    m.insert("format".into(), json!("uri"));
                }
                ValidatorSpec::DateFormat(fmt) => {
                    m.insert("x-this-date-format".into(), json!(fmt));
                }
                ValidatorSpec::Custom(c) => customs.push(json!(c.name)),
                // Required / Optional: presence, expressed by `required`.
                _ => {}
            }
        }
        if !customs.is_empty() {
            m.insert("x-this-validators".into(), Value::Array(customs));
        }
        let transforms: Vec<Value> = f
            .transforms
            .iter()
            .filter(|t| t.on.covers(op))
            .map(|t| match &t.rule {
                TransformSpec::Custom(c) => json!(c.name),
                TransformSpec::Trim => json!("trim"),
                TransformSpec::Uppercase => json!("uppercase"),
                TransformSpec::Lowercase => json!("lowercase"),
                TransformSpec::RoundDecimals(n) => json!(format!("round_decimals({n})")),
                #[allow(unreachable_patterns)]
                _ => json!("unknown"),
            })
            .collect();
        if !transforms.is_empty() {
            m.insert("x-this-transforms".into(), Value::Array(transforms));
        }
    }
    if f.deprecated
        && let Value::Object(m) = &mut s
    {
        m.insert("deprecated".into(), json!(true));
    }
    if f.optional {
        json!({"anyOf": [s, {"type": "null"}]})
    } else {
        s
    }
}

/// JSON Schema of the payload of `op`: writable fields only (no read-only field; no
/// immutable field on update), `required` = [`payload_required`](super::payload_required)
/// (the server's rule), unknown keys refused unless
/// the entity has the `this.allow_unknown_fields` capability.
pub fn write_schema(e: &EntityDesc, op: WriteOp) -> Value {
    let a = applies(op);
    let mut props = Map::new();
    let mut required = Vec::new();
    for f in e
        .all_fields()
        .filter(|f| !f.read_only && !(op == WriteOp::Update && f.immutable))
    {
        props.insert(f.name.clone(), field_schema(f, Some(a)));
        if super::payload_required(e, f, a) {
            required.push(json!(f.name));
        }
    }
    let suffix = match op {
        WriteOp::Create => "Create",
        WriteOp::Update => "Update",
    };
    json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "title": format!("{}{suffix}", pascal_case(&e.name)),
        "type": "object",
        "properties": props,
        "required": required,
        "additionalProperties": e.has_capability(ALLOW_UNKNOWN_FIELDS),
    })
}

/// Schema of an entity as READ (every field; optional = nullable; no rule).
pub fn read_schema(e: &EntityDesc) -> Value {
    let mut props = Map::new();
    let mut required = Vec::new();
    for f in e.all_fields() {
        props.insert(f.name.clone(), field_schema(f, None));
        if !f.optional {
            required.push(json!(f.name));
        }
    }
    json!({
        "title": pascal_case(&e.name),
        "type": "object",
        "properties": props,
        "required": required,
    })
}

/// Component name of the error body. Contains a `.`, which `pascal_case` of an entity name
/// never produces: an entity named `error` cannot overwrite it.
pub const ERROR_SCHEMA: &str = "this.Error";

/// Shape of the error body (`ContractError::http_body`).
fn error_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "error": {"type": "string"},
            "errors": {"type": "array", "items": {"type": "string"}},
            "code": {"type": "string"},
            "kind": {"type": "string"},
            "message_key": {"type": "string"},
            "message": {"type": "string"},
            "fields": {"type": "array", "items": {"type": "object", "properties": {
                "path": {"type": "string"}, "code": {"type": "string"},
                "message_key": {"type": "string"}, "message": {"type": "string"},
                "params": {"type": "object"}
            }}},
            "type": {"type": "string"}, "title": {"type": "string"},
            "status": {"type": "integer"}, "detail": {"type": "string"}
        },
        "required": ["code", "kind", "message"]
    })
}

/// Minimal OpenAPI 3.1 document of the CRUD routes of every DATA entity of `set`
/// (`/{plural}`: list, create; `/{plural}/{id}`: get, update, delete). Links, custom routes
/// and auth schemes are not described.
pub fn openapi(set: &DescriptorSet, title: &str, version: &str) -> Value {
    let mut paths = Map::new();
    let mut schemas = Map::new();
    schemas.insert(ERROR_SCHEMA.into(), error_schema());
    let err = json!({"description": "Error (see `code`)", "content": {"application/json": {"schema": {"$ref": format!("#/components/schemas/{ERROR_SCHEMA}")}}}});
    let r = |name: &str| json!({"$ref": format!("#/components/schemas/{name}")});
    for e in super::entities_sorted(set)
        .into_iter()
        .filter(|e| e.kind == crate::entity::EntityKind::Data)
    {
        let p = pascal_case(&e.name);
        let (create, update) = (format!("{p}Create"), format!("{p}Update"));
        let mut cs = write_schema(e, WriteOp::Create);
        let mut us = write_schema(e, WriteOp::Update);
        for s in [&mut cs, &mut us] {
            if let Value::Object(m) = s {
                m.remove("$schema");
            }
        }
        schemas.insert(p.clone(), read_schema(e));
        schemas.insert(create.clone(), cs);
        schemas.insert(update.clone(), us);
        // CRUD handlers are written by the module (`EntityDescriptor::build_routes`): the
        // exact success status (200 or 201 on create, 200 or 204 on delete) is not in the
        // descriptor, so the document promises a 2XX range, never a guessed code.
        let one = |desc: &str| {
            let mut m = Map::new();
            m.insert(
                "2XX".to_string(),
                json!({"description": desc, "content": {"application/json": {"schema": r(&p)}}}),
            );
            m.insert("default".into(), err.clone());
            Value::Object(m)
        };
        let body = |name: &str| json!({"required": true, "content": {"application/json": {"schema": r(name)}}});
        let id_param = json!([{"name": "id", "in": "path", "required": true, "schema": {"type": "string", "format": "uuid"}}]);
        let q = |name: &str, ty: &str| json!({"name": name, "in": "query", "required": false, "schema": {"type": ty}});
        paths.insert(
            format!("/{}", e.plural),
            json!({
                "get": {"operationId": format!("list{p}"), "tags": [e.name],
                    // `sort` is not listed: the legacy QueryParams never read it.
                    "parameters": [q("limit", "integer"), q("offset", "integer"), q("filter", "string")],
                    // Page shape is the module's (array or paginated object).
                    "responses": {"2XX": {"description": "Page", "content": {"application/json": {"schema": {}}}}, "default": err}},
                "post": {"operationId": format!("create{p}"), "tags": [e.name],
                    "requestBody": body(&create), "responses": one("Created")}
            }),
        );
        paths.insert(
            format!("/{}/{{id}}", e.plural),
            json!({
                "parameters": id_param,
                "get": {"operationId": format!("get{p}"), "tags": [e.name], "responses": one("Entity")},
                "put": {"operationId": format!("update{p}"), "tags": [e.name],
                    "requestBody": body(&update), "responses": one("Updated")},
                "delete": {"operationId": format!("delete{p}"), "tags": [e.name],
                    "responses": {"2XX": {"description": "Deleted"}, "default": err}}
            }),
        );
    }
    json!({
        "openapi": "3.1.0",
        "info": {"title": title, "version": version},
        "paths": paths,
        "components": {"schemas": schemas},
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::module::ModuleDescriptor;

    fn invoice() -> EntityDesc {
        let mut e = EntityDesc::new("invoice");
        e.fields = vec![
            FieldDesc::new("amount", FieldType::F64)
                .validator(ValidatorSpec::Positive, Applies::Always),
            FieldDesc::new("note", FieldType::Text).optional(),
            FieldDesc::new("currency", FieldType::String)
                .validator(
                    ValidatorSpec::InList(vec![json!("EUR"), json!("USD")]),
                    Applies::Create,
                )
                .transform(TransformSpec::Uppercase, Applies::Always),
            FieldDesc::new("tenant_id", FieldType::Uuid).read_only(),
            FieldDesc::new(
                "customer",
                FieldType::Ref {
                    entity: "user".into(),
                },
            )
            .immutable(),
        ];
        e
    }

    #[test]
    fn json_schema_required_on_create_only() {
        let e = invoice();
        let c = crate::derive::json_schema(&e, WriteOp::Create).unwrap();
        let req: Vec<&str> = c["required"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        // Non-optional declared writable fields; not `note` (optional), not the base `name` /
        // `status` (filled by the server unless an explicit rule says otherwise).
        assert_eq!(req, vec!["amount", "currency", "customer"], "{c}");
        assert!(
            c["properties"]["name"].is_object(),
            "base field writable: {c}"
        );
        assert!(c["properties"].get("tenant_id").is_none(), "read-only: {c}");
        assert!(c["properties"].get("id").is_none(), "{c}");
        assert_eq!(c["properties"]["amount"]["exclusiveMinimum"], 0);
        assert_eq!(c["properties"]["currency"]["enum"], json!(["EUR", "USD"]));
        assert_eq!(
            c["properties"]["currency"]["x-this-transforms"],
            json!(["uppercase"])
        );
        assert_eq!(c["properties"]["note"]["anyOf"][1], json!({"type": "null"}));
        assert_eq!(c["additionalProperties"], false);

        let u = crate::derive::json_schema(&e, WriteOp::Update).unwrap();
        assert_eq!(u["required"], json!([]), "nothing required on update: {u}");
        assert!(u["properties"].get("customer").is_none(), "immutable: {u}");
        assert!(
            u["properties"]["currency"].get("enum").is_none(),
            "create-only rule"
        );
        assert_eq!(u["properties"]["amount"]["exclusiveMinimum"], 0);
    }

    #[test]
    fn openapi_lists_crud_paths_and_schemas() {
        let mut m = ModuleDescriptor::new("billing", "1.0.0");
        m.entities.push(invoice());
        // An entity named `error` does not overwrite the error body schema.
        m.entities.push(EntityDesc::new("error"));
        let mut set = DescriptorSet::new();
        set.insert(m).unwrap();
        let doc = openapi(&set, "api", "1.0.0");
        assert_eq!(doc["openapi"], "3.1.0");
        assert!(doc["paths"]["/invoices"]["post"].is_object());
        assert!(doc["paths"]["/invoices/{id}"]["delete"].is_object());
        assert_eq!(
            doc["paths"]["/invoices"]["post"]["requestBody"]["content"]["application/json"]["schema"]
                ["$ref"],
            "#/components/schemas/InvoiceCreate"
        );
        assert!(doc["components"]["schemas"]["Invoice"]["properties"]["id"].is_object());
        assert!(doc["components"]["schemas"][ERROR_SCHEMA]["required"].is_array());
        assert_eq!(doc["components"]["schemas"]["Error"]["title"], "Error");
        // No guessed status code: handlers are the module's.
        let post = &doc["paths"]["/invoices"]["post"]["responses"];
        assert!(
            post["2XX"].is_object() && post.get("201").is_none(),
            "{post}"
        );
        assert!(doc["paths"]["/invoices/{id}"]["delete"]["responses"]["2XX"].is_object());
        let params = doc["paths"]["/invoices"]["get"]["parameters"].to_string();
        assert!(!params.contains("\"sort\""), "sort is never read: {params}");
        // Every $ref resolves to a component.
        let text = doc.to_string();
        for (i, _) in text.match_indices("#/components/schemas/") {
            let name: String = text[i + 21..]
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '.')
                .collect();
            assert!(
                doc["components"]["schemas"].get(&name).is_some(),
                "dangling $ref {name}"
            );
        }
    }
}
