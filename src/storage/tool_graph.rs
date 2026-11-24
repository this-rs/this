//! `ToolGraph` : le dépôt des outils d'auteur du harnais, rangé comme NŒUDS d'un graphe obrain.
//!
//! Implémente [`ToolRepo`] (le port de `this-harness`) sur un `ObrainStore`. Aucun JSON de
//! catalogue : un outil EST du graphe.
//!
//! ```text
//! (:_tool_meta {_schema, _seq})                       un seul nœud : garde de schéma + horloge
//! (:_tool {_name, _active, _current, _seq, _steps_hash, _text, _dropped,
//!          _status_reason, _replaced_by})
//!     ├─[:HAS_VERSION]─▶ (:_tool_version {_tool, _version, _seq, _description, _note,
//!     │                                   _steps_hash, _tags, _params, _param_types, _required,
//!     │                                   _uses, _ok, _failed, _refused, _last_used})
//!     │                     └─[:HAS_STEP {_index}]─▶ (:_tool_step {_tool, _version, _index,
//!     │                                                            _called, _args})
//!     │                                                  └─[:CALLS]─▶ (:_host_tool {_name})
//! ```
//!
//! `(:_host_tool {_name})` est upserté par nom de l'outil de l'hôte : c'est ce qui répond, par
//! parcours, à « quels outils d'auteur cassent si l'outil X de l'hôte disparaît ».
//! [`ToolGraph::authored_calling`] fait ce parcours.
//!
//! # Ce qui est du JSON, et pourquoi
//!
//! Seuls les ARGUMENTS d'une étape (`_args`) sont une chaîne JSON : c'est un arbre arbitraire.
//! Tout le reste est typé (entiers, booléens, listes de chaînes). Le schéma de paramètres est
//! rangé en listes typées parallèles (`_params` = noms, `_param_types`, `_required`) quand il a la
//! forme que le catalogue produit (`{type:"object", properties:{n:{type:t}}, required:[…]}`,
//! sans autre clé). Un schéma d'une autre forme — que `put` accepte, car le port ne le
//! valide pas — est rangé tel quel dans `_params_raw` (chaîne JSON) pour que `get` reste EXACT :
//! c'est un repli, pas le chemin normal.
//!
//! # Un commit par `put`
//!
//! Un `put` écrit l'outil, toutes ses versions, étapes et arêtes, puis committe UNE fois.
//! `tick()` committe aussi (il est durable par contrat) : créer un outil coûte donc deux
//! commits, un `tick` et un `put`.
//!
//! **Le commit n'est pas constant** (mesuré en release, APFS, tests/tool_graph.rs
//! `mesure_d_echelle`) : un `commit_wal` sans rien de neuf coûte ~19 ms à toute taille, mais un
//! commit qui suit une écriture coûte ~22 ms à 100 outils, ~30 ms à 1 000 et ~105 ms à 10 000
//! (c'est le coût d'un `tick`, qui n'écrit qu'une propriété). L'écriture elle-même (nœuds,
//! arêtes, index) vaut ~19 à 27 ms par outil, et la recherche de doublon ~1 µs (index) : le
//! commit domine, pas le balayage. Pour un import en masse, [`ToolGraph::put_many_sync`]
//! écrit un lot en UN commit (10 000 outils en ~10 s en lots de 100, contre ~130 ms par `put`).
//!
//! **Atomicité.** L'API d'écriture d'obrain n'a pas de rollback de lot côté `GraphStoreMut`.
//! `put` valide d'abord (écriture permise, seq représentables) avant de toucher au graphe ; si le
//! commit échoue, il RÉÉCRIT l'ancien enregistrement (ou supprime le nouveau) en mémoire et rend
//! `Err`. Ce qui est vérifié : le chemin « refus avant écriture » (store en lecture seule) et le
//! chemin « commit en échec » (test interne à ce module, commit injecté). Ce qui ne l'est pas : un
//! crash du processus ENTRE les mutations et le commit — le WAL n'ayant rien reçu de commité, la
//! reprise repart de l'état précédent, mais ce n'est pas testé ici.
//!
//! **Remplacement.** `put` sur un nom existant supprime les anciens nœuds versions/étapes (et
//! leurs arêtes) puis réécrit : une version sortie du plafond de versions disparaît. Un
//! `_host_tool` qui n'est plus appelé par personne est supprimé.
//!
//! # Index
//!
//! `open` déclare des index de propriété (`create_property_index`) sur `_name`, `_steps_hash` et
//! `_active`. `ObrainDB::create_property_index` existe sans feature particulière et, sur le
//! substrat, route vers l'index natif (`obrain-engine/src/database/index.rs`). `get` et
//! `active_with_steps` passent par `find_nodes_by_property` (filtré par label : `_name` est aussi
//! porté par les `_host_tool`).
//!
//! **Pas d'index texte.** `create_text_index` / `text_search` sont gatés derrière la feature
//! `text-index` d'`obrain-engine`, que `this` n'active pas (`features = ["algos"]` seulement) :
//! ils ne sont pas compilés ici, donc non utilisés. `candidates` (sans source injectée) et
//! `recent` BALAIENT le label `_tool`. Le coût mesuré est dans le test d'échelle opt-in
//! (`tests/tool_graph.rs`, `cargo test --features obrain --test tool_graph -- --ignored
//! --nocapture`) ; un hôte qui a mieux (BM25 + vecteurs) l'injecte par
//! [`ToolGraph::with_candidate_source`].
//!
//! **Scoring lexical de repli.** `catalogue::tokens` et `doc_vector` ne sont pas publics : ce
//! module a son propre scoring, plus simple (jetons sans accents ni pluriel simple ; cosinus
//! entre le vecteur de fréquence du texte de l'outil — nom ×3, étiquettes ×2, description ×1,
//! par répétition dans `_text` — et la requête pondérée par idf calculé sur le catalogue). Il
//! ne reproduit pas exactement le classement de `MemRepo` ; le catalogue reclasse de toute
//! façon les candidats.
//!
//! # Un seul écrivain
//!
//! Le store est partagé (`Arc<ObrainStore>`), jamais rouvert : une instance en écriture par base
//! et par processus (`OBRAIN-X001`).

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::{Arc, Mutex};

