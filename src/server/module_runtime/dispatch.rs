//! Dispatch layer: serve the routes of runtime-loaded modules on a live router.
//!
//! # Why a layer, not a fallback
//!
//! The host's static router has its own link fallback (`/{entity}/{id}/{route}` and
//! deeper paths): a module mounted as a fallback would be SHADOWED by the link
//! wildcards. This layer runs BEFORE the router: it reads the first path segment, looks
//! it up in the current [`DispatchTable`] (one atomic load per request) and either sends
//! the request to the module's router, or lets it through untouched. A static entity
//! keeps its static routes; only plurals and link routes contributed by runtime modules
//! are intercepted (test `dynamic_module_paths_not_shadowed_by_link_wildcards`).
//!
//! # Placement and policies
//!
//! Mounted by `RestExposure::build_router` OUTSIDE the legacy `Authorization` layer
//! (whose entity table is frozen at build time and would refuse every hot-loaded path
//! by abstention) and INSIDE authentication, CORS and the domain guard, which therefore
//! still apply. Runtime-module requests are judged here, by the descriptor's own
//! policies (`EntityDesc.permissions`, `LinkDesc.permissions`) through the single
//! evaluator `this_descriptor::engine::policy::evaluate`, fail-closed (`owner` is
//! unevaluable at the edge → refused; unknown resolver → refused). With authorization
//! disarmed, nothing is judged (same as the rest of the host); in report-only mode the
//! refusal is logged and the request passes.

use super::runtime::ModuleRuntime;
use crate::core::auth::AuthContext;
use crate::core::errors::ApiError;
use crate::descriptor::engine::policy::{PolicyEnv, evaluate};
use crate::descriptor::framework::codes;
use crate::descriptor::{
    ContractError, DescriptorSet, Operation, PermDesc, PolicyExpr, PrincipalView,
};
use axum::Router;
use axum::extract::{Request, State};
use axum::http::Method;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;
use tokio::sync::{Notify, watch};
use tower::ServiceExt;

/// In-flight accounting and cancellation of one module.
pub struct Gate {
    inflight: AtomicUsize,
    idle: Notify,
    cancel: watch::Sender<bool>,
}

impl Default for Gate {
    fn default() -> Self {
        Self::new()
    }
}

impl Gate {
    pub fn new() -> Self {
        Self {
            inflight: AtomicUsize::new(0),
            idle: Notify::new(),
            cancel: watch::channel(false).0,
        }
    }

    fn enter(self: &Arc<Self>) -> InFlight {
        self.inflight.fetch_add(1, Ordering::SeqCst);
        InFlight(self.clone())
    }

    /// Requests of the module currently being handled.
    pub fn inflight(&self) -> usize {
        self.inflight.load(Ordering::SeqCst)
    }

    /// Wait until no request is in flight, at most `grace`. `true` = idle in time.
    pub async fn wait_idle(&self, grace: Duration) -> bool {
        let deadline = tokio::time::Instant::now() + grace;
        loop {
            let notified = self.idle.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if self.inflight() == 0 {
                return true;
            }
            tokio::select! {
                _ = &mut notified => {}
                _ = tokio::time::sleep_until(deadline) => return self.inflight() == 0,
            }
        }
    }

    /// Cancel the module's streams (handlers watching [`ModuleCancellation`]).
    pub fn cancel(&self) {
        self.cancel.send_replace(true);
    }

    fn token(&self) -> ModuleCancellation {
        ModuleCancellation(self.cancel.subscribe())
    }
}

struct InFlight(Arc<Gate>);
impl Drop for InFlight {
    fn drop(&mut self) {
        if self.0.inflight.fetch_sub(1, Ordering::SeqCst) == 1 {
            self.0.idle.notify_waiters();
        }
    }
}

