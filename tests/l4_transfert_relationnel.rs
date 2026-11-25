#![cfg(all(feature = "obrain", feature = "objects-toolbox"))]
//! **L4** — transfert relationnel entre classes INÉDITES.
//!
//! Un corpus où PLUSIEURS CLASSES DISTINCTES partagent une structure relationnelle
//! pertinente (le *domaine*), mais où cette structure ne donne PAS la réponse :
//! le meilleur bras d'une classe reste à découvrir par l'exploration propre à la
//! classe. Cinq régimes comparés à état initial et tirages identiques :
//!
//! 0. **sans mémoire** — a priori neutre, jamais mis à jour (plancher) ;
//! 1. **Beta par classe** (référence L2) — mémoire en cours de route, aucun transfert ;
//! 2. **Beta persistant dans Obrain** (L3) — même modèle, persisté, aucun transfert ;
//! 3. **modèle hiérarchique** (témoin classique) — partage d'information GLOBAL
//!    (toutes classes confondues), pour ne pas attribuer à Obrain un gain qu'une
//!    méthode classique reproduit ;
//! 4. **rappel relationnel natif Obrain** — partage par le DOMAINE, découvert par
//!    des relations du graphe (`(:classe)-[:APPARTIENT]->(:domaine)`).
//!
//! Évalué sur des classes **inédites** : les tirages sont pré-générés (indépendants
//! de la stratégie) et **aucune** information postérieure à la décision n'est lue.
//! Mesure : taux de réussite, coût (échecs = tours perdus), généralisation (réussite
//! précoce sur l'inédit, avant que la classe n'ait appris d'elle-même).
//!
//! Réutilise `ObrainStore`, `execute_query` (GQL) et le port `Learning` de
//! this-harness. Aucun second moteur cognitif : le tirage de Thompson est
//! `beta_draw`, la primitive même du crate.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use futures::executor::block_on;
use obrain_common::types::Value;
use this::storage::obrain::ObrainStore;
use this_agent::Stop;
use this_harness::routing::{ArmKey, ArmStats, Learning, Pick, beta_draw, seed_of};

// ── Corpus : structure latente DÉTERMINISTE ────────────────────────────────

/// Les quatre bras (un fournisseur, un modèle).
const BRAS: [&str; 4] = ["a", "b", "c", "d"];
const MODELE: &str = "m";

/// (nom de classe, domaine) — 6 d'entraînement (c0..c5) puis 4 inédites (c6..c9).
const CLASSES: [(&str, &str); 10] = [
    ("c0", "d0"),
    ("c1", "d0"),
    ("c2", "d0"),
    ("c3", "d0"), // d0 (« chat ») : le bras `a` domine (4 classes)
    ("c4", "d1"), // d1 (« code ») : le bras `b` domine
    ("c5", "d2"), // d2 (« math ») : le bras `c` domine
    ("c6", "d0"),
    ("c7", "d0"), // inédites de d0
    ("c8", "d1"), // inédite de d1
    ("c9", "d2"), // inédite de d2
];
const NB_ENTRAINEMENT: usize = 6; // c0..c5

/// Tours par classe.
const TOURS: usize = 80;
/// Fenêtre « précoce » (généralisation) : premières décisions d'une classe inédite.
const PRECOCE: usize = 20;
/// Répétitions (tirages régénérés par graine, même structure latente).
const REPS: u64 = 3;

fn fnv1a(s: &str) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in s.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    h
}

fn domaine_de(classe: &str) -> &'static str {
    CLASSES
        .iter()
        .find(|(c, _)| *c == classe)
        .map(|(_, d)| *d)
        .expect("classe connue")
}

fn indice_classe(classe: &str) -> usize {
    CLASSES
        .iter()
        .position(|(c, _)| *c == classe)
        .expect("classe connue")
}

/// Logit global d'un bras (partagé entre toutes les classes).
fn logit_global(bras: &str) -> f64 {
    match bras {
        "a" => 1.6,
        "b" => 0.5,
        "c" => 0.3,
        "d" => -0.8,
        _ => unreachable!(),
    }
}

