//! Module `note` : le domaine de connaissance de Project Orchestrator, porté par un module
//! `this`. Deux entités — `note` et `rfc` — avec les champs réels de PO (`Note`,
//! `NoteType`, `NoteStatus`, `NoteImportance`, `NoteAuthorKind` ; l'RFC est une note de
//! type `rfc` dont le cycle de vie est `draft → proposed → accepted → implemented | rejected`).
//!
//! Le `handle` est un CRUD piloté par le descripteur, qui valide en plus les `Enum`
//! (le CRUD de `plan` ne le faisait pas). Il n'émet que des effets : `plan-store` les applique.
//!
//! # Le critère décisif
//!
//! Ajouter `source_url` à `note` ne demande de toucher QUE le descripteur : le CRUD, le
//! formulaire (dérivé des champs saisissables) et le `Store` ne changent pas.

use std::cmp::Ordering;

use this_module_sdk::{
    Effect, Entity, Field, FieldType, Manifest, Module, Op, Request, Response, Value, json,
};

pub struct NoteModule;

fn entities() -> Vec<Entity> {
    vec![note(), rfc()]
}

fn enum_of(name: &str, variants: &[&str]) -> FieldType {
    FieldType::Enum {
        name: name.to_string(),
        variants: variants.iter().map(|s| s.to_string()).collect(),
    }
}

fn tags() -> Field {
    Field::new("tags", FieldType::List(Box::new(FieldType::String))).default(json!([]))
}

// --- Descripteur ------------------------------------------------------------------

fn note() -> Entity {
    Entity::new("note", "notes")
        .field(Field::new(
            "note_type",
            enum_of(
                "note_type",
                &[
                    "guideline",
                    "gotcha",
                    "pattern",
                    "context",
                    "tip",
                    "observation",
                    "assertion",
                    "rfc",
                ],
            ),
        ))
        // Le contenu long : Text, pas String.
        .field(Field::new("content", FieldType::Text))
        // `status` est réservé en String par le framework (cf. plan) ; vocabulaire PO
        // NoteStatus : active | needs_review | stale | obsolete | archived.
        .field(Field::string("status").default(json!("active")))
        .field(
            Field::new(
                "importance",
                enum_of("importance", &["low", "medium", "high", "critical"]),
            )
            .default(json!("medium"))
            .sortable(),
        )
        .field(tags())
        .field(Field::new("project_id", FieldType::Uuid).optional())
        .field(Field::string("created_by").default(json!("agent")))
        .field(
            Field::new(
                "author_kind",
                enum_of("author_kind", &["human", "agent", "system"]),
            )
            .default(json!("agent")),
        )
        .field(Field::new("confidence", FieldType::F64).optional())
        .field(Field::new("staleness_score", FieldType::F64).default(json!(0.0)))
        .field(Field::new("energy", FieldType::F64).default(json!(1.0)))
        .field(Field::new("scar_intensity", FieldType::F64).default(json!(0.0)))
        .field(Field::new("activation_count", FieldType::I64).default(json!(0)))
        .field(Field::new("valid_from", FieldType::DateTime).optional())
        .field(Field::new("valid_until", FieldType::DateTime).optional())
        // Relations de succession : note → note (auto-référence).
        .field(
            Field::new(
                "supersedes",
                FieldType::Ref {
                    entity: "note".to_string(),
                },
            )
            .optional(),
        )
        .field(
            Field::new(
                "superseded_by",
                FieldType::Ref {
                    entity: "note".to_string(),
                },
            )
            .optional(),
        )
}

fn rfc() -> Entity {
    Entity::new("rfc", "rfcs")
        // Relation rfc → note (la note de type `rfc` qui porte le texte).
        .field(
            Field::new(
                "note",
                FieldType::Ref {
                    entity: "note".to_string(),
                },
            )
            .optional(),
        )
        .field(Field::string("title"))
        .field(Field::new("problem", FieldType::Text))
        .field(Field::new("proposed_solution", FieldType::Text))
        .field(Field::new("alternatives", FieldType::Text))
        .field(Field::new("impact", FieldType::Text))
        // Cycle de vie du Protocol FSM de PO (champ de base `status`, String).
        .field(Field::string("status").default(json!("draft")))
        .field(Field::new("plan_id", FieldType::Uuid).optional())
        .field(tags())
}

