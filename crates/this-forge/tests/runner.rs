//! Le runner de build, sans docker.
//!
//! `ForgeConfig::docker_bin` est un réglage : on y met un faux exécutable `docker` (un script
//! `sh`) qui joue le rôle du conteneur — il lit ce que le runner lui passe (arguments, arbre
//! monté en `/crate`, répertoire de sortie `/out`) et rend ce qu'un vrai build rendrait
//! (JSON cargo sur stdout, `module.wasm` dans `/out`, code de sortie). Le runner, lui, est le
//! vrai : écriture de l'arbre, gabarit imposé, ligne de commande, délai, lecture et épreuve
//! de l'artefact.
//!
//! Le script est GÉNÉRIQUE et piloté par des fichiers posés à côté de lui :
//! `module.wasm` (copié dans `/out`), `stdout.txt`, `stderr.txt`, `code` (code de sortie),
//! `sleep` (secondes), `toolchain` (réponse à `rustc -V`). Il journalise ses arguments dans
//! `args.log` et recopie l'arbre reçu dans `dump/`.
use std::os::unix::fs::PermissionsExt;
use std::time::Duration;

use serde_json::json;
use tempfile::TempDir;
use this_forge::ForgeConfig;
use this_forge::runner::{
    BuildOutcome, BuildRunner, Diagnostic, SourceFile, TEMPLATE_CARGO_LOCK, TEMPLATE_CARGO_TOML,
    parse_cargo_json, valid_module_name, valid_version, validate_files,
};

/// Écrire un script exécutable puis l'exécuter depuis plusieurs fils expose à `ETXTBSY`
/// (un `fork` concurrent hérite du descripteur encore ouvert) : on sérialise ces tests.
static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

const SCRIPT: &str = r#"#!/bin/sh
D="@DIR@"
echo "--INVOCATION--" >> "$D/args.log"
for a in "$@"; do echo "$a" >> "$D/args.log"; done
if [ "$1" = "kill" ]; then touch "$D/killed"; exit 0; fi
case " $* " in
  *" rustc -V "*)
    echo x >> "$D/toolchain_calls"
    if [ -f "$D/toolchain" ]; then cat "$D/toolchain"; exit 0; fi
    exit 1;;
esac
out=""; crate=""
for a in "$@"; do
  case "$a" in
    *:/out:rw) out="${a%:/out:rw}";;
    *:/crate:ro) crate="${a%:/crate:ro}";;
  esac
done
mkdir -p "$D/dump"
cp -R "$crate/." "$D/dump/"
if [ -f "$D/sleep" ]; then sleep "$(cat "$D/sleep")"; fi
if [ -f "$D/module.wasm" ]; then cp "$D/module.wasm" "$out/module.wasm"; fi
if [ -f "$D/stdout.txt" ]; then cat "$D/stdout.txt"; fi
if [ -f "$D/stderr.txt" ]; then cat "$D/stderr.txt" >&2; fi
if [ -f "$D/code" ]; then exit "$(cat "$D/code")"; fi
exit 0
"#;

/// Un module qui respecte l'ABI v1 : `memory`, `alloc`, `manifest`, `handle`, aucun import.
fn abi_wasm() -> Vec<u8> {
    wat::parse_str(
        r#"(module
            (memory (export "memory") 1)
            (func (export "alloc") (param i32) (result i32) local.get 0)
            (func (export "manifest") (result i64) i64.const 0)
            (func (export "handle") (param i32 i32 i32 i32) (result i64) i64.const 0))"#,
    )
    .unwrap()
}

struct Fake {
    dir: TempDir,
    cfg: ForgeConfig,
}

impl Fake {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join("docker");
        let script = SCRIPT.replace("@DIR@", dir.path().to_str().unwrap());
        std::fs::write(&bin, script).unwrap();
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
        let mut cfg = ForgeConfig::new(dir.path().join("data"));
        cfg.docker_bin = bin.to_str().unwrap().to_string();
        Self { dir, cfg }
    }
    fn put(&self, name: &str, content: impl AsRef<[u8]>) {
        std::fs::write(self.dir.path().join(name), content).unwrap();
    }
    fn args(&self) -> Vec<Vec<String>> {
        let log = std::fs::read_to_string(self.dir.path().join("args.log")).unwrap_or_default();
        let mut calls: Vec<Vec<String>> = Vec::new();
        for l in log.lines() {
            if l == "--INVOCATION--" {
                calls.push(Vec::new());
            } else if let Some(c) = calls.last_mut() {
                c.push(l.to_string());
            }
        }
        calls
    }
    fn dumped(&self, rel: &str) -> String {
        std::fs::read_to_string(self.dir.path().join("dump").join(rel)).unwrap()
    }
    fn runner(&self) -> BuildRunner {
        BuildRunner::new(self.cfg.clone())
    }
}

