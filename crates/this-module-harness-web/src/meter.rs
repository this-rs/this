//! La borne de temps d'une page : **le module s'arrête lui-même**.
//!
//! Une page n'a pas de fuel : `WebAssembly` ne s'interrompt pas, et une boucle dans
//! `render` gèle la WebView. Ce fichier réécrit le binaire **avant** de l'instancier :
//! une globale mutable exportée ([`FUEL_EXPORT`]) porte le budget ; chaque **entrée de
//! fonction** et chaque **tête de boucle** la débite du nombre d'instructions du tronçon
//! qui suit, et, la globale devenue négative, le module exécute `unreachable`. Le moteur
//! de la page n'est pas sollicité : c'est le code du module qui se coupe.
//!
//! Toute boucle infinie repasse par une tête de boucle, toute récursion infinie par une
//! entrée de fonction : il n'existe pas de chemin d'exécution sans fin qui ne paie pas.
//! Le débit est un **majorant** (un tronçon est payé en entier même si un `br` en sort
//! plus tôt) : le fuel consommé n'est pas exact, il est toujours suffisant pour arrêter.
//!
//! Tout ce qui précède l'instanciation est ici, **hors de toute API navigateur** : les
//! refus (imports, exports, signatures, mémoire) et la réécriture se testent en natif,
//! et le binaire réécrit s'exécute sous `wasmi` dans les tests.
//!
//! # Pourquoi pas un Worker + `terminate`
//!
//! C'est la seule autre façon d'interrompre du wasm sur une page, et elle a été écartée :
//! un Worker est un **fichier séparé** (un second binaire wasm ou un script que `trunk`
//! doit construire et que l'hôte Tauri doit servir), le module et la requête y passent
//! par `postMessage` (copie), le rendu devient un aller-retour asynchrone par arbre, et
//! le mécanisme n'est testable que dans un vrai navigateur. La réécriture reste dans le
//! binaire de la page, ne coûte aucun fichier, et se teste sans appareil.
//!
//! # Ce que la réécriture ne borne pas
//!
//! - la **récursion infinie** ne boucle pas : elle déborde la pile, et le moteur la rend
//!   en exception (`RangeError`), jamais en gel ; elle est rendue `Trap` ;
//! - `memory.grow` n'est pas débité : le **maximum** de la mémoire est réécrit au plafond
//!   ([`Limits::memory_pages`]), donc un `grow` au-delà échoue (rend `-1`) ;
//! - une **instruction unique** coûteuse (un `memory.fill` de 4 Gio) : une seule
//!   instruction, bornée par la mémoire, donc par le plafond ci-dessus.

use this_module_harness::executor::ExecutorError;
use this_module_harness::render::{ALLOC_EXPORT, Limits, MEMORY_EXPORT, PAGE_BYTES, RENDER_EXPORT};
use wasm_encoder::reencode::{Reencode, RoundtripReencoder};
use wasm_encoder::{
    BlockType, CodeSection, ConstExpr, ExportKind, ExportSection, GlobalSection, GlobalType,
    Instruction, MemorySection, RawSection, ValType,
};
use wasmparser::{CompositeInnerType, ExternalKind, Operator, Parser, Payload};

/// Le nom sous lequel le compteur de fuel est exporté. Un module qui exporte déjà ce
/// nom est refusé : il pourrait se recharger lui-même.
pub const FUEL_EXPORT: &str = "__this_fuel";

/// Un module prêt à être instancié dans la page.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Metered {
    /// Le binaire réécrit : mêmes exports, plus [`FUEL_EXPORT`], fuel déjà posé.
    pub wasm: Vec<u8>,
    /// Le fuel posé à l'instanciation (la section `start` est donc bornée aussi).
    pub fuel: u64,
    /// Taille du binaire d'origine, pour mesurer ce que coûte la réécriture.
    pub original_bytes: usize,
}

