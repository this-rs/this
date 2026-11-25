#![cfg(all(feature = "obrain", feature = "objects-toolbox"))]
//! **L4a** — transfert par structure EXPLICITE, **qualifié à information identique**.
//!
//! Le premier passage (PR #386, `b615ccba`) comparait le rappel relationnel à un
//! témoin nommé « hiérarchique » qui ne réalisait en fait qu'une **mutualisation
//! globale** (brassage uniforme de toutes les classes). Ce n'est pas un modèle
//! hiérarchique multiniveau, et l'écart mesuré ne pouvait donc pas être attribué.
//!
//! Ce fichier refait la comparaison. Toutes les stratégies lisent la **même table
//! d'observations** (classe, fournisseur, favorable) : ce qui change n'est pas
//! l'information disponible, c'est la **structure de partage** que chacune est
//! autorisée à en tirer.
//!
//! | régime | structure de partage | information lue |
//! |---|---|---|
//! | `sans` | aucune | rien |
//! | `l2` | par classe, en mémoire | ses propres observations |
//! | `l3` | par classe, persistée dans Obrain | ses propres observations |
//! | `global` | mutualisation GLOBALE (ex-« hiérarchique », renommé) | tout, brassé |
//! | `domaine` | Beta par DOMAINE, câblé à la main | classe + ses sœurs |
//! | `hier3` | hiérarchique multiniveau à partage PARTIEL (global → domaine → classe) | tout, avec rétrécissement |
//! | `relat` | rappel relationnel natif Obrain (`(:classe)-[:APPARTIENT]->(:domaine)`) | classe + sœurs, découvertes par le graphe |
//! | `relat_melange` | idem, relations MÉLANGÉES (structure réelle, non prédictive) | — |
//! | `relat_bruit` | idem, relations NON PERTINENTES (un seul domaine) | — |
//!
//! Deux protocoles, rapportés séparément :
//!
//! * **figé** — les inédites sont évaluées depuis le MÊME état initial (les lectures
//!   excluent les autres inédites : `c7` ne voit jamais `c6`). C'est ce protocole qui
//!   traite la réserve c6/c7, sans toucher au corpus ;
//! * **continu** — les inédites sont rencontrées successivement, le transfert entre
//!   elles est permis et mesuré.
//!
//! Mesures : réussite AVANT toute observation de la classe (choix déterministe sur
//! l'espérance a posteriori, pas un coup de dés), puis par palier d'observations
//! (0, 1, 2, 4, 8). Et, sur un corpus TROMPEUR (structure similaire, meilleur bras
//! différent), le coût du **transfert négatif** et la vitesse de correction.
//!
//! Prédictions écrites AVANT la mesure — une prédiction ratée se rapporte, elle ne
//! s'efface pas :
//!
//! * P1 — `l2` et `l3` produisent la MÊME trajectoire, à CHAQUE répétition ;
//! * P2 — à 0 observation, `relat`/`domaine`/`hier3` choisissent mieux que `l2` ;
//! * P3 — sur le corpus trompeur, `relat` perd PLUS de tours que `l2` (le transfert
//!   se paie) puis se corrige ;
//! * P4 — **RÉFUTÉE** : « une structure mélangée ne fait pas mieux qu'une structure
//!   informative » est FAUX sur le corpus trompeur (192 tours perdus contre 232).
//!   Une structure qui **ment** coûte plus cher qu'une structure qui n'ordonne rien ;
//! * P5 — **CONFIRMÉE** : `domaine` (câblé à la main) et `relat` (lu dans le graphe)
//!   rendent des chiffres IDENTIQUES, palier par palier. À information identique et
//!   partage identique, le graphe n'ajoute aucune capacité de décision — il rend le
//!   partage **déclaratif** (il vit dans les données, pas dans le code).

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

/// Classes TROMPEUSES : la structure (domaine) suggère un bras, le vrai meilleur
/// bras est différent. `(nom, domaine affiché, meilleur bras réel)`.
const TROMPEUSES: [(&str, &str, &str); 2] = [
    ("x0", "d1", "d"), // d1 dit « b », la réalité est « d »
    ("x1", "d2", "a"), // d2 dit « c », la réalité est « a »
];

const TOURS_ENTRAINEMENT: usize = 60;
const TOURS_NOUVEAU: usize = 20;
const TOURS_TROMPEUR: usize = 40;
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

