//! Le primitif d'appel : un export wasm qui reçoit des tampons JSON `(ptr, len)` et rend
//! `(ptr << 32) | len`, borné en fuel, en mémoire et en taille.
//!
//! C'est le seul endroit où le harnais parle à `wasmi` pour un module « à données » :
//! le contrat de vue ([`crate::render`], export `render`) et l'ABI des modules compilés
//! (`docs/modules-wasm-abi.md`, exports `manifest` et `handle`) passent par
//! [`call_json`], donc par les mêmes refus, dans le même ordre :
//!
//! 1. imports refusés **avant** instanciation ([`ExecutorError::ImportsRefused`]) ;
//! 2. exports `memory`, `alloc` et l'export demandé vérifiés, signature comprise ;
//! 3. mémoire déclarée comparée au plafond ;
//! 4. fuel posé avant l'instanciation, plafond mémoire dans le `Store` ;
//! 5. chaque entrée est écrite dans un tampon obtenu par `alloc` ;
//! 6. l'export est appelé ; `OutOfFuel` et `Memory` sont des valeurs, jamais un blocage ;
//! 7. la réponse est bornée avant d'être lue.
//!
//! Une instance par appel : rien ne survit d'un appel à l'autre, donc un module qui a
//! épuisé son fuel n'empoisonne pas le suivant.

use crate::executor::ExecutorError;

/// La mémoire linéaire : le seul canal entre l'hôte et le module.
pub const MEMORY_EXPORT: &str = "memory";
/// Réserve `n` octets dans la mémoire du module et rend le décalage du tampon.
pub const ALLOC_EXPORT: &str = "alloc";

/// La taille d'une page wasm, en octets.
pub const PAGE_BYTES: usize = 64 * 1024;

/// Les bornes d'un appel. Indépendantes : chacune suffit à arrêter un module.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Limits {
    /// Unités d'exécution accordées à l'appel, posées **avant** l'instanciation.
    ///
    /// `wasmi` les compte en instructions ; un moteur navigateur n'a pas de fuel et
    /// doit borner autrement (horloge, instrumentations) — c'est le moteur qui
    /// interprète ce champ, et il doit refuser l'appel s'il ne sait pas le tenir.
    pub fuel: u64,
    /// Plafond de la mémoire linéaire du module, en pages de 64 Kio.
    pub memory_pages: u32,
    /// Plafond de la requête JSON, en octets (somme des entrées).
    pub request_bytes: usize,
    /// Plafond de la réponse JSON, en octets.
    pub response_bytes: usize,
}

impl Default for Limits {
    /// Des bornes qui protègent par défaut : 2 M d'unités, 16 Mio de mémoire, 256 Kio de
    /// requête, 1 Mio de réponse.
    ///
    /// Mesuré le 06/11/2026 sur une boucle serrée (`wasmi` 2.0, build **debug**) : 10 M
    /// d'unités ≈ 991 ms, donc 2 M ≈ 200 ms — de quoi arrêter une boucle hostile sans
    /// donner l'impression d'un gel. Un module qui rend une grosse liste a besoin de
    /// moins que cela (mesuré : 210 unités pour un arbre d'une brique) ; un appelant qui
    /// en demande plus le dit explicitement en passant ses propres `Limits`.
    fn default() -> Self {
        Self {
            fuel: 2_000_000,
            memory_pages: 256,
            request_bytes: 256 * 1024,
            response_bytes: 1024 * 1024,
        }
    }
}

impl Limits {
    /// Le plafond de mémoire en octets.
    pub fn memory_bytes(&self) -> usize {
        self.memory_pages as usize * PAGE_BYTES
    }

    /// Les bornes par appel de l'ABI des modules compilés (`docs/modules-wasm-abi.md` §8) :
    /// 50 M de fuel, 64 pages (4 Mio), requête 1 Mio, réponse 4 Mio.
    pub fn module_abi() -> Self {
        Self {
            fuel: 50_000_000,
            memory_pages: 64,
            request_bytes: 1024 * 1024,
            response_bytes: 4 * 1024 * 1024,
        }
    }
}

