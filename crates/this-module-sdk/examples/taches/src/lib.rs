//! Module `taches` : l'exemple complet du SDK. Voir le README.

use this_module_sdk::{
    Effect, Entity, Field, Level, Manifest, Module, Op, OperationDecl, Request, Response,
    Transform, Value, json,
};

pub struct Taches;

const ENTITE: &str = "tache";

impl Module for Taches {
    fn manifest() -> Manifest {
        Manifest::new("taches", "0.1.0")
            .entity(
                Entity::new(ENTITE, "taches")
                    .field(Field::string("titre").transform(Transform::trim()))
                    .field(Field::bool("fait").default(json!(false)))
                    .field(Field::int("priorite").default(json!(0))),
            )
            .operation(
                OperationDecl::new("terminer", "Terminer la tâche")
                    .input(json!({"id": "uuid"}))
                    .output(json!({"tache": "tache"})),
            )
    }

    fn handle(req: Request) -> Response {
        match req.op {
            Op::Create => creer(&req),
            Op::Update => modifier(&req),
            Op::Get => lire(&req),
            Op::List => lister(&req),
            Op::Delete => supprimer(&req),
            Op::Call => match req.name.as_deref() {
                Some("terminer") => terminer(&req),
                other => Response::err(
                    "operation_inconnue",
                    format!("opération inconnue : {}", other.unwrap_or("(sans nom)")),
                ),
            },
        }
    }
}

/// Le titre de l'entrée, rogné ; vide ou blanc = refus.
fn titre_valide(v: &Value) -> Result<String, Response> {
    let t = v.as_str().unwrap_or("").trim();
    if t.is_empty() {
        Err(Response::field_err(
            "titre_vide",
            "le titre est vide",
            "titre",
        ))
    } else {
        Ok(t.to_string())
    }
}

fn priorite(v: Option<&Value>) -> Result<i64, Response> {
    match v {
        None | Some(Value::Null) => Ok(0),
        Some(v) => v.as_i64().ok_or_else(|| {
            Response::field_err(
                "priorite_invalide",
                "la priorité doit être un entier",
                "priorite",
            )
        }),
    }
}

fn creer(req: &Request) -> Response {
    let titre = match titre_valide(req.input.get("titre").unwrap_or(&Value::Null)) {
        Ok(t) => t,
        Err(r) => return r,
    };
    let priorite = match priorite(req.input.get("priorite")) {
        Ok(p) => p,
        Err(r) => return r,
    };
    let corps = json!({"titre": titre, "fait": false, "priorite": priorite});
    Response::ok(corps.clone()).effect(Effect::put(ENTITE, corps))
}

fn modifier(req: &Request) -> Response {
    let Some(id) = req.input.get("id").and_then(Value::as_str) else {
        return Response::field_err("id_manquant", "l'id est requis", "id");
    };
    let Some(mut ligne) = req.row(id).cloned() else {
        return Response::err("introuvable", "tâche introuvable");
    };
    if let Some(t) = req.input.get("titre") {
        match titre_valide(t) {
            Ok(t) => ligne["titre"] = json!(t),
            Err(r) => return r,
        }
    }
    if req.input.get("priorite").is_some() {
        match priorite(req.input.get("priorite")) {
            Ok(p) => ligne["priorite"] = json!(p),
            Err(r) => return r,
        }
    }
    if let Some(f) = req.input.get("fait").and_then(Value::as_bool) {
        ligne["fait"] = json!(f);
    }
    Response::ok(ligne.clone()).effect(Effect::put(ENTITE, ligne))
}

fn lire(req: &Request) -> Response {
    let id = req.input.get("id").and_then(Value::as_str).unwrap_or("");
    match req.row(id) {
        Some(l) => Response::ok(l.clone()),
        None => Response::err("introuvable", "tâche introuvable"),
    }
}

fn supprimer(req: &Request) -> Response {
    match req.input.get("id").and_then(Value::as_str) {
        Some(id) => Response::ok(json!({"id": id})).effect(Effect::delete(ENTITE, id)),
        None => Response::field_err("id_manquant", "l'id est requis", "id"),
    }
}

