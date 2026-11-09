//! Servir un arbre de vues **calculé par un module**, sans payer à chaque appel ce que R0 a
//! mesuré comme évitable.
//!
//! R0 (WebView, 09/11/2026) a chiffré quatre gaspillages. Voici où chacun est traité :
//!
//! | mesure R0 | traitement | où |
//! |---|---|---|
//! | `call_json` recompile le module à chaque appel : 1,3 ms au lieu de 18 µs | `Engine` + `Module` gardés par module installé, libérés à la désinstallation ou au remplacement | [`WasmiViews`] (`CompiledModule`, [`crate::call`]) |
//! | l'`INBOX` du SDK fuit 32,8 o/appel sur une instance réutilisée | **jamais d'instance réutilisée** : chaque appel instancie à neuf depuis le module compilé, donc rien à vider — la fuite ne peut pas exister | [`crate::call::CompiledModule`] |
//! | publier un arbre identique coûte un remontage (6,2 ms) contre 0,85–1,23 ms | l'arbre est comparé à celui déjà publié ; identique ⇒ pas de redessin | [`Screen::publish`] |
//! | `issues_against` n'est pas incrémental : 1,95 ms à 1 000 vues déclarées, à chaque appel | verdict mémoïsé par (version de module, arbre rendu) ; un appel qui rend le même arbre ne revalide pas | [`ViewHost`] |
//!
//! Et ce qui ne coûte rien parce qu'on ne le refait pas : un appel **identique** (même
//! module, même version, même surface, même requête) est rendu depuis le cache, sans
//! exécuter le module.
//!
//! # Les trois déclencheurs
//!
//! Un module de vue est une fonction pure de sa requête. L'exécuter n'est donc utile que
//! lorsque **la requête change de sens** — et trois événements, pas un de plus, le font.
//! [`ViewHost::serve`] exige un [`Trigger`] : il n'existe pas de valeur pour « la frappe »,
//! et l'API ne permet pas d'en inventer une sans modifier cette énumération.
//!
//! # Ce que le cache ne couvre pas
//!
//! La clé contient la requête **telle qu'elle est sérialisée** : deux requêtes qui disent
//! la même chose dans un ordre de champs différent ne se reconnaissent que si `serde_json`
//! trie les clés (c'est le cas sans sa feature `preserve_order`). Dans le doute, on
//! réexécute : un défaut de cache coûte du temps, jamais une vue fausse.

use crate::executor::ExecutorError;
use crate::render::{self, Limits, ModuleKey, ViewRenderer};
use std::collections::hash_map::DefaultHasher;
use std::collections::{HashMap, VecDeque};
use std::hash::{Hash, Hasher};
use std::mem::{Discriminant, discriminant};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use this_descriptor::ModuleDescriptor;
use this_descriptor::views::{Surface, ViewDesc};

/// Ce qui autorise l'exécution d'un module de vue. Trois, et seulement trois.
///
/// ```compile_fail
/// // Pas de quatrième : « la frappe » n'est pas un déclencheur.
/// let _ = this_module_harness::serving::Trigger::Keystroke;
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Trigger {
    /// **L'ouverture d'un écran** : la personne arrive sur une vue ; il n'y a encore rien
    /// à montrer, il faut calculer.
    ScreenOpened,
    /// **Une écriture qui change la requête** : un message envoyé, une ligne créée ou
    /// supprimée — une donnée que la vue lit a changé de valeur. La requête est
    /// recalculée par l'hôte ; si elle est identique à une précédente, le cache répond.
    QueryWritten,
    /// **Un changement de conversation** : la requête désigne un autre fil. C'est un
    /// événement distinct d'une écriture : rien n'a été écrit, la portée a changé.
    ConversationChanged,
}

impl Trigger {
    /// Les trois déclencheurs, dans l'ordre de [`Stats::by_trigger`].
    pub const ALL: [Trigger; 3] = [
        Trigger::ScreenOpened,
        Trigger::QueryWritten,
        Trigger::ConversationChanged,
    ];

    fn index(self) -> usize {
        match self {
            Trigger::ScreenOpened => 0,
            Trigger::QueryWritten => 1,
            Trigger::ConversationChanged => 2,
        }
    }
}

// POURQUOI LA FRAPPE N'EST PAS UN DÉCLENCHEUR (et ne doit pas le devenir) :
//
// Pendant qu'on tape, la personne compose un *brouillon* ; ce que la vue lit (les
// messages, les lignes) n'a pas bougé. Rendre à chaque touche exécuterait le module
// 40 fois pour 40 requêtes identiques — ou pire, pour 40 requêtes différentes que
// personne ne voit durer (R0 : un remontage de 200 messages coûte 6,2 ms ; à 40 touches
// par message, c'est le fil qui bégaie sous les doigts). Le brouillon rejoint la requête
// au moment de l'envoi, et c'est alors `QueryWritten`.

/// Compteurs observables : ce que le serveur a réellement fait.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Stats {
    /// Demandes de rendu reçues, tous déclencheurs confondus.
    pub requests: u64,
    /// Fois où le module a réellement été exécuté.
    pub executions: u64,
    /// Demandes répondues par le cache, sans exécuter le module.
    pub cache_hits: u64,
    /// Fois où l'arbre rendu a été analysé et confronté au descripteur.
    pub validations: u64,
    /// Fois où un arbre déjà validé (même version, mêmes octets) a évité la validation.
    pub memo_hits: u64,
    /// Demandes par déclencheur, dans l'ordre de [`Trigger::ALL`].
    pub by_trigger: [u64; 3],
}