/// Ce qu'un moteur rend après un appel borné : les octets de la réponse, et le fuel
/// consommé — un fait mesuré, utile pour comparer deux modules ou deux versions.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Call {
    pub response: Vec<u8>,
    /// Unités consommées par l'appel. `0` quand le moteur ne sait pas les compter.
    pub fuel_used: u64,
}

/// Les entrées d'un appel : chacune est un tampon passé en `(ptr, len)`, dans l'ordre.
///
/// `render` en prend une (la requête), `manifest` aucune, `handle` deux (opération,
/// requête). La signature attendue de l'export s'en déduit : `2 × n` `i32` → `i64`.
#[derive(Clone, Debug, Default)]
pub struct Args<'a> {
    pub buffers: Vec<&'a [u8]>,
}

impl<'a> Args<'a> {
    pub fn none() -> Self {
        Self::default()
    }
    pub fn one(a: &'a [u8]) -> Self {
        Self { buffers: vec![a] }
    }
    pub fn two(a: &'a [u8], b: &'a [u8]) -> Self {
        Self {
            buffers: vec![a, b],
        }
    }
}

/// Les erreurs d'appel sont celles de l'exécuteur : un seul vocabulaire de refus.
pub type CallError = ExecutorError;

/// Appelle `export` de `wasm` avec `args`, sous `limits`, et rend les octets de la réponse.
#[cfg(feature = "wasmi")]
pub fn call_json(
    wasm: &[u8],
    export: &str,
    args: &Args<'_>,
    limits: &Limits,
) -> Result<Call, CallError> {
    use wasmi::{Config, Engine, Instance, Module, Store, StoreLimitsBuilder, Val, ValType};

    struct State {
        limits: wasmi::StoreLimits,
    }

    if limits.fuel == 0 {
        return Err(ExecutorError::OutOfFuel { fuel: 0 });
    }
    if limits.memory_pages == 0 {
        return Err(ExecutorError::Memory {
            limit_bytes: 0,
            message: "le plafond de mémoire est nul : aucun module ne peut démarrer".to_string(),
        });
    }
    let total: usize = args.buffers.iter().map(|b| b.len()).sum();
    if total > limits.request_bytes {
        return Err(ExecutorError::RequestTooLarge {
            len: total,
            max: limits.request_bytes,
        });
    }

    let mut config = Config::default();
    config.consume_fuel(true);
    let engine = Engine::new(&config);
    let module = Module::new(&engine, wasm).map_err(|e| ExecutorError::Module(e.to_string()))?;

    refuse_imports(&module)?;
    check_memory(&module, limits)?;
    check_signature(
        &module,
        ALLOC_EXPORT,
        "`(func (export \"alloc\") (param i32) (result i32) ...)`",
        &[ValType::I32],
        &[ValType::I32],
    )?;
    let params = vec![ValType::I32; args.buffers.len() * 2];
    let expected = format!(
        "`(func (export \"{export}\") {}(result i64) ...)`",
        if params.is_empty() {
            String::new()
        } else {
            format!("(param{}) ", " i32".repeat(params.len()))
        }
    );
    check_signature(&module, export, &expected, &params, &[ValType::I64])?;

    let store_limits = StoreLimitsBuilder::new()
        .memory_size(limits.memory_bytes())
        .trap_on_grow_failure(true)
        .build();
    let mut store = Store::new(
        &engine,
        State {
            limits: store_limits,
        },
    );
    store.limiter(|state| &mut state.limits);
    store
        .set_fuel(limits.fuel)
        .map_err(|e| ExecutorError::Trap(format!("fuel : {e}")))?;

    let instance = Instance::new(&mut store, &module, &[])
        .map_err(|e| classify(e, limits, "instanciation"))?;
    let memory = instance
        .get_memory(&store, MEMORY_EXPORT)
        .ok_or_else(missing_memory)?;
    let alloc = instance
        .get_typed_func::<i32, i32>(&store, ALLOC_EXPORT)
        .map_err(|e| ExecutorError::WrongSignature {
            name: ALLOC_EXPORT.to_string(),
            expected: "`(param i32) (result i32)`".to_string(),
            found: e.to_string(),
        })?;
    let function =
        instance
            .get_func(&store, export)
            .ok_or_else(|| ExecutorError::MissingExport {
                name: export.to_string(),
                how: how_to_declare(export, &expected),
            })?;

    // Chaque entrée entre par la mémoire : `alloc(len)` en donne le décalage.
    let mut inputs: Vec<Val> = Vec::with_capacity(params.len());
    for buffer in &args.buffers {
        let len = i32::try_from(buffer.len()).map_err(|_| ExecutorError::RequestTooLarge {
            len: buffer.len(),
            max: i32::MAX as usize,
        })?;
        let mut ptr = 0i32;
        if !buffer.is_empty() {
            ptr = alloc
                .call(&mut store, len)
                .map_err(|e| classify(e, limits, ALLOC_EXPORT))?;
            let size = memory.data_size(&store);
            let start = usable(ptr, buffer.len(), size)?;
            memory
                .write(&mut store, start, buffer)
                .map_err(|e| ExecutorError::BadBuffer {
                    ptr: i64::from(ptr),
                    len: buffer.len(),
                    size,
                    why: e.to_string(),
                })?;
        }
        inputs.push(Val::I32(ptr));
        inputs.push(Val::I32(len));
    }

    let mut outputs = [Val::I64(0)];
    function
        .call(&mut store, &inputs, &mut outputs)
        .map_err(|e| classify(e, limits, export))?;
    let Val::I64(packed) = outputs[0] else {
        return Err(ExecutorError::Trap(format!(
            "{export} : résultat inattendu"
        )));
    };
    let remaining = store
        .get_fuel()
        .map_err(|e| ExecutorError::Trap(format!("fuel : {e}")))?;
    let fuel_used = limits.fuel.saturating_sub(remaining);

    // La réponse sort par le même chemin, bornée avant d'être lue.
    let out_ptr = ((packed as u64) >> 32) as u32 as usize;
    let out_len = ((packed as u64) & 0xffff_ffff) as u32 as usize;
    let size = memory.data_size(&store);
    let max = limits.response_bytes.min(size);
    if out_len > max {
        return Err(ExecutorError::ResponseTooLarge { len: out_len, max });
    }
    let start = usable(out_ptr as i32, out_len, size)?;
    let response = memory.data(&store)[start..start + out_len].to_vec();

    Ok(Call {
        response,
        fuel_used,
    })
}

