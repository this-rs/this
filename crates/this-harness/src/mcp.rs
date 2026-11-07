//! # Client MCP fédéré, sans réseau
//!
//! Un serveur MCP est une **source d'outils**, pas un outil. Ce module parle
//! JSON-RPC 2.0 (`initialize`, `tools/list`, `tools/call`) à travers un port
//! ([`McpLink`]) que l'hôte implémente (`fetch` en wasm, `reqwest` en natif) :
//! le crate n'ouvre aucune connexion.
//!
//! Quatre règles, toutes fermées par défaut :
//! - l'outil exposé s'appelle `serveur/outil`, jamais le nom nu ;
//! - la description distante et toute réponse sont des **données non fiables** :
//!   elles n'atteignent le modèle qu'enfermées (`this_agent::untrusted`) ;
//! - l'**effet** est déclaré par l'hôte ([`McpServer::allow`]), pas par le serveur :
//!   un outil non déclaré n'est pas annoncé et son appel est refusé sans toucher le lien ;
//! - un serveur qui tombe devient un `Outcome::failed`, jamais une panique, et ne
//!   prive pas les autres.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

use async_trait::async_trait;
use serde_json::{Value, json};
use this_agent::evidence::{Declaration, Effect};
use this_agent::untrusted;

use crate::event::Outcome;
use crate::ports::{Cancel, Toolbox};
use crate::wire::{Failure, ToolCall, ToolSpec};

/// Le transport d'un serveur MCP, fourni par l'hôte.
#[async_trait(?Send)]
pub trait McpLink {
    /// Envoie une requête JSON-RPC au serveur nommé et rend sa réponse
    /// (l'objet JSON-RPC entier, `result` ou `error`).
    async fn request(&self, server: &str, payload: Value) -> Result<Value, Failure>;
}

/// Un serveur et ce que l'hôte en autorise : la liste blanche, avec l'effet de chaque outil.
#[derive(Clone, Debug)]
pub struct McpServer {
    name: String,
    allowed: HashMap<String, Effect>,
}

impl McpServer {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            allowed: HashMap::new(),
        }
    }
    /// Autorise l'outil `tool` du serveur, avec l'effet que **l'hôte** lui reconnaît.
    pub fn allow(mut self, tool: impl Into<String>, effect: Effect) -> Self {
        self.allowed.insert(tool.into(), effect);
        self
    }
    pub fn name(&self) -> &str {
        &self.name
    }
}

/// Les outils de plusieurs serveurs MCP, exposés comme un [`Toolbox`].
pub struct McpToolbox {
    link: Rc<dyn McpLink>,
    servers: Vec<McpServer>,
    /// Outils découverts ET déclarés : `serveur/outil` → (description brute, schéma).
    found: RefCell<Vec<Found>>,
    next_id: Cell<u64>,
}

#[derive(Clone)]
struct Found {
    server: String,
    tool: String,
    effect: Effect,
    description: String,
    schema: Value,
}

impl McpToolbox {
    pub fn new(link: Rc<dyn McpLink>, servers: Vec<McpServer>) -> Self {
        Self {
            link,
            servers,
            found: RefCell::new(Vec::new()),
            next_id: Cell::new(1),
        }
    }

    fn rpc(&self, method: &str, params: Value) -> Value {
        let id = self.next_id.get();
        self.next_id.set(id + 1);
        json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params})
    }

    /// Interroge chaque serveur (`initialize` puis `tools/list`) et retient les
    /// outils **déclarés**. Un serveur en échec n'empêche pas les autres ; ses
    /// échecs sont rendus, avec son nom.
    pub async fn refresh(&self) -> Vec<(String, Failure)> {
        let mut found = Vec::new();
        let mut errors = Vec::new();
        for server in &self.servers {
            match self.discover(server).await {
                Ok(list) => found.extend(list),
                Err(failure) => errors.push((server.name.clone(), failure)),
            }
        }
        *self.found.borrow_mut() = found;
        errors
    }

    async fn discover(&self, server: &McpServer) -> Result<Vec<Found>, Failure> {
        let init = self.rpc(
            "initialize",
            json!({"protocolVersion": "2025-03-26", "capabilities": {},
                   "clientInfo": {"name": "this-harness", "version": env!("CARGO_PKG_VERSION")}}),
        );
        result_of(self.link.request(&server.name, init).await?)?;
        let list = self.rpc("tools/list", json!({}));
        let result = result_of(self.link.request(&server.name, list).await?)?;
        let tools = result
            .get("tools")
            .and_then(Value::as_array)
            .ok_or_else(|| {
                Failure::new(
                    crate::wire::Code::Unreadable,
                    "tools/list sans tableau `tools`",
                )
            })?;
        Ok(tools
            .iter()
            .filter_map(|t| {
                let tool = t.get("name")?.as_str()?;
                // Le serveur ne s'auto-déclare pas : hors liste blanche, l'outil n'existe pas.
                let effect = *server.allowed.get(tool)?;
                Some(Found {
                    server: server.name.clone(),
                    tool: tool.to_string(),
                    effect,
                    description: t
                        .get("description")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                    schema: t
                        .get("inputSchema")
                        .cloned()
                        .unwrap_or_else(|| json!({"type": "object"})),
                })
            })
            .collect())
    }

    fn lookup(&self, exposed: &str) -> Option<Found> {
        self.found
            .borrow()
            .iter()
            .find(|f| exposed_name(&f.server, &f.tool) == exposed)
            .cloned()
    }
}

