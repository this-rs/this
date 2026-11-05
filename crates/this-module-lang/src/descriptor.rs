//! Un programme **est** une règle de descripteur.
//!
//! `this-descriptor` a déjà la couture qu'il faut : un `Custom { name, params }`
//! déclaré dans un descripteur est résolu par nom dans un `CustomRegistry`, et un
//! nom absent de ce registre fait échouer `check()` — jamais ignorer, donc
//! fail-closed par construction. Ce module enregistre, sous un nom, un validateur
//! dont l'implémentation est un programme `this-module-lang`.
//!
//! Ce que ça donne, concrètement : une règle métier s'écrit une fois, en clair,
//! dans le descripteur du module — au lieu d'un `Custom` dont le code Rust vit
//! ailleurs et que le descripteur ne peut pas montrer.
//!
//! ## Le contrat, entier
//!
//! - les **paramètres du programme** sont ses entrées, résolues par nom ;
//! - le programme rend `Bool` : `true` = valide, `false` = violation ;
//! - le programme n'appelle **aucune** primitive d'hôte : une règle de validation
//!   qui écrit n'est pas une règle de validation ;
//! - seuls `Int` et `Bool` entrent (pas de modèle mémoire dans le langage) : un
//!   champ chaîne, un objet ou une liste sont refusés à l'appel, avec un code qui
//!   dit pourquoi.
//!
//! Le programme est compilé **à l'enregistrement** : une règle cassée fait échouer
//! le démarrage de l'hôte, pas la première requête. C'est le même choix que
//! partout ailleurs ici : on refuse tôt, et on refuse bruyamment.

use std::sync::Arc;

use this_descriptor::custom::{CustomRegistry, CustomScope, CustomValidatorFn};
use this_descriptor::error::FieldViolation;

use crate::ast::{Expr, Function, Type};
use crate::eval::{self, Host, Value};
use crate::{check, parse};

/// Le champ reçu n'a pas le type attendu par le programme (`Int` ou `Bool`).
pub const CODE_INPUT_TYPE: &str = "this.lang.input_type";
/// Un paramètre du programme n'a pas d'entrée correspondante.
pub const CODE_MISSING_INPUT: &str = "this.lang.missing_input";
/// Le programme a échoué à l'exécution (division par zéro, débordement).
pub const CODE_EVALUATION: &str = "this.lang.evaluation";

/// Le nom que porte l'entrée qui reçoit la valeur du champ.
const FIELD_VALUE: &str = "value";

/// Un programme compilé, prêt à valider.
#[derive(Clone, Debug)]
pub struct Rule {
    /// Le nom sous lequel la règle est enregistrée — il devient le code d'erreur
    /// de la violation, donc le descripteur et l'erreur disent la même chose.
    name: String,
    function: Function,
}

