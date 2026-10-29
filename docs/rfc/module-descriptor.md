# RFC — ModuleDescriptor : un socle commun front/back, déclaré une fois

- Statut : **v2, fondation implémentée** (29/10/2026). v1 (`md/design`, 501d901) revue par trois lentilles
  (archi, cohérence, philosophie) : 66 objections, toutes traitées au §9 (corrigées, réfutées ou reportées
  avec un propriétaire nommé).
- Auteurs : agent architecte (v1), flux fondation (v2, branche `md/foundation`).
- Portée : dépôt `this` (crate `this-rs` + crate `this-descriptor`), dépôt `this-cli`.
- **Source de vérité des contrats : le code** de `crates/this-descriptor/src/*.rs` et les fichiers this-rs listés
  au §7.0. Ce document en donne la forme et les décisions ; en cas d'écart, le code fait foi et l'RFC est
  corrigée.
- Sources lues (v1, `main` à `c9386e0`) : `src/core/entity.rs`, `src/entities/macros.rs`,
  `src/server/entity_registry.rs`, `src/core/module.rs`, `src/server/builder.rs`, `src/server/host.rs`,
  `src/core/validation/{config,extractor,validators,filters}.rs`, `src/core/query.rs`, `src/core/auth/mod.rs`,
  `src/config/mod.rs`, `src/core/link.rs`, `src/server/exposure/{mod.rs,grpc/entity_service.rs,grpc/proto_generator.rs,graphql/schema_generator.rs}`,
  `this-cli/src/commands/add_entity.rs`, `this-cli/src/templates/entity/*.tera`. Ajoutées en v2 :
  `src/server/middleware/authorization.rs` (`evaluate`, `is_known_policy`, `PERM_PREFIX`), `src/core/pluralize.rs`,
  `src/links/handlers.rs` (`apply_link_filters`), `.github/workflows/{ci,release}.yml`.
- Notes d'audit (Project Orchestrator) : b63fc8ee (inventaire this), 6d11cc02 et b0a76933 (this-cli), 874f4449 (Leptos).
- Diagrammes : `docs/atlas/module-descriptor/*.mmd`.

---

## 1. Problème

Une entité de `this-rs` est déclarée **une fois en Rust** (struct engendrée par `impl_data_entity!`), puis
**redéclarée ou ré-inférée** par chaque protocole et chaque outil. Mesures relevées dans le code :

| Préoccupation | Où elle vit aujourd'hui | Défaut mesuré |
|---|---|---|
| Types | struct Rust ; `graphql/schema_generator.rs` et `grpc/proto_generator.rs` **infèrent** les champs d'un JSON (`EntityFetcher::get_sample_entity`) | `EntityDescriptor` (`server/entity_registry.rs`) n'a que `entity_type/plural/build_routes` ; numéros de champ proto = ordre des clés JSON ; un sample vide donne un type vide |
| Nom / pluriel | YAML `entities` + `Entity::resource_name` + `EntityDescriptor::plural` + `Module::entity_types` + `match` dans `get_entity_fetcher/creator` + pluralizer **divergent** de this-cli | synchronisation manuelle, jusqu'à 6 copies, deux algorithmes (knife → knives / knifes) |
| Erreurs | `anyhow` partout ; `StatusCode::` ad hoc ; gRPC `map_err(\|e\| Status::internal(..))` ; `async_graphql::Error::new` inline | aucun code stable, aucune table ; une erreur métier devient `INTERNAL` en gRPC ; trois formes de corps HTTP |
| Validation | `EntityValidationConfig` + `Validated<T>` (REST, opt-in) | **contournée** par GraphQL et gRPC (`create_from_json` direct) et par le REST engendré par la CLI ; `validate_and_filter` n'itère que sur les clés **présentes** ; crate `validator` inutilisé |
| Filtres de requête | `QueryParams.filter: Option<String>` | JSON malformé **ignoré en silence** ; `sort` jamais lu ; seul consommateur : les listes de liens (`apply_link_filters`, chemins pointés, égalité seule, après pagination) |
| Auth | `AuthPolicy` + `EntityAuthConfig` (YAML, chaînes) ; application par route REST | GraphQL/gRPC/WS refusés tant que les politiques sont armées (`refuse_unenforced_surface`) ou servis **sans** politique ; gRPC n'extrait pas d'identité ; `parse_policy` se replie sur `authenticated` (fail-open) |
| i18n | néant | messages en dur, FR et EN mélangés |
| Modules | trait `Module` | aucune dépendance ; `LinksConfig::merge`, `EntityRegistry::register` et les maps de fetchers : **le dernier gagne en silence** ; hôte figé à la construction |
| CLI | copies manuelles de `LinksConfig` & co. dans `this-cli` | copies en retard → `this add link` **efface** `events`, `sinks`, `auth`, `required_fields` ; grammaire de macro connue 3 fois |
| Front (Leptos) | `FromServerFnError` | ni registre d'erreurs, ni validation, ni filtres typés, ni i18n, ni auth déclarative, ni modules |

Racine commune : **il n'existe aucune description typée et sérialisable d'un module**.

## 2. Philosophie

1. **Une déclaration, un propriétaire par facette.** Chaque facette a UN propriétaire ; une redéclaration est
   une **erreur** (`this.descriptor.declared_twice`), pas une égalité surveillée. Module compilé : champs,
   types, pluriel et règles de champ appartiennent à la macro (`Described`) ; identité, `requires`, liens,
   erreurs, messages, permissions, capacités appartiennent à `descriptor/module.yaml`
   (`ModuleDescriptor::compose`). Module stocké : tout appartient au descripteur stocké. `links.yaml`
   (`entities`, `links`), `Module::entity_types` et les tables d'auth YAML deviennent des **dérivations**.
2. **Dérivations par protocole.** REST, GraphQL, gRPC, WebSocket, le client Leptos et la CLI dérivent leur
   forme du descripteur par des fonctions pures. Une dérivation ne réinvente jamais une règle.
3. **Fail-closed.** Champ inconnu dans un filtre ou un payload, tri non permis, jeton de politique inconnu,
   résolveur inconnu, `Custom` non enregistré, dépendance manquante, doublon (module, entité, pluriel, code,
   fetcher) : refus. Défauts de politique : `authenticated` pour les opérations standard d'un module compilé
   (= `default_auth_policy`, rétrocompatible), **`deny`** pour les opérations métier, **aucun défaut** pour un
   module stocké (politique explicite exigée).
4. **Additif et rétrocompatible.** `Module::descriptor()` vaut `None` par défaut : un module existant compile et
   se comporte à l'identique (mesuré : `cargo test -p this-rs --lib`, §7.0). Les corps d'erreur HTTP gardent
   `error` / `errors` et **ajoutent** `code`, `kind`, `message_key`, `fields` et les clés RFC 9457.
5. **Sérialisable et wasm32.** Descripteur, moteurs purs, table des erreurs et catalogue du framework vivent
   dans `this-descriptor`, sans tokio/axum/tonic par défaut : le front Leptos valide, filtre et traduit avec le
   **même code**.
6. **Extensible via `Custom` nommé**, résolu par `CustomRegistry` ; un `Custom` inconnu fait échouer `check()`.

## 3. Solution

### 3.1 Crate de workspace `crates/this-descriptor`

Décision inchangée (crate séparé, même dépôt). Changements v2 :

- `[workspace] members = [".", "crates/this-descriptor"]` dans le `Cargo.toml` de `this` ;
  `this-descriptor = { version = "0.1", path = "crates/this-descriptor", features = ["yaml","pattern","http"] }`.
  **Toujours `version` + `path`** (publiable). `this-cli` : même forme (`version` + `path`), et sa CI fait un
  checkout de `this-rs/this` au bon chemin (flux cli).