/// Request extension set on every request dispatched to a runtime module. A handler
/// that streams (SSE, WebSocket, long poll) selects on [`ModuleCancellation::cancelled`]
/// and ends its stream when the module is unloaded after the drain grace delay.
#[derive(Clone)]
pub struct ModuleCancellation(watch::Receiver<bool>);

impl ModuleCancellation {
    pub fn is_cancelled(&self) -> bool {
        *self.0.borrow()
    }
    /// Resolves when the module is cancelled (immediately if it already is).
    pub async fn cancelled(&mut self) {
        let _ = self.0.wait_for(|c| *c).await;
    }
}

#[derive(Clone)]
pub(crate) struct EntityRoute {
    pub module: String,
    pub router: Router,
    pub gate: Arc<Gate>,
}

/// What the dispatch layer reads, swapped atomically by the runtime.
#[derive(Default)]
pub struct DispatchTable {
    /// plural → route of a runtime module.
    pub(crate) entities: HashMap<String, EntityRoute>,
    /// `(plural, link route name)` contributed by a runtime module → (module, gate).
    pub(crate) link_routes: HashMap<(String, String), (String, Arc<Gate>)>,
    /// Link router built from the composed snapshot (merged links config, all fetchers).
    pub(crate) links: Option<Router>,
    /// Plurals of UNLOADED runtime modules (not served by anything else): 404
    /// `this.module.not_found` instead of whatever the static fallback would answer.
    pub(crate) tombstones: HashMap<String, String>,
    /// Modules in `Draining`: their requests get 503 `this.module.unavailable`.
    pub(crate) draining: HashSet<String>,
    pub(crate) descriptors: Arc<DescriptorSet>,
    /// `(entity, plural)` of every served entity (static and runtime).
    pub(crate) plurals: Arc<Vec<(String, String)>>,
}

impl DispatchTable {
    pub(crate) fn empty(descriptors: Arc<DescriptorSet>) -> Self {
        Self {
            descriptors,
            ..Default::default()
        }
    }
    pub(crate) fn add_entity(
        &mut self,
        plural: &str,
        module: &str,
        router: Router,
        gate: Arc<Gate>,
    ) {
        self.entities.insert(
            plural.to_string(),
            EntityRoute {
                module: module.to_string(),
                router,
                gate,
            },
        );
    }
    pub(crate) fn is_empty(&self) -> bool {
        self.entities.is_empty()
    }
    fn entity_of_plural(&self, plural: &str) -> Option<&str> {
        self.plurals
            .iter()
            .find(|(_, p)| p == plural)
            .map(|(e, _)| e.as_str())
    }
}

/// Where a request goes.
enum Target {
    Module(Router),
    Links(Router),
}

/// Outcome of routing a path against one dispatch table.
enum Routed {
    /// Not a runtime module path: the static router handles it.
    Pass,
    /// Answered here (tombstone of an unloaded module).
    Respond(ContractError),
    /// Served by a runtime module.
    Module(String, Arc<Gate>, Target),
}

fn route(table: &DispatchTable, segs: &[&str]) -> Routed {
    let first = segs[0];
    if let Some(route) = table.entities.get(first) {
        let target = if segs.len() <= 2 {
            Target::Module(route.router.clone())
        } else {
            match &table.links {
                Some(l) => Target::Links(l.clone()),
                None => return Routed::Pass,
            }
        };
        Routed::Module(route.module.clone(), route.gate.clone(), target)
    } else if segs.len() >= 3
        && let Some((module, gate)) = table
            .link_routes
            .get(&(first.to_string(), segs[2].to_string()))
        && let Some(links) = &table.links
    {
        Routed::Module(module.clone(), gate.clone(), Target::Links(links.clone()))
    } else if let Some(module) = table.tombstones.get(first) {
        Routed::Respond(
            ContractError::framework(codes::MODULE_NOT_FOUND).with_param("module", module),
        )
    } else {
        Routed::Pass
    }
}

