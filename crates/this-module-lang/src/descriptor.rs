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

use crate::ast::{Function, Type};
use crate::check::PURE;
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
        // La surface d'une règle, et non celle d'un module : `emit` et `now`
        // n'y figurent pas, donc une règle qui les appelle ne type même pas.
        check::check_with(&function, PURE)?;

        if function.returns != Type::Bool {
            return Err(format!(
                "une règle rend Bool (true = valide), pas {}",
                function.returns.name()
            ));
        }

        Ok(Rule {
            name: name.into(),
            function,
        })
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

        match eval::run(&self.function, &args, &mut Pure) {
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
        // Une chaîne entre, mais ne se lie pas : elle va directement d'un champ à
        // une primitive pure. C'est un choix, pas un oubli — c'est ce qui permet
        // de traiter des chaînes sans allouer, donc sans modèle mémoire.
        Type::Str => raw
            .as_str()
            .map(|text| Value::Str(text.to_string()))
            .ok_or_else(mismatch),
        Type::Unit => Err(mismatch()),
    }
}

/// L'hôte d'une règle : les primitives pures de [`PURE`], et rien d'autre.
///
/// Le refus d'un appel hors surface a déjà eu lieu au typage ; ceci est la
/// seconde barrière, celle qui tient même si un jour une règle arrivait ici sans
/// être passée par le typeur.
struct Pure;

