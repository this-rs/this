//! Le typage : ce qui rend l'émission wasm possible et l'exécution sûre.
//!
//! Aucune conversion implicite. Un `Int` ne se compare pas à un `Bool`, une
//! `Str` ne s'additionne pas, et une `Str` ne se lie pas : la manipuler demande
//! un modèle mémoire qui n'existe pas encore. Refuser ici plutôt que mal
//! compiler là-bas.

use crate::ast::{BinOp, Expr, Function, Type, UnOp};

/// Une primitive que l'hôte met à disposition. Cette table **est** la surface
/// d'appel d'un module : ce qui n'y figure pas ne peut pas être appelé.
pub struct HostFn {
    pub name: &'static str,
    pub params: &'static [Type],
    pub returns: Type,
}

pub const HOST: &[HostFn] = &[
    HostFn {
        name: "emit",
        params: &[Type::Str],
        returns: Type::Unit,
    },
    HostFn {
        name: "now",
        params: &[],
        returns: Type::Int,
    },
];

fn host(name: &str) -> Option<&'static HostFn> {
    HOST.iter().find(|function| function.name == name)
}

struct Env(Vec<(String, Type)>);

impl Env {
    fn get(&self, name: &str) -> Option<Type> {
        self.0
            .iter()
            .rev()
            .find(|(candidate, _)| candidate == name)
            .map(|(_, ty)| *ty)
    }
}

/// Vérifie le programme entier et rend le type de son corps.
pub fn check(function: &Function) -> Result<Type, String> {
    let mut env = Env(function
        .params
        .iter()
        .map(|param| (param.name.clone(), param.ty))
        .collect());

    for binding in &function.bindings {
        let value = infer(&binding.value, &env)?;
        if value != binding.ty {
            return Err(format!(
                "`{}` est déclaré {} mais vaut {}",
                binding.name,
                binding.ty.name(),
                value.name()
            ));
        }
        if binding.ty == Type::Str {
            return Err(format!(
                "`{}` : une chaîne ne se lie pas (modèle mémoire absent) — passez-la directement à un appel d'hôte",
                binding.name
            ));
        }
        env.0.push((binding.name.clone(), binding.ty));
    }

    let body = infer(&function.body, &env)?;
    if body != function.returns {
        return Err(format!(
            "le corps rend {} alors que la fonction annonce {}",
            body.name(),
            function.returns.name()
        ));
    }
    Ok(body)
}

