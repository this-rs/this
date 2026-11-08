//! L'interpréteur est la **référence**, le wasm émis est la **cible**.
//!
//! Ce fichier existe pour une seule raison : prouver que les deux répondent la
//! même chose. Sans lui, `wat::emit` ne serait vérifié par rien — un module
//! wasm faux se compile sans bruit, et on ne le découvrirait qu'au moment de
//! charger un module en production, c'est-à-dire trop tard.
//!
//! Chaque cas est joué deux fois : une fois par `eval::run`, une fois par un
//! vrai moteur wasm (`wasmi`, qui valide le module puis l'exécute). La valeur
//! rendue **et** la suite des effets observés doivent coïncider.

use std::fmt::Write as _;

use this_module_lang::eval::{Host, Value, run};
use this_module_lang::{Type, compile};
use wasmi::{Caller, Engine, Extern, Linker, Module, Store, Val};

/// Ce que l'hôte observe. Identique des deux côtés : c'est ce qui rend la
/// comparaison honnête.
struct Recorder {
    emitted: Vec<String>,
    calls: usize,
    /// Le budget de boucle restant. Le wasm le consomme par un import ; la
    /// référence le compte chez elle. Les deux doivent s'arrêter au même tour.
    fuel: u64,
}

impl Default for Recorder {
    fn default() -> Self {
        Self {
            emitted: Vec::new(),
            calls: 0,
            fuel: this_module_lang::eval::TOURS_MAX as u64,
        }
    }
}

impl Host for Recorder {
    fn call(&mut self, name: &str, args: &[Value]) -> Result<Value, String> {
        self.calls += 1;
        match (name, args) {
            ("emit", [Value::Str(text)]) => {
                self.emitted.push(text.clone());
                Ok(Value::Unit)
            }
            ("now", []) => Ok(Value::Int(1000)),
            _ => Err(format!("primitive inconnue : {name}")),
        }
    }
}

fn to_val(value: &Value) -> Val {
    match value {
        Value::Int(value) => Val::I64(*value),
        Value::Bool(value) => Val::I32(i32::from(*value)),
        Value::Str(_) => panic!("une chaîne ne traverse pas la frontière en valeur simple"),
        Value::Unit => panic!("Unit n'a pas de représentation wasm"),
    }
}

fn from_val(value: Val, ty: Type) -> Value {
    match (value, ty) {
        (Val::I64(value), Type::Int) => Value::Int(value),
        (Val::I32(value), Type::Bool) => Value::Bool(value != 0),
        (other, ty) => panic!("valeur wasm {other:?} pour un type {}", ty.name()),
    }
}

/// Exécute le module émis dans un vrai moteur wasm.
fn through_wasm(source: &str, args: &[Value]) -> (Result<Value, String>, Recorder) {
    let compiled = compile(source).expect("le programme doit compiler");
    let bytes = wat::parse_str(&compiled.wat.text)
        .unwrap_or_else(|error| panic!("WAT invalide : {error}\n{}", compiled.wat.text));

    let engine = Engine::default();
    let module = Module::new(&engine, &bytes[..])
        .unwrap_or_else(|error| panic!("module wasm invalide : {error}\n{}", compiled.wat.text));

    let mut store = Store::new(&engine, Recorder::default());
    let mut linker = <Linker<Recorder>>::new(&engine);

    linker
        .func_wrap(
            "host",
            "emit",
            |mut caller: Caller<'_, Recorder>, ptr: i32, len: i32| {
                let Some(Extern::Memory(memory)) = caller.get_export("memory") else {
                    panic!("le module appelle `emit` sans exporter de mémoire");
                };
                let bytes = memory.data(&caller);
                let (start, end) = (ptr as usize, (ptr + len) as usize);
                let text = String::from_utf8_lossy(&bytes[start..end]).into_owned();
                caller.data_mut().calls += 1;
                caller.data_mut().emitted.push(text);
            },
        )
        .expect("import emit");
    linker
        .func_wrap("host", "now", |mut caller: Caller<'_, Recorder>| -> i64 {
            caller.data_mut().calls += 1;
            1000
        })
        .expect("import now");
    linker
        .func_wrap("host", "fuel", |mut caller: Caller<'_, Recorder>| -> i32 {
            let left = &mut caller.data_mut().fuel;
            if *left == 0 {
                0
            } else {
                *left -= 1;
                1
            }
        })
        .expect("import fuel");

    let instance = linker
        .instantiate_and_start(&mut store, &module)
        .expect("instanciation");

    let function = instance
        .get_func(&store, "run")
        .expect("le module exporte `run`");

    let inputs: Vec<Val> = args.iter().map(to_val).collect();
    let mut outputs: Vec<Val> = match compiled.function.returns {
        Type::Unit => Vec::new(),
        Type::Int => vec![Val::I64(0)],
        Type::Bool => vec![Val::I32(0)],
        Type::Str => panic!("une fonction ne rend pas de chaîne"),
    };

    let outcome = match function.call(&mut store, &inputs, &mut outputs) {
        Ok(()) => {
            let value = match compiled.function.returns {
                Type::Unit => Value::Unit,
                ty => from_val(outputs[0].clone(), ty),
            };
            Ok(value)
        }
        Err(error) => Err(error.to_string()),
    };
    (outcome, store.into_data())
}