// --- Vues (dérivées du descripteur) ------------------------------------------------

fn fields_of(entity: &str) -> Vec<Field> {
    entities()
        .into_iter()
        .find(|e| e.name == entity)
        .map(|e| e.fields)
        .unwrap_or_default()
}

fn sortable_fields(entity: &str) -> Vec<String> {
    fields_of(entity)
        .into_iter()
        .filter(|f| f.sortable)
        .map(|f| f.name)
        .collect()
}

/// Un champ est « saisissable » (donc dans le formulaire) s'il est textuel, énuméré ou une
/// liste de chaînes : les compteurs, scores, dates et relations sont gérés par le système.
/// Règle sur le TYPE, jamais sur un nom : un nouveau champ String y entre sans autre geste.
/// `status` est exclu (cycle de vie, pas saisie).
fn authorable(f: &Field) -> bool {
    f.name != "status"
        && matches!(
            f.ty,
            FieldType::String | FieldType::Text | FieldType::Enum { .. } | FieldType::List(_)
        )
}

fn list_view(entity: &str, plural: &str, title: &str, label: &str) -> Value {
    let sort: Vec<Value> = sortable_fields(entity)
        .into_iter()
        .map(|f| json!({"field": f, "dir": "asc"}))
        .collect();
    json!({
        "name": plural, "surface": "main", "title": title,
        "root": {"type": "leaf", "brick": {
            "type": "list", "entity": entity, "label": label,
            "subtitle": "status", "sort": sort, "empty": "Aucune entrée"
        }}
    })
}

fn form_view(entity: &str, action: &str, title: &str) -> Value {
    let fields: Vec<Value> = fields_of(entity)
        .iter()
        .filter(|f| authorable(f))
        .map(|f| json!({"field": f.name, "label": f.name}))
        .collect();
    json!({
        "name": action, "surface": "panel", "title": title,
        "root": {"type": "leaf", "brick": {
            "type": "form", "entity": entity, "submit": action, "fields": fields
        }}
    })
}

fn create_action(entity: &str, name: &str) -> Value {
    let fields: Vec<String> = fields_of(entity)
        .into_iter()
        .filter(authorable)
        .map(|f| f.name)
        .collect();
    json!({"kind": "create", "name": name, "entity": entity, "fields": fields})
}

// --- Module -----------------------------------------------------------------------

impl Module for NoteModule {
    fn manifest() -> Manifest {
        let mut m = Manifest::new("note", "0.1.0");
        m.entities = entities();
        m.actions = vec![
            create_action("note", "creer_note"),
            create_action("rfc", "creer_rfc"),
        ];
        m.views = vec![
            list_view("note", "notes", "Notes", "content"),
            list_view("rfc", "rfcs", "RFC", "title"),
            form_view("note", "creer_note", "Nouvelle note"),
            form_view("rfc", "creer_rfc", "Nouvelle RFC"),
        ];
        m
    }

    fn handle(req: Request) -> Response {
        match req.op {
            Op::Create => create(&req),
            Op::Update => update(&req),
            Op::Get => get(&req),
            Op::List => list(&req),
            Op::Delete => delete(&req),
            Op::Call => Response::err("operation_inconnue", "CRUD seulement"),
        }
    }
}

// --- CRUD piloté par le descripteur -----------------------------------------------

fn entity_of(req: &Request) -> Result<&str, Response> {
    let Some(entity) = req.entity.as_deref() else {
        return Err(Response::err("entite_manquante", "champ `entity` requis"));
    };
    if fields_of(entity).is_empty() {
        return Err(Response::err(
            "entite_inconnue",
            format!("entité inconnue : {entity}"),
        ));
    }
    Ok(entity)
}

/// Valide une valeur contre le type DÉCLARÉ du champ (énumérations seulement : le reste
/// est du ressort de l'hôte). Une variante hors vocabulaire est refusée.
fn check_enum(f: &Field, v: &Value) -> Result<(), Response> {
    if let FieldType::Enum { variants, .. } = &f.ty
        && !v.as_str().is_some_and(|s| variants.iter().any(|x| x == s))
    {
        return Err(Response::field_err(
            "variante_inconnue",
            format!("{} : valeur hors de {variants:?}", f.name),
            f.name.clone(),
        ));
    }
    Ok(())
}