/// Refuse ce que le contrat de vue refuse, puis réécrit.
///
/// Même ordre que `call_json` (wasmi) : imports, exports et signatures, mémoire déclarée.
pub fn meter(program: &[u8], limits: &Limits) -> Result<Metered, ExecutorError> {
    if limits.fuel == 0 {
        return Err(ExecutorError::OutOfFuel { fuel: 0 });
    }
    if limits.memory_pages == 0 {
        return Err(ExecutorError::Memory {
            limit_bytes: 0,
            message: "le plafond de mémoire est nul : aucun module ne peut démarrer".to_string(),
        });
    }

    let scan = Scan::of(program)?;
    scan.refuse_imports()?;
    if scan.exports.iter().any(|(name, ..)| name == FUEL_EXPORT) {
        return Err(ExecutorError::Link(format!(
            "le module exporte `{FUEL_EXPORT}`, nom réservé au compteur de fuel de l'hôte"
        )));
    }
    scan.check_memory_export()?;
    scan.check_function(
        ALLOC_EXPORT,
        "`(func (export \"alloc\") (param i32) (result i32) ...)`",
        &[wasmparser::ValType::I32],
        &[wasmparser::ValType::I32],
    )?;
    scan.check_function(
        RENDER_EXPORT,
        "`(func (export \"render\") (param i32 i32) (result i64) ...)`",
        &[wasmparser::ValType::I32, wasmparser::ValType::I32],
        &[wasmparser::ValType::I64],
    )?;
    scan.check_declared_memory(limits)?;

    // Le fuel est un i64 signé dans la globale : tout négatif signifie « épuisé ».
    let fuel = limits.fuel.min(i64::MAX as u64);
    let wasm = rewrite(program, &scan, limits, fuel)
        .map_err(|e| ExecutorError::Module(format!("réécriture : {e}")))?;
    Ok(Metered {
        wasm,
        fuel,
        original_bytes: program.len(),
    })
}

/// Fuel consommé, d'après la valeur lue dans la globale après l'appel.
///
/// `Err(budget)` quand la globale est négative : le module s'est coupé lui-même.
pub fn consumed(budget: u64, remaining: i64) -> Result<u64, u64> {
    if remaining < 0 {
        return Err(budget);
    }
    Ok(budget.saturating_sub(remaining as u64))
}

/// Ce qu'une première lecture du binaire apprend, sans rien exécuter.
struct Scan {
    imports: Vec<String>,
    exports: Vec<(String, ExternalKind, u32)>,
    types: Vec<Option<(Vec<wasmparser::ValType>, Vec<wasmparser::ValType>)>>,
    /// Index de type de chaque fonction définie (aucun import : index de fonction = rang).
    functions: Vec<u32>,
    memories: Vec<wasmparser::MemoryType>,
    globals: u32,
}

impl Scan {
    fn of(program: &[u8]) -> Result<Self, ExecutorError> {
        let bad = |e: wasmparser::BinaryReaderError| ExecutorError::Module(e.to_string());
        let mut scan = Scan {
            imports: Vec::new(),
            exports: Vec::new(),
            types: Vec::new(),
            functions: Vec::new(),
            memories: Vec::new(),
            globals: 0,
        };
        for payload in Parser::new(0).parse_all(program) {
            match payload.map_err(bad)? {
                Payload::TypeSection(reader) => {
                    for group in reader {
                        for sub in group.map_err(bad)?.into_types() {
                            scan.types.push(match &sub.composite_type.inner {
                                CompositeInnerType::Func(f) => {
                                    Some((f.params().to_vec(), f.results().to_vec()))
                                }
                                _ => None,
                            });
                        }
                    }
                }
                Payload::ImportSection(reader) => {
                    for import in reader.into_imports() {
                        let import = import.map_err(bad)?;
                        scan.imports
                            .push(format!("{}.{}", import.module, import.name));
                    }
                }
                Payload::FunctionSection(reader) => {
                    for f in reader {
                        scan.functions.push(f.map_err(bad)?);
                    }
                }
                Payload::MemorySection(reader) => {
                    for m in reader {
                        scan.memories.push(m.map_err(bad)?);
                    }
                }
                Payload::GlobalSection(reader) => scan.globals = reader.count(),
                Payload::ExportSection(reader) => {
                    for e in reader {
                        let e = e.map_err(bad)?;
                        scan.exports.push((e.name.to_string(), e.kind, e.index));
                    }
                }
                _ => {}
            }
        }
        Ok(scan)
    }

    fn refuse_imports(&self) -> Result<(), ExecutorError> {
        if self.imports.is_empty() {
            return Ok(());
        }
        Err(ExecutorError::ImportsRefused {
            count: self.imports.len(),
            found: self.imports.join(", "),
        })
    }

