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
    Catalog, CatalogError, RecipeStep, ToolChange, ToolDef, ToolDraft, ToolRecord, Usage, clip,
    render_args, validate_args,
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

/// Où l'hôte range le catalogue. Le crate ne fait aucune entrée/sortie.
///
/// `save` reçoit l'état ENTIER. Pour une création, une édition, un retour ou un retrait,
/// un échec **annule** l'opération (rien ne change en mémoire). Pour un simple compteur
/// d'usage, l'échec est ignoré : le renforcement est un avis, pas une vérité à protéger. Un
/// hôte qui écrit cher peut regrouper les appels.
pub trait CatalogStore {
    fn save(&self, json: &str) -> Result<(), String>;
}

/// La boîte à outils de l'hôte, plus le catalogue de ce que l'agent a construit.
pub struct CatalogToolbox {
    inner: Rc<dyn Toolbox>,
    catalog: RefCell<Catalog>,
    store: Option<Rc<dyn CatalogStore>>,
    /// Noms chargés, du moins récemment utilisé au plus récent.
    loaded: RefCell<Vec<String>>,
    max_loaded: usize,
    /// Un nonce par boîte, pas par tour : la liste d'outils est le PRÉFIXE du prompt, et un
    /// nonce neuf à chaque tour casserait le cache du fournisseur. Tiré au hasard à la
    /// construction, il ne se déduit d'aucun contenu d'auteur ; et `sanitize` neutralise de
    /// toute façon toute balise du corps.
    nonce: String,
    base_grants: Option<Grants>,
}

