//! The browser `Executor`: `WebAssembly`, host calls, and the effect trace.
//!
//! A host runs modules through `wasmi`; a page runs them through the engine it
//! already has. The trait exists so the harness never names either.
//!
//! ## Why this is asynchronous
//!
//! `js-sys` binds the asynchronous API (`WebAssembly.compile`,
//! `instantiate_buffer`), not the synchronous constructors. Reaching those through
//! `Reflect` would work — synchronous compilation was measured to accept a 43 kB
//! module in Chrome — but it would be an untyped detour into the DOM to avoid an
//! `await`, and the trait can carry the `await` honestly.
//!
//! ## What it reports
//!
//! The value **and** the effect trace. A module that returns `1` in both engines
//! but called `emit` once and not the other is not the same module.

pub mod meter;

#[cfg(target_arch = "wasm32")]
use std::cell::{Cell, RefCell};
#[cfg(target_arch = "wasm32")]
use std::rc::Rc;

use async_trait::async_trait;
#[cfg(target_arch = "wasm32")]
use js_sys::{Object, Reflect, Uint8Array};
use this_module_harness::call::Call;
#[cfg(target_arch = "wasm32")]
use this_module_harness::executor::show;
use this_module_harness::executor::{Executor, ExecutorError, Outcome};
use this_module_harness::render::{Limits, ViewRenderer};
use this_module_lang::eval::Value;
#[cfg(target_arch = "wasm32")]
use wasm_bindgen::closure::Closure;
#[cfg(target_arch = "wasm32")]
use wasm_bindgen::{JsCast, JsValue};
#[cfg(target_arch = "wasm32")]
use wasm_bindgen_futures::JsFuture;

/// An `Executor` backed by the page's own WebAssembly engine.
pub struct BrowserExecutor;

/// Hors wasm, le trait `Executor` exige un futur `Send` (les handlers hôtes l'imposent),
/// alors que celui du navigateur tient des `Rc` et un `JsFuture` : il ne peut pas l'être.
/// Il n'y a d'ailleurs pas de moteur `WebAssembly` hors navigateur. En natif, le type
/// existe donc (pour que ses appelants compilent et que leurs tests tournent) mais
/// refuse d'exécuter ; l'hôte a `WasmiExecutor`.
#[cfg(not(target_arch = "wasm32"))]
#[async_trait]
impl Executor for BrowserExecutor {
    async fn run(
        &self,
        _program: &[u8],
        _args: &[Value],
        _now: i64,
    ) -> Result<Outcome, ExecutorError> {
        Err(ExecutorError::Link(
            "BrowserExecutor : moteur WebAssembly du navigateur absent hors wasm32".into(),
        ))
    }
}

