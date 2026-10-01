//! aletheia_server — serveur this-rs multi-protocoles sur un store obrain aletheia.
//!
//! Expose en LECTURE SEULE un store obrain substrate (par défaut les 38
//! `entity_pages` d'aletheia, nœuds de label `Page`) sur **tous** les
//! protocoles du framework :
//!
//! | protocole  | point d'entrée                                   |
//! |------------|--------------------------------------------------|
//! | REST       | `GET /pages`, `GET /pages/{id}`, `GET /healthz`   |
//! | GraphQL    | `POST /graphql` (+ `/graphql/schema`, playground) |
//! | gRPC       | `this_grpc.EntityService` / `LinkService` (HTTP/2)|
//! | WebSocket  | `GET /ws`                                         |
//! | SSE        | `GET /events/stream`                              |
//! | MCP        | `GET /mcp` (document), `POST /mcp` (JSON-RPC 2.0)  |
//!
//! ## Écart majeur avec `ObrainDataService` (à lire)
//!
//! Les stores d'aletheia sont écrits par le binding obrain Node
//! (`aletheia/src/lib/aletheia/entity-obrain.ts`, `weave-obrain.ts`) : les
//! nœuds portent des **propriétés plates** (`qualified_name`, `kind`, `title`,
//! …) et des **arêtes natives** ; ils n'ont ni `_uuid` ni `_data`.
//! `ObrainDataService::<T>` (this/src/storage/obrain.rs) exige exactement ces
//! deux propriétés (`_data` = JSON de l'entité, `_uuid` = index) : il ne peut
//! donc pas relire ces stores (0 résultat). Ce serveur lit le graphe
//! **nativement** via le moteur de requêtes (`ObrainDB::execute`, GQL) — le
//! même chemin que le binding Node — et n'utilise `ObrainStore` que comme
//! handle de store (et pour `ObrainLinkService`, exposé tel quel).
//!
//! Conséquence : le serveur est **read-only par construction** — aucun
//! `EntityCreator` n'est enregistré, aucune route POST/PUT/DELETE n'existe.
//! `--read-only` ouvre en plus le store avec `ObrainDB::open_read_only`
//! (mutations refusées au niveau moteur) : c'est le mode à utiliser en prod.
//!
//! ## Usage
//!
//! ```text
//! aletheia_server [OPTIONS] [STORE] [ADDR]
//!
//!   --store PATH    store obrain (défaut : entity.obrain d'aletheia)
//!   --weave PATH    store weave optionnel (défaut : weave.obrain voisin du store)
//!   --no-weave      ne pas charger de store weave
//!   --addr ADDR     adresse d'écoute (défaut 127.0.0.1:8090)
//!   --read-only     ouvre le store en lecture seule stricte
//!   --probe         affiche les comptes du store puis quitte
//! ```

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result, anyhow, bail};
use async_trait::async_trait;
use axum::body::Bytes;
use axum::extract::{Path as AxumPath, Query as AxumQuery, State as AxumState};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use chrono::{DateTime, Utc};
use obrain_common::types::Value as ObrainValue;
use serde::Serialize;
use serde_json::{Map, Value as JVal, json};
use uuid::Uuid;

use this::config::LinksConfig;
use this::core::entity::{Data, Entity};
use this::core::field::FieldValue;
use this::core::module::{EntityCreator, EntityFetcher, Module};
use this::server::builder::ServerBuilder;
use this::server::entity_registry::{EntityDescriptor, EntityRegistry};
use this::server::{GraphQLExposure, GrpcExposure, RestExposure, WebSocketExposure};
use this::storage::{ObrainLinkService, ObrainStore};

// ---------------------------------------------------------------------------
// Constantes / CLI
// ---------------------------------------------------------------------------

const DEFAULT_ENTITY_STORE: &str = "/home/triviere/projects/aletheia/data/xth/obrain/entity.obrain";
const DEFAULT_ADDR: &str = "127.0.0.1:8090";

struct Args {
    store: PathBuf,
    weave: Option<PathBuf>,
    addr: String,
    read_only: bool,
    probe: bool,
}

fn usage() -> String {
    "\
aletheia_server — this-rs multi-protocoles sur store obrain aletheia (lecture seule)

USAGE:
    aletheia_server [OPTIONS] [STORE] [ADDR]

ARGS:
    STORE   chemin du store obrain entity (defaut: entity.obrain d'aletheia)
    ADDR    adresse d'ecoute (defaut: 127.0.0.1:8090)

OPTIONS:
    --store PATH   idem STORE
    --weave PATH   store weave (defaut: weave.obrain a cote du store, si present)
    --no-weave     ne pas charger de store weave
    --addr ADDR    idem ADDR
    --read-only    ouvre le store en lecture seule stricte (recommande en prod)
    --probe        affiche les comptes du store puis quitte
    -h, --help     cette aide
"
    .to_string()
}

fn parse_args() -> Result<Args> {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let mut store: Option<PathBuf> = None;
    let mut addr: Option<String> = None;
    let mut weave: Option<PathBuf> = None;
    let mut no_weave = false;
    let mut read_only = false;
    let mut probe = false;

    let mut i = 0;
    while i < argv.len() {
        let a = argv[i].as_str();
        match a {
            "-h" | "--help" => {
                print!("{}", usage());
                std::process::exit(0);
            }
            "--store" => {
                i += 1;
                store = Some(PathBuf::from(
                    argv.get(i).context("--store attend un chemin")?,
                ));
            }
            "--weave" => {
                i += 1;
                weave = Some(PathBuf::from(
                    argv.get(i).context("--weave attend un chemin")?,
                ));
            }
            "--no-weave" => no_weave = true,
            "--addr" => {
                i += 1;
                addr = Some(argv.get(i).context("--addr attend une adresse")?.clone());
            }
            "--read-only" => read_only = true,
            "--probe" => probe = true,
            other if other.starts_with("--") => bail!("option inconnue: {other}"),
            other => {
                if store.is_none() {
                    store = Some(PathBuf::from(other));
                } else if addr.is_none() {
                    addr = Some(other.to_string());
                } else {
                    bail!("argument en trop: {other}");
                }
            }
        }
        i += 1;
    }

    let store = store.unwrap_or_else(|| PathBuf::from(DEFAULT_ENTITY_STORE));
    let addr = addr.unwrap_or_else(|| DEFAULT_ADDR.to_string());

    // Weave par défaut : le voisin `weave.obrain` du store d'entités.
    let weave = if no_weave {
        None
    } else if weave.is_some() {
        weave
    } else {
        let sibling = store
            .parent()
            .map(|d| d.join("weave.obrain"))
            .unwrap_or_else(|| PathBuf::from("weave.obrain"));
        if sibling.exists() {
            Some(sibling)
        } else {
            None
        }
    };

    Ok(Args {
        store,
        weave,
        addr,
        read_only,
        probe,
    })
}

// ---------------------------------------------------------------------------
// Accès natif au store (moteur de requêtes obrain)
// ---------------------------------------------------------------------------

fn value_to_json(v: &ObrainValue) -> JVal {
    match v {
        ObrainValue::Null => JVal::Null,
        ObrainValue::Bool(b) => JVal::Bool(*b),
        ObrainValue::Int64(i) => JVal::Number((*i).into()),
        ObrainValue::Float64(f) => serde_json::Number::from_f64(*f)
            .map(JVal::Number)
            .unwrap_or(JVal::Null),
        ObrainValue::String(s) => JVal::String(s.to_string()),
        ObrainValue::Bytes(b) => JVal::String(format!("<{} bytes>", b.len())),
        other => serde_json::to_value(other).unwrap_or(JVal::Null),
    }
}

/// Exécute une requête GQL sur le store et renvoie les lignes en JSON.
fn obrain_rows(store: &ObrainStore, query: &str) -> Result<Vec<Map<String, JVal>>> {
    let res = store
        .db()
        .execute(query)
        .map_err(|e| anyhow!("requete obrain refusee: {e} [query: {query}]"))?;
    let cols = res.columns;
    let mut out = Vec::with_capacity(res.rows.len());
    for row in &res.rows {
        let mut obj = Map::with_capacity(cols.len());
        for (idx, col) in cols.iter().enumerate() {
            obj.insert(
                col.clone(),
                row.get(idx).map(value_to_json).unwrap_or(JVal::Null),
            );
        }
        out.push(obj);
    }
    Ok(out)
}

fn jstr(m: &Map<String, JVal>, k: &str) -> String {
    m.get(k)
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string()
}

fn jopt_str(m: &Map<String, JVal>, k: &str) -> Option<String> {
    match m.get(k) {
        Some(JVal::String(s)) if !s.is_empty() => Some(s.clone()),
        Some(JVal::Null) | None => None,
        Some(other) => Some(other.to_string()),
    }
}

fn jf64(m: &Map<String, JVal>, k: &str) -> f64 {
    match m.get(k) {
        Some(JVal::Number(n)) => n.as_f64().unwrap_or(0.0),
        Some(JVal::String(s)) => s.parse().unwrap_or(0.0),
        _ => 0.0,
    }
}

fn jbool(m: &Map<String, JVal>, k: &str) -> Option<bool> {
    match m.get(k) {
        Some(JVal::Bool(b)) => Some(*b),
        Some(JVal::String(s)) if s == "1" || s == "true" => Some(true),
        Some(JVal::String(s)) if s == "0" || s == "false" => Some(false),
        _ => None,
    }
}

/// Les props JSON d'aletheia (`neighborhood`, `neighbors`, `openings`) sont
/// stockées stringifiées : on les re-déplie quand c'est possible.
fn jjson(m: &Map<String, JVal>, k: &str) -> JVal {
    match m.get(k) {
        Some(JVal::String(s)) if !s.is_empty() => {
            serde_json::from_str(s).unwrap_or_else(|_| JVal::String(s.clone()))
        }
        Some(v) => v.clone(),
        None => JVal::Null,
    }
}

/// UUID synthétique déterministe (FNV-1a 128 bits, version 8) — les ids
/// d'aletheia sont des slugs (`mp_hodge`), pas des UUID ; GraphQL et le
/// système de liens de this-rs raisonnent en `Uuid`, d'où cette projection
/// stable et réversible via l'index des slugs.
fn synth_uuid(s: &str) -> Uuid {
    fn fnv1a64(bytes: &[u8], seed: u64) -> u64 {
        let mut h = seed;
        for b in bytes {
            h ^= u64::from(*b);
            h = h.wrapping_mul(0x0000_0100_0000_01b3);
        }
        h
    }
    let h1 = fnv1a64(s.as_bytes(), 0xcbf2_9ce4_8422_2325);
    let h2 = fnv1a64(s.as_bytes(), 0x8422_2325_cbf2_9ce4);
    let mut b = [0u8; 16];
    b[..8].copy_from_slice(&h1.to_be_bytes());
    b[8..].copy_from_slice(&h2.to_be_bytes());
    b[6] = (b[6] & 0x0f) | 0x80; // version 8 (custom)
    b[8] = (b[8] & 0x3f) | 0x80; // variante RFC 4122
    Uuid::from_bytes(b)
}

fn parse_ts(raw: Option<&str>) -> DateTime<Utc> {
    let Some(raw) = raw else {
        return Utc::now();
    };
    if let Ok(ms) = raw.parse::<i64>()
        && let Some(dt) = DateTime::<Utc>::from_timestamp_millis(ms)
    {
        return dt;
    }
    DateTime::parse_from_rfc3339(raw)
        .map(|d| d.with_timezone(&Utc))
        .unwrap_or_else(|_| Utc::now())
}

// ---------------------------------------------------------------------------
// Entité « page » (nœuds de label `Page` du store entity.obrain)
// ---------------------------------------------------------------------------

/// Projection explicite des clés — même contrainte que le binding Node :
/// `RETURN n` ne renvoie pas les props, il faut les nommer une à une.
const PAGE_RETURN: &str = "n.qualified_name AS qualified_name, n.kind AS kind, \
     n.title AS title, n.statement AS statement, n.demonstration AS demonstration, \
     n.context AS context, n.status AS status, n.verification AS verification, \
     n.next_work AS next_work, n.provider AS provider, n.model AS model, \
     n.earn_atoms AS earn_atoms, n.lean_source AS lean_source, n.lean_log AS lean_log, \
     n.lean_ok AS lean_ok, n.neighborhood AS neighborhood, n.neighbors AS neighbors, \
     n.openings AS openings, n.updated_at AS updated_at";

#[derive(Debug, Clone, Serialize)]
struct Page {
    /// Id aletheia réel (`mp_hodge`), extrait de `qualified_name` (`entity:mp_hodge`).
    id: String,
    #[serde(rename = "type")]
    entity_type: String,
    name: String,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
    status: String,
    qualified_name: String,
    kind: String,
    title: String,
    statement: String,
    demonstration: String,
    context: String,
    verification: String,
    next_work: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    provider: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    model: Option<String>,
    earn_atoms: f64,
    lean_source: String,
    lean_log: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    lean_ok: Option<bool>,
    neighborhood: JVal,
    neighbors: JVal,
    openings: JVal,
    #[serde(skip)]
    uuid: Uuid,
}

impl Page {
    fn from_row(row: &Map<String, JVal>) -> Self {
        let qualified_name = jstr(row, "qualified_name");
        let id = qualified_name
            .strip_prefix("entity:")
            .unwrap_or(&qualified_name)
            .to_string();
        let updated_raw = jopt_str(row, "updated_at");
        let updated_at = parse_ts(updated_raw.as_deref());
        let title = {
            let t = jstr(row, "title");
            if t.is_empty() { id.clone() } else { t }
        };
        Self {
            uuid: synth_uuid(&id),
            id,
            entity_type: "page".to_string(),
            name: title.clone(),
            created_at: updated_at,
            updated_at,
            status: {
                let s = jstr(row, "status");
                if s.is_empty() { "open".to_string() } else { s }
            },
            qualified_name,
            kind: {
                let k = jstr(row, "kind");
                if k.is_empty() { "node".to_string() } else { k }
            },
            title,
            statement: jstr(row, "statement"),
            demonstration: jstr(row, "demonstration"),
            context: jstr(row, "context"),
            verification: {
                let v = jstr(row, "verification");
                if v.is_empty() {
                    "unverified".to_string()
                } else {
                    v
                }
            },
            next_work: jstr(row, "next_work"),
            provider: jopt_str(row, "provider"),
            model: jopt_str(row, "model"),
            earn_atoms: jf64(row, "earn_atoms"),
            lean_source: jstr(row, "lean_source"),
            lean_log: jstr(row, "lean_log"),
            lean_ok: jbool(row, "lean_ok"),
            neighborhood: jjson(row, "neighborhood"),
            neighbors: jjson(row, "neighbors"),
            openings: jjson(row, "openings"),
        }
    }
}

impl Entity for Page {
    type Service = ();

    fn resource_name() -> &'static str {
        "pages"
    }

    fn resource_name_singular() -> &'static str {
        "page"
    }

    fn service_from_host(
        _host: &Arc<dyn std::any::Any + Send + Sync>,
    ) -> Result<Arc<Self::Service>> {
        Ok(Arc::new(()))
    }

    fn id(&self) -> Uuid {
        self.uuid
    }

    fn entity_type(&self) -> &str {
        &self.entity_type
    }

    fn created_at(&self) -> DateTime<Utc> {
        self.created_at
    }

    fn updated_at(&self) -> DateTime<Utc> {
        self.updated_at
    }

    fn deleted_at(&self) -> Option<DateTime<Utc>> {
        None
    }

    fn status(&self) -> &str {
        &self.status
    }
}