/// Exécute le même programme par l'interpréteur.
fn through_reference(source: &str, args: &[Value]) -> (Result<Value, String>, Recorder) {
    let compiled = compile(source).expect("le programme doit compiler");
    let mut host = Recorder::default();
    let value = run(&compiled.function, args, &mut host);
    (value, host)
}

#[cfg(test)]
mod boucles {
    use super::*;

    #[test]
    fn a_loop_accumulates_like_the_reference() {
        agree(
            "fn run() -> Int { var i: Int = 0; var somme: Int = 0; \
             while i < 10 { set somme = somme + i; set i = i + 1; }; somme }",
            &[],
        );
    }

    #[test]
    fn a_loop_emits_the_same_effects_in_the_same_order() {
        agree(
            "fn run() -> Unit { var i: Int = 0; \
             while i < 3 { emit(\"tour\"); set i = i + 1; } }",
            &[],
        );
    }

    #[test]
    fn a_nested_loop_counts_both_levels() {
        agree(
            "fn run() -> Int { var i: Int = 0; var j: Int = 0; var n: Int = 0; \
             while i < 4 { set j = 0; while j < 3 { set n = n + 1; set j = j + 1; }; \
             set i = i + 1; }; n }",
            &[],
        );
    }

    #[test]
    fn recursion_matches_on_both_paths() {
        agree(
            "fn run(n: Int) -> Int { if n <= 1 { 1 } else { n * run(n - 1) } }",
            &[Value::Int(6)],
        );
    }

    #[test]
    fn a_loop_that_never_ends_fails_on_both_paths() {
        // Le budget est le même des deux côtés : la référence s'arrête par une
        // erreur, le wasm par un import qui rend 0 puis un résultat incomplet —
        // ce qui compte, c'est qu'aucun des deux ne bloque.
        agree(
            "fn run() -> Unit { var i: Int = 0; while true { set i = i + 1; } }",
            &[],
        );
    }
}

/// Les deux chemins, comparés. C'est l'unique assertion de ce fichier.
fn agree(source: &str, args: &[Value]) {
    let (reference, reference_host) = through_reference(source, args);
    let (emitted, emitted_host) = through_wasm(source, args);

    let mut report = String::new();
    let _ = write!(report, "\nprogramme : {source}");

    match (&reference, &emitted) {
        (Ok(left), Ok(right)) => assert_eq!(left, right, "les deux chemins divergent{report}"),
        (Err(left), Err(right)) => {
            // Les messages diffèrent par nature : l'un est une erreur, l'autre un
            // piège wasm. Ce qui doit coïncider, c'est le *fait* d'échouer.
            let _ = write!(report, "\n  référence : {left}\n  wasm : {right}");
        }
        _ => panic!("un chemin échoue et l'autre non{report} : {reference:?} / {emitted:?}"),
    }

    assert_eq!(
        reference_host.emitted, emitted_host.emitted,
        "les effets observés divergent{report}"
    );
    assert_eq!(
        reference_host.calls, emitted_host.calls,
        "les appels d'hôte divergent{report}"
    );
}