#[cfg(target_arch = "wasm32")]
#[async_trait(?Send)]
impl Executor for BrowserExecutor {
    async fn run(
        &self,
        program: &[u8],
        args: &[Value],
        now: i64,
    ) -> Result<Outcome, ExecutorError> {
        // La mémoire n'existe qu'après l'instanciation, mais `emit` en a besoin
        // pendant l'exécution : on la dépose ici dès qu'elle est connue.
        let memory: Rc<RefCell<Option<JsValue>>> = Rc::new(RefCell::new(None));
        let effects: Rc<RefCell<Vec<String>>> = Rc::new(RefCell::new(Vec::new()));

        let emit = {
            let memory = memory.clone();
            let effects = effects.clone();
            Closure::<dyn FnMut(i32, i32)>::new(move |ptr: i32, len: i32| {
                let Some(memory) = memory.borrow().clone() else {
                    return;
                };
                let Ok(buffer) = Reflect::get(&memory, &JsValue::from_str("buffer")) else {
                    return;
                };
                let total = Uint8Array::new(&buffer).length();
                let start = ptr.max(0) as u32;
                let size = len.max(0) as u32;
                if start.checked_add(size).is_none_or(|end| end > total) {
                    effects
                        .borrow_mut()
                        .push(format!("emit(hors mémoire : {start}+{size} > {total})"));
                    return;
                }
                let view = Uint8Array::new_with_byte_offset_and_length(&buffer, start, size);
                let text = String::from_utf8_lossy(&view.to_vec()).into_owned();
                effects.borrow_mut().push(format!("emit({text:?})"));
            })
        };

        let clock = {
            let effects = effects.clone();
            Closure::<dyn FnMut() -> i64>::new(move || {
                effects.borrow_mut().push("now()".to_string());
                now
            })
        };

        // Le budget de tours : épuisé, le module piège (`unreachable`) au lieu
        // de figer l'onglet, qui ne pourrait plus être interrompu.
        let fuel = {
            let left = Rc::new(Cell::new(this_module_lang::eval::TOURS_MAX as u64));
            Closure::<dyn FnMut() -> i32>::new(move || {
                let n = left.get();
                if n == 0 {
                    return 0;
                }
                left.set(n - 1);
                1
            })
        };

        let host = Object::new();
        set(&host, "emit", emit.as_ref().unchecked_ref())?;
        set(&host, "now", clock.as_ref().unchecked_ref())?;
        set(&host, "fuel", fuel.as_ref().unchecked_ref())?;
        let imports = Object::new();
        set(&imports, "host", &host)?;

        // `WebAssembly.instantiate(bytes, imports)` rend `{ module, instance }`.
        let source = JsFuture::from(js_sys::WebAssembly::instantiate_buffer(program, &imports))
            .await
            .map_err(|error| ExecutorError::Link(describe(&error)))?;
        let held = Reflect::get(&source, &JsValue::from_str("instance")).unwrap_or(source);
        let instance: js_sys::WebAssembly::Instance = held
            .dyn_into()
            .map_err(|_| ExecutorError::Link("instanciation : type inattendu".into()))?;

        let exports = js_sys::WebAssembly::Instance::exports(&instance);
        *memory.borrow_mut() = Reflect::get(&exports, &JsValue::from_str("memory")).ok();

        let entry = Reflect::get(&exports, &JsValue::from_str("run"))
            .map_err(|_| ExecutorError::MissingEntry)?;
        let entry: js_sys::Function = entry.dyn_into().map_err(|_| ExecutorError::MissingEntry)?;

        let js_args: Vec<JsValue> = args
            .iter()
            .map(|value| match value {
                Value::Int(n) => js_sys::BigInt::from(*n).into(),
                Value::Bool(b) => JsValue::from_bool(*b),
                _ => JsValue::UNDEFINED,
            })
            .collect();

        let returned = match js_args.len() {
            0 => entry.call0(&JsValue::NULL),
            1 => entry.call1(&JsValue::NULL, &js_args[0]),
            2 => entry.call2(&JsValue::NULL, &js_args[0], &js_args[1]),
            n => return Err(ExecutorError::Trap(format!("{n} arguments : non supporté"))),
        }
        .map_err(|error| ExecutorError::Trap(describe(&error)))?;

        let value = render(&returned);
        let effects = effects.borrow().clone();
        Ok(Outcome { value, effects })
    }
}

/// Le type de retour se lit dans la valeur rendue : `BigInt` pour `Int`, nombre
/// pour `Bool`, `undefined` pour `Unit`. C'est ce qui permet d'exécuter un
/// artefact relu du VFS, dont on n'a que les octets.
#[cfg(target_arch = "wasm32")]
fn render(value: &JsValue) -> String {
    if value.is_undefined() || value.is_null() {
        return show(&Ok(Value::Unit));
    }
    if let Ok(number) = value.clone().dyn_into::<js_sys::Number>() {
        return show(&Ok(Value::Bool(number.as_f64().unwrap_or(0.0) != 0.0)));
    }
    let bigint: &js_sys::BigInt = value.unchecked_ref();
    match bigint.to_string(10) {
        Ok(text) => show(&Ok(Value::Int(
            text.as_string().and_then(|s| s.parse().ok()).unwrap_or(0),
        ))),
        Err(_) => "?".to_string(),
    }
}

#[cfg(target_arch = "wasm32")]
fn set(target: &Object, key: &str, value: &JsValue) -> Result<(), ExecutorError> {
    Reflect::set(target, &JsValue::from_str(key), value)
        .map(|_| ())
        .map_err(|error| ExecutorError::Link(describe(&error)))
}

