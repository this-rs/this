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

pub fn run(function: &Function, args: &[Value], host: &mut impl Host) -> Result<Value, String> {
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
        let value = evaluate(&binding.value, &mut scope, host)?;
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

    let result = evaluate(&function.body, &mut scope, host)?;
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
            let inner = evaluate(expr, scope, host)?;
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
                let left = evaluate(left, scope, host)?;
                return match left {
                    Value::Bool(false) => Ok(Value::Bool(false)),
                    Value::Bool(true) => evaluate(right, scope, host),
                    other => Err(format!("`&&` ne s'applique pas à {}", other.name())),
                };
            }
            if let BinOp::Or = op {
                let left = evaluate(left, scope, host)?;
                return match left {
                    Value::Bool(true) => Ok(Value::Bool(true)),
                    Value::Bool(false) => evaluate(right, scope, host),
                    other => Err(format!("`||` ne s'applique pas à {}", other.name())),
                };
            }

            let left = evaluate(left, scope, host)?;
            let right = evaluate(right, scope, host)?;
            binary(*op, left, right)
        }
        Expr::If {
            condition,
            then_branch,
            else_branch,
        } => match evaluate(condition, scope, host)? {
            Value::Bool(true) => evaluate(then_branch, scope, host),
            Value::Bool(false) => evaluate(else_branch, scope, host),
            other => Err(format!("une condition ne peut pas être {}", other.name())),
        },
        Expr::Seq(items) => {
            let mut last = Value::Unit;
            for item in items {
                last = evaluate(item, scope, host)?;
            }
            Ok(last)
        }
        Expr::Call { name, args } => {
            let mut values = Vec::with_capacity(args.len());
            for arg in args {
                values.push(evaluate(arg, scope, host)?);
            }
            host.call(name, &values)
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
}