/// Ce qui empêche de servir une vue.
#[derive(Clone, Debug, thiserror::Error)]
pub enum ServeError {
    #[error("module `{0}` non installé")]
    NotInstalled(String),
    #[error("requête non sérialisable : {0}")]
    Request(String),
    #[error("{0}")]
    Execution(#[from] ExecutorError),
    #[error("{0}")]
    Views(String),
    #[error("l'arbre rendu contredit le descripteur : {}", .issues.join(" ; "))]
    Rejected { issues: Vec<String> },
    #[error("le module n'a rendu aucune vue pour la surface {surface:?}")]
    NoViewForSurface { surface: Surface },
}

/// Un arbre servi.
#[derive(Clone, Debug)]
pub struct Served {
    /// Les vues de la surface demandée, validées. Même `Arc` pour un même arbre.
    pub views: Arc<Vec<ViewDesc>>,
    /// `true` si le module n'a pas été exécuté pour cette réponse.
    pub from_cache: bool,
}

type Verdict = Result<Arc<Vec<ViewDesc>>, ServeError>;

#[derive(Clone, PartialEq, Eq, Hash)]
struct CacheKey {
    module: String,
    version: String,
    surface: Discriminant<Surface>,
    request_hash: u64,
}

struct CacheEntry {
    /// Les octets de la requête : une collision de hash ne doit jamais servir une vue fausse.
    request: Arc<[u8]>,
    result: Verdict,
}

struct MemoEntry {
    response: Arc<[u8]>,
    verdict: Verdict,
}

struct Installed {
    key: ModuleKey,
    program: Arc<[u8]>,
    descriptor: Arc<ModuleDescriptor>,
    /// Verdicts de validation par hash de la réponse. Né avec la version, mort avec elle.
    memo: HashMap<u64, Vec<MemoEntry>>,
}

#[derive(Default)]
struct Inner {
    installed: HashMap<String, Installed>,
    cache: HashMap<CacheKey, CacheEntry>,
    order: VecDeque<CacheKey>,
    stats: Stats,
}

/// Combien de réponses le cache garde. Au-delà, la plus ancienne part.
pub const CACHE_CAPACITY: usize = 256;
/// Combien de verdicts de validation un module garde.
pub const MEMO_CAPACITY: usize = 64;

fn hash_of(bytes: &[u8]) -> u64 {
    let mut h = DefaultHasher::new();
    bytes.hash(&mut h);
    h.finish()
}

/// Le serveur de vues : des modules installés, un moteur, un cache.
pub struct ViewHost<R: ViewRenderer + Sync> {
    renderer: R,
    inner: Mutex<Inner>,
}

impl<R: ViewRenderer + Sync> ViewHost<R> {
    pub fn new(renderer: R) -> Self {
        Self {
            renderer,
            inner: Mutex::new(Inner::default()),
        }
    }

