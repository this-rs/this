//! # La boîte à outils du catalogue
//!
//! [`CatalogToolbox`] enveloppe la boîte à outils de l'hôte et y ajoute ce qu'un agent a
//! construit — sans jamais lui donner un pouvoir que l'hôte ne lui avait pas donné.
//!
//! ## Économie de jetons : une surface minimale, le reste à la demande
//!
//! Annoncer tous les outils d'auteur à chaque tour, c'est payer leurs descriptions à chaque
//! tour. Ici `specs()` ajoute **deux** méta-outils (`tools_find`, `tools_author`) et, au
//! plus, [`DEFAULT_MAX_LOADED`] outils d'auteur **chargés** : une recherche charge les
//! meilleurs résultats, une création charge l'outil créé, un appel le rafraîchit. Le plus
//! ancien non utilisé sort. Même idée que `ToolSearch` côté orchestrateur.
//!
//! Le harnais refuse un outil non offert à ce tour (`session.rs`) : **chargé = appelable**,
//! et un outil qu'on vient de chercher est utilisable au tour SUIVANT, pas dans le même lot.
//!
//! ## Ce qu'un outil d'auteur ne peut pas
//!
//! - **Élargir un droit.** Chaque étape passe par la boîte de l'hôte (son gate, ses
//!   autorisations). L'effet DÉCLARÉ d'un outil d'auteur est le plus fort de ses étapes
//!   (déclaration absente = `Write`, jamais `Read`) et il n'est ACCORDÉ que si toutes ses
//!   étapes le sont.
//! - **Se composer lui-même.** Une étape ne vise ni `tools_*` ni `custom_*` : pas de
//!   récursion, pas d'outil qui s'écrit un outil.
//! - **Faire passer un texte d'un outil à l'autre.** Pas de `{{prev}}` (voir
//!   [`crate::catalogue`]).
//! - **Être atomique.** Une recette qui échoue à l'étape 3 a déjà agi aux étapes 1 et 2 :
//!   le résultat le dit.
//!
//! Les descriptions d'auteur sont des **données non fiables** : elles reviennent dans la
//! liste d'outils et dans les résultats enfermées dans un conteneur
//! ([`this_agent::untrusted`]). L'hôte doit donc ajouter
//! [`untrusted::PREAMBLE`](this_agent::untrusted::PREAMBLE) à sa consigne, comme pour MCP.
//!
//! ## Autorisations
//!
//! Si l'hôte a installé des autorisations statiques (`Harness::with_grants`), les noms des
//! outils d'auteur n'y sont pas — ils naissent pendant la session. Le gate est fail-closed :
//! sans plus, ils seraient refusés. L'hôte donne donc ses autorisations à la boîte
//! ([`CatalogToolbox::with_base_grants`]), qui les complète à chaque appel.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use async_trait::async_trait;
use serde_json::{Value, json};
use this_agent::cite::SeenLedger;
use this_agent::evidence::{Declaration, Effect};
use this_agent::substrate::{Grant, Grants, Verdict};
use this_agent::untrusted;

use crate::catalogue::{
    Catalog, CatalogError, RecipeStep, ToolChange, ToolDraft, ToolRecord, Usage, clip, render_args,
    validate_args,
};
use crate::event::Outcome;
use crate::ports::{Cancel, Toolbox};
use crate::session::out_hash;
use crate::wire::{ToolCall, ToolSpec};

/// Le méta-outil de lecture : chercher, montrer, historiser.
pub const FIND: &str = "tools_find";
/// Le méta-outil d'écriture : créer, éditer, revenir, retirer.
pub const AUTHOR: &str = "tools_author";
/// Le préfixe des outils d'auteur, côté modèle. RÉSERVÉ au catalogue : un hôte ne doit pas
/// offrir d'outil qui le porte.
///
/// Les noms n'ont ni `/` ni `.` : un moteur compatible OpenAI refuse tout nom hors de
/// `^[a-zA-Z0-9_-]+$` par une 400 (mesuré sur un moteur réel, `tests/catalogue_reel.rs`),
/// et un transport factice ne le voit pas.
pub const PREFIX: &str = "custom_";
/// Outils d'auteur annoncés au plus à la fois.
pub const DEFAULT_MAX_LOADED: usize = 8;
/// Résultats rendus par défaut par une recherche.
pub const DEFAULT_FIND_LIMIT: usize = 5;
/// Résultats rendus au plus.
pub const MAX_FIND_LIMIT: usize = 10;
/// Les meilleurs résultats qu'une recherche charge.
pub const AUTO_LOAD: usize = 3;
/// Plafond du texte rendu par un outil d'auteur.
pub const MAX_RESULT_CHARS: usize = 8_000;
/// Plafond du texte retenu par étape.
pub const STEP_TEXT_CHARS: usize = 1_500;
/// Plafond d'une description dans la liste d'outils.
const SPEC_DESCRIPTION_CHARS: usize = 300;
/// Noms rendus dans l'index du catalogue (`tools_find`) : de quoi savoir QU'IL EXISTE un outil
/// sans payer sa description.
pub const INDEX_NAMES: usize = 12;

