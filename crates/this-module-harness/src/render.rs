//! Le contrat d'appel de vue : comment un module rend un arbre, et ce que l'hôte refuse.
//!
//! Un module qui rend une interface n'est pas un module qui calcule une valeur : il
//! **reçoit des données** et rend un **arbre**. Ce fichier définit ce contrat une fois —
//! les exports attendus, la traversée de la mémoire, ce qui est refusé et quand — puis
//! l'implémente pour `wasmi`. Un autre moteur (une page, `WebAssembly`) implémente
//! [`crate::executor::Executor::render`] avec les mêmes règles : ce sont les [`Limits`]
//! passées à l'appel qui portent les bornes, pas le moteur.
//!
//! # Ce que le module doit exporter
//!
//! | export | signature | rôle |
//! |---|---|---|
//! | `memory` | `(memory (export "memory") 1)` | le seul canal : la requête entre, la réponse sort |
//! | `alloc` | `(func (export "alloc") (param i32) (result i32))` | réserve `n` octets dans sa mémoire et rend le décalage du tampon |
//! | `render` | `(func (export "render") (param i32 i32) (result i64))` | lit la requête en `ptr..ptr+len`, rend `((ptr << 32) | len)` de la réponse |
//!
//! Aucun import n'est admis. Un module de vue est une **fonction pure de ce qu'on lui
//! passe** : ce dont il a besoin (l'horloge, l'identité, un filtre) est dans la requête
//! JSON. Un module qui déclare un import est refusé **avant** d'être instancié, donc
//! avant la première instruction exécutée.
//!
//! # L'aller-retour, dans l'ordre
//!
//! 1. les imports déclarés sont refusés (`ImportsRefused`) ;
//! 2. les exports `memory`, `alloc`, `render` sont vérifiés, signature comprise — un
//!    export manquant est refusé avec le nom de l'export et la ligne à écrire
//!    (`MissingExport`), une signature différente avec l'attendu et le trouvé ;
//! 3. la mémoire **déclarée** est comparée au plafond : un module qui demande d'emblée
//!    plus que [`Limits::memory_pages`] est refusé sans être instancié (`Memory`) ;
//! 4. le fuel est posé **avant** l'instanciation (une section `start` qui boucle est
//!    arrêtée comme le reste), le `Store` porte le plafond mémoire, puis on instancie ;
//! 5. `alloc(len)` rend un décalage ; l'hôte y écrit la requête JSON en UTF-8
//!    (un décalage hors mémoire est refusé : `BadBuffer`) ;
//! 6. `render(ptr, len)` rend le couple `(ptr, len)` de la réponse, empaqueté
//!    `((ptr << 32) | len)` ; l'épuisement du fuel est rendu `OutOfFuel`, jamais un
//!    blocage de l'hôte ;
//! 7. la réponse est bornée par [`Limits::response_bytes`] (`ResponseTooLarge`) puis lue ;
//! 8. l'hôte valide l'arbre (voir [`views_of`] et [`issues_against`]) : un arbre qui ne
//!    respecte pas le contrat de vues est **refusé, jamais dessiné**.
//!
//! La requête est écrite telle quelle : c'est un JSON décidé par l'appelant. La réponse
//! est soit un objet de vue ([`ViewDesc`]), soit un tableau non vide d'objets de vue.
//!
//! # Un module hostile ne fige pas l'hôte
//!
//! Les trois bornes de [`Limits`] sont indépendantes et chacune suffit :
//!
//! - le **fuel** est fixé avant chaque appel : une boucle infinie s'arrête avec un
//!   `OutOfFuel` (mesuré) ;
//! - la **mémoire** est plafonnée par le limiteur du `Store` : un `memory.grow` au-delà
//!   échoue, et le dépassement devient une erreur du harness, pas une allocation de
//!   l'hôte ;
//! - la **taille** de la requête et de la réponse est comparée avant toute lecture.
//!
//! Aucun de ces chemins ne panique et aucun n'attend : chaque refus est une valeur.
//!
//! # Ce que l'appel ne fait pas
//!
//! Il ne dessine rien, ne connaît aucune entité et n'invente aucune vue. Il rend des
//! [`ViewDesc`] validés ; c'est à l'hôte de les peindre (ou de les refuser).
//!
//! ```no_run
//! # async fn demo<V: this_vfs::Vfs, E: this_module_harness::render::ViewRenderer>(
//! #     harness: &this_module_harness::Harness<'_, V>, engine: &E,
//! # ) -> Result<(), Box<dyn std::error::Error>> {
//! use this_module_harness::render::Limits;
//!
//! let request = serde_json::json!({ "view": "orders" });
//! let rendered = harness
//!     .render(engine, "shop", "0.3.0", &request, &Limits::default())
//!     .await?;
//! println!("{} vue(s) en {} unités de fuel", rendered.views.len(), rendered.fuel_used);
//! # Ok(()) }
//! ```