use obrain_common::types::{NodeId, PropertyKey, Value as ObrainValue};
use obrain_core::graph::{Direction, GraphStoreMut};
use serde_json::{Map, Value as Json};
use this_harness::{RecipeStep, ToolDef, ToolRecord, ToolRepo, ToolStats, ToolStatus};

use super::obrain::ObrainStore;

/// Version du schéma que ce code sait lire et écrire.
pub const TOOL_SCHEMA: i64 = 1;

const META: &str = "_tool_meta";
const TOOL: &str = "_tool";
const VERSION: &str = "_tool_version";
const STEP: &str = "_tool_step";
const HOST: &str = "_host_tool";
const HAS_VERSION: &str = "HAS_VERSION";
const HAS_STEP: &str = "HAS_STEP";
const CALLS: &str = "CALLS";

/// Score minimal d'un candidat lexical : en dessous, c'est du bruit.
const CANDIDATE_MIN: f64 = 0.08;
/// Plafond d'outils (même valeur que `this_harness::MAX_TOOLS`).
const CAPACITY: usize = this_harness::MAX_TOOLS;

/// Source de candidats injectée par l'application : `(requête, k) -> noms`.
/// Partagée entre threads : le dépôt doit rester `Send + Sync` pour vivre dans l'état géré de
/// l'application (Tauri).
pub type CandidateSource = Arc<dyn Fn(&str, usize) -> Vec<String> + Send + Sync>;

/// Les outils d'auteur, dans un `ObrainStore`.
pub struct ToolGraph {
    store: Arc<ObrainStore>,
    meta: NodeId,
    source: Option<CandidateSource>,
    capacity: usize,
    /// Un seul écrivain à la fois dans ce processus (`put`, `tick`).
    write: Mutex<()>,
    /// Test seulement : fait échouer le commit pour vérifier la compensation.
    #[cfg(test)]
    fail_commit: std::cell::Cell<bool>,
}

impl ToolGraph {
    /// Ouvre le dépôt sur `store` : crée le méta à la première ouverture, REFUSE un `_schema`
    /// inconnu, déclare les index de propriété.
    pub fn open(store: Arc<ObrainStore>) -> Result<Self, String> {
        let graph = store.db().graph_store_mut();
        let metas = graph.nodes_by_label(META);
        let meta = match metas.as_slice() {
            [] => {
                if store.is_read_only() {
                    return Err("tool graph : base en lecture seule, pas de méta à créer".into());
                }
                store.check_write().map_err(|e| e.to_string())?;
                let id = graph.create_node(&[META]);
                graph.set_node_property(id, "_schema", ObrainValue::from(TOOL_SCHEMA));
                graph.set_node_property(id, "_seq", ObrainValue::from(0_i64));
                store
                    .commit_wal()
                    .map_err(|e| format!("tool graph : création du méta : {e}"))?;
                id
            }
            [id] => {
                let found = graph
                    .get_node_property(*id, &PropertyKey::new("_schema"))
                    .and_then(|v| v.as_int64());
                if found != Some(TOOL_SCHEMA) {
                    return Err(format!(
                        "tool graph : schéma inconnu (base en v{}, ce code lit la v{TOOL_SCHEMA}) : \
                         refus d'ouvrir plutôt que de mal lire",
                        found.map_or_else(|| "?".to_string(), |v| v.to_string()),
                    ));
                }
                *id
            }
            many => {
                return Err(format!(
                    "tool graph : {} nœuds {META} (un seul attendu) : base incohérente",
                    many.len()
                ));
            }
        };
        let db = store.db();
        for prop in ["_name", "_steps_hash", "_active"] {
            if !db.has_property_index(prop) {
                db.create_property_index(prop);
            }
        }
        Ok(Self {
            store,
            meta,
            source: None,
            capacity: CAPACITY,
            write: Mutex::new(()),
            #[cfg(test)]
            fail_commit: std::cell::Cell::new(false),
        })
    }

