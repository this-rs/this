//! Module `plan` : le domaine de plans de Project Orchestrator, porté par un module `this`.
//!
//! Cinq entités — `plan`, `task`, `step`, `decision`, `constraint` — déclarées avec leurs
//! champs réels (repris du backend PO : `PlanNode`, `TaskNode`, `StepNode`, `DecisionNode`,
//! `ConstraintNode`). Le `handle` est un CRUD **générique** : il lit son propre descripteur
//! (`manifest()`) pour savoir quels champs existent, lesquels sont requis, lesquels sont
//! triables. Le module n'émet que des effets `put`/`delete` : il ne touche jamais la base,
//! l'hôte applique les effets. C'est le critère.
//!
//! # Le critère décisif
//!
//! Rendre `task.priority` triable ne demande de toucher QUE le descripteur : `handle(list)`
//! trie selon les champs `sortable` déclarés, et la vue liste dérive son `sort` du même
//! descripteur. Ni le `handle`, ni la vue, ni le test ne changent.

use std::cmp::Ordering;

use this_module_sdk::{
    Effect, Entity, Field, FieldType, Manifest, Module, Op, Request, Response, Value, json,
};

pub mod ecran;

pub struct PlanModule;

/// Les cinq entités du domaine, dans l'ordre de déclaration. Source unique du descripteur.
fn entities() -> Vec<Entity> {
    vec![plan(), task(), step(), decision(), constraint()]
}

// --- Descripteur ------------------------------------------------------------------

/// Le champ `status` d'une entité : le framework réserve `status` en **String** (champ de
/// base) ; un `Enum` y serait refusé (`base_field_redeclared`). On le surcharge donc en
/// gardant le type String, pour porter la valeur de départ du cycle de vie. Le vocabulaire
/// (draft/pending/...) vient de PO et est documenté à l'appel.
fn statut(defaut: &str) -> Field {
    Field::string("status").default(json!(defaut))
}

fn plan() -> Entity {
    Entity::new("plan", "plans")
        .field(Field::string("title"))
        .field(Field::new("description", FieldType::Text))
        // status : draft | approved | in_progress | completed | cancelled (PO PlanStatus)
        .field(statut("draft"))
        .field(
            Field::new("priority", FieldType::I32)
                .default(json!(0))
                .sortable(),
        )
        .field(Field::new("project_id", FieldType::Uuid).optional())
        .field(Field::new("created_by", FieldType::String).optional())
}

fn task() -> Entity {
    Entity::new("task", "tasks")
        // Relation task -> plan (une tâche appartient à un plan).
        .field(Field::new(
            "plan",
            FieldType::Ref {
                entity: "plan".to_string(),
            },
        ))
        .field(Field::string("title").optional())
        .field(Field::new("description", FieldType::Text))
        // status : pending | in_progress | blocked | completed | failed | interrupted (PO TaskStatus)
        .field(statut("pending"))
        .field(Field::new("priority", FieldType::I32).optional().sortable())
        .field(Field::new("assigned_to", FieldType::String).optional())
        .field(Field::new("tags", FieldType::List(Box::new(FieldType::String))).default(json!([])))
        .field(
            Field::new(
                "acceptance_criteria",
                FieldType::List(Box::new(FieldType::String)),
            )
            .default(json!([])),
        )
        .field(
            Field::new(
                "affected_files",
                FieldType::List(Box::new(FieldType::String)),
            )
            .default(json!([])),
        )
        .field(Field::new("estimated_complexity", FieldType::U32).optional())
        .field(Field::new("actual_complexity", FieldType::U32).optional())
        .field(
            Field::new("due_date", FieldType::Date)
                .optional()
                .sortable(),
        )
        .field(Field::new("estimated_minutes", FieldType::U32).optional())
        .field(Field::new("frustration_score", FieldType::F64).default(json!(0.0)))
}

fn step() -> Entity {
    Entity::new("step", "steps")
        // Relation step -> task.
        .field(Field::new(
            "task",
            FieldType::Ref {
                entity: "task".to_string(),
            },
        ))
        .field(Field::new("order", FieldType::U32).default(json!(0)))
        .field(Field::new("description", FieldType::Text))
        // status : pending | in_progress | completed | skipped (PO StepStatus)
        .field(statut("pending"))
        .field(Field::new("verification", FieldType::String).optional())
}

fn decision() -> Entity {
    Entity::new("decision", "decisions")
        // Relation decision -> task.
        .field(Field::new(
            "task",
            FieldType::Ref {
                entity: "task".to_string(),
            },
        ))
        .field(Field::new("description", FieldType::Text))
        .field(Field::new("rationale", FieldType::Text))
        .field(Field::new(
            "alternatives",
            FieldType::List(Box::new(FieldType::String)),
        ))
        .field(Field::new("chosen_option", FieldType::String).optional())
        .field(Field::new("decided_by", FieldType::String).optional())
        // status : proposed | accepted | deprecated | superseded (PO DecisionStatus)
        .field(statut("proposed"))
        .field(Field::new("scar_intensity", FieldType::F64).default(json!(0.0)))
}

