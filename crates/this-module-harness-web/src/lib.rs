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

#[cfg(target_arch = "wasm32")]
use std::cell::{Cell, RefCell};
#[cfg(target_arch = "wasm32")]
use std::rc::Rc;

use async_trait::async_trait;
#[cfg(target_arch = "wasm32")]
use js_sys::{Object, Reflect, Uint8Array};
#[cfg(target_arch = "wasm32")]
use this_module_harness::executor::show;
use this_module_harness::executor::{Executor, ExecutorError, Outcome};
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
