//! Assembler le WAT émis en binaire wasm, dans le processus.
//!
//! ## Pourquoi ici et pas en JavaScript
//!
//! Le plan prévoyait `wabt.js` pour assembler le module **dans la page**. C'est un
//! assembleur JavaScript : une dépendance de plus à charger, à versionner, à faire
//! fonctionner hors ligne, et un second chemin d'assemblage à garder d'accord avec
//! celui du serveur.
//!
//! `wat` fait le même travail en Rust pur et compile pour `wasm32`. Le navigateur
//! reçoit donc des octets et appelle `WebAssembly.instantiate`, exactement comme
//! le serveur reçoit des octets et les donne à `wasmi` ou `wasmtime`. **Un seul
//! assembleur pour les deux côtés**, et le test de parité porte alors sur le code
//! qui est réellement livré.
//!
//! ## Ce que l'assemblage ne fait pas
//!
//! Il ne vérifie pas les types : c'est [`crate::check`] qui le fait, avant
//! l'émission. L'assembleur valide la *forme* du WAT. Un WAT mal formé est refusé
//! avec le message de l'assembleur — jamais tronqué, parce qu'un binaire partiel
//! qui « démarre » est pire qu'une erreur.
//!
//! ## Exemple
//!
//! ```
//! # #[cfg(feature = "assemble")] {
//! use this_module_lang::assemble::compile_to_wasm;
//!
//! let bytes = compile_to_wasm("fn run(a: Int) -> Int { a + 1 }").unwrap();
//! assert_eq!(&bytes[..4], b"\0asm");
//! # }
//! ```

use crate::{Wat, compile};

/// WAT → binaire wasm.
///
/// Prend le texte produit par [`crate::wat::emit`] (ou n'importe quel WAT) et
/// rend les octets que `WebAssembly.instantiate` accepte.
pub fn assemble(wat_source: &str) -> Result<Vec<u8>, String> {
    wat::parse_str(wat_source).map_err(|error| format!("le WAT n'a pas pu être assemblé : {error}"))
}

/// Le WAT déjà émis d'un module compilé, assemblé.
pub fn assemble_module(module: &Wat) -> Result<Vec<u8>, String> {
    assemble(&module.text)
}

/// Source → binaire wasm, en un appel.
///
/// C'est le chemin d'un module produit **dans la page** : compiler, émettre,
/// assembler, puis instancier. Aucun aller-retour serveur.
pub fn compile_to_wasm(source: &str) -> Result<Vec<u8>, String> {
    let module = compile(source)?;
    assemble_module(&module.wat)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_program_becomes_a_wasm_binary() {
        let bytes = compile_to_wasm("fn run(a: Int) -> Int { a + 1 }").expect("assemblage");
        assert_eq!(
            &bytes[..4],
            b"\0asm",
            "une en-tête wasm commence par \\0asm"
        );
        assert!(bytes.len() > 8, "un module vide ne serait pas un module");
    }

    #[test]
    fn a_broken_wat_is_refused_with_a_message() {
        let error = assemble("(module (func").unwrap_err();
        assert!(error.contains("assembl"), "{error}");
    }

    #[test]
    fn a_type_error_is_reported_before_assembly() {
        // L'erreur doit venir du typeur, pas de l'assembleur : la compilation
        // s'arrête avant d'émettre quoi que ce soit.
        let error = compile_to_wasm("fn run() -> Int { emit(\"a\") }").unwrap_err();
        assert!(
            !error.contains("assembl"),
            "ce n'est pas une erreur d'assemblage : {error}"
        );
    }

    #[test]
    fn the_assembled_binary_is_accepted_by_a_real_engine() {
        // Un binaire qui n'est pas valide pour un moteur ne serait pas un module.
        let bytes = compile_to_wasm("fn run() -> Int { 40 + 2 }").expect("assemblage");
        let engine = wasmi::Engine::default();
        wasmi::Module::new(&engine, &bytes[..]).expect("le moteur accepte le module assemblé");
    }
}