    fn export(&self, name: &str) -> Option<(ExternalKind, u32)> {
        self.exports
            .iter()
            .find(|(n, ..)| n == name)
            .map(|(_, k, i)| (*k, *i))
    }

    /// L'export `memory` doit exister et être une mémoire.
    fn check_memory_export(&self) -> Result<(), ExecutorError> {
        match self.export(MEMORY_EXPORT) {
            Some((ExternalKind::Memory, _)) => Ok(()),
            Some(_) => Err(ExecutorError::MissingExport {
                name: MEMORY_EXPORT.to_string(),
                how:
                    "`memory` existe mais n'est pas une mémoire : `(memory (export \"memory\") 1)`"
                        .to_string(),
            }),
            None => Err(missing(MEMORY_EXPORT, "`(memory (export \"memory\") 1)`")),
        }
    }

    fn check_function(
        &self,
        name: &str,
        expected: &str,
        params: &[wasmparser::ValType],
        results: &[wasmparser::ValType],
    ) -> Result<(), ExecutorError> {
        let (kind, index) = self.export(name).ok_or_else(|| missing(name, expected))?;
        if kind != ExternalKind::Func {
            return Err(ExecutorError::WrongSignature {
                name: name.to_string(),
                expected: expected.to_string(),
                found: "un export qui n'est pas une fonction".to_string(),
            });
        }
        let found = self
            .functions
            .get(index as usize)
            .and_then(|t| self.types.get(*t as usize))
            .and_then(Option::as_ref)
            .ok_or_else(|| {
                ExecutorError::Module(format!("`{name}` : type de fonction introuvable"))
            })?;
        if found.0 != params || found.1 != results {
            return Err(ExecutorError::WrongSignature {
                name: name.to_string(),
                expected: expected.to_string(),
                found: format!("{:?} -> {:?}", found.0, found.1),
            });
        }
        Ok(())
    }

    /// Une mémoire déclarée d'emblée au-delà du plafond est refusée sans être instanciée.
    fn check_declared_memory(&self, limits: &Limits) -> Result<(), ExecutorError> {
        for m in &self.memories {
            if m.initial > u64::from(limits.memory_pages) {
                return Err(ExecutorError::Memory {
                    limit_bytes: limits.memory_bytes(),
                    message: format!(
                        "le module déclare {} pages ({} octets) dès l'instanciation, plafond {} pages ({})",
                        m.initial,
                        m.initial.saturating_mul(PAGE_BYTES as u64),
                        limits.memory_pages,
                        limits.memory_bytes()
                    ),
                });
            }
        }
        Ok(())
    }
}

fn missing(name: &str, expected: &str) -> ExecutorError {
    ExecutorError::MissingExport {
        name: name.to_string(),
        how: format!("déclarez-le : {expected}"),
    }
}

/// La réécriture : sections recopiées telles quelles, sauf mémoire (maximum au plafond),
/// globales (+ le compteur), exports (+ le compteur) et code (débits injectés).
fn rewrite(
    program: &[u8],
    scan: &Scan,
    limits: &Limits,
    fuel: u64,
) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let counter = scan.globals; // aucune globale importée : le compteur est la suivante
    let mut out = wasm_encoder::Module::new();
    let mut code: Option<CodeSection> = None;
    let mut globals_written = false;

    let fuel_global = |section: &mut GlobalSection| {
        section.global(
            GlobalType {
                val_type: ValType::I64,
                mutable: true,
                shared: false,
            },
            &ConstExpr::i64_const(fuel as i64),
        );
    };

    for payload in Parser::new(0).parse_all(program) {
        let payload = payload?;
        if !matches!(payload, Payload::CodeSectionEntry(_))
            && let Some(done) = code.take()
        {
            out.section(&done);
        }
        match payload {
            Payload::Version { .. } | Payload::End(_) | Payload::CustomSection(_) => {}
            Payload::MemorySection(reader) => {
                let mut section = MemorySection::new();
                for memory in reader {
                    let mut ty = RoundtripReencoder.memory_type(memory?)?;
                    let cap = u64::from(limits.memory_pages);
                    ty.maximum = Some(ty.maximum.map_or(cap, |m| m.min(cap)));
                    section.memory(ty);
                }
                out.section(&section);
            }
            Payload::GlobalSection(reader) => {
                let mut section = GlobalSection::new();
                RoundtripReencoder.parse_global_section(&mut section, reader)?;
                fuel_global(&mut section);
                out.section(&section);
                globals_written = true;
            }
            Payload::ExportSection(reader) => {
                if !globals_written {
                    let mut section = GlobalSection::new();
                    fuel_global(&mut section);
                    out.section(&section);
                    globals_written = true;
                }
                let mut section = ExportSection::new();
                RoundtripReencoder.parse_export_section(&mut section, reader)?;
                section.export(FUEL_EXPORT, ExportKind::Global, counter);
                out.section(&section);
            }
            Payload::CodeSectionStart { .. } => code = Some(CodeSection::new()),
            Payload::CodeSectionEntry(body) => {
                let section = code.as_mut().ok_or("corps de fonction hors section")?;
                charge_body(section, body, counter)?;
            }
            other => {
                if let Some((id, range)) = other.as_section() {
                    out.section(&RawSection {
                        id,
                        data: &program[range.start as usize..range.end as usize],
                    });
                }
            }
        }
    }
    Ok(out.finish())
}

