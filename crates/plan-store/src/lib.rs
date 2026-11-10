//! `plan-store` — l'adaptateur de stockage d'un module `this`.
//!
//! Un module (`handle(Request) -> Response`) est une fonction pure : il émet des
//! `Effect::put` / `Effect::delete` et ne touche jamais la base. Cet adaptateur est son
//! pendant **hôte**. Il a deux moitiés, symétriques :
//!
//! * **read path** — [`Store::populate`] remplit `Request::rows` depuis le stockage réel
//!   avant l'appel (`get`/`update` → la ligne d'`id`, `list` → toutes les lignes de
//!   l'entité) ;
//! * **write path** — [`Store::apply`] applique les `Effect`s de la réponse après l'appel
//!   (`put` → insère/met à jour la ligne, `delete` → la retire).
//!
//! [`Store::run`] compose les deux autour de `handle` : `populate` → `handle` → `apply`.
//!
//! # Généricité — la mesure qui compte
//!
//! Le `Store` ne connaît **aucune** entité : ni leur nom, ni leur nombre, ni leurs champs.
//! Il ne manipule que des `Value` opaques, indexées par `(entity, id)`. Les noms d'entité
//! lui arrivent par `Request::entity` et `Effect::Put::entity` ; la seule convention qu'il
//! partage avec le module est le nom du champ `id`, fixé par l'ABI v1 (`Request::row`,
//! `Effect::Delete::id`) — pas par un domaine. Le descripteur (`Manifest`) vit uniquement
//! dans le module ; le store n'en a pas besoin.
//!
//! Conséquence : ce crate est réutilisable **tel quel** par n'importe quel module qui
//! déclare des champs de mêmes types. Aucune ligne n'est propre à un domaine.

use std::collections::{BTreeMap, HashMap};

use serde_json::{Value, json};
use this_module_sdk::{Effect, Module, Op, Request, Response};

/// Stockage en mémoire d'un hôte de module : des lignes `Value` indexées par `(entity, id)`.
///
/// La table est un `BTreeMap` : l'itération est déterministe (les `list` avant tri sont
/// stables), sans que le store ait à trier lui-même — le tri est le travail du module.
#[derive(Default)]
pub struct Store {
    /// `entity` (nom singulier, celui de `Request::entity`) → lignes indexées par `id`.
    tables: HashMap<String, BTreeMap<String, Value>>,
    /// Compteur monotone par entité : génère les `id` des `put` de création (sans `id`).
    seq: HashMap<String, u64>,
}

impl Store {
    pub fn new() -> Self {
        Self::default()
    }

    /// Read path. Peuple `req.rows` depuis le stockage. Les autres champs (op, entity,
    /// input, ctx) sont inchangés : le module reçoit exactement ce qu'un hôte réel lui
    /// enverrait.
    pub fn populate(&self, mut req: Request) -> Request {
        let Some(entity) = req.entity.as_deref() else {
            return req;
        };
        match req.op {
            Op::Get | Op::Update => {
                if let Some(id) = req.input.get("id").and_then(Value::as_str)
                    && let Some(row) = self.row(entity, id)
                {
                    req.rows = vec![row.clone()];
                }
            }
            Op::List => {
                req.rows = self.rows(entity).cloned().collect();
            }
            Op::Create | Op::Delete | Op::Call => {}
        }
        req
    }

    /// Write path. Applique les effets d'une réponse, dans l'ordre. Un échec n'a aucun
    /// effet. Les effets `Emit` et `Log` sont ignorés (ce ne sont pas des écritures).
    pub fn apply(&mut self, resp: &Response) {
        if let Response::Ok { effects, .. } = resp {
            for e in effects {
                self.apply_effect(e);
            }
        }
    }

    /// L'aller-retour complet : `populate` → `handle` → `apply`.
    pub fn run<M: Module>(&mut self, req: Request) -> Response {
        let req = self.populate(req);
        let resp = M::handle(req);
        self.apply(&resp);
        resp
    }

    fn apply_effect(&mut self, e: &Effect) {
        match e {
            Effect::Put { entity, body } => self.put(entity, body),
            Effect::Delete { entity, id } => {
                if let Some(t) = self.tables.get_mut(entity) {
                    t.remove(id);
                }
            }
            Effect::Emit { .. } | Effect::Log { .. } => {}
        }
    }

