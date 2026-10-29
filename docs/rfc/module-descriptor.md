# RFC — ModuleDescriptor : un socle commun front/back, déclaré une fois

- Statut : proposé (29/10/2026)
- Auteur : agent architecte (1/12), branche `md/design` issue de `integ/module-descriptor`
- Portée : dépôt `this` (crate `this-rs` + nouveau crate `this-descriptor`), dépôt `this-cli`
- Sources lues avant rédaction (code réel, `main` à `c9386e0`) : `src/core/entity.rs`, `src/entities/macros.rs`,
  `src/server/entity_registry.rs`, `src/core/module.rs`, `src/server/builder.rs`, `src/server/host.rs`,
  `src/core/validation/{config,extractor,validators,filters}.rs`, `src/core/query.rs`, `src/core/auth/mod.rs`,
  `src/config/mod.rs`, `src/core/link.rs`, `src/server/exposure/{mod.rs,grpc/entity_service.rs,grpc/proto_generator.rs,graphql/schema_generator.rs}`,
  `this-cli/src/commands/add_entity.rs`, `this-cli/src/templates/entity/*.tera`.
- Notes d'audit (Project Orchestrator) : b63fc8ee (inventaire this), 6d11cc02 et b0a76933 (this-cli), 874f4449 (Leptos).
- Diagrammes : `docs/atlas/module-descriptor/*.mmd`.

---

## 1. Problème

Une entité de `this-rs` est déclarée **une fois en Rust** (struct engendrée par `impl_data_entity!`), puis
**redéclarée ou ré-inférée** par chaque protocole et chaque outil. Mesures relevées dans le code :

| Préoccupation | Où elle vit aujourd'hui | Défaut mesuré |
|---|---|---|
| Types | struct Rust ; `graphql/schema_generator.rs` et `grpc/proto_generator.rs` **infèrent** les champs d'un JSON (`EntityFetcher::get_sample_entity`) | `EntityDescriptor` (`server/entity_registry.rs`) n'a que `entity_type/plural/build_routes` ; numéros de champ proto = ordre des clés JSON, donc **instables** ; un sample vide donne un type vide |
| Nom / pluriel | YAML `entities` + `Entity::resource_name` + `EntityDescriptor::plural` + `Module::entity_types` + `match` dans `get_entity_fetcher/creator` | synchronisation manuelle, 3 à 6 copies |
| Erreurs | `anyhow` partout ; `StatusCode::` ad hoc ; gRPC `map_err(\|e\| Status::internal(..))` (`entity_service.rs`) ; `async_graphql::Error::new` inline | aucun code stable, aucune table de correspondance, une erreur métier devient `INTERNAL` en gRPC ; trois formes de corps HTTP |
| Validation | `EntityValidationConfig` + `Validated<T>` (REST, opt-in) | **contournée** par GraphQL (`executor/mutation_executor.rs`, `dynamic_schema.rs`, `link_mutations.rs`) et gRPC (`entity_service.rs`) qui appellent `create_from_json` directement ; `validate_and_filter` n'itère que sur les clés **présentes** → `required` ne voit jamais un champ absent ; crate `validator` déclaré, inutilisé |
| Filtres de requête | `QueryParams.filter: Option<String>` | `filter_value()` fait `.ok()` : JSON malformé **ignoré en silence** ; `sort` n'est lu nulle part ; GraphQL/gRPC = limit/offset |
| Auth | `AuthPolicy` + `EntityAuthConfig` (YAML, chaînes) ; application par route REST | GraphQL/gRPC/WS refusés tant que les politiques sont armées (`refuse_unenforced_surface`) ou servis **sans** politique (`*_unenforced`) ; gRPC n'extrait pas d'identité |
| i18n | néant | messages en dur, FR et EN mélangés |
| Modules | trait `Module` | aucune dépendance déclarée ; `LinksConfig::merge` et `EntityRegistry::register` : **le dernier gagne en silence** |
| CLI | copies manuelles de `LinksConfig` & co. dans `this-cli` | copies en retard → `this add link` **efface** `events`, `sinks`, `auth`, `required_fields` de `links.yaml` ; grammaire de macro connue 3 fois ; le contexte Tera (`entity_name`, `fields`, `validated`…) est un descripteur jamais matérialisé |
| Front (Leptos) | `FromServerFnError` | ni registre d'erreurs, ni validation, ni filtres typés, ni i18n, ni auth déclarative, ni modules : le socle ne peut pas venir de Leptos |

Racine commune : **il n'existe aucune description typée et sérialisable d'un module**. Chaque surface
ré-infère ce qu'elle peut, et validation / erreurs / auth / filtres n'ont aucune ancre partagée.

## 2. Philosophie

1. **Une déclaration.** Un module déclare une fois ses entités, champs, liens, règles, erreurs, permissions,
   messages et dépendances. Rien d'autre n'est écrit à la main : ni SDL, ni `.proto`, ni table d'erreurs, ni
   catalogue côté front.
2. **Dérivations par protocole.** REST, GraphQL, gRPC, WebSocket, le client Leptos et la CLI **dérivent** leur
   forme du descripteur par des fonctions pures et testables (`derive::*`). Une dérivation ne réinvente jamais
   une règle : elle traduit.
3. **Fail-closed.** Ce qui n'est pas déclaré est refusé : champ inconnu dans un filtre, tri sur un champ non
   triable, opération sans politique explicite → politique par défaut `authenticated` (identique à
   `default_auth_policy`), résolveur inconnu → refus, module dont une dépendance manque → non chargé,
   doublon d'entité entre modules → erreur (fin du last-wins silencieux).
4. **Additif et rétrocompatible.** `Module::descriptor()` a un défaut `None` : un module existant compile et se
   comporte à l'identique. Les inférences par sample restent le repli quand aucun descripteur n'est fourni.
   Les corps d'erreur HTTP gardent les clés `error` (et `errors` pour 422) et **ajoutent** `code`, `kind`,
   `message_key`, `fields`.
5. **Sérialisable et wasm32.** Le descripteur et ses moteurs purs (validation, filtres de requête, politique,
   rendu des messages, table des erreurs) vivent dans un crate sans tokio/axum/tonic, compilable en
   `wasm32-unknown-unknown` : le front Leptos valide, filtre et traduit **avec le même code**.