/// Boost propre au domaine (la structure relationnelle, non suffisante à répondre).
fn boost_domaine(domaine: &str, bras: &str) -> f64 {
    match (domaine, bras) {
        ("d0", "a") => 0.3,  // chat : a encore meilleur
        ("d1", "a") => -1.3, // code : a faiblit, b émerge
        ("d1", "b") => 2.0,
        ("d2", "a") => -1.3, // math : a faiblit, c émerge
        ("d2", "c") => 2.0,
        _ => 0.0,
    }
}

/// Gigue de classe, déterministe, bornée à ±0.10 (le meilleur bras reste stable).
fn gigue(classe: &str, bras: &str) -> f64 {
    let h = fnv1a(&format!("{classe}/{bras}"));
    ((h % 1000) as f64 / 1000.0) * 0.20 - 0.10
}

fn theta(classe: &str, bras: &str) -> f64 {
    let z = logit_global(bras) + boost_domaine(domaine_de(classe), bras) + gigue(classe, bras);
    1.0 / (1.0 + (-z).exp())
}

fn meilleur_bras(classe: &str) -> &'static str {
    BRAS.iter()
        .copied()
        .max_by(|a, b| theta(classe, a).total_cmp(&theta(classe, b)))
        .expect("un bras")
}

// ── Tirages pré-générés, indépendants de la stratégie ──────────────────────

fn lcg(etat: &mut u64) -> u64 {
    *etat = etat
        .wrapping_mul(6364136223846793005)
        .wrapping_add(1442695040888963407);
    *etat
}

/// Le `j`-ième tirage du bras `bras` dans la classe `classe` (répétition `rep`).
/// Déterministe : la même séquence pour toutes les stratégies.
fn tirage(classe: &str, bras: &str, j: usize, rep: u64) -> bool {
    let mut etat = fnv1a(&format!("{rep}/{classe}/{bras}"));
    for _ in 0..=j {
        lcg(&mut etat);
    }
    let v = etat % 10_000;
    (v as f64) < theta(classe, bras) * 10_000.0
}

// ── Le tirage de Thompson (primitive du crate, pas un second moteur) ───────

fn choisir(bras: &[ArmStats], id: u128) -> (String, String) {
    let mut meilleur: Option<(f64, &ArmStats)> = None;
    for (i, bras_) in bras.iter().enumerate() {
        let graine = seed_of(id) ^ ((i as u64).wrapping_add(1));
        let tirage = beta_draw(bras_.alpha, bras_.beta, graine);
        if meilleur.map(|(m, _)| tirage > m).unwrap_or(true) {
            meilleur = Some((tirage, bras_));
        }
    }
    let (_, bras_) = meilleur.expect("au moins un bras");
    (bras_.key.provider_id.clone(), bras_.key.model.clone())
}

fn run<F: std::future::Future<Output = T>, T>(f: F) -> T {
    block_on(f)
}

fn echapper(s: &str) -> String {
    s.replace('\\', "\\\\").replace('\'', "\\'")
}

// ── Écriture d'observation (partagée par les stratégies persistantes) ──────

fn ecrire_observation(
    store: &Arc<ObrainStore>,
    id: u128,
    classe: &str,
    provider: &str,
    modele: &str,
    favorable: bool,
) {
    let mut s = store.db().session();
    s.begin_transaction().expect("begin");
    let n = s.create_node(&["observation"]);
    s.set_node_property(n, "evenement", Value::String(id.to_string().into()));
    s.set_node_property(n, "classe", Value::String(classe.into()));
    s.set_node_property(n, "provider", Value::String(provider.into()));
    s.set_node_property(n, "modele", Value::String(modele.into()));
    s.set_node_property(n, "favorable", Value::Bool(favorable));
    s.commit().expect("commit");
}

