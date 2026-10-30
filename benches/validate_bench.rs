//! **Banc de la validation par descripteur** (RFC module-descriptor §7.1, flux validation),
//! sans dépendance de mesure.
//!
//! ```bash
//! cargo bench -p this-rs --bench validate_bench
//! ```
//!
//! # Ce qui est mesuré
//!
//! `engine::validate::apply` est désormais sur le chemin de TOUTE écriture d'une entité
//! décrite (REST par la couche, GraphQL / gRPC / liens par `ValidatingCreator`), et
//! `engine::query` sur toute liste filtrée. Un chemin chaud non mesuré est une opinion :
//!
//! 1. `apply` création valide (transformations + validateurs, 6 champs) ;
//! 2. `apply` création invalide (toutes les violations collectées) ;
//! 3. `apply` mise à jour partielle (un seul champ présent) ;
//! 4. `parse_legacy` + `check` d'une requête typée ;
//! 5. `apply_in_memory` (le défaut O(n) de `EntityFetcher::list_query`) sur 10 000 lignes.
//!
//! Harnais explicite (comme `scope_bench`) : échauffement, N tours, médiane, minimum ; le
//! nombre de tours est publié avec la mesure.

use std::hint::black_box;
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use this::descriptor::engine::query::{apply_in_memory, parse_legacy};
use this::descriptor::engine::validate::{Side, ValidateCtx, WriteOp, apply};
use this::descriptor::{
    Applies, CompiledEntity, CustomRegistry, EntityDesc, FieldDesc, FieldType, FilterOp,
    QuerySchema, TransformSpec, ValidatorSpec,
};

struct Measure {
    median: Duration,
    min: Duration,
    rounds: usize,
}

fn measure(rounds: usize, mut f: impl FnMut()) -> Measure {
    for _ in 0..rounds / 10 + 1 {
        f();
    }
    let mut samples: Vec<Duration> = (0..rounds)
        .map(|_| {
            let t = Instant::now();
            f();
            t.elapsed()
        })
        .collect();
    samples.sort();
    Measure {
        median: samples[rounds / 2],
        min: samples[0],
        rounds,
    }
}

fn report(name: &str, m: Measure) {
    println!(
        "{name:<44} median {:>10.2?}  min {:>10.2?}  ({} rounds)",
        m.median, m.min, m.rounds
    );
}

fn invoice() -> CompiledEntity {
    let mut e = EntityDesc::new("invoice");
    e.fields = vec![
        FieldDesc::new("number", FieldType::String)
            .transform(TransformSpec::Trim, Applies::Always)
            .transform(TransformSpec::Uppercase, Applies::Always)
            .validator(
                ValidatorSpec::StringLength { min: 3, max: 50 },
                Applies::Always,
            )
            .filterable(&[FilterOp::Eq])
            .sortable(),
        FieldDesc::new("amount", FieldType::F64)
            .validator(ValidatorSpec::Positive, Applies::Always)
            .filterable(&[FilterOp::Gt, FilterOp::Lt])
            .sortable(),
        FieldDesc::new("currency", FieldType::String).validator(
            ValidatorSpec::InList(vec!["EUR".into(), "USD".into()]),
            Applies::Always,
        ),
        FieldDesc::new("customer", FieldType::String)
            .transform(TransformSpec::Trim, Applies::Always)
            .validator(
                ValidatorSpec::StringLength { min: 1, max: 200 },
                Applies::Always,
            ),
        FieldDesc::new("note", FieldType::String).optional(),
        FieldDesc::new("lines", FieldType::I64).validator(ValidatorSpec::Positive, Applies::Always),
    ];
    CompiledEntity::new("billing", e).expect("valid entity")
}

fn main() {
    let entity = invoice();
    let registry = CustomRegistry::default();
    let ctx = ValidateCtx {
        registry: &registry,
        side: Side::Server,
    };
    let valid = json!({"number": " inv-0042 ", "amount": 120.5, "currency": "EUR",
                       "customer": "  ACME  ", "note": "x", "lines": 3});
    let invalid = json!({"number": "x", "amount": -1, "currency": "GBP",
                         "customer": "", "lines": 0, "ghost": true});

    report(
        "validate create (valid, 6 fields)",
        measure(20_000, || {
            black_box(apply(&entity, WriteOp::Create, valid.clone(), &ctx)).ok();
        }),
    );
    report(
        "validate create (invalid, 6 violations)",
        measure(20_000, || {
            black_box(apply(&entity, WriteOp::Create, invalid.clone(), &ctx)).ok();
        }),
    );
    report(
        "validate update (1 field present)",
        measure(20_000, || {
            black_box(apply(&entity, WriteOp::Update, json!({"amount": 3}), &ctx)).ok();
        }),
    );

    let schema = QuerySchema::for_entity(&entity.desc);
    report(
        "parse_legacy + check (filter + sort)",
        measure(20_000, || {
            black_box(parse_legacy(
                &schema,
                Some(r#"{"amount>": 10, "amount<": 1000, "number": "INV-1"}"#),
                Some("-amount,number"),
                1,
                50,
            ))
            .ok();
        }),
    );

    let rows: Vec<Value> = (0..10_000)
        .map(|i| json!({"number": format!("INV-{i}"), "amount": (i * 37 % 1000) as f64}))
        .collect();
    let q = parse_legacy(&schema, Some(r#"{"amount>": 500}"#), Some("-amount"), 1, 50)
        .expect("valid query");
    report(
        "apply_in_memory (10k rows, filter + sort)",
        measure(50, || {
            black_box(apply_in_memory(&q, rows.clone()));
        }),
    );
}