6. **Extensible via `Custom` nommé.** Chaque énumération ouverte (type de champ, validateur, transformation,
   opération, politique, capacité) a une variante `Custom { name, params }`. Un `Custom` non enregistré est
   **refusé** à la vérification du descripteur, jamais ignoré.

## 3. Solution

### 3.1 Où vit le socle : crate workspace `this-descriptor` dans le dépôt `this`

**Décision : un crate de workspace `crates/this-descriptor`, dans le dépôt `this`**, pas un module de
`this-rs`.

Justification :
- `this-rs` dépend de `tokio` (`full`), `axum`, `tonic`, `sqlx`… : il **ne compile pas** en `wasm32`. Un module
  interne ne pourrait pas être consommé par Leptos (note 874f4449 : « le socle doit vivre dans un crate sans
  UI compilable en wasm32 »).
- `this-cli` ne dépend pas de `this-rs` (note 6d11cc02) et ne doit pas tirer tout le framework pour lire un
  descripteur ; un crate serde-only règle la recopie de types en retard.
- Dans le même dépôt (et non un dépôt séparé) : les deux évoluent ensemble, une PR peut changer le contrat et
  son usage, le versionnement suit `this-rs`. `this-rs` le ré-exporte (`this::descriptor`).

Dépendances du crate : `serde`, `serde_json`, `thiserror`, `semver` (pures). Features :
`yaml` (`serde_yaml`, défaut activé), `pattern` (`regex`, défaut activé). Aucune dépendance async.

Conséquence pour le dépôt : le `Cargo.toml` racine reçoit
`[workspace] members = [".", "crates/this-descriptor"]` et
`this-descriptor = { path = "crates/this-descriptor", version = "0.1" }`. `this-cli` dépend de
`this-descriptor = { path = "../this/crates/this-descriptor" }` (voir §8, piège des chemins relatifs).

### 3.2 Types exacts (crate `this-descriptor`)

Tous les types : `#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]`, `#[serde(rename_all = "snake_case")]`
pour les énumérations, `#[non_exhaustive]` sur les énumérations publiques.

```rust
// ---------- module.rs ----------
pub const DESCRIPTOR_FORMAT: u32 = 1;

pub struct ModuleDescriptor {
    pub format: u32,                      // = DESCRIPTOR_FORMAT ; refus si supérieur
    pub name: String,                     // ^[a-z][a-z0-9_]*$, unique dans un hôte
    pub version: String,                  // semver
    pub requires: Vec<ModuleReq>,
    pub entities: Vec<EntityDesc>,
    pub links: Vec<LinkDesc>,
    pub errors: Vec<ErrorDesc>,           // codes métier du module
    pub i18n: I18nCatalog,                // messages du module, toutes langues
    pub custom: Vec<Custom>,              // capacités déclarées hors du noyau
}

pub struct ModuleReq {
    pub name: String,
    pub version: String,                  // semver::VersionReq, ex. "^1.2"
    #[serde(default)] pub optional: bool, // optionnelle : chargée si présente, jamais exigée
}

impl ModuleDescriptor {
    pub fn check(&self, registry: &CustomRegistry) -> Result<(), Vec<DescriptorIssue>>; // fondation
    pub fn entity(&self, name: &str) -> Option<&EntityDesc>;
    #[cfg(feature = "yaml")] pub fn from_yaml(s: &str) -> Result<Self, DescriptorIssue>;
    pub fn from_json(s: &str) -> Result<Self, DescriptorIssue>;
}

/// Index de tous les descripteurs actifs d'un hôte (porté par `ServerHost`).
pub struct DescriptorSet { /* BTreeMap<module, Arc<ModuleDescriptor>>, index entité → module */ }
impl DescriptorSet {
    pub fn insert(&mut self, d: ModuleDescriptor) -> Result<(), DescriptorIssue>; // doublon = erreur
    pub fn remove(&mut self, module: &str) -> Option<Arc<ModuleDescriptor>>;
    pub fn entity(&self, entity: &str) -> Option<(&ModuleDescriptor, &EntityDesc)>;
    pub fn error(&self, code: &ErrorCode) -> Option<&ErrorDesc>;
    pub fn modules(&self) -> impl Iterator<Item = &ModuleDescriptor>;
}

// ---------- entity.rs ----------
pub enum EntityKind { Data, Link }

pub struct EntityDesc {
    pub name: String,                     // singulier ("invoice")
    pub plural: String,                   // "invoices" — LA source unique du pluriel
    pub kind: EntityKind,
    pub fields: Vec<FieldDesc>,           // champs spécifiques ; champs de base implicites (§3.3)
    #[serde(default)] pub indexed: Vec<String>,
    #[serde(default)] pub reserved_tags: Vec<u32>,   // tags de champs supprimés, jamais réutilisés
    #[serde(default)] pub permissions: PermDesc,
    #[serde(default)] pub i18n: EntityI18n,
    #[serde(default)] pub custom: Vec<Custom>,
}

pub struct EntityI18n { pub label: Option<MessageKey>, pub label_plural: Option<MessageKey> }

// ---------- field.rs ----------
pub struct FieldDesc {
    pub name: String,
    pub ty: FieldType,
    #[serde(default)] pub optional: bool,             // Option<T> côté Rust, nullable ailleurs
    pub proto_tag: u32,                               // STABLE, unique dans l'entité, >= 16
    #[serde(default)] pub validators: Vec<ValidatorRule>,
    #[serde(default)] pub filters: Vec<TransformRule>,   // transformations d'entrée (trim…)
    #[serde(default)] pub filterable: Vec<FilterOp>,     // opérateurs de requête permis ; vide = non filtrable
    #[serde(default)] pub sortable: bool,
    #[serde(default)] pub i18n: FieldI18n,
    #[serde(default)] pub read_only: bool,            // refusé en écriture (create/update)
    #[serde(default)] pub deprecated: bool,
}

pub enum FieldType {
    String, Text, Bool, I32, I64, U32, U64, F32, F64,
    Decimal { scale: u8 }, Uuid, DateTime, Date, Json,
    Enum { name: String, variants: Vec<String> },
    List(Box<FieldType>),
    Ref { entity: String },                           // uuid vers une entité déclarée
    Custom(Custom),
}

pub struct FieldI18n { pub label: Option<MessageKey>, pub help: Option<MessageKey>, pub placeholder: Option<MessageKey> }

/// Quand une règle s'applique. `Always` = create et update.
pub enum Applies { Always, Create, Update }

pub struct ValidatorRule { pub rule: ValidatorSpec, #[serde(default)] pub on: Applies }
pub struct TransformRule { pub rule: TransformSpec, #[serde(default)] pub on: Applies }

pub enum ValidatorSpec {                              // = grammaire actuelle de add_validators_for_field! + ajouts
    Required, Optional, Positive,
    StringLength { min: usize, max: usize },
    MinValue(f64), MaxValue(f64),
    InList(Vec<serde_json::Value>),
    DateFormat(String),
    Pattern(String),                                  // feature "pattern"
    Email, Url,
    Custom(Custom),
}

pub enum TransformSpec { Trim, Uppercase, Lowercase, RoundDecimals(u32), Custom(Custom) }

// ---------- link.rs ----------
pub struct LinkDesc {
    pub link_type: String,
    pub source: String,                   // entité déclarée (ce module ou un module requis)
    pub target: String,
    pub forward_route: String,            // défaut = LinkDefinition::default_forward_route_name
    pub reverse_route: String,
    #[serde(default)] pub fields: Vec<FieldDesc>,     // remplace required_fields: Option<Vec<String>>
    #[serde(default)] pub permissions: PermDesc,      // ops ListLinks/CreateLink/DeleteLink/UpdateLink
    pub description: Option<String>,
}

// ---------- error.rs ----------
/// Code stable, `<espace>.<sujet>.<raison>` : "this.validation.required", "billing.invoice.already_paid".
pub struct ErrorCode(pub String);

pub enum ErrorKind {
    BadRequest, Validation, Unauthenticated, PermissionDenied, NotFound, Conflict,
    PreconditionFailed, RateLimited, NotImplemented, Unavailable, Timeout, Internal,
}

pub struct ErrorDesc {
    pub code: ErrorCode,
    pub kind: ErrorKind,
    pub message_key: MessageKey,
    #[serde(default)] pub params: Vec<String>,        // noms des paramètres attendus par le message
    pub description: Option<String>,
}

/// L'enveloppe unique, sur tous les protocoles, côté serveur ET côté front.
#[derive(thiserror::Error)]
#[error("{code}: {message}")]
pub struct ContractError {
    pub code: ErrorCode,
    pub kind: ErrorKind,
    pub message_key: MessageKey,
    pub message: String,                                  // rendu dans la locale négociée
    #[serde(default)] pub params: BTreeMap<String, serde_json::Value>,
    #[serde(default)] pub fields: Vec<FieldViolation>,
    pub trace_id: Option<String>,
}

pub struct FieldViolation {
    pub path: String,                                     // "amount", "lines[2].qty"
    pub code: ErrorCode,
    pub message_key: MessageKey,
    pub message: String,
    #[serde(default)] pub params: BTreeMap<String, serde_json::Value>,
}

impl ErrorKind {
    pub const fn http_status(self) -> u16;
    pub const fn grpc_code(self) -> i32;                  // valeurs de tonic::Code, sans dépendre de tonic
    pub const fn graphql_code(self) -> &'static str;
}
impl ContractError {
    pub fn new(code: impl Into<ErrorCode>, kind: ErrorKind) -> Self;
    pub fn with_param(self, k: &str, v: impl Serialize) -> Self;
    pub fn with_field(self, v: FieldViolation) -> Self;
    pub fn http_body(&self) -> serde_json::Value;        // forme rétrocompatible, §3.4
}
```

