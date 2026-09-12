//! vite-rs powered SPA frontend hosting for generated this-rs projects.
//!
//! This module lets a generated API binary serve its own frontend — instead of
//! delegating to an external Node/Vite dev server or to hand-rolled static
//! handling — by reusing the [`vite-rs`] crate inside an axum fallback:
//!
//! * **static file rules** come from `vite_rs::statics` (safe path
//!   resolution, `assets/`-vs-root layouts, MIME types, ETag/304 revalidation
//!   and per-URL `Cache-Control` policies),
//! * **dev mode** comes from `vite_rs::dev::DevState`: hot-reload version
//!   polling, rebuild-on-change watcher, browser console/network/error
//!   capture and the **AI command bridge** (`/__dev/command*`),
//! * **embedded (production) mode** serves the assets compiled into the
//!   binary by the generated project's `rust-embed` store through the same
//!   rules, so dev and prod share one serving contract.
//!
//! vite-rs is a self-contained HTTP server of its own; axum cannot run
//! "through" it. Instead this module adapts vite-rs's *per-request* public
//! API (`DevState::handle`, `DevState::inject_html`, `StaticFiles`,
//! `statics::*`) to axum handlers — vite-rs stays the source of truth for
//! every serving decision, and the framework contributes only the glue.
//!
//! # Feature flag
//!
//! Gated behind `#[cfg(feature = "frontend")]`. Enable with:
//! ```toml
//! this-rs = { version = "0.0.10", features = ["frontend"] }
//! ```
//!
//! # Dev mode activation (disk serving)
//!
//! The generated `main.rs` calls [`attach_from_env`], which reads the same
//! environment contract this-cli sets up for `this dev`:
//!
//! | Variable | Meaning | Default |
//! |---|---|---|
//! | `SERVE_FRONTEND` / `THIS_SERVE_FRONTEND` | `"true"` → serve the SPA from disk | off |
//! | `FRONTEND_PATH` / `THIS_FRONTEND_PATH` | web root (`dist/` of the webapp target) | `dist` |
//! | `THIS_DEV` | `"true"` → vite-rs dev loop (watch + reload + `/__dev` bridge) | off |
//! | `THIS_WATCH` | extra source dirs to watch (split on `:` or `;`) | none |
//! | `THIS_REBUILD` | shell command run on change (the bundler) | `npm run build` |
//! | `THIS_REBUILD_CWD` | working dir of the rebuild command | parent of the web root |
//! | `THIS_DEV_LOG` | NDJSON log file (`tail -f` it, the AI bridge logs here) | `$TMPDIR/this-rs-dev.log` |
//! | `THIS_DEV_COMMANDS` | `"false"` disables the AI command bridge | enabled |
//! | `THIS_POLL_MS` | watcher poll interval | `500` |
//!
//! [`vite-rs`]: https://github.com/DeusData/vite-rs

use std::borrow::Cow;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::Router;
use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::{Method, StatusCode, header};
use axum::response::{IntoResponse, Response};
use vite_rs::dev::DevState;
use vite_rs::http::Outgoing;
use vite_rs::statics::{self, StaticFiles};

/// Mirrors vite-rs's request body cap so the adapter never buffers more than
/// the crate itself would.
pub const MAX_DEV_BODY: usize = 8 * 1024 * 1024;

/// Where the SPA assets live.
#[derive(Clone)]
enum Source {
    /// Files on disk (dev, or `SERVE_FRONTEND` production layouts).
    Disk(StaticFiles),
    /// Bytes compiled into the binary (generated `rust-embed` store).
    Embedded(EmbeddedLoader),
}

/// Chargeur d'un actif compilé dans le binaire (`rust-embed`).
///
/// Nommé plutôt qu'écrit en place : le type était suffisamment imbriqué pour que
/// clippy le refuse, et un alias dit mieux l'intention qu'une signature touffue.
type EmbeddedLoader = Arc<dyn Fn(&str) -> Option<Cow<'static, [u8]>> + Send + Sync>;

/// Frontend hosting state shared by every fallback request.
#[derive(Clone)]
pub struct Frontend {
    source: Source,
    /// Present in dev mode only (inert otherwise — see `vite_rs::dev`).
    dev: Option<Arc<DevState>>,
}