fn lib(content: &str) -> Vec<SourceFile> {
    vec![SourceFile {
        path: "src/lib.rs".into(),
        content: content.into(),
    }]
}

fn flag_value<'a>(args: &'a [String], flag: &str) -> &'a str {
    let i = args
        .iter()
        .position(|a| a == flag)
        .unwrap_or_else(|| panic!("option {flag} absente de {args:?}"));
    &args[i + 1]
}

fn compiler_message(level: &str, code: Option<&str>, text: &str, line: u32) -> String {
    json!({
        "reason": "compiler-message",
        "message": {
            "level": level,
            "message": text,
            "code": code.map(|c| json!({ "code": c })),
            "spans": [{
                "file_name": "src/lib.rs", "line_start": line, "column_start": 9,
                "is_primary": true
            }]
        }
    })
    .to_string()
}

// ───────────────────────── validation (pur) ─────────────────────────

fn cfg() -> ForgeConfig {
    ForgeConfig::new("/nulle/part")
}

fn file(path: &str, content: &str) -> SourceFile {
    SourceFile {
        path: path.into(),
        content: content.into(),
    }
}

#[test]
fn validate_accepts_a_tree_with_a_lib_root_and_nested_modules() {
    let files = vec![
        file("src/lib.rs", "mod a;"),
        file("src/a/b.rs", ""),
        file("src/x-y_z.1.rs", ""),
    ];
    assert_eq!(validate_files(&files, &cfg()), Ok(()));
}

#[test]
fn validate_refuses_an_empty_submission() {
    let (code, msg) = validate_files(&[], &cfg()).unwrap_err();
    assert_eq!(code, "invalid_files");
    assert_eq!(msg, "aucun fichier");
}

#[test]
fn validate_refuses_too_many_files() {
    let mut c = cfg();
    c.max_files = 2;
    let files = vec![
        file("src/lib.rs", ""),
        file("src/a.rs", ""),
        file("src/b.rs", ""),
    ];
    let (code, msg) = validate_files(&files, &c).unwrap_err();
    assert_eq!(code, "invalid_files");
    assert_eq!(msg, "3 fichiers, maximum 2");
}

#[test]
fn validate_refuses_oversized_sources_by_total_bytes() {
    let mut c = cfg();
    c.max_source_bytes = 10;
    let files = vec![file("src/lib.rs", "123456"), file("src/a.rs", "123456")];
    let (code, msg) = validate_files(&files, &c).unwrap_err();
    assert_eq!(code, "invalid_files");
    assert_eq!(msg, "sources trop volumineuses : 12 octets, maximum 10");
    // La limite est inclusive : exactement 12 octets passent.
    c.max_source_bytes = 12;
    assert_eq!(validate_files(&files, &c), Ok(()));
}

#[test]
fn validate_refuses_every_forbidden_path_shape() {
    for bad in [
        "Cargo.toml",         // hors de src/
        "build.rs",           // idem
        ".cargo/config.toml", // configuration cargo
        "src/lib.txt",        // pas .rs
        "src/../escape.rs",   // remontée
        "src/./a.rs",         // segment point
        "src//a.rs",          // segment vide
        "src/é.rs",           // hors ASCII
        "src/a b.rs",         // espace
        "/src/lib.rs",        // absolu
        "src/a.rs/",          // finit par /
    ] {
        let files = vec![file("src/lib.rs", ""), file(bad, "")];
        let (code, msg) =
            validate_files(&files, &cfg()).expect_err(&format!("`{bad}` aurait dû être refusé"));
        assert_eq!(code, "invalid_files", "{bad}");
        assert!(
            msg.starts_with(&format!("chemin refusé : `{bad}`")),
            "{msg}"
        );
    }
}