#### Table de correspondance (fondation, pure, testée exhaustivement)

| `ErrorKind` | HTTP | gRPC (`tonic::Code`) | GraphQL `extensions.code` |
|---|---|---|---|
| `BadRequest` | 400 | `INVALID_ARGUMENT` (3) | `BAD_REQUEST` |
| `Validation` | 422 | `INVALID_ARGUMENT` (3) | `BAD_USER_INPUT` |
| `Unauthenticated` | 401 | `UNAUTHENTICATED` (16) | `UNAUTHENTICATED` |
| `PermissionDenied` | 403 | `PERMISSION_DENIED` (7) | `FORBIDDEN` |
| `NotFound` | 404 | `NOT_FOUND` (5) | `NOT_FOUND` |
| `Conflict` | 409 | `ALREADY_EXISTS` (6) | `CONFLICT` |
| `PreconditionFailed` | 412 | `FAILED_PRECONDITION` (9) | `PRECONDITION_FAILED` |
| `RateLimited` | 429 | `RESOURCE_EXHAUSTED` (8) | `RATE_LIMITED` |
| `NotImplemented` | 501 | `UNIMPLEMENTED` (12) | `NOT_IMPLEMENTED` |
| `Unavailable` | 503 | `UNAVAILABLE` (14) | `UNAVAILABLE` |
| `Timeout` | 504 | `DEADLINE_EXCEEDED` (4) | `TIMEOUT` |
| `Internal` | 500 | `INTERNAL` (13) | `INTERNAL_SERVER_ERROR` |

Codes réservés du framework (espace `this.`) : `this.validation.{required,optional,positive,string_length,min_value,max_value,in_list,date_format,pattern,email,url,read_only,unknown_field}`,
`this.query.{malformed_filter,field_not_filterable,op_not_allowed,field_not_sortable,page_out_of_range}`,
`this.auth.{unauthenticated,forbidden,resolver_unknown}`, `this.entity.{not_found,conflict}`,
`this.module.{dependency_missing,dependency_version,cycle,duplicate_entity,has_dependents,format_unsupported,unavailable}`,
`this.vfs.{not_found,precondition_failed,read_only,invalid_path}`, `this.internal`.