/// Réécrit un corps de fonction : un débit à l'entrée, un autre à chaque tête de boucle.
///
/// Le tronçon payé par un débit va de là jusqu'à la prochaine instruction `loop`
/// (comprise) ou la fin de la fonction ; son coût est son nombre d'instructions, au
/// moins 1 : une boucle vide paie quand même chaque tour.
fn charge_body(
    section: &mut CodeSection,
    body: wasmparser::FunctionBody<'_>,
    counter: u32,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut function = RoundtripReencoder.new_function_with_parsed_locals(&body)?;
    let mut operators: Vec<Operator<'_>> = Vec::new();
    for operator in body.get_operators_reader()? {
        operators.push(operator?);
    }

    // `costs[k]` : le prix du k-ième tronçon ; le 0 commence à l'entrée de la fonction.
    let mut costs = vec![0i64];
    for operator in &operators {
        if let Some(last) = costs.last_mut() {
            *last += 1;
        }
        if matches!(operator, Operator::Loop { .. }) {
            costs.push(0);
        }
    }
    let mut costs = costs.into_iter().map(|c| c.max(1));

    let charge = |function: &mut wasm_encoder::Function, cost: i64| {
        for instruction in [
            Instruction::GlobalGet(counter),
            Instruction::I64Const(cost),
            Instruction::I64Sub,
            Instruction::GlobalSet(counter),
            Instruction::GlobalGet(counter),
            Instruction::I64Const(0),
            Instruction::I64LtS,
            Instruction::If(BlockType::Empty),
            Instruction::Unreachable,
            Instruction::End,
        ] {
            function.instruction(&instruction);
        }
    };

    charge(&mut function, costs.next().unwrap_or(1));
    for operator in operators {
        let is_loop = matches!(operator, Operator::Loop { .. });
        function.instruction(&RoundtripReencoder.instruction(operator)?);
        if is_loop {
            charge(&mut function, costs.next().unwrap_or(1));
        }
    }
    section.function(&function);
    Ok(())
}

#[cfg(test)]
#[allow(dead_code)] // chaque cible n'utilise qu'une partie des modules de test
pub(crate) mod fixtures {
    /// Une vue d'une brique, écrite dans la mémoire du module à l'offset 1024.
    pub const VIEW: &str = r#"{"name":"orders","surface":"main","root":{"type":"leaf","brick":{"type":"list","entity":"order","label":"id"}}}"#;

    fn escape(text: &str) -> String {
        text.bytes().map(|b| format!("\\{b:02x}")).collect()
    }