/// `ctx.config` : `masquer_faites` (bool), `priorite_min` (entier), `tri` (`priorite` | `titre`).
fn lister(req: &Request) -> Response {
    let masquer = req
        .config("masquer_faites")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let min = req.config("priorite_min").and_then(Value::as_i64);
    let mut lignes: Vec<&Value> = req
        .rows
        .iter()
        .filter(|l| !(masquer && l["fait"].as_bool().unwrap_or(false)))
        .filter(|l| min.is_none_or(|m| l["priorite"].as_i64().unwrap_or(0) >= m))
        .collect();
    match req
        .config("tri")
        .and_then(Value::as_str)
        .unwrap_or("priorite")
    {
        "titre" => lignes.sort_by_key(|l| l["titre"].as_str().unwrap_or("").to_lowercase()),
        // priorité décroissante ; à égalité, l'ordre de l'hôte est conservé (tri stable)
        _ => lignes.sort_by_key(|l| std::cmp::Reverse(l["priorite"].as_i64().unwrap_or(0))),
    }
    let total = lignes.len();
    let mut r = Response::ok(json!({"items": lignes, "total": total}));
    if req.ctx.truncated {
        r = r.effect(Effect::log(Level::Warn, "liste tronquée par l'hôte"));
    }
    r
}

fn terminer(req: &Request) -> Response {
    let Some(id) = req.input.get("id").and_then(Value::as_str) else {
        return Response::field_err("id_manquant", "l'id est requis", "id");
    };
    let Some(mut ligne) = req.row(id).cloned() else {
        return Response::err("introuvable", "tâche introuvable");
    };
    ligne["fait"] = json!(true);
    Response::ok(ligne.clone()).effect(Effect::put(ENTITE, ligne))
}

this_module_sdk::export!(Taches);

#[cfg(test)]
mod tests {
    use super::*;
    use this_descriptor::{CheckProfile, CustomRegistry, ModuleDescriptor};
    use this_module_sdk::rt::run;

    fn appel(op: &str, req: Value) -> Value {
        serde_json::to_value(run::<Taches>(op.as_bytes(), req.to_string().as_bytes())).unwrap()
    }

    #[test]
    fn le_manifeste_passe_check_stored() {
        let json = serde_json::to_string(&Taches::manifest()).unwrap();
        let d = ModuleDescriptor::from_json(&json).expect("relu par le vrai descripteur");
        d.check(
            &CustomRegistry::default(),
            CheckProfile::Stored {
                allow_public: false,
            },
        )
        .unwrap_or_else(|e| panic!("check(Stored) : {e:?}"));
        assert_eq!(d.entities[0].plural, "taches");
    }

    #[test]
    fn titre_vide_ou_blanc_refuse() {
        for t in ["", "   ", "\t\n"] {
            let r = appel("create", json!({"input": {"titre": t}}));
            assert_eq!(
                r,
                json!({"ok":false,"error":{"code":"titre_vide","message":"le titre est vide","field":"titre"}})
            );
        }
    }

    #[test]
    fn creation_valide_rogne_le_titre() {
        let r = appel(
            "create",
            json!({"input": {"titre": "  Écrire  ", "priorite": 3}}),
        );
        assert_eq!(r["ok"], true);
        assert_eq!(r["effects"][0]["body"]["titre"], "Écrire");
    }

    #[test]
    fn terminer_rend_la_ligne_et_un_put() {
        let ligne = json!({"id":"a","titre":"x","fait":false,"priorite":1});
        let r = appel(
            "call",
            json!({"name":"terminer","input":{"id":"a"},"rows":[ligne]}),
        );
        assert_eq!(r["output"]["fait"], true);
        assert_eq!(r["effects"][0]["kind"], "put");
        assert_eq!(r["effects"][0]["body"]["id"], "a");
    }

    #[test]
    fn list_filtre_et_trie_selon_la_config() {
        let rows = json!([
            {"id":"1","titre":"b","fait":false,"priorite":1},
            {"id":"2","titre":"a","fait":true,"priorite":5},
            {"id":"3","titre":"c","fait":false,"priorite":3}
        ]);
        let ids = |r: &Value| -> Vec<String> {
            r["output"]["items"]
                .as_array()
                .unwrap()
                .iter()
                .map(|l| l["id"].as_str().unwrap().into())
                .collect()
        };
        let r = appel("list", json!({"rows": rows}));
        assert_eq!(ids(&r), ["2", "3", "1"]);
        let r = appel(
            "list",
            json!({"rows": rows, "ctx": {"config": {"masquer_faites": true, "tri": "titre"}}}),
        );
        assert_eq!(ids(&r), ["1", "3"]);
        let r = appel(
            "list",
            json!({"rows": rows, "ctx": {"config": {"priorite_min": 3}}}),
        );
        assert_eq!(ids(&r), ["2", "3"]);
    }
}