impl Rule {
    /// Compile un programme en règle. Toute erreur est définitive : un programme
    /// refusé ici ne sera jamais exécuté.
    pub fn compile(name: impl Into<String>, source: &str) -> Result<Self, String> {
        let function = parse::parse(source)?;
        check::check(&function)?;

        if function.returns != Type::Bool {
            return Err(format!(
                "une règle rend Bool (true = valide), pas {}",
                function.returns.name()
            ));
        }

        let mut called = Vec::new();
        for binding in &function.bindings {
            calls(&binding.value, &mut called);
        }
        calls(&function.body, &mut called);
        if let Some(name) = called.first() {
            return Err(format!(
                "une règle de validation n'appelle aucune primitive : `{name}` est appelé"
            ));
        }

        Ok(Rule {
            name: name.into(),
            function,
        })
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    /// La fonction que `CustomRegistry` attend. Elle capture la règle compilée :
    /// une requête ne paie jamais la compilation.
    pub fn validator(self: &Arc<Self>) -> CustomValidatorFn {
        let rule = Arc::clone(self);
        Arc::new(move |field, value, params| rule.apply(field, value, params))
    }

    /// `Ok(())` = valide. `Err` = violation, avec le nom de la règle comme code.
    pub fn apply(
        &self,
        field: &str,
        value: &serde_json::Value,
        params: &serde_json::Value,
    ) -> Result<(), FieldViolation> {
        let mut args = Vec::with_capacity(self.function.params.len());
        for param in &self.function.params {
            let raw = params
                .get(&param.name)
                .filter(|candidate| candidate.is_number() || candidate.is_boolean())
                .or_else(|| (param.name == FIELD_VALUE || param.name == field).then_some(value))
                .ok_or_else(|| {
                    FieldViolation::new(field, CODE_MISSING_INPUT)
                        .with_param("input", &param.name)
                        .with_param("rule", &self.name)
                })?;

            args.push(to_value(field, &param.name, param.ty, raw)?);
        }

        match eval::run(&self.function, &args, &mut NoEffects) {
            Ok(Value::Bool(true)) => Ok(()),
            Ok(Value::Bool(false)) => Err(FieldViolation::new(field, self.name.as_str())),
            Ok(other) => {
                Err(FieldViolation::new(field, CODE_EVALUATION).with_param("result", other.name()))
            }
            Err(error) => {
                Err(FieldViolation::new(field, CODE_EVALUATION).with_param("error", error))
            }
        }
    }
}

fn to_value(
    field: &str,
    param: &str,
    ty: Type,
    raw: &serde_json::Value,
) -> Result<Value, FieldViolation> {
    let mismatch = || {
        FieldViolation::new(field, CODE_INPUT_TYPE)
            .with_param("input", param)
            .with_param("expected", ty.name())
    };
    match ty {
        Type::Int => raw
            .as_i64()
            .or_else(|| raw.as_u64().and_then(|n| i64::try_from(n).ok()))
            .map(Value::Int)
            .ok_or_else(mismatch),
        Type::Bool => raw.as_bool().map(Value::Bool).ok_or_else(mismatch),
        Type::Str | Type::Unit => Err(mismatch()),
    }
}

/// Un hôte qui refuse : une règle compilée ne peut donc rien déclencher. Le refus
/// est déjà intervenu à la compilation ; ceci est la seconde barrière.
struct NoEffects;

impl Host for NoEffects {
    fn call(&mut self, name: &str, _args: &[Value]) -> Result<Value, String> {
        Err(format!("une règle ne peut pas appeler `{name}`"))
    }
}

/// Enregistre une règle sous `name` dans un registre de descripteur.
///
/// Un descripteur qui déclare `validator: { custom: { name } }` la voit
/// s'appliquer — et un descripteur qui déclare un nom absent du registre est
/// refusé par `check()`, comme avant.
pub fn register_rule(
    registry: &mut CustomRegistry,
    name: &str,
    source: &str,
    scope: CustomScope,
) -> Result<(), String> {
    let rule = Arc::new(Rule::compile(name, source)?);
    registry.register_validator(name, scope, rule.validator());
    Ok(())
}

/// Tous les noms de primitives appelés, dans l'ordre de lecture.
fn calls(expr: &Expr, out: &mut Vec<String>) {
    match expr {
        Expr::Call { name, args } => {
            out.push(name.clone());
            for arg in args {
                calls(arg, out);
            }
        }
        Expr::Binary { left, right, .. } => {
            calls(left, out);
            calls(right, out);
        }
        Expr::Unary { expr, .. } => calls(expr, out),
        Expr::If {
            condition,
            then_branch,
            else_branch,
        } => {
            calls(condition, out);
            calls(then_branch, out);
            calls(else_branch, out);
        }
        Expr::Seq(items) => {
            for item in items {
                calls(item, out);
            }
        }
        Expr::Int(_) | Expr::Bool(_) | Expr::Str(_) | Expr::Var(_) => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn rule(source: &str) -> Arc<Rule> {
        Arc::new(Rule::compile("billing.amount_in_range", source).unwrap())
    }

    #[test]
    fn a_range_rule_accepts_and_refuses() {
        let r = rule("fn run(value: Int, max: Int) -> Bool { value > 0 && value <= max }");
        let params = json!({"max": 10});
        assert!(r.apply("amount", &json!(5), &params).is_ok());
        assert!(r.apply("amount", &json!(0), &params).is_err());
        assert!(r.apply("amount", &json!(11), &params).is_err());
    }

    #[test]
    fn a_refusal_carries_the_rule_name_as_its_code() {
        let r = rule("fn run(value: Int) -> Bool { value > 0 }");
        let violation = r.apply("amount", &json!(-1), &json!({})).unwrap_err();
        assert_eq!(violation.code.as_str(), "billing.amount_in_range");
        assert_eq!(violation.path, "amount");
    }

    #[test]
    fn the_field_answers_to_its_own_name_and_to_value() {
        // Un programme peut nommer son entrée comme le champ...
        let named = rule("fn run(amount: Int) -> Bool { amount > 0 }");
        assert!(named.apply("amount", &json!(1), &json!({})).is_ok());
        // ...ou l'appeler `value`, ce qui rend la règle réutilisable.
        let generic = rule("fn run(value: Int) -> Bool { value > 0 }");
        assert!(generic.apply("total", &json!(1), &json!({})).is_ok());
    }

    #[test]
    fn a_boolean_field_is_valid() {
        let r = rule("fn run(value: Bool) -> Bool { value }");
        assert!(r.apply("active", &json!(true), &json!({})).is_ok());
        assert!(r.apply("active", &json!(false), &json!({})).is_err());
    }

    #[test]
    fn a_string_field_is_refused_with_a_reason() {
        let r = rule("fn run(value: Int) -> Bool { value > 0 }");
        let violation = r.apply("name", &json!("abc"), &json!({})).unwrap_err();
        assert_eq!(violation.code.as_str(), CODE_INPUT_TYPE);
        assert_eq!(
            violation.params.get("expected").and_then(|v| v.as_str()),
            Some("Int")
        );
    }

    #[test]
    fn a_missing_input_is_named() {
        let r = rule("fn run(value: Int, max: Int) -> Bool { value <= max }");
        let violation = r.apply("amount", &json!(1), &json!({})).unwrap_err();
        assert_eq!(violation.code.as_str(), CODE_MISSING_INPUT);
        assert_eq!(
            violation.params.get("input").and_then(|v| v.as_str()),
            Some("max")
        );
    }

    #[test]
    fn a_rule_that_would_emit_is_refused_at_compilation() {
        let error = Rule::compile(
            "billing.x",
            "fn run(value: Int) -> Bool { emit(\"a\"); value > 0 }",
        )
        .unwrap_err();
        assert!(error.contains("emit"), "{error}");
        assert!(error.contains("aucune primitive"), "{error}");
    }

    #[test]
    fn a_rule_that_does_not_return_a_boolean_is_refused() {
        let error =
            Rule::compile("billing.x", "fn run(value: Int) -> Int { value * 2 }").unwrap_err();
        assert!(error.contains("rend Bool"), "{error}");
    }

    #[test]
    fn a_broken_program_fails_at_registration_not_at_call_time() {
        let error =
            Rule::compile("billing.x", "fn run(value: Int) -> Bool { value > }").unwrap_err();
        assert!(error.starts_with("ligne 1"), "{error}");
    }

    /// La couture, éprouvée de bout en bout : un descripteur déclare la règle **par
    /// son nom**, le registre la résout, le moteur de validation l'applique. C'est
    /// ce test qui distingue « ma closure fonctionne » de « un module peut s'en
    /// servir ».
    #[test]
    fn a_descriptor_applies_the_rule_through_the_real_engine() {
        use this_descriptor::CompiledEntity;
        use this_descriptor::ValidatorSpec;
        use this_descriptor::custom::Custom;
        use this_descriptor::engine::validate::{Side, ValidateCtx, WriteOp, apply};
        use this_descriptor::entity::EntityDesc;
        use this_descriptor::field::{Applies, FieldDesc, FieldType};

        let mut registry = CustomRegistry::default();
        register_rule(
            &mut registry,
            "billing.amount_in_range",
            "fn run(value: Int, max: Int) -> Bool { value > 0 && value <= max }",
            CustomScope::Both,
        )
        .unwrap();

        let mut entity = EntityDesc::new("invoice");
        entity.fields = vec![FieldDesc::new("amount", FieldType::U64).validator(
            ValidatorSpec::Custom(Custom {
                name: "billing.amount_in_range".into(),
                params: json!({ "max": 10 }),
            }),
            Applies::Always,
        )];
        let entity = CompiledEntity::new("billing", entity).unwrap();
        let ctx = ValidateCtx {
            registry: &registry,
            side: Side::Server,
        };

        assert!(apply(&entity, WriteOp::Create, json!({"amount": 5}), &ctx).is_ok());

        let error = apply(&entity, WriteOp::Create, json!({"amount": 11}), &ctx).unwrap_err();
        let codes: Vec<String> = error
            .fields
            .iter()
            .map(|v| format!("{}:{}", v.path, v.code.as_str()))
            .collect();
        assert_eq!(codes, vec!["amount:billing.amount_in_range"]);
    }

    #[test]
    fn a_descriptor_naming_an_unregistered_rule_is_refused() {
        use this_descriptor::ValidatorSpec;
        use this_descriptor::custom::Custom;
        use this_descriptor::engine::validate::{Side, ValidateCtx, WriteOp, apply};
        use this_descriptor::entity::EntityDesc;
        use this_descriptor::field::{Applies, FieldDesc, FieldType};

        let mut entity = EntityDesc::new("invoice");
        entity.fields = vec![FieldDesc::new("amount", FieldType::U64).validator(
            ValidatorSpec::Custom(Custom::named("billing.never_registered")),
            Applies::Always,
        )];

        let entity = this_descriptor::CompiledEntity::new("billing", entity).unwrap();
        let registry = CustomRegistry::default();
        let ctx = ValidateCtx {
            registry: &registry,
            side: Side::Server,
        };
        // Fail-closed : un nom inconnu n'est jamais ignoré, même quand la valeur
        // est manifestement valide.
        assert!(apply(&entity, WriteOp::Create, json!({"amount": 1}), &ctx).is_err());
    }

    #[test]
    fn an_evaluation_error_is_a_violation_not_a_panic() {
        let r = rule("fn run(value: Int, d: Int) -> Bool { value / d > 0 }");
        let violation = r.apply("amount", &json!(4), &json!({"d": 0})).unwrap_err();
        assert_eq!(violation.code.as_str(), CODE_EVALUATION);
        assert!(violation.params.contains_key("error"));
    }
}
