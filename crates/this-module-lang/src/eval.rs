//! L'interpréteur. Il existe pour deux raisons, dans cet ordre :
//!
//! 1. **Une référence.** Ce que la machine doit faire est défini ici, en clair,
//!    avant toute émission wasm. Si les deux divergent, c'est l'émission qui a
//!    tort — on peut toujours revenir lire ce fichier.
//! 2. **Un test.** Un même programme s'exécute de deux façons ; le test compare
//!    les deux réponses. C'est ce qui attrape une émission fausse sans monter un
//!    moteur wasm dans la CI.
//!
//! L'hôte est un trait : l'interpréteur ne connaît aucun effet. Ce qui est
//! observable est déclaré par l'appelant (`Host`), jamais deviné.

use crate::ast::{BinOp, Expr, Function, Type, UnOp};

#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Int(i64),
    Bool(bool),
    Str(String),
    Unit,
}

impl Value {
    pub fn ty(&self) -> Type {
        match self {
            Value::Int(_) => Type::Int,
            Value::Bool(_) => Type::Bool,
            Value::Str(_) => Type::Str,
            Value::Unit => Type::Unit,
        }
    }

    pub fn name(&self) -> &'static str {
        self.ty().name()
    }

    pub fn as_int(&self) -> Option<i64> {
        match self {
            Value::Int(value) => Some(*value),
            _ => None,
        }
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Value::Bool(value) => Some(*value),
            _ => None,
        }
    }
}

/// Les primitives que l'hôte accepte d'exécuter. Le typeur restreint les appels
/// à `check::HOST` ; ce trait dit seulement *comment* elles s'exécutent, pour
/// que l'interpréteur reste libre de tout effet.
pub trait Host {
    fn call(&mut self, name: &str, args: &[Value]) -> Result<Value, String>;
}

/// Un hôte qui refuse tout : utile pour exécuter un module pur.
pub struct NoHost;

impl Host for NoHost {
    fn call(&mut self, name: &str, _args: &[Value]) -> Result<Value, String> {
        Err(format!("`{name}` n'a pas d'hôte dans ce contexte"))
    }
}

/// La profondeur maximale d'un appel récursif.
///
/// Sans elle, un programme qui s'appelle sans condition ferait déborder la pile
/// du processus hôte : c'est un défaut d'application, pas un refus de sécurité.
const PROFONDEUR_MAX: usize = 256;

/// Le nombre de tours qu'un `while` peut faire avant d'être déclaré infini.
///
/// La même raison, côté temps : un modèle qui écrit `while true` ne doit pas
/// figer l'application qui exécute son programme.
pub const TOURS_MAX: usize = 1_000_000;

pub fn run(function: &Function, args: &[Value], host: &mut impl Host) -> Result<Value, String> {
    appeler(function, args, host, 0)
}

fn appeler(
    function: &Function,
    args: &[Value],
    host: &mut impl Host,
    depth: usize,
) -> Result<Value, String> {
    if depth > PROFONDEUR_MAX {
        return Err(format!(
            "la récursion a dépassé {PROFONDEUR_MAX} appels : `{}` ne s'arrête pas",
            function.name
        ));
    }
    if args.len() != function.params.len() {
        return Err(format!(
            "`{}` attend {} argument(s), reçu {}",
            function.name,
            function.params.len(),
            args.len()
        ));
    }

    let mut scope: Vec<(String, Value)> = Vec::new();
    for (param, value) in function.params.iter().zip(args) {
        if value.ty() != param.ty {
            return Err(format!(
                "argument `{}` de `{}` : attendu {}, reçu {}",
                param.name,
                function.name,
                param.ty.name(),
                value.name()
            ));
        }
        scope.push((param.name.clone(), value.clone()));
    }

    for binding in &function.bindings {
        let value = evaluate(&binding.value, &mut scope, host, function, depth)?;
        if value.ty() != binding.ty {
            return Err(format!(
                "`{}` est déclaré {} mais vaut {}",
                binding.name,
                binding.ty.name(),
                value.name()
            ));
        }
        scope.push((binding.name.clone(), value));
    }

    let result = evaluate(&function.body, &mut scope, host, function, depth)?;
    if result.ty() != function.returns {
        return Err(format!(
            "`{}` rend {} alors qu'elle annonce {}",
            function.name,
            result.name(),
            function.returns.name()
        ));
    }
    Ok(result)
}