#[test]
fn arithmetic_and_comparisons_agree() {
    for source in [
        "fn run() -> Int { 1 + 2 * 3 }",
        "fn run() -> Int { (1 + 2) * 3 }",
        "fn run() -> Int { 10 - 3 - 2 }",
        "fn run() -> Int { 7 % 3 }",
        "fn run() -> Int { 7 / 2 }",
        "fn run() -> Int { -7 / 2 }",
        "fn run(a: Int) -> Bool { a < 3 }",
        "fn run(a: Int) -> Bool { a <= 3 }",
        "fn run(a: Int) -> Bool { a > 3 }",
        "fn run(a: Int) -> Bool { a >= 3 }",
    ] {
        agree(source, &[Value::Int(3)]);
    }
}

#[test]
fn equality_picks_the_right_width_on_both_sides() {
    agree("fn run() -> Bool { 1 == 1 }", &[]);
    agree(
        "fn run(a: Int, b: Int) -> Bool { a == b }",
        &[Value::Int(2), Value::Int(2)],
    );
    agree(
        "fn run(a: Bool, b: Bool) -> Bool { a != b }",
        &[Value::Bool(true), Value::Bool(false)],
    );
}

#[test]
fn bindings_and_parameters_agree() {
    agree(
        "fn run(a: Int) -> Int { let b: Int = a * 2; let c: Int = b + 1; c }",
        &[Value::Int(5)],
    );
}

#[test]
fn an_if_selects_the_same_branch_on_both_sides() {
    let source = "fn run(a: Int) -> Int { if a > 0 { 1 } else { -1 } }";
    agree(source, &[Value::Int(3)]);
    agree(source, &[Value::Int(-3)]);
}

#[test]
fn negation_of_a_boolean_agrees() {
    agree("fn run(b: Bool) -> Bool { !b }", &[Value::Bool(true)]);
    agree("fn run(b: Bool) -> Bool { !b }", &[Value::Bool(false)]);
}

/// Le court-circuit est la construction la plus facile à émettre de travers :
/// un `i32.and` aurait l'air juste et appellerait la branche droite quand même.
#[test]
fn short_circuit_drops_the_same_calls_on_both_sides() {
    agree("fn run() -> Bool { false && now() > 0 }", &[]);
    agree("fn run() -> Bool { true && now() > 0 }", &[]);
    agree("fn run() -> Bool { true || now() > 0 }", &[]);
    agree("fn run() -> Bool { false || now() > 0 }", &[]);
    agree(
        "fn run(a: Int) -> Bool { a > 0 || now() > 0 }",
        &[Value::Int(-1)],
    );
}

#[test]
fn effects_reach_the_host_in_the_same_order() {
    agree("fn run() -> Unit { emit(\"créé\") }", &[]);
    agree(
        "fn run() -> Unit { emit(\"a\"); emit(\"b\"); emit(\"a\") }",
        &[],
    );
    agree(
        "fn run(b: Bool) -> Unit { if b { emit(\"vrai\") } else { emit(\"faux\") } }",
        &[Value::Bool(true)],
    );
}

/// Une chaîne accentuée traverse la mémoire par ses octets : si la longueur
/// émise était celle des caractères, l'hôte lirait deux octets de trop.
#[test]
fn a_multibyte_string_arrives_whole() {
    agree("fn run() -> Unit { emit(\"éé\") }", &[]);
    agree("fn run() -> Unit { emit(\"aé€\") }", &[]);
}

#[test]
fn division_by_zero_fails_on_both_sides() {
    agree("fn run(a: Int) -> Int { 1 / a }", &[Value::Int(0)]);
    agree("fn run(a: Int) -> Int { 1 % a }", &[Value::Int(0)]);
}

#[test]
fn nested_expressions_agree() {
    let source = "fn run(a: Int, b: Int) -> Int {
        let s: Int = a + b;
        if s > 10 { s * 2 } else { if s > 0 { s } else { -s } }
    }";
    for args in [
        [Value::Int(4), Value::Int(4)],
        [Value::Int(1), Value::Int(2)],
        [Value::Int(-3), Value::Int(-4)],
    ] {
        agree(source, &args);
    }
}