/// Builder for the vite-rs dev loop (hot reload, rebuild watcher, AI bridge).
pub struct Dev {
    www: PathBuf,
    watch: Vec<PathBuf>,
    rebuild: Option<String>,
    rebuild_cwd: PathBuf,
    log_path: PathBuf,
    poll_ms: u64,
    commands: bool,
}

impl Dev {
    /// New dev loop rooted at the served web root.
    pub fn new(www: impl Into<PathBuf>) -> Self {
        let www = www.into();
        Self {
            rebuild_cwd: www
                .parent()
                .map(|p| p.to_path_buf())
                .unwrap_or_else(|| www.clone()),
            log_path: std::env::temp_dir().join("this-rs-dev.log"),
            www,
            watch: Vec::new(),
            rebuild: None,
            poll_ms: 500,
            commands: true,
        }
    }

    /// Directories watched for changes (in addition to the web root).
    pub fn watch(mut self, dirs: impl IntoIterator<Item = PathBuf>) -> Self {
        self.watch = dirs.into_iter().collect();
        self
    }

    /// Shell command run when a watched source dir changes (e.g. the bundler).
    pub fn rebuild(mut self, cmd: Option<String>) -> Self {
        self.rebuild = cmd;
        self
    }

    /// Working directory of the rebuild command (defaults to `www/..`).
    pub fn rebuild_cwd(mut self, cwd: impl Into<PathBuf>) -> Self {
        self.rebuild_cwd = cwd.into();
        self
    }

    /// NDJSON log file for dev events and browser capture.
    pub fn log_path(mut self, p: impl Into<PathBuf>) -> Self {
        self.log_path = p.into();
        self
    }

    /// Watcher poll interval in ms (min 200).
    pub fn poll_ms(mut self, ms: u64) -> Self {
        self.poll_ms = ms;
        self
    }

    /// Disable the AI command bridge for untrusted environments.
    pub fn commands(mut self, on: bool) -> Self {
        self.commands = on;
        self
    }

    /// Start the watcher thread and produce the shared dev state.
    pub fn start(self) -> Result<Arc<DevState>, String> {
        DevState::new(&self.www)
            .dev(true)
            .watch(self.watch)
            .rebuild(self.rebuild)
            .rebuild_cwd(&self.rebuild_cwd)
            .log_path(&self.log_path)
            .poll_ms(self.poll_ms)
            .commands(self.commands)
            .start()
    }
}

impl Frontend {
    /// Serve SPA assets from a directory on disk.
    pub fn disk(www: impl Into<PathBuf>) -> Self {
        Self {
            source: Source::Disk(StaticFiles::new(www)),
            dev: None,
        }
    }

    /// Serve SPA assets from an in-binary byte store.
    ///
    /// The loader receives a web-relative path (`index.html`, `assets/x.js`,
    /// …) and returns the asset bytes — exactly the contract of a
    /// `rust_embed`-derived store's `get(path).map(|f| f.data)`.
    pub fn embedded(
        loader: impl Fn(&str) -> Option<Cow<'static, [u8]>> + Send + Sync + 'static,
    ) -> Self {
        Self {
            source: Source::Embedded(Arc::new(loader)),
            dev: None,
        }
    }

    /// Turn on the vite-rs dev loop (hot reload + `/__dev` bridge + rebuild
    /// watcher). Errors only if the watcher thread cannot spawn.
    pub fn with_dev(self, dev: Dev) -> Result<Self, String> {
        Ok(Self {
            dev: Some(dev.start()?),
            ..self
        })
    }

    /// The web root in dev/disk mode.
    pub fn www(&self) -> Option<&Path> {
        match &self.source {
            Source::Disk(sf) => Some(sf.www()),
            Source::Embedded(_) => None,
        }
    }

    /// Build the fallback router for this frontend (state included).
    pub fn router(self) -> Router {
        Router::new().fallback(serve).with_state(self)
    }

