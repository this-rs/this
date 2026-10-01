//! weave_bench — latences du « tissage mathématique + boucle d'inférence »
//! sur le backend obrain de this-rs (store durable, WAL EveryCommit).
//!
//! Scénario calqué sur le tissage aletheia/DSH : nœuds `Problem`/`Lemma`/`Gate`
//! (ici un seul type `WNode` avec champ `kind`), liens typés nœud→problème,
//! lecture du « contexte d'un problème » (voisinage lemmes+portes) à chaque
//! round, écriture d'une mutation par round avec commit WAL.
//!
//! Usage :
//!   cargo run -p this --release --features obrain --example weave_bench
//!
//! Cibles (inférence) : lecture contexte < 10 ms ; écriture round (nœud+lien,
//! fsync) < 50 ms ; recherche par qualified_name < 10 ms à l'échelle du tissage.

use std::sync::Arc;
use std::time::{Duration, Instant};

use this::core::entity::{Data, Entity};
use this::core::field::FieldValue;
use this::core::link::LinkEntity;
use this::core::{DataService, LinkService};
use this::storage::{ObrainDataService, ObrainLinkService, ObrainStore};

use chrono::{DateTime, Utc};
use uuid::Uuid;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct WNode {
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
    /// id sémantique aletheia : mp_riemann, lm_*, gt_*
    pub qualified_name: String,
    /// "1" = certifié (garde A5 satisfaite), "0" sinon
    pub certified: String,
}

impl WNode {
    fn new(kind: &str, qualified_name: &str, certified: bool) -> Self {
        let now = Utc::now();
        Self {
            id: Uuid::new_v4(),
            entity_type: "wnode".into(),
            name: qualified_name.into(),
            created_at: now,
            updated_at: now,
            deleted_at: None,
            status: "active".into(),
            kind: kind.into(),
            qualified_name: qualified_name.into(),
            certified: if certified { "1" } else { "0" }.into(),
        }
    }
}

impl Entity for WNode {
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

impl Data for WNode {
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
            _ => None,
        }
    }
}

struct Stats {
    n: u32,
    total: Duration,
}

impl Stats {
    fn new() -> Self {
        Self {
            n: 0,
            total: Duration::ZERO,
        }
    }
    fn add(&mut self, d: Duration) {
        self.n += 1;
        self.total += d;
    }
    fn report(&self, label: &str) {
        let avg = self.total.as_secs_f64() * 1e6 / f64::from(self.n);
        println!(
            "  {:<44} {:>10.2} µs/op   ({:>7.1} ops/s, n={})",
            label,
            avg,
            1e6 / avg,
            self.n
        );
    }
}

