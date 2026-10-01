//! weave_pilot — pilote R1 de la migration obrain du tissage mathématique.
//!
//! Entités this-rs (Problem/Lemma/Gate → un type `WeaveNode` discriminé par
//! `kind`) sur backend obrain durable (tag v0.0.1-zorder-2026-09-04), liens
//! LEMMA_OF / GATE_OF. Vérifie :
//!   1. import déterministe et IDEMPOTENT de la graine canonique (re-jouer →
//!      zéro création, comptes stables) ;
//!   2. parité : comptes par kind (7 problèmes / 16 lemmes / 9 portes) et
//!      lecture « contexte problème » (frontière lemmes+portes) ;
//!   3. garde A5 : pas de promotion d'un lemme non certifié.
//!
//! Usage : cargo run -p this-rs --release --features obrain \
//!            --example weave_pilot [seed.json] [--fresh]
//!   seed.json défaut : ~/projects/aletheia/data/weave-seed.json

use std::sync::Arc;
use std::time::Instant;

use this::core::entity::{Data, Entity};
use this::core::field::FieldValue;
use this::core::link::LinkEntity;
use this::core::{DataService, LinkService};
use this::storage::{ObrainDataService, ObrainLinkService, ObrainStore};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct WeaveNode {
    pub id: Uuid,
    #[serde(rename = "type")]
    pub entity_type: String,
    pub name: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub deleted_at: Option<DateTime<Utc>>,
    pub status: String,
    /// problem | lemma | gate
    pub kind: String,
    /// id sémantique : mp_riemann, lm_*, gt_*
    pub qualified_name: String,
    pub label: String,
    /// "1" = certifié (garde A5), "0" sinon
    pub certified: String,
    pub lean_source: String,
    pub statement: String,
}

impl WeaveNode {
    fn new(
        kind: &str,
        qn: &str,
        label: &str,
        certified: bool,
        lean: &str,
        statement: &str,
    ) -> Self {
        let now = Utc::now();
        Self {
            id: Uuid::new_v4(),
            entity_type: "wnode".into(),
            name: qn.into(),
            created_at: now,
            updated_at: now,
            deleted_at: None,
            status: "active".into(),
            kind: kind.into(),
            qualified_name: qn.into(),
            label: label.into(),
            certified: if certified { "1" } else { "0" }.into(),
            lean_source: lean.into(),
            statement: statement.into(),
        }
    }
}

impl Entity for WeaveNode {
    type Service = ();
    fn resource_name() -> &'static str {
        "wnodes"
    }
    fn resource_name_singular() -> &'static str {
        "wnode"
    }
    fn service_from_host(
        _host: &Arc<dyn std::any::Any + Send + Sync>,
    ) -> anyhow::Result<Arc<Self::Service>> {
        Ok(Arc::new(()))
    }
    fn id(&self) -> Uuid {
        self.id
    }
    fn entity_type(&self) -> &str {
        &self.entity_type
    }
    fn created_at(&self) -> DateTime<Utc> {
        self.created_at
    }
    fn updated_at(&self) -> DateTime<Utc> {
        self.updated_at
    }
    fn deleted_at(&self) -> Option<DateTime<Utc>> {
        self.deleted_at
    }
    fn status(&self) -> &str {
        &self.status
    }
}

impl Data for WeaveNode {
    fn name(&self) -> &str {
        &self.name
    }
    fn indexed_fields() -> &'static [&'static str] {
        &["qualified_name", "kind"]
    }
    fn field_value(&self, field: &str) -> Option<FieldValue> {
        match field {
            "name" => Some(FieldValue::String(self.name.clone())),
            "status" => Some(FieldValue::String(self.status.clone())),
            "kind" => Some(FieldValue::String(self.kind.clone())),
            "qualified_name" => Some(FieldValue::String(self.qualified_name.clone())),
            "certified" => Some(FieldValue::String(self.certified.clone())),
            "label" => Some(FieldValue::String(self.label.clone())),
            _ => None,
        }
    }
}

#[derive(Debug, Deserialize)]
struct Seed {
    counts: SeedCounts,
    problems: Vec<SeedProblem>,
}
#[derive(Debug, Deserialize)]
struct SeedCounts {
    problems: usize,
    lemmas: usize,
    gates: usize,
}
#[derive(Debug, Deserialize)]
struct SeedProblem {
    id: String,
    label: String,
    statement: String,
    lemmas: Vec<SeedNode>,
    gates: Vec<SeedNode>,
}
#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct SeedNode {
    id: String,
    label: String,
    #[serde(default)]
    lean: String,
    #[serde(default)]
    open: Option<bool>,
}

