//! `JsonEntityService` : le service d'une entité déclarative, avec et sans logique métier.
//!
//! Le service est construit directement (descripteur compilé + magasin de lignes mémoire +
//! bus d'événements). Deux doubles de test : une `EntityLogic` scriptée (elle rend ce qu'on
//! lui a dit de rendre et note ce qu'elle a reçu) et un `EffectSink` enregistreur (il note
//! ce qu'on lui a demandé de vérifier puis d'appliquer). Les routes REST sont appelées par
//! `oneshot`, comme un client.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use serde_json::{Value, json};
use tower::ServiceExt;
use uuid::Uuid;

use this::core::errors::from_anyhow;
use this::core::events::{EntityEvent, EventBus, EventEnvelope, FrameworkEvent};
use this::core::module::{EntityCreator, EntityFetcher};
use this::descriptor::framework::codes;
use this::descriptor::{
    CompiledEntity, ContractError, CustomRegistry, EntityDesc, ErrorKind, FieldDesc, FieldType,
    FilterOp,
};
use this::server::module_runtime::{
    Effect, EffectSink, EntityLogic, JsonEntityService, LogLevel, LogicOutcome, MemoryRowStore,
    RowStore,
};

// ───────────────────────── fixtures ─────────────────────────

fn task_desc() -> EntityDesc {
    let mut e = EntityDesc::new("task");
    e.plural = "tasks".into();
    let mut title = FieldDesc::new("title", FieldType::String);
    title.filterable = vec![FilterOp::Eq];
    title.sortable = true;
    e.fields.push(title);
    e.fields
        .push(FieldDesc::new("done", FieldType::Bool).optional());
    e
}

fn service(bus: Option<Arc<EventBus>>) -> JsonEntityService {
    let entity = Arc::new(CompiledEntity::new("todo", task_desc()).unwrap());
    JsonEntityService::new(entity, CustomRegistry::default(), bus)
}

/// Une logique scriptée : chaque crochet rend la réponse qu'on a posée (ou un refus), et note
/// sa propre invocation.
#[derive(Default)]
struct Scripted {
    create: Option<LogicOutcome>,
    update: Option<LogicOutcome>,
    delete: Option<LogicOutcome>,
    get: Option<LogicOutcome>,
    list: Option<LogicOutcome>,
    ops: HashMap<String, LogicOutcome>,
    refuse: bool,
    seen: Mutex<Vec<String>>,
}

impl Scripted {
    fn note(&self, what: String) {
        self.seen.lock().unwrap().push(what);
    }
    fn seen(&self) -> Vec<String> {
        self.seen.lock().unwrap().clone()
    }
    /// `None` : la logique refuse. Sinon la réponse posée (ou une réponse vide).
    fn answer(&self, o: &Option<LogicOutcome>) -> Option<LogicOutcome> {
        if self.refuse {
            None
        } else {
            Some(o.clone().unwrap_or_default())
        }
    }
    fn refused() -> ContractError {
        ContractError::framework(codes::VALIDATION_FAILED)
            .with_param("reason", "refus de la logique")
    }
}

#[async_trait]
impl EntityLogic for Scripted {
    async fn create(&self, input: &Value) -> Result<LogicOutcome, ContractError> {
        self.note(format!("create {input}"));
        self.answer(&self.create).ok_or_else(Self::refused)
    }
    async fn update(
        &self,
        id: &str,
        patch: &Value,
        current: &Value,
    ) -> Result<LogicOutcome, ContractError> {
        self.note(format!("update {id} {patch} {}", current["title"]));
        self.answer(&self.update).ok_or_else(Self::refused)
    }
    async fn delete(&self, id: &str, current: &Value) -> Result<LogicOutcome, ContractError> {
        self.note(format!("delete {id} {}", current["title"]));
        self.answer(&self.delete).ok_or_else(Self::refused)
    }
    async fn get(&self, row: &Value) -> Result<LogicOutcome, ContractError> {
        self.note(format!("get {}", row["title"]));
        self.answer(&self.get).ok_or_else(Self::refused)
    }
    async fn list(&self, rows: &[Value]) -> Result<LogicOutcome, ContractError> {
        self.note(format!("list {}", rows.len()));
        self.answer(&self.list).ok_or_else(Self::refused)
    }
    async fn call(
        &self,
        name: &str,
        input: &Value,
        rows: &[Value],
    ) -> Result<Option<LogicOutcome>, ContractError> {
        self.note(format!("call {name} {input} {}", rows.len()));
        if self.refuse {
            return Err(ContractError::framework(codes::VALIDATION_FAILED));
        }
        Ok(self.ops.get(name).cloned())
    }
}

/// Une logique qui ne redéfinit RIEN : tous les crochets par défaut.
struct Passive;
#[async_trait]
impl EntityLogic for Passive {}

