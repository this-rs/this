#![cfg(all(feature = "obrain", feature = "objects-toolbox"))]
//! **L4 natif** — la stratégie relationnelle passée par le CYCLE RÉEL du routeur.
//!
//! Le banc `l4_transfert_relationnel.rs` appelle `arms`/`settle` directement : c'est un
//! laboratoire isolé. Ici, la décision est prise par `TurnRouter`
//! (`route` → `record` → `settle`), et l'attribution est celle du routeur : c'est le
//! `Stop` du tour RÉEL qui nourrit l'apprentissage.
//!
//! ## Ce que le routeur natif impose, et qui change l'expérience
//!
//! La « classe » d'un tour n'est pas libre : c'est `TurnSignature::arm_key()`, donc
//! `TaskClass::key()` — pour un tour de chat, l'une des **cinq intentions** du
//! détecteur à mots-clés (`chat.debug`, `chat.explore`, `chat.impact`, `chat.plan`,
//! `chat.general`). Il n'y a donc pas de « classes inédites » comme dans le banc :
//! il y a quatre intentions, deux servent à l'entraînement, deux restent aveugles.
//!
//! Le partage par domaine n'est pas connu du routeur : c'est une relation **déclarée**
//! dans le graphe (`(:classe)-[:APPARTIENT]->(:domaine)`), exactement comme en L4a.
//!
//! ## Ce qui est mesuré
//!
//! 1. La réussite sur les deux intentions AVEUGLES, avec et sans partage de domaine.
//! 2. Le **coût réel par appel** : `arms`, `record`, `settle`, requêtes GQL par tour,
//!    et croissance du coût avec le volume d'observations.
//! 3. L'attribution : le nombre de `settle` RÉELLEMENT reçus par l'apprentissage,
//!    compté dans l'apprentissage, pas dans le conducteur.

use std::cell::Cell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use obrain_common::types::Value;
use this::storage::obrain::ObrainStore;
use this_agent::Stop;
use this_harness::routing::{
    ArmKey, ArmStats, Capabilities, CostBasis, Decision, Learning, MemoryPool, Pick, RouterSpec,
    RoutingSettings, SequenceSeeds, TurnInput, TurnRouter,
};

fn executer<F: std::future::Future<Output = T>, T>(f: F) -> T {
    futures::executor::block_on(f)
}

fn echapper(s: &str) -> String {
    s.replace('\\', "\\\\").replace('\'', "\\'")
}

fn candidat(p: &str, m: &str) -> this_harness::routing::Candidate {
    this_harness::routing::Candidate {
        cost_basis: CostBasis::Subscription,
        healthy: Some(true),
        ..this_harness::routing::Candidate::new(
            p,
            m,
            Capabilities {
                tools: true,
                images: false,
                reasoning: false,
                context_window: Some(200_000),
            },
        )
    }
}

fn parametres() -> RoutingSettings {
    RoutingSettings {
        exploration_epsilon: 0.1,
        ..Default::default()
    }
}

const PROVIDERS: [&str; 4] = ["a", "b", "c", "d"];
const MODELES: [&str; 4] = ["m1", "m2", "m3", "m4"];

/// Textes qui déclenchent chaque intention (détecteur à mots-clés du harness).
const TEXTES: [(&str, &str); 4] = [
    ("chat.debug", "pourquoi ça échoue avec une erreur ?"),
    ("chat.explore", "explique comment fonctionne le routage"),
    ("chat.impact", "change le code existant et relis-le"),
    (
        "chat.plan",
        "implémente une nouvelle fonctionnalité et planifie-la",
    ),
];

/// Deux domaines DÉCLARÉS : l'un où `a` domine, l'autre où `b` domine.
const DOMAINES: [(&str, [&str; 2]); 2] = [
    ("d0", ["chat.debug", "chat.explore"]),
    ("d1", ["chat.impact", "chat.plan"]),
];

const ENTRAINEMENT: [&str; 2] = ["chat.debug", "chat.impact"];
const AVEUGLES: [&str; 2] = ["chat.explore", "chat.plan"];

const TOURS_ENTRAINEMENT: usize = 60;
const TOURS_AVEUGLE: usize = 40;