    /// Attach this frontend as the fallback of an existing axum app. API
    /// routes registered on `app` keep priority; everything unmatched goes to
    /// the SPA (or `/__dev` endpoints in dev mode).
    ///
    /// # ⚠️ L'ordre décide si vos couches voient le frontend
    ///
    /// `fallback_service` **remplace** le repli existant, y compris celui que
    /// vos couches enveloppaient. Mesuré, dans `tests/frontend_surface.rs` :
    ///
    /// | ordre | ce que reçoit une requête non routée |
    /// |---|---|
    /// | `api.layer(mw)` **puis** `attach` | le repli, **sans** passer par `mw` |
    /// | `attach` **puis** `api.layer(mw)` | `mw` la voit |
    ///
    /// Conséquence : attaché en dernier (l'ordre naturel — « je construis mon
    /// API, puis je branche le frontend »), le SPA **et le pont `/__dev`** sont
    /// servis **hors** de l'authentification et de l'autorisation. Sur des
    /// fichiers statiques c'est sans conséquence ; sur `/__dev`, qui **exécute
    /// la commande de reconstruction**, ce n'est pas acceptable.
    ///
    /// **Appelez `attach` avant vos couches**, ou montez le repli vous-même
    /// avant de les appliquer :
    ///
    /// ```rust,ignore
    /// let app = frontend.attach(api_router);   // 1. le repli d'abord
    /// let app = app.layer(auth).layer(authz);  // 2. les couches ensuite
    /// ```
    ///
    /// Le pont de commandes a par ailleurs sa propre garde : il n'est servi que
    /// si l'application l'a explicitement demandé, et chaque service journalise
    /// un avertissement.
    pub fn attach(self, app: Router) -> Router {
        let router = self.router();
        app.fallback_service(router)
    }
}

/// **Un site par hôte** : plusieurs domaines, plusieurs contenus, un processus.
///
/// # Pourquoi ce type existe
///
/// Un `Frontend` est un **repli** : il sert un contenu. Une application réelle
/// sert souvent **plusieurs domaines** — un site public, une console, un
/// explorateur, un serveur git — chacun avec son dossier. Monter un processus
/// par domaine serait une réponse ; celle-ci en sert N avec les mêmes couches,
/// la même garde de domaine et le même `Caddyfile`.
///
/// ```rust,ignore
/// let sites = FrontendSites::new()
///     .site("www.exemple.fr", Frontend::disk("dist/www"))
///     .site("explorer.exemple.fr", Frontend::disk("web/explorer"))
///     .site("ci.exemple.fr", Frontend::disk("web/ci"))
///     .default_site(Frontend::disk("dist/www"));
/// let app = sites.attach(api_router);   // AVANT vos couches (voir `attach`)
/// ```
///
/// # Deux règles
///
/// * **Le premier site qui correspond gagne.** L'ordre de déclaration est donc
///   significatif : mettez les hôtes exacts avant les jokers.
/// * L'hôte est lu dans `Host`, ou dans `X-Forwarded-Host` **seulement** si vous
///   l'autorisez — la même règle que partout ailleurs, et le même piège.
#[derive(Clone)]
pub struct FrontendSites {
    sites: Vec<(String, std::sync::Arc<Frontend>)>,
    default: Option<std::sync::Arc<Frontend>>,
    trust_forwarded_host: bool,
}

impl Default for FrontendSites {
    fn default() -> Self {
        Self::new()
    }
}

impl FrontendSites {
    /// Un hôte sans site.
    pub fn new() -> Self {
        Self {
            sites: Vec::new(),
            default: None,
            trust_forwarded_host: false,
        }
    }

    /// Un site pour un hôte (ou un motif : `*.exemple.fr`).
    pub fn site(mut self, host: impl Into<String>, frontend: Frontend) -> Self {
        self.sites
            .push((host.into(), std::sync::Arc::new(frontend)));
        self
    }

    /// Le site des hôtes qui ne correspondent à rien.
    ///
    /// Sans lui, un hôte inconnu reçoit **404** — le garde de domaine l'aura
    /// refusé avant s'il est configuré.
    pub fn default_site(mut self, frontend: Frontend) -> Self {
        self.default = Some(std::sync::Arc::new(frontend));
        self
    }

    /// Faire confiance à `X-Forwarded-Host` pour choisir le site.
    pub fn trust_forwarded_host(mut self, on: bool) -> Self {
        self.trust_forwarded_host = on;
        self
    }