- Dépendances : `serde`, `serde_json`, `thiserror`, `semver`, **`chrono`** (`default-features=false`, `alloc` :
  même sémantique `NaiveDate::parse_from_str` que l'ancien `date_format`), **`uuid`** (`default-features=false`).
- Features, **`default = []`** : `yaml` (serde_yaml), `pattern` (regex ; sans elle, `check()` **refuse** un
  `Pattern`), `http` (`http::StatusCode`), `grpc` (`From<ContractError> for tonic::Status`), `graphql`
  (`From<ContractError> for async_graphql::Error`). Les adaptateurs de protocole vivent ici à cause de la règle
  d'orphelinat (le crate possède `ContractError`). this-rs active `grpc`/`graphql` via ses propres features.
- Publication : `release.yml` publie `this-descriptor` **avant** `this-rs` (étape idempotente), puis
  `cargo package/publish -p this-rs`. `ci.yml` teste `-p this-descriptor --all-features` et vérifie wasm32.
- Le pluralizer **déménage** dans `this_descriptor::naming::Pluralizer` ; `this::core::pluralize` le ré-exporte.

### 3.2 Types (crate `this-descriptor`) — forme résumée, le code fait foi

```rust
// module.rs
pub struct ModuleDescriptor { format: u32, name, version, requires: Vec<ModuleReq>, entities: Vec<EntityDesc>,
    links: Vec<LinkDesc>, errors: Vec<ErrorDesc>, i18n: I18nDecl /* clés, pas de textes */, custom: Vec<Custom> }
impl ModuleDescriptor { fn check(&self, &CustomRegistry, CheckProfile) -> Result<(), Vec<DescriptorIssue>>;
    fn normalize(&mut self); fn compose(self, described: &[&EntityDesc]) -> Result<Self, ContractError>;
    fn from_json / from_yaml (feature) -> Result<Self, ContractError>; }
pub struct ModuleReq { name, version /* VersionReq */, optional: bool }
pub struct DescriptorSet;   // immuable une fois publié ; insert refuse module/entité/pluriel/code en double ;
                            // entity(), compiled() -> &Arc<CompiledEntity>, entity_by_plural(), error(), generation()
pub struct CompiledEntity { module, desc: EntityDesc, field_index, patterns /* Regex compilées, feature */ }

// entity.rs
pub struct EntityDesc { name, plural /* sérialisé ; vide en entrée = pluralize(name) */, kind: EntityKind,
    fields: Vec<FieldDesc>, indexed, reserved_tags, permissions: PermDesc, i18n: EntityI18n, custom: Vec<Custom> }
pub fn base_fields(EntityKind) -> &'static [FieldDesc];  // §3.3

// field.rs
pub struct FieldDesc { name, ty: FieldType, optional: bool, default: Option<Value>, proto_tag: Option<u32>,
    validators: Vec<ValidatorRule>, transforms: Vec<TransformRule> /* ex-`filters` */, filterable: Vec<FilterOp>,
    sortable, i18n: FieldI18n, read_only, immutable /* écrit à la création seulement */, deprecated }
FieldDesc::required_on(Applies) // présence DÉRIVÉE de `optional`/`default` ; Required/Optional = surcharges par opération
pub enum FieldType { String, Text, Bool, I32, I64, U32, U64, F32, F64, Decimal{scale}, Uuid, DateTime, Date, Json,
    Enum{name,variants}, List(Box<FieldType>), Ref{entity}, Custom(Custom) }  // FieldType::accepts(&Value)
pub enum ValidatorSpec { Required, Optional, Positive, StringLength{min,max}, MinValue(Number), MaxValue(Number),
    InList(Vec<Value>), DateFormat(String), Pattern(String), Email, Url, Custom(Custom) }
pub enum TransformSpec { Trim, Uppercase, Lowercase, RoundDecimals(u32), Custom(Custom) }
pub enum Applies { Always /* défaut */, Create, Update }
pub trait FieldTypeOf; pub struct FieldProbe<T>;  // type Rust -> FieldType pour les macros (repli `json`)

// link.rs — un LinkDesc par couple (source, target)
pub struct LinkDesc { link_type, source, target, forward_route, reverse_route, fields, permissions, description }
LinkDesc::effective_policy(scope: Option<&EntityDesc>, &Operation) // lien, puis entité de la portée, puis défaut

// error.rs
pub struct ErrorCode(String);  pub enum ErrorKind { BadRequest, Validation, Unauthenticated, PermissionDenied,
    NotFound, Conflict, Aborted, PreconditionFailed, RateLimited, NotImplemented, Unavailable, Timeout, Internal }
pub struct ErrorDesc { code, kind, message_key, params, description }
pub struct ContractError { code, kind, message_key, message, params, fields: Vec<FieldViolation>, trace_id }
ContractError::{new, from_desc, framework(code) /* kind lu dans la table du framework */, with_param, with_field,
    http_body, graphql_extensions, to_json};  pub const ERROR_HEADER = "x-this-error";

// perm.rs
pub enum Operation { List, Get, Create, Update, Delete, ListLinks, GetLink, CreateLink, UpdateLink, DeleteLink,
    Custom(String) }  // sérialisé en CHAÎNE ("get_link", "invoice.pay") : clé de map JSON valide
pub enum PolicyExpr { Public, Authenticated, Owner, ServiceOnly, AdminOnly, Deny, HasRole(Vec<String>),
    Perm(String), Resolver(String), And(Vec<_>), Or(Vec<_>), Not(Box<_>) }  // sérialisé en CHAÎNE canonique
pub struct PermDesc { ops: BTreeMap<Operation, PolicyExpr> }  // effective(op) : explicite ou défaut (§2.3)
pub struct PrincipalView<'a> { authenticated, user_id, roles, admin, service, owner: Option<bool> }

// i18n.rs
pub struct MessageKey(String); pub struct Locale(String);
pub struct I18nDecl { default_locale, locales, keys: Vec<MessageDecl{key, params}> }   // DANS le descripteur
pub struct MessageBundle { module, locale, syntax: MessageSyntax{Simple, Fluent(réservé)}, messages } // HORS descripteur

// query.rs
pub enum FilterOp { Eq, Ne, Gt, Gte, Lt, Lte, In, Contains, StartsWith, IsNull }
pub enum FilterExpr { And, Or, Not, Cmp{field /* chemin pointé */, op, value} }
pub struct Query<T = Untyped> { filter, sort: Vec<SortSpec>, page: Page }  // impls manuelles, aucune borne sur T
impl<T: Described> Query<T> { fn check(&self) }   // un seul mécanisme : le typé délègue au schéma
pub struct QuerySchema; QuerySchema::{for_entity, for_links(link, source, target) /* préfixes source./target. */}

// custom.rs
pub struct Custom { name, params };  pub struct CustomRegistry; pub enum CustomScope { Both, ServerOnly }
// transforms Custom : `register_transform(name, idempotent, f)` refuse idempotent=false

// lifecycle.rs — 6 états observables
pub enum ModuleState { Discovered, Resolved, Active, Draining, Unloaded, Failed{code, at} }
pub const DEFAULT_DRAIN_GRACE_MS: u64 = 30_000;

pub trait Described { fn entity_desc() -> &'static EntityDesc; }  // OnceLock, jamais reconstruit
```

**Moteurs purs** (implémentés dans la fondation, enrichis par les flux) :

```rust
engine::validate::apply(&CompiledEntity, WriteOp, Value, &ValidateCtx{registry, side}) -> Result<ValidationOutcome{value, deferred}, ContractError>
engine::query::{parse_filter_json, parse_sort, parse_legacy(&QuerySchema, ..) /* strict */, check, matches, apply_in_memory /* filtre AVANT pagination */}
engine::policy::{evaluate(&PolicyExpr, &PrincipalView, &dyn PolicyEnv) -> Option<bool>, is_allowed}
engine::i18n::{negotiate, render(&[&MessageBundle], ..), localize, substitute}
resolve::{resolve(candidates, active) -> Result<Resolution{order, skipped_optional}, Vec<ContractError>>, dependents}
compat::check_compat(prev, next) -> Vec<DescriptorIssue>      // tags jamais déplacés ni réutilisés
framework::{FRAMEWORK_ERRORS, codes::*, kind_of, errors(), bundle(locale), descriptor()}  // codes this.* + fr/en
derive::{graphql_sdl, proto_file, json_schema} -> Result<_, ContractError>  // stubs NotImplemented (flux schema)
```

#### Table de correspondance (`ErrorKind`, testée exhaustivement : `error_kind_mapping_is_total`)

| `ErrorKind` | HTTP | gRPC | GraphQL `extensions.code` |
|---|---|---|---|
| `BadRequest` | 400 | `INVALID_ARGUMENT` (3) | `BAD_REQUEST` |
| `Validation` | 422 | `INVALID_ARGUMENT` (3) | `BAD_USER_INPUT` |
| `Unauthenticated` | 401 | `UNAUTHENTICATED` (16) | `UNAUTHENTICATED` |
| `PermissionDenied` | 403 | `PERMISSION_DENIED` (7) | `FORBIDDEN` |
| `NotFound` | 404 | `NOT_FOUND` (5) | `NOT_FOUND` |
| `Conflict` | 409 | `ALREADY_EXISTS` (6) | `CONFLICT` |
| `Aborted` (concurrence optimiste) | 409 | `ABORTED` (10) | `ABORTED` |
| `PreconditionFailed` | 412 | `FAILED_PRECONDITION` (9) | `PRECONDITION_FAILED` |
| `RateLimited` | 429 | `RESOURCE_EXHAUSTED` (8) | `RATE_LIMITED` |
| `NotImplemented` | 501 | `UNIMPLEMENTED` (12) | `NOT_IMPLEMENTED` |
| `Unavailable` | 503 | `UNAVAILABLE` (14) | `UNAVAILABLE` |
| `Timeout` | 504 | `DEADLINE_EXCEEDED` (4) | `TIMEOUT` |
| `Internal` | 500 | `INTERNAL` (13) | `INTERNAL_SERVER_ERROR` |

Codes du framework : **une seule liste**, `this_descriptor::framework::FRAMEWORK_ERRORS` (code, kind, paramètres,
fr, en), dont `this.validation.{failed,required,type_mismatch,positive,string_length,min_value,max_value,in_list,
date_format,pattern,email,url,read_only,immutable,unknown_field,not_object,custom}`, `this.query.*`, `this.auth.*`,
`this.entity.*`, `this.module.*` (dont `duplicate_module`, `duplicate_plural`, `duplicate_error_code`,
`plural_mismatch`, `identity_mismatch`, `reference_unknown`, `illegal_transition`), `this.descriptor.*`,
`this.vfs.*` (`precondition_failed` → `Aborted`), `this.transport.*` (réservés à la glue server_fn),
`this.not_implemented`, `this.internal`. `ContractError::framework(code)` lit le kind dans cette table : un
site d'appel ne peut pas contredire la déclaration.

### 3.3 Champs de base, tags proto

`base_fields(kind)` est la **seule** liste des champs implicites, consommée par `apply`, `check()`, `QuerySchema`,
les dérivations et la CLI :

| Champ | Data | Link | Écriture | Tag réservé |
|---|---|---|---|---|
| `id`, `type`, `created_at`, `updated_at`, `deleted_at` | oui | oui | **lecture seule** | 1..5 |
| `status` | oui | oui | écrit | 6 |
| `name` | oui | — | écrit | 7 |
| `link_type`, `source_id`, `target_id` | — | oui | **immuable** (création seulement) | 9, 10, 11 |

`tenant_id` n'est **pas** un champ de base (la macro ne l'injecte pas) : c'est un **nom réservé** (tag 8 par
convention) qui, déclaré, doit être `read_only` (`check()` : `this.descriptor.tenant_id_writable`) ; la macro le
marque `read_only` d'office. Un champ déclaré peut **surcharger** `name` ou `status` (même type, même mode) pour
porter des règles ; jamais un champ en lecture seule ou immuable.