/// Relit les observations et projette (provider -> (alpha, beta)), a priori 1/1.
/// `classes` : `None` = toutes ; `Some(liste)` = seulement ces classes.
fn lire_observations(
    store: &Arc<ObrainStore>,
    classes: Option<&[&str]>,
) -> HashMap<String, (f64, f64)> {
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
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default();
    let ic = colonnes.iter().position(|c| c == "c").unwrap_or(0);
    let ip = colonnes.iter().position(|c| c == "p").unwrap_or(1);
    let iff = colonnes.iter().position(|c| c == "f").unwrap_or(2);

    let mut table: HashMap<String, (u32, u32)> = HashMap::new();
    for ligne in r
        .get("rows")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default()
    {
        let c = ligne.get(ic).and_then(|v| v.as_str()).unwrap_or_default();
        if let Some(liste) = classes
            && !liste.contains(&c)
        {
            continue;
        }
        let p = ligne.get(ip).and_then(|v| v.as_str()).unwrap_or_default();
        let f = ligne.get(iff).and_then(|v| v.as_bool()).unwrap_or(false);
        let e = table.entry(p.to_string()).or_insert((1, 1));
        if f {
            e.0 += 1;
        } else {
            e.1 += 1;
        }
    }
    table
        .into_iter()
        .map(|(p, (a, b))| (p, (a as f64, b as f64)))
        .collect()
}

fn bras_a_priori(classe: &str, priors: &HashMap<String, (f64, f64)>) -> Vec<ArmStats> {
    BRAS.iter()
        .map(|b| {
            let (a, bb) = priors.get(*b).copied().unwrap_or((1.0, 1.0));
            ArmStats::fresh(ArmKey::new(classe, *b, MODELE), a, bb)
        })
        .collect()
}

fn classe_de_id(id: u128) -> &'static str {
    CLASSES[(id >> 64) as usize].0
}

// ── Stratégie 0 : sans mémoire ─────────────────────────────────────────────

struct SansMemoire;

#[async_trait(?Send)]
impl Learning for SansMemoire {
    async fn arms(&self, classe: &str) -> Vec<ArmStats> {
        bras_a_priori(classe, &HashMap::new())
    }
    async fn settle(&self, _id: u128, _served: Option<&Pick>, _stop: &Stop, _code: Option<&str>) {}
}

// ── Stratégie 1 : Beta par classe, en mémoire (L2) ─────────────────────────

struct ParClasseMemoire {
    bras: Mutex<HashMap<(String, String), (f64, f64)>>,
}

impl ParClasseMemoire {
    fn new() -> Self {
        Self {
            bras: Mutex::new(HashMap::new()),
        }
    }
}

#[async_trait(?Send)]
impl Learning for ParClasseMemoire {
    async fn arms(&self, classe: &str) -> Vec<ArmStats> {
        let m = self.bras.lock().unwrap();
        let priors: HashMap<String, (f64, f64)> = m
            .iter()
            .filter(|((c, _), _)| c == classe)
            .map(|((_, p), v)| (p.clone(), *v))
            .collect();
        bras_a_priori(classe, &priors)
    }
    async fn settle(&self, id: u128, served: Option<&Pick>, stop: &Stop, _code: Option<&str>) {
        let Some(p) = served else { return };
        let (da, db) = match stop {
            Stop::Done => (1.0, 0.0),
            Stop::Failed(_) => (0.0, 1.0),
            _ => return,
        };
        let classe = classe_de_id(id);
        let mut m = self.bras.lock().unwrap();
        let e = m
            .entry((classe.to_string(), p.provider_id.clone()))
            .or_insert((1.0, 1.0));
        e.0 += da;
        e.1 += db;
    }
}

// ── Stratégie 2 : Beta par classe, persistée dans Obrain (L3) ──────────────

struct ParClasseObrain {
    store: Arc<ObrainStore>,
}