    /// Le site d'un hôte, dans l'ordre de déclaration.
    pub fn for_host(&self, host: &str) -> Option<&Frontend> {
        self.sites
            .iter()
            .find(|(pattern, _)| crate::config::server::host_matches(pattern, host))
            .map(|(_, frontend)| frontend.as_ref())
            .or(self.default.as_deref())
    }

    /// Combien de sites sont déclarés (hors site par défaut).
    pub fn len(&self) -> usize {
        self.sites.len()
    }

    /// Aucun site déclaré ?
    pub fn is_empty(&self) -> bool {
        self.sites.is_empty()
    }

    /// Le routeur de repli, qui choisit le site par l'hôte.
    pub fn router(&self) -> Router {
        Router::new()
            .fallback(dispatch_by_host)
            .with_state(std::sync::Arc::new(self.clone()))
    }

    /// Branche les sites comme **repli** de `app`.
    ///
    /// Comme [`Frontend::attach`], à appeler **avant** vos couches : un repli
    /// posé après elles **remplace** celui qu'elles enveloppaient.
    pub fn attach(self, app: Router) -> Router {
        app.fallback_service(self.router())
    }
}

/// Le repli qui choisit le site par l'hôte.
async fn dispatch_by_host(
    State(sites): State<std::sync::Arc<FrontendSites>>,
    req: Request,
) -> Response {
    let host = req
        .headers()
        .get("x-forwarded-host")
        .filter(|_| sites.trust_forwarded_host)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split(',').next())
        .or_else(|| {
            req.headers()
                .get(axum::http::header::HOST)
                .and_then(|v| v.to_str().ok())
        });

    let Some(host) = host else {
        return (StatusCode::NOT_FOUND, "hôte absent").into_response();
    };

    match sites.for_host(&crate::config::server::normalize_host(host)) {
        Some(frontend) => serve(State(frontend.clone()), req).await,
        None => (
            StatusCode::NOT_FOUND,
            format!("aucun site pour l'hôte `{host}`"),
        )
            .into_response(),
    }
}

/// Convenience: build the fallback router that serves an embedded byte store
/// with vite-rs rules (see [`Frontend::embedded`]).
pub fn embedded_router(
    loader: impl Fn(&str) -> Option<Cow<'static, [u8]>> + Send + Sync + 'static,
) -> Router {
    Frontend::embedded(loader).router()
}

// ---------------------------------------------------------------------------
// Environment-driven assembly (used by generated `main.rs`)
// ---------------------------------------------------------------------------

fn env_flag(name: &str) -> bool {
    matches!(
        std::env::var(name).ok().as_deref(),
        Some("1") | Some("true") | Some("yes") | Some("on")
    )
}