/// L'état que les méthodes synchrones de la boîte lisent.
#[derive(Default)]
struct Cache {
    /// Noms chargés, du moins récemment utilisé au plus récent.
    loaded: Vec<String>,
    /// Les enregistrements des outils chargés.
    records: HashMap<String, ToolRecord>,
    /// L'index de `tools_find` : les noms les plus récents, et le nombre d'outils actifs.
    index: Vec<(String, u64)>,
    active: usize,
}

/// La boîte à outils de l'hôte, plus le catalogue de ce que l'agent a construit.
pub struct CatalogToolbox {
    inner: Rc<dyn Toolbox>,
    catalog: Catalog,
    /// Ce que les méthodes SYNCHRONES du `Toolbox` (`specs`, `declarations`, `grants`) lisent :
    /// le dépôt est asynchrone, une page wasm ne peut pas l'attendre ici. Le cache est rafraîchi
    /// à chaque opération asynchrone (chercher, créer, éditer, appeler).
    cache: RefCell<Cache>,
    max_loaded: usize,
    /// Un nonce par boîte, pas par tour : la liste d'outils est le PRÉFIXE du prompt, et un
    /// nonce neuf à chaque tour casserait le cache du fournisseur. Tiré au hasard à la
    /// construction, il ne se déduit d'aucun contenu d'auteur ; et `sanitize` neutralise de
    /// toute façon toute balise du corps.
    nonce: String,
    base_grants: Option<Grants>,
}

impl CatalogToolbox {
    /// `catalog` porte son dépôt : c'est lui qui range les outils (le graphe de l'hôte), la
    /// boîte n'a aucune persistance propre.
    pub fn new(inner: Rc<dyn Toolbox>, catalog: Catalog) -> Self {
        Self {
            inner,
            catalog,
            cache: RefCell::new(Cache::default()),
            max_loaded: DEFAULT_MAX_LOADED,
            nonce: untrusted::fresh_nonce(),
            base_grants: None,
        }
    }

    /// Comme [`CatalogToolbox::new`], puis lit l'index du catalogue : c'est lui que `specs()`
    /// annonce dès le premier tour. Avec `new` seul, l'index est vide jusqu'au premier
    /// [`CatalogToolbox::refresh`].
    pub async fn open(inner: Rc<dyn Toolbox>, catalog: Catalog) -> Self {
        let toolbox = Self::new(inner, catalog);
        toolbox.refresh().await;
        toolbox
    }

    /// Relit l'index du catalogue et les outils chargés (une autre session a pu les changer).
    pub async fn refresh(&self) {
        // Une lecture qui échoue laisse le cache tel qu'il est : mieux vaut annoncer l'état
        // d'avant que d'effacer des outils parce que le stockage a eu un hoquet.
        let (Ok(active), Ok(index)) = (
            self.catalog.active_len().await,
            self.catalog.recent(INDEX_NAMES).await,
        ) else {
            return;
        };
        let names = self.cache.borrow().loaded.clone();
        let mut fresh: Vec<(String, Option<ToolRecord>)> = Vec::new();
        for name in names {
            match self.catalog.get(&name).await {
                Ok(record) => fresh.push((name, record)),
                Err(_) => return,
            }
        }
        let mut cache = self.cache.borrow_mut();
        cache.active = active;
        cache.index = index;
        for (name, record) in fresh {
            match record.filter(ToolRecord::is_active) {
                Some(r) => {
                    cache.records.insert(name, r);
                }
                None => {
                    cache.records.remove(&name);
                    cache.loaded.retain(|n| n != &name);
                }
            }
        }
    }

    /// Change le plafond d'outils d'auteur annoncés (au moins 1).
    pub fn with_max_loaded(mut self, max: usize) -> Self {
        self.max_loaded = max.max(1);
        self
    }

