//! The error CODES a module declares, and how each one crosses a surface.
//!
//! A module declares its business codes (`ModuleDescriptor::errors`) with a `kind`, a
//! `message_key` and the parameters its message expects. `check` enforces that each code is
//! namespaced by the module name (`billing.invoice.already_paid`) and declared once. What was
//! missing is the other half of that contract: nothing exported the table, and nothing read
//! the declaration when an error crossed a surface — only the `kind` carried by the error
//! instance decided the status. So a module could declare `Conflict` and answer 500, and a
//! code nobody declares could travel with no owner at all.
//!
//! Two pieces close it:
//!
//! | Route | Body |
//! |---|---|
//! | `GET /_this/errors[?module=]` | `{ generation, errors: […], kinds: […] }` |
//! | `GET /_this/errors/{module}` | the same document, restricted to one module (and `this`) |
//!
//! - the **catalogue**: one entry per declared code — module, code, kind, HTTP status, gRPC
//!   code, GraphQL code, message key, expected parameters — so a client maps a code it
//!   receives without guessing, and the framework's own `this.*` table is included;
//! - [`normalize`]: the DECLARATION wins over the kind carried by the instance (both are
//!   supplied by the module, but only one is a contract it published), and a code that no
//!   loaded module declares becomes a **named** refusal (`this.module.reference_unknown`)
//!   instead of an anonymous 500.
//!
//! Authorization: `/_this/*` is judged by the REST layer as `Descriptor · read`
//! (`authenticated`), like every other export under that prefix.

use crate::descriptor::framework::codes;
use crate::descriptor::{ContractError, DescriptorSet, ErrorCode, ErrorDesc, ErrorKind};
use crate::server::host::ServerHost;
use axum::extract::{Path, Query};
use axum::routing::get;
use axum::{Json, Router};
use serde_json::{Value, json};
use std::sync::Arc;

/// Router of the error catalogue (`GET /_this/errors`, `GET /_this/errors/{module}`).
pub fn error_routes(host: Arc<ServerHost>) -> Router {
    let (all, one) = (host.clone(), host);
    Router::new()
        .route(
            "/_this/errors",
            get(move |Query(q): Query<Filter>| async move {
                document(&all, q.module.as_deref())
                    .map(Json)
                    .map_err(|e| crate::core::errors::ApiError(*e))
            }),
        )
        .route(
            "/_this/errors/{module}",
            get(move |Path(module): Path<String>| async move {
                document(&one, Some(&module))
                    .map(Json)
                    .map_err(|e| crate::core::errors::ApiError(*e))
            }),
        )
}

/// Query string of the catalogue: `?module=<name>`.
#[derive(Debug, Default, serde::Deserialize)]
struct Filter {
    module: Option<String>,
}

fn not_found(what: &str, id: &str) -> Box<ContractError> {
    Box::new(
        ContractError::framework(codes::ENTITY_NOT_FOUND)
            .with_param("entity", what)
            .with_param("id", id),
    )
}

/// The catalogue: every code the framework and the loaded modules DECLARE, with the mapping
/// each surface applies. `module` restricts it (the framework table stays: a client cannot
/// read a module's codes without the codes they are compared to).
pub fn document(host: &ServerHost, module: Option<&str>) -> Result<Value, Box<ContractError>> {
    let catalog = host.catalog();
    let set = catalog.descriptors.clone();
    if let Some(want) = module
        && set.module(want).is_none()
    {
        return Err(not_found("module", want));
    }

    let mut errors: Vec<Value> = Vec::new();
    for desc in crate::descriptor::framework::errors() {
        errors.push(entry(crate::descriptor::framework::FRAMEWORK_MODULE, &desc));
    }
    for m in set.modules() {
        if module.is_some_and(|want| want != m.name) {
            continue;
        }
        for desc in &m.errors {
            errors.push(entry(&m.name, desc));
        }
    }

    Ok(json!({
        "generation": catalog.generation,
        "errors": errors,
        "kinds": kinds(),
    }))
}

/// One declared code, with the mapping of its kind on the three structured surfaces.
fn entry(module: &str, desc: &ErrorDesc) -> Value {
    let mut out = json!({
        "module": module,
        "code": desc.code.as_str(),
        "kind": desc.kind.as_str(),
        "http_status": desc.kind.http_status(),
        "grpc_code": desc.kind.grpc_code(),
        "grpc_name": grpc_name(desc.kind.grpc_code()),
        "graphql_code": desc.kind.graphql_code(),
        "message_key": desc.message_key.as_str(),
    });
    if !desc.params.is_empty() {
        out["params"] = json!(desc.params);
    }
    if let Some(description) = &desc.description {
        out["description"] = json!(description);
    }
    out
}