impl Data for Page {
    fn name(&self) -> &str {
        &self.name
    }

    fn indexed_fields() -> &'static [&'static str] {
        &["qualified_name", "status", "kind"]
    }

    fn field_value(&self, field: &str) -> Option<FieldValue> {
        match field {
            "id" => Some(FieldValue::String(self.id.clone())),
            "qualified_name" => Some(FieldValue::String(self.qualified_name.clone())),
            "name" | "title" => Some(FieldValue::String(self.title.clone())),
            "status" => Some(FieldValue::String(self.status.clone())),
            "kind" => Some(FieldValue::String(self.kind.clone())),
            "verification" => Some(FieldValue::String(self.verification.clone())),
            _ => None,
        }
    }
}

// ---------------------------------------------------------------------------
// Entité « wnode » (nœuds Problem / Lemma / Gate du store weave.obrain)
// ---------------------------------------------------------------------------

const WNODE_RETURN: &str = "n.qualified_name AS qualified_name, n.kind AS kind, \
     n.label AS label, n.statement AS statement, n.status AS status, \
     n.problemQn AS problem_qn, n.certified AS certified, n.lean_source AS lean_source";

#[derive(Debug, Clone, Serialize)]
struct Wnode {
    id: String,
    #[serde(rename = "type")]
    entity_type: String,
    name: String,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
    status: String,
    qualified_name: String,
    /// problem | lemma | gate — dérivé du label si la prop `kind` manque.
    kind: String,
    label: String,
    statement: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    problem_qn: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    certified: Option<bool>,
    lean_source: String,
    #[serde(skip)]
    uuid: Uuid,
}

impl Wnode {
    fn from_row(row: &Map<String, JVal>, default_kind: &str) -> Self {
        let qualified_name = jstr(row, "qualified_name");
        let kind = {
            let k = jstr(row, "kind");
            if k.is_empty() {
                default_kind.to_string()
            } else {
                k
            }
        };
        let label = {
            let l = jstr(row, "label");
            if l.is_empty() {
                qualified_name.clone()
            } else {
                l
            }
        };
        let now = Utc::now();
        Self {
            uuid: synth_uuid(&format!("wnode:{qualified_name}")),
            id: qualified_name.clone(),
            entity_type: "wnode".to_string(),
            name: label.clone(),
            created_at: now,
            updated_at: now,
            status: {
                let s = jstr(row, "status");
                if s.is_empty() { kind.clone() } else { s }
            },
            qualified_name,
            kind,
            label,
            statement: jstr(row, "statement"),
            problem_qn: jopt_str(row, "problem_qn"),
            certified: jbool(row, "certified"),
            lean_source: jstr(row, "lean_source"),
        }
    }
}

impl Entity for Wnode {
    type Service = ();

    fn resource_name() -> &'static str {
        "wnodes"
    }

    fn resource_name_singular() -> &'static str {
        "wnode"
    }

    fn service_from_host(
        _host: &Arc<dyn std::any::Any + Send + Sync>,
    ) -> Result<Arc<Self::Service>> {
        Ok(Arc::new(()))
    }

    fn id(&self) -> Uuid {
        self.uuid
    }

    fn entity_type(&self) -> &str {
        &self.entity_type
    }

    fn created_at(&self) -> DateTime<Utc> {
        self.created_at
    }

    fn updated_at(&self) -> DateTime<Utc> {
        self.updated_at
    }

    fn deleted_at(&self) -> Option<DateTime<Utc>> {
        None
    }

    fn status(&self) -> &str {
        &self.status
    }
}

impl Data for Wnode {
    fn name(&self) -> &str {
        &self.name
    }

    fn indexed_fields() -> &'static [&'static str] {
        &["qualified_name", "kind", "status"]
    }

    fn field_value(&self, field: &str) -> Option<FieldValue> {
        match field {
            "qualified_name" => Some(FieldValue::String(self.qualified_name.clone())),
            "name" | "label" => Some(FieldValue::String(self.label.clone())),
            "kind" => Some(FieldValue::String(self.kind.clone())),
            "status" => Some(FieldValue::String(self.status.clone())),
            _ => None,
        }
    }
}

// ---------------------------------------------------------------------------
// Lectures : pages
// ---------------------------------------------------------------------------