fn domaine_de(classe: &str) -> &'static str {
    for (d, classes) in DOMAINES {
        if classes.contains(&classe) {
            return d;
        }
    }
    "inconnu"
}

/// Probabilité de réussite latente. Aucun apprenant n'y a accès.
fn theta(classe: &str, provider: &str) -> f64 {
    let (base, boost): (f64, f64) = match provider {
        "a" => (
            0.80,
            if domaine_de(classe) == "d0" {
                0.0
            } else {
                -0.35
            },
        ),
        "b" => (
            0.45,
            if domaine_de(classe) == "d1" {
                0.35
            } else {
                0.0
            },
        ),
        "c" => (0.35, 0.0),
        _ => (0.20, 0.0),
    };
    (base + boost).clamp(0.02, 0.98)
}

fn fnv1a(s: &str) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in s.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    h
}

fn lcg(etat: &mut u64) -> u64 {
    *etat = etat
        .wrapping_mul(6364136223846793005)
        .wrapping_add(1442695040888963407);
    *etat
}

/// Tirage pré-généré, indépendant de la stratégie : mêmes séquences pour les deux
/// apprenants, pour que la comparaison porte sur le partage et rien d'autre.
fn tirage(classe: &str, provider: &str, j: usize, graine: u64) -> bool {
    let mut etat = fnv1a(&format!("{graine}/{classe}/{provider}"));
    for _ in 0..=j {
        lcg(&mut etat);
    }
    (etat % 10_000) as f64 / 10_000.0 < theta(classe, provider)
}

// ── Coûts réellement mesurés ───────────────────────────────────────────────

#[derive(Default)]
struct Couts {
    requetes: AtomicUsize,
    settle_recus: AtomicUsize,
    observations_ecrites: AtomicUsize,
    appels_arms: AtomicUsize,
    duree_arms: Cell<Duration>,
    duree_record: Cell<Duration>,
    duree_settle: Cell<Duration>,
}

impl Couts {
    fn ajouter(&self, champ: &Cell<Duration>, d: Duration) {
        champ.set(champ.get() + d);
    }
    fn bilan(&self, tours: usize) -> String {
        let n_arms = self.appels_arms.load(Ordering::Relaxed).max(1);
        let n_settle = self.settle_recus.load(Ordering::Relaxed).max(1);
        format!(
            "arms {:.1} µs · record {:.1} µs · settle {:.1} µs (par appel) · {:.2} requête(s)/tour · {} observations écrites / {tours} tours",
            self.duree_arms.get().as_secs_f64() * 1e6 / n_arms as f64,
            self.duree_record.get().as_secs_f64() * 1e6 / n_settle as f64,
            self.duree_settle.get().as_secs_f64() * 1e6 / n_settle as f64,
            self.requetes.load(Ordering::Relaxed) as f64 / tours.max(1) as f64,
            self.observations_ecrites.load(Ordering::Relaxed)
        )
    }
}

// ── Lectures strictes (une erreur n'est jamais un résultat vide) ───────────

fn lire_observations(store: &Arc<ObrainStore>, couts: &Couts) -> Vec<(String, String, bool)> {
    couts.requetes.fetch_add(1, Ordering::Relaxed);
    let r = store
        .execute_query(
            "gql",
            "MATCH (o:observation) RETURN o.classe AS c, o.provider AS p, o.favorable AS f",
            100_000,
        )
        .expect("lecture des observations");
    let colonnes: Vec<String> = r
        .get("columns")
        .and_then(|c| c.as_array())
        .expect("réponse sans 'columns'")
        .iter()
        .map(|v| v.as_str().expect("colonne non textuelle").to_string())
        .collect();
    let idx = |nom: &str| {
        colonnes
            .iter()
            .position(|c| c == nom)
            .unwrap_or_else(|| panic!("colonne '{nom}' absente : {colonnes:?}"))
    };
    let (ic, ip, iff) = (idx("c"), idx("p"), idx("f"));
    let rows = r
        .get("rows")
        .and_then(|v| v.as_array())
        .expect("réponse sans 'rows'");
    assert!(
        rows.len() < 100_000,
        "lecture possiblement TRONQUÉE : {} lignes",
        rows.len()
    );
    rows.iter()
        .map(|l| {
            (
                l.get(ic)
                    .and_then(|v| v.as_str())
                    .expect("colonne 'c' non textuelle")
                    .to_string(),
                l.get(ip)
                    .and_then(|v| v.as_str())
                    .expect("colonne 'p' non textuelle")
                    .to_string(),
                l.get(iff)
                    .and_then(|v| v.as_bool())
                    .expect("colonne 'f' non booléenne"),
            )
        })
        .collect()
}