    /// Les autorisations statiques de l'hôte, complétées à chaque appel par celles des outils
    /// d'auteur. Ignorées quand la boîte enveloppée tient déjà les siennes à jour.
    pub fn with_base_grants(mut self, grants: Grants) -> Self {
        self.base_grants = Some(grants);
        self
    }

    /// Le catalogue (un clone : il partage le même dépôt).
    pub fn catalog(&self) -> Catalog {
        self.catalog.clone()
    }

    /// Les noms actuellement annoncés (du moins récent au plus récent).
    pub fn loaded(&self) -> Vec<String> {
        self.cache.borrow().loaded.clone()
    }

    /// Charge `record` : il sera annoncé, donc appelable. Le moins récemment utilisé sort
    /// quand le plafond est atteint ; un outil retiré se décharge.
    fn remember(&self, record: ToolRecord) {
        let mut cache = self.cache.borrow_mut();
        let name = record.name.clone();
        cache.loaded.retain(|n| n != &name);
        if !record.is_active() {
            cache.records.remove(&name);
            return;
        }
        cache.records.insert(name.clone(), record);
        cache.loaded.push(name);
        while cache.loaded.len() > self.max_loaded {
            let out = cache.loaded.remove(0);
            cache.records.remove(&out);
        }
    }

    /// Charge l'outil `name` depuis le dépôt, s'il existe et est actif.
    async fn load(&self, name: &str) {
        if let Ok(Some(record)) = self.catalog.get(name).await {
            self.remember(record);
        }
    }

    /// Les enregistrements ACTIFS des outils chargés (au plus `max_loaded`), du moins récent au
    /// plus récent, tels que le cache les tient. Seuls ces outils peuvent atteindre le gate :
    /// le harnais refuse tout outil non offert à ce tour.
    fn loaded_records(&self) -> Vec<ToolRecord> {
        let cache = self.cache.borrow();
        cache
            .loaded
            .iter()
            .filter_map(|name| cache.records.get(name).cloned())
            .collect()
    }

    fn unload(&self, name: &str) {
        let mut cache = self.cache.borrow_mut();
        cache.loaded.retain(|n| n != name);
        cache.records.remove(name);
    }

    /// Le contrôle des étapes : un outil que l'hôte offre, et qui n'est pas du catalogue.
    fn step_check(&self) -> impl Fn(&str) -> Result<(), String> + use<> {
        let offered: HashSet<String> = self.inner.specs().into_iter().map(|s| s.name).collect();
        move |tool: &str| {
            if tool == FIND || tool == AUTHOR || tool.starts_with(PREFIX) {
                Err("un outil du catalogue ne se compose pas (pas de récursion)".to_string())
            } else if offered.contains(tool) {
                Ok(())
            } else {
                Err("l'hôte n'offre pas cet outil".to_string())
            }
        }
    }

    /// `tools_find`, avec l'INDEX du catalogue : les noms des outils les plus récemment
    /// créés ou modifiés, sans leur description.
    ///
    /// POURQUOI : mesuré sur un moteur réel (`tests/catalogue_reel.rs`), un modèle à qui rien
    /// ne dit qu'un outil existe ne le cherche pas, et refait à la main ce que l'outil fait.
    /// Le chargement paresseux économise la liste d'outils, mais il cache aussi le catalogue.
    /// L'index est trié par RÉCENCE DE VERSION, jamais par usage : il ne change que lorsque le
    /// catalogue change, pas à chaque appel — la liste d'outils est le préfixe du prompt, et
    /// un index qui bouge à chaque tour casserait le cache du fournisseur.
    fn find_spec(&self) -> ToolSpec {
        let mut description = String::from(
            "Cherche dans les outils déjà construits AVANT d'en créer un, ou quand une tâche \
             se répète. search{query} charge les meilleurs ; show{name,version?} montre la \
             recette ; history{name} les versions.",
        );
        let cache = self.cache.borrow();
        let total = cache.active;
        if total > 0 {
            let shown: Vec<&str> = cache.index.iter().map(|(n, _)| n.as_str()).collect();
            let more = if total > shown.len() {
                format!(" … et {} autres", total - shown.len())
            } else {
                String::new()
            };
            description.push_str(&format!(
                "\nCatalogue ({total}) : {}",
                untrusted::wrap_with(
                    &self.nonce,
                    400,
                    "catalogue",
                    &[("kind", "index")],
                    &format!("{}{more}", shown.join(", ")),
                )
            ));
        }
        ToolSpec {
            name: FIND.to_string(),
            description,
            parameters: json!({
                "type": "object",
                "properties": {
                    "action": {"type": "string", "enum": ["search", "show", "history"]},
                    "query": {"type": "string"},
                    "name": {"type": "string"},
                    "version": {"type": "integer"},
                    "limit": {"type": "integer"}
                },
                "required": ["action"]
            }),
        }
    }

