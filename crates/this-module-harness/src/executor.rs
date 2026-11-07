//! Running a compiled module, without choosing an engine here.
//!
//! A host has `wasmi`; a page has `WebAssembly`. If the harness picked one, it
//! would stop compiling for the other — the same trap that made `Vfs` hard to
//! share. So the engine is a trait, and the harness never names one.
//!
//! What the trait carries is deliberately more than a return value: it carries
//! **the effect trace**. A module that returns `1` in both engines but called
//! `emit` once and not the other is not the same module, and a comparison that
//! only looks at the value would call that agreement.

use this_module_lang::eval::Value;

/// What an execution produced.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Outcome {
    /// The returned value, in a form two runs can be compared on.
    pub value: String,
    /// Every host primitive the module called, in order.
    pub effects: Vec<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum ExecutorError {
    #[error("module wasm invalide : {0}")]
    Module(String),
    #[error("instanciation impossible : {0}")]
    Link(String),
    #[error("le module n'exporte pas `run`")]
    MissingEntry,
    #[error("exécution interrompue : {0}")]
    Trap(String),

    // --- Contrat d'appel de vue (`crate::render`) ---
    #[error(
        "le contrat de vue n'admet aucun import ; le module en déclare {count} ({found}) : \
         un module de vue reçoit ses données en argument et n'appelle pas l'hôte"
    )]
    ImportsRefused { count: usize, found: String },
    #[error("le module n'exporte pas `{name}` : {how}")]
    MissingExport { name: String, how: String },
    #[error("`{name}` a la signature {found}, attendu {expected}")]
    WrongSignature {
        name: String,
        expected: String,
        found: String,
    },
    #[error("budget d'exécution épuisé ({fuel} unités de fuel) : la boucle du module est arrêtée")]
    OutOfFuel { fuel: u64 },
    #[error("mémoire refusée (plafond {limit_bytes} octets) : {message}")]
    Memory { limit_bytes: usize, message: String },
    #[error("requête trop grande : {len} octets, plafond {max}")]
    RequestTooLarge { len: usize, max: usize },
    #[error("réponse trop grande : {len} octets, plafond {max}")]
    ResponseTooLarge { len: usize, max: usize },
    #[error("tampon hors mémoire : {ptr}+{len} alors que la mémoire fait {size} octets ({why})")]
    BadBuffer {
        ptr: i64,
        len: usize,
        size: usize,
        why: String,
    },
}

/// Runs a module and reports its value and effects.
///
/// `now` is passed in rather than read by the engine: two runs compared for
/// equality must see the same clock, otherwise a program that reads the time
/// diverges by a few milliseconds and the comparison says nothing.
///
/// Asynchrone, et `Send` seulement hors wasm : `js-sys` n'expose que la forme
/// asynchrone de `WebAssembly` (`compile`, `instantiate_buffer`), et les futures
/// d'un moteur navigateur ne sont pas `Send` (`JsFuture` tient un `Rc`). Sur
/// l'hôte, les handlers exigent `Send`. Le trait dit donc la vérité par cible,
/// comme `Vfs` — au lieu d'imposer à l'un la contrainte de l'autre.
#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
pub trait Executor {
    async fn run(&self, program: &[u8], args: &[Value], now: i64)
    -> Result<Outcome, ExecutorError>;
}

/// Renders a value the same way for every engine, so outcomes are comparable.
pub fn show(value: &Result<Value, String>) -> String {
    match value {
        Ok(Value::Int(n)) => n.to_string(),
        Ok(Value::Bool(b)) => b.to_string(),
        Ok(Value::Str(s)) => format!("{s:?}"),
        Ok(Value::Unit) => "()".to_string(),
        Err(error) => format!("erreur : {error}"),
    }
}

#[cfg(feature = "wasmi")]
pub use wasmi_engine::WasmiExecutor;

#[cfg(feature = "wasmi")]
mod wasmi_engine {
    use super::{Executor, ExecutorError, Outcome, show};
    use this_module_lang::eval::Value;
    use wasmi::{Caller, Engine, Extern, ExternType, Linker, Module, Store, Val, ValType};

    /// Collects effects while the module runs.
    #[derive(Default)]
    struct Recorder {
        effects: Vec<String>,
        now: i64,
    }

    /// An `Executor` backed by `wasmi`, for hosts.
    pub struct WasmiExecutor;

    #[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
    #[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
    impl Executor for WasmiExecutor {
        async fn run(
            &self,
            program: &[u8],
            args: &[Value],
            now: i64,
        ) -> Result<Outcome, ExecutorError> {
            let engine = Engine::default();
            let module =
                Module::new(&engine, program).map_err(|e| ExecutorError::Module(e.to_string()))?;

            let mut store = Store::new(
                &engine,
                Recorder {
                    effects: Vec::new(),
                    now,
                },
            );
            let mut linker = <Linker<Recorder>>::new(&engine);

            linker
                .func_wrap(
                    "host",
                    "emit",
                    |mut caller: Caller<'_, Recorder>, ptr: i32, len: i32| {
                        let Some(Extern::Memory(memory)) = caller.get_export("memory") else {
                            return;
                        };
                        let data = memory.data(&caller);
                        let (start, end) = (ptr.max(0) as usize, (ptr + len).max(0) as usize);
                        if end > data.len() || start > end {
                            caller
                                .data_mut()
                                .effects
                                .push(format!("emit(hors mémoire : {start}+{end})"));
                            return;
                        }
                        let text = String::from_utf8_lossy(&data[start..end]).into_owned();
                        caller.data_mut().effects.push(format!("emit({text:?})"));
                    },
                )
                .map_err(|e| ExecutorError::Link(e.to_string()))?;