#[derive(Default)]
struct Sink {
    checked: Mutex<Vec<Vec<Effect>>>,
    applied: Mutex<Vec<Vec<Effect>>>,
    fail_check: bool,
    fail_apply: bool,
}

#[async_trait]
impl EffectSink for Sink {
    async fn check(&self, effects: &[Effect]) -> Result<(), ContractError> {
        self.checked.lock().unwrap().push(effects.to_vec());
        if self.fail_check {
            return Err(ContractError::framework(codes::VALIDATION_FAILED)
                .with_param("reason", "effet refusé à la vérification"));
        }
        Ok(())
    }
    async fn apply(&self, effects: Vec<Effect>) -> Result<(), ContractError> {
        self.applied.lock().unwrap().push(effects);
        if self.fail_apply {
            return Err(ContractError::framework(codes::INTERNAL)
                .with_param("reason", "effet refusé à l'application"));
        }
        Ok(())
    }
}

fn with_logic(logic: Scripted, sink: Sink) -> (Arc<JsonEntityService>, Arc<Scripted>, Arc<Sink>) {
    let logic = Arc::new(logic);
    let sink = Arc::new(sink);
    let svc = Arc::new(service(None).with_logic(logic.clone(), sink.clone()));
    (svc, logic, sink)
}

fn outcome(output: Option<Value>, effects: Vec<Effect>) -> Option<LogicOutcome> {
    Some(LogicOutcome { output, effects })
}

fn log(msg: &str) -> Effect {
    Effect::Log {
        level: LogLevel::Info,
        message: msg.into(),
    }
}

fn put(entity: &str, body: Value) -> Effect {
    Effect::Put {
        entity: entity.into(),
        body,
    }
}

fn id_of(row: &Value) -> Uuid {
    Uuid::parse_str(row["id"].as_str().unwrap()).unwrap()
}

fn code_of(e: &anyhow::Error) -> String {
    from_anyhow(e).code.as_str().to_string()
}

async fn titles(svc: &JsonEntityService) -> Vec<String> {
    svc.list_as_json(None, None)
        .await
        .unwrap()
        .iter()
        .map(|r| r["title"].as_str().unwrap().to_string())
        .collect()
}

// ───────────────────────── sans logique ─────────────────────────

#[tokio::test]
async fn names_come_from_the_descriptor() {
    let svc = service(None);
    assert_eq!(svc.entity_type(), "task");
    assert_eq!(svc.plural(), "tasks");
}

#[tokio::test]
async fn a_declarative_entity_stamps_stores_and_serves_its_rows() {
    let svc = service(None);
    let row = svc
        .create_from_json(json!({"title": "écrire"}))
        .await
        .unwrap();
    assert_eq!(row["title"], "écrire");
    assert_eq!(row["type"], "task");
    assert_eq!(row["status"], "active");
    assert_eq!(row["created_at"], row["updated_at"]);
    let id = id_of(&row);
    assert_eq!(svc.fetch_as_json(&id).await.unwrap(), row);
    assert_eq!(
        svc.list_as_json(None, None).await.unwrap(),
        vec![row.clone()]
    );

    let updated = svc
        .update_from_json(&id, json!({"done": true}))
        .await
        .unwrap();
    assert_eq!(updated["done"], true);
    assert_eq!(updated["title"], "écrire", "la mise à jour fusionne");
    assert_eq!(updated["created_at"], row["created_at"]);

    EntityCreator::delete(&svc, &id).await.unwrap();
    assert!(svc.list_as_json(None, None).await.unwrap().is_empty());
}

#[tokio::test]
async fn invalid_writes_are_refused_with_the_descriptor_codes() {
    let svc = service(None);
    // Un champ requis manquant, puis un champ inconnu.
    let e = svc.create_from_json(json!({})).await.unwrap_err();
    assert_eq!(from_anyhow(&e).kind, ErrorKind::Validation);
    assert!(svc.list_as_json(None, None).await.unwrap().is_empty());
    assert!(
        svc.create_from_json(json!({"title": "x", "inconnu": 1}))
            .await
            .is_err()
    );
    assert!(svc.list_as_json(None, None).await.unwrap().is_empty());
}

#[tokio::test]
async fn missing_rows_are_not_found_on_every_operation() {
    let svc = service(None);
    let id = Uuid::new_v4();
    for e in [
        svc.fetch_as_json(&id).await.unwrap_err(),
        svc.update_from_json(&id, json!({"done": true}))
            .await
            .unwrap_err(),
        EntityCreator::delete(&svc, &id).await.unwrap_err(),
    ] {
        assert_eq!(code_of(&e), codes::ENTITY_NOT_FOUND.to_string());
    }
}