struct PageSet {
    store: Arc<ObrainStore>,
}

impl PageSet {
    fn all(&self) -> Result<Vec<Page>> {
        let query = format!("MATCH (n:Page) RETURN {PAGE_RETURN}");
        Ok(obrain_rows(&self.store, &query)?
            .iter()
            .map(Page::from_row)
            .collect())
    }

    fn get(&self, id: &str) -> Result<Option<Page>> {
        let want = id.strip_prefix("entity:").unwrap_or(id);
        Ok(self.all()?.into_iter().find(|p| p.id == want))
    }

    fn by_uuid(&self, uuid: &Uuid) -> Result<Option<Page>> {
        Ok(self.all()?.into_iter().find(|p| p.uuid == *uuid))
    }

    fn by_status(&self, status: &str) -> Result<Vec<Page>> {
        Ok(self
            .all()?
            .into_iter()
            .filter(|p| p.status == status)
            .collect())
    }
}

// ---------------------------------------------------------------------------
// Lectures : weave (problèmes / lemmes / portes + arêtes natives)
// ---------------------------------------------------------------------------

struct WnodeSet {
    store: Arc<ObrainStore>,
}

impl WnodeSet {
    fn by_label(&self, label: &str, default_kind: &str) -> Result<Vec<Wnode>> {
        let query = format!("MATCH (n:{label}) RETURN {WNODE_RETURN}");
        Ok(obrain_rows(&self.store, &query)?
            .iter()
            .map(|r| Wnode::from_row(r, default_kind))
            .collect())
    }

    /// Tous les nœuds du tissage : `Problem` + `Lemma` + `Gate`.
    fn all(&self) -> Result<Vec<Wnode>> {
        let mut out = self.by_label("Problem", "problem")?;
        out.extend(self.by_label("Lemma", "lemma")?);
        out.extend(self.by_label("Gate", "gate")?);
        Ok(out)
    }

    fn get(&self, qn: &str) -> Result<Option<Wnode>> {
        Ok(self.all()?.into_iter().find(|w| w.qualified_name == qn))
    }

    fn by_uuid(&self, uuid: &Uuid) -> Result<Option<Wnode>> {
        Ok(self.all()?.into_iter().find(|w| w.uuid == *uuid))
    }

    /// Arêtes natives du store : LEMMA (lemme→problème), GATE (porte→problème),
    /// IMPLIES (lemme→porte). NB : les noms réels sont `LEMMA`/`GATE`/`IMPLIES`,
    /// pas `LEMMA_OF`/`GATE_OF` comme dans `examples/weave_pilot.rs`.
    fn edges(&self, edge_type: &str) -> Result<Vec<(String, String)>> {
        let (src, dst) = match edge_type {
            "LEMMA" => ("Lemma", "Problem"),
            "GATE" => ("Gate", "Problem"),
            "IMPLIES" => ("Lemma", "Gate"),
            other => bail!("type d'arete inconnu: {other}"),
        };
        let query = format!(
            "MATCH (a:{src})-[r:{edge_type}]->(b:{dst}) \
             RETURN a.qualified_name AS from_qn, b.qualified_name AS to_qn"
        );
        Ok(obrain_rows(&self.store, &query)?
            .iter()
            .map(|r| (jstr(r, "from_qn"), jstr(r, "to_qn")))
            .collect())
    }

    /// Voisinage d'un nœud : `{lemme: [...], porte: [...], implique: [...]}`
    /// selon l'orientation réelle des arêtes du store.
    fn neighborhood(&self, qn: &str) -> Result<JVal> {
        let lemmas = self.edges("LEMMA")?;
        let gates = self.edges("GATE")?;
        let implies = self.edges("IMPLIES")?;

        let outgoing = |edges: &[(String, String)]| -> Vec<String> {
            edges
                .iter()
                .filter(|(f, _)| f == qn)
                .map(|(_, t)| t.clone())
                .collect()
        };
        let incoming = |edges: &[(String, String)]| -> Vec<String> {
            edges
                .iter()
                .filter(|(_, t)| t == qn)
                .map(|(f, _)| f.clone())
                .collect()
        };

        Ok(json!({
            "problemQn": qn,
            "lemmas": incoming(&lemmas),
            "gates": incoming(&gates),
            "implies": outgoing(&implies),
            "lemma_of": outgoing(&lemmas),
            "gate_of": outgoing(&gates),
        }))
    }
}

// ---------------------------------------------------------------------------
// Fetchers (GraphQL + enrichissement des liens this-rs)
// ---------------------------------------------------------------------------

struct PageFetcher {
    pages: Arc<PageSet>,
}

#[async_trait]
impl EntityFetcher for PageFetcher {
    async fn fetch_as_json(&self, entity_id: &Uuid) -> Result<JVal> {
        self.pages
            .by_uuid(entity_id)?
            .map(|p| serde_json::to_value(&p).unwrap_or(JVal::Null))
            .ok_or_else(|| anyhow!("page inconnue: {entity_id}"))
    }

    async fn get_sample_entity(&self) -> Result<JVal> {
        Ok(self
            .pages
            .all()?
            .first()
            .map(|p| serde_json::to_value(p).unwrap_or(json!({})))
            .unwrap_or_else(|| json!({})))
    }

    async fn list_as_json(&self, limit: Option<i32>, offset: Option<i32>) -> Result<Vec<JVal>> {
        let mut all: Vec<JVal> = self
            .pages
            .all()?
            .iter()
            .map(|p| serde_json::to_value(p).unwrap_or(JVal::Null))
            .collect();
        let offset = offset.unwrap_or(0).max(0) as usize;
        if offset > 0 {
            all.drain(..offset.min(all.len()));
        }
        if let Some(l) = limit
            && l > 0
        {
            all.truncate(l as usize);
        }
        Ok(all)
    }
}

struct WnodeFetcher {
    wnodes: Arc<WnodeSet>,
}

#[async_trait]
impl EntityFetcher for WnodeFetcher {
    async fn fetch_as_json(&self, entity_id: &Uuid) -> Result<JVal> {
        self.wnodes
            .by_uuid(entity_id)?
            .map(|w| serde_json::to_value(&w).unwrap_or(JVal::Null))
            .ok_or_else(|| anyhow!("wnode inconnu: {entity_id}"))
    }

    async fn get_sample_entity(&self) -> Result<JVal> {
        Ok(self
            .wnodes
            .all()?
            .first()
            .map(|w| serde_json::to_value(w).unwrap_or(json!({})))
            .unwrap_or_else(|| json!({})))
    }

    async fn list_as_json(&self, limit: Option<i32>, offset: Option<i32>) -> Result<Vec<JVal>> {
        let mut all: Vec<JVal> = self
            .wnodes
            .all()?
            .iter()
            .map(|w| serde_json::to_value(w).unwrap_or(JVal::Null))
            .collect();
        let offset = offset.unwrap_or(0).max(0) as usize;
        if offset > 0 {
            all.drain(..offset.min(all.len()));
        }
        if let Some(l) = limit
            && l > 0
        {
            all.truncate(l as usize);
        }
        Ok(all)
    }
}

// ---------------------------------------------------------------------------
// Routes REST
// ---------------------------------------------------------------------------

#[derive(Debug, serde::Deserialize)]
struct ListQuery {
    page: Option<usize>,
    limit: Option<usize>,
    status: Option<String>,
}

fn oops(status: StatusCode, msg: impl Into<String>) -> Response {
    (status, Json(json!({ "error": msg.into() }))).into_response()
}