/// Classes du domaine de `classe`, lues par les relations du graphe.
fn soeurs(store: &Arc<ObrainStore>, couts: &Couts, classe: &str) -> Vec<String> {
    couts.requetes.fetch_add(1, Ordering::Relaxed);
    let r = store
        .execute_query(
            "gql",
            &format!(
                "MATCH (c:classe)-[:APPARTIENT]->(d:domaine) WHERE c.nom = '{}' RETURN d.nom AS nom",
                echapper(classe)
            ),
            10,
        )
        .expect("domaine du graphe");
    let rows = r
        .get("rows")
        .and_then(|v| v.as_array())
        .expect("réponse sans 'rows' pour le domaine");
    let domaine = rows
        .first()
        .and_then(|l| l.get(0))
        .and_then(|v| v.as_str())
        .expect("domaine introuvable")
        .to_string();
    couts.requetes.fetch_add(1, Ordering::Relaxed);
    let r = store
        .execute_query(
            "gql",
            &format!(
                "MATCH (s:classe)-[:APPARTIENT]->(d:domaine) WHERE d.nom = '{}' RETURN s.nom AS nom",
                echapper(&domaine)
            ),
            1000,
        )
        .expect("classes du domaine");
    r.get("rows")
        .and_then(|v| v.as_array())
        .expect("réponse sans 'rows' pour les sœurs")
        .iter()
        .map(|l| {
            l.get(0)
                .and_then(|v| v.as_str())
                .expect("nom de classe non textuel")
                .to_string()
        })
        .collect()
}

fn ecrire(
    store: &Arc<ObrainStore>,
    couts: &Couts,
    id: u128,
    classe: &str,
    p: &str,
    m: &str,
    ok: bool,
) {
    couts.requetes.fetch_add(1, Ordering::Relaxed);
    let mut s = store.db().session();
    s.begin_transaction().expect("begin");
    let n = s.create_node(&["observation"]);
    s.set_node_property(n, "evenement", Value::String(id.to_string().into()));
    s.set_node_property(n, "classe", Value::String(classe.into()));
    s.set_node_property(n, "provider", Value::String(p.into()));
    s.set_node_property(n, "modele", Value::String(m.into()));
    s.set_node_property(n, "favorable", Value::Bool(ok));
    s.commit().expect("commit");
}

/// Construit le graphe de structure : classes (clés de bras RÉELLES du routeur)
/// et domaines déclarés.
fn batir_graphe(store: &Arc<ObrainStore>) {
    let mut s = store.db().session();
    s.begin_transaction().expect("begin");
    for (domaine, classes) in DOMAINES {
        let d = s.create_node(&["domaine"]);
        s.set_node_property(d, "nom", Value::String(domaine.into()));
        for classe in classes {
            let n = s.create_node(&["classe"]);
            s.set_node_property(n, "nom", Value::String(classe.into()));
            s.create_edge(n, d, "APPARTIENT");
        }
    }
    s.commit().expect("commit");
}

fn agreger(
    observations: &[(String, String, bool)],
    classes: &[String],
) -> HashMap<String, (f64, f64)> {
    let mut table: HashMap<String, (f64, f64)> = HashMap::new();
    for (classe, provider, ok) in observations {
        if !classes.iter().any(|c| c == classe) {
            continue;
        }
        let e = table.entry(provider.clone()).or_insert((1.0, 1.0));
        if *ok {
            e.0 += 1.0;
        } else {
            e.1 += 1.0;
        }
    }
    table
}

fn bras_pour(classe: &str, table: &HashMap<String, (f64, f64)>) -> Vec<ArmStats> {
    (0..PROVIDERS.len())
        .map(|i| {
            let p = PROVIDERS[i];
            let (a, b) = table.get(p).copied().unwrap_or((1.0, 1.0));
            ArmStats::fresh(ArmKey::new(classe, p, MODELES[i]), a, b)
        })
        .collect()
}

