#![cfg(all(feature = "obrain", feature = "objects-toolbox"))]
//! **L4a** — transfert par structure EXPLICITE : qualification, récupération, et
//! confiance adaptative.
//!
//! ## Ce que ce fichier sépare désormais
//!
//! * **Invariants fonctionnels** (assertés — leur violation est un défaut, pas un
//!   résultat) : `l2 ≡ l3` à chaque répétition (persister n'apprend rien), et
//!   `domaine ≡ relat` à chaque répétition dans les deux protocoles (même information
//!   + même partage ⇒ même trajectoire ; le graphe ne calcule rien de plus).
//! * **Hypothèses scientifiques** (imprimées avec un verdict, JAMAIS assertées) :
//!   elles doivent pouvoir varier selon le corpus. Un test qui casse parce qu'une
//!   hypothèse est fausse empêche la science ; un rapport qui la tait la falsifie.
//!
//! ## Régimes
//!
//! | régime | structure de partage |
//! |---|---|
//! | `sans` | aucune mémoire |
//! | `l2` / `l3` | par classe, en mémoire / persistée |
//! | `global` | mutualisation globale (brassage) |
//! | `domaine` | Beta par domaine, câblé à la main |
//! | `hier3` | hiérarchique multiniveau, partage partiel (K0 = KD = W = 4) |
//! | `relat` | rappel relationnel natif Obrain, domaine lu dans le graphe |
//! | `relat_melange` / `relat_bruit` | relations mélangées / non pertinentes |
//! | **`adaptatif`** | transfert à **confiance révisable** : λ = 1/(1+contradictions), masse transférée plafonnée |
//! | **`borne`** | transfert par domaine à **masse plafonnée** seule (pas de λ) |
//! | **`selection`** | **sélection adaptative de modèle** : local seul ou domaine, selon la vraisemblance |
//!
//! Aucune stratégie n'accède à `theta` ni au meilleur bras : ces informations
//! n'appartiennent qu'à l'évaluateur (`meilleur_bras`, `theta` ne sont appelés que
//! par les mesures et les auto-contrôles).
//!
//! ## Mesures
//!
//! Réussite **ponctuelle** après exactement 0, 1, 2, 4, 8 et 20 observations
//! (tour `k` = décidé avec `k` observations préalables), et sur le corpus trompeur :
//! premier choix optimal, fréquence optimale ensuite, fréquence optimale sur les
//! 8 derniers tours, **regret cumulé**, **récupération durable** (premier tour après
//! lequel plus aucune décision n'est sous-optimale) et **cas sans récupération**.
//! Distributions (médiane, étendue), pas le meilleur cas.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use futures::executor::block_on;
use obrain_common::types::Value;
use this::storage::obrain::ObrainStore;
use this_agent::Stop;
use this_harness::routing::{ArmKey, ArmStats, Learning, Pick, beta_draw, seed_of};

// ── Corpus : structure latente DÉTERMINISTE ────────────────────────────────

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
const NB_ENTRAINEMENT: usize = 6;

/// Classes TROMPEUSES : la structure suggère un bras, le vrai meilleur bras diffère.
const TROMPEUSES: [(&str, &str, &str); 2] = [
    ("x0", "d1", "d"), // d1 dit « b », la réalité est « d »
    ("x1", "d2", "a"), // d2 dit « c », la réalité est « a »
];

const TOURS_ENTRAINEMENT: usize = 60;
/// 24 tours : permet la mesure ponctuelle « après 20 observations » (tour 20).
const TOURS_NOUVEAU: usize = 24;
const TOURS_TROMPEUR: usize = 40;
const REPS: u64 = 3;
/// Points de mesure ponctuels : le tour `k` est décidé avec `k` observations.
const POINTS: [usize; 6] = [0, 1, 2, 4, 8, 20];
/// Masse maximale (en observations équivalentes) accordée à un transfert.
const MASSE_MAX: f64 = 8.0;

fn fnv1a(s: &str) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in s.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    h
}

fn statique(classe: &str) -> Option<&'static str> {
    if let Some((nom, _)) = CLASSES.iter().find(|(c, _)| *c == classe) {
        return Some(nom);
    }
    TROMPEUSES
        .iter()
        .find(|(c, _, _)| *c == classe)
        .map(|(c, _, _)| *c)
}

fn domaine_de(classe: &str) -> &'static str {
    if let Some((_, d)) = CLASSES.iter().find(|(c, _)| *c == classe) {
        return d;
    }
    TROMPEUSES
        .iter()
        .find(|(c, _, _)| *c == classe)
        .map(|(_, d, _)| *d)
        .expect("classe connue")
}

fn meilleur_force(classe: &str) -> Option<&'static str> {
    TROMPEUSES
        .iter()
        .find(|(c, _, _)| *c == classe)
        .map(|(_, _, m)| *m)
}

fn indice_classe(classe: &str) -> usize {
    if let Some(i) = CLASSES.iter().position(|(c, _)| *c == classe) {
        return i;
    }
    if let Some(i) = TROMPEUSES.iter().position(|(c, _, _)| *c == classe) {
        return CLASSES.len() + i;
    }
    panic!("classe inconnue : {classe}")
}

fn logit_global(bras: &str) -> f64 {
    match bras {
        "a" => 1.6,
        "b" => 0.5,
        "c" => 0.3,
        "d" => -0.8,
        _ => unreachable!(),
    }
}