pub use crate::call::{ALLOC_EXPORT, Call, Limits, MEMORY_EXPORT, PAGE_BYTES};
use crate::executor::ExecutorError;
use this_descriptor::views::ViewDesc;
use this_descriptor::{ModuleDescriptor, views};

/// Un moteur qui sait tenir le contrat de vue.
///
/// Trait distinct de [`Executor`](crate::executor::Executor) à dessein : rendre une
/// valeur et rendre un **arbre borné** sont deux capacités, et un moteur peut avoir la
/// première sans la seconde. Un hôte qui ne sait pas tenir les [`Limits`] ne doit pas
/// prétendre rendre : il n'implémente pas ce trait, et l'absence d'implémentation est
/// vérifiée à la compilation — pas découverte à l'exécution sous la forme d'une
/// interface vide que quelqu'un aurait dessinée.
///
/// Le trait suit la même règle que `Executor` : `Send` sur l'hôte, `?Send` en wasm32
/// (les futures d'un moteur navigateur ne sont pas `Send`).
#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
pub trait ViewRenderer {
    /// Une requête JSON entre, un arbre JSON sort — ou un refus typé.
    async fn render(
        &self,
        program: &[u8],
        request: &[u8],
        limits: &Limits,
    ) -> Result<Call, ExecutorError>;
}

/// Lit la requête, rend `((ptr << 32) | len)` de la réponse.
pub const RENDER_EXPORT: &str = "render";

/// Un arbre rendu, validé côté hôte.
#[derive(Clone, Debug, PartialEq)]
pub struct Rendered {
    /// Au moins une vue, chacune conforme au contrat de vues.
    pub views: Vec<ViewDesc>,
    pub fuel_used: u64,
    /// Taille de la réponse JSON, en octets.
    pub bytes: usize,
}

/// Ce qui empêche une réponse d'être un arbre de vues.
#[derive(Debug, thiserror::Error)]
pub enum ViewsError {
    #[error("réponse de {len} octets, plafond {max} : refusée avant lecture")]
    TooLarge { len: usize, max: usize },
    #[error(
        "le module n'a rien rendu (0 octet) : `render` doit rendre `(ptr << 32) | len` d'un arbre JSON"
    )]
    Empty,
    #[error("la réponse n'est pas du JSON : {0}")]
    NotJson(String),
    #[error(
        "la réponse est {got} JSON : le contrat attend un objet de vue ou un tableau non vide de vues"
    )]
    NotAView { got: &'static str },
    #[error("l'arbre rendu ne respecte pas le contrat de vues : {0}")]
    Invalid(String),
}

/// Interprète la réponse d'un module comme un arbre de vues.
///
/// Le vocabulaire de `this-descriptor` est fermé (`deny_unknown_fields` partout) : une
/// brique, une action, un clic inconnus sont refusés ici, par la même désérialisation
/// qui sert au reste du framework. On ne réécrit donc pas le contrat, on l'applique.
pub fn views_of(bytes: &[u8], max: usize) -> Result<Vec<ViewDesc>, ViewsError> {
    if bytes.len() > max {
        return Err(ViewsError::TooLarge {
            len: bytes.len(),
            max,
        });
    }
    if bytes.is_empty() {
        return Err(ViewsError::Empty);
    }
    let text = std::str::from_utf8(bytes).map_err(|e| ViewsError::NotJson(e.to_string()))?;
    let value: serde_json::Value =
        serde_json::from_str(text).map_err(|e| ViewsError::NotJson(e.to_string()))?;
    match value {
        serde_json::Value::Object(_) => {
            let view: ViewDesc = serde_json::from_value(value)
                .map_err(|e| ViewsError::Invalid(format!("vue : {e}")))?;
            Ok(vec![view])
        }
        serde_json::Value::Array(items) => {
            if items.is_empty() {
                return Err(ViewsError::NotAView {
                    got: "un tableau vide",
                });
            }
            let mut out = Vec::with_capacity(items.len());
            for (index, item) in items.into_iter().enumerate() {
                let view: ViewDesc = serde_json::from_value(item)
                    .map_err(|e| ViewsError::Invalid(format!("vue {index} : {e}")))?;
                out.push(view);
            }
            Ok(out)
        }
        other => Err(ViewsError::NotAView {
            got: json_kind(&other),
        }),
    }
}