// ── Les deux apprenants ────────────────────────────────────────────────────

/// Aucun partage : chaque classe n'apprend que d'elle-même.
struct LocalSeul {
    store: Arc<ObrainStore>,
    couts: Rc<Couts>,
    /// `decision_id → classe` : l'attribution vient de la DÉCISION (portée par
    /// `Decision::class_key()` via `record`), pas d'un thread-local qui suit la
    /// dernière classe consultée par `arms`.
    en_cours: Mutex<HashMap<u128, String>>,
}

/// Partage par domaine, lu dans le graphe Obrain.
struct Relationnel {
    store: Arc<ObrainStore>,
    couts: Rc<Couts>,
    en_cours: Mutex<HashMap<u128, String>>,
}

#[async_trait(?Send)]
impl Learning for LocalSeul {
    async fn arms(&self, classe: &str) -> Vec<ArmStats> {
        let t0 = Instant::now();
        self.couts.appels_arms.fetch_add(1, Ordering::Relaxed);
        let observations = lire_observations(&self.store, &self.couts);
        let table = agreger(&observations, &[classe.to_string()]);
        let bras = bras_pour(classe, &table);
        self.couts.ajouter(&self.couts.duree_arms, t0.elapsed());
        bras
    }
    async fn record(&self, d: &Decision) {
        if d.used.is_some()
            && let Some(c) = d.class_key()
        {
            self.en_cours.lock().unwrap().insert(d.id, c);
        }
    }
    async fn settle(&self, id: u128, served: Option<&Pick>, stop: &Stop, _code: Option<&str>) {
        let t0 = Instant::now();
        self.couts.settle_recus.fetch_add(1, Ordering::Relaxed);
        let classe = self.en_cours.lock().unwrap().remove(&id);
        if let (Some(p), Some(c)) = (served, classe) {
            let ok = matches!(stop, Stop::Done);
            self.couts
                .observations_ecrites
                .fetch_add(1, Ordering::Relaxed);
            ecrire(
                &self.store,
                &self.couts,
                id,
                &c,
                &p.provider_id,
                &p.model,
                ok,
            );
        }
        self.couts.ajouter(&self.couts.duree_settle, t0.elapsed());
    }
}

#[async_trait(?Send)]
impl Learning for Relationnel {
    async fn arms(&self, classe: &str) -> Vec<ArmStats> {
        let t0 = Instant::now();
        self.couts.appels_arms.fetch_add(1, Ordering::Relaxed);
        let mut classes = soeurs(&self.store, &self.couts, classe);
        if !classes.iter().any(|c| c == classe) {
            classes.push(classe.to_string());
        }
        let observations = lire_observations(&self.store, &self.couts);
        let table = agreger(&observations, &classes);
        let bras = bras_pour(classe, &table);
        self.couts.ajouter(&self.couts.duree_arms, t0.elapsed());
        bras
    }
    async fn record(&self, d: &Decision) {
        if d.used.is_some()
            && let Some(c) = d.class_key()
        {
            self.en_cours.lock().unwrap().insert(d.id, c);
        }
    }
    async fn settle(&self, id: u128, served: Option<&Pick>, stop: &Stop, _code: Option<&str>) {
        let t0 = Instant::now();
        self.couts.settle_recus.fetch_add(1, Ordering::Relaxed);
        let classe = self.en_cours.lock().unwrap().remove(&id);
        if let (Some(p), Some(c)) = (served, classe) {
            let ok = matches!(stop, Stop::Done);
            self.couts
                .observations_ecrites
                .fetch_add(1, Ordering::Relaxed);
            ecrire(
                &self.store,
                &self.couts,
                id,
                &c,
                &p.provider_id,
                &p.model,
                ok,
            );
        }
        self.couts.ajouter(&self.couts.duree_settle, t0.elapsed());
    }
}

// ── Le conducteur ──────────────────────────────────────────────────────────

struct Bilan {
    succes: HashMap<String, (usize, usize)>,
    tours: usize,
    appliquees: usize,
    settle_learning: usize,
    observations_ecrites: usize,
}