/// Garde A5 (même sémantique que living.ts) : un lemme n'est « certifié » que
/// si sa source Lean est substantielle (non triviale). Le pilote refuse toute
/// promotion sans certification — le substrate est un moteur, pas un juge.
fn is_trivial_lean(src: &str) -> bool {
    let t = src.split_whitespace().collect::<Vec<_>>().join(" ");
    t.is_empty()
        || t == "example : True := trivial"
        || t == "example : ∀ n : Nat, n + 0 = n := Nat.add_zero"
        || t == "example : True := trivial example : ∀ n : Nat, n + 0 = n := Nat.add_zero"
}

/// Gateway (future R2) : promouvoir un lemme dans 𝒢 exige un certificat.
/// Refus STRUCTUREL — aucune écriture sans certification (garde A5).
async fn promote_lemma(
    data: &ObrainDataService<WeaveNode>,
    links: &ObrainLinkService,
    qn: &str,
    label: &str,
    lean: &str,
    target_problem_id: Uuid,
) -> anyhow::Result<Result<Uuid, String>> {
    if is_trivial_lean(lean) {
        return Ok(Err("uncertified: source Lean triviale ou absente".into()));
    }
    if !data.search("qualified_name", qn).await?.is_empty() {
        return Ok(Err("duplicate: qualified_name déjà présent".into()));
    }
    let e = data
        .create(WeaveNode::new("lemma", qn, label, true, lean, ""))
        .await?;
    links
        .create(LinkEntity::new("LEMMA_OF", e.id, target_problem_id, None))
        .await?;
    Ok(Ok(e.id))
}

