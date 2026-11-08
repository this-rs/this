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

/// Les primitives d'un **module exécuté**. Deux vivent partout : `emit` écrit un
/// effet, `now` lit l'horloge. Les autres demandent la **coquille** (l'hôte natif) :
/// l'appareil et le réseau. Un navigateur ne les fournit pas, parce qu'un appel
/// d'appareil est asynchrone et consenti — l'hôte qui les exécute le dit par
/// [`crate::wat::Wat::shell_only`].
///
/// Les chaînes du langage sont des littéraux : une primitive peut en recevoir, pas
/// en rendre une exploitable. L'appareil rend donc un **nombre** ou un **booléen**
/// au programme, et verse le texte de sa réponse dans les effets, que la personne
/// et l'agent lisent. La capacité (caméra, position…) est dérivée de la méthode par
/// l'hôte, jamais déclarée par le programme, et le consentement est demandé au nom
/// du module, une fois par capacité.
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
    // `device(méthode, arguments JSON)` : l'appel a réussi. La réponse va dans les effets.
    HostFn {
        name: "device",
        params: &[Type::Str, Type::Str],
        returns: Type::Bool,
    },
    // `device_int(méthode, arguments JSON, clé)` : le nombre à cette clé de la réponse.
    // Une réponse sans ce nombre est un échec du programme, pas un zéro silencieux.
    HostFn {
        name: "device_int",
        params: &[Type::Str, Type::Str, Type::Str],
        returns: Type::Int,
    },
    // `device_emit(méthode, arguments JSON, clé)` : verse dans les effets la valeur à
    // cette clé (clé vide : toute la réponse).
    HostFn {
        name: "device_emit",
        params: &[Type::Str, Type::Str, Type::Str],
        returns: Type::Unit,
    },
    // `http_int(url, clé)` : un GET borné, le nombre à cette clé du JSON reçu.
    HostFn {
        name: "http_int",
        params: &[Type::Str, Type::Str],
        returns: Type::Int,
    },
    // `http_emit(url, clé)` : verse dans les effets la valeur à cette clé (clé vide :
    // le corps entier, borné).
    HostFn {
        name: "http_emit",
        params: &[Type::Str, Type::Str],
        returns: Type::Unit,
    },
];

/// Les primitives qu'un **navigateur** sait fournir à un module compilé en wasm :
/// synchrones et sans consentement. Tout autre import exige la coquille.
pub const HOST_NAVIGATEUR: &[&str] = &["emit", "now"];

/// La primitive de [`HOST`] qui porte ce nom.
pub fn host_fn(name: &str) -> Option<&'static HostFn> {
    host(HOST, name)
}

/// Les primitives d'une **règle** : pures, déterministes, sans effet.
///
/// C'est une surface différente de [`HOST`], et c'est le point : une règle ne
/// peut ni écrire ni lire l'horloge, parce qu'une règle doit rendre le même
/// verdict sur le serveur et dans le front, et deux fois de suite. Ce qui n'est
/// pas ici n'est pas appelable — la sûreté est dans la table, pas dans la
/// discipline de l'auteur.
///
/// Les chaînes y entrent, mais restent **opaques** : on les compare, on les
/// mesure, on cherche un préfixe. Aucune n'est construite, donc aucune mémoire
/// n'est allouée — c'est ce qui permet de traiter des chaînes sans modèle
/// mémoire.
pub const PURE: &[HostFn] = &[
    HostFn {
        name: "str_len",
        params: &[Type::Str],
        returns: Type::Int,
    },
    HostFn {
        name: "str_eq",
        params: &[Type::Str, Type::Str],
        returns: Type::Bool,
    },
    HostFn {
        name: "str_starts_with",
        params: &[Type::Str, Type::Str],
        returns: Type::Bool,
    },
    HostFn {
        name: "str_ends_with",
        params: &[Type::Str, Type::Str],
        returns: Type::Bool,
    },
    HostFn {
        name: "str_contains",
        params: &[Type::Str, Type::Str],
        returns: Type::Bool,
    },
];

