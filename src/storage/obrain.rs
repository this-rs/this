//! Obrain-backed storage implementations of DataService and LinkService
//!
//! Provides graph-oriented storage using obrain-engine's native property graph.
//! Entities are stored as graph nodes (label = entity_type, properties = JSON-serialized
//! fields), and links as graph edges (edge_type = link_type).
//!
//! # Architecture
//!
//! Both `ObrainDataService` and `ObrainLinkService` share an `Arc<ObrainStore>` which
//! holds the `ObrainDB` instance and a UUID→NodeId index. This index is necessary
//! because this.rs uses `Uuid` identifiers while obrain uses internal `NodeId` (u64).
//!
//! # Engine & durability
//!
//! The backend couples **this-rs/obrain** (the ultra-benchmarked cognitive graph
//! engine — fastest tested graph DB on the LDBC Social Network Benchmark, in
//! embedded and server configurations). Local checkout: `~/projects/banc-obrain`
//! (workspace `crates/obrain-{engine,core,common,…}`); the path dependencies in
//! `Cargo.toml` point at it, mirroring aletheia's path-dep convention.
//!
//! `ObrainStore::open(path)` opens a **durable substrate store**: the engine
//! persists mmap zones/dict columns and a write-ahead log with `EveryCommit`
//! fsync, and every mutation path in this module ends in `commit_wal()`, so a
//! crash between requests cannot lose an acknowledged write. This holds
//! whatever transport delivered the mutation (REST, WebSocket, SSE, gRPC — or
//! the vite-rs `/__dev` bridge in dev): once the storage service returns Ok,
//! the write is on disk. After reopening a store, the in-memory UUID indexes
//! must be rebuilt from the persisted labels — see [`ObrainStore::rebuild_index_for_label`]
//! and [`ObrainStore::rebuild_link_index`] (tests assert data, links and search
//! all survive a reopen).
//!
//! # Feature flag
//!
//! Gated behind `#[cfg(feature = "obrain")]`. Enable with:
//! ```toml
//! this-rs = { version = "0.0.9", features = ["obrain"] }
//! ```

use crate::core::field::FieldValue;
use crate::core::link::LinkEntity;
use crate::core::{Data, DataService, LinkService};
use anyhow::{Result, anyhow};
use async_trait::async_trait;
use obrain_common::types::{NodeId, Value as ObrainValue};
use obrain_core::Node;
use obrain_engine::ObrainDB;
use serde::Serialize;
use serde::de::DeserializeOwned;
use std::collections::HashMap;
use std::marker::PhantomData;
use std::sync::Mutex;
use std::sync::{Arc, RwLock};
use uuid::Uuid;

// Re-export for advanced users
pub use obrain_common::types::NodeId as ObrainNodeId;

// ---------------------------------------------------------------------------
// ObrainStore — shared state between DataService and LinkService
// ---------------------------------------------------------------------------

/// Shared obrain database handle with UUID↔NodeId mapping.
///
/// Both `ObrainDataService` and `ObrainLinkService` hold an `Arc<ObrainStore>`
/// to share the same database and index. The UUID index is necessary because
/// this.rs identifies entities by `Uuid` while obrain-engine uses internal `NodeId`.
pub struct ObrainStore {
    /// The obrain database instance
    db: ObrainDB,
    /// Sérialise les `commit_wal()` : deux commits concurrents sur le même
    /// moteur se marchent dessus (flush substrate → ENOENT). Le contrat du
    /// harnais exige des creates parallèles sûrs (test_concurrent_access).
    commit_lock: Mutex<()>,
    /// UUID → NodeId index for O(1) entity lookups
    uuid_to_node: RwLock<HashMap<Uuid, NodeId>>,
    /// Link UUID → (EdgeId-placeholder: source_node, target_node, link_uuid)
    /// We store links as nodes with label `_link:{link_type}` because obrain edges
    /// require both endpoints to exist as nodes. Since LinkService is entity-agnostic
    /// (source/target may not exist yet), we use labeled nodes for links.
    link_nodes: RwLock<HashMap<Uuid, NodeId>>,
    /// Espace actif (graphe nommé) — `None` = graphe par défaut.
    ///
    /// **C'est ici, et pas dans obrain, que vit l'espace courant.** Voir
    /// [`ObrainStore::use_graph`] : le contexte de graphe côté `ObrainDB` ne
    /// propage rien sur le chemin substrat, donc this-rs doit le porter lui-même
    /// et l'appliquer à chaque session qu'il ouvre ([`ObrainStore::session`]).
    space: RwLock<Option<String>>,
    /// Index d'identité canonique `Uuid → handle`, **cloisonné par espace**.
    ///
    /// Séparé de `uuid_to_node` à dessein : `ObrainDataService` ne le lit jamais,
    /// donc le chemin historique est inchangé (patch additif). Voir
    /// `crate::core::identity` et `storage::obrain_identity`.
    identity: RwLock<crate::core::identity::IdentityIndex>,
    /// Le bail d'écriture, quand le fencing est armé.
    ///
    /// `None` = comportement historique (aucun fencing). Voir
    /// [`ObrainStore::enable_write_fencing`] et `storage::writer_lease`.
    lease: RwLock<Option<Arc<crate::storage::writer_lease::WriterLeaseHandle>>>,
    /// Identité stable d'un store **volatile** (substrat temporaire) : sans
    /// chemin, c'est le seul moyen de nommer le shard.
    volatile_id: u64,
    /// Qui a délivré le bail en vigueur — publié, jamais deviné.
    lease_authority: RwLock<Option<crate::storage::writer_lease::AuthorityKind>>,
}