fn evaluate(
    expr: &Expr,
    scope: &mut Vec<(String, Value)>,
    host: &mut impl Host,
    function: &Function,
    depth: usize,
) -> Result<Value, String> {
    match expr {
        Expr::Int(value) => Ok(Value::Int(*value)),
        Expr::Bool(value) => Ok(Value::Bool(*value)),
        Expr::Str(value) => Ok(Value::Str(value.clone())),
        Expr::Var(name) => scope
            .iter()
            .rev()
            .find(|(candidate, _)| candidate == name)
            .map(|(_, value)| value.clone())
            .ok_or_else(|| format!("`{name}` n'est pas lié à l'exécution")),
        Expr::Unary { op, expr } => {
            let inner = evaluate(expr, scope, host, function, depth)?;
            match (op, inner) {
                (UnOp::Not, Value::Bool(value)) => Ok(Value::Bool(!value)),
                (UnOp::Neg, Value::Int(value)) => value
                    .checked_neg()
                    .map(Value::Int)
                    .ok_or_else(|| "débordement sur la négation".to_string()),
                (op, other) => Err(format!("`{op:?}` ne s'applique pas à {}", other.name())),
            }
        }
        Expr::Binary { op, left, right } => {
            // Les deux opérateurs logiques court-circuitent : `false && boom()`
            // ne doit pas appeler `boom`. Le compilateur wasm doit faire pareil.
            if let BinOp::And = op {
                let left = evaluate(left, scope, host, function, depth)?;
                return match left {
                    Value::Bool(false) => Ok(Value::Bool(false)),
                    Value::Bool(true) => evaluate(right, scope, host, function, depth),
                    other => Err(format!("`&&` ne s'applique pas à {}", other.name())),
                };
            }
            if let BinOp::Or = op {
                let left = evaluate(left, scope, host, function, depth)?;
                return match left {
                    Value::Bool(true) => Ok(Value::Bool(true)),
                    Value::Bool(false) => evaluate(right, scope, host, function, depth),
                    other => Err(format!("`||` ne s'applique pas à {}", other.name())),
                };
            }

            let left = evaluate(left, scope, host, function, depth)?;
            let right = evaluate(right, scope, host, function, depth)?;
            binary(*op, left, right)
        }
        Expr::If {
            condition,
            then_branch,
            else_branch,
        } => match evaluate(condition, scope, host, function, depth)? {
            Value::Bool(true) => evaluate(then_branch, scope, host, function, depth),
            Value::Bool(false) => evaluate(else_branch, scope, host, function, depth),
            other => Err(format!("une condition ne peut pas être {}", other.name())),
        },
        Expr::Seq(items) => {
            let mut last = Value::Unit;
            for item in items {
                last = evaluate(item, scope, host, function, depth)?;
            }
            Ok(last)
        }
        Expr::Call { name, args } => {
            let mut values = Vec::with_capacity(args.len());
            for arg in args {
                values.push(evaluate(arg, scope, host, function, depth)?);
            }
            if name == &function.name {
                // Un programme qui s'appelle lui-même : la récursion.
                return appeler(function, &values, host, depth + 1);
            }
            host.call(name, &values)
        }
        Expr::Set { name, value } => {
            let value = evaluate(value, scope, host, function, depth)?;
            let Some(slot) = scope
                .iter_mut()
                .rev()
                .find(|(candidate, _)| candidate == name)
            else {
                return Err(format!("`{name}` n'est pas lié à l'exécution"));
            };
            slot.1 = value;
            Ok(Value::Unit)
        }
        Expr::While { condition, body } => {
            let mut tours = 0usize;
            loop {
                match evaluate(condition, scope, host, function, depth)? {
                    Value::Bool(false) => break,
                    Value::Bool(true) => {}
                    other => {
                        return Err(format!("une condition ne peut pas être {}", other.name()));
                    }
                }
                // Le typeur a vérifié que le corps rend Unit : rien ne s'empile.
                evaluate(body, scope, host, function, depth)?;
                tours += 1;
                if tours >= TOURS_MAX {
                    return Err(format!(
                        "la boucle a dépassé {TOURS_MAX} tours sans s'arrêter"
                    ));
                }
            }
            Ok(Value::Unit)
        }
    }
}

