# Audit — publication d'événements au-dessus des backends

Lecture seule, état du dépôt du 2026-11-04 **avec l'arbre de travail non commité** (11 fichiers modifiés, dont `src/server/exposure/guard.rs`, `graphql/executor/mutation_executor.rs`, `grpc/entity_service.rs`). Ce qui est dit « présent » l'est dans l'arbre de travail, pas dans `HEAD`.

## 0. Constat qui change la question

Les protocoles **n'appellent pas `DataService<T>`**. Ils appellent :
- `EntityCreator` / `EntityFetcher` (JSON, `src/core/module.rs:31`, `:126`) pour les entités ;
- `LinkService` directement (`ServerHost.link_service: Arc<dyn LinkService>`, `src/server/host.rs:51`) pour les liens.

`DataService<T>` (typé, `src/core/service.rs:13`) est donc **sous** le seam des protocoles : c'est à l'utilisateur (ou à un store d'exemple, p. ex. `examples/microservice/entities/order/store.rs:89`) de l'adapter en `EntityCreator`. Dans `src/`, seul `ScyllaDataService` le fait (`src/storage/scylladb.rs:544`).

Un décorateur sur `DataService<T>` serait donc invisible pour le chemin JSON de certains stores, et ferait doublon avec le seam existant (§3).

## 1–2. Backends, points d'écriture, trait commun

Trait commun : **oui** — `DataService<T: Data>` (`create/update/delete`, `src/core/service.rs:15,24,27`) et `LinkService` (`create/update/delete/delete_by_entity` + `delete_by_tenant` par défaut, `src/core/service.rs:40,73,76,81,88`). 9 backends, tous les deux traits.

| Backend | DataService (create / update / delete) | LinkService (create / update / delete / delete_by_entity) | Décorable ? |
|---|---|---|---|
| in_memory | `in_memory.rs:58 / :87 / :101` | `:213 / :286 / :299 / :310` | oui |
| postgres | `postgres.rs:261 / :320 / :350` | `:761 / :885 / :917 / :930` | oui |
| mysql | `mysql.rs:223 / :293 / :328` | `:486 / :622 / :653 / :663` | oui |
| mongodb | `mongodb.rs:156 / :212 / :240` | `:387 / :500 / :527 / :537` | oui |
| neo4j | `neo4j.rs:206 / :288 / :320` | `:428 / :568 / :596 / :605` | oui |
| scylladb | `scylladb.rs:174 / :295 / :347` (+ `EntityCreator` `:544/:554/:572`) | `:659 / :774 / :810 / :821` | oui |
| dynamodb | `dynamodb.rs:245 / :292 / :305` | `:532 / :659 / :677 / :690` | oui (voir divergences) |
| lmdb | `lmdb.rs:125 / :171 / :190` | `:293 / :406 / :442 / :468` | oui |
| obrain | `obrain.rs:645 / :770 / :814` | `:965 / :1102 / :1153 / :1178` | oui |

Signatures : `async fn create(&self, entity: T) -> Result<T>`, `update(&self, id: &Uuid, entity: T) -> Result<T>`, `delete(&self, id: &Uuid) -> Result<()>` ; liens : `create(&self, link: LinkEntity) -> Result<LinkEntity>`, `update(&self, id, link)`, `delete(&self, id)`, `delete_by_entity(&self, entity_id)`.

Autres implémenteurs de `LinkService` à ne pas oublier : `TenantScopedLinkService` (`src/storage/tenant_scoped.rs:77`). Précédent de décorateur sur `DataService` : `SoftDeleting<S>` (`src/core/soft_delete.rs:166`).

### Divergences de comportement (elles n'empêchent pas le décorateur, elles faussent l'événement)

- **Delete d'un id inexistant** : in_memory (`:101`), postgres (`:350`), mysql (`:328`), mongodb (`:240`), neo4j (`:320`), scylladb (`:347`), dynamodb (`:305`), lmdb (`:190`) rendent `Ok(())` sans vérifier qu'une ligne a existé ; obrain aussi (`:814`, `None => return Ok(())`). Un décorateur publierait `Deleted` pour un id fantôme sur les 9. Le banc l'accepte explicitement (`tests/storage_harness/data_service_tests.rs:295-312`, « Both … acceptable »). `JsonEntityService` fait le contraire (erreur `not_found`, `json_entity.rs:~185`).
- **Update d'un id inexistant** : erreur attendue par le banc (`data_service_tests.rs:184`). Relu : in_memory, mongodb (`matched_count == 0`), scylladb, lmdb, obrain. **Non relu** : postgres, mysql, neo4j. **DynamoDB : `put_item` = upsert, et `_id` ignoré** (`dynamodb.rs:292-303`) → un « update » peut créer ; l'événement dirait `Updated` pour une création.
- **`created_at/updated_at`, `id`** : produits côté backend ou côté entité selon le backend (`scylladb.rs` `prepare_create_json`). L'événement doit prendre la valeur **retournée** par `create/update`, pas l'entrée.
- **Suppression douce** : `DataService::delete` est un DELETE dur partout ; la suppression douce (opt-in) vit dans `SoftDeleteCreator` (`soft_delete.rs:631`), donc **au-dessus** du store — publier `Deleted` en dessous de lui émettrait `Updated` ou `Deleted` selon l'ordre des couches.

## 3. Point unique appelé par les protocoles

- **Entités** : `SurfaceGuard::creator()` (`src/server/exposure/guard.rs:380`). Il enveloppe le creator du catalogue dans `PublishingCreator` (`guard.rs:43-95`) quand un bus est configuré. REST descriptor (`descriptor_entity.rs:524,557,583`), GraphQL (`mutation_executor.rs`), gRPC (`entity_service.rs`) y passent. **Ce décorateur existe déjà** — il est **non commité**, et les publications explicites de gRPC, GraphQL (`mutation_executor`, `link_mutations` createAndLink) viennent d'être retirées dans la même modification.
- **Liens** : aucun point unique. Chaque surface publie à la main : REST `src/links/handlers.rs:545,645,754,1236` ; gRPC `grpc/link_service.rs:156,340` ; GraphQL `graphql/executor/link_mutations.rs` (publications aux alentours de `:50,:97,:159,:186,:271,:361`, certaines en cours de retrait). Pas de décorateur `LinkService`.

## 4. Écritures qui contournent le point

1. **REST `links/handlers.rs:596` et `:1201`** (createAndLink) : lisent `state.entity_creators` en direct, **sans `SurfaceGuard`** → pas de `PublishingCreator` ; ils publient à la main (`:636`, `:1227`). Cohérent aujourd'hui, mais deux mécanismes.
2. **`JsonEntityService`** (`module_runtime/json_entity.rs:80,~122,~147,~185`) publie lui-même **et** est enregistré dans le catalogue (`loader.rs:183`) → sur les surfaces passant par le guard (gRPC, GraphQL, REST descriptor) l'événement est publié **deux fois**. À confirmer par un test (non exécuté ici) ; la lecture du code suffit à le déduire.
3. **Cascade** : `LinkService::delete_by_entity` n'a **aucun appelant hors `src/storage`** (grep). Supprimer une entité ne supprime pas ses liens et n'émet aucun `LinkEvent::Deleted`.
4. **GDPR** : `delete_by_tenant` (`gdpr_routes.rs:83,92`) supprime en masse sans événement par entité/lien ; seul `FrameworkEvent::GdprErasure` existe (`core/events.rs:159`).
5. **`SurfaceGuard::fetcher`** n'écrit pas ; `ScyllaDataService` implémente `EntityCreator` directement (`scylladb.rs:544`) : passe par le guard comme les autres.
6. **Écritures hors protocoles** (jobs, `ServerHost` utilisé en code, tests, modules WAMI) : si elles appellent un `DataService` directement, elles n'émettent rien. Non inventorié exhaustivement.
7. **Moteur d'événements** (`src/events/…`, `events/bridge/…`) : ses sinks/notifications écrivent via d'autres stores (`NotificationStore`) ; hors périmètre, non audité.

## 5. Signature minimale

Ne pas décorer `DataService<T>` pour les entités (doublon avec le guard). Deux décorateurs, un par trait réellement appelé :

```rust
// Entités : existe déjà (guard.rs:43). Le déplacer dans src/core/ et l'appliquer à la
// construction du catalogue (builder), pas à l'appel, pour que REST links/handlers.rs
// passe aussi par lui.
pub struct EventedCreator { inner: Arc<dyn EntityCreator>, bus: Arc<EventBus>, entity: String }

// Liens : à créer. Seul trait que les protocoles appellent directement.
pub struct EventedLinkService<S: LinkService> { inner: S, bus: Arc<EventBus> }

#[async_trait]
impl<S: LinkService> LinkService for EventedLinkService<S> {
    async fn create(&self, link: LinkEntity) -> Result<LinkEntity> {
        let c = self.inner.create(link).await?;
        self.bus.publish(FrameworkEvent::Link(LinkEvent::Created {
            link_type: c.link_type.clone(), link_id: c.id,
            source_id: c.source_id, target_id: c.target_id, metadata: c.metadata.clone() }));
        Ok(c)
    }
    async fn delete(&self, id: &Uuid) -> Result<()> {
        let before = self.inner.get(id).await?;           // lire AVANT : delete rend () et Ok si absent
        self.inner.delete(id).await?;
        if let Some(l) = before { self.bus.publish(/* LinkEvent::Deleted { … } */); }
        Ok(())
    }
    async fn delete_by_entity(&self, e: &Uuid) -> Result<()> { /* find_by_source+target, puis delete, un événement par lien */ }
    // get/list/find_*: délégation pure ; update: pas de LinkEvent::Updated n'existe (core/events.rs:70)
}

// Optionnel, typé, pour les appelants qui n'utilisent pas EntityCreator :
pub struct EventedStore<T: Data + Serialize, S: DataService<T>> { inner: S, bus: Arc<EventBus>, entity: String, _t: PhantomData<T> }
```

Contrainte : `EventedStore` et `EventedCreator` ne doivent **jamais** être empilés (voir §6). Fournir un seul constructeur dans le builder qui choisit la couche.

## 6. Risques

- **Double publication** : confirmée par lecture pour `JsonEntityService` + guard (§4.2) ; potentielle si `EventedStore` est posé sous un `EntityCreator` déjà décoré ; potentielle sur les liens tant que les `publish` manuels de REST/gRPC/GraphQL ne sont pas tous retirés en même temps que `EventedLinkService` arrive.
- **Événement fantôme** : `Deleted` pour un id absent (9 backends, §1) ; `Updated` pour une création sur DynamoDB. Corriger en lisant avant (`get`) ou en normalisant le contrat des backends (changement de comportement, à décider).
- **Ordre** : l'événement part après le commit mais rien ne garantit l'ordre entre deux écritures concurrentes sur la même entité (le `publish` n'est pas sérialisé avec l'écriture). `PublishingCreator` publie `Updated` avec la donnée retournée, qui peut être déjà périmée à la livraison.
- **Transactions partielles** : `createAndLink` écrit entité puis lien, sans transaction ; si le lien échoue, l'événement `Created` de l'entité est déjà parti (`links/handlers.rs:596→636→645`). Pas d'outbox : un crash entre commit et `publish` perd l'événement (at-most-once).
- **Tâches de fond** : LMDB écrit dans `spawn_blocking` (`lmdb.rs:190`) — le décorateur doit `await` le résultat avant de publier (ce que fait `PublishingCreator`). Obrain a un `check_write` / bail d'écriture (`obrain.rs:815`, `writer_lease.rs`) : une écriture refusée ne doit rien publier (c'est le cas si on publie après `?`).
- **Multi-nœuds** : le bus est en processus ; deux instances sur une même base ne se voient pas. Une écriture directe en base (hors framework) n'émet rien.
- **Bus plein / aucun abonné** : `publish` rend un nombre de récepteurs, ignoré par `PublishingCreator` ; pas de back-pressure.