    fn custom_spec(&self, record: &ToolRecord) -> ToolSpec {
        let def = record.current();
        let version = def.version.to_string();
        ToolSpec {
            name: format!("{PREFIX}{}", record.name),
            description: untrusted::wrap_with(
                &self.nonce,
                SPEC_DESCRIPTION_CHARS,
                "catalogue",
                &[("tool", &record.name), ("version", &version)],
                &def.description,
            ),
            parameters: def.parameters.clone(),
        }
    }

    /// L'empreinte de l'enregistrement `name` (ce que `Outcome::observed` compare pour dire
    /// « rien n'a changé »), ou « absent ».
    async fn fingerprint(&self, name: &str) -> String {
        match self.catalog.get(name).await {
            Ok(Some(r)) => out_hash(&serde_json::to_string(&r).unwrap_or_default()),
            Ok(None) => "absent".to_string(),
            Err(_) => "illisible".to_string(),
        }
    }

    /// Met un résultat du catalogue en `Outcome` quand il est refusé. L'atomicité est celle du
    /// dépôt : le catalogue calcule l'enregistrement entier puis l'écrit en un seul `put`. Un
    /// refus de règle est un `refused` ; un refus du STOCKAGE est un `failed` (ce n'est pas la
    /// faute du modèle) et rien n'a changé.
    fn refusal<T>(result: Result<T, CatalogError>) -> Result<T, Box<Outcome>> {
        result.map_err(|e| {
            Box::new(match e {
                CatalogError::Storage(_) => {
                    Outcome::failed(e.to_string(), format!("catalogue : {e}"))
                }
                _ => Outcome::refused(e.to_string(), e.code()),
            })
        })
    }

    /// L'enregistrement de `name`, si l'outil existe ET est actif.
    async fn active_record(&self, name: &str) -> Result<Option<ToolRecord>, CatalogError> {
        Ok(self.catalog.get(name).await?.filter(ToolRecord::is_active))
    }

    async fn reinforce(&self, name: &str, version: u32, usage: Usage) {
        // Au mieux : le renforcement est un avis (voir `Catalog::record_use`).
        self.catalog.record_use(name, version, usage).await;
    }

    // ----- tools_find -------------------------------------------------------------

    async fn find(&self, args: &Value) -> Outcome {
        match arg_str(args, "action") {
            Some("search") => self.find_search(args).await,
            Some("show") => self.find_show(args).await,
            Some("history") => self.find_history(args).await,
            _ => bad_args("action : search, show ou history"),
        }
    }

    async fn find_search(&self, args: &Value) -> Outcome {
        let Some(query) = arg_str(args, "query").filter(|q| !q.trim().is_empty()) else {
            return bad_args("search demande `query`");
        };
        let limit = arg_u32(args, "limit")
            .map_or(DEFAULT_FIND_LIMIT, |n| n as usize)
            .clamp(1, MAX_FIND_LIMIT);
        let hits = match Self::refusal(self.catalog.search(&clip(query, 512), limit).await) {
            Ok(hits) => hits,
            Err(outcome) => return *outcome,
        };
        let active = self.catalog.active_len().await.unwrap_or(0);
        if hits.is_empty() {
            return Outcome::ok(format!(
                "Aucun outil ne correspond ({active} actif(s) au catalogue). Essaie d'autres \
                 mots avant d'en créer un (tools_author action=create)."
            ));
        }
        let mut body = String::new();
        for hit in &hits {
            let usage = if hit.uses == 0 {
                "jamais appelé".to_string()
            } else {
                format!(
                    "{} appel(s), fiabilité {} %{}",
                    hit.uses,
                    (hit.reliability * 100.0).round(),
                    if hit.fragile { ", FRAGILE" } else { "" }
                )
            };
            body.push_str(&format!(
                "{PREFIX}{} v{} — {} [{usage}]\n",
                hit.name,
                hit.version,
                clip(&hit.description, 160)
            ));
        }
        let loaded_now = hits.len().min(AUTO_LOAD);
        for hit in hits.iter().take(AUTO_LOAD) {
            self.load(&hit.name).await;
        }
        Outcome::ok(format!(
            "{} outil(s) trouvé(s) ; les {loaded_now} premiers sont chargés, appelle-les par \
             leur nom au tour suivant.\n{}",
            hits.len(),
            untrusted::wrap("catalogue", &[("kind", "search")], body.trim_end())
        ))
    }