    /// Une source de candidats fournie par l'application (BM25 + vecteurs, par exemple).
    /// Ses réponses sont filtrées : seuls les noms d'outils ACTIFS existants sont gardés.
    #[must_use]
    pub fn with_candidate_source(mut self, source: CandidateSource) -> Self {
        self.source = Some(source);
        self
    }

    /// Un dépôt qui refuse la création au-delà de `capacity` outils.
    #[must_use]
    pub fn with_capacity(mut self, capacity: usize) -> Self {
        self.capacity = capacity;
        self
    }

    /// Les noms des outils d'auteur ACTIFS dont une étape appelle l'outil `host_tool` de l'hôte,
    /// par parcours : `(:_host_tool {_name})<-[:CALLS]-(étape)<-[:HAS_STEP]-(version)<-[:HAS_VERSION]-(:_tool)`.
    /// Seule la version COURANTE de l'outil compte. Trié, sans doublon.
    #[must_use]
    pub fn authored_calling(&self, host_tool: &str) -> Vec<String> {
        let graph = self.graph();
        let Some(host) = self.find_labeled(HOST, host_tool) else {
            return Vec::new();
        };
        let mut out = std::collections::BTreeSet::new();
        for (step, _) in graph.edges_from(host, Direction::Incoming) {
            for (version, _) in graph.edges_from(step, Direction::Incoming) {
                for (tool, _) in graph.edges_from(version, Direction::Incoming) {
                    let current = self.int(tool, "_current");
                    if self.int(version, "_version") != current {
                        continue;
                    }
                    if self.bool(tool, "_active") != Some(true) {
                        continue;
                    }
                    if let Some(name) = self.string(tool, "_name") {
                        out.insert(name);
                    }
                }
            }
        }
        out.into_iter().collect()
    }

    // ---- accès bas niveau ----------------------------------------------------

    fn graph(&self) -> Arc<dyn GraphStoreMut> {
        self.store.db().graph_store_mut()
    }

    fn prop(&self, id: NodeId, key: &str) -> Option<ObrainValue> {
        self.graph().get_node_property(id, &PropertyKey::new(key))
    }

    fn string(&self, id: NodeId, key: &str) -> Option<String> {
        self.prop(id, key)
            .and_then(|v| v.as_str().map(str::to_string))
    }

    fn int(&self, id: NodeId, key: &str) -> Option<i64> {
        self.prop(id, key).and_then(|v| v.as_int64())
    }

    fn bool(&self, id: NodeId, key: &str) -> Option<bool> {
        self.prop(id, key).and_then(|v| v.as_bool())
    }

    fn strings(&self, id: NodeId, key: &str) -> Option<Vec<String>> {
        let value = self.prop(id, key)?;
        let list = value.as_list()?;
        list.iter()
            .map(|v| v.as_str().map(str::to_string))
            .collect()
    }

    /// Le nœud de `label` dont `_name` vaut `name` (index de propriété, filtré par label).
    fn find_labeled(&self, label: &str, name: &str) -> Option<NodeId> {
        let graph = self.graph();
        graph
            .find_nodes_by_property("_name", &ObrainValue::from(name))
            .into_iter()
            .find(|id| graph.node_has_label(*id, label) == Some(true))
    }

    fn commit(&self) -> Result<(), String> {
        #[cfg(test)]
        if self.fail_commit.get() {
            return Err("commit injecté en échec".to_string());
        }
        self.store.commit_wal().map_err(|e| e.to_string())
    }

    // ---- lecture d'un enregistrement ------------------------------------------

    fn read_record(&self, tool: NodeId) -> Option<ToolRecord> {
        let graph = self.graph();
        let name = self.string(tool, "_name")?;
        let mut versions: Vec<ToolDef> = Vec::new();
        for (version, _) in graph.edges_from(tool, Direction::Outgoing) {
            if graph.node_has_label(version, VERSION) != Some(true) {
                continue;
            }
            versions.push(self.read_version(version)?);
        }
        versions.sort_by_key(|d| d.version);
        let status = if self.bool(tool, "_active")? {
            ToolStatus::Active
        } else {
            ToolStatus::Retired {
                reason: self.string(tool, "_status_reason").unwrap_or_default(),
                replaced_by: self.string(tool, "_replaced_by"),
            }
        };
        Some(ToolRecord {
            name,
            versions,
            status,
            dropped: u32::try_from(self.int(tool, "_dropped")?).ok()?,
        })
    }