/// Compteur d'instances volatiles — monotone, pour que deux stores en mémoire
/// ne partagent jamais une identité de shard.
fn next_volatile_id() -> u64 {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

impl ObrainStore {
    /// Create a new in-memory obrain store
    pub fn new_in_memory() -> Arc<Self> {
        Arc::new(Self {
            db: ObrainDB::new_in_memory(),
            commit_lock: Mutex::new(()),
            uuid_to_node: RwLock::new(HashMap::new()),
            link_nodes: RwLock::new(HashMap::new()),
            space: RwLock::new(None),
            identity: RwLock::new(crate::core::identity::IdentityIndex::default()),
            lease: RwLock::new(None),
            volatile_id: next_volatile_id(),
            lease_authority: RwLock::new(None),
        })
    }

    /// Create a new persistent obrain store
    pub fn open(path: impl AsRef<std::path::Path>) -> Result<Arc<Self>> {
        let db = ObrainDB::open(path).map_err(|e| anyhow!("Failed to open ObrainDB: {}", e))?;
        Ok(Arc::new(Self {
            db,
            commit_lock: Mutex::new(()),
            uuid_to_node: RwLock::new(HashMap::new()),
            link_nodes: RwLock::new(HashMap::new()),
            space: RwLock::new(None),
            identity: RwLock::new(crate::core::identity::IdentityIndex::default()),
            lease: RwLock::new(None),
            volatile_id: next_volatile_id(),
            lease_authority: RwLock::new(None),
        }))
    }

    /// Ouvre un store persistant en **lecture seule stricte**.
    ///
    /// C'est le mode à utiliser pour **publier** le relief d'un store qu'on ne
    /// possède pas : obrain refuse alors les mutations au niveau moteur, et le
    /// verrou est partagé (`LOCK_SH`) — plusieurs processus peuvent lire la
    /// même base simultanément, ce que l'ouverture normale interdit.
    pub fn open_read_only(path: impl AsRef<std::path::Path>) -> Result<Arc<Self>> {
        let db = ObrainDB::open_read_only(path)
            .map_err(|e| anyhow!("Failed to open ObrainDB read-only: {}", e))?;
        Ok(Arc::new(Self {
            db,
            commit_lock: Mutex::new(()),
            uuid_to_node: RwLock::new(HashMap::new()),
            link_nodes: RwLock::new(HashMap::new()),
            space: RwLock::new(None),
            identity: RwLock::new(crate::core::identity::IdentityIndex::default()),
            lease: RwLock::new(None),
            volatile_id: next_volatile_id(),
            lease_authority: RwLock::new(None),
        }))
    }

    /// Create a store wrapping an existing ObrainDB instance
    pub fn from_db(db: ObrainDB) -> Arc<Self> {
        Arc::new(Self {
            db,
            commit_lock: Mutex::new(()),
            uuid_to_node: RwLock::new(HashMap::new()),
            link_nodes: RwLock::new(HashMap::new()),
            space: RwLock::new(None),
            identity: RwLock::new(crate::core::identity::IdentityIndex::default()),
            lease: RwLock::new(None),
            volatile_id: next_volatile_id(),
            lease_authority: RwLock::new(None),
        })
    }

    /// Access the underlying ObrainDB
    pub fn db(&self) -> &ObrainDB {
        &self.db
    }

    /// Commit WAL sérialisé — sûr sous creates parallèles (cf. commit_lock).
    pub fn commit_wal(&self) -> Result<()> {
        let _g = self
            .commit_lock
            .lock()
            .map_err(|_| anyhow!("commit lock poisoned"))?;
        self.db
            .commit_wal()
            .map_err(|e| anyhow!("commit_wal failed: {}", e))
    }

    /// Ouvre une session **dans l'espace actif**.
    ///
    /// **Toutes** les opérations du module passent par ici. C'est le seul point
    /// où l'espace est appliqué, et c'est indispensable : `ObrainDB::session()`
    /// ne propage PAS le contexte de graphe du moteur sur le chemin substrat.
    /// Le code de propagation existe (`obrain-engine/src/database/mod.rs:1305-1308`)
    /// mais il est **inatteignable** — les bases adossées à un substrat sortent
    /// par un retour anticipé quelques dizaines de lignes plus haut
    /// (`mod.rs:1229-1242`, « this early return is the path EVERY real session
    /// takes »). Vérifié empiriquement : `db.set_current_graph(Some("a"))` suivi
    /// de `db.session()` rend une session dont `current_graph()` vaut `None`,
    /// et toutes les lectures retombent sur le graphe par défaut.
    ///
    /// `Session::use_graph`, en revanche, fonctionne : c'est donc à this-rs de
    /// le poser, session par session.
    pub(crate) fn session(&self) -> obrain_engine::Session {
        let session = self.db.session();
        if let Some(space) = self.space_snapshot() {
            session.use_graph(&space);
        }
        session
    }

    /// L'espace actif, cloné (le verrou n'est pas tenu pendant l'usage).
    fn space_snapshot(&self) -> Option<String> {
        self.space.read().ok().and_then(|s| s.clone())
    }

    /// Le magasin de l'espace actif — graphe nommé s'il existe, racine sinon.
    ///
    /// Sert aux reconstructions d'index : elles doivent balayer le magasin de
    /// l'espace courant, pas la racine (`ObrainDB::store()` rend toujours la
    /// racine).
    pub(crate) fn active_store(&self) -> Arc<dyn obrain_engine::GraphStoreMut> {
        let root = self.db.store().clone();
        match self.space_snapshot() {
            Some(name) => root.named_graph(&name).unwrap_or(root),
            None => root,
        }
    }

    /// Bascule l'espace actif (graphe nommé) — la dimension « spacieux ».
    ///
    /// # Pourquoi ce n'est pas un simple appel à obrain
    ///
    /// Obrain sait faire des graphes nommés — ce sont des **sous-substrats
    /// physiques** (`<store>/graphs/<nom>`, `obrain-substrate/src/store.rs:15923`)
    /// avec leur propre portée d'identité (`graph_scope()`,
    /// `obrain-core/src/graph/traits.rs:1060`). Mais son contexte de graphe
    /// persistant (`ObrainDB::set_current_graph`) ne propage rien sur le chemin
    /// substrat (cf. `ObrainStore::session()`, privé — les consommateurs passent par la façade
    /// `GraphBackend`, qui est cloisonnée par espace). this-rs porte donc
    /// l'espace lui-même et l'applique à chaque session.
    ///
    /// # Changement d'espace = invalidation du cache
    ///
    /// Les index `Uuid → NodeId` de ce store sont des **caches** : ils sont
    /// vidés à chaque bascule. C'est nécessaire, et non un détail : deux espaces
    /// sont deux substrats distincts qui allouent chacun leurs `NodeId` à
    /// partir de 1, donc un index partagé ferait résoudre l'entité d'un espace
    /// vers le nœud homonyme d'un autre. Après une bascule, la reprise est
    /// explicite — comme après une réouverture :
    ///
    /// ```rust,ignore
    /// store.use_graph("tenant-a");
    /// for label in ["user", "order"] {
    ///     store.rebuild_index_for_label(label)?;
    /// }
    /// ```
    ///
    /// # Concurrence
    ///
    /// L'espace est porté par le **store**, pas par la requête : deux appelants
    /// concurrents sur des espaces différents s'écrasent. Pour servir plusieurs
    /// espaces en parallèle, il faut un `ObrainStore` par espace.
    pub fn use_graph(&self, name: &str) {
        if let Ok(mut space) = self.space.write() {
            *space = Some(name.to_string());
        }
        self.invalidate_caches();
    }

    /// Quitte l'espace nommé (retour au graphe par défaut).
    ///
    /// Invalide les caches, comme [`ObrainStore::use_graph`].
    pub fn reset_graph(&self) {
        if let Ok(mut space) = self.space.write() {
            *space = None;
        }
        self.invalidate_caches();
    }

    /// Vide les caches `Uuid → NodeId` (entités et liens).
    /// Arme le **fencing d'écriture** : prend un bail pour le shard courant.
    ///
    /// À appeler **avant** de servir des écritures. Renvoie l'époque obtenue —
    /// à publier, pour qu'un opérateur sache quel writer est en titre.
    ///
    /// # Ce que ça change
    ///
    /// Chaque opération d'écriture vérifie le bail **en entrée** (voir
    /// [`ObrainStore::check_write`]). Sans bail, le comportement est celui
    /// d'avant ; avec, un writer périmé est **refusé** au lieu d'écrire dans le
    /// dos de son successeur.
    ///
    /// # Pourquoi en entrée et pas au commit
    ///
    /// Côté obrain, le WAL est écrit **avant** la mutation mmap
    /// (`obrain-substrate/src/writer.rs:287-309`). Un contrôle au commit
    /// laisserait donc la trace du writer périmé déjà sur le disque.
    ///
    /// # Exemple
    ///
    /// ```rust,ignore
    /// let store = ObrainStore::open("./data/app.obrain")?;
    /// let authority = FileAuthority::new("./data/app.obrain");
    /// let epoch = store.enable_write_fencing_with(&authority)?;  // 1
    /// // Un second writer sur le même store sera refusé par `acquire`.
    /// ```
    ///
    /// Le bail couvre `(store, espace courant)` : deux espaces d'un même store
    /// sont deux shards, donc deux baux indépendants.
    pub fn enable_write_fencing_with(
        &self,
        authority: &crate::storage::writer_lease::FileAuthority,
    ) -> Result<u64> {
        let shard = self.shard_id()?;
        let owner = crate::storage::writer_lease::OwnerId::this_process();
        let handle = authority.acquire(&shard, &owner)?;
        let epoch = handle.epoch();
        if let Ok(mut slot) = self.lease.write() {
            *slot = Some(Arc::new(handle));
        }
        if let Ok(mut slot) = self.lease_authority.write() {
            *slot = Some(crate::storage::writer_lease::LeaseAuthority::kind(
                authority,
            ));
        }
        tracing::info!(
            epoch,
            shard = %shard.key(),
            "fencing d'écriture armé (bail pris)"
        );
        Ok(epoch)
    }

    /// L'identité du shard courant — store + espace.
    pub fn shard_id(&self) -> Result<crate::storage::writer_lease::ShardId> {
        use crate::storage::writer_lease::{ShardId, StoreId};
        let store = match self.db.config().path.clone() {
            Some(path) => StoreId::Dir(path),
            None => StoreId::Volatile(self.volatile_id),
        };
        Ok(ShardId {
            store,
            space: self.space_snapshot(),
        })
    }

    /// L'état du bail, s'il est armé — **publié** par le relief.
    pub fn lease_info(&self) -> Option<crate::storage::writer_lease::LeaseInfo> {
        let slot = self.lease.read().ok()?;
        let handle = slot.as_ref()?;
        Some(crate::storage::writer_lease::LeaseInfo {
            epoch: handle.epoch(),
            shard: handle.shard().key(),
            authority: self.lease_authority.read().ok().and_then(|a| *a),
        })
    }

    /// L'époque du bail en vigueur, si le fencing est armé.
    pub fn lease_epoch(&self) -> Option<u64> {
        self.lease
            .read()
            .ok()
            .and_then(|slot| slot.as_ref().map(|h| h.epoch()))
    }

    /// **Le contrôle d'entrée d'écriture.**
    ///
    /// Un seul load atomique quand le fencing est armé ; un no-op sinon. C'est
    /// la garantie que le coût du fencing reste négligeable devant une écriture.
    /// Vérifie le bail d'écriture : à appeler **avant** toute écriture menée
    /// hors des services de données.
    ///
    /// Un programme qui écrit dans le store par ses propres moyens (un tableau de
    /// bord en mode écriture, un outil de migration, un job) doit passer par ici
    /// pour ne pas écrire dans le dos du détenteur du bail : c'est cette fonction
    /// qui refuse un écrivain périmé.
    ///
    /// Fail-closed : sans bail, le comportement est celui d'avant (aucune
    /// vérification) ; avec un bail, un écrivain périmé est **refusé**.
    pub fn check_write(&self) -> Result<()> {
        let guard = self
            .lease
            .read()
            .map_err(|_| anyhow!("bail d'écriture : verrou empoisonné"))?;
        match guard.as_ref() {
            Some(handle) => handle
                .assert_live()
                .map_err(|e| anyhow!("écriture refusée : {e}")),
            None => Ok(()),
        }
    }

    /// Rend le bail (relâche l'exclusion).
    pub fn release_write_fencing(&self) {
        if let Ok(mut slot) = self.lease_authority.write() {
            *slot = None;
        }
        if let Ok(mut slot) = self.lease.write()
            && let Some(handle) = slot.take()
            && let Ok(inner) = Arc::try_unwrap(handle)
        {
            inner.release();
        }
    }

    /// **Couture de test** : simule la reprise du bail par un autre écrivain.
    #[doc(hidden)]
    pub fn fence_write_lease_for_test(&self, by_epoch: u64) {
        if let Ok(slot) = self.lease.read()
            && let Some(handle) = slot.as_ref()
        {
            handle.fence_for_test(by_epoch);
        }
    }

    fn invalidate_caches(&self) {
        if let Ok(mut index) = self.uuid_to_node.write() {
            index.clear();
        }
        if let Ok(mut index) = self.link_nodes.write() {
            index.clear();
        }
        // L'index d'identité est cloisonné par espace : le purger en reprenant
        // l'espace courant, sans quoi il mélangerait deux espaces (dont les
        // NodeId repartent chacun de 1).
        if let Ok(mut index) = self.identity.write() {
            index.clear(self.space_snapshot());
        }
    }

    /// L'index d'identité, pour les adaptateurs du même crate.
    ///
    /// Volontairement `pub(crate)` : l'index est un détail d'implémentation du
    /// stockage, pas une surface publique. Le public passe par
    /// `ObrainStore::identity_get` / `identity_len` / `identity_space`
    /// (`storage::obrain_identity`).
    pub(crate) fn identity_index(&self) -> &RwLock<crate::core::identity::IdentityIndex> {
        &self.identity
    }

    /// Espace (graphe nommé) actuellement actif — `None` = graphe par défaut.
    pub fn space(&self) -> Option<String> {
        self.space_snapshot()
    }

    /// Espaces (graphes nommés) connus du store, triés.
    pub fn spaces(&self) -> Vec<String> {
        self.db.list_graphs()
    }

    /// Crée un espace (graphe nommé). Rend `true` s'il a été créé,
    /// `false` s'il existait déjà.
    ///
    /// Exige un store **adossé au substrat** : les graphes nommés sont des
    /// sous-substrats physiques (`<store>/graphs/<nom>`), et le nom est validé
    /// composant par composant par obrain (`/` autorisé, refus de `""`, `"."`,
    /// `".."`, `\`, `\0`).
    pub fn create_space(&self, name: &str) -> Result<bool> {
        self.db
            .create_graph(name)
            .map_err(|e| anyhow!("create_graph({name}) failed: {e}"))
    }

    /// Supprime un espace (graphe nommé). Rend `true` s'il a été supprimé.
    pub fn drop_space(&self, name: &str) -> bool {
        self.db.drop_graph(name)
    }

    /// Le store est-il adossé au substrat ?
    ///
    /// **Ce que ça conditionne** : toute la couche topologique et géométrique
    /// (simplexes, courbure de Ricci, énergie, HKS, clustering) n'existe que
    /// côté `SubstrateStore`.
    ///
    /// ⚠️ Surprise vérifiée — depuis le cutover T17 d'obrain, **même
    /// `new_in_memory()` est substrat-backed** (une configuration en mémoire
    /// ouvre un substrat temporaire auto-nettoyé,
    /// `obrain-engine/src/database/mod.rs:799-808`). Cette méthode rend donc
    /// `true` dans tous les cas de cette version : elle reste le garde-fou
    /// explicite contre un chemin non-substrat, pas un test de « en mémoire ou
    /// pas ». Ce qui n'est PAS garanti, en revanche, c'est que la couche
    /// simpliciale soit *configurée* — voir
    /// `SubstrateStore::passes_simpliciales_config()`.
    pub fn is_substrate(&self) -> bool {
        self.db.substrate_handle().is_some()
    }

    /// Le store a-t-il été ouvert en lecture seule ?
    pub fn is_read_only(&self) -> bool {
        self.db.is_read_only()
    }

    /// Rebuild the UUID→NodeId index for a specific entity label.
    ///
    /// **Why this exists**: `ObrainStore::open()` (and `from_db`) restore the
    /// underlying `ObrainDB` from disk, but the in-memory `uuid_to_node` index
    /// is left empty. As a result, every operation that resolves a `Uuid`
    /// (get / update / delete / list) returns "not found" on a freshly opened
    /// store, even when the nodes are physically present on disk.
    ///
    /// Call this method after `open()` (or `from_db()`) for each entity label
    /// you intend to serve. It scans nodes by label, reads their `_uuid`
    /// property, and repopulates the index.
    ///
    /// Returns the number of (uuid, node_id) entries that were inserted.
    ///
    /// # Example
    /// ```rust,ignore
    /// let store = ObrainStore::open("./data/evg.obrain")?;
    /// for label in ["event", "member", "date_proposal"] {
    ///     store.rebuild_index_for_label(label)?;
    /// }
    /// ```
    pub fn rebuild_index_for_label(&self, label: &str) -> Result<usize> {
        let store = self.active_store();
        let session = self.session();
        let node_ids = store.nodes_by_label(label);
        let mut index = self
            .uuid_to_node
            .write()
            .map_err(|e| anyhow!("UUID index lock poisoned: {}", e))?;
        let mut inserted = 0_usize;
        for nid in node_ids {
            if let Some(node) = session.get_node(nid)
                && let Some(uuid_val) = node.get_property("_uuid")
                && let Some(uuid_str) = uuid_val.as_str()
                && let Ok(uuid) = Uuid::parse_str(uuid_str)
            {
                index.insert(uuid, nid);
                inserted += 1;
            }
        }
        Ok(inserted)
    }

    /// Rebuild the link UUID→NodeId index for a given link type.
    ///
    /// Links are stored as nodes with label `_link:{link_type}` (see the
    /// `link_nodes` field on `ObrainStore`). After `open()`, that index is
    /// empty and must be repopulated for `LinkService` operations to find
    /// previously-persisted links.
    ///
    /// Returns the number of entries inserted.
    pub fn rebuild_link_index(&self, link_type: &str) -> Result<usize> {
        let full_label = format!("_link:{}", link_type);
        let store = self.active_store();
        let session = self.session();
        let node_ids = store.nodes_by_label(&full_label);
        let mut index = self
            .link_nodes
            .write()
            .map_err(|e| anyhow!("Link index lock poisoned: {}", e))?;
        let mut inserted = 0_usize;
        for nid in node_ids {
            if let Some(node) = session.get_node(nid)
                && let Some(uuid_val) = node.get_property("_uuid")
                && let Some(uuid_str) = uuid_val.as_str()
                && let Ok(uuid) = Uuid::parse_str(uuid_str)
            {
                index.insert(uuid, nid);
                inserted += 1;
            }
        }
        Ok(inserted)
    }
}

// ---------------------------------------------------------------------------
// ObrainDataService
// ---------------------------------------------------------------------------

/// Obrain-backed data service using the native property graph engine.
///
/// Entities are stored as graph nodes with:
/// - **Label**: `entity_type()` (e.g., `"user"`, `"product"`)
/// - **Property `_uuid`**: The entity's UUID as a string
/// - **Property `_data`**: The full entity serialized as JSON string
/// - **Indexed properties**: Each field from `indexed_fields()` stored individually
///   for efficient `search()` queries via `find_nodes_by_property`
///
/// # Type bounds
///
/// Requires `T: Data + Serialize + DeserializeOwned` because entities must be
/// serialized to/from JSON for storage in node properties. This is the same
/// pattern used by `DynamoDBDataService`.
///
/// # Example
///
/// ```rust,ignore
/// use this::storage::obrain::{ObrainStore, ObrainDataService};
///
/// let store = ObrainStore::new_in_memory();
/// let service = ObrainDataService::<MyEntity>::new(store);
/// let entity = service.create(my_entity).await?;
/// ```
pub struct ObrainDataService<T: Data> {
    store: Arc<ObrainStore>,
    _marker: PhantomData<T>,
}

impl<T: Data> ObrainDataService<T> {
    /// Create a new obrain data service backed by the given store
    pub fn new(store: Arc<ObrainStore>) -> Self {
        Self {
            store,
            _marker: PhantomData,
        }
    }
}

impl<T: Data> Clone for ObrainDataService<T> {
    fn clone(&self) -> Self {
        Self {
            store: Arc::clone(&self.store),
            _marker: PhantomData,
        }
    }
}

/// Convert a FieldValue to an obrain Value for property storage
fn field_value_to_obrain(fv: &FieldValue) -> ObrainValue {
    match fv {
        FieldValue::String(s) => ObrainValue::from(s.as_str()),
        FieldValue::Integer(i) => ObrainValue::from(*i),
        FieldValue::Float(f) => ObrainValue::from(*f),
        FieldValue::Boolean(b) => ObrainValue::from(*b),
        FieldValue::Uuid(u) => ObrainValue::from(u.to_string().as_str()),
        FieldValue::DateTime(dt) => ObrainValue::from(dt.to_rfc3339().as_str()),
        FieldValue::Null => ObrainValue::Null,
    }
}

#[async_trait]
impl<T: Data + Serialize + DeserializeOwned> DataService<T> for ObrainDataService<T> {
    async fn create(&self, entity: T) -> Result<T> {
        self.store.check_write()?;
        let session = self.store.session();
        let entity_type = entity.entity_type().to_string();
        let uuid = entity.id();
        let uuid_str = uuid.to_string();

        // Serialize entity to JSON for _data property
        let json_data =
            serde_json::to_string(&entity).map_err(|e| anyhow!("Serialize failed: {}", e))?;

        // Build properties: _uuid + _data + indexed fields
        let mut props: Vec<(&str, ObrainValue)> = Vec::new();
        // We need to own the strings for the property values
        let uuid_val = ObrainValue::from(uuid_str.as_str());
        let data_val = ObrainValue::from(json_data.as_str());
        props.push(("_uuid", uuid_val));
        props.push(("_data", data_val));

        // Store indexed fields as individual properties for search
        let indexed = T::indexed_fields();
        // Collect field values before building props to avoid borrow issues
        let field_vals: Vec<(String, ObrainValue)> = indexed
            .iter()
            .filter_map(|&field| {
                entity
                    .field_value(field)
                    .map(|fv| (format!("f_{}", field), field_value_to_obrain(&fv)))
            })
            .collect();

        // Create node with label and properties
        // We need to build the full props list with owned references
        let label = entity_type.as_str();
        let node_id = {
            // Build property pairs referencing our collected data
            let mut all_props: Vec<(&str, ObrainValue)> = Vec::with_capacity(2 + field_vals.len());
            all_props.push(("_uuid", ObrainValue::from(uuid_str.as_str())));
            all_props.push(("_data", ObrainValue::from(json_data.as_str())));
            for (key, val) in &field_vals {
                all_props.push((key.as_str(), val.clone()));
            }
            session.create_node_with_props(&[label], all_props)
        };

        // Update UUID index
        self.store
            .uuid_to_node
            .write()
            .map_err(|e| anyhow!("UUID index lock poisoned: {}", e))?
            .insert(uuid, node_id);

        // Commit WAL so the mutation is durable across restarts (cf. pattern
        // used by obrain-hub::conversation/cognitive_brain after each write).
        self.store
            .commit_wal()
            .map_err(|e| anyhow!("commit_wal after create failed: {}", e))?;

        Ok(entity)
    }

    async fn get(&self, id: &Uuid) -> Result<Option<T>> {
        let node_id = {
            let index = self
                .store
                .uuid_to_node
                .read()
                .map_err(|e| anyhow!("UUID index lock poisoned: {}", e))?;
            match index.get(id) {
                Some(&nid) => nid,
                None => return Ok(None),
            }
        };

        let session = self.store.session();
        let node = match session.get_node(node_id) {
            Some(n) => n,
            None => return Ok(None),
        };

        // Deserialize from _data property
        let data_val = node
            .get_property("_data")
            .ok_or_else(|| anyhow!("Node {} missing _data property", id))?;

        let json_str = data_val
            .as_str()
            .ok_or_else(|| anyhow!("_data property is not a string for node {}", id))?;

        let entity: T =
            serde_json::from_str(json_str).map_err(|e| anyhow!("Deserialize failed: {}", e))?;

        Ok(Some(entity))
    }

    async fn list(&self) -> Result<Vec<T>> {
        let session = self.store.session();

        // Query all nodes by label (entity_type)
        // We need a dummy instance to get entity_type, but Data doesn't provide that statically.
        // Instead, iterate all nodes in our UUID index that match.
        let index = self
            .store
            .uuid_to_node
            .read()
            .map_err(|e| anyhow!("UUID index lock poisoned: {}", e))?;

        let mut entities = Vec::with_capacity(index.len());
        for (&_uuid, &node_id) in index.iter() {
            if let Some(node) = session.get_node(node_id) {
                // Check label matches T's entity_type — we can't get entity_type statically
                // from T, so we deserialize all nodes in our index. Since each
                // ObrainDataService<T> only inserts nodes of type T, this is safe.
                if let Some(data_val) = node.get_property("_data")
                    && let Some(json_str) = data_val.as_str()
                    && let Ok(entity) = serde_json::from_str::<T>(json_str)
                {
                    entities.push(entity);
                }
            }
        }

        Ok(entities)
    }

    async fn update(&self, id: &Uuid, entity: T) -> Result<T> {
        self.store.check_write()?;
        let node_id = {
            let index = self
                .store
                .uuid_to_node
                .read()
                .map_err(|e| anyhow!("UUID index lock poisoned: {}", e))?;
            *index
                .get(id)
                .ok_or_else(|| anyhow!("Node not found: {}", id))?
        };

        let session = self.store.session();

        // Verify node still exists
        if session.get_node(node_id).is_none() {
            return Err(anyhow!("Node not found in graph: {}", id));
        }

        // Update _data property via the underlying store (GraphStoreMut)
        let json_data =
            serde_json::to_string(&entity).map_err(|e| anyhow!("Serialize failed: {}", e))?;

        let store = self.store.db.store();
        store.set_node_property(node_id, "_data", ObrainValue::from(json_data.as_str()));

        // Update indexed field properties
        let indexed = T::indexed_fields();
        for &field in indexed {
            let prop_key = format!("f_{}", field);
            if let Some(fv) = entity.field_value(field) {
                store.set_node_property(node_id, &prop_key, field_value_to_obrain(&fv));
            }
        }

        // Commit WAL so the mutation is durable across restarts.
        self.store
            .commit_wal()
            .map_err(|e| anyhow!("commit_wal after update failed: {}", e))?;

        Ok(entity)
    }

    async fn delete(&self, id: &Uuid) -> Result<()> {
        self.store.check_write()?;
        let node_id = {
            let mut index = self
                .store
                .uuid_to_node
                .write()
                .map_err(|e| anyhow!("UUID index lock poisoned: {}", e))?;
            match index.remove(id) {
                Some(nid) => nid,
                None => return Ok(()), // Already deleted or never existed
            }
        };

        let store = self.store.db.store();
        // DETACH DELETE — removes edges connected to this node too
        store.delete_node_edges(node_id);
        store.delete_node(node_id);

        // Commit WAL so the deletion is durable.
        self.store
            .commit_wal()
            .map_err(|e| anyhow!("commit_wal after delete failed: {}", e))?;

        Ok(())
    }

    async fn search(&self, field: &str, value: &str) -> Result<Vec<T>> {
        let store = self.store.db.store();
        let session = self.store.session();
        let prop_key = format!("f_{}", field);

        // Use obrain's native property search via GraphStore
        let search_value = ObrainValue::from(value);
        let matching_nodes = store.find_nodes_by_property(&prop_key, &search_value);

        let mut results = Vec::new();
        for node_id in matching_nodes {
            if let Some(node) = session.get_node(node_id)
                && let Some(data_val) = node.get_property("_data")
                && let Some(json_str) = data_val.as_str()
                && let Ok(entity) = serde_json::from_str::<T>(json_str)
            {
                results.push(entity);
            }
        }

        // Also try matching FieldValue variants that convert to string differently
        // (e.g., UUID.to_string(), DateTime.to_rfc3339())
        if results.is_empty() {
            // Fall back to full scan with FieldValue comparison
            let index = self
                .store
                .uuid_to_node
                .read()
                .map_err(|e| anyhow!("UUID index lock poisoned: {}", e))?;

            for &node_id in index.values() {
                if let Some(node) = session.get_node(node_id)
                    && let Some(data_val) = node.get_property("_data")
                    && let Some(json_str) = data_val.as_str()
                    && let Ok(entity) = serde_json::from_str::<T>(json_str)
                    && entity.field_value(field).is_some_and(|fv| match &fv {
                        FieldValue::String(s) => s == value,
                        FieldValue::Integer(i) => i.to_string() == value,
                        FieldValue::Float(f) => f.to_string() == value,
                        FieldValue::Boolean(b) => b.to_string() == value,
                        FieldValue::Uuid(u) => u.to_string() == value,
                        FieldValue::DateTime(dt) => dt.to_rfc3339() == value,
                        FieldValue::Null => false,
                    })
                {
                    results.push(entity);
                }
            }
        }

        Ok(results)
    }
}

// ---------------------------------------------------------------------------
// ObrainLinkService
// ---------------------------------------------------------------------------

/// Obrain-backed link service using graph nodes with link labels.
///
/// Links are stored as graph nodes (not edges) with label `_link:{link_type}`
/// because `LinkService` is entity-agnostic — the source and target entities
/// may be managed by different `ObrainDataService` instances or may not exist
/// yet. Storing links as nodes avoids the constraint that edge endpoints must
/// exist.
///
/// Each link node has properties:
/// - `_uuid`: Link UUID as string
/// - `_data`: Full `LinkEntity` serialized as JSON
/// - `_source_id`: Source entity UUID as string
/// - `_target_id`: Target entity UUID as string
/// - `_link_type`: Link type string
///
/// When both endpoints exist as entity nodes, graph edges are also created
/// to enable native traversal queries (opportunistic edge creation).
///
/// # Example
///
/// ```rust,ignore
/// use this::storage::obrain::{ObrainStore, ObrainLinkService};
///
/// let store = ObrainStore::new_in_memory();
/// let service = ObrainLinkService::new(store);
/// let link = service.create(link_entity).await?;
/// ```
#[derive(Clone)]
pub struct ObrainLinkService {
    store: Arc<ObrainStore>,
}

impl ObrainLinkService {
    /// Create a new obrain link service backed by the given store
    pub fn new(store: Arc<ObrainStore>) -> Self {
        Self { store }
    }

    /// Deserialize a LinkEntity from a node's _data property
    fn link_from_node(&self, node: &Node) -> Option<LinkEntity> {
        let data_val = node.get_property("_data")?;
        let json_str = data_val.as_str()?;
        serde_json::from_str(json_str).ok()
    }

    /// Try to create a graph edge between source and target entity nodes
    /// for native traversal. This is opportunistic — if endpoints don't exist
    /// as entity nodes, no edge is created.
    fn try_create_edge(&self, link: &LinkEntity) {
        let index = match self.store.uuid_to_node.read() {
            Ok(idx) => idx,
            Err(_) => return,
        };

        let source_node = index.get(&link.source_id);
        let target_node = index.get(&link.target_id);

        if let (Some(&src), Some(&tgt)) = (source_node, target_node) {
            let session = self.store.session();
            session.create_edge(src, tgt, &link.link_type);
        }
    }
}

#[async_trait]
impl LinkService for ObrainLinkService {
    async fn create(&self, link: LinkEntity) -> Result<LinkEntity> {
        self.store.check_write()?;
        let session = self.store.session();
        let uuid_str = link.id.to_string();
        let source_str = link.source_id.to_string();
        let target_str = link.target_id.to_string();

        let json_data =
            serde_json::to_string(&link).map_err(|e| anyhow!("Serialize link failed: {}", e))?;

        let label = format!("_link:{}", link.link_type);

        let node_id = session.create_node_with_props(
            &[label.as_str()],
            vec![
                ("_uuid", ObrainValue::from(uuid_str.as_str())),
                ("_data", ObrainValue::from(json_data.as_str())),
                ("_source_id", ObrainValue::from(source_str.as_str())),
                ("_target_id", ObrainValue::from(target_str.as_str())),
                ("_link_type", ObrainValue::from(link.link_type.as_str())),
            ],
        );

        // Update link index
        self.store
            .link_nodes
            .write()
            .map_err(|e| anyhow!("Link index lock poisoned: {}", e))?
            .insert(link.id, node_id);

        // Opportunistically create graph edge for traversal
        self.try_create_edge(&link);

        // Commit WAL so the link is durable.
        self.store
            .commit_wal()
            .map_err(|e| anyhow!("commit_wal after link create failed: {}", e))?;

        Ok(link)
    }

    async fn get(&self, id: &Uuid) -> Result<Option<LinkEntity>> {
        let node_id = {
            let index = self
                .store
                .link_nodes
                .read()
                .map_err(|e| anyhow!("Link index lock poisoned: {}", e))?;
            match index.get(id) {
                Some(&nid) => nid,
                None => return Ok(None),
            }
        };

        let session = self.store.session();
        let node = match session.get_node(node_id) {
            Some(n) => n,
            None => return Ok(None),
        };

        Ok(self.link_from_node(&node))
    }

    async fn list(&self) -> Result<Vec<LinkEntity>> {
        let session = self.store.session();
        let index = self
            .store
            .link_nodes
            .read()
            .map_err(|e| anyhow!("Link index lock poisoned: {}", e))?;

        let mut links = Vec::with_capacity(index.len());
        for &node_id in index.values() {
            if let Some(node) = session.get_node(node_id)
                && let Some(link) = self.link_from_node(&node)
            {
                links.push(link);
            }
        }

        Ok(links)
    }

    async fn find_by_source(
        &self,
        source_id: &Uuid,
        link_type: Option<&str>,
        _target_type: Option<&str>,
    ) -> Result<Vec<LinkEntity>> {
        let store = self.store.db.store();
        let session = self.store.session();
        let source_str = source_id.to_string();
        let search_val = ObrainValue::from(source_str.as_str());

        // Use property search for _source_id via GraphStore
        let matching = store.find_nodes_by_property("_source_id", &search_val);

        let mut results = Vec::new();
        for node_id in matching {
            if let Some(node) = session.get_node(node_id)
                && let Some(link) = self.link_from_node(&node)
                && link_type.is_none_or(|lt| link.link_type == lt)
            {
                results.push(link);
            }
        }

        Ok(results)
    }

    async fn find_by_target(
        &self,
        target_id: &Uuid,
        link_type: Option<&str>,
        _source_type: Option<&str>,
    ) -> Result<Vec<LinkEntity>> {
        let store = self.store.db.store();
        let session = self.store.session();
        let target_str = target_id.to_string();
        let search_val = ObrainValue::from(target_str.as_str());

        // Use property search for _target_id via GraphStore
        let matching = store.find_nodes_by_property("_target_id", &search_val);

        let mut results = Vec::new();
        for node_id in matching {
            if let Some(node) = session.get_node(node_id)
                && let Some(link) = self.link_from_node(&node)
                && link_type.is_none_or(|lt| link.link_type == lt)
            {
                results.push(link);
            }
        }

        Ok(results)
    }

    async fn update(&self, id: &Uuid, link: LinkEntity) -> Result<LinkEntity> {
        self.store.check_write()?;
        let node_id = {
            let index = self
                .store
                .link_nodes
                .read()
                .map_err(|e| anyhow!("Link index lock poisoned: {}", e))?;
            *index
                .get(id)
                .ok_or_else(|| anyhow!("Edge not found: {}", id))?
        };

        let session = self.store.session();
        let store = self.store.db.store();

        if session.get_node(node_id).is_none() {
            return Err(anyhow!("Link node not found in graph: {}", id));
        }

        // Update properties via GraphStoreMut
        let json_data =
            serde_json::to_string(&link).map_err(|e| anyhow!("Serialize link failed: {}", e))?;
        let source_str = link.source_id.to_string();
        let target_str = link.target_id.to_string();

        store.set_node_property(node_id, "_data", ObrainValue::from(json_data.as_str()));
        store.set_node_property(
            node_id,
            "_source_id",
            ObrainValue::from(source_str.as_str()),
        );
        store.set_node_property(
            node_id,
            "_target_id",
            ObrainValue::from(target_str.as_str()),
        );
        store.set_node_property(
            node_id,
            "_link_type",
            ObrainValue::from(link.link_type.as_str()),
        );

        // Commit WAL so the link update is durable.
        self.store
            .commit_wal()
            .map_err(|e| anyhow!("commit_wal after link update failed: {}", e))?;

        Ok(link)
    }

    async fn delete(&self, id: &Uuid) -> Result<()> {
        self.store.check_write()?;
        let node_id = {
            let mut index = self
                .store
                .link_nodes
                .write()
                .map_err(|e| anyhow!("Link index lock poisoned: {}", e))?;
            match index.remove(id) {
                Some(nid) => nid,
                None => return Ok(()),
            }
        };

        let store = self.store.db.store();
        store.delete_node(node_id);

        // Commit WAL so the deletion is durable.
        self.store
            .commit_wal()
            .map_err(|e| anyhow!("commit_wal after link delete failed: {}", e))?;

        Ok(())
    }

    async fn delete_by_entity(&self, entity_id: &Uuid) -> Result<()> {
        self.store.check_write()?;
        let entity_str = entity_id.to_string();
        let session = self.store.session();

        // Find all links where entity is source or target
        let mut to_remove = Vec::new();
        {
            let index = self
                .store
                .link_nodes
                .read()
                .map_err(|e| anyhow!("Link index lock poisoned: {}", e))?;

            for (&link_uuid, &node_id) in index.iter() {
                if let Some(node) = session.get_node(node_id) {
                    let is_source = node
                        .get_property("_source_id")
                        .and_then(|v| v.as_str().map(|s| s == entity_str))
                        .unwrap_or(false);
                    let is_target = node
                        .get_property("_target_id")
                        .and_then(|v| v.as_str().map(|s| s == entity_str))
                        .unwrap_or(false);

                    if is_source || is_target {
                        to_remove.push((link_uuid, node_id));
                    }
                }
            }
        }

        // Remove matching links
        if !to_remove.is_empty() {
            let store = self.store.db.store();
            let mut index = self
                .store
                .link_nodes
                .write()
                .map_err(|e| anyhow!("Link index lock poisoned: {}", e))?;

            for (link_uuid, node_id) in to_remove {
                index.remove(&link_uuid);
                store.delete_node(node_id);
            }

            // Commit WAL once for the whole batch so none of the deletions can
            // be lost on crash (durability contract: every mutation path ends
            // in a commit_wal — see the create/update/delete paths above).
            self.store
                .commit_wal()
                .map_err(|e| anyhow!("commit_wal after delete_by_entity failed: {}", e))?;
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::entity::Entity;
    use crate::core::field::FieldValue;
    use chrono::{DateTime, Utc};

    // -----------------------------------------------------------------------
    // Test entity for ObrainDataService tests
    // -----------------------------------------------------------------------

    #[derive(Clone, Debug, PartialEq, Serialize, serde::Deserialize)]
    struct TestDataEntity {
        id: Uuid,
        entity_name: String,
        status: String,
        created_at: DateTime<Utc>,
        updated_at: DateTime<Utc>,
    }

    impl TestDataEntity {
        fn new(name: &str) -> Self {
            let now = Utc::now();
            Self {
                id: Uuid::new_v4(),
                entity_name: name.to_string(),
                status: "active".to_string(),
                created_at: now,
                updated_at: now,
            }
        }
    }

    impl Entity for TestDataEntity {
        type Service = ();

        fn resource_name() -> &'static str {
            "test_data_entities"
        }

        fn resource_name_singular() -> &'static str {
            "test_data_entity"
        }

        fn service_from_host(
            _: &std::sync::Arc<dyn std::any::Any + Send + Sync>,
        ) -> anyhow::Result<std::sync::Arc<Self::Service>> {
            Ok(std::sync::Arc::new(()))
        }

        fn id(&self) -> Uuid {
            self.id
        }

        fn entity_type(&self) -> &str {
            "test_data"
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

    impl crate::core::Data for TestDataEntity {
        fn name(&self) -> &str {
            &self.entity_name
        }

        fn indexed_fields() -> &'static [&'static str] {
            &["entity_name", "status"]
        }

        fn field_value(&self, field: &str) -> Option<FieldValue> {
            match field {
                "entity_name" => Some(FieldValue::String(self.entity_name.clone())),
                "status" => Some(FieldValue::String(self.status.clone())),
                _ => None,
            }
        }
    }

    /// Helper to create a fresh in-memory store for each test
    fn test_store() -> Arc<ObrainStore> {
        ObrainStore::new_in_memory()
    }

    // -----------------------------------------------------------------------
    // ObrainDataService CRUD tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_data_create_entity() {
        let store = test_store();
        let service = ObrainDataService::<TestDataEntity>::new(store);
        let entity = TestDataEntity::new("Alice");

        let created = service.create(entity.clone()).await.unwrap();
        assert_eq!(created.id, entity.id);
        assert_eq!(created.entity_name, "Alice");
    }

    #[tokio::test]
    async fn test_data_get_entity() {
        let store = test_store();
        let service = ObrainDataService::<TestDataEntity>::new(store);
        let entity = TestDataEntity::new("Bob");

        service.create(entity.clone()).await.unwrap();

        let retrieved = service.get(&entity.id).await.unwrap();
        assert!(retrieved.is_some());
        assert_eq!(retrieved.unwrap().entity_name, "Bob");
    }

    #[tokio::test]
    async fn test_data_get_nonexistent() {
        let store = test_store();
        let service = ObrainDataService::<TestDataEntity>::new(store);

        let retrieved = service.get(&Uuid::new_v4()).await.unwrap();
        assert!(retrieved.is_none());
    }

    #[tokio::test]
    async fn test_data_list_entities() {
        let store = test_store();
        let service = ObrainDataService::<TestDataEntity>::new(store);

        service.create(TestDataEntity::new("Alice")).await.unwrap();
        service.create(TestDataEntity::new("Bob")).await.unwrap();
        service
            .create(TestDataEntity::new("Charlie"))
            .await
            .unwrap();

        let all = service.list().await.unwrap();
        assert_eq!(all.len(), 3);
    }

    #[tokio::test]
    async fn test_data_list_empty() {
        let store = test_store();
        let service = ObrainDataService::<TestDataEntity>::new(store);

        let all = service.list().await.unwrap();
        assert!(all.is_empty());
    }

    #[tokio::test]
    async fn test_data_update_entity() {
        let store = test_store();
        let service = ObrainDataService::<TestDataEntity>::new(store);
        let mut entity = TestDataEntity::new("Alice");

        service.create(entity.clone()).await.unwrap();

        entity.entity_name = "Alice Updated".to_string();
        let updated = service.update(&entity.id, entity.clone()).await.unwrap();

        assert_eq!(updated.entity_name, "Alice Updated");

        // Verify persisted
        let retrieved = service.get(&entity.id).await.unwrap().unwrap();
        assert_eq!(retrieved.entity_name, "Alice Updated");
    }

    #[tokio::test]
    async fn test_data_update_nonexistent() {
        let store = test_store();
        let service = ObrainDataService::<TestDataEntity>::new(store);
        let entity = TestDataEntity::new("Ghost");
        let id = entity.id;

        let result = service.update(&id, entity).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("not found"));
    }

    #[tokio::test]
    async fn test_data_delete_entity() {
        let store = test_store();
        let service = ObrainDataService::<TestDataEntity>::new(store);
        let entity = TestDataEntity::new("Alice");

        service.create(entity.clone()).await.unwrap();
        assert!(service.get(&entity.id).await.unwrap().is_some());

        service.delete(&entity.id).await.unwrap();
        assert!(service.get(&entity.id).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn test_data_delete_nonexistent() {
        let store = test_store();
        let service = ObrainDataService::<TestDataEntity>::new(store);

        // Deleting a nonexistent entity should succeed silently
        let result = service.delete(&Uuid::new_v4()).await;
        assert!(result.is_ok());
    }

    // -----------------------------------------------------------------------
    // ObrainDataService search tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_data_search_by_indexed_field() {
        let store = test_store();
        let service = ObrainDataService::<TestDataEntity>::new(store);

        service.create(TestDataEntity::new("Alice")).await.unwrap();
        service.create(TestDataEntity::new("Bob")).await.unwrap();
        service.create(TestDataEntity::new("Alice")).await.unwrap();

        let results = service.search("entity_name", "Alice").await.unwrap();
        assert_eq!(results.len(), 2);
        assert!(results.iter().all(|e| e.entity_name == "Alice"));
    }

    #[tokio::test]
    async fn test_data_search_no_results() {
        let store = test_store();
        let service = ObrainDataService::<TestDataEntity>::new(store);

        service.create(TestDataEntity::new("Alice")).await.unwrap();

        let results = service.search("entity_name", "Zara").await.unwrap();
        assert!(results.is_empty());
    }

    #[tokio::test]
    async fn test_data_search_by_status() {
        let store = test_store();
        let service = ObrainDataService::<TestDataEntity>::new(store);

        let mut inactive = TestDataEntity::new("Inactive");
        inactive.status = "inactive".to_string();

        service.create(TestDataEntity::new("Active")).await.unwrap();
        service.create(inactive).await.unwrap();

        let results = service.search("status", "inactive").await.unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].entity_name, "Inactive");
    }

    #[tokio::test]
    async fn test_data_search_unknown_field() {
        let store = test_store();
        let service = ObrainDataService::<TestDataEntity>::new(store);

        service.create(TestDataEntity::new("Alice")).await.unwrap();

        // Unknown field returns no results (not indexed, not in FieldValue)
        let results = service
            .search("nonexistent_field", "anything")
            .await
            .unwrap();
        assert!(results.is_empty());
    }

    #[tokio::test]
    async fn test_data_clone_shares_state() {
        let store = test_store();
        let service = ObrainDataService::<TestDataEntity>::new(store);
        let cloned = service.clone();

        service.create(TestDataEntity::new("Alice")).await.unwrap();

        // Clone shares the same backing store
        let all = cloned.list().await.unwrap();
        assert_eq!(all.len(), 1);
    }

    // -----------------------------------------------------------------------
    // Extended test entity for FieldValue variant coverage in search
    // -----------------------------------------------------------------------

    #[derive(Clone, Debug, PartialEq, Serialize, serde::Deserialize)]
    struct ExtendedTestEntity {
        id: Uuid,
        entity_name: String,
        status: String,
        ref_id: Uuid,
        created_at: DateTime<Utc>,
        updated_at: DateTime<Utc>,
    }

    impl ExtendedTestEntity {
        fn new(name: &str, ref_id: Uuid) -> Self {
            let now = Utc::now();
            Self {
                id: Uuid::new_v4(),
                entity_name: name.to_string(),
                status: "active".to_string(),
                ref_id,
                created_at: now,
                updated_at: now,
            }
        }
    }

    impl Entity for ExtendedTestEntity {
        type Service = ();

        fn resource_name() -> &'static str {
            "extended_test_entities"
        }

        fn resource_name_singular() -> &'static str {
            "extended_test_entity"
        }

        fn service_from_host(
            _: &std::sync::Arc<dyn std::any::Any + Send + Sync>,
        ) -> anyhow::Result<std::sync::Arc<Self::Service>> {
            Ok(std::sync::Arc::new(()))
        }

        fn id(&self) -> Uuid {
            self.id
        }

        fn entity_type(&self) -> &str {
            "extended_test"
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

    impl crate::core::Data for ExtendedTestEntity {
        fn name(&self) -> &str {
            &self.entity_name
        }

        fn indexed_fields() -> &'static [&'static str] {
            &["entity_name", "status", "ref_id", "created_at"]
        }

        fn field_value(&self, field: &str) -> Option<FieldValue> {
            match field {
                "entity_name" => Some(FieldValue::String(self.entity_name.clone())),
                "status" => Some(FieldValue::String(self.status.clone())),
                "ref_id" => Some(FieldValue::Uuid(self.ref_id)),
                "created_at" => Some(FieldValue::DateTime(self.created_at)),
                _ => None,
            }
        }
    }

    #[tokio::test]
    async fn test_data_search_by_uuid_field() {
        let store = test_store();
        let service = ObrainDataService::<ExtendedTestEntity>::new(store);
        let target_ref = Uuid::new_v4();
        let other_ref = Uuid::new_v4();

        service
            .create(ExtendedTestEntity::new("Alpha", target_ref))
            .await
            .expect("create Alpha should succeed");
        service
            .create(ExtendedTestEntity::new("Beta", other_ref))
            .await
            .expect("create Beta should succeed");
        service
            .create(ExtendedTestEntity::new("Gamma", target_ref))
            .await
            .expect("create Gamma should succeed");

        let results = service
            .search("ref_id", &target_ref.to_string())
            .await
            .expect("search by ref_id should succeed");
        assert_eq!(
            results.len(),
            2,
            "should find 2 entities with matching ref_id"
        );
        assert!(results.iter().all(|e| e.ref_id == target_ref));
    }

    #[tokio::test]
    async fn test_data_search_by_datetime_field() {
        let store = test_store();
        let service = ObrainDataService::<ExtendedTestEntity>::new(store);
        let entity = ExtendedTestEntity::new("Timed", Uuid::new_v4());
        let created_rfc3339 = entity.created_at.to_rfc3339();

        service.create(entity).await.expect("create should succeed");

        let results = service
            .search("created_at", &created_rfc3339)
            .await
            .expect("search by created_at should succeed");
        assert_eq!(
            results.len(),
            1,
            "should find entity by its created_at timestamp"
        );
        assert_eq!(results[0].entity_name, "Timed");
    }

    // -----------------------------------------------------------------------
    // ObrainLinkService tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_create_link() {
        let store = test_store();
        let service = ObrainLinkService::new(store);
        let user_id = Uuid::new_v4();
        let car_id = Uuid::new_v4();

        let link = LinkEntity::new("owner", user_id, car_id, None);

        let created = service.create(link.clone()).await.unwrap();

        assert_eq!(created.link_type, "owner");
        assert_eq!(created.source_id, user_id);
        assert_eq!(created.target_id, car_id);
    }

    #[tokio::test]
    async fn test_get_link() {
        let store = test_store();
        let service = ObrainLinkService::new(store);
        let link = LinkEntity::new("owner", Uuid::new_v4(), Uuid::new_v4(), None);

        service.create(link.clone()).await.unwrap();

        let retrieved = service.get(&link.id).await.unwrap();
        assert!(retrieved.is_some());
        assert_eq!(retrieved.unwrap().id, link.id);
    }

    #[tokio::test]
    async fn test_get_link_nonexistent() {
        let store = test_store();
        let service = ObrainLinkService::new(store);

        let retrieved = service.get(&Uuid::new_v4()).await.unwrap();
        assert!(retrieved.is_none());
    }

    #[tokio::test]
    async fn test_list_links() {
        let store = test_store();
        let service = ObrainLinkService::new(store);

        let link1 = LinkEntity::new("owner", Uuid::new_v4(), Uuid::new_v4(), None);
        let link2 = LinkEntity::new("driver", Uuid::new_v4(), Uuid::new_v4(), None);

        service.create(link1).await.unwrap();
        service.create(link2).await.unwrap();

        let links = service.list().await.unwrap();
        assert_eq!(links.len(), 2);
    }

    #[tokio::test]
    async fn test_find_by_source() {
        let store = test_store();
        let service = ObrainLinkService::new(store);
        let user_id = Uuid::new_v4();
        let car1_id = Uuid::new_v4();
        let car2_id = Uuid::new_v4();

        // User owns car1
        service
            .create(LinkEntity::new("owner", user_id, car1_id, None))
            .await
            .unwrap();

        // User drives car2
        service
            .create(LinkEntity::new("driver", user_id, car2_id, None))
            .await
            .unwrap();

        // Find all links from user
        let links = service.find_by_source(&user_id, None, None).await.unwrap();
        assert_eq!(links.len(), 2);

        // Find only owner links
        let owner_links = service
            .find_by_source(&user_id, Some("owner"), None)
            .await
            .unwrap();
        assert_eq!(owner_links.len(), 1);
        assert_eq!(owner_links[0].link_type, "owner");
    }

    #[tokio::test]
    async fn test_find_by_target() {
        let store = test_store();
        let service = ObrainLinkService::new(store);
        let user1_id = Uuid::new_v4();
        let user2_id = Uuid::new_v4();
        let car_id = Uuid::new_v4();

        // User1 owns car
        service
            .create(LinkEntity::new("owner", user1_id, car_id, None))
            .await
            .unwrap();

        // User2 drives car
        service
            .create(LinkEntity::new("driver", user2_id, car_id, None))
            .await
            .unwrap();

        // Find all links to car
        let links = service.find_by_target(&car_id, None, None).await.unwrap();
        assert_eq!(links.len(), 2);

        // Find only driver links
        let driver_links = service
            .find_by_target(&car_id, Some("driver"), None)
            .await
            .unwrap();
        assert_eq!(driver_links.len(), 1);
        assert_eq!(driver_links[0].link_type, "driver");
    }

    #[tokio::test]
    async fn test_update_link() {
        let store = test_store();
        let service = ObrainLinkService::new(store);
        let user_id = Uuid::new_v4();
        let company_id = Uuid::new_v4();

        let mut link = LinkEntity::new(
            "worker",
            user_id,
            company_id,
            Some(serde_json::json!({"role": "Developer"})),
        );

        service.create(link.clone()).await.unwrap();

        // Update metadata
        link.metadata = Some(serde_json::json!({"role": "Senior Developer"}));
        link.touch();

        let updated = service.update(&link.id, link.clone()).await.unwrap();
        assert_eq!(
            updated.metadata,
            Some(serde_json::json!({"role": "Senior Developer"}))
        );
    }

    #[tokio::test]
    async fn test_update_link_nonexistent() {
        let store = test_store();
        let service = ObrainLinkService::new(store);
        let link = LinkEntity::new("owner", Uuid::new_v4(), Uuid::new_v4(), None);
        let id = link.id;

        let result = service.update(&id, link).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("not found"));
    }

    #[tokio::test]
    async fn test_delete_link() {
        let store = test_store();
        let service = ObrainLinkService::new(store);
        let link = LinkEntity::new("owner", Uuid::new_v4(), Uuid::new_v4(), None);

        service.create(link.clone()).await.unwrap();

        let retrieved = service.get(&link.id).await.unwrap();
        assert!(retrieved.is_some());

        service.delete(&link.id).await.unwrap();

        let retrieved = service.get(&link.id).await.unwrap();
        assert!(retrieved.is_none());
    }

    #[tokio::test]
    async fn test_delete_link_nonexistent() {
        let store = test_store();
        let service = ObrainLinkService::new(store);

        // Deleting a nonexistent edge should succeed silently
        let result = service.delete(&Uuid::new_v4()).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_delete_by_entity() {
        let store = test_store();
        let service = ObrainLinkService::new(store);
        let user_id = Uuid::new_v4();
        let car1_id = Uuid::new_v4();
        let car2_id = Uuid::new_v4();

        service
            .create(LinkEntity::new("owner", user_id, car1_id, None))
            .await
            .unwrap();
        service
            .create(LinkEntity::new("driver", user_id, car2_id, None))
            .await
            .unwrap();
        service
            .create(LinkEntity::new("owner", Uuid::new_v4(), car1_id, None))
            .await
            .unwrap();

        let links = service.list().await.unwrap();
        assert_eq!(links.len(), 3);

        // Delete all links involving user_id
        service.delete_by_entity(&user_id).await.unwrap();

        let remaining = service.list().await.unwrap();
        assert_eq!(remaining.len(), 1);
        assert_ne!(remaining[0].source_id, user_id);
        assert_ne!(remaining[0].target_id, user_id);
    }

    // -----------------------------------------------------------------------
    // Shared store tests (DataService + LinkService work together)
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_shared_store_data_and_links() {
        let store = test_store();
        let data_svc = ObrainDataService::<TestDataEntity>::new(Arc::clone(&store));
        let link_svc = ObrainLinkService::new(Arc::clone(&store));

        let alice = TestDataEntity::new("Alice");
        let bob = TestDataEntity::new("Bob");

        data_svc.create(alice.clone()).await.unwrap();
        data_svc.create(bob.clone()).await.unwrap();

        let link = LinkEntity::new("friend", alice.id, bob.id, None);
        link_svc.create(link.clone()).await.unwrap();

        // Both services share the same DB
        let entities = data_svc.list().await.unwrap();
        assert_eq!(entities.len(), 2);

        let links = link_svc
            .find_by_source(&alice.id, None, None)
            .await
            .unwrap();
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].target_id, bob.id);
    }

    #[tokio::test]
    async fn test_link_with_metadata() {
        let store = test_store();
        let service = ObrainLinkService::new(store);

        let link = LinkEntity::new(
            "worker",
            Uuid::new_v4(),
            Uuid::new_v4(),
            Some(serde_json::json!({"role": "Developer", "since": "2024-01-01"})),
        );

        service.create(link.clone()).await.unwrap();

        let retrieved = service.get(&link.id).await.unwrap().unwrap();
        assert_eq!(
            retrieved.metadata,
            Some(serde_json::json!({"role": "Developer", "since": "2024-01-01"}))
        );
    }

    // -----------------------------------------------------------------------
    // Index rebuild tests (simulate a fresh open() where nodes exist on disk
    // but the in-memory UUID→NodeId indexes are empty)
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_rebuild_index_for_label_after_index_loss() {
        let store = test_store();
        let service = ObrainDataService::<TestDataEntity>::new(store.clone());
        let entity = TestDataEntity::new("Persisted");
        service.create(entity.clone()).await.unwrap();

        // Simulate a fresh open(): nodes are present, index is empty.
        store.uuid_to_node.write().unwrap().clear();
        assert!(
            service.get(&entity.id).await.unwrap().is_none(),
            "empty index → entity unreachable (the bug this fixes)"
        );

        // Rebuild from the label scan.
        let inserted = store.rebuild_index_for_label("test_data").unwrap();
        assert_eq!(inserted, 1);

        let retrieved = service.get(&entity.id).await.unwrap();
        assert_eq!(retrieved.unwrap().entity_name, "Persisted");
    }

    #[tokio::test]
    async fn test_rebuild_index_for_label_unknown_label() {
        let store = test_store();
        let service = ObrainDataService::<TestDataEntity>::new(store.clone());
        service.create(TestDataEntity::new("Alice")).await.unwrap();

        // Rebuilding a label with no nodes is a no-op, not an error.
        let inserted = store.rebuild_index_for_label("unknown_label").unwrap();
        assert_eq!(inserted, 0);
    }

    #[tokio::test]
    async fn test_rebuild_link_index_after_index_loss() {
        let store = test_store();
        let service = ObrainLinkService::new(store.clone());
        let link = LinkEntity::new("owner", Uuid::new_v4(), Uuid::new_v4(), None);
        service.create(link.clone()).await.unwrap();

        // Simulate a fresh open(): link nodes are present, index is empty.
        store.link_nodes.write().unwrap().clear();
        assert!(
            service.get(&link.id).await.unwrap().is_none(),
            "empty link index → link unreachable (the bug this fixes)"
        );

        let inserted = store.rebuild_link_index("owner").unwrap();
        assert_eq!(inserted, 1);

        let retrieved = service.get(&link.id).await.unwrap().unwrap();
        assert_eq!(retrieved.link_type, "owner");
    }

    #[tokio::test]
    async fn test_persistence_across_reopen_with_rebuilt_index() {
        let dir = std::env::temp_dir().join(format!("this_obrain_test_{}", Uuid::new_v4()));
        let entity = TestDataEntity::new("Durable");

        // Write, then drop the store (process-restart simulation).
        {
            let store = ObrainStore::open(&dir).unwrap();
            let service = ObrainDataService::<TestDataEntity>::new(store);
            service.create(entity.clone()).await.unwrap();
            // No explicit flush: durability must come from the per-mutation
            // commit_wal() calls.
        }

        // Reopen: node must be on disk, index must be rebuildable.
        let store = ObrainStore::open(&dir).unwrap();
        let inserted = store.rebuild_index_for_label("test_data").unwrap();
        assert_eq!(inserted, 1, "entity must survive reopen via WAL commit");

        let service = ObrainDataService::<TestDataEntity>::new(store);
        let retrieved = service.get(&entity.id).await.unwrap();
        assert_eq!(retrieved.unwrap().entity_name, "Durable");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn test_links_survive_reopen_with_rebuilt_index() {
        let dir = std::env::temp_dir().join(format!("this_obrain_links_{}", Uuid::new_v4()));

        // Write data + links, drop the store (restart simulation).
        let (alice, bob, link) = {
            let store = ObrainStore::open(&dir).unwrap();
            let data_svc = ObrainDataService::<TestDataEntity>::new(Arc::clone(&store));
            let link_svc = ObrainLinkService::new(Arc::clone(&store));
            let alice = TestDataEntity::new("Alice");
            let bob = TestDataEntity::new("Bob");
            data_svc.create(alice.clone()).await.unwrap();
            data_svc.create(bob.clone()).await.unwrap();
            let link = LinkEntity::new("friend", alice.id, bob.id, None);
            link_svc.create(link.clone()).await.unwrap();
            (alice, bob, link)
        };

        // Reopen: everything must be on disk via per-mutation WAL commits,
        // and the UUID indexes must be rebuildable from the label scan.
        let store = ObrainStore::open(&dir).unwrap();
        assert_eq!(store.rebuild_index_for_label("test_data").unwrap(), 2);
        assert_eq!(store.rebuild_link_index("friend").unwrap(), 1);

        let data_svc = ObrainDataService::<TestDataEntity>::new(Arc::clone(&store));
        let link_svc = ObrainLinkService::new(Arc::clone(&store));

        let entities = data_svc.list().await.unwrap();
        assert_eq!(entities.len(), 2, "entities survive reopen");
        let found_alice = data_svc.get(&alice.id).await.unwrap();
        assert_eq!(found_alice.unwrap().entity_name, "Alice");

        // Property search works again after reopen (f_* properties persisted).
        let matches = data_svc.search("entity_name", "Bob").await.unwrap();
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].id, bob.id);

        let links = link_svc
            .find_by_source(&alice.id, None, None)
            .await
            .unwrap();
        assert_eq!(links.len(), 1, "link survives reopen");
        assert_eq!(links[0].id, link.id);
        assert_eq!(links[0].target_id, bob.id);

        // Mutations after reopen are durable too: update + delete then reopen
        // one more time and observe the final state.
        let mut updated = link.clone();
        updated.metadata = Some(serde_json::json!({"since": "2024"}));
        updated.touch();
        link_svc.update(&link.id, updated).await.unwrap();
        data_svc.delete(&alice.id).await.unwrap();

        drop(store);
        let store = ObrainStore::open(&dir).unwrap();
        assert_eq!(store.rebuild_index_for_label("test_data").unwrap(), 1);
        assert_eq!(store.rebuild_link_index("friend").unwrap(), 1);

        let data_svc = ObrainDataService::<TestDataEntity>::new(Arc::clone(&store));
        let link_svc = ObrainLinkService::new(store);
        assert!(
            data_svc.get(&alice.id).await.unwrap().is_none(),
            "delete durable"
        );
        assert!(
            data_svc.get(&bob.id).await.unwrap().is_some(),
            "remaining entity intact"
        );
        let mut remaining = link_svc.list().await.unwrap();
        assert_eq!(remaining.len(), 1);
        let survivor = remaining.pop().unwrap();
        assert_eq!(
            survivor
                .metadata
                .as_ref()
                .and_then(|m| m.get("since"))
                .and_then(|v| v.as_str()),
            Some("2024"),
            "post-reopen update durable"
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn test_delete_by_entity_is_durable_across_reopen() {
        let dir = std::env::temp_dir().join(format!("this_obrain_dbe_{}", Uuid::new_v4()));
        let user_id = Uuid::new_v4();

        {
            let store = ObrainStore::open(&dir).unwrap();
            let link_svc = ObrainLinkService::new(store);
            link_svc
                .create(LinkEntity::new("owner", user_id, Uuid::new_v4(), None))
                .await
                .unwrap();
            link_svc
                .create(LinkEntity::new("owner", user_id, Uuid::new_v4(), None))
                .await
                .unwrap();
            // Batch delete: must end in a WAL commit so both deletions are
            // durable even though the batch touches multiple nodes.
            link_svc.delete_by_entity(&user_id).await.unwrap();
        }

        let store = ObrainStore::open(&dir).unwrap();
        assert_eq!(
            store.rebuild_link_index("owner").unwrap(),
            0,
            "batch deletion must survive reopen (commit_wal after batch)"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    // -----------------------------------------------------------------------
    // Espaces (graphes nommés) — la dimension « spacieux »
    // -----------------------------------------------------------------------

    /// Ouvre un store adossé au substrat dans un répertoire temporaire.
    ///
    /// Les graphes nommés d'obrain sont des **sous-substrats physiques**
    /// (`<store>/graphs/<nom>`) : un store en mémoire les refuse, donc ces
    /// tests ne peuvent pas utiliser `test_store()`.
    fn substrate_store(tag: &str) -> (Arc<ObrainStore>, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!("this_obrain_{tag}_{}", Uuid::new_v4()));
        let store = ObrainStore::open(&dir).unwrap();
        (store, dir)
    }

    /// `use_graph` doit **effectivement** changer l'espace de lecture.
    ///
    /// Ce test existe parce que l'implémentation précédente était un no-op :
    /// elle posait le nom de graphe sur une session jetable, détruite aussitôt.
    /// Sans assertion sur le contenu relu, le bug restait invisible.
    #[tokio::test]
    async fn test_use_graph_scopes_reads() {
        let (store, dir) = substrate_store("space_scope");
        assert!(
            store.is_substrate(),
            "un store ouvert sur disque est adossé au substrat"
        );

        assert!(
            store.create_space("tenant-a").unwrap(),
            "création de l'espace A"
        );
        assert!(
            store.create_space("tenant-b").unwrap(),
            "création de l'espace B"
        );
        assert_eq!(
            store.spaces(),
            vec!["tenant-a".to_string(), "tenant-b".to_string()],
            "les espaces créés doivent être listés"
        );

        let service = ObrainDataService::<TestDataEntity>::new(Arc::clone(&store));

        store.use_graph("tenant-a");
        assert_eq!(store.space().as_deref(), Some("tenant-a"));
        let alice = service.create(TestDataEntity::new("Alice")).await.unwrap();

        // Un espace où l'on n'a rien écrit est vide — et le DOIT. C'est
        // l'assertion qui manquait à l'implémentation précédente : elle était
        // un no-op, donc toutes les lectures retombaient sur le graphe par
        // défaut et ce test serait passé sur du code cassé.
        store.use_graph("tenant-b");
        assert_eq!(store.space().as_deref(), Some("tenant-b"));
        assert!(
            service.list().await.unwrap().is_empty(),
            "l'espace B ne doit voir aucune entité de l'espace A"
        );
        assert!(
            service.get(&alice.id).await.unwrap().is_none(),
            "une entité de A ne doit pas être lisible depuis B"
        );

        // Retour dans A : la bascule a invalidé le cache (deux espaces allouent
        // leurs NodeId à partir de 1, un cache partagé aliasing), on le
        // reconstruit — exactement comme après un `open()`.
        store.use_graph("tenant-a");
        assert!(
            service.list().await.unwrap().is_empty(),
            "après une bascule, le cache est vide : la reprise est explicite"
        );
        assert_eq!(
            store.rebuild_index_for_label("test_data").unwrap(),
            1,
            "l'espace A doit contenir exactement son entité"
        );
        assert_eq!(service.list().await.unwrap().len(), 1);
        assert!(service.get(&alice.id).await.unwrap().is_some());

        // Le graphe par défaut est un troisième espace, distinct lui aussi.
        store.reset_graph();
        assert_eq!(store.space(), None);
        assert!(
            service.list().await.unwrap().is_empty(),
            "le graphe par défaut ne doit pas voir les entités de l'espace A"
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    /// Deux espaces peuplés en parallèle ne doivent pas se voir.
    ///
    /// C'est le test qui protège la raison d'être de l'invalidation du cache :
    /// les graphes nommés d'obrain sont des **substrats distincts** qui allouent
    /// chacun leurs `NodeId` à partir de 1. Un cache `Uuid → NodeId` partagé
    /// ferait donc résoudre l'entité d'un espace vers le nœud **homonyme** d'un
    /// autre — et `list()` rendrait deux fois la même entité. La bascule vide le
    /// cache ([`ObrainStore::use_graph`]) : c'est ce qui rend l'aliasing
    /// impossible, au prix d'une reprise explicite.
    ///
    /// Obrain a la notion qui rendrait le cache directement cloisonnable —
    /// `graph_scope()`, qui préfixe les identités par espace
    /// (`obrain-core/src/graph/traits.rs:1060`) — et this-rs ne l'utilise pas
    /// encore : un cache indexé par espace supprimerait la reprise manuelle.
    /// En attendant, ce test garantit qu'aucune entité ne traverse.
    #[tokio::test]
    async fn test_spaces_do_not_alias_through_the_global_uuid_index() {
        let (store, dir) = substrate_store("space_alias");
        assert!(store.create_space("tenant-a").unwrap());
        assert!(store.create_space("tenant-b").unwrap());

        let service = ObrainDataService::<TestDataEntity>::new(Arc::clone(&store));

        store.use_graph("tenant-a");
        let alice = service.create(TestDataEntity::new("Alice")).await.unwrap();

        store.use_graph("tenant-b");
        let bob = service.create(TestDataEntity::new("Bob")).await.unwrap();

        assert_ne!(alice.id, bob.id, "les entités sont distinctes par UUID");

        // Chaque espace ne doit relire que la sienne — après la reprise
        // explicite du cache, comme le veut le contrat du store.
        store.use_graph("tenant-a");
        assert_eq!(store.rebuild_index_for_label("test_data").unwrap(), 1);
        let in_a = service.list().await.unwrap();
        assert_eq!(
            in_a.len(),
            1,
            "l'espace A ne doit lister QUE son entité (alias d'index global ?)"
        );
        assert_eq!(in_a[0].id, alice.id);
        assert!(service.get(&bob.id).await.unwrap().is_none());

        store.use_graph("tenant-b");
        assert_eq!(store.rebuild_index_for_label("test_data").unwrap(), 1);
        let in_b = service.list().await.unwrap();
        assert_eq!(
            in_b.len(),
            1,
            "l'espace B ne doit lister QUE son entité (alias d'index global ?)"
        );
        assert_eq!(in_b[0].id, bob.id);
        assert!(service.get(&alice.id).await.unwrap().is_none());

        std::fs::remove_dir_all(&dir).ok();
    }

    /// Témoin d'un **défaut d'obrain** (ignoré tant qu'il n'est pas corrigé).
    ///
    /// `SubstrateStore::clustering_global()` calcule `Σ_v k(v)·(k(v)−1)/2`. Pour
    /// un nœud vivant de degré nul dans le type d'arête des cliques, `k − 1`
    /// sous-déborde un `u64` : **panic en debug**, résultat correct par wrap en
    /// release — donc invisible depuis `obrain topo stats` (binaire release).
    ///
    /// Le test est `#[ignore]` parce qu'il échoue par construction : c'est un
    /// témoin, pas une garantie. Le jour où obrain pose une garde `k == 0`, il
    /// suffira de le réactiver (il attend `Some(_)` au lieu du panic).
    #[test]
    #[ignore = "défaut obrain connu : obrain-substrate/src/store.rs:10859 (k - 1 sous-déborde)"]
    fn witness_clustering_global_panics_on_isolated_node() {
        let (store, dir) = substrate_store("clustering_defect");
        let sub = store.db().substrate_handle().expect("substrat");
        sub.configurer_passes_simpliciales(&obrain_substrate::simplex_passes::SimplexPassesSpec {
            clique_edge_type: "knows".into(),
            clique_labels: Vec::new(),
            dispatch: None,
            maintenance: true,
        })
        .expect("configuration des passes");
        // Un nœud vivant, sans aucune arête `knows` : degré nul.
        store.db().session().create_node(&["isolated"]);
        sub.materialiser_simplexes().expect("passe");

        let result =
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| sub.clustering_global()));
        assert!(
            result.is_err(),
            "attendu : panic (soustraction débordante) tant que la garde k==0 manque ; \
             si ce test se met à rendre Ok, le défaut est corrigé — le réactiver"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    /// **Le fencing est réel** : un writer périmé est refusé, en entrée d'opération.
    ///
    /// Ce test porte trois propriétés :
    /// 1. le bail se prend et les écritures passent tant qu'il est vivant ;
    /// 2. un **second store dans le même processus** sur le même répertoire est
    ///    refusé — c'est exactement le trou d'obrain (`file.rs:179-207` rend la
    ///    part de verrou existante, donc obrain les laisserait passer tous les
    ///    deux) ;
    /// 3. une fois le bail perdu, **l'écriture est refusée mais la lecture
    ///    reste possible** — un writer périmé peut encore servir des lectures.
    #[tokio::test]
    async fn test_write_fencing_refuses_a_stale_writer() {
        use crate::storage::writer_lease::FileAuthority;

        let dir = std::env::temp_dir().join(format!("this_obrain_lease_{}", Uuid::new_v4()));
        let store = ObrainStore::open(&dir).unwrap();
        let authority = FileAuthority::new(&dir);

        let epoch = store.enable_write_fencing_with(&authority).unwrap();
        assert_eq!(epoch, 1, "premier bail = époque 1");
        assert_eq!(store.lease_epoch(), Some(1));

        let service = ObrainDataService::<TestDataEntity>::new(Arc::clone(&store));

        // 1. Bail vivant : l'écriture passe.
        let entity = service.create(TestDataEntity::new("avant")).await.unwrap();

        // 2. Un second store, MÊME processus, MÊME répertoire : obrain le laisse
        //    passer, le bail non.
        let store2 = ObrainStore::open(&dir).unwrap();
        let refused = store2.enable_write_fencing_with(&authority);
        assert!(
            refused.is_err(),
            "un second écrivain in-process doit être refusé, obtenu {:?}",
            refused.ok()
        );

        // 3. Notre writer est périmé (un autre a repris le bail).
        store.fence_write_lease_for_test(2);
        let write = service.create(TestDataEntity::new("apres")).await;
        assert!(
            write.is_err(),
            "un writer périmé ne doit PLUS écrire : {:?}",
            write.map(|e| e.id)
        );
        assert!(
            write.unwrap_err().to_string().contains("refusée"),
            "le refus doit être explicite"
        );
        // …mais la lecture reste servie.
        assert!(
            service.get(&entity.id).await.unwrap().is_some(),
            "un writer périmé peut encore LIRE"
        );

        // 4. Après relâchement, un successeur prend le bail avec l'époque SUIVANTE.
        store.release_write_fencing();
        let store3 = ObrainStore::open(&dir).unwrap();
        let epoch3 = store3.enable_write_fencing_with(&authority).unwrap();
        assert_eq!(epoch3, 2, "l'époque ne recule jamais");

        std::fs::remove_dir_all(&dir).ok();
    }

    /// Le relief suit les écritures de this-rs — **sans passe batch**.
    ///
    /// C'est la promesse qui rend le relief exploitable : obrain appelle la
    /// maintenance simpliciale *dans la transaction* de l'arête
    /// (`obrain-substrate/src/store.rs:16154`), donc un lien créé par
    /// `ObrainLinkService` met à jour triangles/cliques sans qu'aucun job ne
    /// repasse derrière. Ce test l'exige au lieu de le supposer : il configure
    /// les passes, matérialise **une** fois, puis écrit un second triangle par
    /// le chemin normal et vérifie que le compteur a suivi tout seul.
    #[tokio::test]
    async fn test_simplex_relief_follows_this_rs_writes() {
        use crate::graph::relief::{Relief, ReliefOptions};

        let (store, dir) = substrate_store("simplex_writes");
        let data = ObrainDataService::<TestDataEntity>::new(Arc::clone(&store));
        let links = ObrainLinkService::new(Arc::clone(&store));

        // Triangle 1 : a—b, b—c, a—c. `try_create_edge` ne crée d'arête native
        // que si les deux extrémités sont déjà dans l'index, d'où l'ordre.
        let a = data.create(TestDataEntity::new("a")).await.unwrap();
        let b = data.create(TestDataEntity::new("b")).await.unwrap();
        let c = data.create(TestDataEntity::new("c")).await.unwrap();
        for (src, dst) in [(a.id, b.id), (b.id, c.id), (a.id, c.id)] {
            links
                .create(LinkEntity::new("knows", src, dst, None))
                .await
                .unwrap();
        }

        // Passes simpliciales : triangles sur `knows`, aucun filtre de label,
        // maintenance incrémentale ACTIVE.
        let sub = store
            .db()
            .substrate_handle()
            .expect("store substrat-backed");
        sub.configurer_passes_simpliciales(&obrain_substrate::simplex_passes::SimplexPassesSpec {
            clique_edge_type: "knows".into(),
            clique_labels: Vec::new(),
            dispatch: None,
            maintenance: true,
        })
        .expect("configuration des passes");
        let first = sub.materialiser_simplexes().expect("passe complète");
        sub.flush().expect("flush");
        assert_eq!(
            first.triangles, 1,
            "un triangle fermé = exactement un triangle matérialisé"
        );

        // Le relief publie l'état ET le compte.
        let before = Relief::describe(
            &store,
            ReliefOptions {
                include_counts: true,
            },
        );
        assert!(before.engine.simplex.configured, "passes configurées");
        assert!(
            before.engine.simplex.maintenance,
            "la maintenance au commit doit être active"
        );
        assert!(
            before.engine.simplex.counts_error.is_none(),
            "les compteurs doivent être calculables : {:?}",
            before.engine.simplex.counts_error
        );
        assert_eq!(before.engine.simplex.counts.unwrap().triangles, 1);

        // SECOND triangle (b, c, d), écrit par this-rs : b—d et c—d suffisent,
        // b—c existe déjà. AUCUNE passe n'est relancée ici.
        let d = data.create(TestDataEntity::new("d")).await.unwrap();
        for (src, dst) in [(b.id, d.id), (c.id, d.id)] {
            links
                .create(LinkEntity::new("knows", src, dst, None))
                .await
                .unwrap();
        }

        let after = Relief::describe(
            &store,
            ReliefOptions {
                include_counts: true,
            },
        );
        assert_eq!(
            after.engine.simplex.counts.unwrap().triangles,
            2,
            "la maintenance au commit doit avoir ajouté le triangle sans passe batch"
        );

        std::fs::remove_dir_all(&dir).ok();
    }
}
