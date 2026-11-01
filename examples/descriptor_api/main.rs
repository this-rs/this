//! A complete described module, declared ONCE and served on REST, GraphQL and gRPC.
//!
//! The `billing` module declares in a single `ModuleDescriptor`:
//! - an entity `invoice` with validators (`number`: 3..50 characters, `amount`: > 0),
//!   input transforms (trim + uppercase on `number`) and typed query capabilities;
//! - a typed filter: `amount` may be compared with `eq`/`gt`, `number` with `eq`, and both
//!   are sortable; any other field or operator is refused with a stable code;
//! - an entity `customer` and the link `owner` (invoice -> customer);
//! - a policy: creating an invoice needs `role:admin`, the rest only an identity;
//! - messages in French and English (a module bundle on top of the framework catalog).
//!
//! The same host then answers the same questions on the three protocols: an invalid
//! payload gets the same contract code, a forbidden caller the same refusal, a filter the
//! same rows, and the message comes back in the caller's language.
//!
//! Run it: `cargo run --example descriptor_api --features graphql,grpc`
//! Test it: `cargo test --example descriptor_api --features graphql,grpc`

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use axum::Router;
use axum::body::Body;
use axum::http::{HeaderMap, Method, Request, StatusCode};
use serde_json::{Value, json};
use tower::ServiceExt;
use uuid::Uuid;

use this::config::{EntityAuthConfig, EntityConfig, LinksConfig};
use this::core::auth::{AuthContext, AuthProvider};
use this::core::errors::grpc::contract_error_of;
use this::core::query::ListQuery;
use this::core::{EntityCreator, EntityFetcher};
use this::descriptor::{
    Applies, DescriptorSet, EntityDesc, FieldDesc, FieldType, FilterOp, LinkDesc, Locale,
    MessageBundle, MessageKey, MessageSyntax, ModuleDescriptor, Operation, PolicyExpr,
    TransformSpec, ValidatorSpec,
};
use this::server::entity_registry::{EntityDescriptor, EntityRegistry};
use this::server::exposure::GraphQLExposure;
use this::server::exposure::grpc::entity_service::{
    EntityServiceImpl, FILTER_METADATA, SORT_METADATA,
};
use this::server::exposure::grpc::proto::entity_service_server::EntityService;
use this::server::exposure::grpc::proto::{CreateEntityRequest, ListEntitiesRequest};
use this::server::exposure::rest::RestExposure;
use this::server::host::ServerHost;
use this::storage::InMemoryLinkService;

// ── the descriptor: the ONLY place the module is declared ───────────────────

/// `billing`, declared once. Every surface below derives from this value.
pub fn billing() -> ModuleDescriptor {
    let mut invoice = EntityDesc::new("invoice");
    invoice.fields = vec![
        FieldDesc::new("number", FieldType::String)
            .transform(TransformSpec::Trim, Applies::Always)
            .transform(TransformSpec::Uppercase, Applies::Always)
            .validator(
                ValidatorSpec::StringLength { min: 3, max: 50 },
                Applies::Always,
            )
            .filterable(&[FilterOp::Eq])
            .sortable(),
        FieldDesc::new("amount", FieldType::F64)
            .validator(ValidatorSpec::Positive, Applies::Always)
            .filterable(&[FilterOp::Gt, FilterOp::Eq])
            .sortable(),
        // Not filterable, not sortable: a query on it is refused.
        FieldDesc::new("note", FieldType::String).optional(),
    ];
    // Policy: everything needs an identity, creation needs the `admin` role.
    for op in Operation::CRUD {
        invoice
            .permissions
            .ops
            .insert(op, PolicyExpr::Authenticated);
    }
    invoice
        .permissions
        .ops
        .insert(Operation::Create, PolicyExpr::HasRole(vec!["admin".into()]));

    let mut customer = EntityDesc::new("customer");
    customer.fields = vec![FieldDesc::new("name", FieldType::String)];

    let mut m = ModuleDescriptor::new("billing", "1.0.0");
    m.entities.push(invoice);
    m.entities.push(customer);
    m.links.push(LinkDesc::new("owner", "invoice", "customer"));
    m.i18n.locales = vec![Locale::new("fr"), Locale::new("en")];
    m.normalize();
    m
}

