//! Le binaire : `render` est exporté quand le module nomme une fonction, absent sinon ; et il
//! TOURNE — c'est le premier module Rust réel dont l'ABI de vue est exercée de bout en bout.

use std::path::PathBuf;
use std::process::Command;
use this_module_harness::executor::WasmiExecutor;
use this_module_harness::render::{Limits, ViewRenderer, views_of};

fn racine() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../..")
}

/// Compile le module en wasm32 (avec ou sans la feature `render`) et rend le binaire.
fn compile(avec_render: bool) -> Vec<u8> {
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    let mut cmd = Command::new(cargo);
    cmd.current_dir(racine()).args([
        "build",
        "--release",
        "--target",
        "wasm32-unknown-unknown",
        "-p",
        "this-module-compteur",
    ]);
    if !avec_render {
        cmd.arg("--no-default-features");
    }
    // Sous `cargo llvm-cov`, RUSTFLAGS porte `-C instrument-coverage` : rustc l'applique
    // aussi au build wasm32, qui échoue (mesuré en CI : serde_core, itoa, zmij). Le build
    // enfant n'appartient pas à la couverture, il est donc lancé SANS ces réglages.
    cmd.env_remove("RUSTFLAGS")
        .env_remove("CARGO_ENCODED_RUSTFLAGS")
        .env_remove("LLVM_PROFILE_FILE")
        .env_remove("RUSTC_WRAPPER")
        .env_remove("CARGO_INCREMENTAL");
    let sortie = cmd.output().expect("cargo se lance");
    assert!(
        sortie.status.success(),
        "{}",
        String::from_utf8_lossy(&sortie.stderr)
    );
    let cible = std::env::var("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| racine().join("target"));
    std::fs::read(cible.join("wasm32-unknown-unknown/release/this_module_compteur.wasm"))
        .expect("le .wasm existe")
}

fn leb(b: &[u8], i: &mut usize) -> usize {
    let (mut v, mut s) = (0usize, 0);
    loop {
        let x = b[*i];
        *i += 1;
        v |= ((x & 0x7f) as usize) << s;
        if x & 0x80 == 0 {
            return v;
        }
        s += 7;
    }
}

/// Les noms de la section d'exports (id 7) d'un binaire wasm.
fn exports(wasm: &[u8]) -> Vec<String> {
    let mut i = 8; // magic + version
    while i < wasm.len() {
        let id = wasm[i];
        i += 1;
        let taille = leb(wasm, &mut i);
        if id == 7 {
            let mut j = i;
            let n = leb(wasm, &mut j);
            let mut noms = vec![];
            for _ in 0..n {
                let l = leb(wasm, &mut j);
                noms.push(String::from_utf8(wasm[j..j + l].to_vec()).unwrap());
                j += l;
                j += 1; // genre
                leb(wasm, &mut j); // index
            }
            return noms;
        }
        i += taille;
    }
    vec![]
}

#[tokio::test]
async fn render_est_exporte_seulement_avec_une_fonction_de_rendu() {
    let avec = compile(true);
    let noms = exports(&avec);
    for attendu in ["memory", "alloc", "manifest", "handle", "render"] {
        assert!(
            noms.iter().any(|n| n == attendu),
            "{attendu} manque : {noms:?}"
        );
    }

    // L'ABI de vue de l'hôte, sur ce binaire : l'arbre revient, valide, et dépend de la requête.
    let limites = Limits::view();
    let mut vus = vec![];
    for requete in [r#"{"ouvertes":0}"#, r#"{"ouvertes":3}"#] {
        let appel = WasmiExecutor
            .render(&avec, requete.as_bytes(), &limites)
            .await
            .expect("le module rend");
        let vues = views_of(&appel.response, limites.response_bytes).expect("arbre conforme");
        assert_eq!(vues[0].name, "accueil");
        vus.push(appel.response);
    }
    assert_ne!(vus[0], vus[1]);

    // Une requête qui n'est pas du JSON : une vue d'erreur lisible, pas un trap.
    let appel = WasmiExecutor
        .render(&avec, b"{pas du json", &limites)
        .await
        .expect("flux de contrôle normal, pas de trap");
    let vues = views_of(&appel.response, limites.response_bytes).unwrap();
    assert_eq!(vues[0].name, "render_error");

    // Sans fonction de rendu : le module garde ses trois exports d'avant, et seulement eux.
    let sans = compile(false);
    let mut noms = exports(&sans);
    noms.sort();
    assert_eq!(noms, ["alloc", "handle", "manifest", "memory"]);
    eprintln!(
        "taille wasm : avec render = {} o, sans = {} o",
        avec.len(),
        sans.len()
    );
    assert!(avec.len() > sans.len());
}