#[cfg(feature = "wasmi")]
use wasmi::TrapCode;
#[cfg(feature = "wasmi")]
use wasmi::{
    ExternType, Module, ValType,
    errors::{ErrorKind, MemoryError},
};

/// Traduit un décalage rendu par le module en plage sûre, ou refuse.
#[cfg(feature = "wasmi")]
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

#[cfg(feature = "wasmi")]
fn refuse_imports(module: &Module) -> Result<(), ExecutorError> {
    let found: Vec<String> = module
        .imports()
        .map(|import| format!("{}.{}", import.module(), import.name()))
        .collect();
    if found.is_empty() {
        return Ok(());
    }
    Err(ExecutorError::ImportsRefused {
        count: found.len(),
        found: found.join(", "),
    })
}

#[cfg(feature = "wasmi")]
fn check_memory(module: &Module, limits: &Limits) -> Result<(), ExecutorError> {
    match module.get_export(MEMORY_EXPORT) {
        Some(ExternType::Memory(ty)) => {
            let pages = ty.minimum();
            if pages > u64::from(limits.memory_pages) {
                return Err(ExecutorError::Memory {
                    limit_bytes: limits.memory_bytes(),
                    message: format!(
                        "le module déclare {pages} pages ({} octets) dès l'instanciation, plafond {} pages ({})",
                        pages.saturating_mul(PAGE_BYTES as u64),
                        limits.memory_pages,
                        limits.memory_bytes()
                    ),
                });
            }
            Ok(())
        }
        Some(_) => Err(ExecutorError::WrongSignature {
            name: MEMORY_EXPORT.to_string(),
            expected: format!("`(memory (export \"{MEMORY_EXPORT}\") n)`"),
            found: "un export mémoire qui n'est pas une mémoire".to_string(),
        }),
        None => Err(missing_memory()),
    }
}