/// The module's own texts, fr and en. Validation messages come from the framework catalog.
pub fn bundles() -> Vec<MessageBundle> {
    [("fr", "Facture"), ("en", "Invoice")]
        .into_iter()
        .map(|(locale, text)| MessageBundle {
            module: "billing".into(),
            locale: Locale::new(locale),
            syntax: MessageSyntax::Simple,
            messages: BTreeMap::from([(
                MessageKey::new("billing.invoice.title"),
                text.to_string(),
            )]),
        })
        .collect()
}

// ── storage and routes (what the application writes by hand) ────────────────

/// In-memory store; counts the writes that reach it (a refused call never does).
#[derive(Default)]
pub struct Store {
    rows: Mutex<Vec<Value>>,
    writes: Mutex<usize>,
}

impl Store {
    pub fn writes(&self) -> usize {
        *self.writes.lock().unwrap()
    }
}

#[async_trait]
impl EntityCreator for Store {
    async fn create_from_json(&self, mut v: Value) -> anyhow::Result<Value> {
        *self.writes.lock().unwrap() += 1;
        v["id"] = json!(Uuid::new_v4().to_string());
        self.rows.lock().unwrap().push(v.clone());
        Ok(v)
    }
}

#[async_trait]
impl EntityFetcher for Store {
    async fn fetch_as_json(&self, id: &Uuid) -> anyhow::Result<Value> {
        self.rows
            .lock()
            .unwrap()
            .iter()
            .find(|r| r["id"] == json!(id.to_string()))
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("not found"))
    }
    async fn list_as_json(&self, _: Option<i32>, _: Option<i32>) -> anyhow::Result<Vec<Value>> {
        Ok(self.rows.lock().unwrap().clone())
    }
    // `list_query` keeps its default (bounded in-memory filter/sort/page). A SQL or graph
    // backend would override it to push the query down.
}

/// REST routes, as the CLI generates them: the payload straight to the store. Validation,
/// policy, localization and authentication are layers in FRONT of these handlers.
struct Routes(Arc<Store>);

impl EntityDescriptor for Routes {
    fn entity_type(&self) -> &str {
        "invoice"
    }
    fn plural(&self) -> &str {
        "invoices"
    }
    fn build_routes(&self) -> Router {
        let (creator, lister) = (self.0.clone(), self.0.clone());
        Router::new().route(
            "/invoices",
            axum::routing::post(move |axum::Json(v): axum::Json<Value>| {
                let store = creator.clone();
                async move {
                    let created = store.create_from_json(v).await.unwrap();
                    (StatusCode::CREATED, axum::Json(created))
                }
            })
            // The query already checked against the descriptor (`?filter=&sort=`).
            .get(move |q: ListQuery| {
                let store = lister.clone();
                async move {
                    let (rows, total) = store.list_query(&q.0).await.unwrap();
                    axum::Json(json!({"data": rows, "total": total}))
                }
            }),
        )
    }
}

/// `Bearer admin` -> role `admin`; `Bearer user` -> no role; anything else is refused.
struct Tokens;

#[async_trait]
impl AuthProvider for Tokens {
    async fn extract_context(&self, h: &HeaderMap) -> anyhow::Result<AuthContext> {
        let roles = match h.get("authorization").and_then(|v| v.to_str().ok()) {
            Some("Bearer admin") => vec!["admin".to_string()],
            Some("Bearer user") => vec![],
            _ => anyhow::bail!("no or bad token"),
        };
        Ok(AuthContext::User {
            user_id: Uuid::from_u128(1),
            tenant_id: Uuid::from_u128(2),
            roles,
            scopes: Vec::new(),
        })
    }
    async fn is_owner(&self, _: &Uuid, _: &Uuid, _: &str) -> anyhow::Result<bool> {
        Ok(false)
    }
    async fn has_role(&self, _: &Uuid, _: &str) -> anyhow::Result<bool> {
        Ok(false)
    }
}

// ── the host ────────────────────────────────────────────────────────────────

pub struct Demo {
    pub host: Arc<ServerHost>,
    pub store: Arc<Store>,
    pub rest: Router,
    pub graphql: Router,
}