#[test]
fn validate_refuses_duplicate_paths() {
    let files = vec![file("src/lib.rs", "a"), file("src/lib.rs", "b")];
    let (code, msg) = validate_files(&files, &cfg()).unwrap_err();
    assert_eq!(code, "invalid_files");
    assert_eq!(msg, "chemin en double : `src/lib.rs`");
}

#[test]
fn validate_requires_the_lib_root() {
    let (code, msg) = validate_files(&[file("src/a.rs", "")], &cfg()).unwrap_err();
    assert_eq!(code, "invalid_files");
    assert!(msg.starts_with("`src/lib.rs` manque"), "{msg}");
}

#[test]
fn module_names_follow_the_stable_pattern() {
    assert!(valid_module_name("a"));
    assert!(valid_module_name("taches"));
    assert!(valid_module_name("mon-module_2"));
    assert!(valid_module_name(&format!("a{}", "b".repeat(63)))); // 64 : la limite
    assert!(!valid_module_name(&format!("a{}", "b".repeat(64)))); // 65
    assert!(!valid_module_name(""));
    assert!(!valid_module_name("1abc"));
    assert!(!valid_module_name("-abc"));
    assert!(!valid_module_name("Abc"));
    assert!(!valid_module_name("ab c"));
    assert!(!valid_module_name("abC"));
}

#[test]
fn versions_are_three_numeric_parts_with_optional_suffix() {
    for ok in [
        "0.0.1",
        "1.2.3",
        "10.20.30",
        "1.0.0-rc.1",
        "1.0.0+build.5",
        "1.0.0-a+b",
    ] {
        assert!(valid_version(ok), "{ok}");
    }
    for bad in [
        "",
        "1",
        "1.2",
        "1.2.3.4",
        "1.2.",
        ".1.2",
        "a.b.c",
        "1.2.x",
        "1.0.0-é",
        "1.0.0 ",
        "1.0.0-a b",
    ] {
        assert!(!valid_version(bad), "{bad:?}");
    }
    assert!(!valid_version(&format!("1.0.0-{}", "a".repeat(60)))); // dépasse 64
    assert!(valid_version(&format!("1.0.0-{}", "a".repeat(58)))); // 64 pile
}

#[test]
fn diagnostic_error_has_no_position() {
    let d = Diagnostic::error("boum");
    assert_eq!(
        d,
        Diagnostic {
            file: String::new(),
            line: 0,
            column: 0,
            level: "error".into(),
            message: "boum".into(),
        }
    );
}

// ───────────────────────── parse_cargo_json (pur) ─────────────────────────

#[test]
fn parse_reads_position_level_and_code() {
    let out = parse_cargo_json(&compiler_message(
        "error",
        Some("E0308"),
        "mismatched types",
        3,
    ));
    assert_eq!(
        out,
        vec![Diagnostic {
            file: "src/lib.rs".into(),
            line: 3,
            column: 9,
            level: "error".into(),
            message: "E0308: mismatched types".into(),
        }]
    );
}

#[test]
fn parse_without_code_keeps_the_bare_message() {
    let out = parse_cargo_json(&compiler_message("warning", None, "unused variable", 7));
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].level, "warning");
    assert_eq!(out[0].message, "unused variable");
}

#[test]
fn parse_ignores_noise_lines_and_other_reasons_and_levels() {
    let stdout = [
        "pas du json".to_string(),
        String::new(),
        json!({"reason": "compiler-artifact"}).to_string(),
        json!({"reason": "build-finished", "success": false}).to_string(),
        compiler_message("note", None, "une note", 1),
        compiler_message("help", None, "une aide", 1),
        compiler_message("error", Some("E0001"), "le seul retenu", 2),
    ]
    .join("\n");
    let out = parse_cargo_json(&stdout);
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].message, "E0001: le seul retenu");
}