    fn read_version(&self, version: NodeId) -> Option<ToolDef> {
        let graph = self.graph();
        let mut steps: Vec<(i64, RecipeStep)> = Vec::new();
        for (step, _) in graph.edges_from(version, Direction::Outgoing) {
            if graph.node_has_label(step, STEP) != Some(true) {
                continue;
            }
            let args = self.string(step, "_args")?;
            steps.push((
                self.int(step, "_index")?,
                RecipeStep {
                    tool: self.string(step, "_called")?,
                    args: serde_json::from_str(&args).ok()?,
                },
            ));
        }
        steps.sort_by_key(|(i, _)| *i);
        let u32_of = |key: &str| u32::try_from(self.int(version, key)?).ok();
        let u64_of = |key: &str| u64::try_from(self.int(version, key)?).ok();
        Some(ToolDef {
            version: u32_of("_version")?,
            description: self.string(version, "_description")?,
            parameters: self.read_params(version)?,
            steps: steps.into_iter().map(|(_, s)| s).collect(),
            steps_hash: self.string(version, "_steps_hash")?,
            tags: self.strings(version, "_tags")?,
            note: self.string(version, "_note")?,
            seq: u64_of("_seq")?,
            stats: ToolStats {
                uses: u32_of("_uses")?,
                ok: u32_of("_ok")?,
                failed: u32_of("_failed")?,
                refused: u32_of("_refused")?,
                last_used: u64_of("_last_used")?,
            },
        })
    }

    fn read_params(&self, version: NodeId) -> Option<Json> {
        if let Some(raw) = self.string(version, "_params_raw") {
            return serde_json::from_str(&raw).ok();
        }
        let names = self.strings(version, "_params")?;
        let types = self.strings(version, "_param_types")?;
        if names.len() != types.len() {
            return None;
        }
        let properties: Map<String, Json> = names
            .into_iter()
            .zip(types)
            .map(|(n, t)| (n, serde_json::json!({ "type": t })))
            .collect();
        let mut out = Map::new();
        out.insert("type".into(), Json::String("object".into()));
        out.insert("properties".into(), Json::Object(properties));
        if let Some(required) = self.strings(version, "_required") {
            out.insert(
                "required".into(),
                Json::Array(required.into_iter().map(Json::String).collect()),
            );
        }
        Some(Json::Object(out))
    }

    // ---- écriture d'un enregistrement (sans commit) ------------------------------

    /// Retire les versions et étapes d'un outil (et leurs arêtes). Rend les `_host_tool` qu'elles
    /// visaient, à balayer ensuite.
    fn clear_versions(&self, tool: NodeId) -> Vec<NodeId> {
        let graph = self.graph();
        let mut hosts = Vec::new();
        for (version, _) in graph.edges_from(tool, Direction::Outgoing) {
            if graph.node_has_label(version, VERSION) != Some(true) {
                continue;
            }
            for (step, _) in graph.edges_from(version, Direction::Outgoing) {
                if graph.node_has_label(step, STEP) != Some(true) {
                    continue;
                }
                for (host, _) in graph.edges_from(step, Direction::Outgoing) {
                    hosts.push(host);
                }
                graph.delete_node_edges(step);
                graph.delete_node(step);
            }
            graph.delete_node_edges(version);
            graph.delete_node(version);
        }
        hosts
    }

    /// Supprime les `_host_tool` que plus aucune étape n'appelle.
    fn sweep_hosts(&self, hosts: &[NodeId]) {
        let graph = self.graph();
        let mut seen = HashSet::new();
        for host in hosts {
            if seen.insert(*host) && graph.in_degree(*host) == 0 {
                graph.delete_node(*host);
            }
        }
    }

    fn host_node(&self, cache: &mut HashMap<String, NodeId>, name: &str) -> NodeId {
        if let Some(id) = cache.get(name) {
            return *id;
        }
        let id = self.find_labeled(HOST, name).unwrap_or_else(|| {
            let graph = self.graph();
            let id = graph.create_node(&[HOST]);
            graph.set_node_property(id, "_name", ObrainValue::from(name));
            id
        });
        cache.insert(name.to_string(), id);
        id
    }