#[async_trait(?Send)]
impl Learning for ParClasseObrain {
    async fn arms(&self, classe: &str) -> Vec<ArmStats> {
        let priors = lire_observations(&self.store, Some(&[classe]));
        bras_a_priori(classe, &priors)
    }
    async fn settle(&self, id: u128, served: Option<&Pick>, stop: &Stop, _code: Option<&str>) {
        let Some(p) = served else { return };
        let favorable = match stop {
            Stop::Done => true,
            Stop::Failed(_) => false,
            _ => return,
        };
        let classe = classe_de_id(id);
        ecrire_observation(&self.store, id, classe, &p.provider_id, &p.model, favorable);
    }
}

// ── Stratégie 3 : modèle hiérarchique (témoin classique, partage global) ───

struct Hierarchique {
    store: Arc<ObrainStore>,
}

#[async_trait(?Send)]
impl Learning for Hierarchique {
    async fn arms(&self, classe: &str) -> Vec<ArmStats> {
        let priors = lire_observations(&self.store, None);
        bras_a_priori(classe, &priors)
    }
    async fn settle(&self, id: u128, served: Option<&Pick>, stop: &Stop, _code: Option<&str>) {
        let Some(p) = served else { return };
        let favorable = match stop {
            Stop::Done => true,
            Stop::Failed(_) => false,
            _ => return,
        };
        let classe = classe_de_id(id);
        ecrire_observation(&self.store, id, classe, &p.provider_id, &p.model, favorable);
    }
}

// ── Stratégie 4 : rappel relationnel natif Obrain ──────────────────────────

struct Relationnel {
    store: Arc<ObrainStore>,
}

impl Relationnel {
    /// Bâtit le graphe de structure : classes, domaines, arêtes APPARTIENT.
    fn nouveau(store: Arc<ObrainStore>) -> Self {
        {
            let mut s = store.db().session();
            s.begin_transaction().expect("begin");
            let mut domaines: HashMap<&str, obrain_common::types::NodeId> = HashMap::new();
            for (classe, domaine) in CLASSES {
                let n = s.create_node(&["classe"]);
                s.set_node_property(n, "nom", Value::String(classe.into()));
                let d = *domaines.entry(domaine).or_insert_with(|| {
                    let dn = s.create_node(&["domaine"]);
                    s.set_node_property(dn, "nom", Value::String(domaine.into()));
                    dn
                });
                s.create_edge(n, d, "APPARTIENT");
            }
            s.commit().expect("commit");
        }
        Self { store }
    }

    /// Les classes du domaine de `classe`, par relations du graphe.
    fn classes_du_domaine(&self, classe: &str) -> Vec<String> {
        let domaine = self
            .store
            .execute_query(
                "gql",
                &format!(
                    "MATCH (c:classe)-[:APPARTIENT]->(d:domaine) WHERE c.nom = '{}' RETURN d.nom AS nom",
                    echapper(classe)
                ),
                10,
            )
            .expect("domaine")
            .get("rows")
            .and_then(|v| v.as_array())
            .cloned()
            .expect("réponse sans rows")
            .into_iter()
            .next()
            .and_then(|l| l.get(0).and_then(|v| v.as_str()).map(|s| s.to_string()))
            .expect("domaine trouvé");

        self.store
            .execute_query(
                "gql",
                &format!(
                    "MATCH (s:classe)-[:APPARTIENT]->(d:domaine) WHERE d.nom = '{}' RETURN s.nom AS nom",
                    echapper(&domaine)
                ),
                100_000,
            )
            .expect("classes du domaine")
            .get("rows")
            .and_then(|v| v.as_array())
            .cloned()
            .expect("réponse sans rows")
            .into_iter()
            .filter_map(|l| l.get(0).and_then(|v| v.as_str()).map(|s| s.to_string()))
            .collect()
    }
}

#[async_trait(?Send)]
impl Learning for Relationnel {
    async fn arms(&self, classe: &str) -> Vec<ArmStats> {
        let sœurs: Vec<String> = self.classes_du_domaine(classe);
        let sœurs_ref: Vec<&str> = sœurs.iter().map(|s| s.as_str()).collect();
        let priors = lire_observations(&self.store, Some(&sœurs_ref));
        bras_a_priori(classe, &priors)
    }
    async fn settle(&self, id: u128, served: Option<&Pick>, stop: &Stop, _code: Option<&str>) {
        let Some(p) = served else { return };
        let favorable = match stop {
            Stop::Done => true,
            Stop::Failed(_) => false,
            _ => return,
        };
        let classe = classe_de_id(id);
        ecrire_observation(&self.store, id, classe, &p.provider_id, &p.model, favorable);
    }
}