#[cfg(target_arch = "wasm32")]
fn describe(error: &JsValue) -> String {
    error
        .as_string()
        .or_else(|| {
            Reflect::get(error, &JsValue::from_str("message"))
                .ok()
                .and_then(|m| m.as_string())
        })
        .unwrap_or_else(|| format!("{error:?}"))
}

// ---------------------------------------------------------------------------
// Le rendu de vue dans la page
// ---------------------------------------------------------------------------

/// Un [`ViewRenderer`] pour la page : mêmes refus que `wasmi`, bornes tenues par
/// **instrumentation du binaire** (voir [`meter`]) puisque la page n'a pas de fuel.
///
/// Ce qui est refusé, nommément, et dans cet ordre : une requête au-delà de
/// `Limits::request_bytes` (`RequestTooLarge`), un binaire invalide (`Module`), un import
/// (`ImportsRefused`), un export absent ou mal typé (`MissingExport`, `WrongSignature`),
/// une mémoire déclarée trop grande (`Memory`), un module qui épuise son budget
/// (`OutOfFuel`), une réponse au-delà de `Limits::response_bytes` (`ResponseTooLarge`,
/// avec sa taille réelle), un tampon hors mémoire (`BadBuffer`). Aucun chemin ne panique,
/// aucun n'attend un événement qui ne viendrait pas.
pub struct BrowserRenderer;

/// Ce qui se décide sans moteur : la taille de la requête, puis l'instrumentation (et
/// avec elle tous les refus du binaire). Séparé pour que le natif puisse le tester.
pub fn prepare(
    program: &[u8],
    request: &[u8],
    limits: &Limits,
) -> Result<meter::Metered, ExecutorError> {
    if request.len() > limits.request_bytes {
        return Err(ExecutorError::RequestTooLarge {
            len: request.len(),
            max: limits.request_bytes,
        });
    }
    meter::meter(program, limits)
}

/// Hors wasm32 il n'y a pas de moteur `WebAssembly` : les refus qui n'en ont pas besoin
/// restent rendus (et testés), le reste est refusé nommément.
#[cfg(not(target_arch = "wasm32"))]
#[async_trait]
impl ViewRenderer for BrowserRenderer {
    async fn render(
        &self,
        program: &[u8],
        request: &[u8],
        limits: &Limits,
    ) -> Result<Call, ExecutorError> {
        prepare(program, request, limits)?;
        Err(ExecutorError::Link(
            "BrowserRenderer : moteur WebAssembly du navigateur absent hors wasm32".into(),
        ))
    }
}