    /// Écrit `record` (remplace l'existant). Ne committe pas.
    fn write_record(&self, record: &ToolRecord) {
        let graph = self.graph();
        let tool = self
            .find_labeled(TOOL, &record.name)
            .unwrap_or_else(|| graph.create_node(&[TOOL]));
        let old_hosts = self.clear_versions(tool);

        let current = record.current();
        let text = search_text(&record.name, &current.tags, &current.description);
        let set =
            |id: NodeId, key: &str, value: ObrainValue| graph.set_node_property(id, key, value);
        let int = |n: u64| ObrainValue::from(n as i64);
        set(tool, "_name", ObrainValue::from(record.name.as_str()));
        set(tool, "_active", ObrainValue::from(record.is_active()));
        set(
            tool,
            "_current",
            ObrainValue::from(i64::from(current.version)),
        );
        set(tool, "_seq", int(current.seq));
        set(
            tool,
            "_steps_hash",
            ObrainValue::from(current.steps_hash.as_str()),
        );
        set(tool, "_text", ObrainValue::from(text));
        set(
            tool,
            "_dropped",
            ObrainValue::from(i64::from(record.dropped)),
        );
        match &record.status {
            ToolStatus::Active => {
                graph.remove_node_property(tool, "_status_reason");
                graph.remove_node_property(tool, "_replaced_by");
            }
            ToolStatus::Retired {
                reason,
                replaced_by,
            } => {
                set(tool, "_status_reason", ObrainValue::from(reason.as_str()));
                match replaced_by {
                    Some(r) => set(tool, "_replaced_by", ObrainValue::from(r.as_str())),
                    None => {
                        graph.remove_node_property(tool, "_replaced_by");
                    }
                }
            }
        }

        let mut hosts: HashMap<String, NodeId> = HashMap::new();
        for def in &record.versions {
            let v = graph.create_node(&[VERSION]);
            set(v, "_tool", ObrainValue::from(record.name.as_str()));
            set(v, "_version", ObrainValue::from(i64::from(def.version)));
            set(v, "_seq", int(def.seq));
            set(
                v,
                "_description",
                ObrainValue::from(def.description.as_str()),
            );
            set(v, "_note", ObrainValue::from(def.note.as_str()));
            set(v, "_steps_hash", ObrainValue::from(def.steps_hash.as_str()));
            set(v, "_tags", string_list(&def.tags));
            self.write_params(v, &def.parameters);
            set(v, "_uses", ObrainValue::from(i64::from(def.stats.uses)));
            set(v, "_ok", ObrainValue::from(i64::from(def.stats.ok)));
            set(v, "_failed", ObrainValue::from(i64::from(def.stats.failed)));
            set(
                v,
                "_refused",
                ObrainValue::from(i64::from(def.stats.refused)),
            );
            set(v, "_last_used", int(def.stats.last_used));
            graph.create_edge(tool, v, HAS_VERSION);
            for (index, step) in def.steps.iter().enumerate() {
                let s = graph.create_node(&[STEP]);
                set(s, "_tool", ObrainValue::from(record.name.as_str()));
                set(s, "_version", ObrainValue::from(i64::from(def.version)));
                set(s, "_index", ObrainValue::from(index as i64));
                set(s, "_called", ObrainValue::from(step.tool.as_str()));
                set(s, "_args", ObrainValue::from(step.args.to_string()));
                let edge = graph.create_edge(v, s, HAS_STEP);
                graph.set_edge_property(edge, "_index", ObrainValue::from(index as i64));
                let host = self.host_node(&mut hosts, &step.tool);
                graph.create_edge(s, host, CALLS);
            }
        }
        self.sweep_hosts(&old_hosts);
    }

    fn write_params(&self, version: NodeId, parameters: &Json) {
        let graph = self.graph();
        match reduce_params(parameters) {
            Some((names, types, required)) => {
                graph.set_node_property(version, "_params", string_list(&names));
                graph.set_node_property(version, "_param_types", string_list(&types));
                if let Some(required) = required {
                    graph.set_node_property(version, "_required", string_list(&required));
                }
            }
            None => graph.set_node_property(
                version,
                "_params_raw",
                ObrainValue::from(parameters.to_string()),
            ),
        }
    }

    fn remove_record(&self, name: &str) {
        let graph = self.graph();
        if let Some(tool) = self.find_labeled(TOOL, name) {
            let hosts = self.clear_versions(tool);
            graph.delete_node_edges(tool);
            graph.delete_node(tool);
            self.sweep_hosts(&hosts);
        }
    }

    fn scan_tools(&self) -> Vec<NodeId> {
        self.graph().nodes_by_label(TOOL)
    }
}

fn string_list(items: &[String]) -> ObrainValue {
    let list: Vec<ObrainValue> = items
        .iter()
        .map(|s| ObrainValue::from(s.as_str()))
        .collect();
    ObrainValue::List(list.into())
}

type ReducedParams = (Vec<String>, Vec<String>, Option<Vec<String>>);

/// Le schéma sous forme typée, s'il a EXACTEMENT la forme que le catalogue produit.
fn reduce_params(parameters: &Json) -> Option<ReducedParams> {
    let obj = parameters.as_object()?;
    if obj
        .keys()
        .any(|k| !["type", "properties", "required"].contains(&k.as_str()))
    {
        return None;
    }
    if obj.get("type")?.as_str()? != "object" {
        return None;
    }
    let mut names = Vec::new();
    let mut types = Vec::new();
    for (name, spec) in obj.get("properties")?.as_object()? {
        let spec = spec.as_object()?;
        if spec.len() != 1 {
            return None;
        }
        names.push(name.clone());
        types.push(spec.get("type")?.as_str()?.to_string());
    }
    let required = match obj.get("required") {
        None => None,
        Some(list) => Some(
            list.as_array()?
                .iter()
                .map(|r| r.as_str().map(str::to_string))
                .collect::<Option<Vec<_>>>()?,
        ),
    };
    Some((names, types, required))
}

// ---- lexical -----------------------------------------------------------------------

const STOP: &[&str] = &[
    "le", "la", "les", "un", "une", "des", "de", "du", "et", "ou", "en", "au", "aux", "ce", "ces",
    "the", "an", "of", "to", "in", "on", "and", "or", "for", "is", "my", "mes", "mon", "ma",
];