    pub fn renderer(&self) -> &R {
        &self.renderer
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub fn stats(&self) -> Stats {
        self.lock().stats
    }

    /// Installe (ou met à jour) un module. Une autre version, ou d'autres octets pour la
    /// même version, **libèrent** ce qui était gardé : compilation du moteur, réponses en
    /// cache, verdicts de validation.
    pub fn install(&self, descriptor: ModuleDescriptor, program: Vec<u8>) {
        let key = ModuleKey::new(descriptor.name.clone(), descriptor.version.clone());
        let program: Arc<[u8]> = program.into();
        let mut inner = self.lock();
        if let Some(old) = inner.installed.get(&key.name) {
            if old.key == key && old.program == program {
                // Même module, mêmes octets : seul le descripteur peut avoir changé, et
                // alors les verdicts de validation ne valent plus.
                if *old.descriptor == descriptor {
                    return;
                }
            }
            let old_key = old.key.clone();
            Self::evict(&mut inner, &key.name);
            self.renderer.release(&old_key);
        }
        inner.installed.insert(
            key.name.clone(),
            Installed {
                key,
                program,
                descriptor: Arc::new(descriptor),
                memo: HashMap::new(),
            },
        );
    }

    /// Désinstalle un module et libère tout ce qui le concernait.
    pub fn uninstall(&self, name: &str) {
        let mut inner = self.lock();
        if let Some(old) = inner.installed.remove(name) {
            Self::evict(&mut inner, name);
            self.renderer.release(&old.key);
        }
    }

    fn evict(inner: &mut Inner, name: &str) {
        inner.cache.retain(|k, _| k.module != name);
        inner.order.retain(|k| k.module != name);
        inner.installed.remove(name);
    }

    /// Rend l'arbre de `surface` du module `module`, pour `request`.
    ///
    /// Le module n'est exécuté que si ce triplet `(version, surface, requête)` n'a pas déjà
    /// été rendu. Un refus (borne dépassée, arbre hors contrat) est mis en cache comme un
    /// succès : le module est une fonction pure, le même appel refuserait pareil, et une
    /// boucle hostile ne doit pas coûter son fuel à chaque déclencheur.
    pub async fn serve(
        &self,
        module: &str,
        surface: Surface,
        request: &serde_json::Value,
        limits: &Limits,
        trigger: Trigger,
    ) -> Result<Served, ServeError> {
        let bytes: Arc<[u8]> = serde_json::to_vec(request)
            .map_err(|e| ServeError::Request(e.to_string()))?
            .into();
        let request_hash = hash_of(&bytes);

        let (key, program, descriptor) = {
            let mut inner = self.lock();
            inner.stats.requests += 1;
            inner.stats.by_trigger[trigger.index()] += 1;
            let Some(installed) = inner.installed.get(module) else {
                return Err(ServeError::NotInstalled(module.to_string()));
            };
            let key = installed.key.clone();
            let program = installed.program.clone();
            let descriptor = installed.descriptor.clone();
            let cache_key = CacheKey {
                module: key.name.clone(),
                version: key.version.clone(),
                surface: discriminant(&surface),
                request_hash,
            };
            if let Some(entry) = inner.cache.get(&cache_key)
                && entry.request == bytes
            {
                let result = entry.result.clone();
                inner.stats.cache_hits += 1;
                return result.map(|views| Served {
                    views,
                    from_cache: true,
                });
            }
            (key, program, descriptor)
        };

        // L'exécution se fait sans tenir le verrou : un module peut être lent.
        self.lock().stats.executions += 1;
        let called = self
            .renderer
            .render_module(&key, &program, &bytes, limits)
            .await;
        let verdict = match called {
            Err(e) => Err(ServeError::Execution(e)),
            Ok(call) => self.judge(&key, &descriptor, &call.response, limits, surface),
        };

        let mut inner = self.lock();
        // Le module a pu être remplacé pendant l'exécution : ne rien ranger pour une
        // version qui n'est plus installée.
        if inner.installed.get(&key.name).map(|i| &i.key) == Some(&key) {
            let cache_key = CacheKey {
                module: key.name.clone(),
                version: key.version.clone(),
                surface: discriminant(&surface),
                request_hash,
            };
            if inner.cache.len() >= CACHE_CAPACITY
                && let Some(oldest) = inner.order.pop_front()
            {
                inner.cache.remove(&oldest);
            }
            if inner
                .cache
                .insert(
                    cache_key.clone(),
                    CacheEntry {
                        request: bytes,
                        result: verdict.clone(),
                    },
                )
                .is_none()
            {
                inner.order.push_back(cache_key);
            }
        }
        verdict.map(|views| Served {
            views,
            from_cache: false,
        })
    }

    /// Analyse et valide une réponse, ou rend le verdict déjà rendu pour ces octets.
    fn judge(
        &self,
        key: &ModuleKey,
        descriptor: &ModuleDescriptor,
        response: &[u8],
        limits: &Limits,
        surface: Surface,
    ) -> Verdict {
        let response_hash = hash_of(response);
        let memoized = {
            let mut inner = self.lock();
            let hit = inner
                .installed
                .get(&key.name)
                .filter(|i| i.key == *key)
                .and_then(|i| i.memo.get(&response_hash))
                .and_then(|entries| entries.iter().find(|e| *e.response == *response))
                .map(|e| e.verdict.clone());
            if hit.is_some() {
                inner.stats.memo_hits += 1;
            }
            hit
        };
        let full = match memoized {
            Some(v) => v,
            None => {
                self.lock().stats.validations += 1;
                let v = Self::validate(descriptor, response, limits);
                let mut inner = self.lock();
                if let Some(installed) = inner.installed.get_mut(&key.name)
                    && installed.key == *key
                {
                    if installed.memo.len() >= MEMO_CAPACITY {
                        installed.memo.clear();
                    }
                    installed
                        .memo
                        .entry(response_hash)
                        .or_default()
                        .push(MemoEntry {
                            response: response.into(),
                            verdict: v.clone(),
                        });
                }
                v
            }
        };
        let tree = full?;
        if tree.iter().all(|v| v.surface == surface) {
            return Ok(tree);
        }
        let only: Vec<ViewDesc> = tree
            .iter()
            .filter(|v| v.surface == surface)
            .cloned()
            .collect();
        if only.is_empty() {
            return Err(ServeError::NoViewForSurface { surface });
        }
        Ok(Arc::new(only))
    }

    fn validate(descriptor: &ModuleDescriptor, response: &[u8], limits: &Limits) -> Verdict {
        let views = render::views_of(response, limits.response_bytes)
            .map_err(|e| ServeError::Views(e.to_string()))?;
        let issues = render::issues_against(descriptor, &views);
        if !issues.is_empty() {
            return Err(ServeError::Rejected { issues });
        }
        Ok(Arc::new(views))
    }
}

/// Ce qu'a produit [`Screen::publish`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Publication {
    /// L'arbre diffère de celui affiché : le redessin a eu lieu.
    Redrawn,
    /// L'arbre est identique : **aucun redessin**.
    Unchanged,
}

/// Une surface à l'écran : ce qui y est affiché, et le redessin à faire quand ça change.
///
/// `redraw` est le geste coûteux (dans l'Atelier : poser le signal qui remonte `ViewBody`).
/// `ViewBody` prend sa vue **en valeur**, non réactive : sans cette comparaison, chaque
/// rendu serait un remontage complet. R0 : 6,2 ms contre 0,85–1,23 ms pour 200 messages.
pub struct Screen<F: Fn(&Arc<Vec<ViewDesc>>)> {
    current: Mutex<Option<Arc<Vec<ViewDesc>>>>,
    redraw: F,
    redraws: AtomicU64,
    skipped: AtomicU64,
}

impl<F: Fn(&Arc<Vec<ViewDesc>>)> Screen<F> {
    pub fn new(redraw: F) -> Self {
        Self {
            current: Mutex::new(None),
            redraw,
            redraws: AtomicU64::new(0),
            skipped: AtomicU64::new(0),
        }
    }