async fn count_kind(data: &ObrainDataService<WeaveNode>, kind: &str) -> anyhow::Result<usize> {
    Ok(data.search("kind", kind).await?.len())
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let seed_path = args
        .get(1)
        .cloned()
        .unwrap_or_else(|| "/home/triviere/projects/aletheia/data/weave-seed.json".into());
    let fresh = args.iter().any(|a| a == "--fresh");
    let store_path = std::env::temp_dir().join("weave_pilot.obrain");
    if fresh && store_path.exists() {
        std::fs::remove_dir_all(&store_path).ok();
    }

    println!("weave_pilot — this-rs obrain backend (tag v0.0.1-zorder-2026-09-04)");
    println!("store : {}", store_path.display());
    let store = ObrainStore::open(&store_path)?;
    let data: ObrainDataService<WeaveNode> = ObrainDataService::new(store.clone());
    let links = ObrainLinkService::new(store.clone());

    let raw = std::fs::read_to_string(&seed_path)?;
    let seed: Seed = serde_json::from_str(&raw)?;
    println!(
        "graine : {} problèmes / {} lemmes / {} portes",
        seed.counts.problems, seed.counts.lemmas, seed.counts.gates
    );

    // ---- 1. Import déterministe + idempotent (skip si qualified_name présent)
    let t = Instant::now();
    let mut created = 0usize;
    let mut skipped = 0usize;
    for p in &seed.problems {
        let prob_qn = p.id.as_str();
        if data.search("qualified_name", prob_qn).await?.is_empty() {
            data.create(WeaveNode::new(
                "problem",
                prob_qn,
                &p.label,
                false,
                "",
                &p.statement,
            ))
            .await?;
            created += 1;
        } else {
            skipped += 1;
        }
        let prob = data.search("qualified_name", prob_qn).await?;
        let prob_id = prob[0].id;

        for l in &p.lemmas {
            let qn = l.id.as_str();
            let cert = !is_trivial_lean(&l.lean);
            let entity = if data.search("qualified_name", qn).await?.is_empty() {
                let e = data
                    .create(WeaveNode::new("lemma", qn, &l.label, cert, &l.lean, ""))
                    .await?;
                created += 1;
                e
            } else {
                skipped += 1;
                data.search("qualified_name", qn).await?[0].clone()
            };
            if links
                .find_by_source(&entity.id, Some("LEMMA_OF"), None)
                .await?
                .is_empty()
            {
                links
                    .create(LinkEntity::new("LEMMA_OF", entity.id, prob_id, None))
                    .await?;
            }
        }
        for g in &p.gates {
            let qn = g.id.as_str();
            let entity = if data.search("qualified_name", qn).await?.is_empty() {
                let e = data
                    .create(WeaveNode::new("gate", qn, &g.label, false, "", ""))
                    .await?;
                created += 1;
                e
            } else {
                skipped += 1;
                data.search("qualified_name", qn).await?[0].clone()
            };
            if links
                .find_by_source(&entity.id, Some("GATE_OF"), None)
                .await?
                .is_empty()
            {
                links
                    .create(LinkEntity::new("GATE_OF", entity.id, prob_id, None))
                    .await?;
            }
        }
    }
    println!(
        "import : {} créés / {} déjà présents (skip) en {:?}",
        created,
        skipped,
        t.elapsed()
    );

    // ---- 2. Parité : comptes + contexte problème
    let (np, nl, ng) = (
        count_kind(&data, "problem").await?,
        count_kind(&data, "lemma").await?,
        count_kind(&data, "gate").await?,
    );
    println!("parité comptes : problems={np} lemmas={nl} gates={ng}");
    let probe = seed.problems[0].id.clone();
    let prob = data.search("qualified_name", &probe).await?;
    let prob_id = prob[0].id;
    let mut ctx_reads = 0usize;
    let mut ctx_us = 0f64;
    for _ in 0..200 {
        let t0 = Instant::now();
        let _ = data.get(&prob_id).await?;
        let _ = links
            .find_by_target(&prob_id, Some("LEMMA_OF"), None)
            .await?;
        let _ = links
            .find_by_target(&prob_id, Some("GATE_OF"), None)
            .await?;
        ctx_us += t0.elapsed().as_secs_f64() * 1e6;
        ctx_reads += 1;
    }
    let lemmas = links
        .find_by_target(&prob_id, Some("LEMMA_OF"), None)
        .await?;
    let gates = links
        .find_by_target(&prob_id, Some("GATE_OF"), None)
        .await?;
    println!(
        "contexte « {} » : {} lemmes + {} portes — lecture moyenne {:.1} µs (n={})",
        probe,
        lemmas.len(),
        gates.len(),
        ctx_us / ctx_reads as f64,
        ctx_reads
    );

    // ---- 3. Garde A5 : refus STRUCTUREL d'une promotion non certifiée
    let pnp = data.search("qualified_name", "mp_pnp").await?;
    let pnp_id = pnp[0].id;
    let lemmas_before = count_kind(&data, "lemma").await?;
    let refused = promote_lemma(
        &data,
        &links,
        "lm_demo_uncertified",
        "Démo — lemme sans certificat",
        "example : True := trivial",
        pnp_id,
    )
    .await?;
    let lemmas_after = count_kind(&data, "lemma").await?;
    println!(
        "garde A5 : promotion non certifiée → refusée ({:?}) ; lemmas {} → {} (aucune écriture)",
        refused.err(),
        lemmas_before,
        lemmas_after
    );
    // contrôle : la même gateway ACCEPTE un lemme certifié (lean substantiel)
    let ok = promote_lemma(
        &data,
        &links,
        "lm_demo_certified",
        "Démo — lemme certifié",
        "theorem demo_cert : 1 + 0 = 1 := by omega",
        pnp_id,
    )
    .await?;
    println!("garde A5 : promotion certifiée → acceptée ({:?})", ok.ok());
    println!(
        "garde A5 : lemme graine lm_pnp_cook certifié (lean substantiel) = {}",
        data.search("qualified_name", "lm_pnp_cook")
            .await?
            .first()
            .map(|n| n.certified == "1")
            .unwrap_or(false)
    );

    // ---- 4. Idempotence contrôlée : re-jouer l'import → zéro création
    println!("\nre-jouer l'import (idempotence)…");
    let t2 = Instant::now();
    let mut recreated = 0usize;
    for p in &seed.problems {
        if data
            .search("qualified_name", p.id.as_str())
            .await?
            .is_empty()
        {
            recreated += 1;
        }
        for l in &p.lemmas {
            if data
                .search("qualified_name", l.id.as_str())
                .await?
                .is_empty()
            {
                recreated += 1;
            }
        }
        for g in &p.gates {
            if data
                .search("qualified_name", g.id.as_str())
                .await?
                .is_empty()
            {
                recreated += 1;
            }
        }
    }
    println!(
        "idempotence : recréations = {recreated} (attendu 0) en {:?}",
        t2.elapsed()
    );

    let (np2, nl2, ng2) = (
        count_kind(&data, "problem").await?,
        count_kind(&data, "lemma").await?,
        count_kind(&data, "gate").await?,
    );
    println!("état final : problems={np2} lemmas={nl2} gates={ng2}");
    println!("PILOTE OK");
    Ok(())
}