/// The kind table itself: `kind` → HTTP status, gRPC code, GraphQL code. A client that
/// receives a `kind` (or a code whose declaration it cached) can render the status of any
/// surface from this, and a module cannot invent a kind that is not here.
fn kinds() -> Value {
    Value::Array(
        ErrorKind::ALL
            .iter()
            .map(|kind| {
                json!({
                    "kind": kind.as_str(),
                    "http_status": kind.http_status(),
                    "grpc_code": kind.grpc_code(),
                    "grpc_name": grpc_name(kind.grpc_code()),
                    "graphql_code": kind.graphql_code(),
                })
            })
            .collect(),
    )
}

/// Name of a gRPC status code, so the catalogue is readable without the tonic crate (this
/// module compiles without the `grpc` feature).
fn grpc_name(code: i32) -> &'static str {
    match code {
        1 => "CANCELLED",
        2 => "UNKNOWN",
        3 => "INVALID_ARGUMENT",
        4 => "DEADLINE_EXCEEDED",
        5 => "NOT_FOUND",
        6 => "ALREADY_EXISTS",
        7 => "PERMISSION_DENIED",
        8 => "RESOURCE_EXHAUSTED",
        9 => "FAILED_PRECONDITION",
        10 => "ABORTED",
        11 => "OUT_OF_RANGE",
        12 => "UNIMPLEMENTED",
        13 => "INTERNAL",
        14 => "UNAVAILABLE",
        15 => "DATA_LOSS",
        16 => "UNAUTHENTICATED",
        _ => "UNKNOWN",
    }
}

/// The declaration of `code` and the module that published it, if a loaded module (or the
/// framework) declares it. A code is owned by ONE module: `DescriptorSet` refuses a duplicate.
pub fn declared<'a>(set: &'a DescriptorSet, code: &ErrorCode) -> Option<(&'a str, &'a ErrorDesc)> {
    for m in set.modules() {
        if let Some(desc) = m.errors.iter().find(|d| d.code == *code) {
            return Some((m.name.as_str(), desc));
        }
    }
    None
}

/// The declaration decides, never the call site.
///
/// - a code a loaded module declares → the DECLARED kind, message key and parameters schema
///   (a module that declares `Conflict` cannot answer 500 by raising it with another kind);
/// - an undeclared code namespaced by a module → `this.module.reference_unknown` (409),
///   carrying the module and the code: a NAMED refusal (412, `FAILED_PRECONDITION` — the
///   contract a module published is not the one it honoured), never an anonymous 500;
/// - a framework code → unchanged (its kind is already the framework table's).
pub fn normalize(set: &DescriptorSet, err: ContractError) -> ContractError {
    if err.code.namespace() == crate::descriptor::framework::FRAMEWORK_MODULE {
        return err;
    }
    let Some((_, desc)) = declared(set, &err.code) else {
        // Nobody declares this code. Refuse by name rather than publish a status the
        // declaration does not justify.
        return ContractError::framework(codes::MODULE_REFERENCE_UNKNOWN)
            .with_param("module", err.code.namespace())
            .with_param("entity", err.code.as_str());
    };
    if desc.kind == err.kind && desc.message_key == err.message_key {
        // Already the declaration: keep the instance (its message may be localized already).
        return err;
    }
    let mut out = ContractError::from_desc(desc);
    out.params = err.params;
    out.fields = err.fields;
    out.trace_id = err.trace_id;
    out.message = err.message;
    out
}