#[cfg(target_arch = "wasm32")]
#[async_trait(?Send)]
impl ViewRenderer for BrowserRenderer {
    async fn render(
        &self,
        program: &[u8],
        request: &[u8],
        limits: &Limits,
    ) -> Result<Call, ExecutorError> {
        let metered = prepare(program, request, limits)?;

        // Aucun import n'est admis : l'objet d'imports est vide, et le binaire a déjà
        // été refusé s'il en déclarait.
        let source = JsFuture::from(js_sys::WebAssembly::instantiate_buffer(
            &metered.wasm,
            &Object::new(),
        ))
        .await
        .map_err(|error| {
            fuel_or(&error, None, metered.fuel, |e| {
                ExecutorError::Link(describe(e))
            })
        })?;
        let held = Reflect::get(&source, &JsValue::from_str("instance")).unwrap_or(source);
        let instance: js_sys::WebAssembly::Instance = held
            .dyn_into()
            .map_err(|_| ExecutorError::Link("instanciation : type inattendu".into()))?;
        let exports = js_sys::WebAssembly::Instance::exports(&instance);

        let get = |name: &str| Reflect::get(&exports, &JsValue::from_str(name));
        let memory = get(meter_memory()).map_err(|_| missing_memory())?;
        let alloc: js_sys::Function = get("alloc")
            .ok()
            .and_then(|f| f.dyn_into().ok())
            .ok_or_else(missing_memory)?;
        let render: js_sys::Function = get("render")
            .ok()
            .and_then(|f| f.dyn_into().ok())
            .ok_or_else(missing_memory)?;
        let counter = get(meter::FUEL_EXPORT).ok();

        // `alloc` et `render` sont du code du module : borné par le compteur, jamais par
        // un délai. Une exception dont le compteur porte le sentinel est un épuisement.
        let trapped = |error: &JsValue, step: &str| {
            fuel_or(error, counter.as_ref(), metered.fuel, |e| {
                ExecutorError::Trap(format!("{step} : {}", describe(e)))
            })
        };

        let len = i32::try_from(request.len()).map_err(|_| ExecutorError::RequestTooLarge {
            len: request.len(),
            max: i32::MAX as usize,
        })?;
        let mut ptr = 0i32;
        if !request.is_empty() {
            let got = alloc
                .call1(&JsValue::NULL, &JsValue::from_f64(f64::from(len)))
                .map_err(|e| trapped(&e, "alloc"))?;
            ptr = got.as_f64().map(|n| n as i32).unwrap_or(-1);
            let buffer = buffer_of(&memory)?;
            let size = Uint8Array::new(&buffer).length() as usize;
            let start = usable(ptr, request.len(), size)?;
            Uint8Array::new_with_byte_offset_and_length(
                &buffer,
                start as u32,
                request.len() as u32,
            )
            .copy_from(request);
        }

        let packed = render
            .call2(
                &JsValue::NULL,
                &JsValue::from_f64(f64::from(ptr)),
                &JsValue::from_f64(f64::from(len)),
            )
            .map_err(|e| trapped(&e, "render"))?;
        let packed = i64::try_from(packed)
            .map_err(|_| ExecutorError::Trap("render : résultat inattendu (i64 attendu)".into()))?;

        let remaining = counter
            .as_ref()
            .and_then(|c| Reflect::get(c, &JsValue::from_str("value")).ok())
            .and_then(|v| i64::try_from(v).ok())
            .unwrap_or(0);
        let fuel_used = match meter::consumed(metered.fuel, remaining) {
            Ok(used) => used,
            // Rendu normalement mais compteur à -1 : le module a avalé l'épuisement.
            Err(fuel) => return Err(ExecutorError::OutOfFuel { fuel }),
        };

        let out_ptr = ((packed as u64) >> 32) as u32 as i32;
        let out_len = ((packed as u64) & 0xffff_ffff) as u32 as usize;
        let buffer = buffer_of(&memory)?;
        let size = Uint8Array::new(&buffer).length() as usize;
        // Bornée avant d'être copiée : la taille réelle est dans le refus.
        let max = limits.response_bytes.min(size);
        if out_len > max {
            return Err(ExecutorError::ResponseTooLarge { len: out_len, max });
        }
        let start = usable(out_ptr, out_len, size)?;
        let response =
            Uint8Array::new_with_byte_offset_and_length(&buffer, start as u32, out_len as u32)
                .to_vec();
        Ok(Call {
            response,
            fuel_used,
        })
    }
}

#[cfg(target_arch = "wasm32")]
fn meter_memory() -> &'static str {
    this_module_harness::render::MEMORY_EXPORT
}

#[cfg(target_arch = "wasm32")]
fn missing_memory() -> ExecutorError {
    ExecutorError::Link("export absent après instrumentation (binaire déjà vérifié)".into())
}

#[cfg(target_arch = "wasm32")]
fn buffer_of(memory: &JsValue) -> Result<JsValue, ExecutorError> {
    Reflect::get(memory, &JsValue::from_str("buffer")).map_err(|e| ExecutorError::BadBuffer {
        ptr: 0,
        len: 0,
        size: 0,
        why: describe(&e),
    })
}

/// Traduit un décalage rendu par le module en plage sûre, ou refuse.
#[cfg(target_arch = "wasm32")]
fn usable(ptr: i32, len: usize, size: usize) -> Result<usize, ExecutorError> {
    let bad = |why: String| ExecutorError::BadBuffer {
        ptr: i64::from(ptr),
        len,
        size,
        why,
    };
    let start = usize::try_from(ptr).map_err(|_| {
        bad(format!(
            "`{ptr}` n'est pas un décalage : `alloc` doit rendre un i32 positif"
        ))
    })?;
    let end = start
        .checked_add(len)
        .ok_or_else(|| bad(format!("{start} + {len} déborde un décalage")))?;
    if end > size {
        return Err(bad(format!(
            "{start}+{len} dépasse la mémoire du module ({size} octets)"
        )));
    }
    Ok(start)
}