#[tokio::test]
async fn the_sample_entity_has_every_declared_and_base_field_as_null() {
    let sample = service(None).get_sample_entity().await.unwrap();
    let obj = sample.as_object().unwrap();
    assert_eq!(obj["title"], Value::Null);
    assert_eq!(obj["done"], Value::Null);
    assert!(obj.contains_key("id"), "{obj:?}");
    assert!(obj.values().all(Value::is_null));
}

#[tokio::test]
async fn list_applies_offset_and_limit_and_clamps_negatives() {
    let svc = service(None);
    for t in ["a", "b", "c", "d"] {
        svc.create_from_json(json!({"title": t})).await.unwrap();
    }
    let page = |l, o| {
        let svc = &svc;
        async move {
            svc.list_as_json(l, o)
                .await
                .unwrap()
                .iter()
                .map(|r| r["title"].as_str().unwrap().to_string())
                .collect::<Vec<_>>()
        }
    };
    assert_eq!(page(None, None).await, ["a", "b", "c", "d"]);
    assert_eq!(page(Some(2), None).await, ["a", "b"]);
    assert_eq!(page(Some(2), Some(1)).await, ["b", "c"]);
    assert_eq!(page(None, Some(3)).await, ["d"]);
    assert_eq!(page(Some(-5), None).await, Vec::<String>::new());
    assert_eq!(page(None, Some(-2)).await, ["a", "b", "c", "d"]);
}

#[tokio::test]
async fn named_operations_do_not_exist_without_logic() {
    let svc = service(None);
    let e = svc.call_operation("archiver", json!({})).await.unwrap_err();
    assert_eq!(code_of(&e), codes::TRANSPORT_NOT_FOUND.to_string());
}

// ───────────────────────── logique passive (crochets par défaut) ─────────────────────────

#[tokio::test]
async fn a_logic_that_overrides_nothing_leaves_the_service_unchanged() {
    let svc = Arc::new(service(None).with_logic(Arc::new(Passive), Arc::new(Sink::default())));
    let row = svc.create_from_json(json!({"title": "a"})).await.unwrap();
    let id = id_of(&row);
    assert_eq!(svc.fetch_as_json(&id).await.unwrap(), row);
    assert_eq!(svc.list_as_json(None, None).await.unwrap(), vec![row]);
    let up = svc
        .update_from_json(&id, json!({"title": "b"}))
        .await
        .unwrap();
    assert_eq!(up["title"], "b");
    // `call` rend `None` par défaut : pas d'opération de ce nom.
    let e = svc.call_operation("x", json!(null)).await.unwrap_err();
    assert_eq!(code_of(&e), codes::TRANSPORT_NOT_FOUND.to_string());
    EntityCreator::delete(&*svc, &id).await.unwrap();
    assert!(titles(&svc).await.is_empty());
}

// ───────────────────────── logique : create ─────────────────────────

