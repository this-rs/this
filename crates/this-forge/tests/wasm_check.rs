//! Le contrôle ABI avant signature, sur de vrais binaires assemblés depuis du WAT :
//! chaque refus est provoqué par le module qui le mérite, et le message est affirmé.

use this_forge::wasm_check::{AbiViolation, check_abi};

fn wasm(wat_src: &str) -> Vec<u8> {
    wat::parse_str(wat_src).unwrap()
}

fn messages(r: Result<(), Vec<AbiViolation>>) -> Vec<String> {
    r.unwrap_err().into_iter().map(|v| v.0).collect()
}

const GOOD: &str = r#"(module
    (memory (export "memory") 1)
    (func (export "alloc") (param i32) (result i32) (i32.const 0))
    (func (export "manifest") (result i64) (i64.const 0))
    (func (export "handle") (param i32 i32 i32 i32) (result i64) (i64.const 0)))"#;

#[test]
fn a_conforming_module_is_accepted() {
    assert_eq!(check_abi(&wasm(GOOD)), Ok(()));
}

#[test]
fn bytes_that_are_not_wasm_give_one_invalid_binary_violation() {
    let m = messages(check_abi(b"ceci n'est pas du wasm"));
    assert_eq!(m.len(), 1);
    assert!(m[0].starts_with("binaire wasm invalide : "), "{}", m[0]);
}

#[test]
fn an_empty_input_is_not_a_valid_binary() {
    let m = messages(check_abi(b""));
    assert_eq!(m.len(), 1);
    assert!(m[0].starts_with("binaire wasm invalide : "), "{}", m[0]);
}

#[test]
fn a_truncated_module_is_reported_as_invalid() {
    let mut bytes = wasm(GOOD);
    bytes.truncate(bytes.len() - 3);
    let m = messages(check_abi(&bytes));
    assert_eq!(m.len(), 1);
    assert!(m[0].starts_with("binaire wasm invalide : "), "{}", m[0]);
}

#[test]
fn a_header_only_module_lacks_every_export() {
    let m = messages(check_abi(&wasm("(module)")));
    assert_eq!(
        m,
        vec![
            "export `memory` manquant",
            "export `alloc` manquant",
            "export `manifest` manquant",
            "export `handle` manquant",
        ]
    );
}

#[test]
fn imports_are_listed_and_function_imports_shift_the_index_space() {
    // L'import de fonction occupe l'index 0 : `alloc` (index 1) garde donc son type.
    let m = messages(check_abi(&wasm(
        r#"(module
            (import "env" "log" (func (param i32)))
            (import "env" "mem" (memory 1))
            (memory (export "memory") 1)
            (func (export "alloc") (param i32) (result i32) (i32.const 0))
            (func (export "manifest") (result i64) (i64.const 0))
            (func (export "handle") (param i32 i32 i32 i32) (result i64) (i64.const 0)))"#,
    )));
    assert_eq!(
        m,
        vec!["le module déclare des imports (env::log, env::mem) : l'ABI v1 n'en admet aucun"]
    );
}

#[test]
fn a_memory_export_that_is_not_a_memory_is_refused() {
    let m = messages(check_abi(&wasm(
        r#"(module
            (func (export "memory"))
            (func (export "alloc") (param i32) (result i32) (i32.const 0))
            (func (export "manifest") (result i64) (i64.const 0))
            (func (export "handle") (param i32 i32 i32 i32) (result i64) (i64.const 0)))"#,
    )));
    assert_eq!(m, vec!["`memory` est exporté mais n'est pas une mémoire"]);
}

#[test]
fn an_export_with_the_wrong_signature_shows_what_was_found_and_expected() {
    let m = messages(check_abi(&wasm(
        r#"(module
            (memory (export "memory") 1)
            (func (export "alloc") (param i64) (result i32) (i32.const 0))
            (func (export "manifest") (result i64) (i64.const 0))
            (func (export "handle") (param i32 i32) (result i64) (i64.const 0)))"#,
    )));
    assert_eq!(m.len(), 2, "{m:?}");
    assert!(m[0].starts_with("export `alloc` : signature "), "{}", m[0]);
    assert!(m[0].contains("attendu [I32] -> [I32]"), "{}", m[0]);
    assert!(m[0].contains("[I64]"), "{}", m[0]);
    assert!(m[1].starts_with("export `handle` : signature "), "{}", m[1]);
    assert!(
        m[1].contains("attendu [I32, I32, I32, I32] -> [I64]"),
        "{}",
        m[1]
    );
}

#[test]
fn an_abi_name_exported_as_something_else_than_a_function_is_refused() {
    let m = messages(check_abi(&wasm(
        r#"(module
            (memory (export "memory") 1)
            (global (export "alloc") i32 (i32.const 0))
            (func (export "manifest") (result i64) (i64.const 0))
            (func (export "handle") (param i32 i32 i32 i32) (result i64) (i64.const 0)))"#,
    )));
    assert_eq!(m, vec!["`alloc` est exporté mais n'est pas une fonction"]);
}

#[test]
fn a_valid_module_without_memory_only_misses_the_memory() {
    let m = messages(check_abi(&wasm(
        r#"(module
            (func (export "alloc") (param i32) (result i32) (i32.const 0))
            (func (export "manifest") (result i64) (i64.const 0))
            (func (export "handle") (param i32 i32 i32 i32) (result i64) (i64.const 0)))"#,
    )));
    assert_eq!(m, vec!["export `memory` manquant"]);
}

#[test]
fn a_non_function_type_in_the_type_section_does_not_break_the_check() {
    // Un type `struct` (GC) occupe l'index 0 de la section type : il est rangé comme
    // « sans paramètre ni résultat », et les fonctions suivantes gardent leur type.
    let r = check_abi(&wasm(
        r#"(module
            (type (struct))
            (memory (export "memory") 1)
            (func (export "alloc") (param i32) (result i32) (i32.const 0))
            (func (export "manifest") (result i64) (i64.const 0))
            (func (export "handle") (param i32 i32 i32 i32) (result i64) (i64.const 0)))"#,
    ));
    assert_eq!(r, Ok(()));
}

#[test]
fn several_violations_are_all_reported_in_order() {
    let m = messages(check_abi(&wasm(
        r#"(module
            (import "env" "f" (func))
            (func (export "manifest") (param i32) (result i64) (i64.const 0)))"#,
    )));
    assert_eq!(m.len(), 5, "{m:?}");
    assert!(m[0].starts_with("le module déclare des imports (env::f)"));
    assert_eq!(m[1], "export `memory` manquant");
    assert_eq!(m[2], "export `alloc` manquant");
    assert!(
        m[3].starts_with("export `manifest` : signature "),
        "{}",
        m[3]
    );
    assert_eq!(m[4], "export `handle` manquant");
}

#[test]
fn an_abnormally_large_module_is_still_checked() {
    // 1 Mio de données : la taille n'est pas un critère, seule l'ABI l'est.
    let big = "x".repeat(1 << 20);
    let src = GOOD.replace(
        "(memory (export \"memory\") 1)",
        &format!("(memory (export \"memory\") 32) (data (i32.const 0) \"{big}\")"),
    );
    let bytes = wasm(&src);
    assert!(bytes.len() > 1 << 20);
    assert_eq!(check_abi(&bytes), Ok(()));
}
