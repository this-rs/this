//! Épreuve avant signature : le `.wasm` produit respecte l'ABI v1 (docs/modules-wasm-abi.md
//! §2 et §3) — AUCUN import, et les quatre exports aux signatures exactes.
//!
//! On lit les sections du binaire (`wasmparser`, déjà dans le lock via wasmi) ; on ne
//! l'instancie pas : l'hôte le fera sous `wasmi`, avec ses bornes.

use wasmparser::{ExternalKind, Parser, Payload, TypeRef, ValType};

/// Un manquement au contrat, lisible tel quel par un agent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AbiViolation(pub String);

fn sig(t: &wasmparser::FuncType) -> (Vec<ValType>, Vec<ValType>) {
    (t.params().to_vec(), t.results().to_vec())
}

/// `Ok(())` si l'artefact respecte l'ABI ; sinon la liste des manquements.
pub fn check_abi(wasm: &[u8]) -> Result<(), Vec<AbiViolation>> {
    let mut bad = Vec::new();
    let mut types: Vec<(Vec<ValType>, Vec<ValType>)> = Vec::new();
    let mut func_type_idx: Vec<u32> = Vec::new();
    let mut imports = Vec::new();
    let mut exports: Vec<(String, ExternalKind, u32)> = Vec::new();
    for payload in Parser::new(0).parse_all(wasm) {
        let payload = match payload {
            Ok(p) => p,
            Err(e) => return Err(vec![AbiViolation(format!("binaire wasm invalide : {e}"))]),
        };
        match payload {
            Payload::TypeSection(s) => {
                for g in s {
                    match g {
                        Ok(g) => {
                            for st in g.into_types() {
                                if let wasmparser::CompositeInnerType::Func(f) =
                                    &st.composite_type.inner
                                {
                                    types.push(sig(f));
                                } else {
                                    types.push((vec![], vec![]));
                                }
                            }
                        }
                        Err(e) => return Err(vec![AbiViolation(format!("section type : {e}"))]),
                    }
                }
            }
            Payload::ImportSection(s) => {
                for i in s.into_iter().flatten() {
                    imports.push(format!("{}::{}", i.module, i.name));
                    // Un import de fonction compte dans l'espace des index.
                    if let TypeRef::Func(t) = i.ty {
                        func_type_idx.push(t);
                    }
                }
            }
            Payload::FunctionSection(s) => {
                for t in s.into_iter().flatten() {
                    func_type_idx.push(t);
                }
            }
            Payload::ExportSection(s) => {
                for e in s.into_iter().flatten() {
                    exports.push((e.name.to_string(), e.kind, e.index));
                }
            }
            _ => {}
        }
    }
    if !imports.is_empty() {
        bad.push(AbiViolation(format!(
            "le module déclare des imports ({}) : l'ABI v1 n'en admet aucun",
            imports.join(", ")
        )));
    }
    let find = |n: &str| exports.iter().find(|(name, ..)| name == n);
    match find("memory") {
        Some((_, ExternalKind::Memory, _)) => {}
        Some(_) => bad.push(AbiViolation(
            "`memory` est exporté mais n'est pas une mémoire".into(),
        )),
        None => bad.push(AbiViolation("export `memory` manquant".into())),
    }
    use ValType::{I32, I64};
    let expect: [(&str, Vec<ValType>, Vec<ValType>); 3] = [
        ("alloc", vec![I32], vec![I32]),
        ("manifest", vec![], vec![I64]),
        ("handle", vec![I32, I32, I32, I32], vec![I64]),
    ];
    for (name, params, results) in expect {
        match find(name) {
            None => bad.push(AbiViolation(format!("export `{name}` manquant"))),
            Some((_, ExternalKind::Func, idx)) => {
                let got = func_type_idx
                    .get(*idx as usize)
                    .and_then(|t| types.get(*t as usize));
                if got != Some(&(params.clone(), results.clone())) {
                    bad.push(AbiViolation(format!(
                        "export `{name}` : signature {got:?}, attendu {params:?} -> {results:?}"
                    )));
                }
            }
            Some(_) => bad.push(AbiViolation(format!(
                "`{name}` est exporté mais n'est pas une fonction"
            ))),
        }
    }
    if bad.is_empty() { Ok(()) } else { Err(bad) }
}