#[tokio::test]
async fn create_hands_the_validated_payload_to_the_logic() {
    let (svc, logic, _) = with_logic(Scripted::default(), Sink::default());
    svc.create_from_json(json!({"title": "a"})).await.unwrap();
    let seen = logic.seen();
    assert_eq!(seen.len(), 1);
    assert!(seen[0].starts_with("create {"), "{seen:?}");
    assert!(seen[0].contains(r#""title":"a""#), "{seen:?}");
}

#[tokio::test]
async fn a_logic_output_replaces_the_row_and_is_validated_again() {
    let (svc, _, _) = with_logic(
        Scripted {
            create: outcome(Some(json!({"title": "remplacé"})), vec![]),
            ..Default::default()
        },
        Sink::default(),
    );
    let row = svc
        .create_from_json(json!({"title": "origine"}))
        .await
        .unwrap();
    assert_eq!(row["title"], "remplacé");

    // Une réplique que le descripteur interdit ne peut pas être persistée.
    let (svc, _, _) = with_logic(
        Scripted {
            create: outcome(Some(json!({"title": "x", "interdit": true})), vec![]),
            ..Default::default()
        },
        Sink::default(),
    );
    assert!(svc.create_from_json(json!({"title": "o"})).await.is_err());
    assert!(titles(&svc).await.is_empty());
}

#[tokio::test]
async fn a_put_effect_carrying_an_id_is_not_a_create_and_stays_an_effect() {
    let (svc, _, sink) = with_logic(
        Scripted {
            create: outcome(
                Some(json!({"title": "sortie"})),
                vec![
                    log("avant"),
                    put(
                        "task",
                        json!({"title": "via put", "id": null_id(), "created_at": "ignoré"}),
                    ),
                    put("autre", json!({"k": 1})),
                ],
            ),
            ..Default::default()
        },
        Sink::default(),
    );
    // Le `put` visé porte un `id` (null) : ce n'est donc PAS une création, il reste un effet.
    let row = svc.create_from_json(json!({"title": "o"})).await.unwrap();
    assert_eq!(row["title"], "sortie");
    assert_eq!(titles(&svc).await, ["sortie"]);
    let applied = sink.applied.lock().unwrap().clone();
    assert_eq!(applied.len(), 1);
    assert_eq!(applied[0].len(), 3, "aucun effet retiré : {applied:?}");
}

fn null_id() -> Value {
    json!("00000000-0000-0000-0000-000000000000")
}

#[tokio::test]
async fn the_first_idless_put_on_this_entity_becomes_the_write_and_leaves_the_effects() {
    let (svc, _, sink) = with_logic(
        Scripted {
            create: outcome(
                Some(json!({"title": "sortie"})),
                vec![
                    log("avant"),
                    put("autre", json!({"k": 1})),
                    put(
                        "task",
                        json!({"title": "via put", "created_at": "ignoré", "inconnu": 3}),
                    ),
                    put("task", json!({"title": "second put"})),
                ],
            ),
            ..Default::default()
        },
        Sink::default(),
    );
    let row = svc.create_from_json(json!({"title": "o"})).await.unwrap();
    // Une seule ligne : celle du premier `put`, pas la sortie, pas deux lignes.
    assert_eq!(row["title"], "via put");
    assert_ne!(
        row["created_at"], "ignoré",
        "les champs non déclarés sont retirés"
    );
    assert!(row.get("inconnu").is_none());
    assert_eq!(titles(&svc).await, ["via put"]);
    // Le puits a VÉRIFIÉ les quatre effets, puis APPLIQUÉ les trois qui restent, dans l'ordre.
    let checked = sink.checked.lock().unwrap().clone();
    assert_eq!(checked.len(), 1);
    assert_eq!(checked[0].len(), 4);
    let applied = sink.applied.lock().unwrap().clone();
    assert_eq!(applied.len(), 1);
    assert_eq!(
        applied[0],
        vec![
            log("avant"),
            put("autre", json!({"k": 1})),
            put("task", json!({"title": "second put"})),
        ]
    );
}

#[tokio::test]
async fn a_logic_refusal_stores_nothing() {
    let sink = Arc::new(Sink::default());
    let (svc, rows, _) = seeded(
        Scripted {
            refuse: true,
            ..Default::default()
        },
        sink.clone(),
    );
    assert!(svc.create_from_json(json!({"title": "a"})).await.is_err());
    assert_eq!(rows.count().await.unwrap(), 0);
    assert!(sink.applied.lock().unwrap().is_empty());
}

#[tokio::test]
async fn an_effect_refused_at_check_leaves_no_row_and_applies_nothing() {
    let (svc, _, sink) = with_logic(
        Scripted {
            create: outcome(None, vec![log("x")]),
            ..Default::default()
        },
        Sink {
            fail_check: true,
            ..Default::default()
        },
    );
    let e = svc
        .create_from_json(json!({"title": "a"}))
        .await
        .unwrap_err();
    assert_eq!(code_of(&e), codes::VALIDATION_FAILED.to_string());
    assert!(titles(&svc).await.is_empty(), "la ligne n'est pas écrite");
    assert!(sink.applied.lock().unwrap().is_empty());
}

#[tokio::test]
async fn an_effect_failing_at_apply_surfaces_the_error() {
    let (svc, _, sink) = with_logic(
        Scripted {
            create: outcome(None, vec![log("x")]),
            ..Default::default()
        },
        Sink {
            fail_apply: true,
            ..Default::default()
        },
    );
    let e = svc
        .create_from_json(json!({"title": "a"}))
        .await
        .unwrap_err();
    assert_eq!(code_of(&e), codes::INTERNAL.to_string());
    assert_eq!(sink.applied.lock().unwrap().len(), 1);
}

// ───────────────────────── logique : update ─────────────────────────

#[tokio::test]
async fn update_gives_the_logic_the_id_the_patch_and_the_current_row() {
    let (svc, logic, _) = with_logic(Scripted::default(), Sink::default());
    let row = svc
        .create_from_json(json!({"title": "avant"}))
        .await
        .unwrap();
    let id = id_of(&row);
    let up = svc
        .update_from_json(&id, json!({"title": "après"}))
        .await
        .unwrap();
    assert_eq!(up["title"], "après");
    let seen = logic.seen();
    assert_eq!(
        seen[1],
        format!(r#"update {id} {{"title":"après"}} "avant""#),
        "{seen:?}"
    );
}

#[tokio::test]
async fn update_of_a_missing_row_is_refused_before_the_logic_runs() {
    let (svc, logic, _) = with_logic(Scripted::default(), Sink::default());
    let e = svc
        .update_from_json(&Uuid::new_v4(), json!({"done": true}))
        .await
        .unwrap_err();
    assert_eq!(code_of(&e), codes::ENTITY_NOT_FOUND.to_string());
    assert!(logic.seen().is_empty());
}

#[tokio::test]
async fn an_update_output_replaces_the_patch_and_is_validated_again() {
    let (svc, _, _) = with_logic(
        Scripted {
            update: outcome(Some(json!({"title": "forcé"})), vec![]),
            ..Default::default()
        },
        Sink::default(),
    );
    let id = id_of(&svc.create_from_json(json!({"title": "a"})).await.unwrap());
    let up = svc
        .update_from_json(&id, json!({"done": true}))
        .await
        .unwrap();
    assert_eq!(up["title"], "forcé");
    assert_eq!(up["done"], Value::Null, "le patch d'origine est remplacé");

    let (svc, _, _) = with_logic(
        Scripted {
            update: outcome(Some(json!({"interdit": 1})), vec![]),
            ..Default::default()
        },
        Sink::default(),
    );
    let id = id_of(&svc.create_from_json(json!({"title": "a"})).await.unwrap());
    assert!(
        svc.update_from_json(&id, json!({"done": true}))
            .await
            .is_err()
    );
}

/// Un service dont le magasin est pré-rempli d'une ligne connue : la logique peut alors viser
/// son `id` d'avance.
fn seeded(logic: Scripted, sink: Arc<Sink>) -> (JsonEntityService, Arc<MemoryRowStore>, Uuid) {
    let rows = Arc::new(MemoryRowStore::new());
    let seed = Uuid::new_v4();
    let entity = Arc::new(CompiledEntity::new("todo", task_desc()).unwrap());
    let svc =
        JsonEntityService::with_row_store(entity, CustomRegistry::default(), None, rows.clone())
            .with_logic(Arc::new(logic), sink);
    (svc, rows, seed)
}

async fn seed_row(rows: &MemoryRowStore, id: Uuid, title: &str) {
    rows.insert(id, json!({"id": id, "title": title, "type": "task"}))
        .await
        .unwrap();
}

#[tokio::test]
async fn a_put_effect_aimed_at_this_row_is_the_update_itself() {
    let sink = Arc::new(Sink::default());
    let rows = Arc::new(MemoryRowStore::new());
    let seed = Uuid::new_v4();
    seed_row(&rows, seed, "a").await;
    let entity = Arc::new(CompiledEntity::new("todo", task_desc()).unwrap());
    let logic = Scripted {
        update: outcome(
            None,
            vec![
                put(
                    "task",
                    json!({"id": seed.to_string(), "title": "par effet"}),
                ),
                log("après"),
            ],
        ),
        ..Default::default()
    };
    let svc = JsonEntityService::with_row_store(entity, CustomRegistry::default(), None, rows)
        .with_logic(Arc::new(logic), sink.clone());
    let up = svc
        .update_from_json(&seed, json!({"title": "patch"}))
        .await
        .unwrap();
    // Le `put` visant CETTE ligne est la modification : `title` vient de lui, pas du patch.
    assert_eq!(up["title"], "par effet");
    assert_eq!(titles(&svc).await, ["par effet"]);
    // Il est retiré des effets : seul le `log` reste à appliquer.
    assert_eq!(
        sink.applied.lock().unwrap().clone(),
        vec![vec![log("après")]]
    );
}

#[tokio::test]
async fn a_put_effect_aimed_at_another_row_stays_an_effect_on_update() {
    let sink = Arc::new(Sink::default());
    let (svc, rows, seed) = seeded(
        Scripted {
            update: outcome(
                None,
                vec![put(
                    "task",
                    json!({"id": Uuid::new_v4().to_string(), "title": "ailleurs"}),
                )],
            ),
            ..Default::default()
        },
        sink.clone(),
    );
    seed_row(&rows, seed, "a").await;
    let up = svc
        .update_from_json(&seed, json!({"title": "patch"}))
        .await
        .unwrap();
    assert_eq!(up["title"], "patch");
    assert_eq!(sink.applied.lock().unwrap()[0].len(), 1);
}

// ───────────────────────── logique : delete ─────────────────────────

#[tokio::test]
async fn delete_consults_the_logic_with_the_current_row_then_applies_its_effects() {
    let (svc, logic, sink) = with_logic(
        Scripted {
            delete: outcome(None, vec![log("adieu")]),
            ..Default::default()
        },
        Sink::default(),
    );
    let id = id_of(
        &svc.create_from_json(json!({"title": "partant"}))
            .await
            .unwrap(),
    );
    EntityCreator::delete(&*svc, &id).await.unwrap();
    assert!(titles(&svc).await.is_empty());
    assert_eq!(logic.seen()[1], format!(r#"delete {id} "partant""#));
    assert_eq!(
        sink.applied.lock().unwrap().clone(),
        vec![vec![log("adieu")]]
    );
}

#[tokio::test]
async fn delete_of_a_missing_row_never_reaches_the_logic() {
    let (svc, logic, _) = with_logic(Scripted::default(), Sink::default());
    let e = EntityCreator::delete(&*svc, &Uuid::new_v4())
        .await
        .unwrap_err();
    assert_eq!(code_of(&e), codes::ENTITY_NOT_FOUND.to_string());
    assert!(logic.seen().is_empty());
}

#[tokio::test]
async fn a_delete_refused_by_the_logic_keeps_the_row() {
    let (svc, rows, seed) = seeded(
        Scripted {
            refuse: true,
            ..Default::default()
        },
        Arc::new(Sink::default()),
    );
    seed_row(&rows, seed, "reste").await;
    assert!(EntityCreator::delete(&svc, &seed).await.is_err());
    assert_eq!(rows.count().await.unwrap(), 1);
}

#[tokio::test]
async fn a_delete_whose_effects_are_refused_keeps_the_row() {
    let (svc, rows, seed) = seeded(
        Scripted {
            delete: outcome(None, vec![log("x")]),
            ..Default::default()
        },
        Arc::new(Sink {
            fail_check: true,
            ..Default::default()
        }),
    );
    seed_row(&rows, seed, "reste").await;
    assert!(EntityCreator::delete(&svc, &seed).await.is_err());
    assert_eq!(rows.count().await.unwrap(), 1);
}

// ───────────────────────── logique : get, list, operations ─────────────────────────

#[tokio::test]
async fn get_serves_the_logic_output_instead_of_the_row_and_applies_its_effects() {
    let (svc, logic, sink) = with_logic(
        Scripted {
            get: outcome(Some(json!({"vue": "calculée"})), vec![log("lu")]),
            ..Default::default()
        },
        Sink::default(),
    );
    let id = id_of(&svc.create_from_json(json!({"title": "a"})).await.unwrap());
    assert_eq!(
        svc.fetch_as_json(&id).await.unwrap(),
        json!({"vue": "calculée"})
    );
    assert_eq!(logic.seen()[1], r#"get "a""#);
    assert_eq!(sink.applied.lock().unwrap().clone(), vec![vec![log("lu")]]);

    // Sans sortie, c'est la ligne stockée.
    let (svc, _, _) = with_logic(Scripted::default(), Sink::default());
    let row = svc.create_from_json(json!({"title": "b"})).await.unwrap();
    assert_eq!(svc.fetch_as_json(&id_of(&row)).await.unwrap(), row);
}

#[tokio::test]
async fn a_get_or_list_refused_by_the_logic_is_an_error() {
    let (svc, rows, seed) = seeded(
        Scripted {
            refuse: true,
            ..Default::default()
        },
        Arc::new(Sink::default()),
    );
    seed_row(&rows, seed, "a").await;
    assert!(svc.fetch_as_json(&seed).await.is_err());
    assert!(svc.list_as_json(None, None).await.is_err());
}

#[tokio::test]
async fn list_accepts_a_bare_array_an_items_object_or_nothing() {
    for (output, expected) in [
        (None, vec!["a", "b"]),
        (Some(json!([{"title": "x"}])), vec!["x"]),
        (
            Some(json!({"items": [{"title": "y"}, {"title": "z"}], "total": 2})),
            vec!["y", "z"],
        ),
    ] {
        let (svc, _, _) = with_logic(
            Scripted {
                list: outcome(output, vec![]),
                ..Default::default()
            },
            Sink::default(),
        );
        svc.create_from_json(json!({"title": "a"})).await.unwrap();
        svc.create_from_json(json!({"title": "b"})).await.unwrap();
        assert_eq!(titles(&svc).await, expected);
    }
}

#[tokio::test]
async fn list_refuses_an_output_that_is_not_a_list() {
    for bad in [
        json!({"sans": "items"}),
        json!({"items": "pas un tableau"}),
        json!("texte"),
        json!(42),
    ] {
        let (svc, _, _) = with_logic(
            Scripted {
                list: outcome(Some(bad.clone()), vec![]),
                ..Default::default()
            },
            Sink::default(),
        );
        let e = svc.list_as_json(None, None).await.unwrap_err();
        assert_eq!(code_of(&e), codes::INTERNAL.to_string(), "{bad}");
    }
}

#[tokio::test]
async fn list_applies_the_effects_of_the_logic() {
    let (svc, logic, sink) = with_logic(
        Scripted {
            list: outcome(None, vec![log("listé")]),
            ..Default::default()
        },
        Sink::default(),
    );
    svc.create_from_json(json!({"title": "a"})).await.unwrap();
    svc.list_as_json(Some(1), Some(0)).await.unwrap();
    assert_eq!(logic.seen()[1], "list 1");
    assert_eq!(
        sink.applied.lock().unwrap().clone(),
        vec![vec![log("listé")]]
    );
}

#[tokio::test]
async fn a_named_operation_runs_the_logic_on_all_rows_and_applies_its_effects() {
    let mut ops = HashMap::new();
    ops.insert(
        "archiver".to_string(),
        LogicOutcome {
            output: Some(json!({"archivé": 2})),
            effects: vec![log("archivage")],
        },
    );
    ops.insert(
        "silencieuse".to_string(),
        LogicOutcome {
            output: None,
            effects: vec![],
        },
    );
    let (svc, logic, sink) = with_logic(
        Scripted {
            ops,
            ..Default::default()
        },
        Sink::default(),
    );
    svc.create_from_json(json!({"title": "a"})).await.unwrap();
    svc.create_from_json(json!({"title": "b"})).await.unwrap();

    let out = svc
        .call_operation("archiver", json!({"k": 1}))
        .await
        .unwrap();
    assert_eq!(out, json!({"archivé": 2}));
    assert!(
        logic
            .seen()
            .contains(&r#"call archiver {"k":1} 2"#.to_string())
    );
    assert_eq!(
        sink.applied.lock().unwrap().clone(),
        vec![vec![log("archivage")]]
    );
    // Une opération sans sortie rend `null`.
    assert_eq!(
        svc.call_operation("silencieuse", json!(null))
            .await
            .unwrap(),
        Value::Null
    );
    // Une opération inconnue de la logique est un 404 typé.
    let e = svc.call_operation("inconnue", json!({})).await.unwrap_err();
    assert_eq!(code_of(&e), codes::TRANSPORT_NOT_FOUND.to_string());
    // Un refus de la logique remonte.
    let (svc, _, _) = with_logic(
        Scripted {
            refuse: true,
            ..Default::default()
        },
        Sink::default(),
    );
    assert!(svc.call_operation("archiver", json!({})).await.is_err());
}

// ───────────────────────── routes REST ─────────────────────────

async fn send(
    app: &axum::Router,
    method: Method,
    uri: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let mut req = Request::builder().method(method).uri(uri);
    let body = match body {
        Some(b) => {
            req = req.header("content-type", "application/json");
            Body::from(b.to_string())
        }
        None => Body::empty(),
    };
    let resp = app.clone().oneshot(req.body(body).unwrap()).await.unwrap();
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20)
        .await
        .unwrap();
    let json = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    (status, json)
}

fn drain(rx: &mut tokio::sync::broadcast::Receiver<EventEnvelope>) -> Vec<String> {
    let mut out = vec![];
    while let Ok(env) = rx.try_recv() {
        if let FrameworkEvent::Entity(e) = env.event {
            out.push(match e {
                EntityEvent::Created { entity_type, .. } => format!("created {entity_type}"),
                EntityEvent::Updated { entity_type, .. } => format!("updated {entity_type}"),
                EntityEvent::Deleted { entity_type, .. } => format!("deleted {entity_type}"),
            });
        }
    }
    out
}

#[tokio::test]
async fn rest_crud_serves_the_entity_and_publishes_one_event_per_write() {
    let bus = Arc::new(EventBus::new(64));
    let mut rx = bus.subscribe();
    let svc = Arc::new(service(Some(bus)));
    let app = svc.routes();

    let (st, created) = send(&app, Method::POST, "/tasks", Some(json!({"title": "a"}))).await;
    assert_eq!(st, StatusCode::CREATED, "{created}");
    let id = created["id"].as_str().unwrap().to_string();
    assert_eq!(drain(&mut rx), ["created task"]);

    let (st, got) = send(&app, Method::GET, &format!("/tasks/{id}"), None).await;
    assert_eq!((st, got["title"].as_str()), (StatusCode::OK, Some("a")));
    assert!(drain(&mut rx).is_empty(), "une lecture ne publie rien");

    for method in [Method::PUT, Method::PATCH] {
        let (st, up) = send(
            &app,
            method,
            &format!("/tasks/{id}"),
            Some(json!({"done": true})),
        )
        .await;
        assert_eq!((st, &up["done"]), (StatusCode::OK, &json!(true)), "{up}");
        assert_eq!(drain(&mut rx), ["updated task"]);
    }

    let (st, _) = send(&app, Method::DELETE, &format!("/tasks/{id}"), None).await;
    assert_eq!(st, StatusCode::NO_CONTENT);
    assert_eq!(drain(&mut rx), ["deleted task"]);

    let (st, err) = send(&app, Method::GET, &format!("/tasks/{id}"), None).await;
    assert_eq!(st, StatusCode::NOT_FOUND, "{err}");
}

#[tokio::test]
async fn rest_errors_map_to_http_statuses_without_publishing() {
    let bus = Arc::new(EventBus::new(64));
    let mut rx = bus.subscribe();
    let app = Arc::new(service(Some(bus))).routes();
    let missing = Uuid::new_v4();

    let (st, _) = send(&app, Method::POST, "/tasks", Some(json!({}))).await;
    assert_eq!(st, StatusCode::UNPROCESSABLE_ENTITY);
    let (st, _) = send(
        &app,
        Method::PUT,
        &format!("/tasks/{missing}"),
        Some(json!({"done": true})),
    )
    .await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    let (st, _) = send(&app, Method::DELETE, &format!("/tasks/{missing}"), None).await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    // Un id qui n'est pas un UUID n'atteint pas le service.
    let (st, _) = send(&app, Method::GET, "/tasks/pas-un-uuid", None).await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    assert!(drain(&mut rx).is_empty());
}

#[tokio::test]
async fn rest_list_paginates_filters_and_sorts() {
    let app = Arc::new(service(None)).routes();
    for t in ["b", "a", "c", "a"] {
        let (st, _) = send(&app, Method::POST, "/tasks", Some(json!({"title": t}))).await;
        assert_eq!(st, StatusCode::CREATED);
    }
    let (st, all) = send(&app, Method::GET, "/tasks", None).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(all["data"].as_array().unwrap().len(), 4);
    assert_eq!(all["pagination"]["page"], 1);
    assert_eq!(all["pagination"]["limit"], 20);
    assert_eq!(all["pagination"]["total"], 4);
    assert_eq!(all["pagination"]["total_pages"], 1);

    let (_, page) = send(&app, Method::GET, "/tasks?limit=3&page=2&sort=title", None).await;
    assert_eq!(page["pagination"]["page"], 2);
    assert_eq!(page["pagination"]["limit"], 3);
    assert_eq!(page["pagination"]["total_pages"], 2);
    let titles: Vec<_> = page["data"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["title"].as_str().unwrap())
        .collect();
    assert_eq!(titles, ["c"], "tri par titre : a a b | c");

    let (_, desc) = send(&app, Method::GET, "/tasks?sort=-title&limit=1", None).await;
    assert_eq!(desc["data"][0]["title"], "c");

    let filter = urlenc(r#"{"title":"a"}"#);
    let (st, only_a) = send(&app, Method::GET, &format!("/tasks?filter={filter}"), None).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(only_a["pagination"]["total"], 2);
}

fn urlenc(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

#[tokio::test]
async fn rest_list_refuses_a_malformed_or_forbidden_query_with_400() {
    let app = Arc::new(service(None)).routes();
    for q in [
        format!("filter={}", urlenc("{pas du json")),
        format!("filter={}", urlenc(r#"{"champ_inconnu":1}"#)),
        format!("filter={}", urlenc(r#"{"done":true}"#)), // `done` n'est pas filtrable
        "sort=done".to_string(),                          // `done` n'est pas triable
        "sort=inconnu".to_string(),
    ] {
        let (st, body) = send(&app, Method::GET, &format!("/tasks?{q}"), None).await;
        assert_eq!(st, StatusCode::BAD_REQUEST, "{q} : {body}");
    }
}

#[tokio::test]
async fn the_operations_route_exists_only_with_logic() {
    // Sans logique : la route n'est pas montée.
    let app = Arc::new(service(None)).routes();
    let (st, _) = send(&app, Method::POST, "/tasks/ops/archiver", Some(json!({}))).await;
    assert_ne!(st, StatusCode::OK);

    let mut ops = HashMap::new();
    ops.insert(
        "echo".to_string(),
        LogicOutcome {
            output: Some(json!({"ok": true})),
            effects: vec![],
        },
    );
    let (svc, logic, _) = with_logic(
        Scripted {
            ops,
            ..Default::default()
        },
        Sink::default(),
    );
    let app = svc.routes();
    let (st, out) = send(&app, Method::POST, "/tasks/ops/echo", Some(json!({"k": 7}))).await;
    assert_eq!((st, out), (StatusCode::OK, json!({"ok": true})));
    assert!(logic.seen().contains(&r#"call echo {"k":7} 0"#.to_string()));

    // Sans corps : l'entrée est `null`.
    let (st, _) = send(&app, Method::POST, "/tasks/ops/echo", None).await;
    assert_eq!(st, StatusCode::OK);
    assert!(logic.seen().contains(&"call echo null 0".to_string()));

    // Une opération inconnue : 404.
    let (st, _) = send(&app, Method::POST, "/tasks/ops/inconnue", Some(json!({}))).await;
    assert_eq!(st, StatusCode::NOT_FOUND);
}
