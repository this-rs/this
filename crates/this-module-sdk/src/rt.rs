//! Moteur d'exécution de `export!` : tampons et empaquetage `(ptr << 32) | len`.
//!
//! # Durée de vie des tampons (le piège classique)
//!
//! * **Entrées** (`op`, `req`) : l'hôte appelle `alloc(n)`, écrit dans la mémoire, puis appelle
//!   `handle`. Chaque bloc est un `Vec<u8>` rangé dans `INBOX` ; son adresse ne bouge pas tant
//!   qu'il y reste (le tas ne se déplace pas, seul le `Vec` extérieur grandit). `handle` les
//!   lit, puis **vide `INBOX` à sa sortie** : l'hôte n'a aucune raison de relire ses entrées.
//! * **Sortie** : la réponse est rangée dans `OUT`, un `Vec<u8>` **qui survit au retour** de
//!   `handle`/`manifest`. Le pointeur rendu reste valide jusqu'au **prochain appel** de
//!   `handle`/`manifest` — pas avant : rien ne la libère ni ne la déplace entre-temps, et
//!   `alloc` ne touche pas `OUT`. Un hôte qui lit la réponse avant de rappeler (ce que fait
//!   toute exécution synchrone) ne peut donc pas la voir libérée.
//! * Une instance wasm est mono-fil : `thread_local!` + `RefCell` suffisent, sans `unsafe`
//!   sur l'état partagé. Le seul `unsafe` est la lecture des blocs que l'hôte vient d'écrire.
//! * `alloc(0)` rend un pointeur non nul et aligné (jamais déréférencé).

use crate::{Manifest, Module, Op, Request, Response};
use std::cell::RefCell;

thread_local! {
    static INBOX: RefCell<Vec<Vec<u8>>> = const { RefCell::new(Vec::new()) };
    static OUT: RefCell<Vec<u8>> = const { RefCell::new(Vec::new()) };
}

/// Réserve `n` octets (initialisés à zéro) pour l'hôte et rend leur adresse.
pub fn alloc(n: i32) -> i32 {
    let n = n.max(0) as usize;
    let mut buf = vec![0u8; n];
    let ptr = buf.as_mut_ptr() as usize as i32;
    INBOX.with(|i| i.borrow_mut().push(buf));
    ptr
}

fn pack(bytes: Vec<u8>) -> i64 {
    OUT.with(|o| {
        let mut o = o.borrow_mut();
        *o = bytes; // l'ancienne réponse est libérée ICI, au prochain appel, jamais avant
        ((o.as_ptr() as usize as u64) << 32 | o.len() as u64) as i64
    })
}

fn input<'a>(ptr: i32, len: i32) -> &'a [u8] {
    if len <= 0 {
        return &[];
    }
    // SAFETY: l'hôte a obtenu `ptr` par `alloc(n)` avec n >= len et l'a rempli ; le bloc reste
    // dans INBOX jusqu'à la fin de `handle`, donc au moins aussi longtemps que cette tranche.
    unsafe { std::slice::from_raw_parts(ptr as usize as *const u8, len as usize) }
}

/// Corps de l'export `manifest`.
pub fn manifest<M: Module>() -> i64 {
    let m: Manifest = M::manifest();
    pack(serde_json::to_vec(&m).unwrap_or_else(|_| b"{}".to_vec()))
}

/// Corps de l'export `handle` : total, une entrée invalide devient une réponse d'échec.
pub fn handle<M: Module>(op_ptr: i32, op_len: i32, req_ptr: i32, req_len: i32) -> i64 {
    let resp = run::<M>(input(op_ptr, op_len), input(req_ptr, req_len));
    let bytes = serde_json::to_vec(&resp).unwrap_or_else(|_| {
        br#"{"ok":false,"error":{"code":"serialize_failed","message":"response not serializable"}}"#
            .to_vec()
    });
    let packed = pack(bytes);
    INBOX.with(|i| i.borrow_mut().clear());
    packed
}