#[test]
fn parse_prefers_the_primary_span_and_falls_back_to_the_first() {
    let primary_second = json!({
        "reason": "compiler-message",
        "message": {"level": "error", "message": "m", "spans": [
            {"file_name": "src/a.rs", "line_start": 1, "column_start": 1, "is_primary": false},
            {"file_name": "src/b.rs", "line_start": 9, "column_start": 4, "is_primary": true}
        ]}
    })
    .to_string();
    let out = parse_cargo_json(&primary_second);
    assert_eq!(
        (out[0].file.as_str(), out[0].line, out[0].column),
        ("src/b.rs", 9, 4)
    );

    let no_primary = json!({
        "reason": "compiler-message",
        "message": {"level": "warning", "message": "m", "spans": [
            {"file_name": "src/a.rs", "line_start": 2, "column_start": 5, "is_primary": false},
            {"file_name": "src/b.rs", "line_start": 9, "column_start": 4, "is_primary": false}
        ]}
    })
    .to_string();
    let out = parse_cargo_json(&no_primary);
    assert_eq!(
        (out[0].file.as_str(), out[0].line, out[0].column),
        ("src/a.rs", 2, 5)
    );
}

#[test]
fn parse_drops_summaries_without_position_but_keeps_other_spanless_messages() {
    let spanless = |level: &str, text: &str| {
        json!({
            "reason": "compiler-message",
            "message": {"level": level, "message": text, "spans": []}
        })
        .to_string()
    };
    let stdout = [
        spanless("error", "aborting due to 2 previous errors"),
        spanless("warning", "2 warnings emitted: 1 generated"),
        spanless("error", "linking with `rust-lld` failed"),
        spanless("warning", "trop de choses"),
    ]
    .join("\n");
    let out = parse_cargo_json(&stdout);
    assert_eq!(out.len(), 2);
    assert_eq!(out[0].message, "linking with `rust-lld` failed");
    assert_eq!(
        (out[0].file.as_str(), out[0].line, out[0].column),
        ("", 0, 0)
    );
    assert_eq!(out[1].level, "warning");
}

#[test]
fn parse_tolerates_a_message_without_spans_field_or_level() {
    let no_spans = json!({
        "reason": "compiler-message",
        "message": {"level": "error", "message": "sans spans"}
    })
    .to_string();
    let no_level = json!({
        "reason": "compiler-message",
        "message": {"message": "sans niveau", "spans": []}
    })
    .to_string();
    let out = parse_cargo_json(&format!("{no_spans}\n{no_level}"));
    // Sans niveau, c'est une erreur par défaut ; sans `spans`, pas de position.
    assert_eq!(out.len(), 2);
    assert_eq!(out[0].level, "error");
    assert_eq!(out[1].level, "error");
    assert_eq!(out[1].message, "sans niveau");
}

#[test]
fn parse_stops_at_one_hundred_diagnostics() {
    let stdout = (0..250)
        .map(|i| compiler_message("warning", None, &format!("w{i}"), i))
        .collect::<Vec<_>>()
        .join("\n");
    let out = parse_cargo_json(&stdout);
    assert_eq!(out.len(), 100);
    assert_eq!(out[99].message, "w99");
}

// ───────────────────────── run (faux docker) ─────────────────────────

#[tokio::test]
async fn a_good_build_returns_the_wasm_and_its_warnings() {
    let _g = SERIAL.lock().await;
    let fake = Fake::new();
    let wasm = abi_wasm();
    fake.put("module.wasm", &wasm);
    fake.put(
        "stdout.txt",
        format!(
            "{}\n{}\n",
            compiler_message(
                "warning",
                Some("unused_variables"),
                "unused variable: `x`",
                4
            ),
            json!({"reason": "build-finished", "success": true})
        ),
    );
    let outcome = fake.runner().run(&lib("pub fn f() {}\n")).await;
    match outcome {
        BuildOutcome::Wasm {
            bytes, diagnostics, ..
        } => {
            assert_eq!(bytes, wasm);
            assert_eq!(diagnostics.len(), 1);
            assert_eq!(diagnostics[0].level, "warning");
            assert_eq!(
                diagnostics[0].message,
                "unused_variables: unused variable: `x`"
            );
        }
        other => panic!("attendu Wasm, reçu {other:?}"),
    }
}