fn binary(op: BinOp, left: Value, right: Value) -> Result<Value, String> {
    let pair = match (&left, &right) {
        (Value::Int(a), Value::Int(b)) => Some((*a, *b)),
        _ => None,
    };

    match (op, pair) {
        (BinOp::Add, Some((a, b))) => a
            .checked_add(b)
            .map(Value::Int)
            .ok_or_else(|| "débordement sur l'addition".to_string()),
        (BinOp::Sub, Some((a, b))) => a
            .checked_sub(b)
            .map(Value::Int)
            .ok_or_else(|| "débordement sur la soustraction".to_string()),
        (BinOp::Mul, Some((a, b))) => a
            .checked_mul(b)
            .map(Value::Int)
            .ok_or_else(|| "débordement sur la multiplication".to_string()),
        (BinOp::Div, Some((_, 0))) => Err("division par zéro".into()),
        (BinOp::Div, Some((a, b))) => a
            .checked_div(b)
            .map(Value::Int)
            .ok_or_else(|| "débordement sur la division".to_string()),
        (BinOp::Rem, Some((_, 0))) => Err("reste par zéro".into()),
        (BinOp::Rem, Some((a, b))) => a
            .checked_rem(b)
            .map(Value::Int)
            .ok_or_else(|| "débordement sur le reste".to_string()),
        (BinOp::Lt, Some((a, b))) => Ok(Value::Bool(a < b)),
        (BinOp::Le, Some((a, b))) => Ok(Value::Bool(a <= b)),
        (BinOp::Gt, Some((a, b))) => Ok(Value::Bool(a > b)),
        (BinOp::Ge, Some((a, b))) => Ok(Value::Bool(a >= b)),
        (BinOp::Eq, _) if left.ty() == right.ty() && left.ty() != Type::Unit => {
            Ok(Value::Bool(left == right))
        }
        (BinOp::Ne, _) if left.ty() == right.ty() && left.ty() != Type::Unit => {
            Ok(Value::Bool(left != right))
        }
        (op, _) => Err(format!(
            "`{}` ne s'applique pas à {} et {}",
            op.symbol(),
            left.name(),
            right.name()
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::check;
    use crate::parse::parse;

    /// Un hôte d'essai qui enregistre ce qu'on lui demande.
    #[derive(Default)]
    struct Recorder {
        emitted: Vec<String>,
        calls: usize,
        now: i64,
    }

    impl Host for Recorder {
        fn call(&mut self, name: &str, args: &[Value]) -> Result<Value, String> {
            self.calls += 1;
            match (name, args) {
                ("emit", [Value::Str(text)]) => {
                    self.emitted.push(text.clone());
                    Ok(Value::Unit)
                }
                ("now", []) => Ok(Value::Int(self.now)),
                // L'appareil factice : il répond 42 à toute clé, et note l'appel.
                ("device", [Value::Str(m), Value::Str(a)]) => {
                    self.emitted.push(format!("{m}({a})"));
                    Ok(Value::Bool(true))
                }
                ("device_int", [Value::Str(m), Value::Str(a), Value::Str(k)]) => {
                    self.emitted.push(format!("{m}({a}).{k}"));
                    Ok(Value::Int(42))
                }
                ("device_emit", [Value::Str(m), Value::Str(a), Value::Str(k)]) => {
                    self.emitted.push(format!("{m}({a}).{k} →"));
                    Ok(Value::Unit)
                }
                _ => Err(format!("l'hôte ne connaît pas `{name}` avec ces arguments")),
            }
        }
    }

    fn execute(source: &str, args: &[Value]) -> Result<Value, String> {
        let function = parse(source).unwrap();
        check::check(&function).unwrap();
        run(&function, args, &mut Recorder::default())
    }

    fn execute_with(source: &str, args: &[Value]) -> (Result<Value, String>, Recorder) {
        let function = parse(source).unwrap();
        check::check(&function).unwrap();
        let mut host = Recorder::default();
        let value = run(&function, args, &mut host);
        (value, host)
    }

    #[test]
    fn arithmetic_follows_precedence() {
        assert_eq!(
            execute("fn run() -> Int { 1 + 2 * 3 }", &[]).unwrap(),
            Value::Int(7)
        );
        assert_eq!(
            execute("fn run() -> Int { (1 + 2) * 3 }", &[]).unwrap(),
            Value::Int(9)
        );
        assert_eq!(
            execute("fn run() -> Int { 7 % 3 }", &[]).unwrap(),
            Value::Int(1)
        );
    }

    #[test]
    fn parameters_and_bindings_are_visible() {
        assert_eq!(
            execute(
                "fn run(a: Int) -> Int { let b: Int = a * 2; b + 1 }",
                &[Value::Int(5)]
            )
            .unwrap(),
            Value::Int(11)
        );
    }

    #[test]
    fn an_if_chooses_exactly_one_branch() {
        let source = "fn run(a: Int) -> Int { if a > 0 { 1 } else { -1 } }";
        assert_eq!(execute(source, &[Value::Int(3)]).unwrap(), Value::Int(1));
        assert_eq!(execute(source, &[Value::Int(-3)]).unwrap(), Value::Int(-1));
    }

    /// Un court-circuit qui n'est pas observé n'est pas testé : `now()` compte
    /// ses appels, donc zéro appel prouve que la branche droite n'a pas été
    /// évaluée. C'est la seule preuve qui vaille.
    #[test]
    fn and_does_not_evaluate_its_right_side_on_false() {
        let (value, host) = execute_with("fn run() -> Bool { false && now() > 0 }", &[]);
        assert_eq!(value.unwrap(), Value::Bool(false));
        assert_eq!(host.calls, 0, "la branche droite a été évaluée");
    }

    #[test]
    fn and_evaluates_its_right_side_on_true() {
        let (value, host) = execute_with("fn run() -> Bool { true && now() > 0 }", &[]);
        assert_eq!(value.unwrap(), Value::Bool(false));
        assert_eq!(host.calls, 1);
    }

    #[test]
    fn or_does_not_evaluate_its_right_side_on_true() {
        let (value, host) = execute_with("fn run() -> Bool { true || now() > 0 }", &[]);
        assert_eq!(value.unwrap(), Value::Bool(true));
        assert_eq!(host.calls, 0, "la branche droite a été évaluée");
    }

    #[test]
    fn a_sequence_runs_its_effects_in_order() {
        let (value, host) = execute_with("fn run() -> Unit { emit(\"a\"); emit(\"b\") }", &[]);
        assert_eq!(value.unwrap(), Value::Unit);
        assert_eq!(host.emitted, vec!["a".to_string(), "b".to_string()]);
    }

    #[test]
    fn a_host_call_is_recorded_once() {
        let (value, host) = execute_with("fn run() -> Unit { emit(\"créé\") }", &[]);
        assert_eq!(value.unwrap(), Value::Unit);
        assert_eq!(host.emitted, vec!["créé".to_string()]);
    }

    #[test]
    fn division_by_zero_is_an_error_not_a_panic() {
        let error = execute("fn run(a: Int) -> Int { 1 / a }", &[Value::Int(0)]).unwrap_err();
        assert!(error.contains("division par zéro"), "{error}");
    }

    #[test]
    fn an_overflow_is_an_error_not_a_wrap() {
        let error = execute("fn run() -> Int { 9223372036854775807 + 1 }", &[]).unwrap_err();
        assert!(error.contains("débordement"), "{error}");
    }

    #[test]
    fn a_wrong_argument_type_is_refused_before_running() {
        let error = execute("fn run(a: Int) -> Int { a }", &[Value::Bool(true)]).unwrap_err();
        assert!(error.contains("attendu Int"), "{error}");
    }

    #[test]
    fn the_host_can_refuse() {
        let function = parse("fn run() -> Int { now() }").unwrap();
        let error = run(&function, &[], &mut NoHost).unwrap_err();
        assert!(error.contains("pas d'hôte"), "{error}");
    }

    #[test]
    fn un_programme_appelle_l_appareil_et_lit_un_nombre() {
        let (result, host) = execute_with(
            "fn run() -> Int { if device(\"notificationsPost\", \"{\\\"title\\\":\\\"ok\\\"}\") { device_int(\"healthSteps\", \"{}\", \"count\") + 1 } else { 0 } }",
            &[],
        );
        assert_eq!(result, Ok(Value::Int(43)));
        assert_eq!(
            host.emitted,
            vec![
                "notificationsPost({\"title\":\"ok\"})".to_string(),
                "healthSteps({}).count".to_string()
            ]
        );
    }

    #[test]
    fn un_hote_sans_appareil_refuse_sans_rien_executer_d_autre() {
        let function =
            parse("fn run() -> Int { device_int(\"healthSteps\", \"{}\", \"count\") }").unwrap();
        check::check(&function).unwrap();
        let error = run(&function, &[], &mut NoHost).unwrap_err();
        assert!(error.contains("device_int"), "{error}");
    }
}