pub fn demo() -> Demo {
    let store = Arc::new(Store::default());
    let config = LinksConfig {
        entities: vec![EntityConfig {
            singular: "invoice".into(),
            plural: "invoices".into(),
            auth: EntityAuthConfig::default(),
        }],
        links: vec![],
        validation_rules: None,
        events: None,
        sinks: None,
        auth: Some(serde_yaml::from_str("default_policy: authenticated").unwrap()),
    };
    let mut registry = EntityRegistry::new();
    registry.register(Box::new(Routes(store.clone())));
    let mut fetchers: HashMap<String, Arc<dyn EntityFetcher>> = HashMap::new();
    fetchers.insert("invoice".into(), store.clone());
    let mut creators: HashMap<String, Arc<dyn EntityCreator>> = HashMap::new();
    creators.insert("invoice".into(), store.clone());

    let mut set = DescriptorSet::new();
    set.insert(billing()).expect("the descriptor is coherent");

    let host = ServerHost::from_builder_components(
        Arc::new(InMemoryLinkService::new()),
        config,
        registry,
        fetchers,
        creators,
    )
    .unwrap()
    .with_descriptors(set)
    .with_auth_provider(Arc::new(Tokens));
    host.locales.add_bundles(bundles()).expect("bundles");
    let host = Arc::new(host);
    Demo {
        rest: RestExposure::build_router(host.clone(), vec![]).expect("REST"),
        graphql: GraphQLExposure::build_router(host.clone()).expect("GraphQL"),
        host,
        store,
    }
}

// ── one question, three protocols ───────────────────────────────────────────

pub struct RestReply {
    pub status: StatusCode,
    pub body: Value,
}

impl Demo {
    pub async fn rest(
        &self,
        method: Method,
        uri: &str,
        token: &str,
        lang: &str,
        body: Option<&str>,
    ) -> RestReply {
        let req = Request::builder()
            .method(method)
            .uri(uri)
            .header("content-type", "application/json")
            .header("authorization", format!("Bearer {token}"))
            .header("accept-language", lang)
            .body(Body::from(body.unwrap_or("").to_string()))
            .unwrap();
        let res = self.rest.clone().oneshot(req).await.unwrap();
        let status = res.status();
        let bytes = axum::body::to_bytes(res.into_body(), usize::MAX)
            .await
            .unwrap();
        RestReply {
            status,
            body: serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        }
    }