/// The layer function (`axum::middleware::from_fn_with_state`).
pub async fn dispatch_middleware(
    State(rt): State<Arc<ModuleRuntime>>,
    mut request: Request,
    next: Next,
) -> Response {
    let path = request.uri().path().to_string();
    let segs: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    if segs.is_empty() {
        return next.run(request).await;
    }

    // Route, then ENTER the module's gate, then confirm the table did not change in
    // between. Without the confirmation, a drain could start between our table read and
    // `gate.enter()`: `wait_idle` would see 0 in flight, the module would be unloaded,
    // and this request would still run on the unloaded module's router (race found in
    // review). Either the drain's table store happened before our re-read (we see a new
    // table and route again: 503), or after our `enter` (the drain waits for us).
    let mut table = rt.table.load_full();
    let mut attempts = 0;
    let (target, _inflight) = loop {
        let (module, gate, target) = match route(&table, &segs) {
            Routed::Pass => return next.run(request).await,
            Routed::Respond(e) => return ApiError(e).into_response(),
            Routed::Module(m, g, t) => (m, g, t),
        };
        if table.draining.contains(&module) {
            return ApiError(
                ContractError::framework(codes::MODULE_UNAVAILABLE).with_param("module", &module),
            )
            .into_response();
        }
        let inflight = gate.enter();
        let current = rt.table.load_full();
        if Arc::ptr_eq(&current, &table) {
            request.extensions_mut().insert(gate.token());
            break (target, inflight);
        }
        drop(inflight);
        attempts += 1;
        if attempts >= 8 {
            // The table keeps changing under us: refuse rather than guess.
            return ApiError(
                ContractError::framework(codes::MODULE_UNAVAILABLE).with_param("module", &module),
            )
            .into_response();
        }
        table = current;
    };

    let parts = rt.parts();
    if parts.authorize {
        let ctx = request
            .extensions()
            .get::<AuthContext>()
            .cloned()
            .unwrap_or(AuthContext::Anonymous);
        if let Err(e) = judge(&table, request.method(), &segs, &ctx, parts) {
            if parts.report_only {
                tracing::warn!(%path, code = %e.code.as_str(), "runtime module: refusal OBSERVED (report-only)");
            } else {
                return ApiError(e).into_response();
            }
        }
    }

    let router = match target {
        Target::Module(r) | Target::Links(r) => r,
    };
    match router.oneshot(request).await {
        Ok(r) => r,
        Err(never) => match never {},
    }
}

impl ModuleRuntime {
    /// Put the runtime in front of `app`: admin interface, then dispatch, then `app`.
    ///
    /// BEFORE routing (the layers wrap the router as a service, mounted as the fallback
    /// of an empty router): an `axum::Router::layer` would run AFTER the outer route
    /// match, and the outer path parameters would leak into the module router
    /// (`Wrong number of path arguments`, measured). Layers added to the returned router
    /// (authentication, domain guard, CORS) still run first.
    pub fn mount(self: &Arc<Self>, app: Router) -> Router {
        let svc = tower::ServiceBuilder::new()
            .layer(axum::middleware::from_fn_with_state(
                self.clone(),
                super::admin::admin_middleware,
            ))
            .layer(axum::middleware::from_fn_with_state(
                self.clone(),
                dispatch_middleware,
            ))
            .service(app);
        Router::new().fallback_service(svc)
    }
}

/// The principal as the pure evaluator sees it.
pub(crate) fn principal(ctx: &AuthContext) -> (bool, Option<String>, Vec<String>, bool, bool) {
    match ctx {
        AuthContext::User { user_id, roles, .. } => {
            (true, Some(user_id.to_string()), roles.clone(), false, false)
        }
        AuthContext::Owner { user_id, .. } => {
            (true, Some(user_id.to_string()), vec![], false, false)
        }
        AuthContext::Service { .. } => (true, None, vec![], false, true),
        AuthContext::Admin { admin_id } => (true, Some(admin_id.to_string()), vec![], true, false),
        AuthContext::Anonymous => (false, None, vec![], false, false),
    }
}