    /// Publie `tree` : redessine s'il diffère de l'arbre affiché, sinon ne fait rien.
    /// Un même `Arc` se reconnaît sans rien parcourir ; deux arbres distincts se comparent
    /// structurellement (`ViewDesc: PartialEq`).
    pub fn publish(&self, tree: &Arc<Vec<ViewDesc>>) -> Publication {
        {
            let mut current = self.current.lock().unwrap_or_else(PoisonError::into_inner);
            if let Some(shown) = current.as_ref()
                && (Arc::ptr_eq(shown, tree) || **shown == **tree)
            {
                self.skipped.fetch_add(1, Ordering::Relaxed);
                return Publication::Unchanged;
            }
            *current = Some(tree.clone());
        }
        // Le redessin se fait hors verrou : il peut republier.
        self.redraws.fetch_add(1, Ordering::Relaxed);
        (self.redraw)(tree);
        Publication::Redrawn
    }

    /// L'arbre affiché.
    pub fn shown(&self) -> Option<Arc<Vec<ViewDesc>>> {
        self.current
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// Nombre de redessins effectués.
    pub fn redraws(&self) -> u64 {
        self.redraws.load(Ordering::Relaxed)
    }

    /// Nombre de publications évitées parce que l'arbre était identique.
    pub fn skipped(&self) -> u64 {
        self.skipped.load(Ordering::Relaxed)
    }
}

impl<R: ViewRenderer + Sync> ViewHost<R> {
    /// [`serve`](Self::serve), puis [`Screen::publish`] : le geste complet d'un déclencheur.
    pub async fn serve_into<F: Fn(&Arc<Vec<ViewDesc>>)>(
        &self,
        screen: &Screen<F>,
        module: &str,
        surface: Surface,
        request: &serde_json::Value,
        limits: &Limits,
        trigger: Trigger,
    ) -> Result<Publication, ServeError> {
        let served = self
            .serve(module, surface, request, limits, trigger)
            .await?;
        Ok(screen.publish(&served.views))
    }
}

#[cfg(feature = "wasmi")]
pub use wasmi_views::WasmiViews;

#[cfg(feature = "wasmi")]
mod wasmi_views {
    use super::*;
    use crate::call::{Args, CompiledModule};
    use crate::render::Call;

    /// Le moteur `wasmi` qui **garde la compilation** de chaque module installé.
    ///
    /// Vit ici, à côté du contrat de rendu et de `call_json`, parce que c'est le seul
    /// endroit où le harnais parle à `wasmi` pour un module « à données » : le mettre dans
    /// l'hôte aurait obligé chaque hôte à réécrire la même table. Il n'est pas dans
    /// `WasmiExecutor` (sans état, utilisé aussi pour `run`) : garder de l'état dans un
    /// type sans état changerait son comportement.
    #[derive(Default)]
    pub struct WasmiViews {
        compiled: Mutex<HashMap<ModuleKey, Arc<CompiledModule>>>,
        compilations: AtomicU64,
        instantiations: AtomicU64,
    }

    impl WasmiViews {
        pub fn new() -> Self {
            Self::default()
        }
        /// Compilations réellement faites.
        pub fn compilations(&self) -> u64 {
            self.compilations.load(Ordering::Relaxed)
        }
        /// Instances créées (une par appel exécuté).
        pub fn instantiations(&self) -> u64 {
            self.instantiations.load(Ordering::Relaxed)
        }
        /// Modules dont la compilation est gardée.
        pub fn kept(&self) -> usize {
            self.compiled
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .len()
        }
    }

    #[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
    #[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
    impl ViewRenderer for WasmiViews {
        async fn render(
            &self,
            program: &[u8],
            request: &[u8],
            limits: &Limits,
        ) -> Result<Call, ExecutorError> {
            self.instantiations.fetch_add(1, Ordering::Relaxed);
            self.compilations.fetch_add(1, Ordering::Relaxed);
            crate::render::call_bounded(program, request, limits)
        }

        async fn render_module(
            &self,
            key: &ModuleKey,
            program: &[u8],
            request: &[u8],
            limits: &Limits,
        ) -> Result<Call, ExecutorError> {
            let kept = self
                .compiled
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .get(key)
                .cloned();
            let module = match kept {
                Some(m) => m,
                None => {
                    let m = Arc::new(CompiledModule::compile(program)?);
                    self.compilations.fetch_add(1, Ordering::Relaxed);
                    self.compiled
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner)
                        .insert(key.clone(), m.clone());
                    m
                }
            };
            self.instantiations.fetch_add(1, Ordering::Relaxed);
            module.call(render::RENDER_EXPORT, &Args::one(request), limits)
        }