## 7. Test de contrat unique

Il existe déjà un banc partagé : `data_service_tests!`, `link_service_tests!`, `rest_integration_tests!` (`tests/storage_harness/`), invoqué par fichier de backend. **`dynamodb` n'a aucun fichier `tests/dynamodb_tests.rs`** (relu : `ls tests`) : il n'est pas couvert par le banc partagé — 8 backends sur 9, pas 9. Ajouter une macro sœur :

```rust
// tests/storage_harness/events_tests.rs
macro_rules! event_contract_tests {
    ($data:expr, $links:expr) => { mod event_contract {
        #[tokio::test] async fn one_event_per_successful_write() {
            let bus = Arc::new(EventBus::new(64)); let mut rx = bus.subscribe();
            let creator = EventedCreator::new(as_creator($data), bus.clone(), "test_data_entity");
            let links   = EventedLinkService::new($links, bus.clone());
            let v = creator.create_from_json(json!({"name":"a", …})).await.unwrap();
            let id = Uuid::parse_str(v["id"].as_str().unwrap()).unwrap();
            creator.update_from_json(&id, json!({"name":"b"})).await.unwrap();
            let l = links.create(link(id, other)).await.unwrap();
            links.delete(&l.id).await.unwrap();
            creator.delete(&id).await.unwrap();
            // même séquence, mêmes types, mêmes champs, quel que soit le backend
            assert_eq!(drain_kinds(&mut rx), ["entity.created","entity.updated","link.created","link.deleted","entity.deleted"]);
            assert!(rx.try_recv().is_err());                       // pas de doublon
            // échec => silence
            assert!(creator.update_from_json(&Uuid::new_v4(), json!({})).await.is_err());
            assert!(rx.try_recv().is_err());
        }
        // + delete d'un id absent : aucun événement (verrouille la correction du §1)
    }};
}
```