    async fn find_show(&self, args: &Value) -> Outcome {
        let Some(name) = arg_str(args, "name") else {
            return bad_args("show demande `name`");
        };
        let record = match Self::refusal(self.catalog.get(name).await) {
            Ok(Some(record)) => record,
            Ok(None) => {
                let e = CatalogError::Unknown(name.to_string());
                return Outcome::refused(e.to_string(), e.code());
            }
            Err(outcome) => return *outcome,
        };
        let wanted = arg_u32(args, "version").unwrap_or(record.current().version);
        let Some(def) = record.version(wanted) else {
            let e = CatalogError::UnknownVersion {
                name: name.to_string(),
                version: wanted,
            };
            return Outcome::refused(e.to_string(), e.code());
        };
        let active = record.is_active();
        let body = serde_json::to_string_pretty(&json!({
            "name": record.name,
            "version": def.version,
            "current_version": record.current().version,
            "active": active,
            "description": def.description,
            "parameters": def.parameters,
            "steps": def.steps,
            "tags": def.tags,
            "note": def.note,
            "stats": {
                "uses": def.stats.uses, "ok": def.stats.ok,
                "failed": def.stats.failed, "refused": def.stats.refused,
            },
        }))
        .unwrap_or_default();
        if active {
            self.remember(record.clone());
        }
        Outcome::ok(untrusted::wrap(
            "catalogue",
            &[("kind", "show"), ("tool", name)],
            &body,
        ))
    }

    async fn find_history(&self, args: &Value) -> Outcome {
        let Some(name) = arg_str(args, "name") else {
            return bad_args("history demande `name`");
        };
        let record = match Self::refusal(self.catalog.get(name).await) {
            Ok(Some(record)) => record,
            Ok(None) => {
                let e = CatalogError::Unknown(name.to_string());
                return Outcome::refused(e.to_string(), e.code());
            }
            Err(outcome) => return *outcome,
        };
        let mut body = String::new();
        for def in record.versions.iter().rev() {
            body.push_str(&format!(
                "v{} — {} [{} appel(s), {} réussi(s), {} échoué(s), {} refusé(s)]\n",
                def.version,
                clip(&def.note, 120),
                def.stats.uses,
                def.stats.ok,
                def.stats.failed,
                def.stats.refused
            ));
        }
        let header = format!(
            "{} : {} version(s) gardée(s){}{}",
            record.name,
            record.versions.len(),
            if record.dropped > 0 {
                format!(
                    " ({} plus ancienne(s) sortie(s) du plafond)",
                    record.dropped
                )
            } else {
                String::new()
            },
            if record.is_active() {
                ""
            } else {
                " — RETIRÉ"
            }
        );
        Outcome::ok(format!(
            "{header}\n{}",
            untrusted::wrap(
                "catalogue",
                &[("kind", "history"), ("tool", name)],
                body.trim_end()
            )
        ))
    }

    // ----- tools_author -----------------------------------------------------------

    async fn author(&self, args: &Value) -> Outcome {
        match arg_str(args, "action") {
            Some("create") => self.author_create(args).await,
            Some("edit") => self.author_edit(args).await,
            Some("rollback") => self.author_rollback(args).await,
            Some("retire") => self.author_retire(args).await,
            _ => bad_args("action : create, edit, rollback ou retire"),
        }
    }

    async fn author_create(&self, args: &Value) -> Outcome {
        let draft = match draft_from(args) {
            Ok(d) => d,
            Err(why) => return bad_args(&why),
        };
        let name = draft.name.trim().to_string();
        if self
            .inner
            .specs()
            .iter()
            .any(|s| s.name == format!("{PREFIX}{name}"))
        {
            return bad_args(&format!(
                "« {PREFIX}{name} » est déjà un outil de l'hôte : choisis un autre nom"
            ));
        }
        let check = self.step_check();
        let before = self.fingerprint(&name).await;
        match Self::refusal(self.catalog.create(draft, &check).await) {
            Ok(version) => {
                self.load(&name).await;
                self.refresh().await;
                let after = self.fingerprint(&name).await;
                Outcome::ok(format!(
                    "Créé {PREFIX}{name} v{version}. Il est chargé : appelle-le par son nom au \
                     tour suivant."
                ))
                .observed(before, after)
            }
            Err(outcome) => *outcome,
        }
    }