```rust
// ---------- perm.rs ----------
pub enum Operation {
    List, Get, Create, Update, Delete,
    ListLinks, CreateLink, DeleteLink, UpdateLink,
    Custom(String),                                       // opération métier ("invoice.pay")
}

/// AST sérialisable de `AuthPolicy`. `AuthPolicy::Custom(fn)` n'est pas sérialisable :
/// il s'exprime par `Resolver(nom)`.
pub enum PolicyExpr {
    Public, Authenticated, Owner, ServiceOnly, AdminOnly,
    HasRole(Vec<String>),
    And(Vec<PolicyExpr>), Or(Vec<PolicyExpr>), Not(Box<PolicyExpr>),
    Resolver(String),
}

pub struct PermDesc {
    #[serde(default)] pub ops: BTreeMap<Operation, PolicyExpr>,   // op absente → PermDesc::DEFAULT
}
impl PermDesc {
    pub const DEFAULT: PolicyExpr = PolicyExpr::Authenticated;   // = default_auth_policy()
    pub fn policy(&self, op: &Operation) -> &PolicyExpr;
}
impl std::str::FromStr for PolicyExpr { /* grammaire YAML actuelle de AuthPolicy::parse_policy */ }

/// Vue pure du principal, pour évaluer sans this-rs (front, tests).
pub struct PrincipalView<'a> {
    pub authenticated: bool, pub user_id: Option<&'a str>, pub roles: &'a [String],
    pub admin: bool, pub service: bool, pub owner_of_resource: Option<bool>,
}

// ---------- i18n.rs ----------
pub struct MessageKey(pub String);                        // "billing.invoice.amount.label"
pub struct Locale(pub String);                            // BCP 47 : "fr", "fr-CA", "en"
pub struct I18nCatalog {
    pub default_locale: Locale,
    pub messages: BTreeMap<Locale, BTreeMap<MessageKey, String>>,  // gabarits "{min}…{max}"
}

// ---------- query.rs ----------
pub enum FilterOp { Eq, Ne, Gt, Gte, Lt, Lte, In, Contains, StartsWith, IsNull }
pub enum FilterExpr {
    And(Vec<FilterExpr>), Or(Vec<FilterExpr>), Not(Box<FilterExpr>),
    Cmp { field: String, op: FilterOp, value: serde_json::Value },
}
pub enum SortDir { Asc, Desc }
pub struct SortSpec { pub field: String, pub dir: SortDir }
pub enum Page { Offset { page: u32, limit: u32 }, Cursor { after: Option<String>, limit: u32 } }

/// Requête typée : `T` est le type d'entité (marqueur), pour que le front écrive
/// `Query::<Invoice>::new().filter(..)` et que le serveur vérifie contre `T::entity_desc()`.
pub struct Query<T> {
    pub filter: Option<FilterExpr>,
    pub sort: Vec<SortSpec>,
    pub page: Page,
    #[serde(skip)] _entity: PhantomData<fn() -> T>,
}

/// Lien entre un type Rust et son descripteur (implémenté par les macros d'entité).
pub trait Described { fn entity_desc() -> EntityDesc; }

// ---------- custom.rs ----------
pub struct Custom { pub name: String, #[serde(default)] pub params: serde_json::Value }
/// Registre des extensions connues ; un `Custom` absent du registre fait échouer `check()`.
pub struct CustomRegistry { /* noms connus par catégorie */ }

// ---------- lifecycle.rs ----------
pub enum ModuleState {
    Discovered, Resolving, Resolved, Loading, Loaded, Activating, Active,
    Draining, Unloaded, Failed { code: ErrorCode, at: String },
}
```

**Moteurs purs** (fichiers `engine/*.rs` du crate, signatures figées par la fondation, implémentées par les flux) :

```rust
// engine/validate.rs      (flux validation)
pub fn apply(entity: &EntityDesc, op: WriteOp, payload: serde_json::Value)
    -> Result<serde_json::Value, ContractError>;          // transformations PUIS validateurs ;
                                                          // Create : champ absent + Required → violation
pub enum WriteOp { Create, Update }
// engine/query.rs         (flux validation)
pub fn check_query<T>(entity: &EntityDesc, q: &Query<T>) -> Result<(), ContractError>;
pub fn parse_legacy(entity: &EntityDesc, filter: Option<&str>, sort: Option<&str>, page: usize, limit: usize)
    -> Result<Query<()>, ContractError>;                  // JSON malformé → this.query.malformed_filter
pub fn matches(expr: &FilterExpr, row: &serde_json::Value) -> bool;
// engine/policy.rs        (flux auth)
pub fn evaluate(expr: &PolicyExpr, who: &PrincipalView, resolve: &dyn Fn(&str) -> Option<bool>) -> bool;
// engine/i18n.rs          (flux errors-i18n)
pub fn negotiate(accept: &str, available: &[Locale], default: &Locale) -> Locale;
pub fn render(set: &[&I18nCatalog], locale: &Locale, key: &MessageKey, params: &BTreeMap<String, Value>) -> String;
pub fn localize(err: ContractError, set: &[&I18nCatalog], locale: &Locale) -> ContractError;
// derive/{graphql.rs, proto.rs, json_schema.rs, typescript.rs}   (flux schema)
pub fn graphql_sdl(set: &DescriptorSet) -> String;
pub fn proto_file(set: &DescriptorSet, package: &str) -> String;
pub fn json_schema(entity: &EntityDesc, op: WriteOp) -> serde_json::Value;
```

### 3.3 Champs de base et tags proto stables

Les champs engendrés par `impl_data_entity!` sont implicites et ont des tags **réservés** :
`id=1, type=2, created_at=3, updated_at=4, deleted_at=5, status=6, name=7, tenant_id=8` (9..=15 réservés au
framework). Les champs déclarés commencent à **16**. Règles vérifiées par `check()` : tag unique, `>= 16`,
absent de `reserved_tags`. La macro attribue les tags par **ordre de déclaration** (16, 17, …) et accepte un tag
explicite ; la CLI tient `descriptor.lock` (nom → tag) et `this descriptor check` refuse une réutilisation ou un
réordonnancement qui changerait un tag publié. Le `.proto` dérivé émet `reserved` pour `reserved_tags`.

### 3.4 Dérivations par protocole