/// Une exception du module : si le compteur porte le sentinel (`-1`), c'est l'épuisement
/// du budget, rendu `OutOfFuel` ; sinon l'erreur que `other` décrit.
#[cfg(target_arch = "wasm32")]
fn fuel_or(
    error: &JsValue,
    counter: Option<&JsValue>,
    budget: u64,
    other: impl FnOnce(&JsValue) -> ExecutorError,
) -> ExecutorError {
    let left = counter
        .and_then(|c| Reflect::get(c, &JsValue::from_str("value")).ok())
        .and_then(|v| i64::try_from(v).ok());
    match left.map(|l| meter::consumed(budget, l)) {
        Some(Err(fuel)) => ExecutorError::OutOfFuel { fuel },
        _ => other(error),
    }
}

#[cfg(test)]
mod prepare_tests {
    use super::*;
    use crate::meter::fixtures::*;

    #[test]
    fn a_request_over_64_kib_is_refused_with_its_real_size() {
        let request = vec![b' '; 64 * 1024 + 1];
        match prepare(&answering(VIEW), &request, &Limits::view()) {
            Err(ExecutorError::RequestTooLarge { len, max }) => {
                assert_eq!((len, max), (65_537, 65_536));
            }
            other => panic!("une requête trop grosse doit être refusée : {other:?}"),
        }
    }

    #[test]
    fn a_request_at_the_cap_passes() {
        let request = vec![b' '; 64 * 1024];
        assert!(prepare(&answering(VIEW), &request, &Limits::view()).is_ok());
    }

    #[test]
    fn the_request_is_refused_before_the_binary_is_even_read() {
        // Un binaire invalide ET une requête trop grosse : la requête est nommée d'abord.
        let request = vec![b' '; 70_000];
        assert!(matches!(
            prepare(b"pas du wasm", &request, &Limits::view()),
            Err(ExecutorError::RequestTooLarge { .. })
        ));
    }
}

/// Les tests de la page : `wasm-bindgen-test`, dans le moteur `WebAssembly` réel (nœud
/// ou navigateur selon le runner). Un module qui boucle y est **réellement** exécuté.
#[cfg(all(test, target_arch = "wasm32"))]
mod page_tests {
    use super::*;
    use crate::meter::fixtures::*;
    use wasm_bindgen_test::*;

    fn request() -> Vec<u8> {
        br#"{"view":"orders"}"#.to_vec()
    }

    #[wasm_bindgen_test]
    async fn a_looping_module_is_stopped_and_the_app_lives() {
        let started = js_sys::Date::now();
        let outcome = BrowserRenderer
            .render(&looping(), &request(), &Limits::view())
            .await;
        let elapsed = js_sys::Date::now() - started;
        match outcome {
            Err(ExecutorError::OutOfFuel { fuel }) => assert_eq!(fuel, 30_000_000),
            other => panic!("la boucle doit être arrêtée par son budget : {other:?}"),
        }
        // L'app est vivante : on rend juste après, avec la même instance de moteur.
        let after = BrowserRenderer
            .render(&answering(VIEW), &request(), &Limits::view())
            .await
            .expect("un arbre conforme passe après une boucle");
        assert_eq!(after.response, VIEW.as_bytes());
        // Mesure, pas assertion : la valeur est lue dans la sortie du runner.
        web_sys_log(&format!("MESURE boucle coupée en {elapsed} ms (30 M)"));
    }

    #[wasm_bindgen_test]
    async fn a_looping_start_section_is_stopped() {
        match BrowserRenderer
            .render(&looping_start(), &request(), &Limits::view())
            .await
        {
            Err(ExecutorError::OutOfFuel { .. }) | Err(ExecutorError::Link(_)) => {}
            other => panic!("un `start` qui boucle ne doit pas rendre : {other:?}"),
        }
    }

    #[wasm_bindgen_test]
    async fn the_fuel_budget_is_the_one_asked() {
        let tight = Limits {
            fuel: 1_000,
            ..Limits::view()
        };
        match BrowserRenderer.render(&looping(), &request(), &tight).await {
            Err(ExecutorError::OutOfFuel { fuel }) => assert_eq!(fuel, 1_000),
            other => panic!("{other:?}"),
        }
    }