            linker
                .func_wrap("host", "now", |mut caller: Caller<'_, Recorder>| -> i64 {
                    caller.data_mut().effects.push("now()".to_string());
                    caller.data().now
                })
                .map_err(|e| ExecutorError::Link(e.to_string()))?;

            let instance = linker
                .instantiate_and_start(&mut store, &module)
                .map_err(|e| ExecutorError::Link(e.to_string()))?;

            let function = instance
                .get_func(&store, "run")
                .ok_or(ExecutorError::MissingEntry)?;

            // Le type de retour est lu dans le module : quand on relit un artefact
            // depuis le VFS, on n'a que les octets, pas le type d'origine.
            let results: Vec<ValType> = module
                .exports()
                .find(|e| e.name() == "run")
                .and_then(|e| match e.ty() {
                    ExternType::Func(f) => Some(f.results().to_vec()),
                    _ => None,
                })
                .unwrap_or_default();

            let inputs: Vec<Val> = args
                .iter()
                .map(|v| match v {
                    Value::Int(n) => Val::I64(*n),
                    Value::Bool(b) => Val::I32(i32::from(*b)),
                    Value::Str(_) | Value::Unit => Val::I32(0),
                })
                .collect();

            let mut outputs: Vec<Val> = results
                .iter()
                .map(|t| match t {
                    ValType::I64 => Val::I64(0),
                    _ => Val::I32(0),
                })
                .collect();

            function
                .call(&mut store, &inputs, &mut outputs)
                .map_err(|e| ExecutorError::Trap(e.to_string()))?;

            let value = match outputs.first() {
                Some(Val::I64(n)) => show(&Ok(Value::Int(*n))),
                Some(Val::I32(n)) => show(&Ok(Value::Bool(*n != 0))),
                _ => show(&Ok(Value::Unit)),
            };

            Ok(Outcome {
                value,
                effects: store.data().effects.clone(),
            })
        }
    }

    /// Le contrat de vue, tenu par `wasmi` : borné en fuel et en mémoire (voir
    /// [`crate::render::bounded`]).
    #[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
    #[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
    impl crate::render::ViewRenderer for WasmiExecutor {
        async fn render(
            &self,
            program: &[u8],
            request: &[u8],
            limits: &crate::render::Limits,
        ) -> Result<crate::render::Call, ExecutorError> {
            crate::render::call_bounded(program, request, limits)
        }
    }
}

#[cfg(all(test, feature = "wasmi"))]
mod tests {
    use super::*;
    use this_module_lang::assemble::compile_to_wasm;
    use this_module_lang::compile;
    use this_module_lang::eval::{Host, run};

    const PROGRAM: &str = "fn run(a: Int) -> Int { emit(\"bonjour depuis le module\"); a + 1 }";
    const NOW: i64 = 1_700_000_000_000;

    struct Recorder {
        effects: Vec<String>,
    }

    impl Host for Recorder {
        fn call(&mut self, name: &str, args: &[Value]) -> Result<Value, String> {
            match (name, args) {
                ("emit", [Value::Str(text)]) => {
                    self.effects.push(format!("emit({text:?})"));
                    Ok(Value::Unit)
                }
                ("now", []) => {
                    self.effects.push("now()".to_string());
                    Ok(Value::Int(NOW))
                }
                _ => Err(format!("primitive inconnue : {name}")),
            }
        }
    }

    fn reference(source: &str, args: &[Value]) -> Outcome {
        let compiled = compile(source).expect("le programme compile");
        let mut recorder = Recorder {
            effects: Vec::new(),
        };
        let value = run(&compiled.function, args, &mut recorder);
        Outcome {
            value: show(&value),
            effects: recorder.effects,
        }
    }

    async fn through_wasmi(source: &str, args: &[Value]) -> Outcome {
        let program = compile_to_wasm(source).expect("assemblage");
        WasmiExecutor
            .run(&program, args, NOW)
            .await
            .expect("exécution")
    }

    #[tokio::test]
    async fn the_engine_agrees_with_the_interpreter_on_value_and_effects() {
        let args = [Value::Int(1)];
        let expected = reference(PROGRAM, &args);
        let got = through_wasmi(PROGRAM, &args).await;
        assert_eq!(
            got, expected,
            "les deux exécuteurs doivent dire la même chose"
        );
        assert_eq!(got.value, "2");
        assert_eq!(
            got.effects,
            vec!["emit(\"bonjour depuis le module\")".to_string()]
        );
    }

    #[tokio::test]
    async fn the_clock_is_the_one_that_was_passed() {
        let program = "fn run() -> Int { now() }";
        let expected = reference(program, &[]);
        let got = through_wasmi(program, &[]).await;
        assert_eq!(got.value, expected.value, "même horloge, même valeur");
        assert_eq!(got.effects, vec!["now()".to_string()]);
    }

    #[tokio::test]
    async fn a_program_without_effects_reports_none() {
        let got = through_wasmi("fn run() -> Int { 40 + 2 }", &[]).await;
        assert_eq!(got.value, "42");
        assert!(
            got.effects.is_empty(),
            "aucun effet attendu, reçu {:?}",
            got.effects
        );
    }
}
