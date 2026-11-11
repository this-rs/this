//! Export of the INTERFACE a module declares: its **views** — what to draw, where, and what
//! they replace — together with everything an app needs to draw them without reading the raw
//! descriptor.
//!
//! | Route | Body |
//! |---|---|
//! | `GET /_this/views[?module=&surface=]` | `{ generation, views: […], actions: […], modules: […] }` |
//! | `GET /_this/views/{module}` | the same document, restricted to one module |
//!
//! A view is a declaration, not a drawing: the host renders it with its own widgets
//! (`this_descriptor::views`). So the payload carries the tree **and** the resolution of what
//! the tree points at:
//!
//! - `views[].root`: the tree as declared (containers, bricks, clicks);
//! - `views[].entities`: every entity the bricks read, with its `plural`, its REST `path`,
//!   its i18n label keys (`label`, `label_plural`) and `served` — whether this host actually
//!   serves it right now (a view may name an entity whose module is not loaded);
//! - `actions`: the actions the module declares, that `button`/`form` bricks reference by
//!   name — a name with nothing behind it is a dead button;
//! - `modules`: identity and locale hints (`default_locale`, `locales`) of the modules in
//!   scope, so the app knows which message bundle to fetch.
//!
//! The document is read from the LIVE catalog (`ServerHost::catalog`), so a module published
//! at runtime is exported without rebuilding the router, and unloaded one leaves silently.
//!
//! Everything here is a projection of declarations already hosted: no row, no text, no secret.
//! Authorization: `/_this/*` is judged by the REST layer as `Descriptor · read`
//! (`authenticated`) — see `exposure::descriptor`.

use crate::core::errors::ApiError;
use crate::descriptor::framework::codes;
use crate::descriptor::views::{Brick, Node};
use crate::descriptor::{Action, ContractError, DescriptorSet, Surface, ViewDesc};
use crate::server::catalog::HostCatalog;
use crate::server::host::ServerHost;
use axum::extract::{Path, Query};
use axum::routing::get;
use axum::{Json, Router};
use serde_json::{Value, json};
use std::collections::BTreeSet;
use std::sync::Arc;

/// Router of the interface export (`GET /_this/views`, `GET /_this/views/{module}`).
pub fn views_routes(host: Arc<ServerHost>) -> Router {
    let (all, one) = (host.clone(), host);
    Router::new()
        .route(
            "/_this/views",
            get(move |Query(q): Query<Filter>| async move {
                document(&all, q.module.as_deref(), q.surface.as_deref())
                    .map(Json)
                    .map_err(|e| ApiError(*e))
            }),
        )
        .route(
            "/_this/views/{module}",
            get(
                move |Path(module): Path<String>, Query(q): Query<Filter>| async move {
                    document(&one, Some(&module), q.surface.as_deref())
                        .map(Json)
                        .map_err(|e| ApiError(*e))
                },
            ),
        )
}

/// Query string of the export: `?module=<name>&surface=<sidebar|main|panel|header>`.
/// An unknown value is a named refusal (`this.transport.bad_request`), never silently ignored.
#[derive(Debug, Default, serde::Deserialize)]
struct Filter {
    module: Option<String>,
    surface: Option<String>,
}

fn bad_request(reason: impl Into<String>) -> Box<ContractError> {
    Box::new(
        ContractError::framework(codes::TRANSPORT_BAD_REQUEST).with_param("reason", reason.into()),
    )
}

fn not_found(what: &str, id: &str) -> Box<ContractError> {
    Box::new(
        ContractError::framework(codes::ENTITY_NOT_FOUND)
            .with_param("entity", what)
            .with_param("id", id),
    )
}

/// `surface=` as a [`Surface`]. An unknown name is refused: reading it as "all surfaces"
/// would answer a different question than the one asked.
fn parse_surface(name: &str) -> Result<Surface, Box<ContractError>> {
    match name {
        "sidebar" => Ok(Surface::Sidebar),
        "main" => Ok(Surface::Main),
        "panel" => Ok(Surface::Panel),
        "header" => Ok(Surface::Header),
        other => Err(bad_request(format!(
            "unknown surface `{other}`: expected one of sidebar, main, panel, header"
        ))),
    }
}

/// Name of the GraphQL field that serves the same document as `GET /_this/views`.
pub const GRAPHQL_FIELD: &str = "_thisViews";