fn constraint() -> Entity {
    Entity::new("constraint", "constraints")
        // Relation constraint -> plan.
        .field(Field::new(
            "plan",
            FieldType::Ref {
                entity: "plan".to_string(),
            },
        ))
        .field(Field::new(
            "constraint_type",
            FieldType::Enum {
                name: "constraint_type".to_string(),
                variants: vec![
                    "performance",
                    "compatibility",
                    "security",
                    "style",
                    "testing",
                    "other",
                ]
                .into_iter()
                .map(String::from)
                .collect(),
            },
        ))
        .field(Field::new("description", FieldType::Text))
        .field(Field::new("enforced_by", FieldType::String).optional())
}

// --- Vues (dérivées du descripteur) ------------------------------------------------

/// Les champs `sortable` d'une entité, dans l'ordre de déclaration.
fn sortable_fields(entity: &str) -> Vec<String> {
    entities()
        .into_iter()
        .find(|e| e.name == entity)
        .map(|e| {
            e.fields
                .into_iter()
                .filter(|f| f.sortable)
                .map(|f| f.name)
                .collect()
        })
        .unwrap_or_default()
}

/// Vue liste de `task` : son `sort` est **dérivé** des champs triables du descripteur,
/// jamais écrit en dur. Rendre `priority` triable l'y fait apparaître tout seul.
fn task_list_view() -> Value {
    let sort: Vec<Value> = sortable_fields("task")
        .into_iter()
        .map(|f| json!({"field": f, "dir": "asc"}))
        .collect();
    json!({
        "name": "taches",
        "surface": "main",
        "title": "Tâches",
        "root": {
            "type": "leaf",
            "brick": {
                "type": "list",
                "entity": "task",
                "label": "title",
                "subtitle": "status",
                "sort": sort,
                "empty": "Aucune tâche"
            }
        }
    })
}

/// Vue formulaire de `task`, soumise par l'action `creer_tache`.
fn task_form_view() -> Value {
    json!({
        "name": "nouvelle_tache",
        "surface": "panel",
        "title": "Nouvelle tâche",
        "root": {
            "type": "leaf",
            "brick": {
                "type": "form",
                "entity": "task",
                "submit": "creer_tache",
                "fields": [
                    { "field": "title", "label": "Titre" },
                    { "field": "description", "label": "Description" }
                ]
            }
        }
    })
}

// --- Module -----------------------------------------------------------------------

impl Module for PlanModule {
    fn manifest() -> Manifest {
        let entities = entities();
        let mut m = Manifest::new("plan", "0.1.0");
        m.entities = entities;
        m.actions = vec![json!({
            "kind": "create",
            "name": "creer_tache",
            "entity": "task",
            "fields": ["title", "description", "priority"]
        })];
        m.views = vec![task_list_view(), task_form_view()];
        m
    }

    fn handle(req: Request) -> Response {
        match req.op {
            Op::Create => create(&req),
            Op::Update => update(&req),
            Op::Get => get(&req),
            Op::List => list(&req),
            Op::Delete => delete(&req),
            Op::Call => Response::err(
                "operation_inconnue",
                "ce module ne fait que du CRUD (create/get/list/update/delete)",
            ),
        }
    }
}

// --- CRUD générique, piloté par le descripteur -------------------------------------

fn fields_of(entity: &str) -> Vec<Field> {
    entities()
        .into_iter()
        .find(|e| e.name == entity)
        .map(|e| e.fields)
        .unwrap_or_default()
}

