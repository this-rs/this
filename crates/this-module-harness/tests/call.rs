//! Le primitif d'appel borné, sur de vrais modules wasm assemblés depuis du WAT : chaque
//! refus de `call_json` est provoqué par le module qui le mérite, et chaque succès est lu
//! octet pour octet.
#![cfg(feature = "wasmi")]

use this_module_harness::call::{Args, Call, Limits, PAGE_BYTES, call_json};
use this_module_harness::executor::ExecutorError;

/// Mémoire exportée d'une page + allocateur « bump » à partir de l'octet 1024.
const PRE: &str = r#"
    (memory (export "memory") 1)
    (global $p (mut i32) (i32.const 1024))
    (func (export "alloc") (param $n i32) (result i32)
      (local $r i32)
      (local.set $r (global.get $p))
      (global.set $p (i32.add (global.get $p) (local.get $n)))
      (local.get $r))
"#;

fn module(body: &str) -> Vec<u8> {
    wat::parse_str(format!("(module {PRE} {body})")).unwrap()
}

fn raw(wat: &str) -> Vec<u8> {
    wat::parse_str(wat).unwrap()
}

const ECHO: &str = r#"
    (func (export "echo1") (param $p i32) (param $l i32) (result i64)
      (i64.or (i64.shl (i64.extend_i32_u (local.get $p)) (i64.const 32))
              (i64.extend_i32_u (local.get $l))))
    (func (export "echo2") (param $p1 i32) (param $l1 i32) (param $p2 i32) (param $l2 i32) (result i64)
      (i64.or (i64.shl (i64.extend_i32_u (local.get $p2)) (i64.const 32))
              (i64.extend_i32_u (local.get $l2))))
    (data (i32.const 16) "hello")
    (func (export "hello") (result i64) (i64.const 68719476741)) ;; (16 << 32) | 5
"#;

fn ok(r: Result<Call, ExecutorError>) -> Call {
    r.unwrap_or_else(|e| panic!("attendu un succès : {e}"))
}

fn err(r: Result<Call, ExecutorError>) -> ExecutorError {
    match r {
        Err(e) => e,
        Ok(c) => panic!("attendu un refus, reçu {c:?}"),
    }
}

fn default_call(wasm: &[u8], export: &str, args: &Args<'_>) -> Result<Call, ExecutorError> {
    call_json(wasm, export, args, &Limits::default())
}

// ───────────────────────── bornes et arguments ─────────────────────────

#[test]
fn default_limits_protect_by_default() {
    let l = Limits::default();
    assert_eq!(l.fuel, 2_000_000);
    assert_eq!(l.memory_pages, 256);
    assert_eq!(l.request_bytes, 256 * 1024);
    assert_eq!(l.response_bytes, 1024 * 1024);
    assert_eq!(l.memory_bytes(), 256 * PAGE_BYTES);
    assert_eq!(l.memory_bytes(), 16 * 1024 * 1024);
}

#[test]
fn module_abi_limits_are_the_documented_ones() {
    let l = Limits::module_abi();
    assert_eq!(l.fuel, 50_000_000);
    assert_eq!(l.memory_pages, 64);
    assert_eq!(l.memory_bytes(), 4 * 1024 * 1024);
    assert_eq!(l.request_bytes, 1024 * 1024);
    assert_eq!(l.response_bytes, 4 * 1024 * 1024);
}

#[test]
fn args_hold_their_buffers_in_order() {
    assert!(Args::none().buffers.is_empty());
    let a = Args::one(b"x");
    assert_eq!(a.buffers, vec![&b"x"[..]]);
    let b = Args::two(b"op", b"req");
    assert_eq!(b.buffers, vec![&b"op"[..], &b"req"[..]]);
}

// ───────────────────────── succès ─────────────────────────

#[test]
fn a_call_without_input_reads_the_response_from_memory() {
    let wasm = module(ECHO);
    let c = ok(default_call(&wasm, "hello", &Args::none()));
    assert_eq!(c.response, b"hello");
    assert!(c.fuel_used > 0, "un appel consomme du fuel");
    assert!(c.fuel_used < Limits::default().fuel);
}

#[test]
fn a_call_with_one_input_round_trips_it_through_alloc_and_memory() {
    let wasm = module(ECHO);
    let req = br#"{"hello":"monde","n":[1,2,3]}"#;
    let c = ok(default_call(&wasm, "echo1", &Args::one(req)));
    assert_eq!(c.response, req);
}