    /// Raw GraphQL response (`data` and/or `errors`).
    pub async fn graphql(&self, query: &str, token: &str, lang: &str) -> Value {
        let res = self
            .graphql
            .clone()
            .oneshot(
                Request::post("/graphql")
                    .header("content-type", "application/json")
                    .header("authorization", format!("Bearer {token}"))
                    .header("accept-language", lang)
                    .body(Body::from(json!({ "query": query }).to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        let bytes = axum::body::to_bytes(res.into_body(), usize::MAX)
            .await
            .unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }

    pub async fn grpc_create(
        &self,
        token: &str,
        lang: &str,
        data: prost_types::Struct,
    ) -> Result<(), tonic::Status> {
        let mut req = tonic::Request::new(CreateEntityRequest {
            entity_type: "invoice".into(),
            data: Some(data),
        });
        let md = req.metadata_mut();
        md.insert("authorization", format!("Bearer {token}").parse().unwrap());
        md.insert("accept-language", lang.parse().unwrap());
        EntityServiceImpl::new(self.host.clone())
            .create_entity(req)
            .await
            .map(|_| ())
    }

    /// Invoice numbers returned by a typed filter + sort over gRPC.
    pub async fn grpc_numbers(
        &self,
        token: &str,
        filter: &str,
        sort: &str,
    ) -> Result<Vec<String>, tonic::Status> {
        let mut req = tonic::Request::new(ListEntitiesRequest {
            entity_type: "invoice".into(),
            limit: 0,
            offset: 0,
        });
        let md = req.metadata_mut();
        md.insert("authorization", format!("Bearer {token}").parse().unwrap());
        md.insert(FILTER_METADATA, filter.parse().unwrap());
        md.insert(SORT_METADATA, sort.parse().unwrap());
        let res = EntityServiceImpl::new(self.host.clone())
            .list_entities(req)
            .await?
            .into_inner();
        Ok(res
            .entities
            .iter()
            .map(|e| match &e.fields["number"].kind {
                Some(prost_types::value::Kind::StringValue(s)) => s.clone(),
                other => panic!("number: {other:?}"),
            })
            .collect())
    }
}

pub fn grpc_invoice(number: &str, amount: f64) -> prost_types::Struct {
    use prost_types::value::Kind;
    let v = |kind| prost_types::Value { kind: Some(kind) };
    prost_types::Struct {
        fields: [
            ("number".to_string(), v(Kind::StringValue(number.into()))),
            ("amount".to_string(), v(Kind::NumberValue(amount))),
        ]
        .into(),
    }
}

pub fn grpc_code(status: &tonic::Status) -> String {
    contract_error_of(status)
        .expect("contract envelope")
        .code
        .as_str()
        .to_string()
}

#[tokio::main]
async fn main() {
    let d = demo();
    let bad = r#"{"number": "ab", "amount": -1}"#;
    for lang in ["fr", "en"] {
        let r = d
            .rest(Method::POST, "/invoices", "admin", lang, Some(bad))
            .await;
        println!(
            "[{lang}] POST /invoices {} -> {} ({})",
            r.status, r.body["code"], r.body["message"]
        );
    }
    let ok = d
        .rest(
            Method::POST,
            "/invoices",
            "admin",
            "en",
            Some(r#"{"number": "  f-001 ", "amount": 12.5}"#),
        )
        .await;
    println!("created: {}", ok.body);
    let g = d
        .graphql(
            r#"{ invoices(filter: "{\"amount>\": 1}", sort: "-amount") { number } }"#,
            "user",
            "en",
        )
        .await;
    println!("graphql: {g}");
    println!(
        "grpc: {:?}",
        d.grpc_numbers("user", r#"{"amount>": 1}"#, "-amount").await
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    const INVALID: &str = r#"{"number": "ab", "amount": -1}"#;

    fn field_codes(fields: &Value) -> Vec<String> {
        fields
            .as_array()
            .expect("fields")
            .iter()
            .map(|f| f["code"].as_str().unwrap().to_string())
            .collect()
    }

    #[tokio::test]
    async fn descriptor_api_same_invalid_payload_same_code_on_three_protocols() {
        let d = demo();
        let rest = d
            .rest(Method::POST, "/invoices", "admin", "en", Some(INVALID))
            .await;
        assert_eq!(rest.status, StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(rest.body["code"], "this.validation.failed");
        let gql = d
            .graphql(
                r#"mutation { createInvoice(data: {number: "ab", amount: -1}) { id } }"#,
                "admin",
                "en",
            )
            .await;
        assert_eq!(
            gql["errors"][0]["extensions"]["this_code"], "this.validation.failed",
            "{gql}"
        );
        let status = d
            .grpc_create("admin", "en", grpc_invoice("ab", -1.0))
            .await
            .unwrap_err();
        assert_eq!(status.code(), tonic::Code::InvalidArgument);
        assert_eq!(grpc_code(&status), "this.validation.failed");

        let codes = field_codes(&rest.body["fields"]);
        assert!(
            codes.contains(&"this.validation.string_length".to_string()),
            "{codes:?}"
        );
        assert!(
            codes.contains(&"this.validation.positive".to_string()),
            "{codes:?}"
        );
        assert_eq!(
            codes,
            field_codes(&gql["errors"][0]["extensions"]["fields"])
        );
        assert_eq!(
            codes,
            contract_error_of(&status)
                .unwrap()
                .fields
                .iter()
                .map(|f| f.code.as_str().to_string())
                .collect::<Vec<_>>()
        );
        assert_eq!(d.store.writes(), 0, "the store is never reached");
    }

    #[tokio::test]
    async fn descriptor_api_policy_refuses_a_plain_user_everywhere() {
        let d = demo();
        let rest = d
            .rest(
                Method::POST,
                "/invoices",
                "user",
                "en",
                Some(r#"{"number":"F-1","amount":1}"#),
            )
            .await;
        assert_eq!(rest.status, StatusCode::FORBIDDEN, "{}", rest.body);
        let gql = d
            .graphql(
                r#"mutation { createInvoice(data: {number: "F-1", amount: 1}) { id } }"#,
                "user",
                "en",
            )
            .await;
        assert_eq!(
            gql["errors"][0]["extensions"]["this_code"], "this.auth.forbidden",
            "{gql}"
        );
        let status = d
            .grpc_create("user", "en", grpc_invoice("F-1", 1.0))
            .await
            .unwrap_err();
        assert_eq!(status.code(), tonic::Code::PermissionDenied);
        assert_eq!(d.store.writes(), 0);
    }

    #[tokio::test]
    async fn descriptor_api_message_in_french_then_english() {
        let d = demo();
        for (lang, expected) in [("fr", "Validation échouée"), ("en", "Validation failed")] {
            let rest = d
                .rest(Method::POST, "/invoices", "admin", lang, Some(INVALID))
                .await;
            assert_eq!(rest.body["message"], expected, "REST {lang}");
            let gql = d
                .graphql(
                    r#"mutation { createInvoice(data: {number: "ab", amount: -1}) { id } }"#,
                    "admin",
                    lang,
                )
                .await;
            assert_eq!(gql["errors"][0]["message"], expected, "GraphQL {lang}");
            let status = d
                .grpc_create("admin", lang, grpc_invoice("ab", -1.0))
                .await
                .unwrap_err();
            assert_eq!(status.message(), expected, "gRPC {lang}");
        }
        // The module's own bundle, in the host catalog.
        let cat = d.host.locales.load();
        let key = MessageKey::new("billing.invoice.title");
        assert_eq!(cat.lookup(&Locale::new("fr"), &key), Some("Facture"));
        assert_eq!(cat.lookup(&Locale::new("en"), &key), Some("Invoice"));
    }

    #[tokio::test]
    async fn descriptor_api_typed_filter_and_link_on_all_protocols() {
        let d = demo();
        // Transforms: trim + uppercase before validation.
        let r = d
            .rest(
                Method::POST,
                "/invoices",
                "admin",
                "en",
                Some(r#"{"number":"  a-1 ","amount":5}"#),
            )
            .await;
        assert_eq!(
            (r.status, r.body["number"].clone()),
            (StatusCode::CREATED, json!("A-1"))
        );
        for (n, a) in [("B-2", 50.0), ("C-3", 500.0)] {
            d.grpc_create("admin", "en", grpc_invoice(n, a))
                .await
                .unwrap();
        }

        // REST: filter before pagination, then sort.
        let r = d
            .rest(
                Method::GET,
                "/invoices?filter=%7B%22amount%3E%22%3A6%7D&sort=-amount",
                "user",
                "en",
                None,
            )
            .await;
        assert_eq!(r.status, StatusCode::OK, "{}", r.body);
        let numbers: Vec<_> = r.body["data"]
            .as_array()
            .unwrap()
            .iter()
            .map(|x| x["number"].clone())
            .collect();
        assert_eq!(numbers, vec![json!("C-3"), json!("B-2")]);

        // GraphQL: same rows.
        let g = d
            .graphql(
                r#"{ invoices(filter: "{\"amount>\": 6}", sort: "-amount") { number } }"#,
                "user",
                "en",
            )
            .await;
        assert!(g.get("errors").is_none(), "{g}");
        let g_numbers: Vec<_> = g["data"]["invoices"]
            .as_array()
            .unwrap()
            .iter()
            .map(|x| x["number"].clone())
            .collect();
        assert_eq!(g_numbers, numbers);

        // gRPC: same rows.
        assert_eq!(
            d.grpc_numbers("user", r#"{"amount>": 6}"#, "-amount")
                .await
                .unwrap(),
            vec!["C-3", "B-2"]
        );

        // A field the descriptor does not make filterable: a typed error, never an unfiltered list.
        let g = d
            .graphql(
                r#"{ invoices(filter: "{\"note\": \"x\"}") { number } }"#,
                "user",
                "en",
            )
            .await;
        assert_eq!(
            g["errors"][0]["extensions"]["this_code"], "this.query.field_not_filterable",
            "{g}"
        );
        let r = d
            .rest(Method::GET, "/invoices?sort=note", "user", "en", None)
            .await;
        assert_eq!(r.status, StatusCode::BAD_REQUEST, "{}", r.body);
        assert_eq!(r.body["code"], "this.query.field_not_sortable");
        let st = d
            .grpc_numbers("user", r#"{"note": "x"}"#, "")
            .await
            .unwrap_err();
        assert_eq!(st.code(), tonic::Code::InvalidArgument);

        // The link is derived from the descriptor (routes named after its ends and type).
        let m = billing();
        let l = m
            .links
            .iter()
            .find(|l| l.link_type == "owner")
            .expect("link");
        assert_eq!(
            (l.source.as_str(), l.target.as_str()),
            ("invoice", "customer")
        );
        assert_eq!(l.forward_route, "customers-owners");
    }
}