/// Confronte les vues rendues au descripteur du module, avec la validation de
/// `this-descriptor` ([`views::issues`]) : entités déclarées, actions déclarées, champs
/// connus, place dans l'emplacement. On ne réimplémente aucun de ces contrôles ici.
///
/// Les vues rendues **remplacent** celles du descripteur portant le même nom ; celles
/// que le module déclare et que l'appel ne rend pas restent, pour que `replaces` et les
/// références entre vues gardent un sens.
pub fn issues_against(descriptor: &ModuleDescriptor, rendered: &[ViewDesc]) -> Vec<String> {
    if rendered.is_empty() {
        return vec!["aucune vue rendue : l'appel doit rendre au moins un arbre".to_string()];
    }
    let replaced: std::collections::BTreeSet<&str> =
        rendered.iter().map(|v| v.name.as_str()).collect();
    let mut module = descriptor.clone();
    module.views.retain(|v| !replaced.contains(v.name.as_str()));
    module.views.extend(rendered.iter().cloned());

    views::issues(&module)
        .into_iter()
        .map(|(reason, path, message)| format!("{path} [{reason}] : {message}"))
        .collect()
}

fn json_kind(value: &serde_json::Value) -> &'static str {
    match value {
        serde_json::Value::Null => "null",
        serde_json::Value::Bool(_) => "un booléen",
        serde_json::Value::Number(_) => "un nombre",
        serde_json::Value::String(_) => "une chaîne",
        serde_json::Value::Array(_) => "un tableau",
        serde_json::Value::Object(_) => "un objet",
    }
}