// ── Le conducteur (mêmes tirages, mêmes décisions pour toutes les stratégies) ─

/// Joue `classes` (dans l'ordre) et rend, par classe, la trajectoire (bras, succès).
fn jouer(learning: &dyn Learning, rep: u64, classes: &[&str]) -> Vec<Vec<(String, bool)>> {
    let mut resultat = Vec::new();
    for &classe in classes {
        let mut traj = Vec::new();
        let mut tirages_bras: HashMap<String, usize> = HashMap::new();
        for t in 0..TOURS {
            let bras = run(learning.arms(classe));
            let id = ((indice_classe(classe) as u128) << 64) | (t as u128);
            let (provider, modele) = choisir(&bras, id);
            let j = *tirages_bras.entry(provider.clone()).or_insert(0);
            tirages_bras.insert(provider.clone(), j + 1);
            let ok = tirage(classe, &provider, j, rep);
            let pick = Pick::new(provider.clone(), modele.clone());
            let stop = if ok {
                Stop::Done
            } else {
                Stop::Failed("transport".into())
            };
            run(learning.settle(id, Some(&pick), &stop, None));
            traj.push((provider, ok));
        }
        resultat.push(traj);
    }
    resultat
}

#[derive(Default, Debug, Clone)]
struct Metriques {
    decisions: usize,
    succes: usize,
    decisions_precoces: usize,
    succes_precoces: usize,
}

fn ajouter(mut a: Metriques, b: Metriques) -> Metriques {
    a.decisions += b.decisions;
    a.succes += b.succes;
    a.decisions_precoces += b.decisions_precoces;
    a.succes_precoces += b.succes_precoces;
    a
}

fn metriques_inédites(trajs: &[Vec<(String, bool)>]) -> Metriques {
    let mut m = Metriques::default();
    for traj in trajs.iter().skip(NB_ENTRAINEMENT) {
        for (i, (_, ok)) in traj.iter().enumerate() {
            m.decisions += 1;
            if *ok {
                m.succes += 1;
            }
            if i < PRECOCE {
                m.decisions_precoces += 1;
                if *ok {
                    m.succes_precoces += 1;
                }
            }
        }
    }
    m
}

fn taux(m: &Metriques, precoce: bool) -> f64 {
    let (s, d) = if precoce {
        (m.succes_precoces, m.decisions_precoces)
    } else {
        (m.succes, m.decisions)
    };
    if d == 0 { 0.0 } else { s as f64 / d as f64 }
}

/// Réussite précoce PAR classe inédite (nom, succès, décisions).
fn precoce_par_classe(trajs: &[Vec<(String, bool)>]) -> Vec<(String, usize, usize)> {
    CLASSES[NB_ENTRAINEMENT..]
        .iter()
        .enumerate()
        .map(|(i, (nom, _))| {
            let traj = &trajs[NB_ENTRAINEMENT + i];
            let s = traj.iter().take(PRECOCE).filter(|(_, ok)| *ok).count();
            (nom.to_string(), s, PRECOCE)
        })
        .collect()
}

fn dir_unique(etiquette: &str) -> std::path::PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    static N: AtomicU64 = AtomicU64::new(0);
    let d = std::env::temp_dir().join(format!(
        "l4_{etiquette}_{}_{}",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&d);
    d
}