        fn release(&self, key: &ModuleKey) {
            self.compiled
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .remove(key);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::call::Call;
    use serde_json::json;
    use std::future::Future;
    use std::pin::pin;
    use std::sync::atomic::AtomicUsize;
    use std::task::{Context, Poll, Waker};

    /// Les futurs de ces tests sont prêts du premier coup : pas besoin d'exécuteur.
    fn block_on<T>(f: impl Future<Output = T>) -> T {
        let mut f = pin!(f);
        let mut cx = Context::from_waker(Waker::noop());
        match f.as_mut().poll(&mut cx) {
            Poll::Ready(v) => v,
            Poll::Pending => panic!("futur en attente : ces tests n'attendent rien"),
        }
    }

    /// Une vue valide qui lit l'entité `order` ; `title` (son nom) la distingue d'une autre.
    fn view(title: &str) -> serde_json::Value {
        let name = format!("v_{}", title.to_lowercase().replace([' ', '+'], "_"));
        json!({"name":name,"surface":"main","root":{"type":"leaf",
            "brick":{"type":"list","entity":"order","label":"id"}}})
    }

    fn descriptor(version: &str) -> ModuleDescriptor {
        let mut d = ModuleDescriptor::new("shop", version);
        d.entities.push(this_descriptor::EntityDesc::new("order"));
        d
    }

    /// Un moteur qui rend la requête telle quelle (le module « écho ») et compte ses appels.
    #[derive(Default)]
    struct Echo {
        calls: AtomicUsize,
        released: AtomicUsize,
    }

    #[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
    #[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
    impl ViewRenderer for Echo {
        async fn render(
            &self,
            _program: &[u8],
            request: &[u8],
            _limits: &Limits,
        ) -> Result<Call, ExecutorError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(Call {
                response: request.to_vec(),
                fuel_used: 1,
            })
        }
        fn release(&self, _key: &ModuleKey) {
            self.released.fetch_add(1, Ordering::SeqCst);
        }
    }

    fn host() -> ViewHost<Echo> {
        let host = ViewHost::new(Echo::default());
        host.install(descriptor("1.0.0"), b"v1".to_vec());
        host
    }

    fn serve(
        host: &ViewHost<Echo>,
        request: &serde_json::Value,
        trigger: Trigger,
    ) -> Result<Served, ServeError> {
        block_on(host.serve("shop", Surface::Main, request, &Limits::view(), trigger))
    }

    #[test]
    fn two_identical_calls_run_the_module_once() {
        let host = host();
        let request = view("a");
        let first = serve(&host, &request, Trigger::ScreenOpened).unwrap();
        let second = serve(&host, &request, Trigger::ScreenOpened).unwrap();
        assert!(!first.from_cache && second.from_cache);
        assert_eq!(host.renderer().calls.load(Ordering::SeqCst), 1);
        let stats = host.stats();
        assert_eq!(
            (stats.requests, stats.executions, stats.cache_hits),
            (2, 1, 1)
        );
        assert!(Arc::ptr_eq(&first.views, &second.views));
    }

    #[test]
    fn another_request_or_surface_is_another_call() {
        let host = host();
        serve(&host, &view("a"), Trigger::ScreenOpened).unwrap();
        serve(&host, &view("b"), Trigger::QueryWritten).unwrap();
        assert_eq!(host.renderer().calls.load(Ordering::SeqCst), 2);
        // Une autre surface n'est pas la même entrée : la vue est `main`, donc `sidebar`
        // n'a rien à montrer, et ce refus est nommé.
        let refused = block_on(host.serve(
            "shop",
            Surface::Sidebar,
            &view("a"),
            &Limits::view(),
            Trigger::ScreenOpened,
        ));
        assert!(matches!(
            refused,
            Err(ServeError::NoViewForSurface {
                surface: Surface::Sidebar
            })
        ));
    }