| Surface | Dérivation | Fichier |
|---|---|---|
| REST | `Validated<T>` → `engine::validate::apply` ; `QueryParams` → `engine::query::parse_legacy` ; erreur → `ApiError(ContractError)` (`IntoResponse`) | `core/validation/extractor.rs`, `core/query.rs`, `core/errors/http.rs` |
| GraphQL | SDL depuis `derive::graphql` (repli : sample) ; mutations validées par `ValidatingCreator` ; garde `authorize_op` par résolveur ; `ContractError` → `async_graphql::Error` + `extensions` | `graphql/schema_generator.rs`, `graphql/executor/*`, `core/errors/graphql.rs` |
| gRPC | `.proto` depuis `derive::proto` (tags stables) ; identité depuis les métadonnées ; `ContractError` → `Status` + `details` (enveloppe JSON) | `grpc/proto_generator.rs`, `grpc/identity.rs`, `core/errors/grpc.rs` |
| WebSocket | mêmes gardes que GraphQL pour les abonnements | `websocket/dispatcher.rs` (intégration) |
| Front Leptos | dépend de `this-descriptor` en wasm32 : `apply`, `check_query`, `render`, `ContractError` ; `impl FromServerFnError` dans une glue hors périmètre de ce lot | (suivi) |
| CLI | le contexte Tera **est** le `ModuleDescriptor` ; `this descriptor check/export` | `this-cli/src/descriptor/*` |

Corps HTTP d'erreur (rétrocompatible) :

```json
{ "error": "Validation failed", "errors": ["amount doit être positif"],
  "code": "this.validation.failed", "kind": "validation", "message_key": "this.validation.failed",
  "fields": [{ "path": "amount", "code": "this.validation.positive", "message_key": "this.validation.positive", "message": "amount doit être positif", "params": {} }] }
```

Transport inter-couches : `ContractError: std::error::Error`, il voyage **dans `anyhow::Error`** (les traits
`EntityCreator`/`EntityFetcher` restent inchangés). Chaque surface fait
`err.downcast_ref::<ContractError>()` et ne retombe sur `Internal` que si la descente échoue
(`core/errors/mod.rs::from_anyhow`). C'est ce qui remplace `map_err(|e| Status::internal(..))`.

**Piège d'orphelinat** : `this-rs` ne peut pas `impl IntoResponse for this_descriptor::ContractError` (trait et
type étrangers). D'où le newtype `this::core::errors::ApiError(pub ContractError)`.

### 3.5 Validation multi-protocole

