//! Le langage de module de this-rs.
//!
//! ## Pourquoi un langage
//!
//! Un module chargé à chaud doit pouvoir être exécuté sans qu'on lui fasse
//! confiance. Un binaire wasm arbitraire, même signé, peut boucler, allouer,
//! lire une mémoire qu'il n'a pas écrite. Signer dit **qui** a produit le
//! module, pas **ce qu'il fait**.
//!
//! Ce langage dit ce qu'un module peut faire en le rendant impossible à écrire
//! autrement :
//!
//! - **pas de boucle** — un module termine, donc il ne peut pas affamer l'hôte ;
//! - **pas de récursion** — un programme est une fonction, et une fonction ne
//!   peut pas s'appeler (seules les primitives d'hôte sont appelables) ;
//! - **pas d'état** — aucune mutation, donc aucune course entre deux appels ;
//! - **pas d'I/O implicite** — tout effet passe par une primitive d'hôte
//!   nommée, visible dans `check::HOST` et dans la section imports du wasm.
//!
//! La sûreté ne vient pas d'un bac à sable qu'on espère étanche : elle vient de
//! la grammaire. Il n'y a rien à échapper parce que rien n'est exprimable.
//!
//! ## Le trajet
//!
//! ```text
//! source ──lex──▶ jetons ──parse──▶ AST ──check──▶ AST typé
//!                                                    │
//!                              ┌─────────────────────┴──────────┐
//!                              ▼                                ▼
//!                        eval (référence)                 wat (cible)
//! ```
//!
//! L'interpréteur n'est pas un doublon : c'est la **définition** du sens. Le
//! test qui compte est celui qui exécute le même programme des deux côtés et
//! compare les réponses et les effets observés.
//!
//! ## Exemple
//!
//! ```
//! use this_module_lang::{compile, eval, Value};
//!
//! let module = compile("fn run(n: Int) -> Int { if n > 0 { n * 2 } else { 0 } }").unwrap();
//! let mut host = eval::NoHost;
//! assert_eq!(
//!     eval::run(&module.function, &[Value::Int(21)], &mut host).unwrap(),
//!     Value::Int(42)
//! );
//! ```

pub mod ast;
pub mod check;
#[cfg(feature = "descriptor")]
pub mod descriptor;
pub mod eval;
pub mod lex;
pub mod parse;
pub mod wat;

#[cfg(feature = "descriptor")]
pub use descriptor::{Rule, register_rule};

pub use ast::{Function, Type};
pub use check::{HOST, HostFn, PURE, check, check_with};
pub use eval::{Host, NoHost, Value, run};
pub use parse::parse;
pub use wat::{Wat, emit};

/// Un module prêt à être exécuté ou émis. La compilation s'arrête à la première
/// erreur, avec sa position quand elle vient de la source.
#[derive(Clone, Debug, PartialEq)]
pub struct Compiled {
    /// L'AST vérifié : c'est lui qu'on interprète.
    pub function: Function,
    /// Le module wasm textuel équivalent.
    pub wat: Wat,
}

/// Source → AST vérifié → wasm. Le chemin complet, en un appel.
pub fn compile(source: &str) -> Result<Compiled, String> {
    let function = parse::parse(source)?;
    check::check(&function)?;
    let wat = wat::emit(&function)?;
    Ok(Compiled { function, wat })
}

#[cfg(test)]
mod tests {
    use super::*;

    // La comparaison interpréteur / wasm vit dans `tests/parity.rs` : elle a
    // besoin d'un vrai moteur, donc d'une dépendance de test, et elle mérite
    // d'être lue pour elle-même.

    #[test]
    fn a_module_compiles_end_to_end() {
        let module = compile("fn run(a: Int) -> Int { let b: Int = a + 1; b * 2 }").unwrap();
        assert_eq!(module.function.name, "run");
        assert!(module.wat.text.starts_with("(module"));
        assert!(module.wat.text.trim_end().ends_with(')'));
    }

    #[test]
    fn a_syntax_error_says_where_it_is() {
        let error = compile("fn run() -> Int { 1 + }").unwrap_err();
        assert!(error.starts_with("ligne 1, colonne"), "{error}");
    }

    #[test]
    fn a_type_error_is_reported_before_any_emission() {
        let error = compile("fn run() -> Int { emit(\"a\") }").unwrap_err();
        assert!(error.contains("Unit") && error.contains("Int"), "{error}");
    }
}