    #[wasm_bindgen_test]
    async fn a_conforming_tree_passes() {
        let call = BrowserRenderer
            .render(&answering(VIEW), &request(), &Limits::view())
            .await
            .expect("conforme");
        assert_eq!(call.response, VIEW.as_bytes());
        assert!(
            call.fuel_used > 0 && call.fuel_used < 1_000,
            "{}",
            call.fuel_used
        );
    }

    #[wasm_bindgen_test]
    async fn a_tree_over_32_kib_is_refused_with_its_real_size() {
        let big = "x".repeat(40_000);
        match BrowserRenderer
            .render(&answering(&big), &request(), &Limits::view())
            .await
        {
            Err(error @ ExecutorError::ResponseTooLarge { .. }) => {
                let message = error.to_string();
                assert!(
                    message.contains("40000"),
                    "taille réelle dans le message : {message}"
                );
                assert!(
                    message.contains("32768"),
                    "plafond dans le message : {message}"
                );
            }
            other => panic!("{other:?}"),
        }
    }

    #[wasm_bindgen_test]
    async fn a_request_over_64_kib_is_refused() {
        let big = vec![b' '; 65 * 1024];
        match BrowserRenderer
            .render(&answering(VIEW), &big, &Limits::view())
            .await
        {
            Err(ExecutorError::RequestTooLarge { len, max }) => {
                assert_eq!((len, max), (65 * 1024, 64 * 1024));
            }
            other => panic!("{other:?}"),
        }
    }

    #[wasm_bindgen_test]
    async fn an_import_is_refused_by_name() {
        match BrowserRenderer
            .render(&importing(), &request(), &Limits::view())
            .await
        {
            Err(ExecutorError::ImportsRefused { found, .. }) => assert_eq!(found, "env.now"),
            other => panic!("{other:?}"),
        }
    }

    #[wasm_bindgen_test]
    async fn a_trap_other_than_fuel_is_a_trap_not_a_fuel_refusal() {
        let wasm = wat::parse_str(
            r#"(module
  (memory (export "memory") 1)
  (func (export "alloc") (param i32) (result i32) (i32.const 0))
  (func (export "render") (param i32 i32) (result i64) unreachable))"#,
        )
        .unwrap();
        match BrowserRenderer
            .render(&wasm, &request(), &Limits::view())
            .await
        {
            Err(ExecutorError::Trap(_)) => {}
            other => panic!("{other:?}"),
        }
    }

    /// Ce que l'instrumentation coûte en vitesse : la même boucle, brute puis instrumentée.
    #[wasm_bindgen_test]
    async fn instrumentation_speed_is_measured() {
        let raw = counting();
        let wide = Limits {
            fuel: 4_000_000_000,
            ..Limits::view()
        };
        let metered = meter::meter(&raw, &wide).unwrap();
        let n = 20_000_000f64;
        let time = |wasm: Vec<u8>| async move {
            let source = JsFuture::from(js_sys::WebAssembly::instantiate_buffer(
                &wasm,
                &Object::new(),
            ))
            .await
            .unwrap();
            let instance: js_sys::WebAssembly::Instance =
                Reflect::get(&source, &JsValue::from_str("instance"))
                    .unwrap()
                    .dyn_into()
                    .unwrap();
            let exports = js_sys::WebAssembly::Instance::exports(&instance);
            let f: js_sys::Function = Reflect::get(&exports, &JsValue::from_str("render"))
                .unwrap()
                .dyn_into()
                .unwrap();
            let t = js_sys::Date::now();
            f.call2(
                &JsValue::NULL,
                &JsValue::from_f64(n),
                &JsValue::from_f64(0.0),
            )
            .unwrap();
            js_sys::Date::now() - t
        };
        let brut = time(raw.clone()).await;
        let instrumente = time(metered.wasm.clone()).await;
        web_sys_log(&format!(
            "MESURE vitesse 20 M tours : brut {brut} ms, instrumenté {instrumente} ms ; taille {} -> {} octets",
            raw.len(),
            metered.wasm.len()
        ));
    }

    fn web_sys_log(text: &str) {
        js_sys::eval(&format!("console.log({text:?})")).ok();
    }
}