    #[test]
    fn a_new_version_invalidates_the_cache_and_releases_the_old_one() {
        let host = host();
        let request = view("a");
        serve(&host, &request, Trigger::ScreenOpened).unwrap();
        host.install(descriptor("1.1.0"), b"v2".to_vec());
        assert_eq!(host.renderer().released.load(Ordering::SeqCst), 1);
        let again = serve(&host, &request, Trigger::ScreenOpened).unwrap();
        assert!(
            !again.from_cache,
            "la version a changé : on ne répond pas depuis l'ancien cache"
        );
        assert_eq!(host.renderer().calls.load(Ordering::SeqCst), 2);
        // Réinstaller exactement la même chose ne casse rien.
        host.install(descriptor("1.1.0"), b"v2".to_vec());
        assert!(
            serve(&host, &request, Trigger::ScreenOpened)
                .unwrap()
                .from_cache
        );
        host.uninstall("shop");
        assert!(matches!(
            serve(&host, &request, Trigger::ScreenOpened),
            Err(ServeError::NotInstalled(_))
        ));
        assert_eq!(host.renderer().released.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn validation_is_not_redone_for_a_tree_already_judged() {
        let host = host();
        // Deux requêtes différentes qui rendent le même arbre (le module écho rend la
        // requête : on les distingue par un champ ignoré… sauf qu'il est refusé).
        // On passe donc par deux espaces blancs différents : mêmes arbre, autres octets.
        let compact = view("a");
        serve(&host, &compact, Trigger::ScreenOpened).unwrap();
        serve(&host, &view("b"), Trigger::QueryWritten).unwrap();
        serve(&host, &view("a"), Trigger::ConversationChanged).unwrap(); // cache
        let s = host.stats();
        assert_eq!(s.executions, 2);
        assert_eq!(
            s.validations, 2,
            "un arbre par réponse distincte, jamais un de plus"
        );
        assert_eq!(s.by_trigger, [1, 1, 1]);

        // Même réponse, requêtes différentes : un module qui ignore une partie de sa
        // requête rend le même arbre, et celui-ci ne se revalide pas.
        struct Const(Vec<u8>);
        #[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
        #[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
        impl ViewRenderer for Const {
            async fn render(&self, _: &[u8], _: &[u8], _: &Limits) -> Result<Call, ExecutorError> {
                Ok(Call {
                    response: self.0.clone(),
                    fuel_used: 0,
                })
            }
        }
        let host = ViewHost::new(Const(serde_json::to_vec(&view("c")).unwrap()));
        host.install(descriptor("1.0.0"), vec![]);
        for n in 0..5 {
            block_on(host.serve(
                "shop",
                Surface::Main,
                &json!({ "page": n }),
                &Limits::view(),
                Trigger::QueryWritten,
            ))
            .unwrap();
        }
        let s = host.stats();
        assert_eq!((s.executions, s.validations, s.memo_hits), (5, 1, 4));
    }

    #[test]
    fn a_tree_that_contradicts_the_descriptor_is_refused_and_remembered() {
        let host = host();
        let bad = json!({"name":"x","surface":"main","root":{"type":"leaf",
            "brick":{"type":"list","entity":"ghost","label":"id"}}});
        for _ in 0..2 {
            match serve(&host, &bad, Trigger::ScreenOpened) {
                Err(ServeError::Rejected { issues }) => {
                    assert!(issues.iter().any(|i| i.contains("ghost")), "{issues:?}");
                }
                other => panic!("attendu un refus nommé, reçu {other:?}"),
            }
        }
        assert_eq!(
            host.stats().executions,
            1,
            "un refus est un résultat comme un autre"
        );
    }

    #[test]
    fn an_unchanged_tree_causes_no_redraw_and_a_changed_one_causes_one() {
        let host = host();
        let drawn = Arc::new(AtomicUsize::new(0));
        let counter = drawn.clone();
        let screen = Screen::new(move |_| {
            counter.fetch_add(1, Ordering::SeqCst);
        });
        let go = |request: &serde_json::Value, trigger| {
            block_on(host.serve_into(
                &screen,
                "shop",
                Surface::Main,
                request,
                &Limits::view(),
                trigger,
            ))
            .unwrap()
        };
        assert_eq!(go(&view("a"), Trigger::ScreenOpened), Publication::Redrawn);
        assert_eq!(
            go(&view("a"), Trigger::QueryWritten),
            Publication::Unchanged
        );
        assert_eq!(
            go(&view("a"), Trigger::ConversationChanged),
            Publication::Unchanged
        );
        assert_eq!(drawn.load(Ordering::SeqCst), 1);
        assert_eq!(go(&view("b"), Trigger::QueryWritten), Publication::Redrawn);
        assert_eq!(drawn.load(Ordering::SeqCst), 2);
        // Retour à `a` : l'arbre diffère de celui affiché (`b`), donc redessin, même si le
        // cache a répondu sans exécuter le module.
        assert_eq!(
            go(&view("a"), Trigger::ConversationChanged),
            Publication::Redrawn
        );
        assert_eq!((screen.redraws(), screen.skipped()), (3, 2));
        assert_eq!(host.renderer().calls.load(Ordering::SeqCst), 2);
    }

    /// Deux arbres structurellement égaux mais rendus par deux réponses distinctes (autres
    /// octets, mêmes valeurs) ne redessinent pas non plus.
    #[test]
    fn equal_trees_from_different_bytes_do_not_redraw() {
        let host = host();
        let screen = Screen::new(|_| {});
        let a = view("a");
        let spaced = format!(" {} ", serde_json::to_string(&a).unwrap());
        let tree = |text: &str| Arc::new(render::views_of(text.as_bytes(), 4096).unwrap());
        let _ = host;
        assert_eq!(screen.publish(&tree(&spaced)), Publication::Redrawn);
        let second = tree(&serde_json::to_string(&a).unwrap());
        assert_eq!(screen.publish(&second), Publication::Unchanged);
    }

    /// Mesure (pas une assertion) : `cargo test -p this-module-harness --features wasmi
    /// --release -- --ignored --nocapture measure`.
    #[test]
    #[ignore]
    fn measure_publish_comparison_cost() {
        let rows: Vec<serde_json::Value> = (0..200)
            .map(|i| json!({"type":"leaf","brick":{"type":"list","entity":"order","label":format!("champ_{i}")}}))
            .collect();
        let big = json!({"name":"thread","surface":"main",
            "root":{"type":"stack","children":rows}});
        let parse = |v: &serde_json::Value| {
            Arc::new(render::views_of(&serde_json::to_vec(v).unwrap(), usize::MAX).unwrap())
        };
        let (a, b) = (parse(&big), parse(&big));
        let screen = Screen::new(|_| {});
        screen.publish(&a);
        let n = 2000;
        let t = std::time::Instant::now();
        for _ in 0..n {
            assert_eq!(screen.publish(&b), Publication::Unchanged);
        }
        println!(
            "publier un arbre identique (200 lignes, deux Arc distincts) : {:?} par publication",
            t.elapsed() / n
        );
    }
}

#[cfg(all(test, feature = "wasmi"))]
mod engine_tests {
    use super::*;
    use serde_json::json;
    use std::future::Future;
    use std::pin::pin;
    use std::task::{Context, Poll, Waker};
    use this_module_lang::assemble::assemble;

    fn block_on<T>(f: impl Future<Output = T>) -> T {
        let mut f = pin!(f);
        match f.as_mut().poll(&mut Context::from_waker(Waker::noop())) {
            Poll::Ready(v) => v,
            Poll::Pending => panic!("futur en attente"),
        }
    }

    /// Le module « écho » : `render` rend la requête telle quelle, `alloc` est un
    /// allocateur « bump » dont le curseur est exporté (`next`) pour qu'on puisse y lire
    /// ce qu'un appel laisse derrière lui — l'équivalent de l'`INBOX` du SDK.
    fn echo_module() -> Vec<u8> {
        assemble(
            r#"(module
  (memory (export "memory") 2)
  (global $next (export "next") (mut i32) (i32.const 1024))
  (func (export "alloc") (param $len i32) (result i32)
    (local $ptr i32)
    (local.set $ptr (global.get $next))
    (global.set $next (i32.add (global.get $next) (local.get $len)))
    (local.get $ptr))
  (func (export "render") (param $ptr i32) (param $len i32) (result i64)
    (i64.or (i64.shl (i64.extend_i32_u (local.get $ptr)) (i64.const 32))
            (i64.extend_i32_u (local.get $len)))))"#,
        )
        .expect("WAT assemblé")
    }

    fn view(title: &str) -> serde_json::Value {
        let name = format!("v_{}", title.to_lowercase().replace([' ', '+'], "_"));
        json!({"name":name,"surface":"main","root":{"type":"leaf",
            "brick":{"type":"list","entity":"order","label":"id"}}})
    }

    fn descriptor(version: &str) -> ModuleDescriptor {
        let mut d = ModuleDescriptor::new("shop", version);
        d.entities.push(this_descriptor::EntityDesc::new("order"));
        d
    }

    fn serve(
        host: &ViewHost<WasmiViews>,
        request: &serde_json::Value,
        trigger: Trigger,
    ) -> Result<Served, ServeError> {
        block_on(host.serve("shop", Surface::Main, request, &Limits::view(), trigger))
    }

    /// Le scénario d'une conversation, tel qu'un écran le vit : ouverture, 40 frappes
    /// (qui ne déclenchent rien), envoi, changement de fil, retour, réouverture.
    /// Avant : un `call_json` par rendu demandé, y compris à chaque touche.
    /// Après : le serveur de vues.
    #[test]
    fn a_conversation_scenario_runs_the_module_far_less_often() {
        let thread_a = view("fil A");
        let thread_a_sent = view("fil A + un message");
        let thread_b = view("fil B");
        // (déclencheur, requête). Les touches ne sont pas ici : elles ne sont pas un
        // déclencheur, l'hôte ne rend pas.
        let events = [
            (Trigger::ScreenOpened, &thread_a),
            (Trigger::QueryWritten, &thread_a_sent),
            (Trigger::ConversationChanged, &thread_b),
            (Trigger::ConversationChanged, &thread_a_sent),
            (Trigger::ScreenOpened, &thread_a_sent),
            (Trigger::ConversationChanged, &thread_b),
        ];
        let keystrokes = 40;

        // AVANT : un rendu par événement ET par frappe, chacun recompile et exécute.
        let wasm = echo_module();
        let before = events.len() + keystrokes;
        let t = std::time::Instant::now();
        for i in 0..before {
            let request = serde_json::to_vec(events[i % events.len()].1).unwrap();
            crate::call::call_json(
                &wasm,
                render::RENDER_EXPORT,
                &crate::call::Args::one(&request),
                &Limits::view(),
            )
            .unwrap();
        }
        let before_time = t.elapsed();

        // APRÈS : les six événements, par le serveur.
        let host = ViewHost::new(WasmiViews::new());
        host.install(descriptor("1.0.0"), wasm);
        let t = std::time::Instant::now();
        for (trigger, request) in events {
            serve(&host, request, trigger).unwrap();
        }
        let after_time = t.elapsed();
        let stats = host.stats();
        println!(
            "SCÉNARIO avant : {before} exécutions, {before} compilations, {before_time:?} | \
             après : {} exécutions, {} compilations, {} instanciations, {} validations, \
             {} réponses du cache, {after_time:?}",
            stats.executions,
            host.renderer().compilations(),
            host.renderer().instantiations(),
            stats.validations,
            stats.cache_hits,
        );
        assert_eq!(
            stats.executions, 3,
            "trois requêtes distinctes, pas une de plus"
        );
        assert_eq!(host.renderer().compilations(), 1);
        assert_eq!(stats.cache_hits, 3);
        assert!(stats.executions < before as u64);
    }

    #[test]
    fn the_compilation_is_kept_per_module_and_freed_with_it() {
        let host = ViewHost::new(WasmiViews::new());
        host.install(descriptor("1.0.0"), echo_module());
        for i in 0..10 {
            serve(&host, &view(&format!("{i}")), Trigger::QueryWritten).unwrap();
        }
        assert_eq!(
            host.renderer().compilations(),
            1,
            "dix appels, une compilation"
        );
        assert_eq!(host.renderer().instantiations(), 10);
        assert_eq!(host.renderer().kept(), 1);
        host.install(descriptor("2.0.0"), echo_module());
        assert_eq!(
            host.renderer().kept(),
            0,
            "la version remplacée est libérée"
        );
        serve(&host, &view("x"), Trigger::ScreenOpened).unwrap();
        assert_eq!(host.renderer().kept(), 1);
        host.uninstall("shop");
        assert_eq!(
            host.renderer().kept(),
            0,
            "désinstallé : plus rien de gardé"
        );
    }

    fn cursor(store: &impl wasmi::AsContext, instance: &wasmi::Instance) -> i32 {
        match instance
            .get_global(store, "next")
            .expect("global `next`")
            .get(store)
        {
            wasmi::Val::I32(n) => n,
            other => panic!("curseur inattendu : {other:?}"),
        }
    }

    /// L'`INBOX` du SDK est une liste qui grossit à chaque `alloc` et qu'on ne vide qu'à
    /// la sortie de `handle`. Le curseur du module écho joue le même rôle : il avance à
    /// chaque `alloc`. Mesure avant/après sur 100 appels.
    #[test]
    fn the_inbox_does_not_leak_over_100_calls() {
        let wasm = echo_module();
        let request = serde_json::to_vec(&view("fil")).unwrap();
        let args = crate::call::Args::one(&request);

        // AVANT : une instance réutilisée pour 100 appels (ce que ferait une façade
        // naïve qui garde l'instance).
        let mut config = wasmi::Config::default();
        config.consume_fuel(true);
        let engine = wasmi::Engine::new(&config);
        let module = wasmi::Module::new(&engine, &wasm[..]).unwrap();
        let mut store = wasmi::Store::new(&engine, ());
        store.set_fuel(u64::MAX / 2).unwrap();
        let instance = wasmi::Instance::new(&mut store, &module, &[]).unwrap();
        let alloc = instance
            .get_typed_func::<i32, i32>(&store, "alloc")
            .unwrap();
        let start = cursor(&store, &instance);
        for _ in 0..100 {
            alloc.call(&mut store, request.len() as i32).unwrap();
        }
        let reused = cursor(&store, &instance) - start;

        // APRÈS : le module compilé, une instance à neuf par appel.
        let compiled = crate::call::CompiledModule::compile(&wasm).unwrap();
        let mut residues = Vec::new();
        let limits = Limits::view();
        for _ in 0..100 {
            let (_, store, instance) = compiled.run(render::RENDER_EXPORT, &args, &limits).unwrap();
            residues.push(cursor(&store, &instance) - 1024);
        }
        println!(
            "INBOX sur 100 appels — avant (instance réutilisée) : +{reused} o au curseur \
             ({} o/appel) ; après : {:?} o par appel, min {} max {}",
            reused / 100,
            residues.first().unwrap(),
            residues.iter().min().unwrap(),
            residues.iter().max().unwrap()
        );
        assert_eq!(reused as usize, 100 * request.len());
        assert!(
            residues.iter().all(|r| *r as usize == request.len()),
            "chaque appel ne laisse que sa propre requête : {residues:?}"
        );
    }

    fn looping_module() -> Vec<u8> {
        assemble(
            r#"(module
  (memory (export "memory") 1)
  (func (export "alloc") (param $len i32) (result i32) (i32.const 16))
  (func (export "render") (param $ptr i32) (param $len i32) (result i64)
    (loop $forever (br $forever))
    (i64.const 0)))"#,
        )
        .unwrap()
    }