fn fold(c: char) -> char {
    match c {
        'à' | 'á' | 'â' | 'ä' => 'a',
        'ç' => 'c',
        'è' | 'é' | 'ê' | 'ë' => 'e',
        'ì' | 'í' | 'î' | 'ï' => 'i',
        'ò' | 'ó' | 'ô' | 'ö' | 'œ' => 'o',
        'ù' | 'ú' | 'û' | 'ü' => 'u',
        other => other,
    }
}

fn tokens(text: &str) -> Vec<String> {
    let flat: String = text.to_lowercase().chars().map(fold).collect();
    flat.split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|t| t.len() >= 2 && !STOP.contains(t))
        .map(|t| {
            if t.len() > 4 && t.ends_with('s') {
                t[..t.len() - 1].to_string()
            } else {
                t.to_string()
            }
        })
        .collect()
}

/// Le texte cherchable : nom ×3, étiquettes ×2, description ×1 (par répétition).
fn search_text(name: &str, tags: &[String], description: &str) -> String {
    let mut out = String::new();
    for _ in 0..3 {
        out.push_str(name);
        out.push(' ');
    }
    for tag in tags {
        out.push_str(tag);
        out.push(' ');
        out.push_str(tag);
        out.push(' ');
    }
    out.push_str(description);
    out
}

impl ToolGraph {
    /// [`ToolRepo::get`], synchrone.
    ///
    /// `Ok(None)` = absent. Un nœud présent mais illisible (corrompu, écrit par un autre
    /// schéma) est une `Err` : une panne de lecture ne passe jamais pour une absence.
    pub fn get_sync(&self, name: &str) -> Result<Option<ToolRecord>, String> {
        let Some(tool) = self.find_labeled(TOOL, name) else {
            return Ok(None);
        };
        self.read_record(tool)
            .map(Some)
            .ok_or_else(|| format!("tool graph : l'outil « {name} » est présent mais illisible"))
    }

    /// [`ToolRepo::put`], synchrone : un enregistrement, un commit.
    pub fn put_sync(&self, record: &ToolRecord) -> Result<(), String> {
        self.put_many_sync(std::slice::from_ref(record))
    }

    /// Écrit PLUSIEURS enregistrements en UN commit (import en masse). Même contrat que `put` :
    /// `Err` ⇒ rien n'a changé (l'état d'avant est réécrit en mémoire, dans l'ordre inverse).
    /// Hors du trait : le catalogue, lui, écrit un enregistrement à la fois.
    ///
    /// Pourquoi : le commit coûte cher et GRANDIT avec la taille de la base (voir la mesure
    /// d'échelle) ; le répartir sur un lot est le seul levier sans changer le contrat.
    pub fn put_many_sync(&self, records: &[ToolRecord]) -> Result<(), String> {
        let _guard = self
            .write
            .lock()
            .map_err(|_| "verrou d'écriture empoisonné")?;
        // Tout ce qui peut refuser se décide AVANT de toucher au graphe.
        if self.store.is_read_only() {
            return Err("tool graph : base en lecture seule".into());
        }
        self.store.check_write().map_err(|e| e.to_string())?;
        let too_big = |n: u64| i64::try_from(n).is_err();
        let mut fresh: HashSet<&str> = HashSet::new();
        for record in records {
            if record.versions.is_empty() {
                return Err("un enregistrement a toujours une version".into());
            }
            if record
                .versions
                .iter()
                .any(|d| too_big(d.seq) || too_big(d.stats.last_used))
            {
                return Err("seq hors de la plage i64".into());
            }
            if self.find_labeled(TOOL, &record.name).is_none() {
                fresh.insert(record.name.as_str());
            }
        }
        if !fresh.is_empty() && self.len_sync() + fresh.len() > self.capacity {
            return Err(format!("dépôt plein ({} outils)", self.capacity));
        }
        // L'état d'avant, pris UNE fois par nom (le premier de la liste), avant toute écriture.
        let mut before: Vec<(&str, Option<ToolRecord>)> = Vec::new();
        let mut taken: HashSet<&str> = HashSet::new();
        for record in records {
            if taken.insert(record.name.as_str()) {
                before.push((record.name.as_str(), self.get_sync(&record.name)?));
            }
        }
        for record in records {
            self.write_record(record);
        }
        if let Err(why) = self.commit() {
            // Compensation : le lot n'est pas durable ; on remet la mémoire dans l'état d'avant.
            for (name, old) in before.iter().rev() {
                match old {
                    Some(old) => self.write_record(old),
                    None => self.remove_record(name),
                }
            }
            return Err(format!("commit refusé : {why}"));
        }
        Ok(())
    }

    /// [`ToolRepo::len`], synchrone.
    pub fn len_sync(&self) -> usize {
        self.graph().node_count_by_label(TOOL)
    }

    /// [`ToolRepo::active_len`], synchrone.
    pub fn active_len_sync(&self) -> usize {
        self.graph()
            .find_nodes_by_property("_active", &ObrainValue::from(true))
            .into_iter()
            .filter(|id| self.graph().node_has_label(*id, TOOL) == Some(true))
            .count()
    }