/// [`normalize`] for an `anyhow::Error` about to cross a surface holding `host`.
pub fn normalize_anyhow(host: &ServerHost, err: &anyhow::Error) -> ContractError {
    normalize(&host.descriptors(), crate::core::errors::from_anyhow(err))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::LinksConfig;
    use crate::descriptor::ModuleDescriptor;
    use crate::server::entity_registry::EntityRegistry;
    use crate::storage::InMemoryLinkService;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use std::collections::HashMap;
    use tower::ServiceExt;

    /// `billing` declares `billing.invoice.already_paid` as a Conflict.
    fn billing() -> ModuleDescriptor {
        let json = json!({
            "name": "billing",
            "version": "1.0.0",
            "entities": [{
                "name": "invoice",
                "plural": "invoices",
                "fields": [{ "name": "amount", "ty": "f64" }]
            }],
            "errors": [{
                "code": "billing.invoice.already_paid",
                "kind": "conflict",
                "message_key": "billing.invoice.already_paid",
                "params": ["invoice"],
                "description": "The invoice was already paid."
            }]
        })
        .to_string();
        ModuleDescriptor::from_json(&json).expect("descriptor")
    }

    fn set() -> DescriptorSet {
        let mut set = DescriptorSet::new();
        set.insert(billing()).expect("insert");
        set
    }

    fn host(set: DescriptorSet) -> Arc<ServerHost> {
        let mut h = ServerHost::from_builder_components(
            Arc::new(InMemoryLinkService::new()),
            LinksConfig::default_config(),
            EntityRegistry::new(),
            HashMap::new(),
            HashMap::new(),
        )
        .expect("host");
        h.authorize = false;
        Arc::new(h.with_descriptors(set))
    }

    async fn get(host: Arc<ServerHost>, uri: &str) -> (StatusCode, Value) {
        let res = error_routes(host)
            .oneshot(Request::get(uri).body(Body::empty()).expect("request"))
            .await
            .expect("response");
        let status = res.status();
        let bytes = axum::body::to_bytes(res.into_body(), 1 << 20)
            .await
            .expect("body");
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        )
    }

    #[tokio::test]
    async fn error_catalogue_maps_every_declared_code_to_each_surface() {
        let (s, v) = get(host(set()), "/_this/errors").await;
        assert_eq!(s, StatusCode::OK, "{v}");
        let entries = v["errors"].as_array().expect("errors");
        let paid = entries
            .iter()
            .find(|e| e["code"] == json!("billing.invoice.already_paid"))
            .expect("the module's code is exported");
        assert_eq!(paid["module"], json!("billing"));
        assert_eq!(paid["kind"], json!("conflict"));
        assert_eq!(paid["http_status"], json!(409));
        assert_eq!(paid["grpc_code"], json!(6));
        assert_eq!(paid["grpc_name"], json!("ALREADY_EXISTS"));
        assert_eq!(paid["graphql_code"], json!("CONFLICT"));
        assert_eq!(paid["params"], json!(["invoice"]));
        assert_eq!(paid["description"], json!("The invoice was already paid."));
        // The framework's own table is served too, so a client can map `this.*` codes.
        assert!(
            entries
                .iter()
                .any(|e| e["module"] == json!("this")
                    && e["code"] == json!("this.validation.failed")),
            "framework codes are part of the catalogue"
        );
        // The kind table: closed, and consistent with the entries above.
        let conflict = v["kinds"]
            .as_array()
            .expect("kinds")
            .iter()
            .find(|k| k["kind"] == json!("conflict"))
            .expect("conflict kind");
        assert_eq!(conflict["http_status"], json!(409));
        assert_eq!(conflict["grpc_name"], json!("ALREADY_EXISTS"));
    }

    #[tokio::test]
    async fn error_catalogue_filters_by_module_and_refuses_an_unknown_one() {
        let h = host(set());
        let (s, v) = get(h.clone(), "/_this/errors/billing").await;
        assert_eq!(s, StatusCode::OK, "{v}");
        // Only the framework table + that module's codes.
        assert!(
            v["errors"]
                .as_array()
                .expect("errors")
                .iter()
                .all(|e| e["module"] == json!("billing") || e["module"] == json!("this")),
            "{v}"
        );
        let (s, v) = get(h, "/_this/errors/nope").await;
        assert_eq!(s, StatusCode::NOT_FOUND, "{v}");
        assert_eq!(v["code"], json!("this.entity.not_found"));
    }

    #[test]
    fn normalize_keeps_framework_codes_and_the_declaration_wins() {
        let set = set();
        // Framework code: untouched.
        let fw = ContractError::framework(codes::ENTITY_NOT_FOUND);
        assert_eq!(normalize(&set, fw.clone()), fw);

        // Declared code raised with the wrong kind: the declaration decides (409, not 500).
        let raised = ContractError::new("billing.invoice.already_paid", ErrorKind::Internal)
            .with_param("invoice", "42");
        let out = normalize(&set, raised);
        assert_eq!(out.kind, ErrorKind::Conflict);
        assert_eq!(out.kind.http_status(), 409);
        assert_eq!(out.kind.grpc_code(), 6);
        assert_eq!(out.message_key.as_str(), "billing.invoice.already_paid");
        assert_eq!(out.params["invoice"], json!("42"), "params are kept");

        // Already right: the instance is kept as is (its message may already be localized).
        let right = ContractError::from_desc(&set.module("billing").expect("module").errors[0]);
        assert_eq!(normalize(&set, right.clone()), right);
    }

    #[test]
    fn normalize_refuses_an_undeclared_module_code_by_name() {
        let set = set();
        // Namespaced by a loaded module but never declared there.
        let ghost = ContractError::new("billing.invoice.ghost", ErrorKind::Internal);
        let out = normalize(&set, ghost);
        assert_eq!(out.code.as_str(), codes::MODULE_REFERENCE_UNKNOWN);
        assert_eq!(
            out.kind.http_status(),
            412,
            "a named refusal (precondition failed), not a 500"
        );
        assert_eq!(out.kind.grpc_code(), 9, "FAILED_PRECONDITION");
        assert_eq!(out.params["module"], json!("billing"));
        assert_eq!(out.params["entity"], json!("billing.invoice.ghost"));

        // Namespace of no loaded module at all: same named refusal.
        let stray = ContractError::new("ghost.foo", ErrorKind::Internal);
        assert_eq!(
            normalize(&set, stray).code.as_str(),
            codes::MODULE_REFERENCE_UNKNOWN
        );
    }
}