fn jouer(learning: Rc<dyn Learning>, couts: Rc<Couts>) -> Bilan {
    let pool = Rc::new(MemoryPool::new(vec![
        candidat("a", "m1"),
        candidat("b", "m2"),
        candidat("c", "m3"),
        candidat("d", "m4"),
    ]));
    let routeur = TurnRouter::new(
        RouterSpec::new(
            pool,
            Rc::new(SequenceSeeds::from(11)),
            "a".to_string(),
            "m1".to_string(),
        )
        .learning(learning)
        .auto()
        .settings(parametres()),
    );

    let mut bilan = Bilan {
        succes: HashMap::new(),
        tours: 0,
        appliquees: 0,
        settle_learning: 0,
        observations_ecrites: 0,
    };
    let mut compteur = 0usize;

    for phase in 0..2usize {
        let classes: &[&str] = if phase == 0 { &ENTRAINEMENT } else { &AVEUGLES };
        let tours = if phase == 0 {
            TOURS_ENTRAINEMENT
        } else {
            TOURS_AVEUGLE
        };
        for _ in 0..tours {
            for classe in classes {
                let texte = TEXTES
                    .iter()
                    .find(|(c, _)| c == classe)
                    .expect("texte de la classe")
                    .1;
                let routed = executer(routeur.route(TurnInput {
                    text: texte,
                    has_images: false,
                    hints: Default::default(),
                    honored: true,
                }));
                let Some(dec) = routed.decision.clone() else {
                    continue;
                };
                let Some(used) = dec.used.clone() else {
                    continue;
                };
                bilan.tours += 1;
                if dec.applied {
                    bilan.appliquees += 1;
                }

                let t0 = Instant::now();
                executer(routeur.record(&dec));
                couts.ajouter(&couts.duree_record, t0.elapsed());

                // Attribution RÉELLE : le tirage dépend de la paire qui a servi.
                let ok = tirage(classe, &used.provider_id, compteur, 11);
                let stop = if ok {
                    Stop::Done
                } else {
                    Stop::Failed("transport".to_string())
                };
                executer(routeur.settle(&stop, None, true));

                let e = bilan.succes.entry((*classe).to_string()).or_insert((0, 0));
                e.1 += 1;
                if ok {
                    e.0 += 1;
                }
                compteur += 1;
            }
        }
    }
    bilan.settle_learning = couts.settle_recus.load(Ordering::Relaxed);
    bilan.observations_ecrites = couts.observations_ecrites.load(Ordering::Relaxed);
    bilan
}