Tags : `proto_tag: Option<u32>`. La macro **n'attribue jamais** de tag. Ils viennent de `descriptor.lock` (CLI)
ou d'une déclaration explicite ; `check()` impose unicité, `>= 16` (sauf `tenant_id`=8) et l'absence de
`reserved_tags` ; `compat::check_compat(prev, next)` refuse un tag déplacé, réutilisé, ou un champ supprimé dont
le tag n'est pas réservé. **Le fil gRPC servi est `google.protobuf.Struct`** : les tags ne protègent aujourd'hui
que le `.proto` de documentation (génération de clients) ; ils deviendront un contrat le jour où un service typé
est servi (suite, non planifiée dans ce lot). `Struct` encode les nombres en double : I64/U64/Decimal perdent de
la précision sur ce fil (documenté dans `derive/mod.rs`).

### 3.4 Dérivations par protocole

| Surface | Dérivation | Fichier(s) | Flux |
|---|---|---|---|
| REST | couche de validation structurelle (POST `[plural]`, PUT/PATCH `[plural, id]`, même résolveur que `Authorization::requirement`) → `engine::validate` ; `Validated<T>` = shim ; `QueryParams` → `parse_legacy` ; erreur → `ApiError` (`core/errors`) | `core/validation/*`, `core/query.rs` | validation |
| GraphQL | SDL `derive::graphql_sdl` en cache par `generation` (repli sample) ; créateurs enveloppés ; gardes `authorize_op` + `guarded_fetcher` (y compris liens imbriqués) ; `ContractError` → `async_graphql::Error` | `graphql/*` | schema, auth |
| gRPC | identité depuis les métadonnées ; `ContractError` → `Status` (fondation) ; `.proto` dérivé = **documentation** | `grpc/*` | auth, schema |
| WebSocket | mêmes gardes que GraphQL pour les abonnements | `websocket/dispatcher.rs`, `graphql/subscription_handler.rs` | auth |
| Front Leptos | `this-descriptor` en wasm32 : `apply` (côté `Client`), `Query::check`, `render`, `ContractError` | glue (suite) | — |
| CLI | contexte Tera = `ModuleDescriptor` ; `this descriptor check|export|lock` | this-cli | cli |