fn host<'a>(surface: &'a [HostFn], name: &str) -> Option<&'a HostFn> {
    surface.iter().find(|function| function.name == name)
}

struct Env {
    vars: Vec<(String, Type, bool)>,
    /// La signature de la fonction en cours : un programme peut s'appeler
    /// lui-même. `None` pour une règle, qui n'a pas de nom à rappeler.
    recursion: Option<(String, Vec<Type>, Type)>,
}

impl Env {
    fn get(&self, name: &str) -> Option<Type> {
        self.vars
            .iter()
            .rev()
            .find(|(candidate, _, _)| candidate == name)
            .map(|(_, ty, _)| *ty)
    }

    fn mutable(&self, name: &str) -> Option<bool> {
        self.vars
            .iter()
            .rev()
            .find(|(candidate, _, _)| candidate == name)
            .map(|(_, _, mutable)| *mutable)
    }
}

/// Vérifie un programme contre la surface d'un **module exécuté**.
pub fn check(function: &Function) -> Result<Type, String> {
    check_with(function, HOST)
}

/// Vérifie un programme contre une surface donnée. Le typeur ne connaît aucune
/// primitive en dur : ce qu'un programme peut appeler est exactement ce que
/// l'appelant met dans `surface`.
pub fn check_with(function: &Function, surface: &[HostFn]) -> Result<Type, String> {
    let mut env = Env {
        vars: function
            .params
            .iter()
            .map(|param| (param.name.clone(), param.ty, false))
            .collect(),
        recursion: Some((
            function.name.clone(),
            function.params.iter().map(|param| param.ty).collect(),
            function.returns,
        )),
    };

    for binding in &function.bindings {
        let value = infer(&binding.value, &env, surface)?;
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
        env.vars
            .push((binding.name.clone(), binding.ty, binding.mutable));
    }

    let body = infer(&function.body, &env, surface)?;
    if body != function.returns {
        return Err(format!(
            "le corps rend {} alors que la fonction annonce {}",
            body.name(),
            function.returns.name()
        ));
    }
    Ok(body)
}