#[test]
fn l4_natif_cycle_du_routeur_et_couts() {
    let base = std::env::temp_dir().join(format!("l4n_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);

    let mut resultats: Vec<(&str, Bilan, String)> = Vec::new();
    for partage in [false, true] {
        let d = base.join(if partage { "relat" } else { "local" });
        let store = ObrainStore::open(&d).expect("ouverture");
        batir_graphe(&store);
        let couts = Rc::new(Couts::default());
        let learning: Rc<dyn Learning> = if partage {
            Rc::new(Relationnel {
                store: store.clone(),
                couts: couts.clone(),
                en_cours: Mutex::new(HashMap::new()),
            })
        } else {
            Rc::new(LocalSeul {
                store: store.clone(),
                couts: couts.clone(),
                en_cours: Mutex::new(HashMap::new()),
            })
        };
        let bilan = jouer(learning, couts.clone());
        let libelle = couts.bilan(bilan.tours);
        resultats.push((
            if partage { "relationnel" } else { "local seul" },
            bilan,
            libelle,
        ));
        store.commit_wal().expect("flush");
        drop(store);
    }

    println!("===== L4 natif · cycle réel du routeur (route → record → settle) =====");
    println!(
        "  {: <12} {: >12} {: >12} {: >10} {: >9}",
        "apprenant", "debug(entr.)", "impact(entr.)", "explore", "plan"
    );
    for (nom, bilan, _) in &resultats {
        let taux = |c: &str| match bilan.succes.get(c) {
            Some((s, n)) if *n > 0 => format!("{s}/{n}"),
            _ => "—".to_string(),
        };
        println!(
            "  {: <12} {: >12} {: >12} {: >10} {: >9}",
            nom,
            taux("chat.debug"),
            taux("chat.impact"),
            taux("chat.explore"),
            taux("chat.plan")
        );
    }
    println!("  --- coûts mesurés ---");
    for (nom, bilan, cout) in &resultats {
        println!(
            "  {nom: <12} {cout} · {} tours · {} appliquées · {} settle reçus par l'apprentissage",
            bilan.tours, bilan.appliquees, bilan.settle_learning
        );
    }
    println!("=======================================================================");

    // ── INVARIANTS (assertés) ─────────────────────────────────────────────
    for (nom, bilan, _) in &resultats {
        assert!(
            bilan.tours > 0 && bilan.appliquees > 0,
            "{nom} : aucune décision appliquée — le cycle natif n'a pas tourné"
        );
        // L'attribution : le `settle` reçu par l'apprentissage ne peut pas dépasser
        // les décisions appliquées par le routeur (une décision non appliquée n'ouvre
        // aucune attente).
        assert!(
            bilan.settle_learning > 0 && bilan.settle_learning <= bilan.appliquees,
            "{nom} : {} settle pour {} décisions appliquées — l'attribution est incohérente",
            bilan.settle_learning,
            bilan.appliquees
        );
        assert!(
            bilan.observations_ecrites > 0,
            "{nom} : aucune observation écrite — rien n'a été appris, la mesure serait vide"
        );
    }

    // ── HYPOTHÈSES (imprimées, jamais assertées) ──────────────────────────
    let local = &resultats[0].1;
    let relat = &resultats[1].1;
    let taux_de = |b: &Bilan, c: &str| {
        b.succes
            .get(c)
            .map(|(s, n)| if *n == 0 { 0.0 } else { *s as f64 / *n as f64 })
            .unwrap_or(0.0)
    };
    let moy =
        |b: &Bilan| AVEUGLES.iter().map(|c| taux_de(b, c)).sum::<f64>() / AVEUGLES.len() as f64;
    let _ = (moy(local), moy(relat));
    // Seuil : 4 réussites de plus sur 80 décisions = 5 points. La comparaison est
    // faite sur les FRACTIONS EXACTES, pas sur l'affichage arrondi (0,587 vs 0,637
    // cache 47/80 et 51/80). `>=` et non `>` : atteindre le seuil n'est pas le rater.
    let (succ_local, tot_local) =
        AVEUGLES
            .iter()
            .fold((0usize, 0usize), |(s, n), c| match local.succes.get(*c) {
                Some((a, b)) => (s + a, n + b),
                None => (s, n),
            });
    let (succ_relat, tot_relat) =
        AVEUGLES
            .iter()
            .fold((0usize, 0usize), |(s, n), c| match relat.succes.get(*c) {
                Some((a, b)) => (s + a, n + b),
                None => (s, n),
            });
    let ecart = (succ_relat as i64 - succ_local as i64) as f64 / tot_local.max(1) as f64;
    println!(
        "  [{}] H1 le partage par domaine améliore les intentions aveugles à travers le cycle natif — {succ_local}/{tot_local} vs {succ_relat}/{tot_relat} = {ecart:+.4} (seuil +0.05)",
        if ecart >= 0.05 {
            "SEUIL ATTEINT (non concluant : 4 décisions d'écart)"
        } else {
            "RÉFUTÉE  "
        }
    );
    println!(
        "  [{}] H2 le partage ne dégrade pas les intentions d'entraînement — debug {:.3} vs {:.3}, impact {:.3} vs {:.3}",
        if taux_de(relat, "chat.debug") + taux_de(relat, "chat.impact")
            >= taux_de(local, "chat.debug") + taux_de(local, "chat.impact")
        {
            "CONFIRMÉE"
        } else {
            "RÉFUTÉE  "
        },
        taux_de(local, "chat.debug"),
        taux_de(relat, "chat.debug"),
        taux_de(local, "chat.impact"),
        taux_de(relat, "chat.impact")
    );
    println!(
        "  [MESURÉ  ] H3 le coût du partage est mesurable — voir requêtes/tour et durées par appel"
    );

    let _ = std::fs::remove_dir_all(&base);
}