Transport inter-couches : `ContractError` voyage **dans `anyhow::Error`** ; chaque surface appelle
`this::core::errors::from_anyhow` (descente sur toute la chaîne de causes, sinon `this.internal` sans fuite du
texte d'origine). La fondation fournit aussi `ApiError` (`IntoResponse`, en-tête `x-this-error`), `to_status`
et `to_graphql_error` : aucun flux ne réimplémente la table.

Corps HTTP (additif ; RFC 9457 compris) :

```json
{ "error": "Validation échouée", "errors": ["Le champ 'amount' doit être positif (valeur: -1)"],
  "code": "this.validation.failed", "kind": "validation", "message_key": "this.validation.failed",
  "message": "Validation échouée",
  "fields": [{ "path": "amount", "code": "this.validation.positive", "message_key": "this.validation.positive",
               "message": "Le champ 'amount' doit être positif (valeur: -1)", "params": {"field": "amount", "value": -1} }],
  "type": "urn:this:error:this.validation.failed", "title": "Validation failed", "status": 422,
  "detail": "Validation échouée" }
```

### 3.5 Validation multi-protocole

- **Structurelle**, pas opt-in : (1) le flux validation construit les créateurs du `HostCatalog` déjà enveloppés
  dans `ValidatingCreator` pour toute entité décrite (GraphQL, gRPC, liens) ; (2) une **couche REST** reconnaît
  les écritures CRUD et valide avant le handler (attrape les handlers engendrés par la CLI, qui capturent leur
  propre créateur et dont l'`update` ne passe par aucun créateur) ; (3) `Validated<T>` devient un shim.
- `engine::validate::apply` : forme → clés (inconnue : `unknown_field`, sauf capacité
  `this.allow_unknown_fields` ; lecture seule ; immuable sur update) → transformations → présence (création :
  champ requis absent ; toute opération : `null` explicite sur un champ non optionnel) → **type**
  (`type_mismatch`, plus jamais un 500) → validateurs. Toutes les violations dans UNE `this.validation.failed`.
- Double passage (shim REST puis `ValidatingCreator`) : sans risque car une transformation `Custom` doit être
  déclarée idempotente à l'enregistrement (refus sinon) ; le flux validation peut en plus marquer le payload.
- `Custom` : `CustomScope::{Both, ServerOnly}` ; côté `Side::Client`, un `ServerOnly` est rendu dans `deferred`,
  jamais ignoré ni bloquant ; côté serveur, un nom sans implémentation est un refus.

### 3.6 Auth multi-protocole

- **Un seul évaluateur** : `engine::policy::evaluate -> Option<bool>` (trois valeurs, `Not(None)=None`, `None` au
  sommet = refus), avec `Perm` (IAM) et `Deny`. Différence assumée avec `check_with_resolver` : un résolveur
  **inconnu** est « non tranchable » (donc `not:resolver:inconnu` refuse).
- `PolicyExpr` : grammaire YAML historique + `perm:` + forme imbriquée `all(..)/any(..)/not(..)` + `role:a|b` ;
  **jeton inconnu = erreur** ; `Display` canonique (aller-retour testé). Le YAML et le descripteur ont UNE
  syntaxe ; l'égalité se compare **sur l'AST**, plus sur des chaînes.
- Conversion : `PolicyExpr → AuthPolicy` directe (`TryFrom`, flux auth) ; les opérations métier vont dans une table
  `custom_ops` lue par `authorize_op`. Pas de passage par les 9 chaînes d'`EntityAuthConfig`.
- Liens : priorité `LinkDesc.permissions` → entité de la portée → défaut (`LinkDesc::effective_policy`). Un
  tableau de lien partiel **ne complète pas** à `authenticated` (écart volontaire avec `default_link_auth_policy`,
  limité aux liens décrits ; test `link_policy_precedence_matches_rest` du flux auth).
- `server/auth/surface.rs::authorize_op(&PolicyIndex, entity, &Operation, &AuthContext, owner)` (sous `server/`,
  pas `core/`). `Owner` sur `List` : `owner = None` → non tranchable → refus, sauf filtrage par ligne après
  lecture (documenté par le flux auth).
- Les fetchers ne sont plus accessibles qu'à travers `host.guarded_fetcher(entity, Operation, &AuthContext)`
  (flux auth) ; un test de couverture échoue si `entity_fetchers.get(` apparaît hors de la garde.

### 3.7 Modules au runtime

- **`HostCatalog`** (fondation, `src/server/catalog.rs`) : instantané immuable `{ descriptors, fetchers, creators,
  links_config, plurals, generation }` publié par `LiveCatalog` (`ArcSwap`, une lecture atomique par requête, aucun
  verrou tenu à travers un `.await`). Construit par `ServerHost::from_builder_components`, égal aux champs
  historiques. **Lecteurs** : Authorization, GraphQL, gRPC (flux auth), liens et répartition (flux
  modules-runtime). Tant qu'un lecteur n'est pas migré, il voit l'instantané initial : le chargement à chaud
  n'est **déclaré servi** que sur les surfaces migrées, et le test `hot_loaded_module_served_on_rest_graphql_grpc`
  (intégration) le vérifie avec l'autorisation **armée**.
- **Pas de fallback** pour le routage dynamique : une **couche tower de répartition** placée avant le `Router` (et
  sous les couches auth/CORS) lit le premier segment, le cherche dans `HostCatalog::entity_of_plural` et envoie
  au routeur du module ou laisse passer. Tests `dynamic_module_paths_not_shadowed_by_link_wildcards` (avec et sans
  feature `frontend`).
- Résolution (`this_descriptor::resolve`) : tri topologique stable (indépendant de l'ordre d'enregistrement),
  semver, cycles, doublons de module/entité, références (liens, `Ref`) locales ou dans la fermeture des
  `requires`. **`ServerBuilder::register_module` ne fait plus que stocker** ; `build_host` résout, vérifie
  l'identité (`name`/`version`/`entity_types` ⇔ descripteur), enregistre les entités dans l'ordre résolu,
  vérifie le pluriel (`EntityDesc.plural == EntityDescriptor::plural == EntityConfig.plural`, sinon
  `this.module.plural_mismatch`) et refuse un fetcher/créateur en double dès qu'un module décrit est en cause.
- Cycle de vie : 6 états ; `Draining → Active` (annulation), `Active → Failed` (panne) ; délai de grâce 30 s puis
  annulation des flux (SSE/WS/gRPC). `replace(name, version)` vérifie les `VersionReq` des dépendants et échange
  par l'instantané (flux modules-runtime).
- `ModuleLoader` (trait) / `ModuleSource` (enum) / `ModuleInterface` / `ModuleStatus` : `src/server/module_runtime/`.
- Confiance : `load`/`put` derrière l'opération admin `this.module.load` ; module stocké vérifié avec
  `CheckProfile::Stored` (politiques explicites, `public` interdit sauf option d'hôte `allow_public`) ; espaces de
  noms imposés (codes et clés préfixés `<module>.`, `this.` réservé) ; liste blanche / signature : suite.
- `JsonEntityService` (entités déclaratives) = **adaptateur** `EntityFetcher`/`EntityCreator` branché sur les mêmes
  chemins d'événements et de liens (test `json_entity_publishes_event_and_links`).

### 3.8 `ModuleStore` / VFS

`src/storage/vfs/mod.rs` (fondation) : `VfsPath` (absolu, refuse `..`, `.`, `\`, NUL, segments vides),
`VfsMeta`, `VfsEntry`, `Precondition`, `VfsCaps`, `VfsError` (→ `ContractError` `this.vfs.*`, `IfMatch` raté →
`Aborted`), trait `Vfs`, et **trait** `ModuleStore { put(descriptor, bundles), get, bundle(name, version, locale),
list, remove }`. Disposition : `descriptor.json` **sans textes**, `i18n/<locale>.json` = un `MessageBundle` par
langue, `assets/**`. `put` appelle `check(Stored)` **et** `check_compat` contre la plus haute version.
Implémentations (`InMemoryVfs`, `LocalFsVfs`, `ObrainVfs`, `VfsModuleStore`) : flux vfs.

## 4. Alternatives rejetées

| Alternative | Pourquoi rejetée |
|---|---|
| Module `this::descriptor` interne à `this-rs` | pas compilable en wasm32, impose tout le framework à la CLI |
| Dépôt séparé pour le descripteur | contrat et usages divergeraient ; deux PR pour un changement |
| Proc-macro `#[derive(Entity)]` | changement de syntaxe pour tous les projets ; les `macro_rules!` émettent `Described` de façon additive (mesuré). Gardé comme évolution (types exacts sans sonde) |
| `descriptor/module.yaml` propriétaire des champs d'un module compilé | le struct Rust existe déjà : le YAML redéclarerait chaque champ (objection coh. C1, voir §9) |
| Inférer depuis le sample JSON en mieux | indécidable pour les `Option`, les enums, les tags |
| Schéma-d'abord (`.proto`, SDL) | seconde source ; ne porte ni validateurs, ni i18n, ni permissions |
| Crate `validator` (derive) | non sérialisable ; **retiré** (flux validation) |
| Codes d'erreur `u32` | illisibles, collisions ; chaîne à espace de noms |
| Chargement de code (dylib, `abi_stable`) | ABI Rust instable ; plugins wasm notés comme suite |
| `DynamicRouter` en `fallback` | le fallback est déjà pris (liens, sites `frontend`) et les jokers de liens capturent les chemins dynamiques |
| `Arc<RwLock<DescriptorSet>>` | garde tenue à travers `.await` ; remplacé par l'instantané `ArcSwap` |
| `derive::typescript` | aucun consommateur (le front est Leptos) : suite |

## 5. Impact fichier par fichier

Dépôt `this` :

| Fichier | Changement | Flux |
|---|---|---|
| `Cargo.toml` | `[workspace]`, `this-descriptor` (version+path), `arc-swap` ; features grpc/graphql → descriptor ; retrait `validator` (validation) | fondation |
| `.github/workflows/{ci,release}.yml` | test + wasm32 du crate ; publication de `this-descriptor` avant `this-rs` | fondation |
| `crates/this-descriptor/**` | tout le §3.2, moteurs, table, catalogue framework | fondation (moteurs enrichis par les flux) |
| `crates/this-descriptor/src/derive/*.rs` | SDL, proto (documentation), JSON Schema | schema |
| `src/lib.rs` | `pub use this_descriptor as descriptor;` | fondation |
| `src/core/pluralize.rs` | ré-export de `this_descriptor::naming::Pluralizer` | fondation |
| `src/core/module.rs` | `Module::descriptor() -> None` ; `EntityFetcher::list_query` (défaut O(n)) | fondation |
| `src/core/errors/mod.rs` | `from_anyhow`, `ApiError`, `http_status`, `to_status`, `to_graphql_error` | fondation (enrichi : errors-i18n) |
| `src/core/i18n/mod.rs` | ré-exports + `framework_bundles` ; extracteurs `Locale` | fondation / errors-i18n |
| `src/server/catalog.rs` | `HostCatalog`, `LiveCatalog` | fondation |
| `src/server/host.rs` | champ `catalog`, `with_descriptors`, `descriptors()` ; `guarded_fetcher` (auth) | fondation, auth |
| `src/server/builder.rs` | `register_module` stocke ; `build_host` résout, vérifie identité et pluriels, ordre topologique, doublons | fondation (runtime ensuite) |
| `src/server/module_runtime/{mod,loader}.rs` | `ModuleLoader`, `ModuleSource`, `ModuleInterface`, `ModuleStatus` | fondation |
| `src/server/module_runtime/{runtime,dispatch,json_entity}.rs` | runtime, couche de répartition, adaptateur JSON | modules-runtime |
| `src/storage/vfs/mod.rs` | traits `Vfs`, `ModuleStore`, types | fondation |
| `src/storage/vfs/{memory,fs,obrain,module_store}.rs` | implémentations | vfs |
| `src/entities/macros.rs` | `impl Described` + `Type::descriptor()` émis par `impl_data_entity!` et `_validated!` | fondation |
| `src/core/validation/{gate,layer,extractor,config}.rs`, `src/core/query.rs` | `ValidatingCreator`, couche REST, shim | validation |
| `src/core/auth/descriptor_policy.rs`, `src/server/auth/surface.rs`, `grpc/{identity,entity_service,link_service,mod}.rs`, `graphql/{executor/*,dynamic_schema,schema,field_resolver?,subscription_handler,mod}.rs`, `exposure/mod.rs`, `middleware/authorization.rs` | évaluateur unique, gardes, `guarded_fetcher` | auth |
| `graphql/schema_generator.rs`, `grpc/proto_generator.rs` | descripteur d'abord, cache par `generation` | schema |
| `src/config/mod.rs`, `src/server/entity_registry.rs` | `from_descriptor` (fusion par clé), `merge_strict`, `try_register` | modules-runtime |
| `src/links/handlers.rs` (l. 194-204, 572-575), `tests/module_descriptor_e2e.rs`, `examples/descriptor_api/` | erreurs typées dans les liens, e2e multi-protocole | intégration |

Dépôt `this-cli` : `Cargo.toml` (version+path), `.github/workflows/{ci,release}.yml` (checkout de `this`),
`src/descriptor/{mod,io,lock}.rs`, `src/commands/{add_entity,add_link,doctor,descriptor}.rs`,
`src/codegen/introspect.rs`, `src/utils/naming.rs` (→ `this_descriptor::naming`),
`src/templates/entity/{descriptor.rs.tera,handlers.rs.tera,model.rs.tera}`, `src/templates/project/module.rs.tera`.

## 6. Plan de migration additif

1. **Fondation** (faite) : crate, contrats, moteurs, `Module::descriptor()` à `None`, catalogue, résolution au
   build. Aucun comportement ne change sans descripteur.
2. **Sept flux en parallèle** sur `md/<flux>` issus de `integ/module-descriptor` (fondation fusionnée). Chaque
   flux n'active son comportement que pour les entités **décrites**.
3. **Intégration** : branchements transverses, exemple + e2e multi-protocole.
4. **Dépréciations** (lot suivant) : `EntityDescriptor` sans champs, `*_unenforced`, inférence par sample,
   `AuthPolicy::parse_policy` (repli `authenticated` → `warn`).
5. Rien n'est retiré dans ce lot, sauf la dépendance morte `validator`.

## 7. Découpage en flux

### 7.0 Fondation — fait, contrats publics consommés par les flux

Rust (`this_descriptor::…`) : `ModuleDescriptor`, `ModuleReq`, `DescriptorSet`, `CompiledEntity`, `EntityDesc`,
`EntityKind`, `base_fields`, `FieldDesc`, `FieldType`, `FieldTypeOf`, `ValidatorSpec`, `ValidatorRule`,
`TransformSpec`, `TransformRule`, `Applies`, `LinkDesc`, `ErrorCode`, `ErrorKind`, `ErrorDesc`, `ContractError`,
`FieldViolation`, `error::ERROR_HEADER`, `framework::{codes, FRAMEWORK_ERRORS, kind_of, bundle, descriptor}`,
`Operation`, `PolicyExpr`, `PermDesc`, `PrincipalView`, `MessageKey`, `Locale`, `I18nDecl`, `MessageDecl`,
`MessageBundle`, `MessageSyntax`, `FilterOp`, `FilterExpr`, `SortSpec`, `SortDir`, `Page`, `Query<T>`, `Untyped`,
`QuerySchema`, `Custom`, `CustomRegistry`, `CustomKind`, `CustomScope`, `ModuleState`, `Described`,
`CheckProfile`, `DescriptorIssue`, `check::issues_to_error`, `resolve::{resolve, dependents, Resolution}`,
`compat::check_compat`, `naming::{Pluralizer, is_identifier, to_snake_identifier}`,
`engine::{validate::{apply, apply_desc, WriteOp, Side, ValidateCtx, ValidationOutcome}, query::{parse_legacy,
parse_filter_json, parse_sort, check, matches, apply_in_memory}, policy::{evaluate, is_allowed, PolicyEnv},
i18n::{negotiate, render, localize, substitute}}`, `derive::{graphql_sdl, proto_file, json_schema}` (stubs).

this-rs : `this::descriptor` (ré-export), `this::core::module::{Module::descriptor, EntityFetcher::list_query}`,
`this::core::errors::{from_anyhow, ApiError, http_status, to_status, to_graphql_error}`,
`this::core::i18n::framework_bundles`, `this::server::catalog::{HostCatalog, LiveCatalog}`,
`ServerHost::{catalog, with_descriptors, descriptors}`,
`this::server::module_runtime::{ModuleLoader, ModuleSource, ModuleInterface, ModuleStatus}`,
`this::storage::vfs::{Vfs, VfsPath, VfsMeta, VfsEntry, Precondition, VfsCaps, VfsError, ModuleStore}`, macros
`impl_data_entity!` / `impl_data_entity_validated!` → `impl Described` + `Type::descriptor()`.

### 7.1 Flux (fichiers disjoints, sauf `host.rs`/`builder.rs` touchés en séquence : fondation → auth → modules-runtime)

| Flux | Fichiers | Tests d'acceptation (noms visibles) |
|---|---|---|
| **validation** | `src/core/validation/{gate,layer,extractor,config}.rs`, `src/core/query.rs`, `Cargo.toml` (retrait `validator`), `benches/validate_bench.rs` (harnais maison) | `validating_creator_blocks_invalid_graphql_payload`, `rest_create_validated_without_validated_extractor`, `generated_rest_update_rejects_invalid_payload`, `legacy_validated_body_keeps_error_and_errors_keys`, `query_params_malformed_filter_is_400`, `link_filter_on_target_field_still_accepted` (via handlers) |
| **errors-i18n** | `src/core/errors/{http,grpc,graphql}.rs`, `src/core/i18n/extract.rs` | `locale_extractor_reads_accept_language`, `grpc_locale_from_metadata`, `contract_error_to_grpc_status_code_and_details`, `contract_error_to_graphql_extensions`, `localized_body_in_requested_locale` |
| **auth** | `src/core/auth/descriptor_policy.rs`, `src/server/auth/surface.rs`, `src/server/host.rs` (`guarded_fetcher`), `middleware/authorization.rs` (évaluateur unique, catalogue), `grpc/{identity,entity_service,link_service,mod}.rs`, `graphql/{executor/*,dynamic_schema,schema,subscription_handler,mod}.rs`, `exposure/mod.rs`, `websocket/dispatcher.rs` | `perm_desc_nested_roundtrip`, `perm_desc_to_auth_policy_equivalent`, `policy_evaluator_matches_middleware_table`, `grpc_identity_from_metadata`, `grpc_create_denied_without_role`, `graphql_mutation_denied_without_role`, `graphql_nested_link_denied_without_get_policy`, `no_entity_fetchers_access_outside_guard`, `ws_subscription_denied_without_role`, `link_policy_precedence_matches_rest`, `graphql_served_with_armed_policies_when_covered` |
| **schema** | `derive/{graphql,proto,json_schema}.rs`, `graphql/schema_generator.rs`, `grpc/proto_generator.rs` | `proto_emits_reserved_tags`, `proto_requires_tags_only_when_deriving`, `sdl_marks_optional_nullable`, `sdl_from_descriptor_without_sample`, `sdl_cached_by_generation`, `json_schema_required_on_create_only`, `generators_fall_back_to_sample_without_descriptor` |
| **modules-runtime** | `src/server/module_runtime/{runtime,dispatch,json_entity}.rs`, `src/server/builder.rs` (après auth), `src/server/entity_registry.rs`, `src/config/mod.rs` | `unload_refused_with_active_dependents`, `load_then_routes_served_unload_then_404`, `dynamic_module_paths_not_shadowed_by_link_wildcards`, `replace_checks_dependents_version_req`, `draining_times_out_and_cancels_streams`, `merge_strict_reports_conflicts`, `from_descriptor_preserves_host_level_keys`, `json_entity_publishes_event_and_links` |
| **vfs** | `src/storage/vfs/{memory,fs,obrain,module_store}.rs` | `vfs_conformance_*` par backend, `fs_vfs_sandboxed`, `module_store_put_rejects_invalid_descriptor`, `module_store_put_rejects_tag_reuse`, `module_store_get_highest_version`, `module_store_bundles_per_locale`, `obrain_vfs_conformance` |
| **cli** (this-cli) | voir §5 | `add_link_preserves_unknown_yaml_keys`, `add_entity_writes_descriptor`, `descriptor_lock_detects_tag_reuse`, `cli_uses_shared_pluralizer`, `templates_render_from_descriptor`, `descriptor_check_cli_exit_code`, un test par contrôle de `doctor` (pluriel, politiques à l'AST, couverture i18n, espaces de noms, lock, entity_types, entités citées par events/sinks), `generated_descriptor_matches_code` |

Puis **intégration** : `tests/module_descriptor_e2e.rs` (même payload invalide en REST/GraphQL/gRPC → même code
`this.validation.positive`, 422 / `INVALID_ARGUMENT` / `BAD_USER_INPUT`, message `fr` puis `en` ; refus de
politique sur les trois ; `hot_loaded_module_served_on_rest_graphql_grpc` et
`hot_loaded_module_policy_enforced_on_all_surfaces` avec autorisation armée ; `rest_linked_create_returns_422_contract_error`).

## 8. Risques et pièges connus

- **Chemins relatifs des dépendances locales** : depuis `_wt-md/<flux>`, `../../banc-obrain-j4` et `../../vite-rs`
  pointent sous `/Users/triviere/projects/this-rs/`. Remède appliqué (hors checkout) : liens symboliques
  `this-rs/banc-obrain-j4 → ../banc-obrain-j4` et `this-rs/vite-rs → ../vite-rs`.
- **Espace de travail parent** : `/Users/triviere/projects/this-rs/Cargo.toml` est un workspace local qui liste
  `this` comme membre. `this` déclarant désormais son propre `[workspace]`, ce parent doit exclure `this` (ou
  lister `this/crates/this-descriptor`) ; sinon cargo signale deux racines. Fichier hors dépôt : à ajuster par
  l'utilisateur.
- **Ordre d'enregistrement** : les entités sont désormais enregistrées dans `build_host`, plus dans
  `register_module`. Un code qui lisait le registre du builder entre les deux n'existe pas dans le dépôt (mesuré
  par lecture), mais un projet externe pourrait en dépendre.
- **Typo d'opération dans `impl_data_entity_validated!`** (`creat:`) : c'était ignoré en silence, c'est désormais
  une **erreur de compilation** (seul changement non additif, voulu).
- **Règle de macro sur un champ inconnu** : la construction du descripteur panique (message explicite) au premier
  `entity_desc()` ; l'ancien chemin `ValidatableEntity` l'ignorait.
- **Fail-closed sur clés inconnues** : actif seulement avec un descripteur, désactivable par la capacité
  `this.allow_unknown_fields`.

## 9. Réponses aux objections

Légende : **C** = corrigé (code ou RFC), **R** = réfuté (raison), **D** = reporté à un flux ou une suite nommés.
Les noms de tests cités sous « fondation » sont exécutés dans `md/foundation` (voir rapport du flux) ; ceux
cités pour un flux sont ses tests d'acceptation (§7.1).

### 9.1 Lentille archi

| # | Objection | Verdict | Où |
|---|---|---|---|
| A1 | Hôte figé : modules à chaud absents de GraphQL/gRPC/liens/autorisation | **C** (fondation) + **D** (lecteurs) | `server/catalog.rs` (`HostCatalog`, `LiveCatalog`, test `readers_keep_their_snapshot_across_publication`) ; migration des lecteurs : auth (Authorization, GraphQL, gRPC), modules-runtime (liens, répartition) ; e2e armé `hot_loaded_module_*` (§3.7) |
| A2 | `DynamicRouter` en fallback : emplacement pris, jokers de liens | **C** | §3.7 : couche tower de répartition avant le `Router` ; test `dynamic_module_paths_not_shadowed_by_link_wildcards` (modules-runtime) ; alternative rejetée §4 |
| A3 | `evaluate -> bool`, pas de `perm:`, repli fail-open | **C** | `engine/policy.rs` (`Option<bool>`), `PolicyExpr::Perm`, `FromStr` en erreur ; tests `not_of_unevaluable_stays_unevaluable`, `policy_expr_rejects_unknown_token`, `three_valued_equivalence_table` |
| A4 | GraphQL imbriqué / gRPC liens lisent sans politique | **D** (auth) | `guarded_fetcher`, fichiers `field_resolver.rs`, `schema.rs`, `subscription_handler.rs`, `link_service.rs` ajoutés au flux auth ; tests `graphql_nested_link_denied_without_get_policy`, `no_entity_fetchers_access_outside_guard` |
| A5 | this-cli → descripteur par chemin seul : CI et publication cassées | **C** (this) + **D** (cli) | `Cargo.toml` (version+path), `release.yml` publie `this-descriptor` d'abord, `ci.yml` le teste ; workflows this-cli confiés au flux cli |
| A6 | Validation REST non structurelle | **C** (décision) + **D** (validation) | §3.5 couche REST ; test `rest_create_validated_without_validated_extractor` |
| A7 | PermDesc → EntityAuthConfig par chaînes : perte | **C** | conversion AST directe (§3.6), forme imbriquée + `role:a\|b`, test `policy_display_fromstr_roundtrip_with_nesting` ; `custom_ops` (auth) |
| A8 | `Operation::Custom` clé de map non sérialisable | **C** | `Operation` sérialisé en chaîne ; `descriptor_json_roundtrip` inclut `invoice.pay` |
| A9 | Champs de base sans sémantique d'écriture | **C** | `base_fields` (lecture seule / immuable), `tenant_id` `read_only` ; tests `base_fields_read_only_on_update`, `read_only_and_unknown_fields_rejected` |
| A10 | Filtres typés : pas de plomberie, liens à chemins pointés | **C** + **D** | `QuerySchema::for_links` (préfixes), `EntityFetcher::list_query` (O(n) documenté), `apply_in_memory` filtre avant pagination ; tests `link_filter_on_target_field_still_accepted`, `filter_applies_before_pagination` ; GraphQL/gRPC : **REST seulement dans ce lot** (écrit) |
| A11 | Conversion d'erreur nécessaire à plusieurs flux | **C** | `core/errors/mod.rs` (`from_anyhow`, `ApiError`, `to_status`, `to_graphql_error`) ; tests `from_anyhow_downcasts_contract_error`, `from_anyhow_unknown_is_internal`, `contract_error_to_grpc_code` ; `links/handlers.rs` → intégration |
| A12 | Résolution dépendante de l'ordre ; entités enregistrées avant | **C** | `register_module` stocke ; `build_host` résout puis enregistre ; tests `resolve_independent_of_registration_order`, `build_host_resolve_independent_of_registration_order` |
| A13 | Moteurs purs sans chrono/uuid | **C** | dépendances ajoutées ; test `date_format_matches_legacy_validator` ; wasm32 vérifié |
| A14 | Performance (regex, descripteur par requête, SDL par requête, garde) | **C** + **D** | `CompiledEntity` (regex compilées), `Described -> &'static` (`OnceLock`), `ArcSwap`, `generation` ; cache SDL (schema) ; banc : **R** sur criterion (dépôt construit hors ligne, bancs à harnais maison) → `benches/validate_bench.rs` `harness=false` (validation) |
| A15 | Pas d'isolation d'espaces de noms ni de modèle de confiance | **C** + **D** | `check()` : codes/clés préfixés, `this` réservé ; `DescriptorSet` refuse les codes en double ; `render` cherche les lots du module avant le framework (espaces disjoints) ; `CheckProfile::Stored{allow_public}` ; références vérifiées par `resolve` ; opération admin `this.module.load` (runtime) ; signature / liste blanche : suite |
| A16 | Arêtes core → server | **C** | `module_runtime` sous `server/` ; `surface.rs` sous `server/auth/` ; `authorize_op(&PolicyIndex, ..)` |
| A17 | Pluriel à trois sources, sans contrôle au démarrage | **C** | `ServerBuilder::check_plurals` ; test `plural_drift_fails_at_startup` |
| A18 | Tags proto : protègent un artefact qui n'est pas le fil | **C** | `proto_tag: Option<u32>`, documenté §3.3 |
| A19 | Taille wasm : features par défaut | **C** | `default = []` ; `yaml`, `pattern` opt-in |
| A20 | Pas de remplacement à chaud ; Draining sans borne | **C** + **D** | transitions `Draining→Active`, `Active→Failed` (`lifecycle_transitions_are_legal_only`) ; `DEFAULT_DRAIN_GRACE_MS` ; `replace` (modules-runtime) |
| A21 | Détails d'API (bornes de Query, i18n embarqué, null explicite, Owner sur List, Custom côté front) | **C** sauf un **R** | `#[serde(bound="")]` + impls manuelles ; lots par locale ; `update_validates_only_present_fields_but_refuses_explicit_null` ; `owner: Option<bool>` (None = non tranchable) ; **R** « Custom inconnu sauté côté front » : fail-closed conservé, un `ServerOnly` déclaré est rendu `deferred` (`client_profile_defers_server_only_custom`) |

### 9.2 Lentille cohérence

| # | Objection | Verdict | Où |
|---|---|---|---|
| C1 | Deux sources (macro et module.yaml) sans priorité | **C**, proposition « YAML propriétaire des champs » **R** | un propriétaire par facette (§2.1), `ModuleDescriptor::compose` (`compose_gives_each_facet_one_owner`) ; la macro n'attribue jamais de tag ; types via `FieldTypeOf` + sonde (`probe_specialises_known_types_and_falls_back`). Rejet : rendre le YAML propriétaire des champs obligerait à redéclarer le struct Rust |
| C2 | Pluriel à 6 endroits, deux algorithmes | **C** + **D** (cli) | `this_descriptor::naming::Pluralizer` (unique), macro et `EntityDesc::new` l'utilisent (`macro_emits_described_types_and_plural` : `knives`) ; `descriptor.rs.tera`, `utils/naming.rs` → cli (`cli_uses_shared_pluralizer`) |
| C3 | Validation : REST engendré par la CLI hors du goulot | **C** + **D** | couche REST structurelle (§3.5) ; templates `handlers.rs.tera`/`descriptor.rs.tera` au flux cli ; `generated_rest_update_rejects_invalid_payload` |
| C4 | Tags proto : contrat non servi, précision de `Struct` | **C** (option a) | §3.3 et `derive/mod.rs` : documentation seulement, perte de précision écrite |
| C5 | Chargement à chaud incompatible avec un hôte figé | **C** + **D** | = A1 ; `host.rs` touché en séquence fondation → auth → modules-runtime (§7.1) |
| C6 | `from_str` hérite du repli fail-open ; pas d'imbrication | **C** | = A3/A7 ; `policy_expr_rejects_unknown_token` (`admin_onyl`) |
| C7 | Permissions de liens : opérations et priorité | **C** | `Operation::GetLink` ; `LinkDesc::effective_policy` (`link_policy_precedence_link_then_entity_then_default`) ; écart volontaire sur le tableau partiel, écrit §3.6 |
| C8 | Clé `Operation::Custom` en JSON | **C** | = A8 ; `operation_string_form_and_custom_default_deny` |
| C9 | Obligation déclarée deux fois ; pas de contrôle de type | **C** | `required_on` dérivé d'`optional` ; `check()` : `contradiction` (`check_rejects_required_on_optional_field`) ; `type_mismatch` (`wrong_type_is_validation_not_internal`) |
| C10 | Table de tags incohérente ; liste redéclarée par moteur | **C** | `base_fields(EntityKind)` unique, tags des liens 9-11, `tenant_id` nom réservé ; `base_fields_read_only_semantics` |
| C11 | Catalogue framework dans this-rs (non wasm32) | **C** | `this_descriptor::framework` (fr, en) ; `framework_codes_are_unique_namespaced_and_translated` |
| C12 | code→kind redéclaré ; espaces de noms ; `this.validation.failed` absent | **C** | `ContractError::from_desc`, `ContractError::framework` ; `check_enforces_namespaces` ; `all_emitted_codes_are_declared` |
| C13 | Traductions à deux endroits | **C** | `I18nDecl` (clés) dans le descripteur, `MessageBundle` par locale hors descripteur ; `ModuleStore::bundle` |
| C14 | Filtres « multi-protocole » REST seulement | **C** (portée écrite) | = A10 ; GraphQL/gRPC : REST seulement dans ce lot |
| C15 | Copies CLI de `LinksConfig` ; events/sinks/auth hors descripteur | **D** + **C** (classement) | classement écrit : `entities`/`links`/`validation_rules` dérivés du descripteur ; `events`/`sinks`/`auth` = niveau hôte, hors descripteur, préservés par `from_descriptor` (`from_descriptor_preserves_host_level_keys`). Extraction des types serde de `config` vers `this_descriptor::config` : **reportée** (touche `config/mod.rs`, fichier du flux modules-runtime) → tâche de suite ; la CLI garde `flatten` en attendant |
| C16 | Identité du module redéclarée ; collisions de fetchers | **C** + **D** | `build_host` exige `name` (normalisé snake) / `version` / `entity_types` ⇔ descripteur (`identity_mismatch_between_trait_and_descriptor_fails`) ; doublon de fetcher/créateur = erreur si un module décrit est en cause ; `module.rs.tera` → cli |
| C17 | Stabilité des tags garantie par la seule CLI | **C** | `compat::check_compat` (`tag_reuse_move_and_unreserved_removal_detected`) ; contrat de `ModuleStore::put` ; `module_store_put_rejects_tag_reuse` (vfs) |
| C18 | Chemin seul : publication et CI de this-cli | **C** + **D** | = A5 |
| C19 | Doctor statique, contrôles non listés | **D** (cli) | liste des contrôles et un test chacun (§7.1) ; `generated_descriptor_matches_code` |
| C20 | Conversion PermDesc nécessaire à deux flux | **C** | `Display`/`FromStr` de `PolicyExpr` dans la fondation ; `from_descriptor` produit des chaînes canoniques |
| C21 | Grammaire `validate:` : opération inconnue ignorée ; pas de fusion | **C** | `__this_applies!` → `compile_error!` ; fusion Create+Update → Always (`normalize_fills_plural_and_merges_create_update_rules`, `macro_emits_described_with_validators`) ; test trybuild **R** : pas de dev-dépendance trybuild, le cas est un `compile_error!` direct |
| C22 | `entity_desc()` alloue à chaque appel | **C** | `&'static` via `OnceLock` (`macro_emits_described_types_and_plural` vérifie l'identité du pointeur) |
| C23 | Idempotence supposée des transformations Custom | **C** | `register_transform(.., idempotent, ..)` refuse `false` (`non_idempotent_transform_is_refused`) |
| C24 | WebSocket sans propriétaire ; derives manquants | **C** + **D** | `Copy/Eq/Hash/Ord` sur `ErrorKind`/`Operation`, `Display` sur `ErrorCode`/`MessageKey`, `Default=Always` ; `ws_subscription_denied_without_role` (auth) |

### 9.3 Lentille philosophie

| # | Objection | Verdict | Où |
|---|---|---|---|
| P1 | Moteur auth sans trois valeurs (régression) | **C** | = A3 ; `Authorization::evaluate` remplacé par le moteur (auth, `policy_evaluator_matches_middleware_table`) |
| P2 | Runtime REST seulement, politique indéfinie | **C** + **D** | = A1/A2 ; la répartition est montée **sous** l'autorisation ; e2e armé |
| P3 | « Déclaré une fois » non tenu | **C** | §2.1 + `compose` ; dérive = erreur de démarrage (`plural_drift_fails_at_startup`, `identity_mismatch_…`) |
| P4 | Tags proto ; seule la CLI les garde | **C** | = C4/C17 ; contrôle d'exécution : `check_compat` dans `ModuleStore::put` |
| P5 | `authenticated` par défaut pour Custom et modules stockés | **C** | `Deny` pour les opérations métier, `CheckProfile::Stored` ; `stored_module_without_permissions_rejected`, `operation_string_form_and_custom_default_deny` |
| P6 | Trois façons de dire « obligatoire » ; collision `filters` | **C** | `transforms` (renommé), présence dérivée ; `check_rejects_required_on_optional_field` |
| P7 | Custom non exécutables sur le front | **C** | `CustomScope`, `Side::Client` → `deferred` |
| P8 | `this.validation.failed` hors liste ; `tenant_id=8` | **C** | = C12/C10 |
| P9 | Le descripteur ne couvre pas tout `LinksConfig` | **C** (classement) + **D** | = C15 |
| P10 | Glue Leptos impossible hors du crate (orphelinat) | **C** (codes) + **D** | codes `this.transport.*` réservés ; l'`impl FromServerFnError for ContractError` ira dans `this-descriptor` derrière une feature `server-fn` (légal : le type est local) quand le coût wasm de `server_fn` sera mesuré — tâche de suite « glue Leptos » |
| P11 | i18n à deux endroits, sans pluriels | **C** + **D** | = C13 ; `MessageSyntax::Fluent` réservé, refusé par `check_bundle_syntax` jusqu'à un moteur (suite, inspiration `fluent-bundle`, à mesurer en wasm32) |
| P12 | `Query<T>` fantôme inutile | **C** | `Query<T = Untyped>` ; `Query::<T: Described>::check()` délègue au schéma (un mécanisme) |
| P13 | 10 états, trait + enum pour un chargeur | **C** (états) / **R** (trait) | 6 états ; le trait `ModuleLoader` est gardé : la tâche fondation l'exige et les tests du runtime ont besoin d'un chargeur factice (deuxième implémentation) ; `ModuleSource` reste l'enum des sources |
| P14 | `derive::typescript` et second moteur JSON | **C** + **D** | `typescript` retiré ; `JsonEntityService` = adaptateur (`json_entity_publishes_event_and_links`, modules-runtime) |
| P15 | Leptos : copier / adapter / combiner / inspirer / ne pas faire | **C** (A) + **D** (B, C, D) | A : `ERROR_HEADER = "x-this-error"` posé par `ApiError` (`api_error_status_body_and_header`) ; B = P10 ; C : `FieldViolation.path` compatible avec un rendu de formulaire sans JS (suite glue) ; D : enregistrement au lien type `inventory` côté serveur (suite) ; ne pas faire : server_fn comme 4e protocole, types réactifs dans le descripteur, `ServerFnError<String>` |
| P16 | Corps RFC 9457 | **C** | `http_body` ajoute `type`, `title`, `status`, `detail` ; `PROBLEM_JSON` ; négociation `Accept` (errors-i18n) |
| P17 | `Aborted`, précision des bornes | **C** | `ErrorKind::Aborted` (409 / `ABORTED`), `this.vfs.precondition_failed` → Aborted ; bornes en `serde_json::Number` (`min_max_keep_integer_precision`) ; `check()` refuse une borne non entière sur un champ entier |
| P18 | Idempotence de la double validation | **C** | = C23 |
