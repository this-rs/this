//! Démonstration exécutable : des règles métier écrites dans le langage,
//! appliquées par le **vrai** moteur de validation de `this-descriptor`.
//!
//!     cargo run -p this-module-lang --example rules --features descriptor
//!
//! Rien n'est simulé ici : `engine::validate::apply` est le même chemin que
//! celui qu'emprunte une requête REST ou gRPC sur un module stocké.

use serde_json::json;
use this_descriptor::custom::{Custom, CustomRegistry, CustomScope};
use this_descriptor::engine::validate::{Side, ValidateCtx, WriteOp, apply};
use this_descriptor::entity::EntityDesc;
use this_descriptor::field::{Applies, FieldDesc, FieldType};
use this_descriptor::{CompiledEntity, ValidatorSpec};
use this_module_lang::register_rule;

fn titre(texte: &str) {
    println!("\n\x1b[1m{texte}\x1b[0m");
    println!("{}", "─".repeat(texte.chars().count()));
}

fn main() {
    titre("1. Trois règles, écrites dans le langage du module");

    let regles = [
        (
            "billing.number_prefixed",
            r#"fn run(value: Str) -> Bool { str_starts_with(value, "INV-") && str_len(value) <= 12 }"#,
        ),
        (
            "billing.amount_in_range",
            "fn run(value: Int, max: Int) -> Bool { value > 0 && value <= max }",
        ),
        (
            "billing.email_domain",
            r#"fn run(value: Str) -> Bool { str_ends_with(value, "@this.rs") }"#,
        ),
    ];

    for (nom, source) in regles {
        println!("\n  \x1b[36m{nom}\x1b[0m");
        println!("    {source}");
    }

    // ── L'enregistrement ────────────────────────────────────────────────────
    let mut registry = CustomRegistry::default();
    for (nom, source) in regles {
        register_rule(&mut registry, nom, source, CustomScope::Both)
            .expect("une règle correcte doit s'enregistrer");
    }

    titre("2. Le descripteur ne déclare que des NOMS");

    let mut facture = EntityDesc::new("invoice");
    facture.fields = vec![
        FieldDesc::new("number", FieldType::String).validator(
            ValidatorSpec::Custom(Custom::named("billing.number_prefixed")),
            Applies::Always,
        ),
        FieldDesc::new("amount", FieldType::U64).validator(
            ValidatorSpec::Custom(Custom {
                name: "billing.amount_in_range".into(),
                params: json!({ "max": 1000 }),
            }),
            Applies::Always,
        ),
        FieldDesc::new("email", FieldType::String)
            .optional()
            .validator(
                ValidatorSpec::Custom(Custom::named("billing.email_domain")),
                Applies::Always,
            ),
    ];
    let facture = CompiledEntity::new("billing", facture).expect("le descripteur est valide");

    println!(
        r#"
  number : string, custom {{ name: billing.number_prefixed }}
  amount : u64,    custom {{ name: billing.amount_in_range, params: {{ max: 1000 }} }}
  email  : string, custom {{ name: billing.email_domain }}   (optionnel)

  La règle vit DANS le module. Elle n'est pas dans du Rust que le descripteur
  ne peut pas montrer."#
    );

    // ── L'application ───────────────────────────────────────────────────────
    titre("3. Le moteur applique — même chemin qu'une vraie requête");

    let ctx = ValidateCtx {
        registry: &registry,
        side: Side::Server,
    };

    let essais = [
        (
            "tout est bon",
            json!({"number": "INV-2026-001", "amount": 500, "email": "a@this.rs"}),
        ),
        (
            "mauvais préfixe",
            json!({"number": "FAC-2026-001", "amount": 500}),
        ),
        (
            "numéro trop long",
            json!({"number": "INV-2026-000123", "amount": 500}),
        ),
        (
            "montant hors borne",
            json!({"number": "INV-2026-002", "amount": 5000}),
        ),
        (
            "mauvais domaine",
            json!({"number": "INV-2026-003", "amount": 10, "email": "a@elsewhere.dev"}),
        ),
    ];

    for (etiquette, charge) in essais {
        match apply(&facture, WriteOp::Create, charge, &ctx) {
            Ok(_) => println!("  \x1b[32m✔\x1b[0m {etiquette:<20}"),
            Err(erreur) => {
                let codes: Vec<String> = erreur
                    .fields
                    .iter()
                    .map(|v| format!("{} → {}", v.path, v.code.as_str()))
                    .collect();
                println!(
                    "  \x1b[31m✘\x1b[0m {etiquette:<20} \x1b[2m{}\x1b[0m",
                    codes.join(", ")
                );
            }
        }
    }

    // ── Le refus, tôt ───────────────────────────────────────────────────────
    titre("4. Ce qui est refusé, et à quel moment");

    let mut neuf = CustomRegistry::default();

    match register_rule(
        &mut neuf,
        "billing.effet",
        r#"fn run(value: Int) -> Bool { emit("coucou"); value > 0 }"#,
        CustomScope::Both,
    ) {
        Ok(()) => println!("  (inattendu : la règle a été acceptée)"),
        Err(erreur) => println!(
            "  règle qui écrit        → refusée à l'enregistrement :\n    \x1b[2m{erreur}\x1b[0m"
        ),
    }

    match register_rule(
        &mut neuf,
        "billing.horloge",
        "fn run(value: Int) -> Bool { now() > 0 }",
        CustomScope::Both,
    ) {
        Ok(()) => println!("  (inattendu : la règle a été acceptée)"),
        Err(erreur) => println!(
            "  règle qui lit l'heure  → refusée à l'enregistrement :\n    \x1b[2m{erreur}\x1b[0m"
        ),
    }

    match register_rule(
        &mut neuf,
        "billing.casse",
        "fn run(value: Int) -> Bool { value > }",
        CustomScope::Both,
    ) {
        Ok(()) => println!("  (inattendu : la règle a été acceptée)"),
        Err(erreur) => println!(
            "  règle mal formée       → refusée à l'enregistrement :\n    \x1b[2m{erreur}\x1b[0m"
        ),
    }

    let mut orphelin = EntityDesc::new("invoice");
    orphelin.fields = vec![FieldDesc::new("x", FieldType::U64).validator(
        ValidatorSpec::Custom(Custom::named("billing.jamais_enregistree")),
        Applies::Always,
    )];
    let orphelin = CompiledEntity::new("billing", orphelin).unwrap();
    let ctx_vide = ValidateCtx {
        registry: &CustomRegistry::default(),
        side: Side::Server,
    };
    match apply(&orphelin, WriteOp::Create, json!({"x": 1}), &ctx_vide) {
        Ok(_) => println!("  (inattendu : le nom inconnu a été ignoré)"),
        Err(_) => println!(
            "  nom jamais enregistré  → la validation refuse (fail-closed), même sur une valeur valide"
        ),
    }

    // ── L'autre moitié : la même sémantique compilée en wasm ────────────────
    titre("5. La même sémantique, compilée en WebAssembly");

    let module = this_module_lang::compile(
        "fn run(value: Int, max: Int) -> Bool { value > 0 && value <= max }",
    )
    .expect("le programme compile");
    println!("\n  programme  : fn run(value: Int, max: Int) -> Bool");
    println!("  imports    : {:?}", module.wat.imports);
    println!("  module wasm (extrait) :");
    for ligne in module.wat.text.lines().take(12) {
        println!("    {ligne}");
    }
    println!("\n  L'interpréteur et le module émis sont comparés par tests/parity.rs,");
    println!("  valeurs ET effets observés. Un module futé ne peut pas diverger.");
}