fn infer(expr: &Expr, env: &Env, surface: &[HostFn]) -> Result<Type, String> {
    match expr {
        Expr::Int(_) => Ok(Type::Int),
        Expr::Bool(_) => Ok(Type::Bool),
        Expr::Str(_) => Ok(Type::Str),
        Expr::Var(name) => env
            .get(name)
            .ok_or_else(|| format!("`{name}` n'est ni un paramètre ni une liaison")),
        Expr::Unary { op, expr } => {
            let inner = infer(expr, env, surface)?;
            match (op, inner) {
                (UnOp::Not, Type::Bool) => Ok(Type::Bool),
                (UnOp::Neg, Type::Int) => Ok(Type::Int),
                _ => Err(format!("`{op:?}` ne s'applique pas à {}", inner.name())),
            }
        }
        Expr::Binary { op, left, right } => {
            let left = infer(left, env, surface)?;
            let right = infer(right, env, surface)?;
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
            if infer(condition, env, surface)? != Type::Bool {
                return Err("la condition d'un `if` doit être Bool".into());
            }
            let then_ty = infer(then_branch, env, surface)?;
            let else_ty = infer(else_branch, env, surface)?;
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
                let ty = infer(item, env, surface)?;
                if ty != Type::Unit {
                    return Err(format!(
                        "une expression intermédiaire doit être Unit, pas {} — sa valeur serait perdue",
                        ty.name()
                    ));
                }
            }
            infer(last, env, surface)
        }
        Expr::Set { name, value } => {
            let Some(declared) = env.get(name) else {
                return Err(format!("`{name}` n'est pas déclaré"));
            };
            if !env.mutable(name).unwrap_or(false) {
                return Err(format!(
                    "`{name}` est une liaison `let` : déclare-la `var` pour pouvoir la changer"
                ));
            }
            let actual = infer(value, env, surface)?;
            if actual != declared {
                return Err(format!(
                    "`{name}` est {} : on ne peut pas y mettre {}",
                    declared.name(),
                    actual.name()
                ));
            }
            Ok(Type::Unit)
        }
        Expr::While { condition, body } => {
            let condition = infer(condition, env, surface)?;
            if condition != Type::Bool {
                return Err(format!(
                    "la condition d'un `while` doit être Bool, pas {}",
                    condition.name()
                ));
            }
            let body = infer(body, env, surface)?;
            if body != Type::Unit {
                return Err(format!(
                    "le corps d'un `while` doit être Unit, pas {} — sa valeur serait perdue à chaque tour",
                    body.name()
                ));
            }
            Ok(Type::Unit)
        }
        Expr::Call { name, args } => {
            // Un programme peut s'appeler lui-même : c'est la récursion, bornée
            // à l'exécution par la profondeur de pile.
            if let Some((own, params, returns)) = &env.recursion
                && own == name
            {
                if args.len() != params.len() {
                    return Err(format!(
                        "`{name}` attend {} argument(s), reçu {}",
                        params.len(),
                        args.len()
                    ));
                }
                for (index, (arg, expected)) in args.iter().zip(params).enumerate() {
                    let actual = infer(arg, env, surface)?;
                    if actual != *expected {
                        return Err(format!(
                            "argument {} de `{name}` : attendu {}, reçu {}",
                            index + 1,
                            expected.name(),
                            actual.name()
                        ));
                    }
                }
                return Ok(*returns);
            }
            let Some(signature) = host(surface, name) else {
                return Err(format!("`{name}` n'est pas une primitive disponible ici"));
            };
            if args.len() != signature.params.len() {
                return Err(format!(
                    "`{name}` attend {} argument(s), reçu {}",
                    signature.params.len(),
                    args.len()
                ));
            }
            for (index, (arg, expected)) in args.iter().zip(signature.params).enumerate() {
                let actual = infer(arg, env, surface)?;
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
        // L'appareil : des littéraux en entrée, un nombre ou un booléen en sortie.
        assert_eq!(
            check(&parse("fn run() -> Bool { device(\"notificationsPost\", \"{}\") }").unwrap())
                .unwrap(),
            Type::Bool
        );
        assert_eq!(
            check(
                &parse("fn run() -> Int { device_int(\"healthSteps\", \"{}\", \"count\") }")
                    .unwrap()
            )
            .unwrap(),
            Type::Int
        );
        assert_eq!(
            check(
                &parse("fn run() -> Unit { device_emit(\"locationCurrent\", \"{}\", \"\") }")
                    .unwrap()
            )
            .unwrap(),
            Type::Unit
        );
        assert_eq!(
            check(
                &parse("fn run() -> Int { http_int(\"https://x.test/a.json\", \"n\") }").unwrap()
            )
            .unwrap(),
            Type::Int
        );
        assert_eq!(
            check(
                &parse("fn run() -> Unit { http_emit(\"https://x.test/a.json\", \"\") }").unwrap()
            )
            .unwrap(),
            Type::Unit
        );
        refuses("fn run() -> Bool { device(\"a\") }", "attend 2 argument(s)");
        refuses(
            "fn run() -> Int { device_int(\"a\", 3, \"k\") }",
            "attendu Str, reçu Int",
        );
        // Une primitive d'appareil ne peut pas être une règle.
        assert!(
            check_with(
                &parse("fn run() -> Bool { device(\"a\", \"{}\") }").unwrap(),
                PURE
            )
            .is_err()
        );
        assert_eq!(host_fn("device").map(|h| h.returns), Some(Type::Bool));
        assert!(host_fn("exists").is_none());
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