async fn list_pages(
    AxumState(pages): AxumState<Arc<PageSet>>,
    AxumQuery(q): AxumQuery<ListQuery>,
) -> Response {
    let rows = match q.status.as_deref() {
        Some(s) => pages.by_status(s),
        None => pages.all(),
    };
    let rows = match rows {
        Ok(r) => r,
        Err(e) => return oops(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    };
    let total = rows.len();
    let limit = q.limit.unwrap_or(20).clamp(1, 500);
    let page = q.page.unwrap_or(1).max(1);
    let start = ((page - 1) * limit).min(total);
    let end = (start + limit).min(total);
    let data: Vec<&Page> = rows[start..end].iter().collect();
    let total_pages = total.div_ceil(limit);
    Json(json!({
        "data": data,
        "pagination": {
            "page": page,
            "limit": limit,
            "total": total,
            "total_pages": total_pages,
            "has_next": page < total_pages,
            "has_prev": page > 1,
        }
    }))
    .into_response()
}

async fn get_page(
    AxumState(pages): AxumState<Arc<PageSet>>,
    AxumPath(id): AxumPath<String>,
) -> Response {
    match pages.get(&id) {
        Ok(Some(p)) => Json(p).into_response(),
        Ok(None) => oops(StatusCode::NOT_FOUND, format!("page inconnue: {id}")),
        Err(e) => oops(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

/// Voisinage déclaré d'une page : la prop `neighbors` du store (le store
/// entity.obrain n'a aucune arête native — `substrate.edges` est vide).
async fn page_neighbors(
    AxumState(pages): AxumState<Arc<PageSet>>,
    AxumPath(id): AxumPath<String>,
) -> Response {
    match pages.get(&id) {
        Ok(Some(p)) => Json(json!({ "id": p.id, "neighbors": p.neighbors })).into_response(),
        Ok(None) => oops(StatusCode::NOT_FOUND, format!("page inconnue: {id}")),
        Err(e) => oops(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

struct PageDescriptor {
    pages: Arc<PageSet>,
}

impl EntityDescriptor for PageDescriptor {
    fn entity_type(&self) -> &str {
        "page"
    }
    fn plural(&self) -> &str {
        "pages"
    }
    fn build_routes(&self) -> Router {
        Router::new()
            .route("/pages", get(list_pages))
            .route("/pages/{id}", get(get_page))
            .route("/pages/{id}/neighbors", get(page_neighbors))
            .with_state(self.pages.clone())
    }
}

async fn list_wnodes(
    AxumState(w): AxumState<Arc<WnodeSet>>,
    AxumQuery(q): AxumQuery<ListQuery>,
) -> Response {
    let rows = match w.all() {
        Ok(r) => r,
        Err(e) => return oops(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    };
    let filtered: Vec<Wnode> = match q.status.as_deref() {
        Some(k) => rows.into_iter().filter(|n| n.kind == k).collect(),
        None => rows,
    };
    let total = filtered.len();
    let limit = q.limit.unwrap_or(50).clamp(1, 500);
    let page = q.page.unwrap_or(1).max(1);
    let start = ((page - 1) * limit).min(total);
    let end = (start + limit).min(total);
    Json(json!({
        "data": &filtered[start..end],
        "pagination": {
            "page": page,
            "limit": limit,
            "total": total,
            "total_pages": total.div_ceil(limit),
        }
    }))
    .into_response()
}

async fn get_wnode(
    AxumState(w): AxumState<Arc<WnodeSet>>,
    AxumPath(qn): AxumPath<String>,
) -> Response {
    match w.get(&qn) {
        Ok(Some(node)) => {
            let edges = w.neighborhood(&qn).unwrap_or(JVal::Null);
            Json(json!({ "node": node, "edges": edges })).into_response()
        }
        Ok(None) => oops(StatusCode::NOT_FOUND, format!("wnode inconnu: {qn}")),
        Err(e) => oops(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

async fn wnode_links(
    AxumState(w): AxumState<Arc<WnodeSet>>,
    AxumPath(qn): AxumPath<String>,
) -> Response {
    match w.neighborhood(&qn) {
        Ok(v) => Json(v).into_response(),
        Err(e) => oops(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

struct WnodeDescriptor {
    wnodes: Arc<WnodeSet>,
}

impl EntityDescriptor for WnodeDescriptor {
    fn entity_type(&self) -> &str {
        "wnode"
    }
    fn plural(&self) -> &str {
        "wnodes"
    }
    fn build_routes(&self) -> Router {
        Router::new()
            .route("/wnodes", get(list_wnodes))
            .route("/wnodes/{qn}", get(get_wnode))
            .route("/wnodes/{qn}/links", get(wnode_links))
            .with_state(self.wnodes.clone())
    }
}

// ---------------------------------------------------------------------------
// Supervision : /healthz/store
// ---------------------------------------------------------------------------

struct HealthState {
    store_path: PathBuf,
    weave_path: Option<PathBuf>,
    read_only: bool,
    pages: Arc<PageSet>,
    wnodes: Option<Arc<WnodeSet>>,
}

async fn healthz_store(AxumState(h): AxumState<Arc<HealthState>>) -> Response {
    let pages = h.pages.all();
    let page_count = pages.as_ref().map(|v| v.len()).unwrap_or(0);
    let weave_count = match &h.wnodes {
        Some(w) => w.all().map(|v| v.len()).unwrap_or(0),
        None => 0,
    };
    let err = pages.err().map(|e| e.to_string());
    let status = if err.is_none() { "ok" } else { "degraded" };
    (
        if err.is_none() {
            StatusCode::OK
        } else {
            StatusCode::SERVICE_UNAVAILABLE
        },
        Json(json!({
            "status": status,
            "service": "aletheia-server",
            "protocols": ["rest", "graphql", "grpc", "websocket", "sse", "mcp"],
            "read_only": h.read_only,
            "entity_store": h.store_path.display().to_string(),
            "weave_store": h.weave_path.as_ref().map(|p| p.display().to_string()),
            "pages": page_count,
            "wnodes": weave_count,
            "error": err,
        })),
    )
        .into_response()
}

// ---------------------------------------------------------------------------
// MCP — Model Context Protocol (JSON-RPC 2.0 sur HTTP), chemin `/mcp`
// ---------------------------------------------------------------------------
//
// Transport « Streamable HTTP » en mode **sans session** : chaque `POST /mcp`
// est une requête JSON-RPC 2.0 autonome, répondue par un unique document JSON
// (`application/json`) — pas d'en-tête `Mcp-Session-Id`, pas de flux SSE
// serveur→client (un serveur sans session est explicitement autorisé par la
// spec MCP). `GET /mcp` renvoie un document d'information non normatif, utile
// aux sondes de supervision et à l'exploration humaine.
//
// Tous les outils sont en LECTURE : `tools/list` n'annonce que des lectures et
// `tools/call` refuse tout outil hors de cette liste (-32602). Aucune capacité
// d'écriture n'est déclarée dans `capabilities` (seul `tools` l'est) : les
// méthodes `resources/*`, `prompts/*`, `sampling/*` et `completion/*`
// répondent -32601.

const MCP_PROTOCOL_VERSION: &str = "2025-06-18";
/// Versions du protocole que ce serveur sait servir (négociation `initialize`).
const MCP_SUPPORTED_VERSIONS: &[&str] = &["2024-11-05", "2025-03-26", "2025-06-18"];
const MCP_SERVER_NAME: &str = "aletheia-this-rs";
const MCP_SERVER_VERSION: &str = env!("CARGO_PKG_VERSION");
const MCP_METHODS: &[&str] = &["initialize", "ping", "tools/list", "tools/call"];

// Codes d'erreur JSON-RPC 2.0.
const RPC_PARSE_ERROR: i64 = -32700;
const RPC_INVALID_REQUEST: i64 = -32600;
const RPC_METHOD_NOT_FOUND: i64 = -32601;
const RPC_INVALID_PARAMS: i64 = -32602;

struct McpState {
    pages: Arc<PageSet>,
    wnodes: Option<Arc<WnodeSet>>,
    store_path: PathBuf,
    weave_path: Option<PathBuf>,
    /// Vrai si le store a été ouvert par `ObrainDB::open_read_only` (`--read-only`).
    strict_read_only: bool,
}

/// Erreur de haut niveau d'un outil : la distinction pilote la réponse MCP —
/// `Invalid` ⇒ erreur JSON-RPC `-32602`, `Failed` ⇒ résultat `isError: true`.
#[derive(Debug)]
enum ToolErr {
    Invalid(String),
    Failed(String),
}

// ── Description des outils (source unique de vérité : `tools/list`) ─────────

fn mcp_tools() -> Vec<JVal> {
    vec![
        json!({
            "name": "aletheia_pages_list",
            "title": "Lister les pages d'aletheia",
            "description": "Liste paginee des pages du store entity.obrain (noeuds de label `Page`). \
        Renvoie un resume compact par defaut (id, titre, kind, statut, verification, updated_at) ; \
        `full: true` renvoie chaque page complete. Lecture seule.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "status": {
                        "type": "string",
                        "description": "Filtre exact sur le statut de la page (ex. \"open\")."
                    },
                    "limit": {
                        "type": "integer", "minimum": 1, "maximum": 500, "default": 20,
                        "description": "Nombre maximum de pages renvoyees."
                    },
                    "offset": {
                        "type": "integer", "minimum": 0, "default": 0,
                        "description": "Decalage dans la liste filtree."
                    },
                    "full": {
                        "type": "boolean", "default": false,
                        "description": "true = objets page complets ; false = resume compact."
                    }
                },
                "additionalProperties": false
            }
        }),
        json!({
            "name": "aletheia_page_get",
            "title": "Lire une page d'aletheia",
            "description": "Renvoie une page par son identifiant aletheia (slug, ex. \"mp_hodge\"). \
        Les champs `lean_source` et `lean_log` (volumineux) sont omis sauf `include_lean: true`. Lecture seule.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "id": {
                        "type": "string", "minLength": 1,
                        "description": "Id de la page : slug aletheia (\"mp_hodge\") ou qualified_name (\"entity:mp_hodge\")."
                    },
                    "include_lean": {
                        "type": "boolean", "default": false,
                        "description": "true = inclut lean_source et lean_log."
                    }
                },
                "required": ["id"],
                "additionalProperties": false
            }
        }),
        json!({
            "name": "aletheia_weave_list",
            "title": "Lister les noeuds du tissage",
            "description": "Liste les noeuds du store weave.obrain (labels `Problem`, `Lemma`, `Gate`), \
        avec filtre optionnel par `kind`. Si ce gateway n'a pas charge de store weave, l'outil renvoie une erreur \
        d'outil (`isError: true`) expliquant pourquoi. Lecture seule.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "kind": {
                        "type": "string", "enum": ["problem", "lemma", "gate"],
                        "description": "Ne garder que ce type de noeud."
                    },
                    "limit": {
                        "type": "integer", "minimum": 1, "maximum": 500, "default": 50,
                        "description": "Nombre maximum de noeuds renvoyes."
                    },
                    "offset": {
                        "type": "integer", "minimum": 0, "default": 0,
                        "description": "Decalage dans la liste filtree."
                    }
                },
                "additionalProperties": false
            }
        }),
        json!({
            "name": "aletheia_weave_get",
            "title": "Lire un noeud du tissage",
            "description": "Renvoie un noeud du store weave.obrain par son `qualified_name`, \
        avec son voisinage d'aretes natives (LEMMA, GATE, IMPLIES). Lecture seule.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "qualified_name": {
                        "type": "string", "minLength": 1,
                        "description": "qualified_name du noeud (ex. \"problem:mp_pnp\")."
                    },
                    "include_lean": {
                        "type": "boolean", "default": false,
                        "description": "true = inclut lean_source."
                    }
                },
                "required": ["qualified_name"],
                "additionalProperties": false
            }
        }),
        json!({
            "name": "aletheia_store_stats",
            "title": "Statistiques des stores",
            "description": "Comptes de noeuds et d'aretes par label des stores ouverts, chemins des stores, \
        mode lecture seule et protocoles exposes. Aucun argument. Lecture seule.",
            "inputSchema": {
                "type": "object",
                "properties": {},
                "additionalProperties": false
            }
        }),
    ]
}

/// Noms d'outils, dérivés de `mcp_tools()` (aucune liste parallèle à maintenir).
fn mcp_tool_names() -> Vec<String> {
    mcp_tools()
        .iter()
        .filter_map(|t| t.get("name").and_then(|v| v.as_str()).map(str::to_string))
        .collect()
}

// ── Validation des arguments (miroir exact des `inputSchema`) ───────────────

fn check_args(args: &Map<String, JVal>, allowed: &[&str]) -> Result<(), ToolErr> {
    if let Some(k) = args.keys().find(|k| !allowed.contains(&k.as_str())) {
        let liste = if allowed.is_empty() {
            "(aucun)".to_string()
        } else {
            allowed.join(", ")
        };
        return Err(ToolErr::Invalid(format!(
            "argument inconnu `{k}` (arguments supportes: {liste})"
        )));
    }
    Ok(())
}

fn arg_usize(
    args: &Map<String, JVal>,
    key: &str,
    default: usize,
    min: usize,
    max: usize,
) -> Result<usize, ToolErr> {
    match args.get(key) {
        None | Some(JVal::Null) => Ok(default),
        Some(JVal::Number(n)) => {
            let v = n
                .as_i64()
                .ok_or_else(|| ToolErr::Invalid(format!("`{key}` doit etre un entier")))?;
            if v < min as i64 || v > max as i64 {
                return Err(ToolErr::Invalid(format!(
                    "`{key}` hors bornes [{min}, {max}] (recu: {v})"
                )));
            }
            Ok(v as usize)
        }
        Some(_) => Err(ToolErr::Invalid(format!("`{key}` doit etre un entier"))),
    }
}

fn arg_bool(args: &Map<String, JVal>, key: &str, default: bool) -> Result<bool, ToolErr> {
    match args.get(key) {
        None | Some(JVal::Null) => Ok(default),
        Some(JVal::Bool(b)) => Ok(*b),
        Some(_) => Err(ToolErr::Invalid(format!("`{key}` doit etre un booleen"))),
    }
}

fn arg_str(args: &Map<String, JVal>, key: &str) -> Result<Option<String>, ToolErr> {
    match args.get(key) {
        None | Some(JVal::Null) => Ok(None),
        Some(JVal::String(s)) if !s.trim().is_empty() => Ok(Some(s.trim().to_string())),
        Some(JVal::String(_)) => Err(ToolErr::Invalid(format!("`{key}` ne doit pas etre vide"))),
        Some(_) => Err(ToolErr::Invalid(format!("`{key}` doit etre une chaine"))),
    }
}

fn arg_required_str(args: &Map<String, JVal>, key: &str) -> Result<String, ToolErr> {
    arg_str(args, key)?
        .ok_or_else(|| ToolErr::Invalid(format!("argument requis manquant: `{key}`")))
}

/// Fenêtre `[offset, offset + limit)` d'une liste déjà filtrée.
fn window(total: usize, offset: usize, limit: usize) -> (usize, usize) {
    let start = offset.min(total);
    (start, (start + limit).min(total))
}

// ── Implémentation des outils (lecture seule) ───────────────────────────────

/// Résumé compact d'une page, ou objet complet si `full`.
fn page_json(p: &Page, full: bool) -> JVal {
    if full {
        return serde_json::to_value(p).unwrap_or(JVal::Null);
    }
    json!({
        "id": p.id,
        "title": p.title,
        "kind": p.kind,
        "status": p.status,
        "verification": p.verification,
        "earn_atoms": p.earn_atoms,
        "updated_at": p.updated_at.to_rfc3339(),
    })
}

fn wnode_json(w: &Wnode, include_lean: bool) -> JVal {
    let mut v = serde_json::to_value(w).unwrap_or(JVal::Null);
    if !include_lean && let Some(obj) = v.as_object_mut() {
        obj.remove("lean_source");
        obj.insert("lean_omitted".to_string(), JVal::Bool(true));
    }
    v
}

fn tool_pages_list(mcp: &McpState, args: &Map<String, JVal>) -> Result<JVal, ToolErr> {
    check_args(args, &["status", "limit", "offset", "full"])?;
    let status = arg_str(args, "status")?;
    let limit = arg_usize(args, "limit", 20, 1, 500)?;
    let offset = arg_usize(args, "offset", 0, 0, 1_000_000)?;
    let full = arg_bool(args, "full", false)?;

    let all = mcp
        .pages
        .all()
        .map_err(|e| ToolErr::Failed(format!("lecture du store entity impossible: {e}")))?;
    let total = all.len();
    let matched: Vec<Page> = match &status {
        Some(s) => all.into_iter().filter(|p| p.status == *s).collect(),
        None => all,
    };
    let matched_count = matched.len();
    let (start, end) = window(matched_count, offset, limit);
    let data: Vec<JVal> = matched[start..end]
        .iter()
        .map(|p| page_json(p, full))
        .collect();

    Ok(json!({
        "store": mcp.store_path.display().to_string(),
        "total": total,
        "matched": matched_count,
        "offset": start,
        "limit": limit,
        "count": data.len(),
        "full": full,
        "data": data,
        "read_only": true,
    }))
}

fn tool_page_get(mcp: &McpState, args: &Map<String, JVal>) -> Result<JVal, ToolErr> {
    check_args(args, &["id", "include_lean"])?;
    let id = arg_required_str(args, "id")?;
    let include_lean = arg_bool(args, "include_lean", false)?;

    let page = mcp
        .pages
        .get(&id)
        .map_err(|e| ToolErr::Failed(format!("lecture du store entity impossible: {e}")))?
        .ok_or_else(|| ToolErr::Failed(format!("page inconnue: {id}")))?;

    let mut v = serde_json::to_value(&page)
        .map_err(|e| ToolErr::Failed(format!("serialisation de la page impossible: {e}")))?;
    if !include_lean && let Some(obj) = v.as_object_mut() {
        obj.remove("lean_source");
        obj.remove("lean_log");
        obj.insert("lean_omitted".to_string(), JVal::Bool(true));
    }
    Ok(v)
}

fn tool_weave_list(mcp: &McpState, args: &Map<String, JVal>) -> Result<JVal, ToolErr> {
    check_args(args, &["kind", "limit", "offset"])?;
    let kind = arg_str(args, "kind")?;
    if let Some(k) = &kind
        && !matches!(k.as_str(), "problem" | "lemma" | "gate")
    {
        return Err(ToolErr::Invalid(format!(
            "`kind` doit valoir problem, lemma ou gate (recu: {k})"
        )));
    }
    let limit = arg_usize(args, "limit", 50, 1, 500)?;
    let offset = arg_usize(args, "offset", 0, 0, 1_000_000)?;

    let wset = mcp.wnodes.as_ref().ok_or_else(|| {
        ToolErr::Failed(
            "aucun store weave charge par ce gateway (lancement avec --no-weave, ou weave.obrain \
             absent a cote du store entity) : l'outil aletheia_weave_list est indisponible"
                .to_string(),
        )
    })?;

    let all = wset
        .all()
        .map_err(|e| ToolErr::Failed(format!("lecture du store weave impossible: {e}")))?;
    let total = all.len();
    let matched: Vec<Wnode> = match &kind {
        Some(k) => all.into_iter().filter(|w| w.kind == *k).collect(),
        None => all,
    };
    let matched_count = matched.len();
    let (start, end) = window(matched_count, offset, limit);
    let data: Vec<JVal> = matched[start..end]
        .iter()
        .map(|w| wnode_json(w, false))
        .collect();

    Ok(json!({
        "store": mcp.weave_path.as_ref().map(|p| p.display().to_string()),
        "total": total,
        "matched": matched_count,
        "kind": kind,
        "offset": start,
        "limit": limit,
        "count": data.len(),
        "data": data,
        "read_only": true,
    }))
}

fn tool_weave_get(mcp: &McpState, args: &Map<String, JVal>) -> Result<JVal, ToolErr> {
    check_args(args, &["qualified_name", "include_lean"])?;
    let qn = arg_required_str(args, "qualified_name")?;
    let include_lean = arg_bool(args, "include_lean", false)?;

    let wset = mcp.wnodes.as_ref().ok_or_else(|| {
        ToolErr::Failed(
            "aucun store weave charge par ce gateway (lancement avec --no-weave, ou weave.obrain \
             absent a cote du store entity) : l'outil aletheia_weave_get est indisponible"
                .to_string(),
        )
    })?;

    let node = wset
        .get(&qn)
        .map_err(|e| ToolErr::Failed(format!("lecture du store weave impossible: {e}")))?
        .ok_or_else(|| ToolErr::Failed(format!("noeud de tissage inconnu: {qn}")))?;
    let edges = wset.neighborhood(&qn).unwrap_or(JVal::Null);

    Ok(json!({
        "node": wnode_json(&node, include_lean),
        "edges": edges,
        "read_only": true,
    }))
}

fn tool_store_stats(mcp: &McpState, args: &Map<String, JVal>) -> Result<JVal, ToolErr> {
    check_args(args, &[])?;

    let pages = mcp
        .pages
        .all()
        .map_err(|e| ToolErr::Failed(format!("lecture du store entity impossible: {e}")))?
        .len();

    let mut weave_nodes = Map::new();
    let mut weave_edges = Map::new();
    if let Some(w) = &mcp.wnodes {
        for (label, default_kind) in [("Problem", "problem"), ("Lemma", "lemma"), ("Gate", "gate")]
        {
            let n = w
                .by_label(label, default_kind)
                .map_err(|e| ToolErr::Failed(format!("lecture du store weave impossible: {e}")))?
                .len();
            weave_nodes.insert(label.to_lowercase(), json!(n));
        }
        for edge in ["LEMMA", "GATE", "IMPLIES"] {
            let n = w
                .edges(edge)
                .map_err(|e| ToolErr::Failed(format!("lecture des aretes weave impossible: {e}")))?
                .len();
            weave_edges.insert(edge.to_lowercase(), json!(n));
        }
    }

    Ok(json!({
        "service": MCP_SERVER_NAME,
        "entity_store": mcp.store_path.display().to_string(),
        "weave_store": mcp.weave_path.as_ref().map(|p| p.display().to_string()),
        "weave_loaded": mcp.wnodes.is_some(),
        "read_only": true,
        "strict_read_only": mcp.strict_read_only,
        "pages": pages,
        "weave_nodes": weave_nodes,
        "weave_edges": weave_edges,
        "protocols": ["rest", "graphql", "grpc", "websocket", "sse", "mcp"],
        "mcp": {
            "protocolVersion": MCP_PROTOCOL_VERSION,
            "transport": "http",
            "endpoint": "/mcp",
            "tools": mcp_tool_names(),
        },
    }))
}

fn mcp_run_tool(mcp: &McpState, name: &str, args: &Map<String, JVal>) -> Result<JVal, ToolErr> {
    match name {
        "aletheia_pages_list" => tool_pages_list(mcp, args),
        "aletheia_page_get" => tool_page_get(mcp, args),
        "aletheia_weave_list" => tool_weave_list(mcp, args),
        "aletheia_weave_get" => tool_weave_get(mcp, args),
        "aletheia_store_stats" => tool_store_stats(mcp, args),
        other => Err(ToolErr::Invalid(format!("outil inconnu: {other}"))),
    }
}

// ── Enveloppes JSON-RPC 2.0 ─────────────────────────────────────────────────

fn rpc_result(id: JVal, result: JVal) -> JVal {
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

fn rpc_error(id: JVal, code: i64, message: &str, data: Option<JVal>) -> JVal {
    let mut err = json!({ "code": code, "message": message });
    if let Some(d) = data
        && let Some(obj) = err.as_object_mut()
    {
        obj.insert("data".to_string(), d);
    }
    json!({ "jsonrpc": "2.0", "id": id, "error": err })
}

fn rpc_json(body: JVal) -> Response {
    Json(body).into_response()
}

/// Négociation de version : on sert la version demandée si on la connaît,
/// sinon la nôtre (la spec impose alors que le client raccroche ou s'aligne).
fn mcp_initialize(params: &JVal) -> JVal {
    let requested = params.get("protocolVersion").and_then(|v| v.as_str());
    let protocol_version = match requested {
        Some(v) if MCP_SUPPORTED_VERSIONS.contains(&v) => v.to_string(),
        _ => MCP_PROTOCOL_VERSION.to_string(),
    };
    json!({
        "protocolVersion": protocol_version,
        "capabilities": {
            "tools": {}
        },
        "serverInfo": {
            "name": MCP_SERVER_NAME,
            "title": "aletheia this-rs gateway (obrain, lecture seule)",
            "version": MCP_SERVER_VERSION,
        },
        "instructions": "Gateway this-rs en LECTURE SEULE sur les stores obrain d'aletheia \
    (REST, GraphQL, gRPC, WebSocket, SSE et MCP sur le meme port). Les outils exposes par \
    tools/list interrogent les noeuds `Page` du store entity et `Problem`/`Lemma`/`Gate` du \
    store weave ; aucun outil n'ecrit, aucune methode d'ecriture n'est disponible.",
    })
}

fn mcp_tools_call(mcp: &McpState, id: JVal, params: &JVal) -> JVal {
    let Some(pobj) = params.as_object() else {
        return rpc_error(
            id,
            RPC_INVALID_PARAMS,
            "params doit etre un objet JSON",
            None,
        );
    };
    let Some(name) = pobj.get("name").and_then(|v| v.as_str()) else {
        return rpc_error(
            id,
            RPC_INVALID_PARAMS,
            "params.name manquant (chaine attendue)",
            None,
        );
    };
    if !mcp_tool_names().iter().any(|n| n == name) {
        return rpc_error(
            id,
            RPC_INVALID_PARAMS,
            &format!("outil inconnu: {name}"),
            Some(json!({ "tools": mcp_tool_names() })),
        );
    }
    let args: Map<String, JVal> = match pobj.get("arguments") {
        None | Some(JVal::Null) => Map::new(),
        Some(JVal::Object(o)) => o.clone(),
        Some(_) => {
            return rpc_error(
                id,
                RPC_INVALID_PARAMS,
                "params.arguments doit etre un objet JSON",
                None,
            );
        }
    };

    match mcp_run_tool(mcp, name, &args) {
        Err(ToolErr::Invalid(msg)) => {
            rpc_error(id, RPC_INVALID_PARAMS, &msg, Some(json!({ "tool": name })))
        }
        // Echec d'execution : resultat d'outil en erreur (spec MCP `isError`),
        // pas une erreur de protocole.
        Err(ToolErr::Failed(msg)) => rpc_result(
            id,
            json!({
                "content": [{ "type": "text", "text": msg }],
                "isError": true,
            }),
        ),
        Ok(value) => {
            let text = serde_json::to_string_pretty(&value).unwrap_or_else(|_| "null".to_string());
            rpc_result(
                id,
                json!({
                    "content": [{ "type": "text", "text": text }],
                    "structuredContent": value,
                    "isError": false,
                }),
            )
        }
    }
}

// ── Handlers HTTP ───────────────────────────────────────────────────────────

/// `GET /mcp` — document d'information (hors spec MCP, sans effet de bord).
async fn mcp_info(AxumState(mcp): AxumState<Arc<McpState>>) -> Response {
    rpc_json(json!({
        "name": MCP_SERVER_NAME,
        "protocol": "mcp",
        "transport": "http",
        "endpoint": "/mcp",
        "protocolVersion": MCP_PROTOCOL_VERSION,
        "supportedProtocolVersions": MCP_SUPPORTED_VERSIONS,
        "methods": MCP_METHODS,
        "tools": mcp_tool_names(),
        "read_only": true,
        "strict_read_only": mcp.strict_read_only,
        "entity_store": mcp.store_path.display().to_string(),
        "weave_store": mcp.weave_path.as_ref().map(|p| p.display().to_string()),
        "weave_loaded": mcp.wnodes.is_some(),
        "protocols": ["rest", "graphql", "grpc", "websocket", "sse", "mcp"],
        "usage": {
            "GET": "ce document d'information",
            "POST": "JSON-RPC 2.0 : initialize, ping, tools/list, tools/call",
        },
    }))
}

/// `POST /mcp` — une requête JSON-RPC 2.0, une réponse JSON.
///
/// Une requête sans `id` est une notification : réponse `202 Accepted` vide
/// (cas de `notifications/initialized` envoyé par les clients MCP).
async fn mcp_post(AxumState(mcp): AxumState<Arc<McpState>>, body: Bytes) -> Response {
    let req: JVal = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(e) => {
            return rpc_json(rpc_error(
                JVal::Null,
                RPC_PARSE_ERROR,
                "parse error: corps JSON invalide",
                Some(json!(e.to_string())),
            ));
        }
    };

    let Some(obj) = req.as_object() else {
        return rpc_json(rpc_error(
            JVal::Null,
            RPC_INVALID_REQUEST,
            "invalid request: un objet JSON-RPC 2.0 est attendu (les lots JSON-RPC ne sont pas supportes par MCP)",
            None,
        ));
    };

    let id = obj.get("id").cloned();
    let Some(method) = obj
        .get("method")
        .and_then(|m| m.as_str())
        .map(str::to_string)
    else {
        return rpc_json(rpc_error(
            id.unwrap_or(JVal::Null),
            RPC_INVALID_REQUEST,
            "invalid request: champ `method` manquant",
            None,
        ));
    };
    let params = obj.get("params").cloned().unwrap_or_else(|| json!({}));

    // Notification : aucune réponse (202 Accepted, corps vide).
    let Some(id) = id else {
        return StatusCode::ACCEPTED.into_response();
    };

    let out = match method.as_str() {
        "initialize" => rpc_result(id, mcp_initialize(&params)),
        "ping" => rpc_result(id, json!({})),
        "tools/list" => rpc_result(id, json!({ "tools": mcp_tools() })),
        "tools/call" => mcp_tools_call(&mcp, id, &params),
        other => rpc_error(
            id,
            RPC_METHOD_NOT_FOUND,
            &format!("methode inconnue: {other}"),
            Some(json!({ "supported": MCP_METHODS })),
        ),
    };
    rpc_json(out)
}

// ---------------------------------------------------------------------------
// Module this-rs
// ---------------------------------------------------------------------------

fn links_config(with_weave: bool) -> Result<LinksConfig> {
    let mut yaml = String::from("entities:\n  - singular: page\n    plural: pages\n");
    if with_weave {
        yaml.push_str("  - singular: wnode\n    plural: wnodes\n");
        yaml.push_str(
            "links:\n\
             \x20 - link_type: LEMMA\n\
             \x20   source_type: wnode\n\
             \x20   target_type: wnode\n\
             \x20   forward_route_name: lemma-of\n\
             \x20   reverse_route_name: lemmas\n\
             \x20   description: \"Un lemme se reduit a son probleme\"\n\
             \x20 - link_type: GATE\n\
             \x20   source_type: wnode\n\
             \x20   target_type: wnode\n\
             \x20   forward_route_name: gate-of\n\
             \x20   reverse_route_name: gates\n\
             \x20   description: \"Une porte appartient a un probleme\"\n\
             \x20 - link_type: IMPLIES\n\
             \x20   source_type: wnode\n\
             \x20   target_type: wnode\n\
             \x20   forward_route_name: implies\n\
             \x20   reverse_route_name: implied-by\n\
             \x20   description: \"Un lemme implique une porte\"\n",
        );
    } else {
        yaml.push_str("links: []\n");
    }
    LinksConfig::from_yaml_str(&yaml).context("config de liens invalide")
}

struct AletheiaModule {
    pages: Arc<PageSet>,
    wnodes: Option<Arc<WnodeSet>>,
    config: LinksConfig,
}

impl Module for AletheiaModule {
    fn name(&self) -> &str {
        "aletheia-obrain"
    }

    fn version(&self) -> &str {
        env!("CARGO_PKG_VERSION")
    }

    fn entity_types(&self) -> Vec<&str> {
        if self.wnodes.is_some() {
            vec!["page", "wnode"]
        } else {
            vec!["page"]
        }
    }

    fn links_config(&self) -> Result<LinksConfig> {
        Ok(self.config.clone())
    }

    fn register_entities(&self, registry: &mut EntityRegistry) {
        registry.register(Box::new(PageDescriptor {
            pages: self.pages.clone(),
        }));
        if let Some(w) = &self.wnodes {
            registry.register(Box::new(WnodeDescriptor { wnodes: w.clone() }));
        }
    }

    fn get_entity_fetcher(&self, entity_type: &str) -> Option<Arc<dyn EntityFetcher>> {
        match entity_type {
            "page" => Some(Arc::new(PageFetcher {
                pages: self.pages.clone(),
            })),
            "wnode" => self
                .wnodes
                .as_ref()
                .map(|w| Arc::new(WnodeFetcher { wnodes: w.clone() }) as Arc<dyn EntityFetcher>),
            _ => None,
        }
    }

    /// Lecture seule : aucune route ni mutation d'écriture n'est exposée.
    fn get_entity_creator(&self, _entity_type: &str) -> Option<Arc<dyn EntityCreator>> {
        None
    }
}

// ---------------------------------------------------------------------------
// Ouverture du store
// ---------------------------------------------------------------------------

fn open_store(path: &Path, read_only: bool) -> Result<Arc<ObrainStore>> {
    if !path.exists() {
        bail!("store obrain introuvable: {}", path.display());
    }
    if read_only {
        let db = obrain_engine::ObrainDB::open_read_only(path)
            .map_err(|e| anyhow!("open_read_only({}) a echoue: {e}", path.display()))?;
        Ok(ObrainStore::from_db(db))
    } else {
        ObrainStore::open(path)
    }
}

/// Comptes par label — sert au mode `--probe` et au log de démarrage.
fn probe_store(store: &ObrainStore, labels: &[&str]) -> Result<Vec<(String, usize)>> {
    let mut out = Vec::new();
    for label in labels {
        let q = format!("MATCH (n:{label}) RETURN count(n) AS n");
        let n = obrain_rows(store, &q)?
            .first()
            .map(|r| jf64(r, "n") as usize)
            .unwrap_or(0);
        out.push(((*label).to_string(), n));
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// main
// ---------------------------------------------------------------------------

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| {
                // Le substrat obrain est très bavard à l'ouverture : on ne garde
                // que les logs du serveur sauf demande explicite (RUST_LOG).
                "info,obrain_substrate=warn,obrain_engine=warn,obrain_core=warn,obrain_adapters=warn"
                    .into()
            }),
        )
        .init();

    let args = parse_args()?;

    println!("aletheia_server — this-rs multi-protocoles / store obrain");
    println!("  entity store : {}", args.store.display());
    println!(
        "  weave store  : {}",
        args.weave
            .as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "(aucun)".to_string())
    );
    println!(
        "  mode         : {}",
        if args.read_only {
            "lecture seule stricte (ObrainDB::open_read_only)"
        } else {
            "lecture seule applicative (ObrainStore::open, aucune route d'ecriture)"
        }
    );

    let store = open_store(&args.store, args.read_only)?;
    let pages = Arc::new(PageSet {
        store: store.clone(),
    });

    let wnodes = match &args.weave {
        Some(p) => {
            let wstore = open_store(p, args.read_only)?;
            Some(Arc::new(WnodeSet { store: wstore }))
        }
        None => None,
    };

    // Étiquette de parité : comptes réels lus dans chaque store.
    match probe_store(&store, &["Page"]) {
        Ok(counts) => {
            for (l, n) in &counts {
                println!("  entity {l:<8} : {n}");
            }
        }
        Err(e) => println!("  ATTENTION comptes entity illisibles: {e}"),
    }
    if let Some(w) = &wnodes {
        match probe_store(&w.store, &["Problem", "Lemma", "Gate"]) {
            Ok(counts) => {
                for (l, n) in &counts {
                    println!("  weave  {l:<8} : {n}");
                }
            }
            Err(e) => println!("  ATTENTION comptes weave illisibles: {e}"),
        }
        for e in ["LEMMA", "GATE", "IMPLIES"] {
            match w.edges(e) {
                Ok(v) => println!("  aretes {e:<7} : {}", v.len()),
                Err(err) => println!("  aretes {e:<7} : erreur ({err})"),
            }
        }
    }

    if args.probe {
        let sample = pages.all()?.into_iter().next();
        match sample {
            Some(p) => println!(
                "\npage echantillon : {}\n{}",
                p.id,
                serde_json::to_string_pretty(&p)?
            ),
            None => println!("\naucune page dans le store"),
        }
        if let Some(w) = &wnodes {
            match w.all()?.into_iter().next() {
                Some(n) => println!(
                    "\nwnode echantillon : {}\n{}",
                    n.qualified_name,
                    serde_json::to_string_pretty(&n)?
                ),
                None => println!("\naucun wnode dans le store weave"),
            }
        }
        println!("\n--probe : fin (aucun serveur demarre)");
        return Ok(());
    }

    let module = AletheiaModule {
        pages: pages.clone(),
        wnodes: wnodes.clone(),
        config: links_config(wnodes.is_some())?,
    };

    // Routage custom monté dans l'exposition REST : supervision détaillée.
    let health = Arc::new(HealthState {
        store_path: args.store.clone(),
        weave_path: args.weave.clone(),
        read_only: args.read_only,
        pages: pages.clone(),
        wnodes: wnodes.clone(),
    });
    let custom = Router::new()
        .route("/healthz/store", get(healthz_store))
        .with_state(health);

    let host = Arc::new(
        ServerBuilder::new()
            .with_link_service(ObrainLinkService::new(store.clone()))
            // EventBus requis pour SSE (/events/stream) et /graphql/ws.
            .with_event_bus(1024)
            .register_module(module)?
            .build_host()?,
    );

    // NB : `ServerBuilder::with_custom_routes` alimente un champ interne que
    // `build_host()` ne consomme pas — les routes custom doivent être passées à
    // `RestExposure::build_router(host, routes)` (constaté à l'exécution :
    // /healthz/store tombait dans le fallback des routes de liens).
    let rest = RestExposure::build_router(host.clone(), vec![custom])?;
    // **Surfaces non appliquées, assumées.** GraphQL et WebSocket ne sont pas
    // résolus par route : les politiques `auth:` ne s'y rattachent pas. Le
    // framework refuse donc de les monter quand elles sont armées ; cet exemple
    // les accepte explicitement (le choix est visible ici, et journalisé en
    // avertissement à chaque montage).
    //
    // Ce que cela implique : les entités de cet hôte sont accessibles par
    // GraphQL — lecture **et mutation** — sans passer par les politiques
    // déclarées. À resserrer (contrôle dans les résolveurs) avant toute
    // exposition non maîtrisée.
    let graphql = GraphQLExposure::build_router_unenforced(host.clone())?;
    let ws = WebSocketExposure::build_router_unenforced(host.clone())?;
    // gRPC garde son fallback : il ne peut pas être mergé (double fallback),
    // il est donc monté en fallback — mais **filtré par protocole** (cf.
    // `dispatch_by_protocol` plus bas). Le routeur tonic répond en effet
    // `200 OK` + `content-type: application/grpc` + `grpc-status: 12` +
    // corps vide à TOUTE requête qu'il ne route pas, HTTP/1.1 comme h2c :
    // monté en `fallback_service` brut, il avalait donc chaque chemin
    // inconnu (un 404 devenait un faux 200 gRPC au corps vide — c'est ce
    // que l'edge public recevait au lieu du document MCP).
    // Même acceptation explicite pour gRPC (voir ci-dessus).
    let grpc = GrpcExposure::build_router_unenforced(host.clone())?;
    let rest_fallback = rest.clone();

    // MCP : monté au même niveau que les expositions. `/mcp` ne dépend pas du
    // framework (aucun `EntityDescriptor` requis) et n'ajoute pas de fallback,
    // il peut donc être mergé directement ici.
    let mcp = Router::new()
        .route("/mcp", get(mcp_info).post(mcp_post))
        // Alias explicite : l'edge public réécrit tout en `/api/mcp` (cf.
        // `normalize_target_path`) — la route est montée telle quelle pour que
        // le contrat public soit visible dans le code, pas seulement déduit de
        // la normalisation.
        .route("/api/mcp", get(mcp_info).post(mcp_post))
        .with_state(Arc::new(McpState {
            pages: pages.clone(),
            wnodes: wnodes.clone(),
            store_path: args.store.clone(),
            weave_path: args.weave.clone(),
            strict_read_only: args.read_only,
        }));

    let inner = Router::new()
        .merge(rest)
        .merge(graphql)
        .merge(ws)
        .merge(mcp)
        .fallback(move |req: axum::extract::Request| {
            let grpc = grpc.clone();
            let rest = rest_fallback.clone();
            async move { dispatch_by_protocol(grpc, rest, req).await }
        });

    // Couche d'ingress : **la toute première** à voir la requête telle que
    // l'edge l'a envoyée. Elle journalise la cible brute (une ligne par
    // requête, jamais le corps) puis réécrit le chemin en forme origine
    // normalisée (cf. [`edge_ingress`]).
    //
    // Elle enveloppe le routeur **entier** (`tower::Layer`) et non les routes
    // (`Router::layer`) : le middleware de `Router::layer` ne s'exécute
    // qu'*après* le matching d'axum — mesuré, `/mcp/` y était bien normalisé
    // en `/mcp` dans le journal mais restait non routé (404). Le routeur
    // exposé n'a donc **aucune route** : tout part dans son
    // `fallback_service`, qui est le routeur réel enveloppé de l'ingress. La
    // réécriture précède ainsi le matching.
    let routed = tower::Layer::layer(&axum::middleware::from_fn(edge_ingress), inner);
    let app = Router::new().fallback_service(routed);

    let listener = tokio::net::TcpListener::bind(&args.addr)
        .await
        .with_context(|| format!("bind impossible sur {}", args.addr))?;
    let local = listener.local_addr()?;

    println!("\nserveur en ecoute sur http://{local}");
    println!("  REST       GET  /pages, /pages/{{id}}, /pages/{{id}}/neighbors");
    if wnodes.is_some() {
        println!("  REST       GET  /wnodes, /wnodes/{{qn}}, /wnodes/{{qn}}/links");
    }
    println!("  sante      GET  /healthz, /healthz/store");
    println!("  GraphQL    POST /graphql   (GET /graphql/schema, /graphql/playground)");
    println!("  gRPC       this_grpc.EntityService / LinkService / EventService (HTTP/2)");
    println!("  WebSocket  GET  /ws");
    println!("  SSE        GET  /events/stream");
    println!(
        "  MCP        GET  /mcp (document), POST /mcp (JSON-RPC 2.0 : initialize, tools/list, tools/call)"
    );

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .context("serve a echoue")?;

    println!("arret propre");
    Ok(())
}