fn boost_domaine(domaine: &str, bras: &str) -> f64 {
    match (domaine, bras) {
        ("d0", "a") => 0.3,
        ("d1", "a") => -1.3,
        ("d1", "b") => 2.0,
        ("d2", "a") => -1.3,
        ("d2", "c") => 2.0,
        _ => 0.0,
    }
}

fn gigue(classe: &str, bras: &str) -> f64 {
    let h = fnv1a(&format!("{classe}/{bras}"));
    ((h % 1000) as f64 / 1000.0) * 0.20 - 0.10
}

/// P(bas) du corpus. N'est appelé QUE par l'évaluateur et les auto-contrôles.
fn theta(classe: &str, bras: &str) -> f64 {
    let mut z = logit_global(bras) + boost_domaine(domaine_de(classe), bras) + gigue(classe, bras);
    if Some(bras) == meilleur_force(classe) {
        z += 4.0;
    }
    1.0 / (1.0 + (-z).exp())
}

/// Le meilleur bras réel. Réservé à l'évaluateur.
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

fn tirage(classe: &str, bras: &str, j: usize, rep: u64) -> bool {
    let mut etat = fnv1a(&format!("{rep}/{classe}/{bras}"));
    for _ in 0..=j {
        lcg(&mut etat);
    }
    let v = etat % 10_000;
    (v as f64) < theta(classe, bras) * 10_000.0
}

// ── Décisions ──────────────────────────────────────────────────────────────

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

/// Argmax de l'espérance a posteriori — aucune chance dans la mesure à 0 observation.
fn choix_deterministe(bras: &[ArmStats]) -> String {
    bras.iter()
        .max_by(|x, y| {
            let mx = x.alpha / (x.alpha + x.beta);
            let my = y.alpha / (y.alpha + y.beta);
            mx.total_cmp(&my)
        })
        .expect("au moins un bras")
        .key
        .provider_id
        .clone()
}

fn run<F: std::future::Future<Output = T>, T>(f: F) -> T {
    block_on(f)
}

fn echapper(s: &str) -> String {
    s.replace('\\', "\\\\").replace('\'', "\\'")
}

// ── Lectures strictes (une erreur n'est jamais un résultat vide) ───────────