impl Host for Pure {
    fn call(&mut self, name: &str, args: &[Value]) -> Result<Value, String> {
        match (name, args) {
            // La longueur est celle des **caractères**, pas des octets : une règle
            // qui dit « au plus 40 » parle de ce qu'un humain voit, et « éé » vaut
            // deux, pas quatre.
            ("str_len", [Value::Str(text)]) => Ok(Value::Int(text.chars().count() as i64)),
            ("str_eq", [Value::Str(a), Value::Str(b)]) => Ok(Value::Bool(a == b)),
            ("str_starts_with", [Value::Str(text), Value::Str(prefix)]) => {
                Ok(Value::Bool(text.starts_with(prefix.as_str())))
            }
            ("str_ends_with", [Value::Str(text), Value::Str(suffix)]) => {
                Ok(Value::Bool(text.ends_with(suffix.as_str())))
            }
            ("str_contains", [Value::Str(text), Value::Str(needle)]) => {
                Ok(Value::Bool(text.contains(needle.as_str())))
            }
            _ => Err(format!("une règle ne peut pas appeler `{name}`")),
        }
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

    /// Une règle ne dispose pas de la surface d'un module : `emit` n'y figure pas,
    /// donc le typeur refuse. Il n'y a plus de balayage d'appels à maintenir, ni
    /// de discipline à espérer de l'auteur.
    #[test]
    fn a_rule_cannot_emit() {
        let error = Rule::compile(
            "billing.x",
            "fn run(value: Int) -> Bool { emit(\"a\"); value > 0 }",
        )
        .unwrap_err();
        assert!(error.contains("`emit`"), "{error}");
        assert!(error.contains("pas une primitive disponible"), "{error}");
    }

    /// Ni lire l'horloge : une règle doit rendre le même verdict sur le serveur et
    /// dans le front, et deux fois de suite.
    #[test]
    fn a_rule_cannot_read_the_clock() {
        let error =
            Rule::compile("billing.x", "fn run(value: Int) -> Bool { now() > 0 }").unwrap_err();
        assert!(error.contains("`now`"), "{error}");
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

    /// Le même chemin, sur un champ **chaîne** : c'est le cas réel (un numéro, un
    /// code, une adresse) et celui qui aurait été impossible sans surface pure.
    #[test]
    fn a_descriptor_applies_a_string_rule_through_the_real_engine() {
        use this_descriptor::CompiledEntity;
        use this_descriptor::ValidatorSpec;
        use this_descriptor::custom::Custom;
        use this_descriptor::engine::validate::{Side, ValidateCtx, WriteOp, apply};
        use this_descriptor::entity::EntityDesc;
        use this_descriptor::field::{Applies, FieldDesc, FieldType};

        let mut registry = CustomRegistry::default();
        register_rule(
            &mut registry,
            "billing.number_prefixed",
            "fn run(value: Str) -> Bool { str_starts_with(value, \"INV-\") && str_len(value) <= 12 }",
            CustomScope::Both,
        )
        .unwrap();

        let mut entity = EntityDesc::new("invoice");
        entity.fields = vec![FieldDesc::new("number", FieldType::String).validator(
            ValidatorSpec::Custom(Custom::named("billing.number_prefixed")),
            Applies::Always,
        )];
        let entity = CompiledEntity::new("billing", entity).unwrap();
        let ctx = ValidateCtx {
            registry: &registry,
            side: Side::Server,
        };

        assert!(
            apply(
                &entity,
                WriteOp::Create,
                json!({"number": "INV-2026"}),
                &ctx
            )
            .is_ok()
        );

        let error = apply(
            &entity,
            WriteOp::Create,
            json!({"number": "FAC-2026"}),
            &ctx,
        )
        .unwrap_err();
        let codes: Vec<String> = error
            .fields
            .iter()
            .map(|v| format!("{}:{}", v.path, v.code.as_str()))
            .collect();
        assert_eq!(codes, vec!["number:billing.number_prefixed"]);
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

    /// Une chaîne entre dans une règle — opaque, jamais liée, mais utilisable.
    /// C'est la forme la plus fréquente d'une vraie règle métier.
    #[test]
    fn a_string_prefix_rule_accepts_and_refuses() {
        let r = rule("fn run(value: Str) -> Bool { str_starts_with(value, \"INV-\") }");
        assert!(
            r.apply("number", &json!("INV-2026-001"), &json!({}))
                .is_ok()
        );
        assert!(
            r.apply("number", &json!("FAC-2026-001"), &json!({}))
                .is_err()
        );
    }

    #[test]
    fn a_length_rule_counts_characters_not_bytes() {
        let r = rule("fn run(value: Str, max: Int) -> Bool { str_len(value) <= max }");
        let params = json!({"max": 4});
        // Quatre caractères, six octets : c'est la longueur visible qui compte.
        assert!(r.apply("label", &json!("créé"), &params).is_ok());
        assert!(r.apply("label", &json!("créés"), &params).is_err());
    }

    #[test]
    fn the_string_predicates_cover_the_usual_moves() {
        let ends = rule("fn run(value: Str) -> Bool { str_ends_with(value, \"@this.rs\") }");
        assert!(ends.apply("email", &json!("a@this.rs"), &json!({})).is_ok());
        assert!(
            ends.apply("email", &json!("a@elsewhere.dev"), &json!({}))
                .is_err()
        );

        let contains = rule("fn run(value: Str) -> Bool { str_contains(value, \"::\") }");
        assert!(
            contains
                .apply("arn", &json!("arn:wami::x"), &json!({}))
                .is_ok()
        );

        let exact = rule("fn run(value: Str) -> Bool { str_eq(value, \"paid\") }");
        assert!(exact.apply("status", &json!("paid"), &json!({})).is_ok());
        assert!(exact.apply("status", &json!("unpaid"), &json!({})).is_err());
    }

    /// Une chaîne ne se *lie* toujours pas (pas de modèle mémoire) : seul le
    /// passage direct champ → primitive est ouvert.
    #[test]
    fn a_string_still_cannot_be_bound() {
        let error = Rule::compile(
            "billing.x",
            "fn run(value: Str) -> Bool { let s: Str = value; str_eq(s, s) }",
        )
        .unwrap_err();
        assert!(error.contains("ne se lie pas"), "{error}");
    }

    /// Un membre de `params` qui n'est ni un nombre ni un booléen n'est pas une
    /// entrée : il ne doit pas être converti à l'aveugle.
    #[test]
    fn a_non_scalar_param_falls_through_to_the_field_name() {
        let r = rule("fn run(value: Int, max: Int) -> Bool { value <= max }");
        // `max` est une chaîne dans les params : elle ne compte pas, et comme le
        // champ ne s'appelle pas `max`, l'entrée manque.
        let violation = r
            .apply("amount", &json!(1), &json!({"max": "10"}))
            .unwrap_err();
        assert_eq!(violation.code.as_str(), CODE_MISSING_INPUT);
    }

    /// La recherche d'appels doit descendre partout, pas seulement à la racine.
    #[test]
    fn a_call_nested_in_a_branch_is_still_refused() {
        let error = Rule::compile(
            "billing.x",
            "fn run(value: Int) -> Bool { if value > 0 { now() > 0 } else { false } }",
        )
        .unwrap_err();
        assert!(error.contains("now"), "{error}");
    }

    #[test]
    fn a_call_nested_in_a_binding_is_still_refused() {
        let error = Rule::compile(
            "billing.x",
            "fn run(value: Int) -> Bool { let t: Bool = now() > 0; t && value > 0 }",
        )
        .unwrap_err();
        assert!(error.contains("now"), "{error}");
    }

    #[test]
    fn an_evaluation_error_is_a_violation_not_a_panic() {
        let r = rule("fn run(value: Int, d: Int) -> Bool { value / d > 0 }");
        let violation = r.apply("amount", &json!(4), &json!({"d": 0})).unwrap_err();
        assert_eq!(violation.code.as_str(), CODE_EVALUATION);
        assert!(violation.params.contains_key("error"));
    }
}