Point d'étranglement unique : toute création/mise à jour GraphQL et gRPC passe par
`host.entity_creators` (mesuré : `grpc/entity_service.rs`, `graphql/dynamic_schema.rs`,
`graphql/executor/mutation_executor.rs`, `graphql/executor/link_mutations.rs`). Le flux validation enveloppe
chaque créateur dont l'entité a un descripteur dans `ValidatingCreator` à la construction de l'hôte
(`server/host.rs`). REST passe par `Validated<T>` réécrit sur le moteur. Champ `read_only` dans un payload →
`this.validation.read_only` ; clé inconnue → `this.validation.unknown_field` (fail-closed, désactivable par
entité via `Custom{name:"this.allow_unknown_fields"}` pour l'adoption).

### 3.6 Auth multi-protocole

- `PermDesc` → `EntityAuthConfig` (conversion) : le YAML et le descripteur produisent la **même**
  `AuthPolicy`. Si les deux déclarent une entité, ils doivent être égaux, sinon erreur au démarrage.
- `core/auth/surface.rs` : `authorize_op(host, entity, &Operation, &AuthContext, owner: Option<Uuid>)
  -> Result<(), ContractError>` — même résolution que le REST (registre de résolveurs, fail-closed).
- gRPC : `grpc/identity.rs` convertit `tonic::metadata::MetadataMap` en `HeaderMap` puis appelle
  `AuthProvider::extract_context` ; pas d'en-tête → `AuthContext::Anonymous`.
- Une surface dont **toutes** les opérations passent par `authorize_op` n'appelle plus
  `refuse_unenforced_surface` ; les `*_unenforced` restent pour compatibilité.

### 3.7 Modules au runtime

Rust n'a pas d'ABI stable : charger du **code** arbitraire (dylib) est rejeté (§4). Le runtime charge :
- (a) des modules **compilés** (`Arc<dyn Module>`) enregistrés dans un catalogue et activés/désactivés à chaud ;
- (b) des modules **déclaratifs** lus d'un `ModuleStore` (VFS) : descripteur + catalogues i18n + actifs, dont
  les entités sont servies par un service JSON générique (`JsonEntityService`, in-memory ou obrain).

```rust
// src/core/module_runtime/loader.rs   (this-rs)
pub enum ModuleSource { Compiled(Arc<dyn Module>), Stored { store: Arc<ModuleStore>, name: String, version: Option<String> } }

#[async_trait]
pub trait ModuleLoader: Send + Sync {
    /// Lit et vérifie le descripteur, construit l'interface. Ne l'active pas.
    async fn load(&self, source: &ModuleSource) -> Result<ModuleInterface, ContractError>;
}

/// Ce qu'un module chargé expose à l'hôte.
pub struct ModuleInterface {
    pub descriptor: Arc<ModuleDescriptor>,
    pub fetchers: HashMap<String, Arc<dyn EntityFetcher>>,
    pub creators: HashMap<String, Arc<dyn EntityCreator>>,
    pub routes: Option<axum::Router>,
}

// src/core/module_runtime/runtime.rs
pub struct ModuleRuntime { /* catalogue, états, DescriptorSet partagé, DynamicRouter */ }
impl ModuleRuntime {
    pub async fn load(&self, src: ModuleSource) -> Result<ModuleStatus, ContractError>;   // résout, charge, active
    pub async fn unload(&self, name: &str) -> Result<ModuleStatus, ContractError>;        // refus si dépendants actifs
    pub fn interface(&self, name: &str) -> Option<Arc<ModuleInterface>>;
    pub fn status(&self) -> Vec<ModuleStatus>;                                            // { name, version, state, requires }
    pub fn subscribe(&self) -> tokio::sync::broadcast::Receiver<ModuleStatus>;
}
```

Résolution : tri topologique de `requires` (semver), cycle → `this.module.cycle`, version → `this.module.dependency_version`,
manquante non optionnelle → `this.module.dependency_missing`, entité déjà fournie → `this.module.duplicate_entity`.
`ServerBuilder::register_module` passe par la même résolution ; `LinksConfig::merge_strict` remplace le
last-wins pour les modules à descripteur (le `merge` historique reste pour les autres).
Routage à chaud : `DynamicRouter` (service tower monté une fois en `fallback`, `RwLock<Router>` reconstruit à
chaque activation/désactivation). Déchargement : `Active → Draining` (nouvelles requêtes refusées avec
`this.module.unavailable`, kind `Unavailable`, donc 503 / `UNAVAILABLE`), attente des requêtes en vol, puis `Unloaded`.

### 3.8 `ModuleStore` / VFS

```rust
// src/storage/vfs/mod.rs   (this-rs)
pub struct VfsPath(String);              // absolu, normalisé ; refuse "..", "\\", NUL, segments vides
pub struct VfsMeta { pub size: u64, pub etag: String /* sha256 hex */, pub modified: DateTime<Utc> }
pub enum VfsEntry { File { path: VfsPath, meta: VfsMeta }, Dir { path: VfsPath } }
pub enum Precondition { None, IfMatch(String), IfNoneMatch }
pub struct VfsCaps { pub read_only: bool, pub versioned: bool }
#[derive(thiserror::Error)] pub enum VfsError { NotFound(VfsPath), PreconditionFailed { path: VfsPath, current: Option<String> }, ReadOnly, InvalidPath(String), Backend(anyhow::Error) }
impl From<VfsError> for ContractError { /* this.vfs.* */ }

#[async_trait]
pub trait Vfs: Send + Sync {
    async fn read(&self, path: &VfsPath) -> Result<Vec<u8>, VfsError>;
    async fn write(&self, path: &VfsPath, bytes: &[u8], pre: Precondition) -> Result<VfsMeta, VfsError>;
    async fn stat(&self, path: &VfsPath) -> Result<Option<VfsMeta>, VfsError>;
    async fn list(&self, dir: &VfsPath) -> Result<Vec<VfsEntry>, VfsError>;
    async fn delete(&self, path: &VfsPath, pre: Precondition) -> Result<(), VfsError>;
    fn caps(&self) -> VfsCaps;
}

/// Façade module au-dessus d'un VFS. Disposition :
/// /modules/<name>/<version>/descriptor.json, /modules/<name>/<version>/i18n/<locale>.json, …/assets/**
pub struct ModuleStore { vfs: Arc<dyn Vfs> }
impl ModuleStore {
    pub fn new(vfs: Arc<dyn Vfs>) -> Self;
    pub async fn put(&self, d: &ModuleDescriptor) -> Result<VfsMeta, ContractError>;      // check() avant écriture
    pub async fn get(&self, name: &str, version: Option<&str>) -> Result<ModuleDescriptor, ContractError>; // None = plus haute
    pub async fn list(&self) -> Result<Vec<(String, String)>, ContractError>;
    pub async fn remove(&self, name: &str, version: &str) -> Result<(), ContractError>;
}
```

Implémentations : `InMemoryVfs` (toujours), `LocalFsVfs` (racine bac à sable, écriture atomique
temp+rename), `ObrainVfs` (`#[cfg(feature = "obrain")]`, nœuds `ThisVfsEntry {path, etag, size, modified, bytes}`
dans un espace dédié, `IfMatch` vérifié dans la même transaction, respecte `check_write()` du bail d'écriture).
Le stockage VFS est **optionnel** : sans `ModuleStore`, seuls les modules compilés existent.

## 4. Alternatives rejetées

| Alternative | Pourquoi rejetée |
|---|---|
| Module `this::descriptor` interne à `this-rs` | pas compilable en wasm32, impose tout le framework à la CLI |
| Dépôt séparé pour le descripteur | contrat et usages divergeraient ; deux PR pour un changement |
| Proc-macro `#[derive(Entity)]` (nouveau crate proc-macro) | changement de syntaxe pour tous les projets ; les `macro_rules!` existants peuvent émettre `Described` de façon additive. Gardé comme évolution |
| Inférer depuis le sample JSON en mieux | indécidable pour les `Option`, les enums, les tags ; c'est la cause du défaut |
| Générer le descripteur depuis `.proto` ou SDL (schéma-d'abord) | réintroduit une seconde source ; ne porte ni validateurs, ni i18n, ni permissions |
| Crate `validator` (derive) | ne se sérialise pas, n'existe pas côté descripteur dynamique ; il sera **retiré** des dépendances |
| Codes d'erreur = `u32` | illisibles, collisions entre modules ; chaîne à espace de noms |
| Chargement dynamique de code (dylib, `abi_stable`) | ABI Rust instable, sûreté mémoire non garantie ; plugins wasm (component model) notés comme suite |
| Garder `LinksConfig::merge` last-wins | contraire au fail-closed : un module masque en silence l'entité d'un autre |
| VFS = `std::fs` direct | non testable, pas d'obrain, pas de précondition d'écriture |

## 5. Impact fichier par fichier

Dépôt `this` :

| Fichier | Changement | Flux |
|---|---|---|
| `Cargo.toml` | `[workspace]`, dépendance `this-descriptor` ; retrait de `validator` | fondation (retrait : validation) |
| `crates/this-descriptor/**` | nouveau crate : types, `check()`, table d'erreurs, stubs des moteurs | fondation |
| `crates/this-descriptor/src/engine/validate.rs`, `engine/query.rs` | moteurs | validation |
| `crates/this-descriptor/src/engine/i18n.rs` | négociation, rendu, `localize` | errors-i18n |
| `crates/this-descriptor/src/engine/policy.rs` | `evaluate`, `FromStr` de `PolicyExpr` | auth |
| `crates/this-descriptor/src/derive/*.rs` | SDL, proto, JSON Schema, TypeScript | schema |
| `src/lib.rs` | `pub use this_descriptor as descriptor;` + prélude | fondation |
| `src/core/module.rs` | `fn descriptor(&self) -> Option<ModuleDescriptor> { None }` | fondation |
| `src/server/host.rs` | champ `descriptors: Arc<RwLock<DescriptorSet>>` (fondation) ; enveloppe `ValidatingCreator` (validation) | fondation, validation |
| `src/server/builder.rs` | collecte des descripteurs (fondation) ; résolution de dépendances, `ModuleRuntime`, `DynamicRouter` (modules-runtime) | fondation, modules-runtime |
| `src/core/validation/{gate.rs (nouveau), extractor.rs, config.rs}` | `ValidatingCreator`, `Validated<T>` sur moteur | validation |
| `src/core/query.rs` | `QueryParams::to_query(&EntityDesc)` | validation |
| `src/core/errors/{mod,http,grpc,graphql}.rs` (nouveau) | `ApiError`, `from_anyhow`, conversions | errors-i18n |
| `src/core/i18n/{mod,extract}.rs` (nouveau) | extracteur `Locale` (Accept-Language, métadonnée gRPC `accept-language`) | errors-i18n |
| `src/core/auth/{descriptor_policy.rs, surface.rs}` (nouveaux) | `PermDesc` ↔ `AuthPolicy`, `authorize_op` | auth |
| `src/server/exposure/grpc/{identity.rs (nouveau), entity_service.rs, link_service.rs, mod.rs}` | identité gRPC, gardes | auth |
| `src/server/exposure/graphql/{executor/*.rs, dynamic_schema.rs, mod.rs}`, `exposure/mod.rs` | gardes par résolveur, fin du refus si couvert | auth |
| `src/server/exposure/graphql/schema_generator.rs`, `grpc/proto_generator.rs` | descripteur d'abord, sample en repli | schema |
| `src/entities/macros.rs` | `impl Described` émis par `impl_data_entity!` / `_validated!` (grammaire → `ValidatorSpec`) | schema |
| `src/server/entity_registry.rs` | `register` refuse les doublons (`try_register`), `register` historique conservé | modules-runtime |
| `src/config/mod.rs` | `LinksConfig::merge_strict`, `from_descriptor` | modules-runtime |
| `src/core/module_runtime/{mod,loader,runtime,resolve,dynamic_router,json_entity}.rs` (nouveau) | runtime | modules-runtime |
| `src/storage/vfs/{mod,memory,fs,obrain,module_store}.rs` (nouveau), `src/storage/mod.rs` (une ligne) | VFS | vfs (`mod.rs` stub : fondation) |
| `tests/module_descriptor_e2e.rs`, `examples/descriptor_api/` | preuve multi-protocole | intégration |
| erreurs gRPC/GraphQL restantes (`map_err(Status::internal)`) | `from_anyhow` | intégration |

Dépôt `this-cli` :

| Fichier | Changement |
|---|---|
| `Cargo.toml` | `this-descriptor` (chemin) |
| `src/descriptor/{mod,io,from_fields,lock}.rs` (nouveau) | lire/écrire `descriptor/module.yaml`, `descriptor.lock` |
| `src/commands/add_entity.rs` | écrit l'`EntityDesc` ; contexte Tera = descripteur |
| `src/commands/add_link.rs` | `#[serde(flatten)] extra` sur toutes les copies (fin de la perte de clés) + `LinkDesc` |
| `src/codegen/introspect.rs` | lit le descripteur s'il existe (regex en repli) |
| `src/commands/doctor.rs` | dérive descripteur ↔ `links.yaml` ↔ code, tags |
| `src/commands/{mod.rs, descriptor.rs (nouveau)}`, `src/main.rs` | `this descriptor check|export|lock` |
| `src/templates/entity/*.tera`, `src/templates/project/module.rs.tera` | consomment le descripteur ; `fn descriptor()` engendrée |

## 6. Plan de migration additif

1. **Fondation** (seule, d'abord) : crate + contrats + `Module::descriptor()` à défaut `None` + champ hôte vide.
   Aucun comportement ne change ; toute la suite de tests existante passe.
2. **Sept flux en parallèle**, chacun dans son worktree `/Users/triviere/projects/this-rs/_wt-md/<flux>` sur
   `md/<flux>` issue de `integ/module-descriptor` après fusion de la fondation. Chaque flux n'active son
   comportement **que si un descripteur existe** pour l'entité concernée.
3. **Intégration** : branchements transverses (§5, lignes « intégration »), exemple + test e2e
   multi-protocole, migration des `Status::internal` restants.
4. **Dépréciations** (lot suivant, pas ici) : `EntityDescriptor` sans champs → `#[deprecated]` ;
   `*_unenforced` → avertissement ; inférence par sample → journal `warn` quand un descripteur manque.
5. Rien n'est retiré dans ce lot, sauf la dépendance morte `validator`.

## 7. Découpage en flux

Flux 0 — **fondation** (précède tout) :
- Fichiers : `Cargo.toml`, `crates/this-descriptor/**` (dont stubs `engine/*.rs`, `derive/*.rs` qui rendent
  `ContractError{kind: NotImplemented}`), `src/lib.rs`, `src/core/module.rs`, `src/server/host.rs` (champ),
  `src/server/builder.rs` (collecte), `src/storage/mod.rs` + `src/storage/vfs/mod.rs` (trait `Vfs`, `VfsPath`,
  `VfsError`, `ModuleStore` signatures), `src/core/module_runtime/mod.rs` + `loader.rs` (trait `ModuleLoader`,
  `ModuleInterface`, `ModuleSource`), `src/core/mod.rs` (déclarations `errors`, `i18n`, `module_runtime`).
- Contrats produits : tous les types de §3.2, la table §3.4 (implémentée et testée), `check()`.
- Acceptation : `cargo test -p this-descriptor` (tests nommés `error_kind_mapping_is_total`,
  `check_rejects_duplicate_proto_tag`, `check_rejects_tag_below_16`, `check_rejects_unknown_custom`,
  `descriptor_json_roundtrip`, `descriptor_yaml_roundtrip`) ; `cargo check -p this-descriptor --target
  wasm32-unknown-unknown` ; `cargo test -p this-rs --lib` inchangé.

| Flux | Fichiers (disjoints) | Contrats consommés | Tests d'acceptation (noms visibles dans la sortie) |
|---|---|---|---|
| **validation** | `engine/validate.rs`, `engine/query.rs`, `src/core/validation/{gate.rs,extractor.rs,config.rs}`, `src/core/query.rs`, `src/server/host.rs` (enveloppe), `Cargo.toml` (retrait `validator`) | `EntityDesc`, `FieldDesc`, `ValidatorSpec`, `TransformSpec`, `Query`, `ContractError`, `ErrorKind::http_status` | `required_detects_absent_field_on_create`, `update_validates_only_present_fields`, `transforms_run_before_validators`, `read_only_field_rejected`, `unknown_field_rejected`, `malformed_filter_is_error_not_ignored`, `sort_on_non_sortable_rejected`, `validating_creator_blocks_invalid_graphql_payload` (via `EntityCreator`), `legacy_validated_body_keeps_error_and_errors_keys` |
| **errors-i18n** | `engine/i18n.rs`, `src/core/errors/**`, `src/core/i18n/**` | `ContractError`, `ErrorKind`, `MessageKey`, `I18nCatalog`, `Locale` | `negotiate_picks_best_quality`, `negotiate_falls_back_to_default`, `render_substitutes_params`, `missing_key_renders_key_not_panic`, `api_error_http_body_shape`, `contract_error_to_grpc_status_code_and_details`, `contract_error_to_graphql_extensions`, `from_anyhow_downcasts_contract_error`, `from_anyhow_unknown_is_internal`, `locale_extractor_reads_accept_language` |
| **auth** | `engine/policy.rs`, `src/core/auth/{descriptor_policy.rs,surface.rs}`, `src/server/exposure/grpc/{identity.rs,entity_service.rs,link_service.rs,mod.rs}`, `src/server/exposure/graphql/{executor/*.rs,dynamic_schema.rs,mod.rs}`, `src/server/exposure/mod.rs` | `PermDesc`, `PolicyExpr`, `Operation`, `PrincipalView`, `ContractError` | `policy_expr_parses_existing_yaml_grammar` (table des chaînes de `parse_policy`), `perm_desc_to_auth_policy_equivalent`, `unknown_resolver_denies`, `grpc_identity_from_metadata`, `grpc_create_denied_without_role`, `graphql_mutation_denied_without_role`, `graphql_served_with_armed_policies_when_covered` |
| **schema** | `derive/{graphql,proto,json_schema,typescript}.rs`, `src/server/exposure/graphql/schema_generator.rs`, `src/server/exposure/grpc/proto_generator.rs`, `src/entities/macros.rs` | `DescriptorSet`, `EntityDesc`, `FieldType`, `Described` | `proto_tags_stable_across_field_reorder_in_json`, `proto_emits_reserved_tags`, `sdl_marks_optional_nullable`, `sdl_from_descriptor_without_sample`, `json_schema_required_on_create_only`, `macro_emits_described_with_validators`, `macro_tags_start_at_16`, `generators_fall_back_to_sample_without_descriptor` |
| **modules-runtime** | `src/core/module_runtime/{runtime,resolve,dynamic_router,json_entity}.rs`, `src/server/builder.rs`, `src/server/entity_registry.rs`, `src/config/mod.rs` | `ModuleDescriptor`, `ModuleReq`, `ModuleState`, `ModuleLoader`, `ModuleInterface`, `ModuleStore` (signature) | `resolve_orders_by_dependencies`, `resolve_detects_cycle`, `resolve_rejects_version_mismatch`, `optional_dependency_skipped`, `duplicate_entity_across_modules_fails`, `unload_refused_with_active_dependents`, `load_then_routes_served_unload_then_404`, `lifecycle_transitions_are_legal_only`, `merge_strict_reports_conflicts` |
| **vfs** | `src/storage/vfs/{memory.rs,fs.rs,obrain.rs,module_store.rs}` (+ corps de `mod.rs` après stub) | `Vfs`, `VfsPath`, `ModuleStore`, `ContractError`, `ModuleDescriptor::check` | banc partagé `vfs_conformance_*` exécuté sur chaque backend : `..._read_write_roundtrip`, `..._if_match_conflict`, `..._if_none_match_create_only`, `..._list_dir`, `..._delete_missing_not_found`; `vfs_path_rejects_dotdot`, `fs_vfs_sandboxed`, `module_store_put_rejects_invalid_descriptor`, `module_store_get_highest_version`, `obrain_vfs_conformance` (feature `obrain`) |
| **cli** (dépôt `this-cli`) | voir §5 tableau `this-cli` | `ModuleDescriptor`, `EntityDesc`, `LinkDesc`, `check()`, `derive::*` (lecture) | `add_link_preserves_unknown_yaml_keys`, `add_entity_writes_descriptor`, `descriptor_lock_detects_tag_reuse`, `doctor_reports_plural_drift`, `templates_render_from_descriptor`, `descriptor_check_cli_exit_code` |

Puis **intégration** (dépend des 7) et **revue finale & référencement** (dépend de l'intégration).

Acceptation de l'intégration : `tests/module_descriptor_e2e.rs` — un même module déclaré une fois ;
le même payload invalide envoyé en REST, GraphQL et gRPC rend **le même `code`** (`this.validation.positive`),
le bon statut par protocole (422 / `INVALID_ARGUMENT` / `BAD_USER_INPUT`) et un message dans la locale
demandée (`fr` puis `en`) ; une requête refusée par politique est refusée sur les trois ; un module chargé à
chaud depuis un `InMemoryVfs` sert ses routes puis 404 après déchargement.

## 8. Risques et pièges connus

- **Chemins relatifs des dépendances locales** : `Cargo.toml` référence `../../banc-obrain-j4/…` et
  `../../vite-rs`. Depuis un worktree `/Users/triviere/projects/this-rs/_wt-md/<flux>`, ces chemins pointent vers
  `/Users/triviere/projects/this-rs/banc-obrain-j4` et `…/this-rs/vite-rs`, qui **n'existent pas** (le checkout
  principal est un niveau plus haut). Cargo doit lire les manifestes des dépendances `path` même optionnelles
  (c'est pourquoi la CI crée des stubs : `.github/create-local-dep-stubs.sh`). Remède à décider par
  l'orchestrateur : liens symboliques `this-rs/banc-obrain-j4 → ../banc-obrain-j4` et `this-rs/vite-rs → ../vite-rs`
  (hors worktree), ou worktrees à la même profondeur que le checkout. Même piège pour `this-cli` →
  `../this/crates/this-descriptor` : depuis `_wt-md/cli`, il faut `_wt-md/this → <worktree d'intégration>`.
- **Orphelinat** : conversions vers axum/tonic/async-graphql par newtype `ApiError` (§3.4).
- **Tags proto** : la seule garantie de stabilité est `descriptor.lock` + `reserved_tags` ; un réordonnancement
  dans la macro change les tags — la CLI et un test de la macro doivent l'attraper.
- **Fail-closed sur clés inconnues** peut casser l'adoption d'un projet existant : actif seulement quand un
  descripteur est fourni, et désactivable par entité.
- **`ValidatingCreator`** double la validation REST si un handler REST appelle aussi un créateur : idempotent
  (transformations idempotentes : trim, casse, arrondi), coût négligeable.