impl CatalogToolbox {
    pub fn new(
        inner: Rc<dyn Toolbox>,
        catalog: Catalog,
        store: Option<Rc<dyn CatalogStore>>,
    ) -> Self {
        Self {
            inner,
            catalog: RefCell::new(catalog),
            store,
            loaded: RefCell::new(Vec::new()),
            max_loaded: DEFAULT_MAX_LOADED,
            nonce: untrusted::fresh_nonce(),
            base_grants: None,
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

    /// Le catalogue entier, tel qu'il serait rangé.
    pub fn catalog_json(&self) -> String {
        self.catalog.borrow().to_json()
    }

    /// Une copie du catalogue.
    pub fn catalog(&self) -> Catalog {
        self.catalog.borrow().clone()
    }

    /// Les noms actuellement annoncés (du moins récent au plus récent).
    pub fn loaded(&self) -> Vec<String> {
        self.loaded.borrow().clone()
    }

    fn load(&self, name: &str) {
        let mut loaded = self.loaded.borrow_mut();
        loaded.retain(|n| n != name);
        loaded.push(name.to_string());
        while loaded.len() > self.max_loaded {
            loaded.remove(0);
        }
    }

    fn unload(&self, name: &str) {
        self.loaded.borrow_mut().retain(|n| n != name);
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
        let catalog = self.catalog.borrow();
        let mut names: Vec<(&str, u64)> = catalog
            .active()
            .map(|r| (r.name.as_str(), r.current().seq))
            .collect();
        if !names.is_empty() {
            names.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
            let total = names.len();
            let shown: Vec<&str> = names.iter().take(INDEX_NAMES).map(|(n, _)| *n).collect();
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

    /// Applique `op` sur une COPIE, la range, et ne la fait vivre qu'après. Rend la valeur et
    /// l'empreinte avant/après, ou le refus déjà mis en `Outcome`.
    fn mutate<T>(
        &self,
        op: impl FnOnce(&mut Catalog) -> Result<T, CatalogError>,
    ) -> Result<(T, String, String), Box<Outcome>> {
        let before = self.catalog.borrow().clone();
        let mut next = before.clone();
        let value = match op(&mut next) {
            Ok(v) => v,
            Err(e) => return Err(Box::new(Outcome::refused(e.to_string(), e.code()))),
        };
        let json = next.to_json();
        if let Some(store) = &self.store
            && let Err(why) = store.save(&json)
        {
            return Err(Box::new(Outcome::failed(
                format!("rien n'a changé : le catalogue n'a pas pu être enregistré ({why})"),
                format!("catalogue non enregistré : {why}"),
            )));
        }
        let fingerprints = (out_hash(&before.to_json()), out_hash(&json));
        *self.catalog.borrow_mut() = next;
        Ok((value, fingerprints.0, fingerprints.1))
    }

    /// La version courante de `name`, si l'outil existe ET est actif.
    fn active_def(&self, name: &str) -> Option<ToolDef> {
        self.catalog
            .borrow()
            .get(name)
            .filter(|r| r.is_active())
            .map(|r| r.current().clone())
    }

    fn reinforce(&self, name: &str, version: u32, usage: Usage) {
        let json = {
            let mut catalog = self.catalog.borrow_mut();
            catalog.record_use(name, version, usage);
            catalog.to_json()
        };
        if let Some(store) = &self.store {
            let _ = store.save(&json);
        }
    }

    // ----- tools_find -------------------------------------------------------------

    fn find(&self, args: &Value) -> Outcome {
        match arg_str(args, "action") {
            Some("search") => self.find_search(args),
            Some("show") => self.find_show(args),
            Some("history") => self.find_history(args),
            _ => bad_args("action : search, show ou history"),
        }
    }

    fn find_search(&self, args: &Value) -> Outcome {
        let Some(query) = arg_str(args, "query").filter(|q| !q.trim().is_empty()) else {
            return bad_args("search demande `query`");
        };
        let limit = arg_u32(args, "limit")
            .map_or(DEFAULT_FIND_LIMIT, |n| n as usize)
            .clamp(1, MAX_FIND_LIMIT);
        let (hits, active) = {
            let catalog = self.catalog.borrow();
            (
                catalog.search(&clip(query, 512), limit),
                catalog.active().count(),
            )
        };
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
            self.load(&hit.name);
        }
        Outcome::ok(format!(
            "{} outil(s) trouvé(s) ; les {loaded_now} premiers sont chargés, appelle-les par \
             leur nom au tour suivant.\n{}",
            hits.len(),
            untrusted::wrap("catalogue", &[("kind", "search")], body.trim_end())
        ))
    }

    fn find_show(&self, args: &Value) -> Outcome {
        let Some(name) = arg_str(args, "name") else {
            return bad_args("show demande `name`");
        };
        let catalog = self.catalog.borrow();
        let Some(record) = catalog.get(name) else {
            let e = CatalogError::Unknown(name.to_string());
            return Outcome::refused(e.to_string(), e.code());
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
        drop(catalog);
        if active {
            self.load(name);
        }
        Outcome::ok(untrusted::wrap(
            "catalogue",
            &[("kind", "show"), ("tool", name)],
            &body,
        ))
    }

    fn find_history(&self, args: &Value) -> Outcome {
        let Some(name) = arg_str(args, "name") else {
            return bad_args("history demande `name`");
        };
        let catalog = self.catalog.borrow();
        let Some(record) = catalog.get(name) else {
            let e = CatalogError::Unknown(name.to_string());
            return Outcome::refused(e.to_string(), e.code());
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

    fn author(&self, args: &Value) -> Outcome {
        match arg_str(args, "action") {
            Some("create") => self.author_create(args),
            Some("edit") => self.author_edit(args),
            Some("rollback") => self.author_rollback(args),
            Some("retire") => self.author_retire(args),
            _ => bad_args("action : create, edit, rollback ou retire"),
        }
    }

    fn author_create(&self, args: &Value) -> Outcome {
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
        match self.mutate(|c| c.create(draft, &check)) {
            Ok((version, before, after)) => {
                self.load(&name);
                Outcome::ok(format!(
                    "Créé {PREFIX}{name} v{version}. Il est chargé : appelle-le par son nom au \
                     tour suivant."
                ))
                .observed(before, after)
            }
            Err(outcome) => *outcome,
        }
    }

    fn author_edit(&self, args: &Value) -> Outcome {
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
        match self.mutate(|c| c.edit(name, expected, change, &check)) {
            Ok((version, before, after)) => {
                self.load(name);
                Outcome::ok(format!("{PREFIX}{name} est en v{version}.")).observed(before, after)
            }
            Err(outcome) => *outcome,
        }
    }

    fn author_rollback(&self, args: &Value) -> Outcome {
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
        match self.mutate(|c| c.rollback(name, to, expected, &check)) {
            Ok((version, before, after)) => {
                self.load(name);
                Outcome::ok(format!(
                    "{PREFIX}{name} est en v{version} (copie de la v{to})."
                ))
                .observed(before, after)
            }
            Err(outcome) => *outcome,
        }
    }

    fn author_retire(&self, args: &Value) -> Outcome {
        let Some(name) = arg_str(args, "name") else {
            return bad_args("retire demande `name`");
        };
        let Some(reason) = arg_str(args, "reason").filter(|r| !r.trim().is_empty()) else {
            return bad_args("retire demande `reason`");
        };
        let replaced_by = arg_str(args, "replaced_by");
        match self.mutate(|c| c.retire(name, reason, replaced_by)) {
            Ok(((), before, after)) => {
                self.unload(name);
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
        def: ToolDef,
        cancel: &Cancel,
    ) -> Outcome {
        // Une erreur d'arguments est celle du MODÈLE : elle ne compte pas contre l'outil.
        if let Err(why) = validate_args(&def.parameters, &call.arguments) {
            return Outcome::refused(format!("arguments invalides : {why}"), why);
        }
        self.load(name);
        let total = def.steps.len();
        let mut trace = String::new();
        let (mut before, mut after) = (String::new(), String::new());
        let mut read: Vec<String> = Vec::new();
        for (i, step) in def.steps.iter().enumerate() {
            let n = i + 1;
            if cancel.is_cancelled() {
                self.reinforce(name, def.version, Usage::Failed);
                return Outcome::failed(
                    format!("{trace}Annulé avant l'étape {n}/{total}."),
                    "annulé",
                );
            }
            let args = match render_args(&step.args, &call.arguments) {
                Ok(a) => a,
                Err(why) => {
                    self.reinforce(name, def.version, Usage::Failed);
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
                self.reinforce(name, def.version, Usage::Refused);
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
                self.reinforce(name, def.version, Usage::Failed);
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
        self.reinforce(name, def.version, Usage::Ok);
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
        let catalog = self.catalog.borrow();
        for name in self.loaded.borrow().iter() {
            if let Some(record) = catalog.get(name).filter(|r| r.is_active()) {
                specs.push(self.custom_spec(record));
            }
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
        for record in self.catalog.borrow().active() {
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
        for record in self.catalog.borrow().active() {
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
            FIND => self.find(&call.arguments),
            AUTHOR => self.author(&call.arguments),
            name => match name.strip_prefix(PREFIX) {
                Some(tool) => match self.active_def(tool) {
                    Some(def) => self.run_recipe(call, tool, def, cancel).await,
                    // Le préfixe est réservé : un `custom_…` qui n'est pas un outil actif n'est
                    // JAMAIS transmis à l'hôte (il le prendrait pour un de ses noms), sauf si
                    // l'hôte l'offre lui-même.
                    None if !self.inner.specs().iter().any(|s| s.name == name) => {
                        let e = match self.catalog.borrow().get(tool) {
                            Some(_) => CatalogError::Retired(tool.to_string()),
                            None => CatalogError::Unknown(tool.to_string()),
                        };
                        Outcome::refused(e.to_string(), e.code())
                    }
                    None => self.inner.call_with_cancel(call, cancel).await,
                },
                None => self.inner.call_with_cancel(call, cancel).await,
            },
        }
    }
}