    fn greedy_module() -> Vec<u8> {
        assemble(
            r#"(module
  (memory (export "memory") 65)
  (func (export "alloc") (param $len i32) (result i32) (i32.const 16))
  (func (export "render") (param $ptr i32) (param $len i32) (result i64) (i64.const 0)))"#,
        )
        .unwrap()
    }

    #[test]
    fn a_module_beyond_the_view_bounds_gets_a_named_refusal() {
        // Fuel : une boucle est arrêtée par `Limits::view()`, et le refus est mis en cache.
        let host = ViewHost::new(WasmiViews::new());
        host.install(descriptor("1.0.0"), looping_module());
        for _ in 0..2 {
            match serve(&host, &view("x"), Trigger::ScreenOpened) {
                Err(ServeError::Execution(ExecutorError::OutOfFuel { fuel })) => {
                    assert_eq!(fuel, Limits::view().fuel);
                }
                other => panic!("attendu OutOfFuel, reçu {other:?}"),
            }
        }
        assert_eq!(
            host.stats().executions,
            1,
            "le refus ne coûte pas le fuel deux fois"
        );

        // Mémoire : 65 pages déclarées, plafond 64.
        host.install(descriptor("2.0.0"), greedy_module());
        assert!(matches!(
            serve(&host, &view("x"), Trigger::ScreenOpened),
            Err(ServeError::Execution(ExecutorError::Memory { .. }))
        ));

        // Réponse : plus de 32 Kio.
        host.install(descriptor("3.0.0"), echo_module());
        let big = json!({"name":"orders","surface":"main","root":{"type":"leaf",
            "brick":{"type":"list","entity":"order","label":"x".repeat(33 * 1024)}}});
        match serve(&host, &big, Trigger::ScreenOpened) {
            Err(ServeError::Execution(ExecutorError::RequestTooLarge { .. }))
            | Err(ServeError::Execution(ExecutorError::ResponseTooLarge { .. })) => {}
            other => panic!("attendu un refus de taille, reçu {other:?}"),
        }
    }

    /// Mesure : `cargo test -p this-module-harness --features wasmi --release --
    /// --ignored --nocapture measure_wasmi`.
    #[test]
    #[ignore]
    fn measure_wasmi_compile_vs_kept() {
        let wasm = echo_module();
        let request = serde_json::to_vec(&view("fil")).unwrap();
        let args = crate::call::Args::one(&request);
        let n = 2000;
        let t = std::time::Instant::now();
        for _ in 0..n {
            crate::call::call_json(&wasm, render::RENDER_EXPORT, &args, &Limits::view()).unwrap();
        }
        let recompiled = t.elapsed() / n;
        let compiled = crate::call::CompiledModule::compile(&wasm).unwrap();
        let t = std::time::Instant::now();
        for _ in 0..n {
            compiled
                .call(render::RENDER_EXPORT, &args, &Limits::view())
                .unwrap();
        }
        let kept = t.elapsed() / n;
        println!("call_json (recompile) : {recompiled:?}/appel ; module gardé : {kept:?}/appel");
    }
}