/// Rend la version `'static` d'un nom de classe connu (entraînement, inédite, trompeuse).
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

/// Le meilleur bras imposé d'une classe trompeuse, s'il y en a un.
fn meilleur_force(classe: &str) -> Option<&'static str> {
    TROMPEUSES
        .iter()
        .find(|(c, _, _)| *c == classe)
        .map(|(_, _, m)| *m)
}

/// Index stable de classe (sert à fabriquer l'identifiant de décision).
fn indice_classe(classe: &str) -> usize {
    if let Some(i) = CLASSES.iter().position(|(c, _)| *c == classe) {
        return i;
    }
    if let Some(i) = TROMPEUSES.iter().position(|(c, _, _)| *c == classe) {
        return CLASSES.len() + i;
    }
    panic!("classe inconnue : {classe}")
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
    let mut z = logit_global(bras) + boost_domaine(domaine_de(classe), bras) + gigue(classe, bras);
    if Some(bras) == meilleur_force(classe) {
        z += 4.0;
    }
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

/// Choix DÉTERMINISTE : argmax de l'espérance a posteriori. Sert à mesurer ce que
/// la stratégie « sait » avant tout tirage (aucune chance dans la mesure).
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

// ── Lectures : strictes (une erreur n'est jamais un résultat vide) ─────────

/// Lit toute la table d'observations. Refuse une réponse inexploitable ou tronquée.
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

fn ajouter_observation(
    table: &mut HashMap<(String, String), (f64, f64)>,
    classe: &str,
    provider: &str,
    favorable: bool,
) {
    let e = table
        .entry((classe.to_string(), provider.to_string()))
        .or_insert((1.0, 1.0));
    if favorable {
        e.0 += 1.0;
    } else {
        e.1 += 1.0;
    }
}

/// Agrégat par fournisseur, sur les classes autorisées et/ou un domaine.
fn agreger_par_provider(
    lignes: &[(String, String, bool)],
    autorisees: Option<&[&'static str]>,
    domaine_filtre: Option<&str>,
) -> HashMap<String, (f64, f64)> {
    let mut par_classe: HashMap<(String, String), (f64, f64)> = HashMap::new();
    for (classe, provider, favorable) in lignes {
        if let Some(liste) = autorisees
            && !liste.contains(&classe.as_str())
        {
            continue;
        }
        if let Some(d) = domaine_filtre
            && domaine_de(classe) != d
        {
            continue;
        }
        ajouter_observation(&mut par_classe, classe, provider, *favorable);
    }
    let mut par_provider: HashMap<String, (f64, f64)> = HashMap::new();
    for ((_, provider), (a, b)) in par_classe {
        let e = par_provider.entry(provider).or_insert((1.0, 1.0));
        e.0 += a - 1.0;
        e.1 += b - 1.0;
    }
    par_provider
}

/// Comptes bruts par (classe, fournisseur) — pour le modèle hiérarchique.
fn compter_par_classe(
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
        ajouter_observation(&mut table, classe, provider, *favorable);
    }
    table
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

/// Classes lisibles par une stratégie qui transfère (hors « sa propre classe »,
/// toujours lisible). `None` = tout est lisible (protocole continu).
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

// ── Stratégie `sans` : aucune mémoire ──────────────────────────────────────

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

// ── `l3` : Beta par classe, persisté dans Obrain ───────────────────────────
//
// L2 et L3 ne lisent que leur PROPRE classe : les deux protocoles leur sont donc
// indifférents. C'est exactement ce qui rend leur égalité de trajectoire exacte.

struct ParClasseObrain {
    store: Arc<ObrainStore>,
}

#[async_trait(?Send)]
impl Learning for ParClasseObrain {
    async fn arms(&self, classe: &str) -> Vec<ArmStats> {
        let lignes = lire_lignes(&self.store, 100_000);
        let sienne = [statique(classe).expect("classe connue")];
        let priors = agreger_par_provider(&lignes, Some(&sienne), None);
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

// ── `global` : mutualisation GLOBALE (ex-« hiérarchique », renommé) ────────

struct MutualisationGlobale {
    store: Arc<ObrainStore>,
    protocole: Protocole,
}

#[async_trait(?Send)]
impl Learning for MutualisationGlobale {
    async fn arms(&self, classe: &str) -> Vec<ArmStats> {
        let aut = autorisees(self.protocole, classe);
        let lignes = lire_lignes(&self.store, 100_000);
        let priors = agreger_par_provider(&lignes, aut.as_deref(), None);
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

// ── `domaine` : Beta par domaine, câblé à la main (même information qu'Obrain) ──

struct BetaDomaine {
    store: Arc<ObrainStore>,
    protocole: Protocole,
}

#[async_trait(?Send)]
impl Learning for BetaDomaine {
    async fn arms(&self, classe: &str) -> Vec<ArmStats> {
        let aut = autorisees(self.protocole, classe);
        let lignes = lire_lignes(&self.store, 100_000);
        let priors = agreger_par_provider(&lignes, aut.as_deref(), Some(domaine_de(classe)));
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

// ── `hier3` : hiérarchique multiniveau à partage PARTIEL ──────────────────
//
// Beta-Bernoulli hiérarchique à estimateur fermé (rétrécissement type James-Stein) :
//   m0(bras)         = moyenne GLOBALE du bras
//   md(domaine, bras)= moyenne du domaine, rétrécie vers m0        (poids K0)
//   mc(classe, bras) = moyenne de la classe, rétrécie vers md      (poids KD)
// puis un Beta(mc * W, (1-mc) * W) : c'est un partage PARTIEL, pas un brassage.

const K0: f64 = 4.0;
const KD: f64 = 4.0;
const W: f64 = 4.0;

struct Hierarchique3Niveaux {
    store: Arc<ObrainStore>,
    protocole: Protocole,
}

#[async_trait(?Send)]
impl Learning for Hierarchique3Niveaux {
    async fn arms(&self, classe: &str) -> Vec<ArmStats> {
        let aut = autorisees(self.protocole, classe);
        let lignes = lire_lignes(&self.store, 100_000);
        let comptes = compter_par_classe(&lignes, aut.as_deref());

        let mut global: HashMap<String, (f64, f64)> = HashMap::new(); // succès, essais
        let mut par_domaine: HashMap<(String, String), (f64, f64)> = HashMap::new();
        for ((c, p), (a, b)) in &comptes {
            let (s, n) = (a - 1.0, (a - 1.0) + (b - 1.0));
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
                .map(|(a, b)| (a - 1.0, (a - 1.0) + (b - 1.0)))
                .unwrap_or((0.0, 0.0));
            let mc = (cs + KD * md) / (cn + KD);
            priors.insert(bras.to_string(), (1.0 + mc * W, 1.0 + (1.0 - mc) * W));
        }
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

// ── `relat` : rappel relationnel natif Obrain (et ses variantes trompeuses) ─

#[derive(Clone, Copy, PartialEq, Eq)]
enum Variante {
    /// Le domaine réel, porté par le graphe.
    Normale,
    /// Relations MÉLANGÉES : le graphe existe, il n'est pas prédictif.
    Melangee,
    /// Relations NON PERTINENTES : un seul domaine pour tout le monde.
    Bruit,
}

struct Relationnel {
    store: Arc<ObrainStore>,
    protocole: Protocole,
}

impl Relationnel {
    /// Bâtit le graphe de structure : classes, domaines, arêtes APPARTIENT.
    fn nouveau(store: Arc<ObrainStore>, variante: Variante, protocole: Protocole) -> Self {
        let domaine_selon = |classe: &str| -> String {
            match variante {
                Variante::Normale => domaine_de(classe).to_string(),
                Variante::Melangee => format!("m{}", fnv1a(classe) % 3),
                Variante::Bruit => "bruit".to_string(),
            }
        };
        {
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
        Self { store, protocole }
    }

    /// Les classes du domaine de `classe`, par relations du graphe.
    fn classes_du_domaine(&self, classe: &str) -> Vec<String> {
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
            .collect()
    }
}

#[async_trait(?Send)]
impl Learning for Relationnel {
    async fn arms(&self, classe: &str) -> Vec<ArmStats> {
        let soeurs = self.classes_du_domaine(classe);
        let aut = autorisees(self.protocole, classe);
        let mut lisibles: Vec<&'static str> = soeurs.iter().filter_map(|s| statique(s)).collect();
        if let Some(liste) = &aut {
            lisibles.retain(|s| liste.contains(s));
        }
        if let Some(s) = statique(classe)
            && !lisibles.contains(&s)
        {
            lisibles.push(s);
        }
        let lignes = lire_lignes(&self.store, 100_000);
        let priors = agreger_par_provider(&lignes, Some(&lisibles), None);
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
}

const REGIMES: [(&str, Regime); 9] = [
    ("sans", Regime::Sans),
    ("l2", Regime::L2),
    ("l3", Regime::L3),
    ("global", Regime::Global),
    ("domaine", Regime::Domaine),
    ("hier3", Regime::Hier3),
    ("relat", Regime::Relat),
    ("relat_melange", Regime::RelatMelange),
    ("relat_bruit", Regime::RelatBruit),
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

/// Instancie un régime. Rend le store à libérer s'il y en a un.
fn instancier(regime: Regime, protocole: Protocole) -> (Box<dyn Learning>, Nettoyage) {
    match regime {
        Regime::Sans => (Box::new(SansMemoire), None),
        Regime::L2 => (Box::new(ParClasseMemoire::new()), None),
        _ => {
            let d = dir_unique(nom_regime(regime));
            let store = ObrainStore::open(&d).expect("ouverture du store");
            let learning: Box<dyn Learning> = match regime {
                Regime::L3 => Box::new(ParClasseObrain {
                    store: store.clone(),
                }),
                Regime::Global => Box::new(MutualisationGlobale {
                    store: store.clone(),
                    protocole,
                }),
                Regime::Domaine => Box::new(BetaDomaine {
                    store: store.clone(),
                    protocole,
                }),
                Regime::Hier3 => Box::new(Hierarchique3Niveaux {
                    store: store.clone(),
                    protocole,
                }),
                Regime::Relat => Box::new(Relationnel::nouveau(
                    store.clone(),
                    Variante::Normale,
                    protocole,
                )),
                Regime::RelatMelange => Box::new(Relationnel::nouveau(
                    store.clone(),
                    Variante::Melangee,
                    protocole,
                )),
                Regime::RelatBruit => Box::new(Relationnel::nouveau(
                    store.clone(),
                    Variante::Bruit,
                    protocole,
                )),
                Regime::Sans | Regime::L2 => unreachable!(),
            };
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

// ── Le conducteur (mêmes tirages, mêmes décisions pour toutes les stratégies) ─

/// Joue `classes` (dans l'ordre) et rend, par classe, la trajectoire (bras, succès).
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

/// Palier d'observation : 0, 1, 2, 4, 8 (borne basse du bloc).
fn palier(nb_obs: usize) -> usize {
    match nb_obs {
        0 => 0,
        1 => 1,
        2..=3 => 2,
        4..=7 => 3,
        _ => 4,
    }
}

#[derive(Default, Clone)]
struct Echelle {
    succes: [usize; 5],
    total: [usize; 5],
}

fn taux(e: &Echelle, i: usize) -> f64 {
    if e.total[i] == 0 {
        0.0
    } else {
        e.succes[i] as f64 / e.total[i] as f64
    }
}

fn echelle_de(traj: &[(String, bool)]) -> Echelle {
    let mut e = Echelle::default();
    for (i, (_, ok)) in traj.iter().enumerate() {
        let p = palier(i);
        e.total[p] += 1;
        if *ok {
            e.succes[p] += 1;
        }
    }
    e
}

fn ajouter_echelle(a: &mut Echelle, b: &Echelle) {
    for i in 0..5 {
        a.succes[i] += b.succes[i];
        a.total[i] += b.total[i];
    }
}

/// Tours où le bras joué n'était PAS le meilleur, et rang du premier tour correct.
fn cout_trompeur(traj: &[(String, bool)], meilleur: &str) -> (usize, Option<usize>) {
    let perdus = traj.iter().filter(|(b, _)| b != meilleur).count();
    let premier = traj.iter().position(|(b, _)| b == meilleur);
    (perdus, premier)
}

// ── Le test ────────────────────────────────────────────────────────────────

#[test]
fn l4a_qualification_a_information_identique() {
    // ── Auto-contrôle du corpus ───────────────────────────────────────────
    assert_eq!(meilleur_bras("c0"), "a", "d0 : a domine");
    assert_eq!(meilleur_bras("c4"), "b", "d1 : b domine");
    assert_eq!(meilleur_bras("c5"), "c", "d2 : c domine");
    assert_eq!(meilleur_bras("c6"), "a", "inédite de d0 : a domine");
    assert_eq!(meilleur_bras("c8"), "b", "inédite de d1 : b domine");
    assert_eq!(meilleur_bras("c9"), "c", "inédite de d2 : c domine");
    // Trompeuses : la structure dit une chose, la réalité en dit une autre.
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

    let mut fige_echelle: HashMap<Regime, Echelle> = HashMap::new();
    let mut fige_zero: HashMap<Regime, (usize, usize)> = HashMap::new();
    let mut continu_echelle: HashMap<Regime, Echelle> = HashMap::new();
    let mut trompeur: HashMap<Regime, (usize, usize, Option<usize>)> = HashMap::new();

    // P1 — trajectoires de `l2` et `l3`, répétition par répétition.
    let mut traj_l2: Vec<Vec<Vec<(String, bool)>>> = Vec::new();
    let mut traj_l3: Vec<Vec<Vec<(String, bool)>>> = Vec::new();

    for rep in 0..REPS {
        for (_, regime) in REGIMES {
            // ── FIGÉ ──────────────────────────────────────────────────────
            let (learning, nettoyage) = instancier(regime, Protocole::Fige);
            let _ = jouer(&*learning, rep, &entrainement, TOURS_ENTRAINEMENT);
            // 0 observation : ce que la stratégie sait avant tout tirage.
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
            let e = fige_echelle.entry(regime).or_default();
            for traj in &trajs {
                ajouter_echelle(e, &echelle_de(traj));
            }
            if regime == Regime::L2 {
                traj_l2.push(trajs.clone());
            }
            if regime == Regime::L3 {
                traj_l3.push(trajs.clone());
            }
            liberer(nettoyage);

            // ── CONTINU ───────────────────────────────────────────────────
            let (learning, nettoyage) = instancier(regime, Protocole::Continu);
            let _ = jouer(&*learning, rep, &entrainement, TOURS_ENTRAINEMENT);
            let trajs = jouer(&*learning, rep, &inedites, TOURS_NOUVEAU);
            let e = continu_echelle.entry(regime).or_default();
            for traj in &trajs {
                ajouter_echelle(e, &echelle_de(traj));
            }
            liberer(nettoyage);

            // ── TROMPEUR ──────────────────────────────────────────────────
            let (learning, nettoyage) = instancier(regime, Protocole::Fige);
            let _ = jouer(&*learning, rep, &entrainement, TOURS_ENTRAINEMENT);
            let trajs = jouer(&*learning, rep, &trompeuses, TOURS_TROMPEUR);
            let t = trompeur.entry(regime).or_insert((0, 0, None));
            for (i, traj) in trajs.iter().enumerate() {
                let (perdus, premier) = cout_trompeur(traj, meilleur_bras(trompeuses[i]));
                t.0 += perdus;
                t.1 += traj.len();
                if let Some(p) = premier {
                    t.2 = Some(t.2.map_or(p, |m: usize| m.min(p)));
                }
            }
            liberer(nettoyage);
        }
    }

    // ── Rapport : protocole FIGÉ ──────────────────────────────────────────
    println!("===== L4a · protocole FIGÉ (même état initial pour chaque inédite) =====");
    println!(
        "  {: <14} {: >7} {: >7} {: >7} {: >7} {: >7}   {: >12}",
        "régime", "0 obs", "1 obs", "2 obs", "4 obs", "8 obs", "avant tout"
    );
    for (nom, regime) in REGIMES {
        let e = &fige_echelle[&regime];
        let z = fige_zero[&regime];
        println!(
            "  {: <14} {: >7.3} {: >7.3} {: >7.3} {: >7.3} {: >7.3}   {: >12}",
            nom,
            taux(e, 0),
            taux(e, 1),
            taux(e, 2),
            taux(e, 3),
            taux(e, 4),
            format!("{}/{}", z.0, z.1)
        );
    }

    // ── Rapport : protocole CONTINU ───────────────────────────────────────
    println!("===== L4a · protocole CONTINU (les inédites se voient) =====");
    println!(
        "  {: <14} {: >7} {: >7} {: >7} {: >7} {: >7}",
        "régime", "0 obs", "1 obs", "2 obs", "4 obs", "8 obs"
    );
    for (nom, regime) in REGIMES {
        let e = &continu_echelle[&regime];
        println!(
            "  {: <14} {: >7.3} {: >7.3} {: >7.3} {: >7.3} {: >7.3}",
            nom,
            taux(e, 0),
            taux(e, 1),
            taux(e, 2),
            taux(e, 3),
            taux(e, 4)
        );
    }

    // ── Rapport : corpus TROMPEUR ─────────────────────────────────────────
    println!("===== L4a · corpus TROMPEUR (structure similaire, meilleur bras différent) =====");
    println!(
        "  {: <14} {: >10} {: >7} {: >14}",
        "régime", "tours perdus", "sur", "1er bon choix"
    );
    for (nom, regime) in REGIMES {
        let (perdus, total, premier) = trompeur[&regime];
        println!(
            "  {: <14} {: >10} {: >7} {: >14}",
            nom,
            perdus,
            total,
            premier.map_or("jamais".to_string(), |p| format!("tour {p}"))
        );
    }
    println!("========================================================================");

    // ── P1 : `l2` et `l3` sont la MÊME stratégie, à CHAQUE répétition ─────
    for (i, (a, b)) in traj_l2.iter().zip(traj_l3.iter()).enumerate() {
        assert_eq!(
            a, b,
            "répétition {i} : L2 et L3 divergent (la persistance change la trajectoire)"
        );
    }
    assert_eq!(
        traj_l2.len() as u64,
        REPS,
        "toutes les répétitions sont comparées"
    );

    // ── P2 : le transfert existe à 0 observation ──────────────────────────
    let z_l2 = fige_zero[&Regime::L2];
    let z_relat = fige_zero[&Regime::Relat];
    assert!(
        z_relat.0 > z_l2.0,
        "à 0 observation, le rappel relationnel doit choisir mieux que L2 : {}/{} vs {}/{}",
        z_relat.0,
        z_relat.1,
        z_l2.0,
        z_l2.1
    );

    // ── P3 : le transfert NÉGATIF coûte, et il se corrige ─────────────────
    let (perdus_l2, _, _) = trompeur[&Regime::L2];
    let (perdus_relat, _, premier_relat) = trompeur[&Regime::Relat];
    assert!(
        perdus_relat > perdus_l2,
        "sur un corpus trompeur, le transfert doit COÛTER : relationnel {} tours perdus vs L2 {}",
        perdus_relat,
        perdus_l2
    );
    assert!(
        premier_relat.is_some(),
        "le transfert négatif doit être corrigeable : le meilleur bras est-il joué au moins une fois ?"
    );

    // ── P4 (RÉFUTÉE) : la conclusion assertée est celle qui a été MESURÉE ──
    // Prédiction écrite avant la mesure : « une structure mélangée ne fait pas mieux
    // qu'une structure informative ». Faux : 192 tours perdus (mélange) contre 232
    // (relationnel). Une structure qui MENT coûte donc plus cher qu'une structure
    // qui n'ordonne rien — la prédiction était trop optimiste sur le bruit.
    let (perdus_melange, _, _) = trompeur[&Regime::RelatMelange];
    let (perdus_bruit, _, _) = trompeur[&Regime::RelatBruit];
    assert!(
        perdus_relat > perdus_bruit && perdus_relat > perdus_melange,
        "une structure qui MENT doit coûter plus cher qu'un domaine unique ({} vs bruit {}) ou qu'un mélange ({}), sinon le transfert négatif n'est pas mesuré",
        perdus_relat,
        perdus_bruit,
        perdus_melange
    );
    // Et sur le corpus INFORMATIF, la structure vraie bat le mélange à 0 observation.
    let z_melange = fige_zero[&Regime::RelatMelange];
    assert!(
        z_relat.0 > z_melange.0,
        "sur un corpus informatif, la structure vraie doit battre une structure mélangée : {}/{} vs {}/{}",
        z_relat.0,
        z_relat.1,
        z_melange.0,
        z_melange.1
    );

    // ── P5 : le graphe n'ajoute pas de capacité, il rend le partage déclaratif ──
    // `domaine` et `relat` lisent la même information et partagent le même domaine :
    // leurs trajectoires doivent être IDENTIQUES, pas « proches ». Un écart signalerait
    // que l'un des deux ne fait pas ce qu'il prétend.
    let e_dom = &fige_echelle[&Regime::Domaine];
    let e_rel = &fige_echelle[&Regime::Relat];
    assert_eq!(
        e_dom.succes, e_rel.succes,
        "Beta/domaine et rappel relationnel doivent réussir exactement autant (succès par palier : {:?} vs {:?})",
        e_dom.succes, e_rel.succes
    );
    assert_eq!(
        e_dom.total, e_rel.total,
        "Beta/domaine et rappel relationnel doivent décider exactement autant"
    );
}