fn entity_of(req: &Request) -> Result<&str, Response> {
    let Some(entity) = req.entity.as_deref() else {
        return Err(Response::err(
            "entite_manquante",
            "l'entité est requise (champ `entity`)",
        ));
    };
    if fields_of(entity).is_empty() {
        return Err(Response::err(
            "entite_inconnue",
            format!("entité inconnue : {entity}"),
        ));
    }
    Ok(entity)
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

/// Trie les lignes selon les champs `sortable` déclarés pour l'entité, dans l'ordre de
/// déclaration. Aucun nom de champ en dur : rendre un champ triable suffit.
fn list(req: &Request) -> Response {
    let entity = match entity_of(req) {
        Ok(e) => e,
        Err(r) => return r,
    };
    let sortable = sortable_fields(entity);
    let mut lignes: Vec<&Value> = req.rows.iter().collect();
    // Tri stable, de la dernière clé vers la première : la première déclarée est la clé
    // principale.
    for champ in sortable.iter().rev() {
        lignes.sort_by(|a, b| cmp_field(a, b, champ));
    }
    Response::ok(json!({"items": lignes, "total": lignes.len()}))
}

fn cmp_field(a: &Value, b: &Value, field: &str) -> Ordering {
    match (a.get(field), b.get(field)) {
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

this_module_sdk::export!(PlanModule);

#[cfg(test)]
mod tests {
    use super::*;
    use this_descriptor::{CheckProfile, CustomRegistry, ModuleDescriptor};
    use this_module_sdk::rt::run;

    fn appel(op: &str, req: Value) -> Value {
        serde_json::to_value(run::<PlanModule>(op.as_bytes(), req.to_string().as_bytes())).unwrap()
    }

    #[test]
    fn manifeste_passe_check_stored() {
        let json = serde_json::to_string(&PlanModule::manifest()).unwrap();
        let d = ModuleDescriptor::from_json(&json).expect("relu par le vrai descripteur");
        d.check(
            &CustomRegistry::default(),
            CheckProfile::Stored {
                allow_public: false,
            },
        )
        .unwrap_or_else(|e| panic!("check(Stored) : {e:?}"));
        assert_eq!(d.entities.len(), 5);
        assert_eq!(d.entities[0].plural, "plans");
    }

    #[test]
    fn crud_sans_stockage() {
        // Créer un plan : l'effet est un `put`, jamais une écriture en base.
        let r = appel(
            "create",
            json!({"entity": "plan", "input": {"title": "Parité", "description": "un plan"}}),
        );
        assert_eq!(r["ok"], true);
        assert_eq!(r["effects"][0]["kind"], "put");
        assert_eq!(r["effects"][0]["entity"], "plan");
        assert_eq!(r["effects"][0]["body"]["status"], "draft");
        assert_eq!(r["effects"][0]["body"]["priority"], 0);

        // Créer une tâche (ref plan + description requise, défauts appliqués).
        let r = appel(
            "create",
            json!({"entity": "task", "input": {"plan": "plan-1", "title": "T1", "description": "faire"}}),
        );
        assert_eq!(r["ok"], true);
        assert_eq!(r["effects"][0]["body"]["plan"], "plan-1");
        assert_eq!(r["effects"][0]["body"]["status"], "pending");

        // Mettre à jour la tâche (l'hôte fournit la ligne dans `rows`).
        let ligne = json!({"id": "t1", "plan": "plan-1", "title": "T1", "description": "faire", "status": "pending"});
        let r = appel(
            "update",
            json!({"entity": "task", "input": {"id": "t1", "status": "in_progress"}, "rows": [ligne]}),
        );
        assert_eq!(r["output"]["status"], "in_progress");
        assert_eq!(r["effects"][0]["kind"], "put");

        // Lire une ligne.
        let r = appel(
            "get",
            json!({"entity": "task", "input": {"id": "t1"}, "rows": [{"id": "t1", "title": "T1"}]}),
        );
        assert_eq!(r["output"]["id"], "t1");

        // Supprimer : un effet `delete`, pas d'écriture.
        let r = appel("delete", json!({"entity": "task", "input": {"id": "t1"}}));
        assert_eq!(r["effects"][0]["kind"], "delete");
        assert_eq!(r["effects"][0]["id"], "t1");
    }

    #[test]
    fn create_refuse_un_champ_requis_manquant() {
        let r = appel(
            "create",
            json!({"entity": "task", "input": {"plan": "plan-1", "title": "sans description"}}),
        );
        assert_eq!(r["ok"], false);
        assert_eq!(r["error"]["field"], "description");
    }

    #[test]
    fn liste_trie_par_les_champs_triables_du_descripteur() {
        let rows = json!([
            {"id": "a", "title": "b", "priority": 5, "status": "pending"},
            {"id": "b", "title": "a", "priority": 1, "status": "pending"},
            {"id": "c", "title": "c", "priority": 3, "status": "pending"}
        ]);
        let r = appel("list", json!({"entity": "task", "rows": rows}));
        let ids: Vec<String> = r["output"]["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|l| l["id"].as_str().unwrap().to_string())
            .collect();
        assert_eq!(ids, ["b", "c", "a"], "trié par priorité croissante");
    }

    #[test]
    fn la_vue_liste_derive_son_tri_du_descripteur() {
        let m = PlanModule::manifest();
        let vue = m.views.iter().find(|v| v["name"] == "taches").unwrap();
        assert_eq!(
            vue["root"]["brick"]["sort"],
            json!([{"field": "priority", "dir": "asc"}, {"field": "due_date", "dir": "asc"}])
        );
    }
}