    /// Un module conforme : rend `payload` tel quel, à l'offset 1024.
    pub fn answering(payload: &str) -> Vec<u8> {
        let size = payload.len();
        let pages = (1024 + size).div_ceil(65536).max(1);
        wat::parse_str(format!(
            r#"(module
  (memory (export "memory") {pages})
  (data (i32.const 1024) "{}")
  (func (export "alloc") (param i32) (result i32) (i32.const 2048))
  (func (export "render") (param i32 i32) (result i64)
    (i64.or (i64.shl (i64.const 1024) (i64.const 32)) (i64.const {size}))))"#,
            escape(payload)
        ))
        .expect("wat valide")
    }

    /// Un module dont `render` ne rend jamais.
    pub fn looping() -> Vec<u8> {
        wat::parse_str(
            r#"(module
  (memory (export "memory") 1)
  (func (export "alloc") (param i32) (result i32) (i32.const 2048))
  (func (export "render") (param i32 i32) (result i64)
    (loop $forever (br $forever))
    (i64.const 0)))"#,
        )
        .expect("wat valide")
    }

    /// `render(n, _)` fait `n` tours d'une boucle serrée : la charge de la mesure de vitesse.
    pub fn counting() -> Vec<u8> {
        wat::parse_str(
            r#"(module
  (memory (export "memory") 1)
  (func (export "alloc") (param i32) (result i32) (i32.const 0))
  (func (export "render") (param $n i32) (param i32) (result i64)
    (local $acc i64)
    (block $out (loop $l
      (br_if $out (i32.eqz (local.get $n)))
      (local.set $acc (i64.add (local.get $acc) (i64.const 3)))
      (local.set $n (i32.sub (local.get $n) (i32.const 1)))
      (br $l)))
    (local.get $acc)))"#,
        )
        .expect("wat valide")
    }

    /// Un module dont la section `start` ne rend jamais.
    pub fn looping_start() -> Vec<u8> {
        wat::parse_str(
            r#"(module
  (memory (export "memory") 1)
  (func $spin (loop $l (br $l)))
  (start $spin)
  (func (export "alloc") (param i32) (result i32) (i32.const 2048))
  (func (export "render") (param i32 i32) (result i64) (i64.const 0)))"#,
        )
        .expect("wat valide")
    }

    pub fn importing() -> Vec<u8> {
        wat::parse_str(
            r#"(module
  (import "env" "now" (func $now (result i64)))
  (memory (export "memory") 1)
  (func (export "alloc") (param i32) (result i32) (i32.const 0))
  (func (export "render") (param i32 i32) (result i64) (call $now)))"#,
        )
        .expect("wat valide")
    }

    pub fn wrong_signature() -> Vec<u8> {
        wat::parse_str(
            r#"(module
  (memory (export "memory") 1)
  (func (export "alloc") (param i32) (result i32) (i32.const 0))
  (func (export "render") (param i32) (result i32) (i32.const 0)))"#,
        )
        .expect("wat valide")
    }

    pub fn missing_render() -> Vec<u8> {
        wat::parse_str(
            r#"(module
  (memory (export "memory") 1)
  (func (export "alloc") (param i32) (result i32) (i32.const 0)))"#,
        )
        .expect("wat valide")
    }

    /// Déclare d'emblée 100 pages.
    pub fn greedy() -> Vec<u8> {
        wat::parse_str(
            r#"(module
  (memory (export "memory") 100)
  (func (export "alloc") (param i32) (result i32) (i32.const 0))
  (func (export "render") (param i32 i32) (result i64) (i64.const 0)))"#,
        )
        .expect("wat valide")
    }

    /// Demande à grandir de 200 pages et rend le résultat de `memory.grow` en offset.
    pub fn growing() -> Vec<u8> {
        wat::parse_str(
            r#"(module
  (memory (export "memory") 1)
  (func (export "alloc") (param i32) (result i32) (i32.const 0))
  (func (export "render") (param i32 i32) (result i64)
    (i64.extend_i32_s (memory.grow (i32.const 200)))))"#,
        )
        .expect("wat valide")
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::fixtures::*;
    use super::*;
    use wasmi::{Engine, Linker, Module as WModule, Store, Val};

    /// Exécute `render` du binaire instrumenté sous wasmi **sans fuel wasmi** : seul le
    /// compteur injecté peut l'arrêter. Rend le résultat et la valeur finale du compteur.
    fn run(wasm: &[u8]) -> (Result<(u64, u64), String>, Option<i64>) {
        let engine = Engine::default();
        let module = WModule::new(&engine, wasm).expect("module valide");
        let mut store = Store::new(&engine, ());
        let instance = match Linker::<()>::new(&engine).instantiate_and_start(&mut store, &module) {
            Ok(i) => i,
            Err(e) => return (Err(e.to_string()), None),
        };
        let fuel = instance.get_global(&store, FUEL_EXPORT).expect("compteur");
        let render = instance.get_func(&store, "render").expect("render");
        let mut out = [Val::I64(0)];
        let result = render
            .call(&mut store, &[Val::I32(0), Val::I32(0)], &mut out)
            .map(|()| {
                let Val::I64(p) = out[0] else { unreachable!() };
                ((p as u64) >> 32, (p as u64) & 0xffff_ffff)
            })
            .map_err(|e| e.to_string());
        let Val::I64(left) = fuel.get(&store) else {
            panic!("le compteur est un i64")
        };
        (result, Some(left))
    }

    #[test]
    fn a_looping_render_stops_by_itself() {
        // Aucun fuel wasmi : sans l'instrumentation, ce test ne finirait pas.
        let metered = meter(&looping(), &Limits::view()).expect("conforme");
        let (result, left) = run(&metered.wasm);
        assert!(result.is_err(), "la boucle doit être coupée : {result:?}");
        assert_eq!(left, Some(-1), "le compteur porte le sentinel d'épuisement");
        assert_eq!(consumed(metered.fuel, -1), Err(Limits::view().fuel));
    }

    #[test]
    fn a_looping_start_section_is_stopped_too() {
        let metered = meter(&looping_start(), &Limits::view()).expect("conforme");
        let (result, _) = run(&metered.wasm);
        assert!(result.is_err(), "un `start` qui boucle est coupé");
    }

    #[test]
    fn the_budget_is_the_one_of_limits_view() {
        let metered = meter(&answering(VIEW), &Limits::view()).expect("conforme");
        assert_eq!(metered.fuel, 30_000_000);
        // Rien n'a encore tourné : la globale vaut exactement le budget.
        let engine = Engine::default();
        let module = WModule::new(&engine, &metered.wasm).unwrap();
        let mut store = Store::new(&engine, ());
        let instance = Linker::<()>::new(&engine)
            .instantiate_and_start(&mut store, &module)
            .unwrap();
        let g = instance.get_global(&store, FUEL_EXPORT).unwrap();
        assert!(matches!(g.get(&store), Val::I64(30_000_000)));
    }

    #[test]
    fn a_conforming_tree_runs_and_costs_little() {
        let metered = meter(&answering(VIEW), &Limits::view()).expect("conforme");
        let (result, left) = run(&metered.wasm);
        let (ptr, len) = result.expect("un module conforme rend");
        assert_eq!((ptr, len as usize), (1024, VIEW.len()));
        let used = consumed(metered.fuel, left.unwrap()).expect("pas épuisé");
        assert!(
            used > 0 && used < 1_000,
            "un rendu simple coûte peu : {used}"
        );
    }

    #[test]
    fn instrumenting_changes_no_export() {
        let metered = meter(&answering(VIEW), &Limits::view()).expect("conforme");
        let names: Vec<String> = Scan::of(&metered.wasm)
            .expect("le binaire réécrit se relit")
            .exports
            .into_iter()
            .map(|(name, ..)| name)
            .collect();
        let names: Vec<&str> = names.iter().map(String::as_str).collect();
        for expected in ["memory", "alloc", "render", FUEL_EXPORT] {
            assert!(names.contains(&expected), "{expected} manque : {names:?}");
        }
        assert_eq!(names.len(), 4);
    }

    #[test]
    fn imports_are_refused_by_name() {
        match meter(&importing(), &Limits::view()) {
            Err(ExecutorError::ImportsRefused { count: 1, found }) => {
                assert_eq!(found, "env.now");
            }
            other => panic!("un import doit être refusé nommément : {other:?}"),
        }
    }

    #[test]
    fn a_wrong_signature_is_refused_with_what_was_found() {
        match meter(&wrong_signature(), &Limits::view()) {
            Err(ExecutorError::WrongSignature { name, found, .. }) => {
                assert_eq!(name, "render");
                assert!(found.contains("I32"), "{found}");
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_missing_export_is_named() {
        match meter(&missing_render(), &Limits::view()) {
            Err(ExecutorError::MissingExport { name, .. }) => assert_eq!(name, "render"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn garbage_is_a_named_refusal_not_a_panic() {
        match meter(b"pas du wasm", &Limits::view()) {
            Err(ExecutorError::Module(_)) => {}
            other => panic!("{other:?}"),
        }
        match meter(&[], &Limits::view()) {
            Err(ExecutorError::Module(_)) => {}
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_greedy_module_is_refused_before_running() {
        match meter(&greedy(), &Limits::view()) {
            Err(ExecutorError::Memory { limit_bytes, .. }) => {
                assert_eq!(limit_bytes, 64 * 65536);
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn memory_grow_past_the_cap_fails_instead_of_allocating() {
        let metered = meter(&growing(), &Limits::view()).expect("déclare 1 page : accepté");
        let (result, _) = run(&metered.wasm);
        let (ptr, len) = result.expect("grow échoue, il ne piège pas");
        // memory.grow rend -1 : étendu en i64 puis empaqueté, ptr = 0xffffffff.
        assert_eq!(
            (ptr, len),
            (0xffff_ffff, 0xffff_ffff),
            "grow doit rendre -1"
        );
    }

    #[test]
    fn a_reserved_export_name_is_refused() {
        let wasm = wat::parse_str(
            r#"(module
  (memory (export "memory") 1)
  (global (export "__this_fuel") (mut i64) (i64.const 1))
  (func (export "alloc") (param i32) (result i32) (i32.const 0))
  (func (export "render") (param i32 i32) (result i64) (i64.const 0)))"#,
        )
        .unwrap();
        assert!(matches!(
            meter(&wasm, &Limits::view()),
            Err(ExecutorError::Link(_))
        ));
    }

    #[test]
    fn instrumentation_cost_is_measured() {
        // Une mesure, pas une assertion de performance : elle s'écrit dans la sortie.
        for (name, wasm) in [("conforme", answering(VIEW)), ("boucle", looping())] {
            let metered = meter(&wasm, &Limits::view()).unwrap();
            println!(
                "TAILLE {name}: {} -> {} octets",
                metered.original_bytes,
                metered.wasm.len()
            );
        }
    }

    /// Un vrai module Rust (`this-module-sdk`, rustc récent : bulk-memory, conversions
    /// saturantes) est réécrit et `wasmi` — qui valide — l'accepte. C'est ce que
    /// `wasm-instrument` 0.4 refusait (`UnknownOpcode(252)`).
    #[test]
    fn a_real_rust_module_with_bulk_memory_is_rewritten() {
        let real = include_bytes!("../../../tests/fixtures/taches.wasm");
        let after = meter_unchecked(real).expect("un vrai module Rust doit être réécrit");
        assert!(after > 0);
    }

    /// Mesure sur de vrais modules Rust (`this-module-sdk`), pas des modules de test.
    /// `REAL_WASM` : chemins séparés par `:` ; sans la variable, le test ne mesure rien.
    #[test]
    fn size_cost_on_real_modules_is_measured() {
        let Ok(paths) = std::env::var("REAL_WASM") else {
            return;
        };
        for path in paths.split(':') {
            let real = std::fs::read(path).expect("module lisible");
            match meter_unchecked(&real) {
                Ok(after) => println!("TAILLE réel {path}: {} -> {after} octets", real.len()),
                Err(e) => println!("TAILLE réel {path}: refusé ({e})"),
            }
        }
    }

    /// Réécrit sans les contrôles de contrat (un module SDK n'exporte pas `render`), et
    /// vérifie que `wasmi` — qui valide — accepte le résultat.
    fn meter_unchecked(real: &[u8]) -> Result<usize, String> {
        let scan = Scan::of(real).map_err(|e| e.to_string())?;
        let out = rewrite(real, &scan, &Limits::view(), 30_000_000).map_err(|e| e.to_string())?;
        wasmi::Module::new(&wasmi::Engine::default(), &out[..]).map_err(|e| e.to_string())?;
        // Le même binaire recopié sans aucun débit : sépare le prix de la réécriture de
        // celui du simple réencodage (sections `custom` retirées, LEB réécrits au plus court).
        let mut plain = wasm_encoder::Module::new();
        wasm_encoder::reencode::utils::parse_core_module(
            &mut RoundtripReencoder,
            &mut plain,
            Parser::new(0),
            real,
        )
        .map_err(|e| e.to_string())?;
        println!("TAILLE recopie sans debit: {} octets", plain.finish().len());
        Ok(out.len())
    }
}