#[test]
fn a_call_with_two_inputs_places_each_in_its_own_buffer() {
    let wasm = module(ECHO);
    let c = ok(default_call(
        &wasm,
        "echo2",
        &Args::two(b"create", br#"{"a":1}"#),
    ));
    assert_eq!(c.response, br#"{"a":1}"#);
    // Et le premier est bien celui que le module ne rend pas : les tampons ne se chevauchent pas.
    let c = ok(default_call(&wasm, "echo2", &Args::two(b"create", b"")));
    assert_eq!(c.response, b"");
}

#[test]
fn an_empty_input_is_passed_without_calling_alloc() {
    let wasm = module(ECHO);
    let c = ok(default_call(&wasm, "echo1", &Args::one(b"")));
    assert_eq!(c.response, b"");
}

#[test]
fn each_call_gets_a_fresh_instance() {
    // L'allocateur « bump » repart de 1024 à chaque appel : sans instance neuve, le second
    // appel recevrait un décalage plus loin.
    let wasm = module(ECHO);
    let a = ok(default_call(&wasm, "echo1", &Args::one(b"premier")));
    let b = ok(default_call(&wasm, "echo1", &Args::one(b"second")));
    assert_eq!(
        (a.response.as_slice(), b.response.as_slice()),
        (&b"premier"[..], &b"second"[..])
    );
    assert_eq!(
        a.fuel_used,
        ok(default_call(&wasm, "echo1", &Args::one(b"premier"))).fuel_used
    );
}

// ───────────────────────── refus avant l'instanciation ─────────────────────────

#[test]
fn zero_fuel_is_refused_before_anything_runs() {
    let limits = Limits {
        fuel: 0,
        ..Limits::default()
    };
    let e = err(call_json(&module(ECHO), "hello", &Args::none(), &limits));
    assert!(matches!(e, ExecutorError::OutOfFuel { fuel: 0 }), "{e:?}");
}

#[test]
fn a_zero_memory_cap_is_refused() {
    let limits = Limits {
        memory_pages: 0,
        ..Limits::default()
    };
    let e = err(call_json(&module(ECHO), "hello", &Args::none(), &limits));
    match e {
        ExecutorError::Memory {
            limit_bytes,
            message,
        } => {
            assert_eq!(limit_bytes, 0);
            assert!(message.contains("plafond de mémoire est nul"), "{message}");
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_request_over_the_cap_is_refused_by_total_size() {
    let limits = Limits {
        request_bytes: 5,
        ..Limits::default()
    };
    let e = err(call_json(
        &module(ECHO),
        "echo2",
        &Args::two(b"abc", b"def"),
        &limits,
    ));
    assert!(
        matches!(e, ExecutorError::RequestTooLarge { len: 6, max: 5 }),
        "{e:?}"
    );
    // À la limite exacte, ça passe.
    let limits = Limits {
        request_bytes: 6,
        ..Limits::default()
    };
    assert!(call_json(&module(ECHO), "echo2", &Args::two(b"abc", b"def"), &limits).is_ok());
}

#[test]
fn bytes_that_are_not_wasm_are_refused() {
    let e = err(default_call(
        b"ceci n'est pas du wasm",
        "hello",
        &Args::none(),
    ));
    assert!(matches!(e, ExecutorError::Module(_)), "{e:?}");
}

#[test]
fn a_module_with_imports_is_refused_and_names_them() {
    let wasm = raw(r#"(module
        (import "env" "log" (func (param i32)))
        (import "env" "now" (func (result i64)))
        (memory (export "memory") 1))"#);
    let e = err(default_call(&wasm, "hello", &Args::none()));
    match e {
        ExecutorError::ImportsRefused { count, found } => {
            assert_eq!(count, 2);
            assert_eq!(found, "env.log, env.now");
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_module_without_memory_is_told_how_to_declare_it() {
    let wasm = raw(r#"(module (func (export "alloc") (param i32) (result i32) (i32.const 0)))"#);
    match err(default_call(&wasm, "hello", &Args::none())) {
        ExecutorError::MissingExport { name, how } => {
            assert_eq!(name, "memory");
            assert!(how.contains(r#"(memory (export "memory") 1)"#), "{how}");
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn an_exported_memory_that_is_not_a_memory_is_refused() {
    let wasm = raw(r#"(module (func (export "memory")))"#);
    match err(default_call(&wasm, "hello", &Args::none())) {
        ExecutorError::WrongSignature { name, found, .. } => {
            assert_eq!(name, "memory");
            assert_eq!(found, "un export mémoire qui n'est pas une mémoire");
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_memory_declared_over_the_cap_is_refused_at_load() {
    let wasm = raw(r#"(module
        (memory (export "memory") 3)
        (func (export "alloc") (param i32) (result i32) (i32.const 0)))"#);
    let limits = Limits {
        memory_pages: 2,
        ..Limits::default()
    };
    match err(call_json(&wasm, "hello", &Args::none(), &limits)) {
        ExecutorError::Memory {
            limit_bytes,
            message,
        } => {
            assert_eq!(limit_bytes, 2 * PAGE_BYTES);
            assert!(message.contains("3 pages"), "{message}");
            assert!(message.contains("plafond 2 pages"), "{message}");
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_missing_alloc_is_told_how_to_declare_it() {
    let wasm = raw(r#"(module (memory (export "memory") 1))"#);
    match err(default_call(&wasm, "hello", &Args::none())) {
        ExecutorError::MissingExport { name, how } => {
            assert_eq!(name, "alloc");
            assert!(how.contains("(param i32) (result i32)"), "{how}");
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn an_alloc_with_the_wrong_signature_shows_what_it_has() {
    let wasm = raw(r#"(module
        (memory (export "memory") 1)
        (func (export "alloc") (param i64) (result i32) (i32.const 0)))"#);
    match err(default_call(&wasm, "hello", &Args::none())) {
        ExecutorError::WrongSignature {
            name,
            expected,
            found,
        } => {
            assert_eq!(name, "alloc");
            assert!(expected.contains("(param i32) (result i32)"), "{expected}");
            assert_eq!(found, "(param i64) (result i32)");
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn an_alloc_that_is_not_a_function_is_refused() {
    let wasm = raw(r#"(module
        (memory (export "memory") 1)
        (global (export "alloc") i32 (i32.const 0)))"#);
    match err(default_call(&wasm, "hello", &Args::none())) {
        ExecutorError::WrongSignature { name, found, .. } => {
            assert_eq!(name, "alloc");
            assert_eq!(found, "un export qui n'est pas une fonction");
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_missing_export_is_named_with_the_expected_signature_per_input_count() {
    let wasm = module("");
    match err(default_call(&wasm, "absent", &Args::none())) {
        ExecutorError::MissingExport { name, how } => {
            assert_eq!(name, "absent");
            assert!(
                how.contains(r#"(func (export "absent") (result i64) ...)"#),
                "{how}"
            );
        }
        other => panic!("{other:?}"),
    }
    match err(default_call(&wasm, "absent", &Args::two(b"a", b"b"))) {
        ExecutorError::MissingExport { how, .. } => {
            assert!(
                how.contains(
                    r#"(func (export "absent") (param i32 i32 i32 i32) (result i64) ...)"#
                ),
                "{how}"
            );
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_missing_render_export_points_at_the_view_contract() {
    let wasm = module("");
    match err(default_call(&wasm, "render", &Args::one(b"{}"))) {
        ExecutorError::MissingExport { name, how } => {
            assert_eq!(name, "render");
            assert!(how.contains("contrat de vue"), "{how}");
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn an_export_with_the_wrong_arity_shows_its_signature_as_wat() {
    // Un export `echo1` qui ne prend qu'un `i32` alors qu'il reçoit (ptr, len).
    let wasm = module(r#"(func (export "echo1") (param i32) (result i64) (i64.const 0))"#);
    match err(default_call(&wasm, "echo1", &Args::one(b"x"))) {
        ExecutorError::WrongSignature {
            name,
            expected,
            found,
        } => {
            assert_eq!(name, "echo1");
            assert!(
                expected.contains("(param i32 i32) (result i64)"),
                "{expected}"
            );
            assert_eq!(found, "(param i32) (result i64)");
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn signatures_are_written_for_every_result_shape_and_type() {
    let cases = [
        // Pas de résultat.
        (
            r#"(func (export "x") (param i32 i32))"#,
            "(param i32 i32) (result)",
        ),
        // Un résultat du mauvais type, paramètres flottants.
        (
            r#"(func (export "x") (param f32 f64) (result i32) (i32.const 0))"#,
            "(param f32 f64) (result i32)",
        ),
        // Plusieurs résultats.
        (
            r#"(func (export "x") (param i32 i32) (result i64 i64) (i64.const 0) (i64.const 0))"#,
            "(param i32 i32) (result i64 i64)",
        ),
        // Un type de référence : ni entier ni flottant.
        (
            r#"(func (export "x") (param i32 externref) (result i64) (i64.const 0))"#,
            "(param i32 ?) (result i64)",
        ),
    ];
    for (func, shown) in cases {
        match err(default_call(&module(func), "x", &Args::one(b"x"))) {
            ExecutorError::WrongSignature { found, .. } => assert_eq!(found, shown, "{func}"),
            other => panic!("{func} : {other:?}"),
        }
    }
}

#[test]
fn an_export_that_is_not_a_function_is_refused() {
    let wasm = module(r#"(global (export "x") i32 (i32.const 0))"#);
    match err(default_call(&wasm, "x", &Args::none())) {
        ExecutorError::WrongSignature { name, found, .. } => {
            assert_eq!(name, "x");
            assert_eq!(found, "un export qui n'est pas une fonction");
        }
        other => panic!("{other:?}"),
    }
}

// ───────────────────────── refus pendant l'exécution ─────────────────────────

#[test]
fn an_infinite_loop_runs_out_of_fuel_instead_of_blocking() {
    let wasm = module(r#"(func (export "spin") (result i64) (loop $l (br $l)) (i64.const 0))"#);
    let limits = Limits {
        fuel: 10_000,
        ..Limits::default()
    };
    let e = err(call_json(&wasm, "spin", &Args::none(), &limits));
    assert!(
        matches!(e, ExecutorError::OutOfFuel { fuel: 10_000 }),
        "{e:?}"
    );
}

#[test]
fn a_start_function_that_loops_runs_out_of_fuel_at_instantiation() {
    let wasm = raw(r#"(module
        (memory (export "memory") 1)
        (func (export "alloc") (param i32) (result i32) (i32.const 0))
        (func (export "hello") (result i64) (i64.const 0))
        (func $start (loop $l (br $l)))
        (start $start))"#);
    let limits = Limits {
        fuel: 5_000,
        ..Limits::default()
    };
    let e = err(call_json(&wasm, "hello", &Args::none(), &limits));
    assert!(
        matches!(e, ExecutorError::OutOfFuel { fuel: 5_000 }),
        "{e:?}"
    );
}

#[test]
fn a_start_function_that_traps_is_reported_at_the_instantiation_step() {
    let wasm = raw(r#"(module
        (memory (export "memory") 1)
        (func (export "alloc") (param i32) (result i32) (i32.const 0))
        (func (export "hello") (result i64) (i64.const 0))
        (func $start unreachable)
        (start $start))"#);
    match err(default_call(&wasm, "hello", &Args::none())) {
        ExecutorError::Trap(m) => assert!(m.starts_with("instanciation : "), "{m}"),
        other => panic!("{other:?}"),
    }
}

#[test]
fn growing_memory_past_the_cap_is_a_memory_refusal() {
    let wasm = module(
        r#"(func (export "grow") (result i64) (drop (memory.grow (i32.const 10))) (i64.const 0))"#,
    );
    let limits = Limits {
        memory_pages: 2,
        ..Limits::default()
    };
    match err(call_json(&wasm, "grow", &Args::none(), &limits)) {
        ExecutorError::Memory { limit_bytes, .. } => assert_eq!(limit_bytes, 2 * PAGE_BYTES),
        other => panic!("{other:?}"),
    }
    // Sous le plafond, la même croissance passe.
    let limits = Limits {
        memory_pages: 16,
        ..Limits::default()
    };
    assert!(call_json(&wasm, "grow", &Args::none(), &limits).is_ok());
}

#[test]
fn a_secondary_memory_over_the_cap_is_refused_by_the_store_limits() {
    // Seule la mémoire exportée est comparée au plafond avant l'instanciation : une mémoire
    // interne trop grande est arrêtée par les limites du `Store`. Wasmi la rapporte comme un
    // échec d'instanciation, que `classify` range en `Trap` (et non en `Memory`).
    let wasm =
        module(r#"(memory $interne 100) (func (export "hello") (result i64) (i64.const 0))"#);
    let limits = Limits {
        memory_pages: 4,
        ..Limits::default()
    };
    match err(call_json(&wasm, "hello", &Args::none(), &limits)) {
        ExecutorError::Trap(m) => {
            assert!(m.starts_with("instanciation : "), "{m}");
            assert!(m.contains("resource limiter denied"), "{m}");
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_trap_in_the_export_is_reported_with_the_export_name() {
    let wasm = module(r#"(func (export "boom") (result i64) unreachable)"#);
    match err(default_call(&wasm, "boom", &Args::none())) {
        ExecutorError::Trap(m) => assert!(m.starts_with("boom : "), "{m}"),
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_trap_in_alloc_is_reported_with_the_alloc_step() {
    let wasm = raw(r#"(module
        (memory (export "memory") 1)
        (func (export "alloc") (param i32) (result i32) unreachable)
        (func (export "echo1") (param i32 i32) (result i64) (i64.const 0)))"#);
    match err(default_call(&wasm, "echo1", &Args::one(b"x"))) {
        ExecutorError::Trap(m) => assert!(m.starts_with("alloc : "), "{m}"),
        other => panic!("{other:?}"),
    }
}

fn with_alloc(alloc_result: &str) -> Vec<u8> {
    raw(&format!(
        r#"(module
            (memory (export "memory") 1)
            (func (export "alloc") (param i32) (result i32) {alloc_result})
            (func (export "echo1") (param i32 i32) (result i64) (i64.const 0)))"#
    ))
}

#[test]
fn an_alloc_that_returns_a_negative_offset_is_refused() {
    match err(default_call(
        &with_alloc("(i32.const -8)"),
        "echo1",
        &Args::one(b"abc"),
    )) {
        ExecutorError::BadBuffer {
            ptr,
            len,
            size,
            why,
        } => {
            assert_eq!((ptr, len, size), (-8, 3, PAGE_BYTES));
            assert!(why.contains("n'est pas un décalage"), "{why}");
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn an_alloc_that_returns_an_offset_past_the_memory_is_refused() {
    match err(default_call(
        &with_alloc("(i32.const 65534)"),
        "echo1",
        &Args::one(b"abc"),
    )) {
        ExecutorError::BadBuffer {
            ptr,
            len,
            size,
            why,
        } => {
            assert_eq!((ptr, len, size), (65534, 3, PAGE_BYTES));
            assert!(why.contains("dépasse la mémoire du module"), "{why}");
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_response_over_the_cap_is_refused_with_its_length() {
    let limits = Limits {
        response_bytes: 4,
        ..Limits::default()
    };
    let e = err(call_json(
        &module(ECHO),
        "echo1",
        &Args::one(b"abcdef"),
        &limits,
    ));
    assert!(
        matches!(e, ExecutorError::ResponseTooLarge { len: 6, max: 4 }),
        "{e:?}"
    );
}

#[test]
fn a_response_cap_above_the_memory_size_is_bounded_by_the_memory() {
    // Le module annonce 70 000 octets de réponse dans une mémoire de 65 536 : refusé comme
    // trop grand, plafonné à la taille de la mémoire et non au plafond de 1 Mio.
    let wasm = module(
        r#"(func (export "big") (result i64)
             (i64.or (i64.shl (i64.const 0) (i64.const 32)) (i64.const 70000)))"#,
    );
    let e = err(default_call(&wasm, "big", &Args::none()));
    assert!(
        matches!(e, ExecutorError::ResponseTooLarge { len: 70_000, max } if max == PAGE_BYTES),
        "{e:?}"
    );
}

#[test]
fn a_response_pointing_outside_the_memory_is_refused() {
    let wasm = module(
        r#"(func (export "far") (result i64)
             (i64.or (i64.shl (i64.const 65530) (i64.const 32)) (i64.const 100)))"#,
    );
    match err(default_call(&wasm, "far", &Args::none())) {
        ExecutorError::BadBuffer {
            ptr,
            len,
            size,
            why,
        } => {
            assert_eq!((ptr, len, size), (65530, 100, PAGE_BYTES));
            assert!(why.contains("dépasse la mémoire du module"), "{why}");
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_response_at_the_very_end_of_the_memory_is_read() {
    let wasm = module(
        r#"(data (i32.const 65531) "fin!!")
           (func (export "tail") (result i64)
             (i64.or (i64.shl (i64.const 65531) (i64.const 32)) (i64.const 5)))"#,
    );
    assert_eq!(
        ok(default_call(&wasm, "tail", &Args::none())).response,
        b"fin!!"
    );
}