/// The document for a GUARDED surface (GraphQL): the same rule the REST layer applies to
/// `/_this/*` — `Descriptor · read` = `authenticated` — so the interface of the host is not
/// published to an anonymous caller when the policies are armed. Disarmed host or
/// report-only: nothing is refused, exactly like everywhere else.
pub fn document_guarded(
    guard: &crate::server::exposure::SurfaceGuard,
    host: &ServerHost,
    module: Option<&str>,
    surface: Option<&str>,
) -> Result<Value, Box<ContractError>> {
    if guard.is_enforced() && matches!(guard.context(), crate::core::auth::AuthContext::Anonymous) {
        return Err(Box::new(ContractError::framework(
            codes::AUTH_UNAUTHENTICATED,
        )));
    }
    document(host, module, surface)
}

/// The whole document, optionally restricted to one module and/or one surface.
pub fn document(
    host: &ServerHost,
    module: Option<&str>,
    surface: Option<&str>,
) -> Result<Value, Box<ContractError>> {
    let catalog = host.catalog();
    let set = catalog.descriptors.clone();
    let surface = surface.map(parse_surface).transpose()?;
    if let Some(want) = module
        && set.module(want).is_none()
    {
        return Err(not_found("module", want));
    }

    let mut views: Vec<Value> = Vec::new();
    let mut actions: Vec<Value> = Vec::new();
    let mut modules: Vec<Value> = Vec::new();
    for m in set.modules() {
        if module.is_some_and(|want| want != m.name) {
            continue;
        }
        modules.push(json!({
            "name": m.name,
            "version": m.version,
            "default_locale": m.i18n.default_locale.as_str(),
            "locales": m.i18n.locales.iter().map(|l| l.as_str()).collect::<Vec<_>>(),
        }));
        for a in &m.actions {
            actions.push(action_entry(&m.name, a));
        }
        for v in &m.views {
            if surface.is_some_and(|want| want != v.surface) {
                continue;
            }
            views.push(view_entry(&set, &catalog, &m.name, v));
        }
    }

    Ok(json!({
        "generation": catalog.generation,
        "views": views,
        "actions": actions,
        "modules": modules,
    }))
}

/// One view, with what its tree points at resolved against the live host.
fn view_entry(set: &DescriptorSet, catalog: &HostCatalog, module: &str, v: &ViewDesc) -> Value {
    let entities: Vec<Value> = referenced_entities(&v.root)
        .into_iter()
        .map(|name| entity_entry(set, catalog, &name))
        .collect();
    let mut out = json!({
        "module": module,
        "name": v.name,
        "surface": v.surface,
        "entities": entities,
        "root": v.root,
    });
    if let Some(title) = &v.title {
        out["title"] = json!(title);
    }
    if let Some(replaces) = &v.replaces {
        out["replaces"] = json!(replaces);
    }
    if let Some(look) = &v.look {
        out["look"] = json!(look);
    }
    out
}

/// One action the module declares: its name (what bricks reference) and its declaration.
fn action_entry(module: &str, action: &Action) -> Value {
    json!({
        "module": module,
        "name": action.name(),
        "action": action,
    })
}

/// One entity a brick reads: identity, path, i18n label keys, and whether it is served.
///
/// `served` is read from the live catalog (the fetcher map), not from the descriptor: a view
/// may name an entity of a module that is not loaded on this host, and an app must be able to
/// tell "nothing to show yet" from "this address is wrong".
fn entity_entry(set: &DescriptorSet, catalog: &HostCatalog, name: &str) -> Value {
    let desc = set.entity(name).map(|(_, e)| e);
    let plural = desc
        .map(|e| e.plural.clone())
        .or_else(|| catalog.plural_of(name).map(String::from));
    let mut out = json!({
        "name": name,
        "served": catalog.fetchers.contains_key(name),
    });
    if let Some(e) = desc {
        if let Some(label) = &e.i18n.label {
            out["label"] = json!(label.as_str());
        }
        if let Some(label) = &e.i18n.label_plural {
            out["label_plural"] = json!(label.as_str());
        }
    }
    if let Some(plural) = plural {
        out["plural"] = json!(plural);
        out["path"] = json!(format!("/{plural}"));
    }
    out
}