    /// [`ToolRepo::active_with_steps`], synchrone.
    pub fn active_with_steps_sync(&self, steps_hash: &str, except: Option<&str>) -> Option<String> {
        let graph = self.graph();
        let mut found: Vec<String> = graph
            .find_nodes_by_property("_steps_hash", &ObrainValue::from(steps_hash))
            .into_iter()
            .filter(|id| graph.node_has_label(*id, TOOL) == Some(true))
            .filter(|id| self.bool(*id, "_active") == Some(true))
            .filter_map(|id| self.string(id, "_name"))
            .filter(|name| Some(name.as_str()) != except)
            .collect();
        found.sort();
        found.into_iter().next()
    }

    /// [`ToolRepo::candidates`], synchrone.
    pub fn candidates_sync(&self, query: &str, k: usize) -> Vec<String> {
        if k == 0 {
            return Vec::new();
        }
        if let Some(source) = &self.source {
            let mut seen = HashSet::new();
            return source(query, k)
                .into_iter()
                .filter(|name| seen.insert(name.clone()))
                .filter(|name| {
                    self.find_labeled(TOOL, name)
                        .is_some_and(|t| self.bool(t, "_active") == Some(true))
                })
                .take(k)
                .collect();
        }
        let q_tokens = tokens(query);
        if q_tokens.is_empty() {
            return Vec::new();
        }
        let wanted: HashSet<&str> = q_tokens.iter().map(String::as_str).collect();
        let mut n_docs = 0.0_f64;
        let mut df: HashMap<&str, f64> = HashMap::new();
        // (nom, tf des termes de la requête, norme du vecteur tf)
        let mut docs: Vec<(String, HashMap<String, f64>, f64)> = Vec::new();
        for tool in self.scan_tools() {
            if self.bool(tool, "_active") != Some(true) {
                continue;
            }
            n_docs += 1.0;
            let Some(text) = self.string(tool, "_text") else {
                continue;
            };
            let mut tf: BTreeMap<String, f64> = BTreeMap::new();
            for t in tokens(&text) {
                *tf.entry(t).or_insert(0.0) += 1.0;
            }
            let hits: HashMap<String, f64> = tf
                .iter()
                .filter(|(t, _)| wanted.contains(t.as_str()))
                .map(|(t, w)| (t.clone(), *w))
                .collect();
            if hits.is_empty() {
                continue;
            }
            for t in hits.keys() {
                if let Some(w) = wanted.get(t.as_str()) {
                    *df.entry(*w).or_insert(0.0) += 1.0;
                }
            }
            let norm = tf.values().map(|w| w * w).sum::<f64>().sqrt();
            if let Some(name) = self.string(tool, "_name") {
                docs.push((name, hits, norm));
            }
        }
        let idf = |t: &str| (1.0 + n_docs / df.get(t).copied().unwrap_or(1.0)).ln();
        let mut q: BTreeMap<&str, f64> = BTreeMap::new();
        for t in &q_tokens {
            *q.entry(t.as_str()).or_insert(0.0) += 1.0;
        }
        let q: Vec<(&str, f64)> = q
            .into_iter()
            .filter(|(t, _)| df.contains_key(t))
            .map(|(t, c)| (t, c * idf(t)))
            .collect();
        let q_norm = q.iter().map(|(_, w)| w * w).sum::<f64>().sqrt();
        if q_norm == 0.0 {
            return Vec::new();
        }
        let mut scored: Vec<(f64, String)> = docs
            .into_iter()
            .filter_map(|(name, hits, norm)| {
                let dot: f64 = q
                    .iter()
                    .filter_map(|(t, wq)| hits.get(*t).map(|tf| wq * tf))
                    .sum();
                let score = dot / (q_norm * norm);
                (score >= CANDIDATE_MIN).then_some((score, name))
            })
            .collect();
        scored.sort_by(|a, b| {
            b.0.partial_cmp(&a.0)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then(a.1.cmp(&b.1))
        });
        scored.into_iter().take(k).map(|(_, n)| n).collect()
    }

    /// [`ToolRepo::recent`], synchrone.
    pub fn recent_sync(&self, n: usize) -> Vec<(String, u64)> {
        if n == 0 {
            return Vec::new();
        }
        let mut all: Vec<(String, u64)> = Vec::new();
        for tool in self.scan_tools() {
            if self.bool(tool, "_active") != Some(true) {
                continue;
            }
            let (Some(name), Some(seq)) = (self.string(tool, "_name"), self.int(tool, "_seq"))
            else {
                continue;
            };
            all.push((name, u64::try_from(seq).unwrap_or(0)));
        }
        all.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        all.truncate(n);
        all
    }