#[cfg(feature = "wasmi")]
fn check_signature(
    module: &Module,
    name: &str,
    expected: &str,
    params: &[ValType],
    results: &[ValType],
) -> Result<(), ExecutorError> {
    match module.get_export(name) {
        Some(ExternType::Func(ty)) => {
            if ty.params() == params && ty.results() == results {
                Ok(())
            } else {
                Err(ExecutorError::WrongSignature {
                    name: name.to_string(),
                    expected: expected.to_string(),
                    found: wasm_signature(ty.params(), ty.results()),
                })
            }
        }
        Some(_) => Err(ExecutorError::WrongSignature {
            name: name.to_string(),
            expected: expected.to_string(),
            found: "un export qui n'est pas une fonction".to_string(),
        }),
        None => Err(ExecutorError::MissingExport {
            name: name.to_string(),
            how: how_to_declare(name, expected),
        }),
    }
}

/// Une signature wasm écrite comme on l'écrirait en WAT : `(param i32) (result i64)`.
#[cfg(feature = "wasmi")]
fn wasm_signature(params: &[ValType], results: &[ValType]) -> String {
    let name = |ty: &ValType| match ty {
        ValType::I32 => "i32",
        ValType::I64 => "i64",
        ValType::F32 => "f32",
        ValType::F64 => "f64",
        _ => "?",
    };
    let join = |types: &[ValType]| types.iter().map(name).collect::<Vec<_>>().join(" ");
    match results {
        [] => format!("(param {}) (result)", join(params)),
        [one] => format!("(param {}) (result {})", join(params), name(one)),
        many => format!("(param {}) (result {})", join(params), join(many)),
    }
}

#[cfg(feature = "wasmi")]
fn missing_memory() -> ExecutorError {
    ExecutorError::MissingExport {
        name: MEMORY_EXPORT.to_string(),
        how: how_to_declare(MEMORY_EXPORT, ""),
    }
}

/// Ce qu'il faut écrire au module pour qu'il satisfasse le contrat, export par export.
#[cfg(feature = "wasmi")]
fn how_to_declare(name: &str, expected: &str) -> String {
    match name {
        MEMORY_EXPORT => format!(
            "les données entrent et sortent par la mémoire linéaire : ajoutez `(memory (export \"{MEMORY_EXPORT}\") 1)`"
        ),
        ALLOC_EXPORT => format!(
            "l'hôte y écrit la requête : ajoutez `(func (export \"{ALLOC_EXPORT}\") (param i32) (result i32) ...)` qui rend le décalage d'un tampon de `n` octets"
        ),
        "render" => String::from(
            "c'est l'entrée du contrat de vue : ajoutez `(func (export \"render\") (param i32 i32) (result i64) ...)` — il reçoit `(ptr, len)` de la requête et rend `((ptr << 32) | len)` de la réponse",
        ),
        _ => format!("ajoutez {expected} : le contrat d'appel de `{name}` le demande"),
    }
}

/// Traduit une erreur de wasmi en refus typé. Le fuel et la mémoire ont leur
/// variante ; le reste reste un trap, avec l'étape qui l'a produit.
#[cfg(feature = "wasmi")]
fn classify(error: wasmi::Error, limits: &Limits, step: &str) -> ExecutorError {
    match error.as_trap_code() {
        Some(TrapCode::OutOfFuel) => {
            return ExecutorError::OutOfFuel { fuel: limits.fuel };
        }
        // `trap_on_grow_failure(true)` : un `memory.grow` au-delà du plafond trap
        // ici, et nulle part ailleurs dans ce chemin.
        Some(TrapCode::GrowthOperationLimited) => {
            return ExecutorError::Memory {
                limit_bytes: limits.memory_bytes(),
                message: error.to_string(),
            };
        }
        _ => {}
    }
    match error.kind() {
        ErrorKind::Memory(MemoryError::OutOfBoundsGrowth) => ExecutorError::Memory {
            limit_bytes: limits.memory_bytes(),
            message: error.to_string(),
        },
        ErrorKind::UserLimits(_) => ExecutorError::Memory {
            limit_bytes: limits.memory_bytes(),
            message: error.to_string(),
        },
        _ => ExecutorError::Trap(format!("{step} : {error}")),
    }
}