fn create(req: &Request) -> Response {
    let entity = match entity_of(req) {
        Ok(e) => e,
        Err(r) => return r,
    };
    let mut body = serde_json::Map::new();
    for f in &fields_of(entity) {
        match req.input.get(&f.name) {
            Some(Value::Null) if f.optional => {}
            Some(v) => {
                if let Err(r) = check_enum(f, v) {
                    return r;
                }
                body.insert(f.name.clone(), v.clone());
            }
            None => {
                if let Some(d) = &f.default {
                    body.insert(f.name.clone(), d.clone());
                } else if !f.optional {
                    return Response::field_err(
                        "champ_requis",
                        format!("{} est requis", f.name),
                        f.name.clone(),
                    );
                }
            }
        }
    }
    let body = Value::Object(body);
    Response::ok(body.clone()).effect(Effect::put(entity, body))
}

fn update(req: &Request) -> Response {
    let entity = match entity_of(req) {
        Ok(e) => e,
        Err(r) => return r,
    };
    let Some(id) = req.input.get("id").and_then(Value::as_str) else {
        return Response::field_err("id_manquant", "l'id est requis", "id");
    };
    let Some(mut ligne) = req.row(id).cloned() else {
        return Response::err("introuvable", format!("{entity} introuvable"));
    };
    for f in &fields_of(entity) {
        if let Some(v) = req.input.get(&f.name) {
            if let Err(r) = check_enum(f, v) {
                return r;
            }
            ligne[f.name.as_str()] = v.clone();
        }
    }
    Response::ok(ligne.clone()).effect(Effect::put(entity, ligne))
}

fn get(req: &Request) -> Response {
    let id = req.input.get("id").and_then(Value::as_str).unwrap_or("");
    match req.row(id) {
        Some(l) => Response::ok(l.clone()),
        None => Response::err("introuvable", "ligne introuvable"),
    }
}

fn delete(req: &Request) -> Response {
    let entity = match entity_of(req) {
        Ok(e) => e,
        Err(r) => return r,
    };
    match req.input.get("id").and_then(Value::as_str) {
        Some(id) => Response::ok(json!({"id": id})).effect(Effect::delete(entity, id)),
        None => Response::field_err("id_manquant", "l'id est requis", "id"),
    }
}

/// Tri par les champs `sortable`. Une énumération se trie dans l'ordre de DÉCLARATION de
/// ses variantes (low < medium < high < critical), pas alphabétiquement.
fn list(req: &Request) -> Response {
    let entity = match entity_of(req) {
        Ok(e) => e,
        Err(r) => return r,
    };
    let fields = fields_of(entity);
    let mut lignes: Vec<&Value> = req.rows.iter().collect();
    for name in sortable_fields(entity).iter().rev() {
        let f = fields.iter().find(|f| &f.name == name);
        lignes.sort_by(|a, b| cmp_field(a, b, name, f));
    }
    Response::ok(json!({"items": lignes, "total": lignes.len()}))
}

fn rank(f: Option<&Field>, v: &Value) -> Option<usize> {
    match (f.map(|f| &f.ty), v.as_str()) {
        (Some(FieldType::Enum { variants, .. }), Some(s)) => variants.iter().position(|x| x == s),
        _ => None,
    }
}

fn cmp_field(a: &Value, b: &Value, name: &str, f: Option<&Field>) -> Ordering {
    match (a.get(name), b.get(name)) {
        (Some(x), Some(y)) if rank(f, x).is_some() => rank(f, x).cmp(&rank(f, y)),
        (Some(Value::Number(x)), Some(Value::Number(y))) => x
            .as_f64()
            .partial_cmp(&y.as_f64())
            .unwrap_or(Ordering::Equal),
        (Some(Value::String(x)), Some(Value::String(y))) => x.cmp(y),
        (None, Some(_)) => Ordering::Less,
        (Some(_), None) => Ordering::Greater,
        _ => Ordering::Equal,
    }
}

this_module_sdk::export!(NoteModule);