async fn run_phase<F, Fut>(label: &str, warmup: u32, n: u32, mut f: F) -> Stats
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = ()>,
{
    for _ in 0..warmup {
        f().await;
    }
    let mut s = Stats::new();
    for _ in 0..n {
        let t = Instant::now();
        f().await;
        s.add(t.elapsed());
    }
    s.report(label);
    s
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    println!("weave_bench — this-rs obrain backend (durable, WAL)");
    println!("======================================================");

    let path = std::env::temp_dir().join("weave_bench.obrain");
    if path.exists() {
        std::fs::remove_dir_all(&path).ok();
    }
    println!("store : {}", path.display());
    let store = ObrainStore::open(&path)?;
    let data: ObrainDataService<WNode> = ObrainDataService::new(store.clone());
    let links = ObrainLinkService::new(store.clone());

    // ---- Seed : 21 problèmes × (8 lemmes + 4 portes) — ordre de grandeur du tissage
    const P: u32 = 21;
    const L: u32 = 8;
    const G: u32 = 4;
    let t = Instant::now();
    let mut problem_ids = Vec::with_capacity(P as usize);
    for p in 0..P {
        let prob = data
            .create(WNode::new("problem", &format!("mp_{p:02}"), false))
            .await?;
        problem_ids.push(prob.id);
        for l in 0..L {
            let lemma = data
                .create(WNode::new("lemma", &format!("lm_{p:02}_{l}"), l % 4 == 0))
                .await?;
            links
                .create(LinkEntity::new("lemma", lemma.id, prob.id, None))
                .await?;
        }
        for g in 0..G {
            let gate = data
                .create(WNode::new("gate", &format!("gt_{p:02}_{g}"), false))
                .await?;
            links
                .create(LinkEntity::new("gate", gate.id, prob.id, None))
                .await?;
        }
    }
    println!(
        "seed : {} problèmes × ({} lemmes + {} portes) = {} nœuds + {} liens en {:?}",
        P,
        L,
        G,
        P * (1 + L + G),
        P * (L + G),
        t.elapsed()
    );

    // ---- Phases (warmup puis mesure)
    let mut rng_p = 0usize;
    let prob_ids_cycle = problem_ids.clone();

    let _ = run_phase("B1 entity create (commit WAL)", 20, 300, || {
        let data = &data;
        let p = rng_p;
        rng_p = rng_p.wrapping_add(1);
        async move {
            let qn = format!("tmp_{}_{}", p, Uuid::new_v4().simple());
            let _ = data.create(WNode::new("lemma", &qn, false)).await;
        }
    })
    .await;

    let _ = run_phase("B2 link create (commit WAL)", 20, 300, || {
        let data = &data;
        let links = &links;
        let prob = prob_ids_cycle[0];
        async move {
            let lemma = data
                .create(WNode::new("lemma", "tmp_l", false))
                .await
                .unwrap();
            let _ = links
                .create(LinkEntity::new("lemma", lemma.id, prob, None))
                .await;
        }
    })
    .await;
    let mut view_p = 0usize;
    let _ = run_phase(
        "B3 contexte problème (get + voisinage lemmes/portes)",
        50,
        500,
        || {
            let data = &data;
            let links = &links;
            let id = prob_ids_cycle[view_p % problem_ids.len()];
            view_p += 1;
            async move {
                let _ = data.get(&id).await.unwrap();
                let _ = links
                    .find_by_target(&id, Some("lemma"), None)
                    .await
                    .unwrap();
                let _ = links.find_by_target(&id, Some("gate"), None).await.unwrap();
            }
        },
    )
    .await;

    let _ = run_phase("B4 search qualified_name (scan champs)", 20, 200, || {
        let data = &data;
        let q = "mp_07";
        async move {
            let _ = data.search("qualified_name", q).await.unwrap();
        }
    })
    .await;

    let mut round_p = 0usize;
    let _ = run_phase(
        "B5 round inference : contexte + nœud + lien (WAL)",
        20,
        200,
        || {
            let data = &data;
            let links = &links;
            let id = prob_ids_cycle[round_p % problem_ids.len()];
            round_p += 1;
            async move {
                let _ = data.get(&id).await.unwrap();
                let _ = links
                    .find_by_target(&id, Some("lemma"), None)
                    .await
                    .unwrap();
                let v = data
                    .create(WNode::new("lemma", "round_lemma", false))
                    .await
                    .unwrap();
                let _ = links.create(LinkEntity::new("lemma", v.id, id, None)).await;
            }
        },
    )
    .await;

    println!("\n-- contrôle : même scénario sur store en MÉMOIRE (sans fsync WAL) --");
    let mem = ObrainStore::new_in_memory();
    let data_m: ObrainDataService<WNode> = ObrainDataService::new(mem.clone());
    let links_m = ObrainLinkService::new(mem.clone());
    let t = Instant::now();
    let mut mem_problems = Vec::new();
    for p in 0..3u32 {
        let prob = data_m
            .create(WNode::new("problem", &format!("mp_m{p}"), false))
            .await?;
        mem_problems.push(prob.id);
        for l in 0..3u32 {
            let lemma = data_m
                .create(WNode::new("lemma", &format!("lm_m{p}_{l}"), l == 0))
                .await?;
            links_m
                .create(LinkEntity::new("lemma", lemma.id, prob.id, None))
                .await?;
        }
        for g in 0..2u32 {
            let gate = data_m
                .create(WNode::new("gate", &format!("gt_m{p}_{g}"), false))
                .await?;
            links_m
                .create(LinkEntity::new("gate", gate.id, prob.id, None))
                .await?;
        }
    }
    println!(
        "seed mémoire : 3×(1+3+2) nœuds + 3×5 liens en {:?}",
        t.elapsed()
    );

    let _ = run_phase("B1m entity create (mémoire)", 20, 200, || {
        let data = &data_m;
        async move {
            let qn = format!("tmp_m_{}", Uuid::new_v4().simple());
            let _ = data.create(WNode::new("lemma", &qn, false)).await;
        }
    })
    .await;

    let _ = run_phase("B2m link create (mémoire)", 20, 200, || {
        let data = &data_m;
        let links = &links_m;
        let prob = mem_problems[0];
        async move {
            let lemma = data
                .create(WNode::new("lemma", "tmp_lm", false))
                .await
                .unwrap();
            let _ = links
                .create(LinkEntity::new("lemma", lemma.id, prob, None))
                .await;
        }
    })
    .await;

    let _ = run_phase("B5m round inference (mémoire)", 20, 200, || {
        let data = &data_m;
        let links = &links_m;
        let id = mem_problems[0];
        async move {
            let _ = data.get(&id).await.unwrap();
            let _ = links
                .find_by_target(&id, Some("lemma"), None)
                .await
                .unwrap();
            let v = data
                .create(WNode::new("lemma", "round_lm", false))
                .await
                .unwrap();
            let _ = links.create(LinkEntity::new("lemma", v.id, id, None)).await;
        }
    })
    .await;

    println!("\nCibles inférence : B3 < 10 ms · B5 (batch) < 50 ms · B4 < 10 ms");
    Ok(())
}