#[tokio::test]
async fn the_runner_imposes_the_template_and_the_container_limits() {
    let _g = SERIAL.lock().await;
    let mut fake = Fake::new();
    fake.cfg.cpus = 3;
    fake.cfg.memory = "512m".into();
    fake.cfg.cpu_shares = 200;
    fake.cfg.blkio_weight = 50;
    fake.cfg.image = "mon-image:1".into();
    fake.put("module.wasm", abi_wasm());
    let files = vec![
        file("src/lib.rs", "mod sous;"),
        file("src/sous/mod.rs", "pub fn g() {}"),
    ];
    let outcome = fake.runner().run(&files).await;
    assert!(matches!(outcome, BuildOutcome::Wasm { .. }), "{outcome:?}");

    // Ce que le conteneur a reçu : le gabarit imposé et les sources, rien d'autre.
    assert_eq!(fake.dumped("Cargo.toml"), TEMPLATE_CARGO_TOML);
    assert_eq!(fake.dumped("Cargo.lock"), TEMPLATE_CARGO_LOCK);
    assert_eq!(fake.dumped("src/lib.rs"), "mod sous;");
    assert_eq!(fake.dumped("src/sous/mod.rs"), "pub fn g() {}");

    // La ligne de commande : sans réseau, système de fichiers en lecture seule, capacités
    // retirées, limites de la configuration, utilisateur non-root de la forge.
    let calls = fake.args();
    assert_eq!(calls.len(), 1);
    let a = &calls[0];
    assert_eq!(a[0], "run");
    assert!(a.contains(&"--rm".to_string()));
    assert!(a.contains(&"--read-only".to_string()));
    assert_eq!(flag_value(a, "--network"), "none");
    assert_eq!(flag_value(a, "--cap-drop"), "ALL");
    assert_eq!(flag_value(a, "--security-opt"), "no-new-privileges");
    assert_eq!(flag_value(a, "--cpus"), "3");
    assert_eq!(flag_value(a, "--cpu-shares"), "200");
    assert_eq!(flag_value(a, "--blkio-weight"), "50");
    assert_eq!(flag_value(a, "--memory"), "512m");
    assert_eq!(flag_value(a, "--memory-swap"), "512m");
    assert_eq!(flag_value(a, "--pids-limit"), "256");
    assert!(flag_value(a, "--name").starts_with("forge-"));
    let (uid, gid) = flag_value(a, "--user").split_once(':').unwrap();
    assert!(uid.parse::<u32>().is_ok() && gid.parse::<u32>().is_ok());
    assert_eq!(flag_value(a, "-w"), "/crate");
    assert!(a.contains(&"CARGO_TARGET_DIR=/tmp/target".to_string()));
    assert!(a.contains(&"mon-image:1".to_string()));
    let script = a.last().unwrap();
    assert!(script.contains("cargo build --offline --locked --release"));
    assert!(script.contains("--target wasm32-unknown-unknown"));
    assert_eq!(
        a[a.len() - 3..a.len() - 1],
        ["sh".to_string(), "-c".to_string()]
    );
}

#[tokio::test]
async fn extra_files_land_after_the_template_without_validation() {
    let _g = SERIAL.lock().await;
    let fake = Fake::new();
    fake.put("module.wasm", abi_wasm());
    let outcome = fake
        .runner()
        .run_with_extra(&lib("pub fn f() {}"), &[("build.rs", "fn main() {}")])
        .await;
    assert!(matches!(outcome, BuildOutcome::Wasm { .. }), "{outcome:?}");
    assert_eq!(fake.dumped("build.rs"), "fn main() {}");
}

#[tokio::test]
async fn a_compile_error_is_returned_as_structured_diagnostics() {
    let _g = SERIAL.lock().await;
    let fake = Fake::new();
    fake.put(
        "stdout.txt",
        format!(
            "{}\n",
            compiler_message("error", Some("E0308"), "mismatched types", 2)
        ),
    );
    fake.put("code", "101");
    match fake.runner().run(&lib("x")).await {
        BuildOutcome::Failed { diagnostics, .. } => {
            assert_eq!(
                diagnostics,
                vec![Diagnostic {
                    file: "src/lib.rs".into(),
                    line: 2,
                    column: 9,
                    level: "error".into(),
                    message: "E0308: mismatched types".into(),
                }]
            );
        }
        other => panic!("attendu Failed, reçu {other:?}"),
    }
}