    async fn author_edit(&self, args: &Value) -> Outcome {
        let Some(name) = arg_str(args, "name") else {
            return bad_args("edit demande `name`");
        };
        let Some(expected) = arg_u32(args, "expected_version") else {
            return bad_args("edit demande `expected_version` (la version que tu as lue)");
        };
        let change = match change_from(args) {
            Ok(c) => c,
            Err(why) => return bad_args(&why),
        };
        let check = self.step_check();
        let before = self.fingerprint(name).await;
        match Self::refusal(self.catalog.edit(name, expected, change, &check).await) {
            Ok(version) => {
                self.load(name).await;
                self.refresh().await;
                let after = self.fingerprint(name).await;
                Outcome::ok(format!("{PREFIX}{name} est en v{version}.")).observed(before, after)
            }
            Err(outcome) => *outcome,
        }
    }

    async fn author_rollback(&self, args: &Value) -> Outcome {
        let Some(name) = arg_str(args, "name") else {
            return bad_args("rollback demande `name`");
        };
        let (Some(to), Some(expected)) = (
            arg_u32(args, "to_version"),
            arg_u32(args, "expected_version"),
        ) else {
            return bad_args("rollback demande `to_version` et `expected_version`");
        };
        let check = self.step_check();
        let before = self.fingerprint(name).await;
        match Self::refusal(self.catalog.rollback(name, to, expected, &check).await) {
            Ok(version) => {
                self.load(name).await;
                self.refresh().await;
                let after = self.fingerprint(name).await;
                Outcome::ok(format!(
                    "{PREFIX}{name} est en v{version} (copie de la v{to})."
                ))
                .observed(before, after)
            }
            Err(outcome) => *outcome,
        }
    }

    async fn author_retire(&self, args: &Value) -> Outcome {
        let Some(name) = arg_str(args, "name") else {
            return bad_args("retire demande `name`");
        };
        let Some(reason) = arg_str(args, "reason").filter(|r| !r.trim().is_empty()) else {
            return bad_args("retire demande `reason`");
        };
        let replaced_by = arg_str(args, "replaced_by");
        let before = self.fingerprint(name).await;
        match Self::refusal(self.catalog.retire(name, reason, replaced_by).await) {
            Ok(()) => {
                self.unload(name);
                self.refresh().await;
                let after = self.fingerprint(name).await;
                Outcome::ok(format!("{PREFIX}{name} est retiré.")).observed(before, after)
            }
            Err(outcome) => *outcome,
        }
    }

    // ----- l'exécution d'une recette ----------------------------------------------

    async fn run_recipe(
        &self,
        call: &ToolCall,
        name: &str,
        record: ToolRecord,
        cancel: &Cancel,
    ) -> Outcome {
        let def = record.current().clone();
        // Une erreur d'arguments est celle du MODÈLE : elle ne compte pas contre l'outil.
        if let Err(why) = validate_args(&def.parameters, &call.arguments) {
            return Outcome::refused(format!("arguments invalides : {why}"), why);
        }
        self.remember(record);
        let total = def.steps.len();
        let mut trace = String::new();
        let (mut before, mut after) = (String::new(), String::new());
        let mut read: Vec<String> = Vec::new();
        for (i, step) in def.steps.iter().enumerate() {
            let n = i + 1;
            if cancel.is_cancelled() {
                self.reinforce(name, def.version, Usage::Failed).await;
                return Outcome::failed(
                    format!("{trace}Annulé avant l'étape {n}/{total}."),
                    "annulé",
                );
            }
            let args = match render_args(&step.args, &call.arguments) {
                Ok(a) => a,
                Err(why) => {
                    self.reinforce(name, def.version, Usage::Failed).await;
                    return Outcome::failed(
                        format!("{trace}Étape {n}/{total} : {why}."),
                        format!("étape {n} : {why}"),
                    );
                }
            };
            let sub = ToolCall {
                id: format!("{}#{n}", call.id),
                name: step.tool.clone(),
                arguments: args,
            };
            let out = self.inner.call_with_cancel(&sub, cancel).await;
            trace.push_str(&format!("── étape {n}/{total} · {}\n", step.tool));
            trace.push_str(&clip(&out.text, STEP_TEXT_CHARS));
            trace.push('\n');
            let already = if n > 1 {
                format!(" Les étapes 1 à {} ont déjà agi.", n - 1)
            } else {
                String::new()
            };
            if let Some(motif) = &out.refused {
                self.reinforce(name, def.version, Usage::Refused).await;
                return Outcome::refused(
                    format!(
                        "{}Étape {n}/{total} « {} » refusée : {motif}. Les suivantes n'ont pas \
                         été exécutées.{already}",
                        clip(&trace, MAX_RESULT_CHARS),
                        step.tool
                    ),
                    format!("étape {n} « {} » refusée : {motif}", step.tool),
                );
            }
            if let Some(cause) = &out.failed {
                self.reinforce(name, def.version, Usage::Failed).await;
                return Outcome::failed(
                    format!(
                        "{}Étape {n}/{total} « {} » a échoué : {cause}. Les suivantes n'ont pas \
                         été exécutées.{already}",
                        clip(&trace, MAX_RESULT_CHARS),
                        step.tool
                    ),
                    format!("étape {n} « {} » : {cause}", step.tool),
                );
            }
            if before.is_empty() && !out.before.is_empty() {
                before = out.before.clone();
            }
            if !out.after.is_empty() {
                after = out.after.clone();
            }
            if let Some(observable) = &out.observable {
                read.push(observable.clone());
            }
        }
        self.reinforce(name, def.version, Usage::Ok).await;
        let mut result =
            Outcome::ok(clip(trace.trim_end(), MAX_RESULT_CHARS)).observed(before, after);
        if !read.is_empty() {
            result = result.reading(read.join("\n"));
        }
        result
    }
}