/// Une requête est « gRPC » si — et seulement si — son `content-type`
/// commence par `application/grpc` (spec gRPC sur HTTP/2) ou si son chemin
/// cible une surface gRPC : un service déclaré (`/this_grpc.<Service>/…`,
/// cf. `proto/this_grpc.proto`) ou l'export de schéma `/grpc/proto`.
///
/// Le protocole de transport (HTTP/1.1, h2c par *prior knowledge* ou par
/// *Upgrade*) n'entre pas en jeu : ce qui décide, c'est la nature de la
/// requête, pas la version de HTTP.
fn is_grpc_request(req: &axum::extract::Request) -> bool {
    let content_type = req
        .headers()
        .get(axum::http::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    if content_type
        .trim_start()
        .to_ascii_lowercase()
        .starts_with("application/grpc")
    {
        return true;
    }
    let path = req.uri().path();
    path.starts_with("/this_grpc.") || path == "/grpc/proto"
}

/// Fallback de dispatch du serveur : route la requête vers le routeur gRPC
/// **uniquement** si elle est réellement gRPC (cf. [`is_grpc_request`]),
/// sinon vers le routeur REST/GraphQL/MCP normal.
///
/// Le fallback « liens » du framework REST répond `400
/// {"error":"Invalid path format"}` à tout chemin qu'aucune route ne
/// reconnaît (chemin vide, `/`, `*`, segment solitaire…). Ce `400` est
/// **masquant** : il ne dit ni quel chemin a été reçu ni qu'il n'existe pas.
/// Il est donc traduit ici en `404` JSON explicite, qui **réémet le chemin
/// effectivement routé** — c'est ce qui rend visible, depuis le domaine
/// public seul, un chemin réécrit en amont par l'edge.
async fn dispatch_by_protocol(grpc: Router, rest: Router, req: axum::extract::Request) -> Response {
    use tower::ServiceExt;

    if is_grpc_request(&req) {
        return match grpc.oneshot(req).await {
            Ok(response) => response,
            // `Router` est infaillible : la branche d'erreur est inhabitée.
            Err(never) => match never {},
        };
    }

    let path = req.uri().path().to_string();
    let response = match rest.oneshot(req).await {
        Ok(response) => response,
        Err(never) => match never {},
    };

    if response.status() != StatusCode::BAD_REQUEST {
        return response;
    }

    // On ne traduit que la signature exacte du fallback « liens » du
    // framework ; tout autre 400 est rendu tel quel, corps intact.
    let (parts, body) = response.into_parts();
    let bytes = match axum::body::to_bytes(body, 8192).await {
        Ok(b) => b,
        Err(_) => return unrouted_path_response(&path),
    };
    let invalid_path = serde_json::from_slice::<JVal>(&bytes)
        .ok()
        .and_then(|v| {
            v.get("error")
                .and_then(|e| e.as_str())
                .map(|s| s == "Invalid path format")
        })
        .unwrap_or(false);

    if invalid_path {
        return unrouted_path_response(&path);
    }
    Response::from_parts(parts, axum::body::Body::from(bytes))
}

/// Surfaces publiques annoncées dans la réponse `404` explicite.
const PUBLIC_SURFACES: &[&str] = &[
    "/healthz",
    "/healthz/store",
    "/pages",
    "/pages/{id}",
    "/pages/{id}/neighbors",
    "/graphql",
    "/graphql/schema",
    "/events/stream",
    "/ws",
    "/mcp",
    "/this_grpc.<Service>/<Method>",
];

/// `404` JSON explicite pour un chemin qu'aucune surface ne reconnaît : il
/// **réémet le chemin routé** et la liste des surfaces existantes (là où le
/// fallback du framework renvoyait un `400 "Invalid path format"` muet).
fn unrouted_path_response(path: &str) -> Response {
    (
        StatusCode::NOT_FOUND,
        Json(json!({
            "error": "not_found",
            "path": path,
            "message": "aucune surface this-rs a ce chemin — voir `surfaces`",
            "surfaces": PUBLIC_SURFACES,
        })),
    )
        .into_response()
}

/// Normalise une cible de requête **brute** en chemin d'origine exploitable.
///
/// Tolère, dans cet ordre : fragment, query (retirée du chemin — elle reste
/// portée par l'URI transmise au handler), **forme absolue**
/// (`http://host:port/mcp` → `/mcp`, y compris la forme sans chemin
/// `http://host:port` → `/`), autorité seule, cible vide et cible `*`
/// (toutes deux → `/`), `/` multiples (`//mcp`, `///mcp` → `/mcp`) et slash
/// final (`/mcp/` → `/mcp`). Le résultat est toujours non vide et commence
/// par `/` : il ne reste donc plus de « chemin inexploitable » à router.
fn normalize_target_path(raw: &str) -> String {
    let mut s = raw.trim();
    if let Some(i) = s.find('#') {
        s = &s[..i];
    }
    if let Some(i) = s.find('?') {
        s = &s[..i];
    }
    // Forme absolue (proxy) : on ne garde que le chemin.
    if let Some(i) = s.find("://") {
        let after = &s[i + 3..];
        s = match after.find('/') {
            Some(j) => &after[j..],
            None => "/",
        };
    }
    if s.is_empty() || s == "*" {
        return "/".to_string();
    }
    let mut out = String::with_capacity(s.len() + 1);
    for c in s.chars() {
        if c == '/' && out.ends_with('/') {
            continue;
        }
        out.push(c);
    }
    if !out.starts_with('/') {
        out.insert(0, '/');
    }
    while out.len() > 1 && out.ends_with('/') {
        out.pop();
    }
    // Préfixe `/api` : l'edge public `mcp.aletheia-chain.com` **réécrit toute
    // requête** en `/api/mcp` (reverse_proxy dont l'amont porte le chemin)
    // avant de l'atteindre — mesuré, cf. `edge_ingress`. On accepte donc la
    // surface aussi bien à la racine que derrière un unique segment `/api`.
    if let Some(rest) = out.strip_prefix("/api") {
        out = if rest.is_empty() {
            "/".to_string()
        } else if rest.starts_with('/') {
            rest.to_string()
        } else {
            out
        };
    }
    out
}

/// Couche d'ingress : **journalise la cible brute reçue** (méthode, URI telle
/// quelle — forme absolue, `*`, `//…`, query —, chemin routé après
/// normalisation, code de réponse, `Host` et en-têtes `X-Forwarded-*`) puis
/// réécrit le chemin de la requête en forme origine normalisée, query
/// préservée, avant tout matching.
///
/// Une ligne par requête, jamais le corps : `journalctl --user -u
/// this-rs-aletheia-8080` montre donc exactement ce que l'edge envoie.
async fn edge_ingress(mut req: axum::extract::Request, next: axum::middleware::Next) -> Response {
    use std::str::FromStr;

    // On extrait **tout** ce qui est journalisé avant le `await` : rien
    // n'emprunte la requête au-delà du point de suspension.
    let method = req.method().clone();
    let raw_target = req.uri().to_string();
    let raw_path = req.uri().path().to_string();
    let query = req.uri().query().map(str::to_string);
    let headers = req.headers().clone();
    let pick = |name: &str| {
        headers
            .get(name)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("-")
            .to_string()
    };
    let host = pick("host");
    let forwarded = format!(
        "for={} proto={} host={} xri={}",
        pick("x-forwarded-for"),
        pick("x-forwarded-proto"),
        pick("x-forwarded-host"),
        pick("x-real-ip"),
    );

    // Normalisation calculée sur la cible **brute** (et non sur
    // `uri().path()`) : c'est la seule qui couvre la forme absolue, `*` et
    // l'autorité seule ; `raw_path` reste journalisé tel quel.
    let routed_path = normalize_target_path(&raw_target);
    if routed_path != raw_path {
        let pq = match &query {
            Some(q) => format!("{routed_path}?{q}"),
            None => routed_path.clone(),
        };
        match axum::http::uri::PathAndQuery::from_str(&pq) {
            Ok(pq) => {
                let mut parts = req.uri().clone().into_parts();
                parts.path_and_query = Some(pq);
                if let Ok(uri) = axum::http::Uri::from_parts(parts) {
                    *req.uri_mut() = uri;
                }
            }
            Err(e) => tracing::warn!(
                target: "aletheia::ingress",
                "reecriture de chemin impossible ({pq:?}) : {e}"
            ),
        }
    }

    let response = next.run(req).await;
    tracing::info!(
        target: "aletheia::ingress",
        "{method} raw=\"{raw_target}\" recu=\"{raw_path}\" route=\"{routed_path}\" status={} host=\"{host}\" fwd=\"{forwarded}\"",
        response.status().as_u16(),
    );
    response
}

async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut s) => {
                s.recv().await;
            }
            Err(_) => std::future::pending::<()>().await,
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }
    println!("\nsignal recu — arret du serveur");
}