/// Read `SERVE_FRONTEND` (legacy) / `THIS_SERVE_FRONTEND` and return the
/// frontend configured from the environment, or `None` when serving is off.
///
/// See the module docs for the full environment contract.
pub fn from_env() -> Option<Frontend> {
    if !(env_flag("SERVE_FRONTEND") || env_flag("THIS_SERVE_FRONTEND")) {
        return None;
    }
    let www = std::env::var("FRONTEND_PATH")
        .or_else(|_| std::env::var("THIS_FRONTEND_PATH"))
        .unwrap_or_else(|_| "dist".to_string());
    let www = PathBuf::from(www);

    // Assemble the dev loop first (it borrows nothing from `fe`).
    let dev = if env_flag("THIS_DEV") {
        let mut dev = Dev::new(&www);
        // Non-empty env var, or None when unset.
        let env_opt = |k: &str| std::env::var(k).ok().filter(|s| !s.trim().is_empty());
        let watch = std::env::var("THIS_WATCH")
            .unwrap_or_default()
            .split([':', ';'])
            .filter(|s| !s.is_empty())
            .map(PathBuf::from)
            .collect::<Vec<_>>();
        if !watch.is_empty() {
            dev = dev.watch(watch);
        }
        dev = dev.rebuild(env_opt("THIS_REBUILD").or_else(|| Some("npm run build".to_string())));
        if let Some(cwd) = env_opt("THIS_REBUILD_CWD") {
            dev = dev.rebuild_cwd(PathBuf::from(cwd));
        }
        if let Some(log) = env_opt("THIS_DEV_LOG") {
            dev = dev.log_path(PathBuf::from(log));
        }
        if let Some(ms) = env_opt("THIS_POLL_MS").and_then(|v| v.parse().ok()) {
            dev = dev.poll_ms(ms);
        }
        // **Le pont de commandes est FERMÉ par défaut.** Il exécute la commande
        // de reconstruction (`THIS_REBUILD`, `npm run build` par défaut) sur
        // demande HTTP : l'ouvrir doit être un acte explicite de l'exploitant.
        // Avant, `THIS_DEV=true` suffisait à l'ouvrir — un déploiement qui hérite
        // de cette variable exposait donc un pont de commandes, hors des couches
        // d'authentification quand le frontend est attaché en dernier.
        let bridge_requested = matches!(
            std::env::var("THIS_DEV_COMMANDS").ok().as_deref(),
            Some("1") | Some("true") | Some("yes") | Some("on")
        );
        if bridge_requested {
            tracing::warn!(
                "THIS_DEV_COMMANDS : le pont de commandes `/__dev` est OUVERT — il exécute la \
                 commande de reconstruction sur demande HTTP. À réserver au développement local."
            );
        } else {
            dev = dev.commands(false);
        }
        Some(dev)
    } else {
        None
    };

    let base = Frontend::disk(&www);
    let fe = match dev {
        Some(dev) => base.with_dev(dev).unwrap_or_else(|e| {
            eprintln!("[frontend] dev loop unavailable: {e}; serving statically");
            Frontend::disk(&www)
        }),
        None => base,
    };
    Some(fe)
}

/// Attach the environment-configured frontend to `app`, if any.
///
/// This is the one-liner generated `main.rs` uses for non-embedded builds:
/// ```rust,ignore
/// fn attach_frontend(app: axum::Router) -> axum::Router {
///     this::server::frontend::attach_from_env(app)
/// }
/// ```
pub fn attach_from_env(app: Router) -> Router {
    match from_env() {
        Some(fe) => fe.attach(app),
        None => app,
    }
}

// ---------------------------------------------------------------------------
// Serving logic (vite-rs rules, axum transport)
// ---------------------------------------------------------------------------

async fn serve(State(fe): State<Frontend>, req: Request) -> Response {
    let method = req.method().clone();
    let url = req
        .uri()
        .path_and_query()
        .map(|pq| pq.as_str().to_string())
        .unwrap_or_else(|| "/".to_string());
    let p = statics::path_only(&url);

    // 1. Dev endpoints (/__dev*) — handled entirely by vite-rs DevState
    //    (version, log capture, AI command bridge).
    if let Some(dev) = &fe.dev
        && (p == "/__dev" || p.starts_with("/__dev/"))
    {
        let body = match axum::body::to_bytes(req.into_body(), MAX_DEV_BODY).await {
            Ok(b) => b.to_vec(),
            Err(_) => return StatusCode::PAYLOAD_TOO_LARGE.into_response(),
        };
        let vreq = vite_rs::http::Request {
            method: method.to_string(),
            url,
            headers: Vec::new(),
            body,
            remote: "127.0.0.1:0"
                .parse()
                .unwrap_or_else(|_| "0.0.0.0:0".parse().unwrap()),
            http11: true,
            want_keepalive: false,
        };
        return match dev.handle(&vreq) {
            Some(out) => outgoing_to_response(out),
            None => StatusCode::NOT_FOUND.into_response(),
        };
    }

    // 2. Static SPA serving (GET/HEAD only).
    if method != Method::GET && method != Method::HEAD {
        return StatusCode::METHOD_NOT_ALLOWED.into_response();
    }
    let inm = req
        .headers()
        .get(header::IF_NONE_MATCH)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());
    let is_head = method == Method::HEAD;

    let mut out = match &fe.source {
        Source::Disk(sf) => serve_disk(sf, fe.dev.as_deref(), &url, inm.as_deref()),
        Source::Embedded(loader) => {
            serve_embedded(loader.as_ref(), fe.dev.as_deref(), &url, inm.as_deref())
        }
    };

    if is_head {
        out.body = vite_rs::http::Body::Bytes(Vec::new());
    }
    outgoing_to_response(out)
}