fn infer(expr: &Expr, env: &Env) -> Result<Type, String> {
    match expr {
        Expr::Int(_) => Ok(Type::Int),
        Expr::Bool(_) => Ok(Type::Bool),
        Expr::Str(_) => Ok(Type::Str),
        Expr::Var(name) => env
            .get(name)
            .ok_or_else(|| format!("`{name}` n'est ni un paramètre ni une liaison")),
        Expr::Unary { op, expr } => {
            let inner = infer(expr, env)?;
            match (op, inner) {
                (UnOp::Not, Type::Bool) => Ok(Type::Bool),
                (UnOp::Neg, Type::Int) => Ok(Type::Int),
                _ => Err(format!("`{op:?}` ne s'applique pas à {}", inner.name())),
            }
        }
        Expr::Binary { op, left, right } => {
            let left = infer(left, env)?;
            let right = infer(right, env)?;
            match (op, left, right) {
                (
                    BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div | BinOp::Rem,
                    Type::Int,
                    Type::Int,
                ) => Ok(Type::Int),
                (BinOp::And | BinOp::Or, Type::Bool, Type::Bool) => Ok(Type::Bool),
                (BinOp::Eq | BinOp::Ne, a, b) if a == b && matches!(a, Type::Int | Type::Bool) => {
                    Ok(Type::Bool)
                }
                (BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge, Type::Int, Type::Int) => {
                    Ok(Type::Bool)
                }
                (op, left, right) => Err(format!(
                    "`{}` ne s'applique pas à {} et {}",
                    op.symbol(),
                    left.name(),
                    right.name()
                )),
            }
        }
        Expr::If {
            condition,
            then_branch,
            else_branch,
        } => {
            if infer(condition, env)? != Type::Bool {
                return Err("la condition d'un `if` doit être Bool".into());
            }
            let then_ty = infer(then_branch, env)?;
            let else_ty = infer(else_branch, env)?;
            if then_ty != else_ty {
                return Err(format!(
                    "les deux branches d'un `if` doivent avoir le même type : {} et {}",
                    then_ty.name(),
                    else_ty.name()
                ));
            }
            Ok(then_ty)
        }
        Expr::Seq(items) => {
            let (last, earlier) = items
                .split_last()
                .ok_or_else(|| "une séquence vide n'a pas de type".to_string())?;
            for item in earlier {
                let ty = infer(item, env)?;
                if ty != Type::Unit {
                    return Err(format!(
                        "une expression intermédiaire doit être Unit, pas {} — sa valeur serait perdue",
                        ty.name()
                    ));
                }
            }
            infer(last, env)
        }
        Expr::Call { name, args } => {
            let Some(signature) = host(name) else {
                return Err(format!("`{name}` n'est pas une primitive de l'hôte"));
            };
            if args.len() != signature.params.len() {
                return Err(format!(
                    "`{name}` attend {} argument(s), reçu {}",
                    signature.params.len(),
                    args.len()
                ));
            }
            for (index, (arg, expected)) in args.iter().zip(signature.params).enumerate() {
                let actual = infer(arg, env)?;
                if actual != *expected {
                    return Err(format!(
                        "argument {} de `{name}` : attendu {}, reçu {}",
                        index + 1,
                        expected.name(),
                        actual.name()
                    ));
                }
            }
            Ok(signature.returns)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse::parse;

    fn accepts(source: &str) {
        let function = parse(source).unwrap();
        check(&function).expect("ce programme devrait être accepté");
    }

    fn refuses(source: &str, expected: &str) {
        let function = parse(source).unwrap();
        let error = check(&function).unwrap_err();
        assert!(
            error.contains(expected),
            "message inattendu : {error}\nattendu : {expected}"
        );
    }

    #[test]
    fn a_well_typed_program_is_accepted() {
        accepts("fn run(a: Int, b: Bool) -> Int { let c: Int = a + 1; if b { c } else { c % 2 } }");
    }

    #[test]
    fn an_unknown_variable_is_named() {
        refuses("fn run() -> Int { nope }", "`nope`");
    }

    #[test]
    fn a_binding_of_the_wrong_type_is_refused() {
        refuses(
            "fn run() -> Int { let c: Bool = 1; c }",
            "déclaré Bool mais vaut Int",
        );
    }

    #[test]
    fn arithmetic_on_a_boolean_is_refused() {
        refuses("fn run(b: Bool) -> Int { b + 1 }", "ne s'applique pas");
    }

    #[test]
    fn comparing_two_different_types_is_refused() {
        refuses("fn run() -> Bool { 1 == true }", "ne s'applique pas");
    }

    #[test]
    fn a_non_boolean_condition_is_refused() {
        refuses("fn run() -> Int { if 1 { 1 } else { 2 } }", "condition");
    }

    #[test]
    fn branches_of_different_types_are_refused() {
        refuses(
            "fn run(b: Bool) -> Int { if b { 1 } else { true } }",
            "même type",
        );
    }

    #[test]
    fn a_body_that_does_not_match_the_announced_type_is_refused() {
        refuses("fn run() -> Bool { 1 }", "annonce Bool");
    }

    #[test]
    fn a_sequence_of_effects_is_accepted() {
        accepts("fn run() -> Unit { emit(\"a\"); emit(\"b\") }");
    }

    #[test]
    fn a_value_thrown_away_before_the_end_is_refused() {
        refuses("fn run() -> Int { 1; 2 }", "intermédiaire doit être Unit");
    }

    #[test]
    fn a_string_cannot_be_bound() {
        refuses("fn run() -> Int { let s: Str = \"a\"; 1 }", "ne se lie pas");
    }

    #[test]
    fn a_string_reaches_a_host_call() {
        accepts("fn run() -> Unit { emit(\"créé\") }");
    }

    #[test]
    fn an_unknown_host_function_is_refused() {
        refuses("fn run() -> Int { exists(\"x\") }", "pas une primitive");
    }

    #[test]
    fn a_host_call_with_the_wrong_arity_is_refused() {
        refuses("fn run() -> Unit { emit() }", "attend 1 argument");
    }

    #[test]
    fn a_host_call_with_the_wrong_argument_type_is_refused() {
        refuses("fn run() -> Unit { emit(1) }", "attendu Str");
    }

    #[test]
    fn negation_is_reserved_to_numbers_and_bools_apart() {
        refuses("fn run(b: Bool) -> Bool { !1 }", "ne s'applique pas");
        refuses("fn run() -> Int { -true }", "ne s'applique pas");
    }
}