/// Corps de l'export `render` : lit la requête (JSON libre, décidé par l'hôte), appelle la
/// fonction de rendu, rend l'arbre. **Total** : une requête illisible ou un rendu en échec
/// devient une vue d'erreur valide ([`crate::views::error_view`]), jamais un trap.
pub fn render<R: crate::views::IntoViews>(
    ptr: i32,
    len: i32,
    f: impl FnOnce(serde_json::Value) -> R,
) -> i64 {
    let packed = pack(render_bytes(input(ptr, len), f));
    INBOX.with(|i| i.borrow_mut().clear());
    packed
}

/// Cœur testable hors wasm de `render` : octets de la requête -> octets de la réponse.
pub fn render_bytes<R: crate::views::IntoViews>(
    req: &[u8],
    f: impl FnOnce(serde_json::Value) -> R,
) -> Vec<u8> {
    let value = if req.is_empty() {
        Ok(serde_json::Value::Null)
    } else {
        serde_json::from_slice(req).map_err(|e| format!("requête de rendu illisible : {e}"))
    };
    let views = value.and_then(|v| f(v).into_views());
    let bytes = match views {
        Ok(views) => {
            serde_json::to_vec(&views).map_err(|e| format!("rendu non sérialisable : {e}"))
        }
        Err(m) => Err(m),
    };
    bytes.unwrap_or_else(|m| {
        serde_json::to_vec(&[crate::views::error_view(&m)]).unwrap_or_else(|_| {
            br#"[{"name":"render_error","surface":"main","root":{"type":"leaf","brick":{"type":"banner","tone":"error","text":"erreur de rendu"}}}]"#.to_vec()
        })
    })
}

/// Cœur testable hors wasm : octets -> `Response`.
pub fn run<M: Module>(op: &[u8], req: &[u8]) -> Response {
    let mut v: serde_json::Value = match serde_json::from_slice(req) {
        Ok(v) => v,
        Err(e) => return Response::err("bad_request", format!("requête illisible : {e}")),
    };
    // L'argument `op` fait foi sur le champ `op` du corps (les deux disent la même chose).
    if let Ok(s) = std::str::from_utf8(op)
        && !s.is_empty()
    {
        if Op::parse(s).is_none() {
            return Response::err("bad_request", format!("opération inconnue : {s}"));
        }
        if let Some(o) = v.as_object_mut() {
            o.insert("op".into(), s.into());
        }
    }
    match serde_json::from_value::<Request>(v) {
        Ok(r) => M::handle(r),
        Err(e) => Response::err("bad_request", format!("requête invalide : {e}")),
    }
}

/// Génère les exports de l'ABI v1 : `memory` (fourni par la cible wasm), `alloc`, `manifest`,
/// `handle` — et `render` **seulement** si le module nomme une fonction de rendu.
///
/// ```ignore
/// this_module_sdk::export!(MonModule);                    // pas de `render`
/// this_module_sdk::export!(MonModule, render = mon_rendu); // + `render`
/// // fn mon_rendu(requete: serde_json::Value) -> Result<View, ViewError>
/// ```
#[macro_export]
macro_rules! export {
    ($module:ty) => {
        #[unsafe(no_mangle)]
        pub extern "C" fn alloc(n: i32) -> i32 {
            $crate::rt::alloc(n)
        }
        #[unsafe(no_mangle)]
        pub extern "C" fn manifest() -> i64 {
            $crate::rt::manifest::<$module>()
        }
        #[unsafe(no_mangle)]
        pub extern "C" fn handle(op_ptr: i32, op_len: i32, req_ptr: i32, req_len: i32) -> i64 {
            $crate::rt::handle::<$module>(op_ptr, op_len, req_ptr, req_len)
        }
    };
    ($module:ty, render = $render:path) => {
        $crate::export!($module);
        #[unsafe(no_mangle)]
        pub extern "C" fn render(ptr: i32, len: i32) -> i64 {
            $crate::rt::render(ptr, len, $render)
        }
    };
}