#[test]
fn l4_transfert_relationnel_entre_classes_inedites() {
    // ── Auto-contrôle de la structure latente ─────────────────────────────
    assert_eq!(meilleur_bras("c0"), "a", "d0 : a domine");
    assert_eq!(meilleur_bras("c4"), "b", "d1 : b domine");
    assert_eq!(meilleur_bras("c5"), "c", "d2 : c domine");
    assert_eq!(meilleur_bras("c8"), "b", "inédite d1 : b domine");
    assert_eq!(meilleur_bras("c9"), "c", "inédite d2 : c domine");

    let toutes: Vec<&str> = CLASSES.iter().map(|(c, _)| *c).collect();

    // Métriques agrégées par stratégie (somme sur les répétitions).
    let mut agg: HashMap<&str, Metriques> = HashMap::new();
    for nom in ["sans", "l2", "l3", "hier", "relat"] {
        agg.insert(nom, Metriques::default());
    }

    // Réussite précoce par classe inédite (hiérarchique vs relationnel).
    let mut par_classe_hier: HashMap<String, (usize, usize)> = HashMap::new();
    let mut par_classe_relat: HashMap<String, (usize, usize)> = HashMap::new();

    // Trajectoires inédites de L2 et L3 (doivent être IDENTIQUES : la persistance
    // n'apporte aucun transfert).
    let mut traj_l2: Vec<Vec<(String, bool)>> = Vec::new();
    let mut traj_l3: Vec<Vec<(String, bool)>> = Vec::new();

    for rep in 0..REPS {
        // 0 — sans mémoire
        let sans = SansMemoire;
        let trajs = jouer(&sans, rep, &toutes);
        *agg.get_mut("sans").unwrap() = ajouter(agg["sans"].clone(), metriques_inédites(&trajs));

        // 1 — L2 en mémoire
        let l2 = ParClasseMemoire::new();
        let trajs = jouer(&l2, rep, &toutes);
        *agg.get_mut("l2").unwrap() = ajouter(agg["l2"].clone(), metriques_inédites(&trajs));
        if rep == 0 {
            traj_l2 = trajs[NB_ENTRAINEMENT..].to_vec();
        }

        // 2 — L3 persistant
        let d = dir_unique("l3");
        let store = ObrainStore::open(&d).expect("ouverture L3");
        let l3 = ParClasseObrain {
            store: store.clone(),
        };
        let trajs = jouer(&l3, rep, &toutes);
        *agg.get_mut("l3").unwrap() = ajouter(agg["l3"].clone(), metriques_inédites(&trajs));
        if rep == 0 {
            traj_l3 = trajs[NB_ENTRAINEMENT..].to_vec();
        }
        store.commit_wal().expect("flush");
        drop(store);
        let _ = std::fs::remove_dir_all(&d);

        // 3 — hiérarchique
        let d = dir_unique("hier");
        let store = ObrainStore::open(&d).expect("ouverture hier");
        let hier = Hierarchique {
            store: store.clone(),
        };
        let trajs = jouer(&hier, rep, &toutes);
        *agg.get_mut("hier").unwrap() = ajouter(agg["hier"].clone(), metriques_inédites(&trajs));
        for (nom, s, n) in precoce_par_classe(&trajs) {
            let e = par_classe_hier.entry(nom).or_insert((0, 0));
            e.0 += s;
            e.1 += n;
        }
        store.commit_wal().expect("flush");
        drop(store);
        let _ = std::fs::remove_dir_all(&d);

        // 4 — relationnel
        let d = dir_unique("relat");
        let store = ObrainStore::open(&d).expect("ouverture relat");
        let relat = Relationnel::nouveau(store.clone());
        let trajs = jouer(&relat, rep, &toutes);
        *agg.get_mut("relat").unwrap() = ajouter(agg["relat"].clone(), metriques_inédites(&trajs));
        for (nom, s, n) in precoce_par_classe(&trajs) {
            let e = par_classe_relat.entry(nom).or_insert((0, 0));
            e.0 += s;
            e.1 += n;
        }
        store.commit_wal().expect("flush");
        drop(store);
        let _ = std::fs::remove_dir_all(&d);
    }

    // ── La persistance seule n'apporte aucun transfert ─────────────────────
    assert_eq!(
        traj_l2, traj_l3,
        "L2 et L3 doivent produire la MÊME trajectoire inédite (persistance ≠ transfert)"
    );

    // ── Rapport ────────────────────────────────────────────────────────────
    let prec_sans = taux(&agg["sans"], true);
    let prec_l2 = taux(&agg["l2"], true);
    let prec_hier = taux(&agg["hier"], true);
    let prec_relat = taux(&agg["relat"], true);
    let tot_sans = taux(&agg["sans"], false);
    let tot_l2 = taux(&agg["l2"], false);
    let tot_hier = taux(&agg["hier"], false);
    let tot_relat = taux(&agg["relat"], false);
    let echecs = |nom: &str| agg[nom].decisions - agg[nom].succes;

    println!("========== L4 — réussite sur classes INÉDITES ==========");
    println!(
        "  sans mémoire   : précoce {prec_sans:.3} · totale {tot_sans:.3} · échecs {}",
        echecs("sans")
    );
    println!(
        "  Beta/classe    : précoce {prec_l2:.3} · totale {tot_l2:.3} · échecs {}",
        echecs("l2")
    );
    println!(
        "  Beta persistant: précoce {:.3} · échecs {}",
        taux(&agg["l3"], true),
        echecs("l3")
    );
    println!(
        "  hiérarchique   : précoce {prec_hier:.3} · totale {tot_hier:.3} · échecs {}",
        echecs("hier")
    );
    println!(
        "  relationnel    : précoce {prec_relat:.3} · totale {tot_relat:.3} · échecs {}",
        echecs("relat")
    );
    println!("-------- réussite PRÉCOCE par classe inédite --------");
    for &(nom, _) in &CLASSES[NB_ENTRAINEMENT..] {
        let h = par_classe_hier.get(nom).copied().unwrap_or((0, 0));
        let r = par_classe_relat.get(nom).copied().unwrap_or((0, 0));
        let hh = if h.1 > 0 {
            h.0 as f64 / h.1 as f64
        } else {
            0.0
        };
        let rr = if r.1 > 0 {
            r.0 as f64 / r.1 as f64
        } else {
            0.0
        };
        println!(
            "    {nom} ({}) : hiérarchique {hh:.3} · relationnel {rr:.3} · meilleur bras {}",
            domaine_de(nom),
            meilleur_bras(nom)
        );
    }
    println!("========================================================");

    // ── Amélioration CUMULATIVE : sans < per-classe < relationnel ──────────
    // La mémoire de classe bat l'absence de mémoire (précoce ET au total).
    assert!(
        prec_l2 > prec_sans + 0.05,
        "Beta/classe doit dépasser sans mémoire en généralisation : {prec_l2:.3} > {prec_sans:.3}"
    );
    assert!(
        tot_l2 > tot_sans + 0.05,
        "Beta/classe doit dépasser sans mémoire au total : {tot_l2:.3} > {tot_sans:.3}"
    );

    // Le rappel relationnel bat la mémoire de classe — et ce n'est pas un simple
    // meilleur rappel : l'avantage tient AU TOTAL (regret cumulé plus faible),
    // pas seulement au départ.
    assert!(
        prec_relat > prec_l2 + 0.08,
        "relationnel doit dépasser Beta/classe en généralisation : {prec_relat:.3} > {prec_l2:.3}"
    );
    assert!(
        tot_relat > tot_l2 + 0.05,
        "relationnel doit dépasser Beta/classe au total (amélioration cumulative) : {tot_relat:.3} > {tot_l2:.3}"
    );

    // Le rappel relationnel bat le partage GLOBAL (témoin classique) : la structure
    // relationnelle apporte plus qu'un simple brassage de toutes les classes.
    assert!(
        prec_relat > prec_hier + 0.08,
        "relationnel doit dépasser le partage global en généralisation : {prec_relat:.3} > {prec_hier:.3}"
    );

    // Le coût (échecs) suit le même ordre : relationnel est le moins coûteux.
    assert!(
        echecs("relat") < echecs("l2"),
        "relationnel doit coûter moins cher que Beta/classe : {} < {}",
        echecs("relat"),
        echecs("l2")
    );
}