    /// Insère ou remplace la ligne. Sans `id` dans le corps (création), en génère un ;
    /// avec (mise à jour), le conserve — et n'avance pas le compteur.
    fn put(&mut self, entity: &str, body: &Value) {
        let id = body
            .get("id")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .unwrap_or_else(|| self.next_id(entity));
        let mut row = body.clone();
        if let Some(obj) = row.as_object_mut() {
            obj.insert("id".to_owned(), json!(id));
        }
        self.tables
            .entry(entity.to_owned())
            .or_default()
            .insert(id, row);
    }

    fn next_id(&mut self, entity: &str) -> String {
        let n = self.seq.entry(entity.to_owned()).or_default();
        *n += 1;
        format!("{entity}-{n}")
    }

    /// Une ligne par `id`, si elle existe.
    pub fn row(&self, entity: &str, id: &str) -> Option<&Value> {
        self.tables.get(entity).and_then(|t| t.get(id))
    }

    /// Toutes les lignes d'une entité, dans l'ordre déterministe de l'`id`.
    pub fn rows(&self, entity: &str) -> impl Iterator<Item = &Value> {
        self.tables.get(entity).into_iter().flat_map(|t| t.values())
    }

    /// Nombre de lignes stockées pour une entité.
    pub fn len(&self, entity: &str) -> usize {
        self.tables.get(entity).map_or(0, BTreeMap::len)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use this_module_sdk::{Entity, Field, FieldType, Manifest};

    /// Un second module, volontairement *autre* que le domaine de plans : une entité
    /// `note` aux champs inédits. S'il passe par le même `Store` sans qu'aucune ligne de
    /// celui-ci change, la généricité est prouvée par l'usage, pas par l'affirmation.
    struct Notes;

    impl Module for Notes {
        fn manifest() -> Manifest {
            let mut m = Manifest::new("notes", "0.1.0");
            m.entities = vec![
                Entity::new("note", "notes")
                    .field(Field::string("texte"))
                    .field(Field::new("tiret", FieldType::Bool)),
            ];
            m
        }

        fn handle(req: Request) -> Response {
            match req.op {
                Op::Create => {
                    let entity = req.entity.clone().unwrap_or_default();
                    Response::ok(req.input.clone()).effect(Effect::put(entity, req.input))
                }
                Op::List => Response::ok(json!({ "items": req.rows })),
                Op::Get => match req.input.get("id").and_then(Value::as_str) {
                    Some(id) => match req.row(id) {
                        Some(r) => Response::ok(r.clone()),
                        None => Response::err("introuvable", "absent"),
                    },
                    None => Response::field_err("id_manquant", "id requis", "id"),
                },
                _ => Response::err("inconnu", "op non gérée"),
            }
        }
    }

    fn req(v: Value) -> Request {
        serde_json::from_value(v).expect("requête valide")
    }

    #[test]
    fn populate_list_injecte_les_lignes() {
        let mut s = Store::new();
        s.put("note", &json!({"texte": "a"}));
        s.put("note", &json!({"texte": "b"}));
        let r = s.populate(req(json!({"op": "list", "entity": "note"})));
        assert_eq!(r.rows.len(), 2);
    }

    #[test]
    fn put_sans_id_genere_un_id() {
        let mut s = Store::new();
        s.put("note", &json!({"texte": "a"}));
        let id = s.rows("note").next().unwrap()["id"]
            .as_str()
            .unwrap()
            .to_owned();
        assert!(s.row("note", &id).is_some());
    }

    #[test]
    fn le_store_est_generique_deux_domaines_sans_changement() {
        let mut s = Store::new();
        let resp = s.run::<Notes>(req(json!({
            "op": "create", "entity": "note",
            "input": {"texte": "bonjour", "tiret": true}
        })));
        assert!(matches!(resp, Response::Ok { .. }));
        assert_eq!(s.len("note"), 1);

        let resp = s.run::<Notes>(req(json!({"op": "list", "entity": "note"})));
        let Response::Ok { output, .. } = resp else {
            panic!("liste attendue")
        };
        let items = output["items"].as_array().unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0]["texte"], "bonjour");
        assert_eq!(items[0]["tiret"], true);
    }
}