struct Env<'a> {
    ctx: &'a AuthContext,
    resolvers: &'a crate::core::auth::AuthResolverRegistry,
    resource: Option<uuid::Uuid>,
}
impl PolicyEnv for Env<'_> {
    fn resolver(&self, name: &str) -> Option<bool> {
        self.resolvers
            .get(name)
            .map(|r| r.check(self.ctx, self.resource.as_ref()))
    }
    fn permission(&self, _: &str) -> Option<bool> {
        // No materialized IAM at this edge: unevaluable, therefore refused.
        None
    }
}

/// Evaluate `policy` for `ctx`: `Ok` only on an evaluable `true`.
pub(crate) fn check_policy(
    policy: &PolicyExpr,
    ctx: &AuthContext,
    resolvers: &crate::core::auth::AuthResolverRegistry,
    resource: Option<uuid::Uuid>,
    operation: &str,
) -> Result<(), ContractError> {
    let (authenticated, user, roles, admin, service) = principal(ctx);
    let who = PrincipalView {
        authenticated,
        user_id: user.as_deref(),
        roles: &roles,
        admin,
        service,
        owner: None,
    };
    let env = Env {
        ctx,
        resolvers,
        resource,
    };
    match evaluate(policy, &who, &env) {
        Some(true) => Ok(()),
        _ if !authenticated => Err(ContractError::framework(codes::AUTH_UNAUTHENTICATED)),
        _ => {
            Err(ContractError::framework(codes::AUTH_FORBIDDEN).with_param("operation", operation))
        }
    }
}

/// Policy of a request to a runtime module's path, from the descriptors.
fn judge(
    table: &DispatchTable,
    method: &Method,
    segs: &[&str],
    ctx: &AuthContext,
    parts: &super::runtime::HostParts,
) -> Result<(), ContractError> {
    let forbidden =
        |op: &str| Err(ContractError::framework(codes::AUTH_FORBIDDEN).with_param("operation", op));
    let Some(scope) = table.entity_of_plural(segs[0]) else {
        return forbidden("unknown");
    };
    let mut scope = scope.to_string();
    let op = match (method, segs.len()) {
        (&Method::GET | &Method::HEAD, 1) => Operation::List,
        (&Method::POST, 1) => Operation::Create,
        (&Method::GET | &Method::HEAD, 2) => Operation::Get,
        (&Method::PUT | &Method::PATCH, 2) => Operation::Update,
        (&Method::DELETE, 2) => Operation::Delete,
        (&Method::GET | &Method::HEAD, n) if n >= 3 => Operation::ListLinks,
        (&Method::POST, 3) => match table.entity_of_plural(segs[2]) {
            Some(target) => {
                scope = target.to_string();
                Operation::Create
            }
            None => return forbidden("create"),
        },
        (&Method::POST, n) if n >= 4 => Operation::CreateLink,
        (&Method::PUT | &Method::PATCH, 4) => Operation::UpdateLink,
        (&Method::DELETE, n) if n >= 4 => Operation::DeleteLink,
        _ => return forbidden("unknown"),
    };
    let entity = table.descriptors.entity(&scope).map(|(_, e)| e);
    let link = if segs.len() >= 3 && op != Operation::Create {
        table
            .descriptors
            .modules()
            .flat_map(|m| m.links.iter())
            .find(|l| {
                (l.source == scope && l.forward_route == segs[2])
                    || (l.target == scope && l.reverse_route == segs[2])
            })
    } else {
        None
    };
    let policy = match (link, entity) {
        (Some(l), e) => l.effective_policy(e, &op),
        (None, Some(e)) => e.permissions.effective(&op),
        (None, None) => PermDesc::default_policy(&op),
    };
    let resource = segs.get(1).and_then(|s| uuid::Uuid::parse_str(s).ok());
    check_policy(&policy, ctx, &parts.resolvers, resource, op.as_str())
}