Invocation par backend, à côté de `data_service_tests!` :

| Backend | Environnement | Fichier |
|---|---|---|
| in_memory | mémoire | `tests/in_memory_tests.rs:12` |
| lmdb | embarqué (répertoire temporaire), pas de conteneur | `tests/lmdb_tests.rs:56` |
| obrain | embarqué (feature `obrain`), pas de conteneur | `tests/obrain_tests.rs:26` |
| postgres | conteneur (testcontainers) | `tests/postgres_tests.rs:151` |
| mysql | conteneur | `tests/mysql_tests.rs:139` |
| mongodb | conteneur | `tests/mongodb_tests.rs:119` |
| neo4j | conteneur | `tests/neo4j_tests.rs:133` |
| scylladb | conteneur (opt-in CI, « CQL not ready » en local) | `tests/scylladb_tests.rs:164` |
| dynamodb | conteneur DynamoDB Local — **fichier à créer** | absent |

Les harnais utilisent testcontainers (relu pour mongodb, `tests/mongodb_tests.rs:9-10`), pas docker-compose ; les docker-compose par backend sont dans `this-examples`, non relus ici.

## Limites de cet audit

Rien n'a été compilé ni exécuté. Non relu : bodies de `update` postgres/mysql/neo4j (comportement sur id inexistant), chemin exact des publications de `link_mutations.rs` après modification en cours, `src/events/*`. L'arbre de travail est modifié par quelqu'un d'autre pendant la lecture : les numéros de ligne de `guard.rs`, `mutation_executor.rs`, `entity_service.rs` peuvent bouger.