    /// [`ToolRepo::tick`], synchrone : durable (un commit).
    pub fn tick_sync(&self) -> Result<u64, String> {
        let _guard = self.write.lock().unwrap_or_else(|e| e.into_inner());
        let graph = self.graph();
        let next = self.int(self.meta, "_seq").unwrap_or(0) + 1;
        graph.set_node_property(self.meta, "_seq", ObrainValue::from(next));
        if let Err(why) = self.commit() {
            // Non durable : on annule l'incrément en mémoire et on le dit.
            graph.set_node_property(self.meta, "_seq", ObrainValue::from(next - 1));
            return Err(format!("tick non durable : {why}"));
        }
        u64::try_from(next).map_err(|_| "horloge négative".to_string())
    }
}

/// Le port : une fine couche sur les méthodes `*_sync` (le stockage obrain est synchrone).
#[async_trait::async_trait(?Send)]
impl ToolRepo for ToolGraph {
    async fn get(&self, name: &str) -> Result<Option<ToolRecord>, String> {
        self.get_sync(name)
    }
    async fn put(&self, record: &ToolRecord) -> Result<(), String> {
        self.put_sync(record)
    }
    async fn len(&self) -> Result<usize, String> {
        Ok(self.len_sync())
    }
    async fn active_len(&self) -> Result<usize, String> {
        Ok(self.active_len_sync())
    }
    fn capacity(&self) -> usize {
        self.capacity
    }
    async fn active_with_steps(
        &self,
        steps_hash: &str,
        except: Option<&str>,
    ) -> Result<Option<String>, String> {
        Ok(self.active_with_steps_sync(steps_hash, except))
    }
    async fn candidates(&self, query: &str, k: usize) -> Result<Vec<String>, String> {
        Ok(self.candidates_sync(query, k))
    }
    async fn recent(&self, n: usize) -> Result<Vec<(String, u64)>, String> {
        Ok(self.recent_sync(n))
    }
    async fn tick(&self) -> Result<u64, String> {
        self.tick_sync()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use this_harness::steps_hash;

    fn dir() -> std::path::PathBuf {
        std::env::temp_dir().join(format!("this_tg_unit_{}", uuid::Uuid::new_v4()))
    }

    fn rec(name: &str, n: u32, seq: u64) -> ToolRecord {
        let steps = vec![RecipeStep {
            tool: "task_list".into(),
            args: json!({ "n": n }),
        }];
        ToolRecord {
            name: name.into(),
            versions: vec![ToolDef {
                version: 1,
                description: "d".into(),
                parameters: json!({"type":"object","properties":{},"required":[]}),
                steps_hash: steps_hash(&steps),
                steps,
                tags: vec![],
                note: "n".into(),
                seq,
                stats: ToolStats::default(),
            }],
            status: ToolStatus::Active,
            dropped: 0,
        }
    }

    #[test]
    fn un_commit_en_echec_laisse_l_etat_d_avant() {
        let d = dir();
        let store = ObrainStore::open(&d).unwrap();
        let g = ToolGraph::open(Arc::clone(&store)).unwrap();
        let a = rec("alpha_un", 1, 1);
        g.put_sync(&a).unwrap();
        g.fail_commit.set(true);

        // Remplacement raté : l'ancien enregistrement est intact, sans orphelin.
        let mut changed = a.clone();
        changed.versions[0].description = "autre".into();
        changed.versions[0].steps[0].tool = "autre_outil".into();
        assert!(g.put_sync(&changed).is_err());
        assert_eq!(g.get_sync("alpha_un").unwrap(), Some(a.clone()));
        assert!(
            g.find_labeled(HOST, "autre_outil").is_none(),
            "hôte orphelin"
        );
        assert!(g.find_labeled(HOST, "task_list").is_some());

        // Création ratée : rien n'existe.
        assert!(g.put_sync(&rec("bravo_deux", 2, 2)).is_err());
        assert!(g.get_sync("bravo_deux").unwrap().is_none());
        assert_eq!(g.len_sync(), 1);
        assert_eq!(g.graph().node_count_by_label(VERSION), 1);
        assert_eq!(g.graph().node_count_by_label(STEP), 1);
        drop(g);
        drop(store);
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn un_lot_est_un_commit_et_s_annule_en_bloc() {
        let d = dir();
        let store = ObrainStore::open(&d).unwrap();
        let g = ToolGraph::open(Arc::clone(&store)).unwrap();
        let a = rec("alpha_un", 1, 1);
        g.put_sync(&a).unwrap();
        let mut a2 = a.clone();
        a2.versions[0].description = "changé".into();
        let batch = [a2, rec("bravo_deux", 2, 2), rec("charlie_trois", 3, 3)];

        g.fail_commit.set(true);
        assert!(g.put_many_sync(&batch).is_err());
        assert_eq!(g.get_sync("alpha_un").unwrap(), Some(a));
        assert!(g.get_sync("bravo_deux").unwrap().is_none());
        assert_eq!(g.len_sync(), 1);

        g.fail_commit.set(false);
        g.put_many_sync(&batch).unwrap();
        assert_eq!(g.len_sync(), 3);
        assert_eq!(g.get_sync("alpha_un").unwrap().unwrap(), batch[0]);
        drop(g);
        drop(store);
        std::fs::remove_dir_all(&d).ok();
    }
}