#[tokio::test]
async fn a_failure_without_structured_error_reports_the_end_of_stderr() {
    let _g = SERIAL.lock().await;
    let fake = Fake::new();
    fake.put("stderr.txt", "  le conteneur a manqué de mémoire\n");
    fake.put("code", "137");
    match fake.runner().run(&lib("x")).await {
        BuildOutcome::Failed { diagnostics, .. } => {
            assert_eq!(diagnostics.len(), 1);
            assert_eq!(diagnostics[0].level, "error");
            assert_eq!(diagnostics[0].message, "le conteneur a manqué de mémoire");
        }
        other => panic!("attendu Failed, reçu {other:?}"),
    }
}

#[tokio::test]
async fn a_long_stderr_is_cut_to_its_last_two_thousand_characters() {
    let _g = SERIAL.lock().await;
    let fake = Fake::new();
    let long = format!("{}{}", "a".repeat(5000), "FIN");
    fake.put("stderr.txt", &long);
    fake.put("code", "1");
    match fake.runner().run(&lib("x")).await {
        BuildOutcome::Failed { diagnostics, .. } => {
            let m = &diagnostics[0].message;
            assert_eq!(m.chars().count(), 2000);
            assert!(m.ends_with("aaFIN"), "{}", &m[m.len() - 10..]);
        }
        other => panic!("attendu Failed, reçu {other:?}"),
    }
}

#[tokio::test]
async fn warnings_alone_do_not_hide_a_failed_exit_code() {
    let _g = SERIAL.lock().await;
    let fake = Fake::new();
    fake.put(
        "stdout.txt",
        format!(
            "{}\n",
            compiler_message("warning", None, "juste un avertissement", 1)
        ),
    );
    fake.put("stderr.txt", "link error");
    fake.put("code", "1");
    match fake.runner().run(&lib("x")).await {
        BuildOutcome::Failed { diagnostics, .. } => {
            // L'avertissement est gardé, et une erreur explicative est ajoutée.
            assert_eq!(diagnostics.len(), 2);
            assert_eq!(diagnostics[0].level, "warning");
            assert_eq!(diagnostics[1].level, "error");
            assert_eq!(diagnostics[1].message, "link error");
        }
        other => panic!("attendu Failed, reçu {other:?}"),
    }
}

#[tokio::test]
async fn a_successful_build_without_artifact_is_a_failure() {
    let _g = SERIAL.lock().await;
    let fake = Fake::new(); // pas de module.wasm, code 0
    match fake.runner().run(&lib("x")).await {
        BuildOutcome::Failed { diagnostics, .. } => {
            assert_eq!(diagnostics.len(), 1);
            assert_eq!(
                diagnostics[0].message,
                "le build a réussi mais n'a produit aucun module.wasm"
            );
        }
        other => panic!("attendu Failed, reçu {other:?}"),
    }
}

#[tokio::test]
async fn an_oversized_artifact_is_refused_with_its_size() {
    let _g = SERIAL.lock().await;
    let mut fake = Fake::new();
    let wasm = abi_wasm();
    fake.cfg.max_wasm_bytes = wasm.len() - 1;
    fake.put("module.wasm", &wasm);
    match fake.runner().run(&lib("x")).await {
        BuildOutcome::Failed { diagnostics, .. } => {
            assert_eq!(
                diagnostics[0].message,
                format!(
                    "artefact trop gros : {} octets, maximum {}",
                    wasm.len(),
                    wasm.len() - 1
                )
            );
        }
        other => panic!("attendu Failed, reçu {other:?}"),
    }
}

#[tokio::test]
async fn an_artifact_at_the_size_limit_passes() {
    let _g = SERIAL.lock().await;
    let mut fake = Fake::new();
    let wasm = abi_wasm();
    fake.cfg.max_wasm_bytes = wasm.len();
    fake.put("module.wasm", &wasm);
    assert!(matches!(
        fake.runner().run(&lib("x")).await,
        BuildOutcome::Wasm { .. }
    ));
}