fn lire_lignes(store: &Arc<ObrainStore>, limite: usize) -> Vec<(String, String, bool)> {
    let r = store
        .execute_query(
            "gql",
            "MATCH (o:observation) RETURN o.classe AS c, o.provider AS p, o.favorable AS f",
            limite,
        )
        .expect("lecture des observations");
    let colonnes: Vec<String> = r
        .get("columns")
        .and_then(|c| c.as_array())
        .expect("réponse sans 'columns' : la lecture n'est pas exploitable")
        .iter()
        .map(|v| v.as_str().expect("colonne non textuelle").to_string())
        .collect();
    let idx = |nom: &str| {
        colonnes
            .iter()
            .position(|c| c == nom)
            .unwrap_or_else(|| panic!("colonne '{nom}' absente de la réponse : {colonnes:?}"))
    };
    let (ic, ip, iff) = (idx("c"), idx("p"), idx("f"));
    let rows = r
        .get("rows")
        .and_then(|v| v.as_array())
        .expect("réponse sans 'rows' : la lecture n'est pas exploitable");
    assert!(
        rows.len() < limite,
        "réponse possiblement TRONQUÉE : {} lignes pour une limite de {limite}",
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

// ── Agrégats ───────────────────────────────────────────────────────────────

/// provider -> (succès, essais), sur les classes autorisées et/ou un domaine.
fn comptes(
    lignes: &[(String, String, bool)],
    autorisees: Option<&[&'static str]>,
    domaine: Option<&str>,
) -> HashMap<String, (f64, f64)> {
    let mut table: HashMap<String, (f64, f64)> = HashMap::new();
    for (classe, provider, favorable) in lignes {
        if let Some(liste) = autorisees
            && !liste.contains(&classe.as_str())
        {
            continue;
        }
        if let Some(d) = domaine
            && domaine_de(classe) != d
        {
            continue;
        }
        let e = table.entry(provider.clone()).or_insert((0.0, 0.0));
        e.1 += 1.0;
        if *favorable {
            e.0 += 1.0;
        }
    }
    table
}

/// comptes par (classe, provider) — pour le modèle hiérarchique et l'adaptatif.
fn comptes_par_classe(
    lignes: &[(String, String, bool)],
    autorisees: Option<&[&'static str]>,
) -> HashMap<(String, String), (f64, f64)> {
    let mut table: HashMap<(String, String), (f64, f64)> = HashMap::new();
    for (classe, provider, favorable) in lignes {
        if let Some(liste) = autorisees
            && !liste.contains(&classe.as_str())
        {
            continue;
        }
        let e = table
            .entry((classe.clone(), provider.clone()))
            .or_insert((0.0, 0.0));
        e.1 += 1.0;
        if *favorable {
            e.0 += 1.0;
        }
    }
    table
}

/// Beta(1 + λ·succès, 1 + λ·(essais - succès)), masse optionnellement plafonnée.
fn priors_depuis(
    comptes: &HashMap<String, (f64, f64)>,
    lam: f64,
    plafond: Option<f64>,
) -> HashMap<String, (f64, f64)> {
    let total: f64 = comptes.values().map(|(_, n)| n).sum();
    let mut l = lam;
    if let Some(m) = plafond
        && l * total > m
        && *&total > 0.0
    {
        l = m / total;
    }
    comptes
        .iter()
        .map(|(p, (s, n))| (p.clone(), (1.0 + l * s, 1.0 + l * (n - s))))
        .collect()
}

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

fn bras_a_priori(classe: &str, priors: &HashMap<String, (f64, f64)>) -> Vec<ArmStats> {
    BRAS.iter()
        .map(|b| {
            let (a, bb) = priors.get(*b).copied().unwrap_or((1.0, 1.0));
            ArmStats::fresh(ArmKey::new(classe, *b, MODELE), a, bb)
        })
        .collect()
}

fn classe_de_id(id: u128) -> &'static str {
    let i = (id >> 64) as usize;
    if i < CLASSES.len() {
        CLASSES[i].0
    } else {
        TROMPEUSES[i - CLASSES.len()].0
    }
}

// ── Protocoles ─────────────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq, Eq)]
enum Protocole {
    /// Chaque inédite part du même état : les lectures excluent les autres inédites.
    Fige,
    /// Les inédites se succèdent et se voient mutuellement.
    Continu,
}

fn autorisees(protocole: Protocole, classe: &str) -> Option<Vec<&'static str>> {
    match protocole {
        Protocole::Continu => None,
        Protocole::Fige => {
            let mut v: Vec<&'static str> =
                CLASSES[..NB_ENTRAINEMENT].iter().map(|(c, _)| *c).collect();
            if let Some(s) = statique(classe)
                && !v.contains(&s)
            {
                v.push(s);
            }
            Some(v)
        }
    }
}

// ── `sans` : aucune mémoire ────────────────────────────────────────────────

struct SansMemoire;

#[async_trait(?Send)]
impl Learning for SansMemoire {
    async fn arms(&self, classe: &str) -> Vec<ArmStats> {
        bras_a_priori(classe, &HashMap::new())
    }
    async fn settle(&self, _id: u128, _served: Option<&Pick>, _stop: &Stop, _code: Option<&str>) {}
}

// ── `l2` : Beta par classe, en mémoire ─────────────────────────────────────

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

// ── Le partage ─────────────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq, Eq)]
enum Variante {
    Normale,
    Melangee,
    Bruit,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Partage {
    /// Uniquement les observations de la classe (c'est L3).
    Classe,
    /// Toutes les observations, brassées.
    Global,
    /// Domaine, câblé à la main (table constante).
    Domaine,
    /// Hiérarchique multiniveau, partage partiel.
    Hier3,
    /// Domaine, masse transférée PLAFONNÉE.
    Borne,
    /// Domaine, confiance λ révisable + masse plafonnée.
    Adaptatif,
    /// Sélection adaptative local / domaine selon la vraisemblance.
    Selection,
    /// Domaine lu dans le graphe Obrain (et ses variantes trompeuses).
    Graphe(Variante),
}

const K0: f64 = 4.0;
const KD: f64 = 4.0;
const W: f64 = 4.0;

#[derive(Default)]
struct EtatAdaptatif {
    /// Observations qui contredisent la prédiction transférée.
    contradictions: u32,
}

struct Strategie {
    store: Arc<ObrainStore>,
    protocole: Protocole,
    partage: Partage,
    etat: Mutex<HashMap<String, EtatAdaptatif>>,
}

impl Strategie {
    fn nouvelle(store: Arc<ObrainStore>, protocole: Protocole, partage: Partage) -> Self {
        if let Partage::Graphe(variante) = partage {
            Self::batir_graphe(&store, variante);
        }
        Self {
            store,
            protocole,
            partage,
            etat: Mutex::new(HashMap::new()),
        }
    }

    fn batir_graphe(store: &Arc<ObrainStore>, variante: Variante) {
        let domaine_selon = |classe: &str| -> String {
            match variante {
                Variante::Normale => domaine_de(classe).to_string(),
                Variante::Melangee => format!("m{}", fnv1a(classe) % 3),
                Variante::Bruit => "bruit".to_string(),
            }
        };
        let mut toutes: Vec<(&'static str, &'static str)> = CLASSES.to_vec();
        for (c, d, _) in TROMPEUSES {
            toutes.push((c, d));
        }
        let mut s = store.db().session();
        s.begin_transaction().expect("begin");
        let mut domaines: HashMap<String, obrain_common::types::NodeId> = HashMap::new();
        for (classe, _) in toutes {
            let n = s.create_node(&["classe"]);
            s.set_node_property(n, "nom", Value::String(classe.into()));
            let dnom = domaine_selon(classe);
            let d = match domaines.get(&dnom) {
                Some(d) => *d,
                None => {
                    let dn = s.create_node(&["domaine"]);
                    s.set_node_property(dn, "nom", Value::String(dnom.as_str().into()));
                    domaines.insert(dnom, dn);
                    dn
                }
            };
            s.create_edge(n, d, "APPARTIENT");
        }
        s.commit().expect("commit");
    }

    /// Les classes du domaine de `classe`, lues par les relations du graphe.
    fn soeurs_du_graphe(&self, classe: &str) -> Vec<&'static str> {
        let r = self
            .store
            .execute_query(
                "gql",
                &format!(
                    "MATCH (c:classe)-[:APPARTIENT]->(d:domaine) WHERE c.nom = '{}' RETURN d.nom AS nom",
                    echapper(classe)
                ),
                10,
            )
            .expect("domaine");
        let rows = r
            .get("rows")
            .and_then(|v| v.as_array())
            .expect("réponse sans 'rows' pour le domaine");
        let domaine = rows
            .first()
            .and_then(|l| l.get(0))
            .and_then(|v| v.as_str())
            .expect("domaine du graphe introuvable")
            .to_string();
        let r = self
            .store
            .execute_query(
                "gql",
                &format!(
                    "MATCH (s:classe)-[:APPARTIENT]->(d:domaine) WHERE d.nom = '{}' RETURN s.nom AS nom",
                    echapper(&domaine)
                ),
                100_000,
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
            .filter_map(|s| statique(&s))
            .collect()
    }

    /// Priors du modèle hiérarchique à partage partiel.
    fn priors_hier3(
        &self,
        classe: &str,
        lignes: &[(String, String, bool)],
    ) -> HashMap<String, (f64, f64)> {
        let aut = autorisees(self.protocole, classe);
        let comptes = comptes_par_classe(lignes, aut.as_deref());
        let mut global: HashMap<String, (f64, f64)> = HashMap::new();
        let mut par_domaine: HashMap<(String, String), (f64, f64)> = HashMap::new();
        for ((c, p), (s, n)) in &comptes {
            let g = global.entry(p.clone()).or_insert((0.0, 0.0));
            g.0 += s;
            g.1 += n;
            let d = par_domaine
                .entry((domaine_de(c).to_string(), p.clone()))
                .or_insert((0.0, 0.0));
            d.0 += s;
            d.1 += n;
        }
        let mut priors: HashMap<String, (f64, f64)> = HashMap::new();
        for bras in BRAS {
            let (gs, gn) = global.get(bras).copied().unwrap_or((0.0, 0.0));
            let m0 = (gs + 1.0) / (gn + 2.0);
            let (ds, dn) = par_domaine
                .get(&(domaine_de(classe).to_string(), bras.to_string()))
                .copied()
                .unwrap_or((0.0, 0.0));
            let md = (ds + K0 * m0) / (dn + K0);
            let (cs, cn) = comptes
                .get(&(classe.to_string(), bras.to_string()))
                .copied()
                .unwrap_or((0.0, 0.0));
            let mc = (cs + KD * md) / (cn + KD);
            priors.insert(bras.to_string(), (1.0 + mc * W, 1.0 + (1.0 - mc) * W));
        }
        priors
    }

    /// Comptes du transfert (domaine ou graphe) et comptes locaux, séparément.
    fn comptes_locaux_et_transferes(
        &self,
        classe: &str,
        lignes: &[(String, String, bool)],
    ) -> (HashMap<String, (f64, f64)>, HashMap<String, (f64, f64)>) {
        let aut = autorisees(self.protocole, classe);
        let sienne = statique(classe).expect("classe connue");
        let local = comptes(lignes, Some(&[sienne]), None);
        let mut transfere = match self.partage {
            Partage::Graphe(_) => {
                let mut soeurs = self.soeurs_du_graphe(classe);
                if let Some(liste) = &aut {
                    soeurs.retain(|s| liste.contains(s));
                }
                comptes(lignes, Some(&soeurs), None)
            }
            _ => {
                let mut t = comptes(lignes, aut.as_deref(), Some(domaine_de(classe)));
                // Retire la contribution propre : le transfert vient des sœurs.
                if let Some((ls, ln)) = local.get(sienne) {
                    let e = t.entry(sienne.to_string()).or_insert((0.0, 0.0));
                    e.0 -= ls;
                    e.1 -= ln;
                }
                t
            }
        };
        transfere.retain(|_, (_, n)| *n > 0.0);
        (local, transfere)
    }

    fn priors_adaptatif(
        &self,
        classe: &str,
        lignes: &[(String, String, bool)],
    ) -> HashMap<String, (f64, f64)> {
        let (local, transfere) = self.comptes_locaux_et_transferes(classe, lignes);
        let etat = self.etat.lock().unwrap();
        let contradictions = etat.get(classe).map(|e| e.contradictions).unwrap_or(0);
        let lam = 1.0 / (1.0 + contradictions as f64);
        let t = priors_depuis(&transfere, lam, Some(MASSE_MAX));
        BRAS.iter()
            .map(|b| {
                let (la, lb) = local.get(*b).copied().unwrap_or((0.0, 0.0));
                let (ta, tb) = t.get(*b).copied().unwrap_or((1.0, 1.0));
                // (ta, tb) sont déjà des paramètres Beta : on les ajoute aux locaux.
                (
                    b.to_string(),
                    (1.0 + la + (ta - 1.0), 1.0 + lb + (tb - 1.0)),
                )
            })
            .collect()
    }

    /// Sélection adaptative de modèle : local seul ou domaine, au vu des observations
    /// de la classe (vraisemblance Bernoulli des bras réellement joués).
    fn priors_selection(
        &self,
        classe: &str,
        lignes: &[(String, String, bool)],
    ) -> HashMap<String, (f64, f64)> {
        let (local, transfere) = self.comptes_locaux_et_transferes(classe, lignes);
        let total_local: f64 = local.values().map(|(_, n)| n).sum();
        if total_local == 0.0 {
            // Aucune observation propre : la sélection n'a rien à comparer.
            return priors_depuis(&transfere, 1.0, Some(MASSE_MAX));
        }
        let p_local = priors_depuis(&local, 1.0, None);
        let p_domaine = priors_depuis(&transfere, 1.0, Some(MASSE_MAX));
        // Vraisemblance des deux a priori sur les observations RÉELLES de la classe.
        let mut ll_local = 0.0;
        let mut ll_domaine = 0.0;
        for (prov, (s, n)) in &local {
            let taux = s / n;
            let ml = p_local.get(prov).map(|(a, b)| a / (a + b)).unwrap_or(0.5);
            let md = p_domaine.get(prov).map(|(a, b)| a / (a + b)).unwrap_or(0.5);
            ll_local += n * (taux * ml.ln() + (1.0 - taux) * (1.0 - ml).ln());
            ll_domaine += n * (taux * md.ln() + (1.0 - taux) * (1.0 - md).ln());
        }
        if ll_domaine > ll_local {
            p_domaine
        } else {
            priors_depuis(&local, 1.0, None)
        }
    }
}

#[async_trait(?Send)]
impl Learning for Strategie {
    async fn arms(&self, classe: &str) -> Vec<ArmStats> {
        let lignes = lire_lignes(&self.store, 100_000);
        let aut = autorisees(self.protocole, classe);
        let sienne = statique(classe).expect("classe connue");
        let priors = match self.partage {
            Partage::Classe => comptes(&lignes, Some(&[sienne]), None)
                .iter()
                .map(|(p, (s, n))| (p.clone(), (1.0 + s, 1.0 + (n - s))))
                .collect(),
            Partage::Global => priors_depuis(&comptes(&lignes, aut.as_deref(), None), 1.0, None),
            Partage::Domaine | Partage::Graphe(_) => {
                let (local, transfere) = self.comptes_locaux_et_transferes(classe, &lignes);
                let mut tot = transfere;
                for (p, (s, n)) in local {
                    let e = tot.entry(p).or_insert((0.0, 0.0));
                    e.0 += s;
                    e.1 += n;
                }
                priors_depuis(&tot, 1.0, None)
            }
            Partage::Borne => {
                let (local, transfere) = self.comptes_locaux_et_transferes(classe, &lignes);
                let mut p = priors_depuis(&transfere, 1.0, Some(MASSE_MAX));
                for (prov, (s, n)) in local {
                    let e = p.entry(prov).or_insert((1.0, 1.0));
                    e.0 += s;
                    e.1 += n;
                }
                p
            }
            Partage::Hier3 => self.priors_hier3(classe, &lignes),
            Partage::Adaptatif => self.priors_adaptatif(classe, &lignes),
            Partage::Selection => self.priors_selection(classe, &lignes),
        };
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

        // Contradiction AVANT écriture : la prédiction transférée a-t-elle été démentie ?
        let mut contradiction = false;
        if self.partage == Partage::Adaptatif {
            let lignes = lire_lignes(&self.store, 100_000);
            let (_, transfere) = self.comptes_locaux_et_transferes(classe, &lignes);
            if let Some(predite) = transfere
                .iter()
                .max_by(|x, y| {
                    let tx = x.1.0 / x.1.1.max(1.0);
                    let ty = y.1.0 / y.1.1.max(1.0);
                    tx.total_cmp(&ty)
                })
                .map(|(prov, _)| prov.clone())
            {
                contradiction = (predite == p.provider_id) != favorable;
            }
        }

        ecrire_observation(&self.store, id, classe, &p.provider_id, &p.model, favorable);

        if contradiction {
            let mut etat = self.etat.lock().unwrap();
            etat.entry(classe.to_string()).or_default().contradictions += 1;
        }
    }
}

// ── Régimes ────────────────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum Regime {
    Sans,
    L2,
    L3,
    Global,
    Domaine,
    Hier3,
    Relat,
    RelatMelange,
    RelatBruit,
    Adaptatif,
    Borne,
    Selection,
}

const REGIMES: [(&str, Regime); 12] = [
    ("sans", Regime::Sans),
    ("l2", Regime::L2),
    ("l3", Regime::L3),
    ("global", Regime::Global),
    ("domaine", Regime::Domaine),
    ("hier3", Regime::Hier3),
    ("relat", Regime::Relat),
    ("relat_melange", Regime::RelatMelange),
    ("relat_bruit", Regime::RelatBruit),
    ("adaptatif", Regime::Adaptatif),
    ("borne", Regime::Borne),
    ("selection", Regime::Selection),
];

fn nom_regime(r: Regime) -> &'static str {
    REGIMES.iter().find(|(_, x)| *x == r).expect("régime").0
}

fn dir_unique(etiquette: &str) -> std::path::PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    static N: AtomicU64 = AtomicU64::new(0);
    let d = std::env::temp_dir().join(format!(
        "l4a_{etiquette}_{}_{}",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&d);
    d
}

type Nettoyage = Option<(std::path::PathBuf, Arc<ObrainStore>)>;

fn instancier(regime: Regime, protocole: Protocole) -> (Box<dyn Learning>, Nettoyage) {
    match regime {
        Regime::Sans => (Box::new(SansMemoire), None),
        Regime::L2 => (Box::new(ParClasseMemoire::new()), None),
        _ => {
            let d = dir_unique(nom_regime(regime));
            let store = ObrainStore::open(&d).expect("ouverture du store");
            let partage = match regime {
                Regime::L3 => Partage::Classe,
                Regime::Global => Partage::Global,
                Regime::Domaine => Partage::Domaine,
                Regime::Hier3 => Partage::Hier3,
                Regime::Borne => Partage::Borne,
                Regime::Adaptatif => Partage::Adaptatif,
                Regime::Selection => Partage::Selection,
                Regime::Relat => Partage::Graphe(Variante::Normale),
                Regime::RelatMelange => Partage::Graphe(Variante::Melangee),
                Regime::RelatBruit => Partage::Graphe(Variante::Bruit),
                Regime::Sans | Regime::L2 => unreachable!(),
            };
            let learning: Box<dyn Learning> =
                Box::new(Strategie::nouvelle(store.clone(), protocole, partage));
            (learning, Some((d, store)))
        }
    }
}

fn liberer(nettoyage: Nettoyage) {
    if let Some((d, store)) = nettoyage {
        store.commit_wal().expect("flush");
        drop(store);
        let _ = std::fs::remove_dir_all(&d);
    }
}

// ── Le conducteur (mêmes tirages pour toutes les stratégies) ────────────────

fn jouer(
    learning: &dyn Learning,
    rep: u64,
    classes: &[&'static str],
    tours: usize,
) -> Vec<Vec<(String, bool)>> {
    let mut resultat = Vec::new();
    for &classe in classes {
        let mut traj = Vec::new();
        let mut tirages_bras: HashMap<String, usize> = HashMap::new();
        for t in 0..tours {
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

// ── Mesures ────────────────────────────────────────────────────────────────

/// Réussite PONCTUELLE au tour `k` (décidé avec `k` observations préalables),
/// agrégée sur toutes les classes × répétitions.
#[derive(Default, Clone)]
struct Points {
    succes: [usize; POINTS.len()],
    total: [usize; POINTS.len()],
}

fn ajouter_points(a: &mut Points, traj: &[(String, bool)]) {
    for (i, k) in POINTS.iter().enumerate() {
        if let Some((_, ok)) = traj.get(*k) {
            a.total[i] += 1;
            if *ok {
                a.succes[i] += 1;
            }
        }
    }
}

fn taux_point(p: &Points, i: usize) -> f64 {
    if p.total[i] == 0 {
        0.0
    } else {
        p.succes[i] as f64 / p.total[i] as f64
    }
}

/// Récupération, mesurée PAR CLASSE ET PAR RÉPÉTITION.
#[derive(Clone, Copy, Default)]
struct Recup {
    premier_optimal: Option<usize>,
    /// Fréquence optimale sur les tours qui SUIVENT le premier choix optimal.
    freq_apres: Option<f64>,
    /// Fréquence optimale sur les 8 derniers tours.
    freq_fin: f64,
    regret: f64,
    /// Premier tour après lequel plus aucune décision n'est sous-optimale.
    durable: Option<usize>,
}

fn mesurer_recup(traj: &[(String, bool)], classe: &str, rep: u64) -> Recup {
    let meilleur = meilleur_bras(classe);
    let n = traj.len();
    let est_optimal = |i: usize| traj[i].0 == meilleur;

    let premier_optimal = (0..n).find(|i| est_optimal(*i));
    let freq_apres = premier_optimal.map(|p| {
        let suite: Vec<usize> = ((p + 1)..n).collect();
        if suite.is_empty() {
            0.0
        } else {
            suite.iter().filter(|i| est_optimal(**i)).count() as f64 / suite.len() as f64
        }
    });
    let debut_fin = n.saturating_sub(8);
    let freq_fin = if n == debut_fin {
        0.0
    } else {
        (debut_fin..n).filter(|i| est_optimal(*i)).count() as f64 / (n - debut_fin) as f64
    };
    let mut regret = 0.0;
    for (i, (bras, _)) in traj.iter().enumerate() {
        // Tirages pré-générés : le regret est celui de la décision, pas du hasard.
        let _ = i;
        regret += theta(classe, meilleur) - theta(classe, bras);
    }
    let durable = (0..n).find(|i| (*i..n).all(|j| est_optimal(j)));
    let _ = rep;
    Recup {
        premier_optimal,
        freq_apres,
        freq_fin,
        regret,
        durable,
    }
}

fn mediane(mut v: Vec<f64>) -> Option<f64> {
    if v.is_empty() {
        return None;
    }
    v.sort_by(|a, b| a.total_cmp(b));
    let n = v.len();
    Some(if n % 2 == 1 {
        v[n / 2]
    } else {
        (v[n / 2 - 1] + v[n / 2]) / 2.0
    })
}

fn moyenne(v: &[f64]) -> Option<f64> {
    if v.is_empty() {
        None
    } else {
        Some(v.iter().sum::<f64>() / v.len() as f64)
    }
}

fn fmt_opt(v: Option<f64>, chiffres: usize) -> String {
    match v {
        None => "—".to_string(),
        Some(x) => format!("{x:.chiffres$}"),
    }
}

// ── Le test ────────────────────────────────────────────────────────────────

#[test]
fn l4a_qualification_transfert_et_recuperation() {
    // ── Auto-contrôle du corpus ───────────────────────────────────────────
    assert_eq!(meilleur_bras("c0"), "a", "d0 : a domine");
    assert_eq!(meilleur_bras("c4"), "b", "d1 : b domine");
    assert_eq!(meilleur_bras("c5"), "c", "d2 : c domine");
    assert_eq!(
        meilleur_bras("x0"),
        "d",
        "x0 (d1) : d domine CONTRE son domaine"
    );
    assert_eq!(
        meilleur_bras("x1"),
        "a",
        "x1 (d2) : a domine CONTRE son domaine"
    );
    assert_ne!(meilleur_bras("x0"), meilleur_bras("c4"), "x0 contredit d1");

    let entrainement: Vec<&'static str> =
        CLASSES[..NB_ENTRAINEMENT].iter().map(|(c, _)| *c).collect();
    let inedites: Vec<&'static str> = CLASSES[NB_ENTRAINEMENT..].iter().map(|(c, _)| *c).collect();
    let trompeuses: Vec<&'static str> = TROMPEUSES.iter().map(|(c, _, _)| *c).collect();

    let mut fige_points: HashMap<Regime, Points> = HashMap::new();
    let mut fige_zero: HashMap<Regime, (usize, usize)> = HashMap::new();
    let mut continu_points: HashMap<Regime, Points> = HashMap::new();
    let mut recups: HashMap<Regime, Vec<Recup>> = HashMap::new();

    // INVARIANT 1 — trajectoires de `l2` et `l3`, répétition par répétition.
    let mut traj_l2: Vec<Vec<Vec<(String, bool)>>> = Vec::new();
    let mut traj_l3: Vec<Vec<Vec<(String, bool)>>> = Vec::new();
    // INVARIANT 2 — `domaine` et `relat` : mêmes trajectoires, deux protocoles.
    let mut traj_domaine: Vec<Vec<Vec<(String, bool)>>> = Vec::new();
    let mut traj_relat: Vec<Vec<Vec<(String, bool)>>> = Vec::new();

    for rep in 0..REPS {
        for (_, regime) in REGIMES {
            // ── FIGÉ ──────────────────────────────────────────────────────
            let (learning, nettoyage) = instancier(regime, Protocole::Fige);
            let _ = jouer(&*learning, rep, &entrainement, TOURS_ENTRAINEMENT);
            for &classe in &inedites {
                let bras = run(learning.arms(classe));
                let attendu = meilleur_bras(classe);
                let z = fige_zero.entry(regime).or_insert((0, 0));
                z.1 += 1;
                if choix_deterministe(&bras) == attendu {
                    z.0 += 1;
                }
            }
            let trajs = jouer(&*learning, rep, &inedites, TOURS_NOUVEAU);
            let pts = fige_points.entry(regime).or_default();
            for traj in &trajs {
                ajouter_points(pts, traj);
            }
            if regime == Regime::L2 {
                traj_l2.push(trajs.clone());
            }
            if regime == Regime::L3 {
                traj_l3.push(trajs.clone());
            }
            if regime == Regime::Domaine {
                traj_domaine.push(trajs.clone());
            }
            if regime == Regime::Relat {
                traj_relat.push(trajs.clone());
            }
            liberer(nettoyage);

            // ── CONTINU ───────────────────────────────────────────────────
            let (learning, nettoyage) = instancier(regime, Protocole::Continu);
            let _ = jouer(&*learning, rep, &entrainement, TOURS_ENTRAINEMENT);
            let trajs = jouer(&*learning, rep, &inedites, TOURS_NOUVEAU);
            let pts = continu_points.entry(regime).or_default();
            for traj in &trajs {
                ajouter_points(pts, traj);
            }
            if regime == Regime::Domaine {
                traj_domaine.push(trajs.clone());
            }
            if regime == Regime::Relat {
                traj_relat.push(trajs.clone());
            }
            liberer(nettoyage);

            // ── TROMPEUR (récupération, par classe et par répétition) ─────
            let (learning, nettoyage) = instancier(regime, Protocole::Fige);
            let _ = jouer(&*learning, rep, &entrainement, TOURS_ENTRAINEMENT);
            let trajs = jouer(&*learning, rep, &trompeuses, TOURS_TROMPEUR);
            let r = recups.entry(regime).or_default();
            for (i, traj) in trajs.iter().enumerate() {
                r.push(mesurer_recup(traj, trompeuses[i], rep));
            }
            liberer(nettoyage);
        }
    }

    // ── Rapport : protocole FIGÉ, réussite ponctuelle ─────────────────────
    println!("===== L4a · FIGÉ · réussite APRÈS exactement k observations =====");
    print!("  {: <14}", "régime");
    for k in POINTS {
        print!(" {: >7}", format!("k={k}"));
    }
    println!("   {: >10}", "pré-choix");
    for (nom, regime) in REGIMES {
        let p = &fige_points[&regime];
        let z = fige_zero[&regime];
        print!("  {nom: <14}");
        for i in 0..POINTS.len() {
            print!(" {: >7.3}", taux_point(p, i));
        }
        println!("   {: >10}", format!("{}/{}", z.0, z.1));
    }

    // ── Rapport : protocole CONTINU, réussite ponctuelle ──────────────────
    println!("===== L4a · CONTINU · réussite APRÈS exactement k observations =====");
    print!("  {: <14}", "régime");
    for k in POINTS {
        print!(" {: >7}", format!("k={k}"));
    }
    println!();
    for (nom, regime) in REGIMES {
        let p = &continu_points[&regime];
        print!("  {nom: <14}");
        for i in 0..POINTS.len() {
            print!(" {: >7.3}", taux_point(p, i));
        }
        println!();
    }

    // ── Rapport : corpus TROMPEUR, distributions de récupération ──────────
    println!(
        "===== L4a · TROMPEUR · récupération par classe × répétition ({} cas) =====",
        trompeuses.len() as u64 * REPS
    );
    println!(
        "  {: <14} {: >9} {: >10} {: >9} {: >9} {: >9} {: >12} {: >13}",
        "régime",
        "1er opt.",
        "opt. après",
        "opt. fin",
        "regret méd.",
        "regret max",
        "récup. durable",
        "sans récup."
    );
    for (nom, regime) in REGIMES {
        let r = &recups[&regime];
        let premiers: Vec<f64> = r
            .iter()
            .filter_map(|x| x.premier_optimal.map(|p| p as f64))
            .collect();
        let apres: Vec<f64> = r.iter().filter_map(|x| x.freq_apres).collect();
        let fins: Vec<f64> = r.iter().map(|x| x.freq_fin).collect();
        let regrets: Vec<f64> = r.iter().map(|x| x.regret).collect();
        let durables: Vec<f64> = r
            .iter()
            .filter_map(|x| x.durable.map(|d| d as f64))
            .collect();
        let sans_recup = r.iter().filter(|x| x.durable.is_none()).count();
        let max_regret = regrets.iter().copied().fold(f64::MIN, f64::max);
        let jamais = r.iter().filter(|x| x.premier_optimal.is_none()).count();
        println!(
            "  {nom: <14} {: >9} {: >10} {: >9} {: >9} {: >9} {: >12} {: >13}",
            format!("{} ({jamais} n.j.)", fmt_opt(mediane(premiers.clone()), 0)),
            fmt_opt(moyenne(&apres), 3),
            fmt_opt(moyenne(&fins), 3),
            fmt_opt(mediane(regrets.clone()), 2),
            format!("{max_regret:.2}"),
            format!("{}/{n}", durables.len(), n = r.len()),
            format!("{sans_recup}/{}", r.len())
        );
    }
    println!("==================================================================");

    // ── INVARIANTS (assertés) ─────────────────────────────────────────────
    assert_eq!(
        traj_l2.len() as u64,
        REPS,
        "toutes les répétitions comparées"
    );
    for (i, (a, b)) in traj_l2.iter().zip(traj_l3.iter()).enumerate() {
        assert_eq!(
            a, b,
            "INVARIANT : répétition {i} — L2 et L3 divergent (persister change la trajectoire)"
        );
    }
    assert_eq!(
        traj_domaine.len() as u64,
        2 * REPS,
        "les deux protocoles sont comparés pour l'invariant domaine ≡ relat"
    );
    for (i, (a, b)) in traj_domaine.iter().zip(traj_relat.iter()).enumerate() {
        assert_eq!(
            a, b,
            "INVARIANT : cas {i} — Beta/domaine et rappel relationnel divergent (même information, même partage ⇒ même trajectoire)"
        );
    }

    // ── HYPOTHÈSES (imprimées, jamais assertées) ──────────────────────────
    // Elles doivent pouvoir varier selon le corpus ; les figer en assertion
    // empêcherait la mesure de les contredire.
    let mut refutees = 0usize;
    macro_rules! hypothese {
        ($nom:expr, $cond:expr, $mesure:expr) => {{
            let tenue = $cond;
            if !tenue {
                refutees += 1;
            }
            println!(
                "  [{}] {} — {}",
                if tenue { "CONFIRMÉE" } else { "RÉFUTÉE  " },
                $nom,
                $mesure
            );
        }};
    }

    let p_fige = |r: Regime, i: usize| taux_point(&fige_points[&r], i);
    let regret_med =
        |r: Regime| mediane(recups[&r].iter().map(|x| x.regret).collect()).unwrap_or(f64::NAN);
    let sans_recup = |r: Regime| recups[&r].iter().filter(|x| x.durable.is_none()).count();

    println!("===== L4a · hypothèses (verdicts, aucune assertion) =====");
    hypothese!(
        "H1 transfert positif à 0 observation",
        p_fige(Regime::Relat, 0) > p_fige(Regime::L2, 0),
        format!(
            "relat {:.3} vs l2 {:.3} → {:+.3}",
            p_fige(Regime::Relat, 0),
            p_fige(Regime::L2, 0),
            p_fige(Regime::Relat, 0) - p_fige(Regime::L2, 0)
        )
    );
    hypothese!(
        "H2 le transfert rigide se paie quand la structure ment",
        regret_med(Regime::Relat) > regret_med(Regime::L2),
        format!(
            "regret médian relat {:.2} vs l2 {:.2} vs adaptatif {:.2}",
            regret_med(Regime::Relat),
            regret_med(Regime::L2),
            regret_med(Regime::Adaptatif)
        )
    );
    hypothese!(
        "H3 la confiance adaptative réduit le coût du transfert négatif",
        regret_med(Regime::Adaptatif) < regret_med(Regime::Relat),
        format!(
            "adaptatif {:.2} vs relat {:.2} (borne {:.2})",
            regret_med(Regime::Adaptatif),
            regret_med(Regime::Relat),
            regret_med(Regime::Borne)
        )
    );
    hypothese!(
        "H4 la confiance adaptative préserve le transfert positif",
        p_fige(Regime::Adaptatif, 0) >= p_fige(Regime::L2, 0),
        format!(
            "adaptatif {:.3} vs l2 {:.3} à 0 observation",
            p_fige(Regime::Adaptatif, 0),
            p_fige(Regime::L2, 0)
        )
    );
    hypothese!(
        "H5 une structure qui ment coûte plus cher qu'un bruit",
        regret_med(Regime::Relat) > regret_med(Regime::RelatBruit),
        format!(
            "relat {:.2} vs bruit {:.2} vs mélange {:.2}",
            regret_med(Regime::Relat),
            regret_med(Regime::RelatBruit),
            regret_med(Regime::RelatMelange)
        )
    );
    hypothese!(
        "H6 le partage partiel récupère plus souvent qu'un partage dur",
        sans_recup(Regime::Hier3) <= sans_recup(Regime::Relat),
        format!(
            "hier3 {}/{} sans récupération vs relat {}/{}",
            sans_recup(Regime::Hier3),
            recups[&Regime::Hier3].len(),
            sans_recup(Regime::Relat),
            recups[&Regime::Relat].len()
        )
    );
    println!("  → {refutees} hypothèse(s) réfutée(s) sur 6 (aucune n'invalide le test)");
}