/// Serve a static request from disk following vite-rs's layouts/rules.
fn serve_disk(sf: &StaticFiles, dev: Option<&DevState>, url: &str, inm: Option<&str>) -> Outgoing {
    let p = statics::path_only(url);
    if let Some(path) = sf.resolve(p) {
        let mut out = sf.serve_existing(&path, p, inm);
        maybe_inject(dev, &mut out);
        return out;
    }
    if statics::safe_rel(p).is_none() && p != "/" {
        // Traversal / encoded / unsafe path — never SPA-fall back.
        return Outgoing::text(404, "not found", "text/plain; charset=utf-8");
    }
    if p == "/" || !statics::is_asset_url(p) {
        // SPA fallback: unknown non-asset route → index.html.
        let index = sf.www().join("index.html");
        if index.is_file() {
            let mut out = sf.serve_existing(&index, "/index.html", inm);
            maybe_inject(dev, &mut out);
            return out;
        }
    }
    Outgoing::text(404, "not found", "text/plain; charset=utf-8")
}

/// Serve a static request from an embedded byte store (vite-rs layouts).
fn serve_embedded(
    loader: &(dyn Fn(&str) -> Option<Cow<'static, [u8]>> + Send + Sync),
    dev: Option<&DevState>,
    url: &str,
    inm: Option<&str>,
) -> Outgoing {
    let p = statics::path_only(url);
    let rel = statics::safe_rel(p);
    let hit = rel.as_deref().and_then(|rel| {
        // Same layout acceptance as StaticFiles::resolve: exact first, then
        // assets/-vs-root alternates.
        loader(rel)
            .or_else(|| loader(&format!("assets/{rel}")))
            .or_else(|| rel.strip_prefix("assets/").and_then(loader))
    });
    if let Some(bytes) = hit {
        return embedded_out(bytes, p, inm);
    }
    if rel.is_none() && p != "/" {
        return Outgoing::text(404, "not found", "text/plain; charset=utf-8");
    }
    if (p == "/" || !statics::is_asset_url(p))
        && let Some(bytes) = loader("index.html")
    {
        let mut out = embedded_out(bytes, "/index.html", inm);
        maybe_inject(dev, &mut out);
        return out;
    }
    Outgoing::text(404, "not found", "text/plain; charset=utf-8")
}

/// ETag for a raw byte payload (embedded mode): FNV-1a over the payload plus
/// the length, hex formatted like vite-rs's file ETags, weak-comparable.
fn bytes_etag(bytes: &[u8]) -> String {
    let mut h = 0xcbf29ce484222325u64;
    for b in bytes {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x100000001b3);
    }
    format!("\"{:x}-{:x}\"", h, bytes.len())
}

/// Build a cached (ETag/304) 200/304 response from raw embedded bytes.
fn embedded_out(bytes: Cow<'static, [u8]>, url_path: &str, inm: Option<&str>) -> Outgoing {
    let etag = bytes_etag(&bytes);
    let cache = statics::cache_control_for(url_path);
    let weak_match = inm.is_some_and(|inm| {
        inm.split(',').any(|t| {
            let t = t.trim();
            t == "*" || t == etag || t.strip_prefix("W/").unwrap_or(t) == etag
        })
    });
    if weak_match {
        return Outgoing {
            status: 304,
            headers: vec![
                ("ETag".into(), etag),
                ("Cache-Control".into(), cache.into()),
            ],
            body: vite_rs::http::Body::Bytes(Vec::new()),
        };
    }
    Outgoing {
        status: 200,
        headers: vec![
            ("Content-Type".into(), statics::mime_of(url_path).into()),
            ("ETag".into(), etag),
            ("Cache-Control".into(), cache.into()),
        ],
        body: vite_rs::http::Body::Bytes(bytes.into_owned()),
    }
}

/// In dev mode, inject the vite-rs reload/AI-bridge snippet into HTML pages.
fn maybe_inject(dev: Option<&DevState>, out: &mut Outgoing) {
    if let Some(dev) = dev {
        let is_html = out
            .headers
            .iter()
            .any(|(k, v)| k.eq_ignore_ascii_case("content-type") && v.contains("text/html"));
        if is_html {
            dev.inject_html(out, &[]);
        }
    }
}