/// Du plus faible au plus fort : ce que l'effet le plus fort d'une recette doit couvrir.
fn strength(effect: Effect) -> u8 {
    match effect {
        Effect::Read => 0,
        Effect::Note => 1,
        Effect::Network => 2,
        Effect::Write => 3,
        Effect::Exec => 4,
    }
}

fn bad_args(why: &str) -> Outcome {
    Outcome::refused(format!("arguments refusés : {why}"), why.to_string())
}

fn arg_str<'a>(args: &'a Value, key: &str) -> Option<&'a str> {
    args.get(key).and_then(Value::as_str)
}

fn arg_u32(args: &Value, key: &str) -> Option<u32> {
    args.get(key)
        .and_then(Value::as_u64)
        .and_then(|n| u32::try_from(n).ok())
}

fn steps_from(value: &Value) -> Result<Vec<RecipeStep>, String> {
    serde_json::from_value(value.clone())
        .map_err(|_| "steps : une liste de {tool, args} est attendue".to_string())
}

fn tags_from(value: &Value) -> Result<Vec<String>, String> {
    let list = value
        .as_array()
        .ok_or_else(|| "tags : une liste de textes est attendue".to_string())?;
    list.iter()
        .map(|t| {
            t.as_str()
                .map(str::to_string)
                .ok_or_else(|| "tags : une liste de textes est attendue".to_string())
        })
        .collect()
}

fn draft_from(args: &Value) -> Result<ToolDraft, String> {
    Ok(ToolDraft {
        name: arg_str(args, "name")
            .ok_or("create demande `name`")?
            .to_string(),
        description: arg_str(args, "description")
            .ok_or("create demande `description`")?
            .to_string(),
        parameters: args
            .get("parameters")
            .cloned()
            .unwrap_or_else(|| json!({"type": "object", "properties": {}})),
        steps: steps_from(args.get("steps").ok_or("create demande `steps`")?)?,
        tags: args
            .get("tags")
            .map(tags_from)
            .transpose()?
            .unwrap_or_default(),
        confirm_distinct: args
            .get("confirm_distinct")
            .and_then(Value::as_bool)
            .unwrap_or(false),
    })
}

fn change_from(args: &Value) -> Result<ToolChange, String> {
    Ok(ToolChange {
        description: arg_str(args, "description").map(str::to_string),
        parameters: args.get("parameters").cloned(),
        steps: args.get("steps").map(steps_from).transpose()?,
        tags: args.get("tags").map(tags_from).transpose()?,
        note: arg_str(args, "note").unwrap_or_default().to_string(),
    })
}

fn author_spec() -> ToolSpec {
    ToolSpec {
        name: AUTHOR.to_string(),
        description: "Construit un outil réutilisable : au plus 8 appels à tes outils existants, \
                      arguments {{param}}. create | edit (expected_version) | rollback | retire. \
                      Cherche d'abord avec tools_find."
            .to_string(),
        parameters: json!({
            "type": "object",
            "properties": {
                "action": {"type": "string", "enum": ["create", "edit", "rollback", "retire"]},
                "name": {"type": "string"},
                "description": {"type": "string"},
                "parameters": {"type": "object"},
                "steps": {"type": "array"},
                "tags": {"type": "array"},
                "expected_version": {"type": "integer"},
                "to_version": {"type": "integer"},
                "note": {"type": "string"},
                "reason": {"type": "string"},
                "replaced_by": {"type": "string"},
                "confirm_distinct": {"type": "boolean"}
            },
            "required": ["action", "name"]
        }),
    }
}