#[cfg(feature = "wasmi")]
pub(crate) fn call_bounded(
    program: &[u8],
    request: &[u8],
    limits: &Limits,
) -> Result<Call, ExecutorError> {
    crate::call::call_json(
        program,
        RENDER_EXPORT,
        &crate::call::Args::one(request),
        limits,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const VIEW: &str = r#"{"name":"orders","surface":"main","root":{"type":"leaf","brick":{"type":"list","entity":"order","label":"id"}}}"#;

    #[test]
    fn a_view_object_is_one_view() {
        let views = views_of(VIEW.as_bytes(), 4096).expect("un objet de vue est un arbre");
        assert_eq!(views.len(), 1);
        assert_eq!(views[0].name, "orders");
    }

    #[test]
    fn an_array_of_views_is_an_interface() {
        let two = format!("[{VIEW},{VIEW}]");
        let views = views_of(two.as_bytes(), 4096).expect("un tableau de vues est une interface");
        assert_eq!(views.len(), 2);
    }

    #[test]
    fn an_unknown_brick_is_refused_by_the_host() {
        let alien = r#"{"name":"orders","root":{"type":"leaf","brick":{"type":"carousel"}}}"#;
        match views_of(alien.as_bytes(), 4096) {
            Err(ViewsError::Invalid(message)) => {
                assert!(
                    message.contains("carousel"),
                    "le refus doit nommer la brique inconnue, reçu : {message}"
                );
            }
            other => panic!("une brique inconnue doit être refusée, reçu {other:?}"),
        }
    }

    #[test]
    fn something_that_is_not_a_view_is_refused() {
        match views_of(b"[1,2,3]", 4096) {
            Err(ViewsError::Invalid(_)) => {}
            other => panic!("un tableau de nombres n'est pas une vue, reçu {other:?}"),
        }
        match views_of(b"\"bonjour\"", 4096) {
            Err(ViewsError::NotAView { .. }) => {}
            other => panic!("une chaîne n'est pas une vue, reçu {other:?}"),
        }
        match views_of(b"[]", 4096) {
            Err(ViewsError::NotAView { .. }) => {}
            other => panic!("un tableau vide n'est pas une interface, reçu {other:?}"),
        }
        match views_of(b"", 4096) {
            Err(ViewsError::Empty) => {}
            other => panic!("une réponse vide doit être refusée, reçu {other:?}"),
        }
    }

    #[test]
    fn a_response_bigger_than_the_cap_is_refused_before_being_parsed() {
        match views_of(VIEW.as_bytes(), 8) {
            Err(ViewsError::TooLarge { len, max }) => {
                assert_eq!((len, max), (VIEW.len(), 8));
            }
            other => panic!("une réponse hors plafond doit être refusée, reçu {other:?}"),
        }
    }

    #[test]
    fn a_tree_naming_an_undeclared_entity_contradicts_the_descriptor() {
        let views = views_of(VIEW.as_bytes(), 4096).expect("arbre valide");
        let descriptor = ModuleDescriptor::new("shop", "1.0.0");
        let issues = issues_against(&descriptor, &views);
        assert!(
            issues
                .iter()
                .any(|issue| issue.contains("order") && issue.contains("declared entity")),
            "l'entité `order` n'est pas déclarée : {issues:?}"
        );
    }

    #[test]
    fn a_tree_that_matches_the_descriptor_has_no_issue() {
        let views = views_of(VIEW.as_bytes(), 4096).expect("arbre valide");
        let mut descriptor = ModuleDescriptor::new("shop", "1.0.0");
        descriptor
            .entities
            .push(this_descriptor::EntityDesc::new("order"));
        assert_eq!(
            issues_against(&descriptor, &views),
            Vec::<String>::new(),
            "une vue qui ne lit que des entités déclarées est conforme"
        );
    }
}

#[cfg(all(test, feature = "wasmi"))]
mod engine_tests {
    use super::*;
    use crate::Harness;
    use crate::executor::{ExecutorError, WasmiExecutor};
    use this_module_lang::assemble::assemble;
    use this_vfs::{InMemoryVfs, Precondition, Vfs, VfsPath};

    const VIEW: &str = r#"{"name":"orders","surface":"main","root":{"type":"leaf","brick":{"type":"list","entity":"order","label":"id"}}}"#;

    /// Un octet, écrit en échappement hexadécimal : un JSON entre dans un segment de
    /// données sans une seule citation à échapper.
    fn escaped(bytes: &[u8]) -> String {
        bytes.iter().map(|byte| format!("\\{byte:02x}")).collect()
    }

    /// Un module conforme : `alloc` rend des tampons à partir de 32768, `render` rend
    /// toujours les mêmes `response` octets, posés à l'adresse 16.
    fn render_module(response: &[u8]) -> Vec<u8> {
        let wat = format!(
            r#"(module
  (memory (export "memory") 1)
  (global $next (mut i32) (i32.const 32768))
  (data (i32.const 16) "{data}")
  (func (export "alloc") (param $len i32) (result i32)
    (local $ptr i32)
    (local.set $ptr (global.get $next))
    (global.set $next (i32.add (global.get $next) (local.get $len)))
    (local.get $ptr))
  (func (export "render") (param $ptr i32) (param $len i32) (result i64)
    (i64.or (i64.shl (i64.const 16) (i64.const 32)) (i64.const {len}))))"#,
            data = escaped(response),
            len = response.len()
        );
        assemble(&wat).expect("le WAT de test s'assemble")
    }

    /// Un module dont `render` boucle sans fin.
    fn looping_module() -> Vec<u8> {
        let wat = r#"(module
  (memory (export "memory") 1)
  (func (export "alloc") (param $len i32) (result i32) (i32.const 16))
  (func (export "render") (param $ptr i32) (param $len i32) (result i64)
    (loop $forever (br $forever))
    (i64.const 0)))"#;
        assemble(wat).expect("le WAT de test s'assemble")
    }

    /// Un module dont la section `start` boucle : il boucle avant même `render`.
    fn looping_start_module() -> Vec<u8> {
        let wat = r#"(module
  (memory (export "memory") 1)
  (func $start (loop $forever (br $forever)))
  (start $start)
  (func (export "alloc") (param $len i32) (result i32) (i32.const 16))
  (func (export "render") (param $ptr i32) (param $len i32) (result i64) (i64.const 0)))"#;
        assemble(wat).expect("le WAT de test s'assemble")
    }

    /// Un module qui demande 1024 pages (64 Mio) alors que le plafond est de 16 Mio.
    fn greedy_module() -> Vec<u8> {
        let wat = r#"(module
  (memory (export "memory") 1024)
  (func (export "alloc") (param $len i32) (result i32) (i32.const 16))
  (func (export "render") (param $ptr i32) (param $len i32) (result i64) (i64.const 0)))"#;
        assemble(wat).expect("le WAT de test s'assemble")
    }

    /// Un module qui essaie d'agrandir sa mémoire pendant l'appel.
    fn growing_module() -> Vec<u8> {
        let wat = r#"(module
  (memory (export "memory") 1)
  (func (export "alloc") (param $len i32) (result i32) (i32.const 16))
  (func (export "render") (param $ptr i32) (param $len i32) (result i64)
    (drop (memory.grow (i32.const 1000)))
    (i64.const 0)))"#;
        assemble(wat).expect("le WAT de test s'assemble")
    }

    /// Un module conforme, mais dont `render` n'a pas la signature du contrat.
    fn wrong_signature_module() -> Vec<u8> {
        let wat = r#"(module
  (memory (export "memory") 1)
  (func (export "alloc") (param $len i32) (result i32) (i32.const 16))
  (func (export "render") (param $ptr i32) (result i64) (i64.const 0)))"#;
        assemble(wat).expect("le WAT de test s'assemble")
    }

    /// Un module qui importe une primitive de l'hôte.
    fn importing_module() -> Vec<u8> {
        let wat = r#"(module
  (import "host" "emit" (func $emit (param i32 i32)))
  (memory (export "memory") 1)
  (func (export "alloc") (param $len i32) (result i32) (i32.const 16))
  (func (export "render") (param $ptr i32) (param $len i32) (result i64) (i64.const 0)))"#;
        assemble(wat).expect("le WAT de test s'assemble")
    }

    #[test]
    fn the_default_budget_stops_a_loop_too() {
        let started = std::time::Instant::now();
        let outcome = crate::render::call_bounded(&looping_module(), b"{}", &Limits::default());
        let elapsed = started.elapsed();
        match outcome {
            Err(ExecutorError::OutOfFuel { fuel }) => assert_eq!(fuel, Limits::default().fuel),
            other => panic!("les bornes par défaut doivent arrêter une boucle, reçu {other:?}"),
        }
        eprintln!("boucle arrêtée par le budget par défaut en {elapsed:?}");
        assert!(
            elapsed < std::time::Duration::from_secs(5),
            "une boucle arrêtée doit rendre la main en quelques dizaines de ms, mesuré {elapsed:?}"
        );
    }

    #[test]
    fn a_signature_outside_the_contract_is_refused_with_what_was_expected() {
        match crate::render::call_bounded(&wrong_signature_module(), b"{}", &Limits::default()) {
            Err(ExecutorError::WrongSignature {
                name,
                expected,
                found,
            }) => {
                assert_eq!(name, RENDER_EXPORT);
                assert!(expected.contains("param i32 i32"), "attendu : {expected}");
                assert!(
                    found.contains("(param i32) (result i64)"),
                    "le refus doit montrer la signature trouvée, reçu : {found}"
                );
            }
            other => panic!("une signature hors contrat doit être refusée, reçu {other:?}"),
        }
    }

    #[test]
    fn a_request_bigger_than_the_cap_is_refused_before_the_module_runs() {
        let limits = Limits {
            request_bytes: 1,
            ..Limits::default()
        };
        match crate::render::call_bounded(&render_module(VIEW.as_bytes()), b"{}", &limits) {
            Err(ExecutorError::RequestTooLarge { len, max }) => {
                assert_eq!((len, max), (2, 1));
            }
            other => panic!("une requête hors plafond doit être refusée, reçu {other:?}"),
        }
    }

    /// Un module conforme, mais dont `render` rend un décalage hors mémoire.
    fn lying_module() -> Vec<u8> {
        let wat = r#"(module
  (memory (export "memory") 1)
  (func (export "alloc") (param $len i32) (result i32) (i32.const 16))
  (func (export "render") (param $ptr i32) (param $len i32) (result i64)
    (i64.or (i64.shl (i64.const 65000) (i64.const 32)) (i64.const 4000))))"#;
        assemble(wat).expect("le WAT de test s'assemble")
    }

    /// Range un module wasm déjà assemblé là où le layout l'attend, manifeste compris :
    /// le même chemin que `Harness::compile`, sans compilateur.
    async fn store(vfs: &InMemoryVfs, name: &str, version: &str, wasm: &[u8]) {
        let manifest = this_vfs::layout::Manifest {
            format: this_vfs::layout::MANIFEST_FORMAT,
            name: name.to_string(),
            version: version.to_string(),
            files: [(crate::PROGRAM_FILE.to_string(), this_vfs::etag_of(wasm))]
                .into_iter()
                .collect(),
            created: chrono::Utc::now(),
            signature: None,
        };
        let encoded = serde_json::to_vec_pretty(&manifest).expect("manifeste");
        let dir = format!("/modules/{name}/{version}");
        for (file, bytes) in [
            (crate::PROGRAM_FILE, wasm),
            (this_vfs::layout::MANIFEST_FILE, encoded.as_slice()),
        ] {
            let path = VfsPath::parse(&format!("{dir}/{file}")).expect("chemin");
            vfs.write(&path, bytes, Precondition::None)
                .await
                .expect("écriture");
        }
    }

    #[tokio::test]
    async fn a_small_module_renders_a_tree_and_reports_its_cost() {
        let vfs = InMemoryVfs::default();
        let harness = Harness::new(&vfs);
        store(&vfs, "shop", "0.3.0", &render_module(VIEW.as_bytes())).await;

        let rendered = harness
            .render(
                &WasmiExecutor,
                "shop",
                "0.3.0",
                &serde_json::json!({ "view": "orders" }),
                &Limits::default(),
            )
            .await
            .expect("un module conforme rend son arbre");

        assert_eq!(rendered.views.len(), 1);
        assert_eq!(rendered.views[0].name, "orders");
        assert_eq!(rendered.bytes, VIEW.len());
        assert!(
            rendered.fuel_used > 0,
            "le coût de l'appel est mesuré, pas supposé"
        );
        assert!(
            rendered.fuel_used < Limits::default().fuel,
            "un petit module ne doit pas frôler le plafond : {} unités",
            rendered.fuel_used
        );
    }

    #[tokio::test]
    async fn an_infinite_loop_is_stopped_by_the_fuel() {
        let limits = Limits {
            fuel: 100_000,
            ..Limits::default()
        };
        match crate::render::call_bounded(&looping_module(), b"{}", &limits) {
            Err(ExecutorError::OutOfFuel { fuel }) => assert_eq!(fuel, 100_000),
            other => panic!("une boucle infinie doit être arrêtée par le fuel, reçu {other:?}"),
        }
    }

    #[tokio::test]
    async fn a_start_section_that_loops_does_not_freeze_the_host() {
        let limits = Limits {
            fuel: 100_000,
            ..Limits::default()
        };
        match crate::render::call_bounded(&looping_start_module(), b"", &limits) {
            Err(ExecutorError::OutOfFuel { .. }) => {}
            other => panic!("le fuel doit être posé avant l'instanciation, reçu {other:?}"),
        }
    }

    #[test]
    fn a_memory_growth_past_the_cap_is_refused() {
        match crate::render::call_bounded(&growing_module(), b"{}", &Limits::default()) {
            Err(ExecutorError::Memory { limit_bytes, .. }) => {
                assert_eq!(limit_bytes, Limits::default().memory_bytes());
            }
            other => panic!("un dépassement de mémoire doit être refusé, reçu {other:?}"),
        }
    }

    #[test]
    fn a_module_declaring_too_much_memory_is_refused_before_running() {
        match crate::render::call_bounded(&greedy_module(), b"{}", &Limits::default()) {
            Err(ExecutorError::Memory { message, .. }) => {
                assert!(
                    message.contains("1024"),
                    "le refus doit dire ce que le module demande, reçu : {message}"
                );
            }
            other => panic!("une mémoire déclarée hors plafond doit être refusée, reçu {other:?}"),
        }
    }

    #[test]
    fn an_undeclared_import_is_refused_before_execution() {
        match crate::render::call_bounded(&importing_module(), b"{}", &Limits::default()) {
            Err(ExecutorError::ImportsRefused { count, found }) => {
                assert_eq!(count, 1);
                assert_eq!(found, "host.emit");
            }
            other => panic!("un import doit être refusé avant instanciation, reçu {other:?}"),
        }
    }

    #[tokio::test]
    async fn a_module_without_the_entry_is_refused_with_a_way_to_declare_it() {
        // Un module du langage de module : il exporte `run`, pas le contrat de vue.
        let program = this_module_lang::assemble::compile_to_wasm("fn run() -> Int { 1 }")
            .expect("assemblage");
        let error = match crate::render::call_bounded(&program, b"{}", &Limits::default()) {
            Err(error) => error,
            Ok(call) => panic!("un module hors contrat doit être refusé, reçu {call:?}"),
        };
        let ExecutorError::MissingExport { name, how } = &error else {
            panic!("attendu un export manquant, reçu {error:?}");
        };
        assert_eq!(name, MEMORY_EXPORT);
        assert!(
            how.contains("(memory (export \"memory\")"),
            "le refus doit dire quelle ligne écrire, reçu : {how}"
        );
        let message = error.to_string();
        assert!(
            message.contains("memory") && message.contains("export"),
            "le refus doit nommer l'export manquant, reçu : {message}"
        );
    }

    #[test]
    fn a_buffer_out_of_memory_is_refused_rather_than_read() {
        match crate::render::call_bounded(&lying_module(), b"{}", &Limits::default()) {
            Err(ExecutorError::BadBuffer { ptr, len, size, .. }) => {
                assert_eq!(ptr, 65_000);
                assert_eq!(len, 4_000);
                assert_eq!(size, 65_536);
            }
            other => panic!("un décalage hors mémoire doit être refusé, reçu {other:?}"),
        }
    }

    #[tokio::test]
    async fn a_tree_outside_the_contract_is_refused_by_the_host_not_drawn() {
        let vfs = InMemoryVfs::default();
        let harness = Harness::new(&vfs);
        let alien = r#"{"name":"orders","root":{"type":"leaf","brick":{"type":"carousel"}}}"#;
        store(&vfs, "shop", "0.3.0", &render_module(alien.as_bytes())).await;

        match harness
            .render(
                &WasmiExecutor,
                "shop",
                "0.3.0",
                &serde_json::json!({}),
                &Limits::default(),
            )
            .await
        {
            Err(crate::Error::InvalidViews(message)) => assert!(
                message.contains("carousel"),
                "le refus doit nommer la brique inconnue, reçu : {message}"
            ),
            other => panic!("un arbre invalide ne doit pas sortir du harness, reçu {other:?}"),
        }
    }

    #[tokio::test]
    async fn a_tree_contradicting_the_descriptor_is_refused() {
        let vfs = InMemoryVfs::default();
        let harness = Harness::new(&vfs);
        store(&vfs, "shop", "0.3.0", &render_module(VIEW.as_bytes())).await;
        let descriptor = this_descriptor::ModuleDescriptor::new("shop", "0.3.0");

        match harness
            .render_against(
                &WasmiExecutor,
                "shop",
                "0.3.0",
                &serde_json::json!({ "view": "orders" }),
                &Limits::default(),
                &descriptor,
            )
            .await
        {
            Err(crate::Error::ViewsRejected { issues }) => assert!(
                issues.iter().any(|issue| issue.contains("order")),
                "le refus doit citer l'entité inconnue, reçu : {issues:?}"
            ),
            other => {
                panic!("un arbre qui contredit le descripteur doit être refusé, reçu {other:?}")
            }
        }
    }

    #[tokio::test]
    async fn a_conforming_tree_is_accepted_against_its_descriptor() {
        let vfs = InMemoryVfs::default();
        let harness = Harness::new(&vfs);
        store(&vfs, "shop", "0.3.0", &render_module(VIEW.as_bytes())).await;
        let mut descriptor = this_descriptor::ModuleDescriptor::new("shop", "0.3.0");
        descriptor
            .entities
            .push(this_descriptor::EntityDesc::new("order"));

        let rendered = harness
            .render_against(
                &WasmiExecutor,
                "shop",
                "0.3.0",
                &serde_json::json!({ "view": "orders" }),
                &Limits::default(),
                &descriptor,
            )
            .await
            .expect("une vue conforme passe le descripteur");
        assert_eq!(rendered.views[0].name, "orders");
    }
}