#[tokio::test]
async fn an_artifact_that_breaks_the_abi_is_refused_with_each_violation() {
    let _g = SERIAL.lock().await;
    let fake = Fake::new();
    fake.put("module.wasm", wat::parse_str("(module)").unwrap());
    match fake.runner().run(&lib("x")).await {
        BuildOutcome::Failed { diagnostics, .. } => {
            let msgs: Vec<&str> = diagnostics.iter().map(|d| d.message.as_str()).collect();
            assert_eq!(
                msgs,
                [
                    "ABI : export `memory` manquant",
                    "ABI : export `alloc` manquant",
                    "ABI : export `manifest` manquant",
                    "ABI : export `handle` manquant",
                ]
            );
        }
        other => panic!("attendu Failed, reçu {other:?}"),
    }
}

#[tokio::test]
async fn a_build_over_the_time_limit_is_cut_and_the_container_killed() {
    let _g = SERIAL.lock().await;
    let mut fake = Fake::new();
    fake.cfg.build_timeout = Duration::from_millis(300);
    fake.put("sleep", "5");
    fake.put("module.wasm", abi_wasm());
    let started = std::time::Instant::now();
    match fake.runner().run(&lib("x")).await {
        BuildOutcome::Failed { diagnostics, .. } => {
            assert_eq!(
                diagnostics[0].message,
                "build interrompu : durée maximale de 0 s dépassée"
            );
        }
        other => panic!("attendu Failed, reçu {other:?}"),
    }
    assert!(
        started.elapsed() < Duration::from_secs(4),
        "le délai n'a pas coupé"
    );
    // Le runner a tué le conteneur par son nom (kill_on_drop ne tue que le client docker).
    assert!(fake.dir.path().join("killed").exists());
    let calls = fake.args();
    let run_name = flag_value(&calls[0], "--name").to_string();
    assert_eq!(calls[1], ["kill".to_string(), run_name]);
}

#[tokio::test]
async fn a_missing_docker_binary_is_reported_as_a_launch_failure() {
    let mut cfg = ForgeConfig::new("/nulle/part");
    cfg.docker_bin = "/chemin/qui/n/existe/pas/docker".into();
    match BuildRunner::new(cfg).run(&lib("x")).await {
        BuildOutcome::Failed { diagnostics, .. } => {
            assert_eq!(diagnostics.len(), 1);
            assert!(
                diagnostics[0]
                    .message
                    .starts_with("la forge n'a pas pu lancer le build : "),
                "{}",
                diagnostics[0].message
            );
        }
        other => panic!("attendu Failed, reçu {other:?}"),
    }
}

#[tokio::test]
async fn toolchain_is_read_once_from_the_image() {
    let _g = SERIAL.lock().await;
    let fake = Fake::new();
    fake.put("toolchain", "rustc 1.93.1 (01234abcd 2026-02-11)\n");
    let runner = fake.runner();
    assert_eq!(runner.toolchain().await, "rustc 1.93.1");
    assert_eq!(runner.toolchain().await, "rustc 1.93.1");
    let calls = fake.args();
    assert_eq!(calls.len(), 1, "la chaîne n'est lue qu'une fois");
    assert_eq!(
        calls[0],
        [
            "run",
            "--rm",
            "--network",
            "none",
            "this-forge-builder:1.93",
            "rustc",
            "-V"
        ]
    );
}

#[tokio::test]
async fn toolchain_falls_back_to_unknown_when_the_image_cannot_answer() {
    let _g = SERIAL.lock().await;
    let fake = Fake::new(); // pas de fichier `toolchain` : le faux docker échoue
    assert_eq!(fake.runner().toolchain().await, "rustc unknown");
}

#[tokio::test]
async fn an_extra_file_that_cannot_be_written_is_a_launch_failure() {
    let _g = SERIAL.lock().await;
    let fake = Fake::new();
    // `src/lib.rs` est un fichier : y poser un enfant échoue avant tout lancement de docker.
    let outcome = fake
        .runner()
        .run_with_extra(&lib("pub fn f() {}"), &[("src/lib.rs/enfant.rs", "x")])
        .await;
    match outcome {
        BuildOutcome::Failed { diagnostics, .. } => {
            assert_eq!(diagnostics.len(), 1);
            assert!(
                diagnostics[0]
                    .message
                    .starts_with("la forge n'a pas pu lancer le build : "),
                "{}",
                diagnostics[0].message
            );
        }
        other => panic!("attendu Failed, reçu {other:?}"),
    }
    assert!(fake.args().is_empty(), "docker n'a pas été lancé");
}
