//! Les types du primitif d'appel, sans moteur : ils existent sans la feature `wasmi`
//! (une page les utilise aussi) et doivent donc être testés sans elle.

use this_module_harness::call::{
    ALLOC_EXPORT, Args, Call, CallError, Limits, MEMORY_EXPORT, PAGE_BYTES,
};
use this_module_harness::executor::ExecutorError;

#[test]
fn the_export_names_and_page_size_are_the_abi_ones() {
    assert_eq!(MEMORY_EXPORT, "memory");
    assert_eq!(ALLOC_EXPORT, "alloc");
    assert_eq!(PAGE_BYTES, 65_536);
}

#[test]
fn default_limits_are_the_documented_ones() {
    let l = Limits::default();
    assert_eq!(
        (l.fuel, l.memory_pages, l.request_bytes, l.response_bytes),
        (2_000_000, 256, 256 * 1024, 1024 * 1024)
    );
    assert_eq!(l.memory_bytes(), 16 * 1024 * 1024);
}

#[test]
fn module_abi_limits_are_tighter_on_memory_and_looser_on_fuel() {
    let l = Limits::module_abi();
    assert_eq!(
        (l.fuel, l.memory_pages, l.request_bytes, l.response_bytes),
        (50_000_000, 64, 1024 * 1024, 4 * 1024 * 1024)
    );
    assert_eq!(l.memory_bytes(), 4 * 1024 * 1024);
    assert!(l.fuel > Limits::default().fuel);
    assert!(l.memory_pages < Limits::default().memory_pages);
    assert_ne!(l, Limits::default());
}

#[test]
fn memory_bytes_scales_with_pages_and_does_not_overflow_a_u32_page_count() {
    let l = Limits {
        memory_pages: 0,
        ..Limits::default()
    };
    assert_eq!(l.memory_bytes(), 0);
    let l = Limits {
        memory_pages: 1,
        ..Limits::default()
    };
    assert_eq!(l.memory_bytes(), PAGE_BYTES);
    // 65 536 pages = 4 Gio : sur 64 bits, le produit tient (il déborderait un u32).
    #[cfg(target_pointer_width = "64")]
    {
        let l = Limits {
            memory_pages: 65_536,
            ..Limits::default()
        };
        assert_eq!(l.memory_bytes(), 4 * 1024 * 1024 * 1024);
    }
}

#[test]
fn args_constructors_keep_their_buffers_in_order() {
    assert!(Args::none().buffers.is_empty());
    assert!(Args::default().buffers.is_empty());
    assert_eq!(Args::one(b"a").buffers, vec![&b"a"[..]]);
    assert_eq!(Args::two(b"op", b"").buffers, vec![&b"op"[..], &b""[..]]);
    let cloned = Args::two(b"x", b"y").clone();
    assert_eq!(cloned.buffers.len(), 2);
}

#[test]
fn call_compares_by_response_and_fuel() {
    let a = Call {
        response: b"ok".to_vec(),
        fuel_used: 7,
    };
    assert_eq!(a.clone(), a);
    assert_ne!(
        a,
        Call {
            response: b"ok".to_vec(),
            fuel_used: 8
        }
    );
    assert!(format!("{a:?}").contains("fuel_used: 7"));
}

#[test]
fn a_call_error_is_an_executor_error_with_the_shared_wording() {
    let e: CallError = ExecutorError::OutOfFuel { fuel: 10 };
    assert_eq!(
        e.to_string(),
        "budget d'exécution épuisé (10 unités de fuel) : la boucle du module est arrêtée"
    );
    let e: CallError = ExecutorError::RequestTooLarge { len: 6, max: 5 };
    assert_eq!(e.to_string(), "requête trop grande : 6 octets, plafond 5");
    let e: CallError = ExecutorError::ResponseTooLarge { len: 9, max: 4 };
    assert_eq!(e.to_string(), "réponse trop grande : 9 octets, plafond 4");
    let e: CallError = ExecutorError::BadBuffer {
        ptr: -8,
        len: 3,
        size: 65_536,
        why: "négatif".into(),
    };
    assert_eq!(
        e.to_string(),
        "tampon hors mémoire : -8+3 alors que la mémoire fait 65536 octets (négatif)"
    );
}