#[async_trait(?Send)]
impl Toolbox for CatalogToolbox {
    fn specs(&self) -> Vec<ToolSpec> {
        let mut specs = self.inner.specs();
        specs.push(self.find_spec());
        specs.push(author_spec());
        for record in self.loaded_records() {
            specs.push(self.custom_spec(&record));
        }
        specs
    }

    fn declarations(&self) -> Vec<Declaration> {
        let mut declarations = self.inner.declarations();
        let known: HashMap<String, Effect> = declarations
            .iter()
            .map(|d| (d.tool.clone(), d.effect))
            .collect();
        declarations.push(Declaration::new(FIND, Effect::Read));
        // L'agent écrit pour lui-même : jamais une preuve d'action sur le monde.
        declarations.push(Declaration::new(AUTHOR, Effect::Note));
        // Seuls les outils CHARGÉS peuvent atteindre le gate : le harnais refuse tout outil non
        // offert à ce tour. Énumérer le catalogue entier serait O(N) à chaque décision.
        for record in self.loaded_records() {
            // Une étape sans déclaration vaut `Write` : une déclaration manquante ne vaut
            // pas un droit, et ne doit jamais faire passer une recette pour une lecture.
            let effect = record
                .current()
                .steps
                .iter()
                .map(|s| known.get(&s.tool).copied().unwrap_or(Effect::Write))
                .max_by_key(|e| strength(*e))
                .unwrap_or(Effect::Write);
            declarations.push(Declaration::new(format!("{PREFIX}{}", record.name), effect));
        }
        declarations
    }

    fn grants(&self) -> Option<Grants> {
        let base = self.inner.grants().or_else(|| self.base_grants.clone())?;
        let mut grants = base.clone();
        // Ce que l'hôte a écrit sur un méta-outil fait foi ; sinon l'activer vaut l'accorder.
        for meta in [FIND, AUTHOR] {
            if !base.grants.iter().any(|g| g.tool == meta) {
                grants.put(Grant::granted(meta, "catalogue"));
            }
        }
        for record in self.loaded_records() {
            let name = format!("{PREFIX}{}", record.name);
            let refused =
                record
                    .current()
                    .steps
                    .iter()
                    .find_map(|step| match base.allows(&step.tool) {
                        Verdict::Allow => None,
                        Verdict::Refuse(why) => Some((step.tool.clone(), why)),
                    });
            grants.put(match refused {
                None => Grant::granted(name, "catalogue"),
                Some((tool, why)) => Grant::refused(
                    name,
                    "catalogue",
                    format!("l'étape « {tool} » n'est pas accordée : {why}"),
                ),
            });
        }
        Some(grants)
    }

    fn vus(&self) -> Option<Rc<RefCell<SeenLedger>>> {
        self.inner.vus()
    }

    async fn call(&self, call: &ToolCall) -> Outcome {
        self.call_with_cancel(call, &Cancel::new()).await
    }

    async fn call_with_cancel(&self, call: &ToolCall, cancel: &Cancel) -> Outcome {
        match call.name.as_str() {
            FIND => self.find(&call.arguments).await,
            AUTHOR => self.author(&call.arguments).await,
            name => match name.strip_prefix(PREFIX) {
                Some(tool) => match Self::refusal(self.active_record(tool).await) {
                    Ok(Some(record)) => self.run_recipe(call, tool, record, cancel).await,
                    // Le préfixe est réservé : un `custom_…` qui n'est pas un outil actif n'est
                    // JAMAIS transmis à l'hôte (il le prendrait pour un de ses noms), sauf si
                    // l'hôte l'offre lui-même.
                    Ok(None) if !self.inner.specs().iter().any(|s| s.name == name) => {
                        let e = match self.catalog.get(tool).await {
                            Ok(Some(_)) => CatalogError::Retired(tool.to_string()),
                            _ => CatalogError::Unknown(tool.to_string()),
                        };
                        Outcome::refused(e.to_string(), e.code())
                    }
                    Ok(None) => self.inner.call_with_cancel(call, cancel).await,
                    // Le stockage n'a pas pu dire si l'outil existe : on ne devine pas.
                    Err(outcome) => *outcome,
                },
                None => self.inner.call_with_cancel(call, cancel).await,
            },
        }
    }
}