fn exposed_name(server: &str, tool: &str) -> String {
    format!("{server}/{tool}")
}

/// Données non fiables : enfermées avec le serveur (et l'outil) comme provenance.
fn enclose(server: &str, tool: &str, body: &str) -> String {
    untrusted::wrap("mcp", &[("server", server), ("tool", tool)], body)
}

/// `result` d'une réponse JSON-RPC, ou l'erreur du serveur en `Failure`.
fn result_of(response: Value) -> Result<Value, Failure> {
    if let Some(error) = response.get("error") {
        let code = error.get("code").and_then(Value::as_i64).unwrap_or(0);
        let message = error
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("erreur sans message");
        return Err(Failure::new(
            crate::wire::Code::Provider,
            format!("erreur JSON-RPC {code} : {message}"),
        ));
    }
    response.get("result").cloned().ok_or_else(|| {
        Failure::new(
            crate::wire::Code::Unreadable,
            "réponse JSON-RPC sans `result`",
        )
    })
}

#[async_trait(?Send)]
impl Toolbox for McpToolbox {
    fn specs(&self) -> Vec<ToolSpec> {
        self.found
            .borrow()
            .iter()
            .map(|f| ToolSpec {
                name: exposed_name(&f.server, &f.tool),
                description: enclose(&f.server, &f.tool, &f.description),
                parameters: f.schema.clone(),
            })
            .collect()
    }

    fn declarations(&self) -> Vec<Declaration> {
        self.found
            .borrow()
            .iter()
            .map(|f| Declaration::new(exposed_name(&f.server, &f.tool), f.effect))
            .collect()
    }

    async fn call(&self, call: &ToolCall) -> Outcome {
        self.call_with_cancel(call, &Cancel::new()).await
    }

    async fn call_with_cancel(&self, call: &ToolCall, cancel: &Cancel) -> Outcome {
        let Some(found) = self.lookup(&call.name) else {
            return Outcome::refused(
                format!("outil MCP non autorisé : {}", call.name),
                format!("« {} » n'est pas déclaré par l'hôte", call.name),
            );
        };
        if cancel.is_cancelled() {
            return Outcome::failed("appel annulé", "annulé avant l'appel au serveur MCP");
        }
        let payload = self.rpc(
            "tools/call",
            json!({"name": found.tool, "arguments": call.arguments}),
        );
        let response = match self.link.request(&found.server, payload).await {
            Ok(r) => r,
            Err(f) => {
                return Outcome::failed(
                    format!("le serveur « {} » ne répond pas", found.server),
                    format!("serveur MCP « {} » : {}", found.server, f.message),
                );
            }
        };
        if cancel.is_cancelled() {
            return Outcome::failed("appel annulé", "annulé pendant l'appel au serveur MCP");
        }
        let result = match result_of(response) {
            Ok(r) => r,
            Err(f) => {
                return Outcome::failed(
                    enclose(&found.server, &found.tool, &f.message),
                    format!("serveur MCP « {} » : {}", found.server, f.message),
                );
            }
        };
        let text = result
            .get("content")
            .and_then(Value::as_array)
            .map(|parts| {
                parts
                    .iter()
                    .filter_map(|p| p.get("text").and_then(Value::as_str))
                    .collect::<Vec<_>>()
                    .join("\n")
            })
            .unwrap_or_default();
        let wrapped = enclose(&found.server, &found.tool, &text);
        if result.get("isError").and_then(Value::as_bool) == Some(true) {
            return Outcome::failed(
                wrapped,
                format!("l'outil « {} » a signalé une erreur", call.name),
            );
        }
        Outcome::ok(wrapped)
    }
}