/// Convert a vite-rs `Outgoing` into an axum `Response`.
fn outgoing_to_response(out: Outgoing) -> Response {
    let mut builder = Response::builder().status(out.status);
    for (k, v) in &out.headers {
        if k.eq_ignore_ascii_case("content-length")
            || k.eq_ignore_ascii_case("connection")
            || k.eq_ignore_ascii_case("date")
            || k.eq_ignore_ascii_case("server")
        {
            continue;
        }
        if let Ok(hv) = header::HeaderValue::from_str(v) {
            builder = builder.header(k, hv);
        }
    }
    for (k, v) in vite_rs::headers::DEFAULT_SECURITY_HEADERS {
        let already = out.headers.iter().any(|(ek, _)| ek.eq_ignore_ascii_case(k));
        if !already && let Ok(hv) = header::HeaderValue::from_str(v) {
            builder = builder.header(*k, hv);
        }
    }
    let body = match out.body {
        vite_rs::http::Body::Bytes(b) => Body::from(b),
        vite_rs::http::Body::File { mut file, .. } => {
            let mut buf = Vec::new();
            let _ = file.read_to_end(&mut buf);
            Body::from(buf)
        }
        vite_rs::http::Body::Stream { .. } => Body::empty(),
    };
    builder
        .body(body)
        .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use tower::ServiceExt;

    fn loader_from(store: HashMap<String, Vec<u8>>) -> impl Fn(&str) -> Option<Cow<'static, [u8]>> {
        move |p: &str| store.get(p).map(|b| Cow::Owned(b.clone()))
    }

    fn embedded_store(files: &[(&str, &str)]) -> HashMap<String, Vec<u8>> {
        files
            .iter()
            .map(|(k, v)| (k.to_string(), v.as_bytes().to_vec()))
            .collect()
    }

    async fn get(router: Router, uri: &str) -> Response {
        router
            .oneshot(
                axum::http::Request::builder()
                    .uri(uri)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap()
    }

    async fn body_text(res: Response) -> (u16, String, Vec<(String, String)>) {
        let status = res.status().as_u16();
        let headers = res
            .headers()
            .iter()
            .map(|(k, v)| {
                (
                    k.as_str().to_string(),
                    v.to_str().unwrap_or_default().to_string(),
                )
            })
            .collect::<Vec<_>>();
        let bytes = axum::body::to_bytes(res.into_body(), usize::MAX)
            .await
            .unwrap();
        (
            status,
            String::from_utf8_lossy(&bytes).into_owned(),
            headers,
        )
    }

    #[tokio::test]
    async fn embedded_serves_index_and_assets() {
        let fe = Frontend::embedded(loader_from(embedded_store(&[
            ("index.html", "<html><body>app</body></html>"),
            ("app.js", "export default 1;"),
            ("assets/hashed.css", "body{}"),
        ])));
        let router = fe.router();

        let (s, body, hdrs) = body_text(get(router.clone(), "/").await).await;
        assert_eq!(s, 200);
        assert!(body.contains("app"));
        assert!(
            hdrs.iter()
                .any(|(k, v)| k == "content-type" && v.contains("text/html"))
        );

        let (s, body, _) = body_text(get(router.clone(), "/index.html").await).await;
        assert_eq!(s, 200);
        assert!(body.contains("app"));

        let (s, _, hdrs) = body_text(get(router.clone(), "/assets/hashed.css").await).await;
        assert_eq!(s, 200);
        assert!(
            hdrs.iter()
                .any(|(k, v)| k == "cache-control" && v.contains("immutable"))
        );
        assert!(
            hdrs.iter()
                .any(|(k, v)| k == "content-type" && v.contains("text/css"))
        );
    }

    #[tokio::test]
    async fn embedded_spa_fallback_and_asset_404() {
        let fe = Frontend::embedded(loader_from(embedded_store(&[(
            "index.html",
            "<html>app</html>",
        )])));
        let router = fe.router();

        // Unknown route → index.html (SPA fallback).
        let (s, body, _) = body_text(get(router.clone(), "/products/42").await).await;
        assert_eq!(s, 200);
        assert!(body.contains("app"));

        // Missing hashed asset → 404, never index.html.
        let (s, _, _) = body_text(get(router.clone(), "/assets/old-chunk.js").await).await;
        assert_eq!(s, 404);
    }

    #[tokio::test]
    async fn traversal_is_rejected() {
        let fe = Frontend::embedded(loader_from(embedded_store(&[("index.html", "x")])));
        let router = fe.router();
        for uri in ["/../etc/passwd", "/a/../b", "/%2e%2e/secret"] {
            let (s, _, _) = body_text(get(router.clone(), uri).await).await;
            assert_eq!(s, 404, "{uri} must not SPA-fall back");
        }
    }

    #[tokio::test]
    async fn etag_round_trip_and_304() {
        let fe = Frontend::embedded(loader_from(embedded_store(&[(
            "app.js",
            "export default 1;",
        )])));
        let router = fe.router();

        let (s, _, hdrs) = body_text(get(router.clone(), "/app.js").await).await;
        assert_eq!(s, 200);
        let etag = hdrs
            .iter()
            .find(|(k, _)| k == "etag")
            .map(|(_, v)| v.clone())
            .expect("etag present");

        let req = axum::http::Request::builder()
            .uri("/app.js")
            .header("if-none-match", etag)
            .body(Body::empty())
            .unwrap();
        let res = router.oneshot(req).await.unwrap();
        assert_eq!(res.status().as_u16(), 304);
    }

    #[tokio::test]
    async fn non_get_methods_are_405() {
        let fe = Frontend::embedded(loader_from(embedded_store(&[("index.html", "x")])));
        let router = fe.router();
        let req = axum::http::Request::builder()
            .method("POST")
            .uri("/")
            .body(Body::empty())
            .unwrap();
        let res = router.oneshot(req).await.unwrap();
        assert_eq!(res.status().as_u16(), 405);
    }

    #[tokio::test]
    async fn dev_bridge_round_trip_over_http() {
        let dir = std::env::temp_dir().join(format!("this-frontend-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("index.html"), "<html><body>dev</body></html>").unwrap();
        let router = Frontend::disk(&dir)
            .with_dev(Dev::new(&dir).rebuild(None).log_path(dir.join("dev.log")))
            .unwrap()
            .router();

        let (s, body, _) = body_text(get(router.clone(), "/__dev/version").await).await;
        assert_eq!(s, 200);
        assert!(body.contains("\"v\":0"), "{body}");

        let req = axum::http::Request::builder()
            .method("POST")
            .uri("/__dev/command")
            .header("content-type", "application/json")
            .body(Body::from(r#"{"code":"document.title"}"#))
            .unwrap();
        let res = router.clone().oneshot(req).await.unwrap();
        assert_eq!(res.status().as_u16(), 200);

        let (s, next, _) = body_text(get(router.clone(), "/__dev/command/next").await).await;
        assert_eq!(s, 200);
        assert!(next.contains("document.title"), "{next}");

        let req = axum::http::Request::builder()
            .method("POST")
            .uri("/__dev/command/result")
            .body(Body::from(r#"{"id":"1","ok":true,"result":"My Page"}"#))
            .unwrap();
        let res = router.clone().oneshot(req).await.unwrap();
        assert_eq!(res.status().as_u16(), 204);

        let (s, got, _) = body_text(get(router, "/__dev/command/result?id=1").await).await;
        assert_eq!(s, 200);
        assert!(got.contains("\"ok\":true"), "{got}");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn dev_mode_injects_reload_snippet_into_html() {
        let dir =
            std::env::temp_dir().join(format!("this-frontend-inject-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("index.html"),
            "<html><head></head><body>dev</body></html>",
        )
        .unwrap();
        let router = Frontend::disk(&dir)
            .with_dev(Dev::new(&dir).rebuild(None))
            .unwrap()
            .router();

        let (s, body, _) = body_text(get(router, "/").await).await;
        assert_eq!(s, 200);
        assert!(
            body.contains("__VITE_RS_DEV"),
            "dev snippet injected: {body}"
        );
        std::fs::remove_dir_all(&dir).ok();
    }
}