/// Every entity named by the bricks of `node`, in a stable order.
pub fn referenced_entities(node: &Node) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    collect_entities(node, &mut out);
    out
}

fn collect_entities(node: &Node, out: &mut BTreeSet<String>) {
    match node {
        Node::Stack { children } | Node::Row { children } => {
            for child in children {
                collect_entities(child, out);
            }
        }
        Node::Tabs { tabs } => {
            for tab in tabs {
                for child in &tab.children {
                    collect_entities(child, out);
                }
            }
        }
        Node::Drawer { children, .. } | Node::Modal { children, .. } => {
            for child in children {
                collect_entities(child, out);
            }
        }
        // Une page livrée par le module ne nomme aucune entité : rien à ajouter.
        Node::Html { .. } => {}
        Node::Leaf { brick } => {
            if let Some(entity) = brick_entity(brick) {
                out.insert(entity.to_string());
            }
        }
        // La condition lit son entité : la vue en dépend autant que d'une brique, et un
        // consommateur sans droit sur elle ne doit pas la voir passer par un `when`.
        Node::When { entity, child, .. } => {
            out.insert(entity.clone());
            collect_entities(child, out);
        }
    }
}

/// The entity a brick reads (`None` for the state bricks: banner, capability request,
/// connection indicator without an entity).
fn brick_entity(brick: &Brick) -> Option<&str> {
    brick.entity()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::LinksConfig;
    use crate::descriptor::DescriptorSet;
    use crate::server::entity_registry::EntityRegistry;
    use crate::storage::InMemoryLinkService;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use std::collections::HashMap;
    use tower::ServiceExt;

    /// A `shop` module whose `conversations` view reads the `conversation` entity, at the
    /// sidebar, replacing the `threads` view of the same module.
    fn shop() -> crate::descriptor::ModuleDescriptor {
        let json = json!({
            "name": "shop",
            "version": "1.2.0",
            "i18n": { "default_locale": "fr", "locales": ["fr", "en"] },
            "entities": [{
                "name": "conversation",
                "plural": "conversations",
                "fields": [{ "name": "title", "ty": "string" }],
                "i18n": { "label": "shop.conversation.label",
                          "label_plural": "shop.conversation.plural" }
            }],
            "views": [{
                "name": "threads",
                "surface": "sidebar",
                "root": { "type": "leaf", "brick": {
                    "type": "list", "entity": "conversation", "label": "title" } }
            }, {
                "name": "conversations",
                "surface": "sidebar",
                "title": "Conversations",
                "replaces": "threads",
                "root": { "type": "stack", "children": [
                    { "type": "leaf", "brick": {
                        "type": "metrics", "entity": "conversation" } },
                    { "type": "leaf", "brick": {
                        "type": "list", "entity": "conversation", "label": "title",
                        "on_click": { "kind": "open_detail" } } }
                ] }
            }, {
                "name": "conversation_detail",
                "surface": "main",
                "root": { "type": "leaf", "brick": {
                    "type": "card", "entity": "conversation", "fields": ["title"] } }
            }],
            "actions": [{ "kind": "create", "name": "new_conversation",
                          "entity": "conversation", "fields": ["title"] }]
        })
        .to_string();
        crate::descriptor::ModuleDescriptor::from_json(&json).expect("descriptor")
    }

    /// A host carrying the descriptors, with NO fetcher: nothing is served yet.
    fn host(modules: Vec<crate::descriptor::ModuleDescriptor>) -> Arc<ServerHost> {
        let mut set = DescriptorSet::new();
        for m in modules {
            set.insert(m).expect("insert");
        }
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
        let res = views_routes(host)
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
    async fn views_export_resolves_what_the_tree_points_at() {
        let (s, v) = get(host(vec![shop()]), "/_this/views").await;
        assert_eq!(s, StatusCode::OK, "{v}");
        assert_eq!(v["generation"], json!(1));
        assert_eq!(v["views"].as_array().map(Vec::len), Some(3), "{v}");

        let detail = v["views"]
            .as_array()
            .expect("views")
            .iter()
            .find(|e| e["name"] == json!("conversation_detail"))
            .expect("conversation_detail");
        assert_eq!(detail["module"], json!("shop"));
        assert_eq!(detail["surface"], json!("main"));
        // The brick's entity, resolved: plural, REST path, i18n label keys, and `served`
        // (no fetcher on this host, so the app knows there is nothing to draw).
        assert_eq!(detail["entities"][0]["name"], json!("conversation"));
        assert_eq!(detail["entities"][0]["plural"], json!("conversations"));
        assert_eq!(detail["entities"][0]["path"], json!("/conversations"));
        assert_eq!(detail["entities"][0]["served"], json!(false));
        assert_eq!(
            detail["entities"][0]["label"],
            json!("shop.conversation.label")
        );
        assert_eq!(
            detail["entities"][0]["label_plural"],
            json!("shop.conversation.plural")
        );

        // Replacement is exported as declared (the host hides the replaced one).
        let replacing = v["views"]
            .as_array()
            .expect("views")
            .iter()
            .find(|e| e["name"] == json!("conversations"))
            .expect("conversations");
        assert_eq!(replacing["replaces"], json!("threads"));
        assert_eq!(replacing["title"], json!("Conversations"));
        // A brick with no entity (banner, capability request) contributes nothing.
        assert_eq!(replacing["entities"].as_array().map(Vec::len), Some(1));

        // Actions, by name, with their declaration.
        assert_eq!(v["actions"][0]["module"], json!("shop"));
        assert_eq!(v["actions"][0]["name"], json!("new_conversation"));
        assert_eq!(v["actions"][0]["action"]["kind"], json!("create"));

        // Locale hints, so the app knows which bundle to fetch.
        assert_eq!(v["modules"][0]["name"], json!("shop"));
        assert_eq!(v["modules"][0]["default_locale"], json!("fr"));
        assert_eq!(v["modules"][0]["locales"], json!(["fr", "en"]));
    }

    #[tokio::test]
    async fn views_export_filters_by_module_and_by_surface() {
        let h = host(vec![shop()]);
        let (s, v) = get(h.clone(), "/_this/views/shop").await;
        assert_eq!(s, StatusCode::OK, "{v}");
        assert_eq!(v["views"].as_array().map(Vec::len), Some(3));

        let (s, v) = get(h.clone(), "/_this/views?surface=main").await;
        assert_eq!(s, StatusCode::OK, "{v}");
        let names: Vec<&Value> = v["views"]
            .as_array()
            .expect("views")
            .iter()
            .map(|e| &e["name"])
            .collect();
        assert_eq!(names, vec![&json!("conversation_detail")], "{v}");

        let (s, v) = get(h, "/_this/views?module=shop&surface=sidebar").await;
        assert_eq!(s, StatusCode::OK, "{v}");
        assert_eq!(v["views"].as_array().map(Vec::len), Some(2), "{v}");
    }

    #[tokio::test]
    async fn views_export_refuses_an_unknown_module_or_surface_by_name() {
        let h = host(vec![shop()]);
        let (s, v) = get(h.clone(), "/_this/views/nope").await;
        assert_eq!(s, StatusCode::NOT_FOUND, "{v}");
        assert_eq!(v["code"], json!("this.entity.not_found"));

        let (s, v) = get(h, "/_this/views?surface=dashboard").await;
        assert_eq!(s, StatusCode::BAD_REQUEST, "{v}");
        assert_eq!(v["code"], json!("this.transport.bad_request"));
        assert!(
            v.to_string().contains("dashboard"),
            "the refusal names the value: {v}"
        );
    }

    #[tokio::test]
    async fn views_export_of_a_host_without_descriptors_is_empty() {
        let (s, v) = get(host(vec![]), "/_this/views").await;
        assert_eq!(s, StatusCode::OK, "{v}");
        assert_eq!(v["views"], json!([]));
        assert_eq!(v["actions"], json!([]));
        assert_eq!(v["modules"], json!([]));
    }

    #[test]
    fn a_when_condition_makes_its_entity_a_dependency_of_the_view() {
        let node: Node = serde_json::from_value(json!({
            "type": "when", "entity": "session",
            "filter": {"cmp": {"field": "connected", "op": "eq", "value": true}},
            "child": {"type": "leaf", "brick": {"type": "list", "entity": "key", "label": "name"}}
        }))
        .expect("node");
        let got: Vec<String> = referenced_entities(&node).into_iter().collect();
        assert_eq!(got, ["key", "session"]);
    }
}
