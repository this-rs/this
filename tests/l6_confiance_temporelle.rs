#![cfg(all(feature = "obrain", feature = "objects-toolbox"))]
//! **L6** — confiance absolue et crédit temporel du transfert.
//!
//! ## La question, et ce qu'elle ajoute à `l5_transfert_adaptatif`
//!
//! `l5` compare quatre politiques (A local, B global, C relationnel à poids fixe,
//! D relationnel à poids appris par crédit cumulé) et montre que D sait *attribuer*
//! la masse transférée à la relation qui prédit. Le crédit de D est **cumulé** :
//! `q = Σ(ll − ln ½) / (n + SHRINK)`. Deux limites, mesurées dans `l5` :
//!
//! 1. **l'inertie** : un crédit sans oubli moyenne les régimes successifs. Quand la
//!    pertinence bascule (S3), D revient vers l'uniforme — donc vers C — exactement
//!    au moment où il faudrait trancher vite (`l5` H9).
//! 2. **l'absence de récupération** : la confiance λ = 1/(1+contradictions) de `l4a`
//!    est **monotone décroissante** — une relation discréditée le reste à jamais.
//!
//! Ici on pose la question suivante : *une confiance qui se répare* — un **crédit
//! temporel à oubli** — fait-elle mieux qu'une confiance **absolue** qui ne se répare
//! pas ? Et l'échelle de temps de l'oubli (rapide / lent) peut-elle être choisie par
//! le système lui-même plutôt que fixée ?
//!
//! ## Les huit politiques
//!
//! | | transfert | pondération des relations |
//! |---|---|---|
//! | **A** | aucun — mémoire locale de la classe seule | — |
//! | **B** | toutes les observations des autres classes, brassées | plafonnée |
//! | **C** | voisinages des relations, poids fixe 1 par relation | plafonnée |
//! | **D** | idem, poids **crédit cumulé** (softmax) | plafonnée |
//! | **E** | idem, poids **confiance absolue λ** = 1/(1+contradictions) | plafonnée |
//! | **F** | idem, poids **crédit temporel multi-horizons** (moyenne fixe) | plafonnée |
//! | **G** | idem, poids **λ ⊙ temporel** (confiance × crédit) | plafonnée |
//! | **H** | idem, poids **combinaison adaptative** des horizons (hedge) | plafonnée |
//!
//! **A/B/C/D sont des témoins** : A et B bornent l'effet du transfert, C est la
//! référence « transfert aveugle », D est le crédit cumulé que `l5` a déjà mesuré.
//! E/F/G/H sont les variantes nouvelles. Toutes les politiques relationnelles (C..H)
//! transfèrent **exactement la même masse** que C (normalisée à `n_rel`, puis
//! renormalisée sur la masse de C) : seule la *répartition* entre relations change.
//! C'est ce qui rend les comparaisons C..H interprétables.
//!
//! ## Les trois mises à jour nouvelles
//!
//! Le signal instantané est celui de D : pour le bras joué, l'excès de
//! log-vraisemblance du voisinage de la relation sur le hasard,
//!
//! ```text
//! δ = [ y·ln p + (1−y)·ln(1−p) ] − ln(1/2),   p = taux du voisinage pour ce bras
//! ```
//!
//! * **E (λ)** : `contradictions += 1` quand δ < 0 ; `λ = 1/(1+contradictions)`.
//!   C'est la confiance **absolue** de `l4a`, portée **par relation**. Monotone
//!   décroissante : une relation discréditée ne se répare pas.
//! * **F (temporel)** : `q_h(t+1) = ρ_h·q_h(t) + (1−ρ_h)·δ` pour chaque horizon
//!   `h ∈ {rapide (ρ=0.6), lent (ρ=0.95)}` ; le poids combine les horizons par une
//!   moyenne **fixe** `q = (q_rapide + q_lent)/2`.
//! * **G (λ + temporel)** : la même récursion temporelle, et le poids est
//!   `λ · q` — la confiance absolue **module** le crédit temporel.
//! * **H (adaptatif)** : la même récursion temporelle, mais la combinaison des
//!   horizons est **apprise** par un *hedge* exponentiel : chaque horizon porte une
//!   perte cumulée `L_h += (q_h − δ)²`, et son poids vaut
//!   `α_h ∝ exp(−η·L_h)` ; `q = Σ_h α_h·q_h`. Un régime stationnaire favorise le
//!   lent (stable), un régime qui bascule favorise le rapide (réactif).
//!
//! À état vide, toutes les pondérations valent l'uniforme : E/F/G/H démarrent
//! exactement comme C, terme pour terme (asserté au tour 0).
//!
//! ## Les sept mondes exigés (+ un témoin)
//!
//! Le monde est une table θ(classe, bras) **explicite** en trois phases ; le corpus
//! *est* la table, donc auditable.
//!
//! | situation | relations | construction |
//! |---|---|---|
//! | relation utile | `S1` | une seule relation `un`, dont chaque voisinage partage le meilleur bras |
//! | trompeuse unique | `S2` | une seule relation `deux`, dont **aucun** voisin ne partage le meilleur bras |
//! | pertinence changeante | `S3` | `un` et `deux` ; en phase 1 les rôles **s'inversent** |
//! | concurrentes | `S4` | `un` utile et `deux` trompeuse, monde figé en phase 0 |
//! | plusieurs trompeuses | `S5` | `deux` et `trois`, **toutes deux** trompeuses en phase 0 |
//! | redondantes | `S6` | `un` et `quatre` **identiques** (mêmes voisinages) |
//! | capacités inédites | `S7` | `un` et `deux` ; en phase 2 les meilleurs bras changent de telle sorte que **les deux** relations deviennent périmées |
//! | absence de relation | `S8` | aucune relation dans le graphe (témoin) |
//!
//! `S5` isole l'effet d'un **double** transfert négatif ; `S6` vérifie que la
//! redondance n'est ni comptée deux fois ni diluée ; `S7` (≠ `S3`) ne propose
//! **aucune** relation de rechange : il faut *lâcher* le transfert et revenir au
//! local — c'est l'épreuve inverse de la bascule.
//!
//! ## Invariants assertés / hypothèses imprimées
//!
//! Même doctrine que `l4a`/`l5` : on **asserte** la forme (une violation est un défaut
//! de code ou de corpus), on **imprime avec verdict** les hypothèses scientifiques
//! (une réfutation est un résultat, jamais une panne).
//!
//! *Asserté* : forme du corpus (meilleur bras unique et margé dans les trois phases ;
//! `un` homogène en phase 0 ; `deux` et `trois` systématiquement hétérogènes en
//! phase 0 ; l'inverse en phase 1 pour `un`/`deux` ; en phase 2 les deux relations
//! sont hétérogènes) ; **A identique dans tous les mondes figés** ; **S8 : toutes les
//! politiques relationnelles ≡ A** ; **une seule relation ⇒ toutes les politiques
//! relationnelles ≡ C** ; **S6 (redondantes) ⇒ toutes les politiques relationnelles
//! ≡ C** ; **tour 0 : E/F/G/H choisissent comme C** (état vide ⇒ uniforme).
//!
//! *Imprimé avec verdict, jamais asserté* : le transfert positif/négatif (témoins),
//! le gain d'attribution, la récupération (temporel contre cumulé et absolu), le
//! repli sur le local quand aucune relation n'aide, et l'effet de la redondance.
//! Le résultat peut être négatif : le rapport le dit.
//!
//! ## Bornes et coût
//!
//! 10 classes × 24 tours × 3 graines × 8 politiques × 8 scénarios = 46 080 tours,
//! un magasin Obrain par (scénario, politique, graine) — 192 magasins, tous détruits
//! en fin de test. Le **coût réel** est mesuré (temps écoulé par politique), pas
//! supposé : le crédit temporel et le hedge ne lisent **rien** de plus que D (même
//! nombre de lectures GQL par tour) — c'est vérifié par construction, la seule
//! différence est du calcul en mémoire.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use futures::executor::block_on;
use obrain_common::types::Value;
use this::storage::obrain::ObrainStore;
use this_agent::Stop;
use this_harness::routing::{ArmKey, ArmStats, Learning, Pick, beta_draw, seed_of};

// ════════════════════════════════════════════════════════════════════════════
// 1. Le monde — une table explicite, trois phases
// ════════════════════════════════════════════════════════════════════════════

const BRAS: [&str; 4] = ["a", "b", "c", "d"];
const MODELE: &str = "m";
const CLASSES: [&str; 10] = ["c0", "c1", "c2", "c3", "c4", "c5", "c6", "c7", "c8", "c9"];

/// θ(classe, bras) en **phase 0**. Chaque ligne a un maximum net (marge ≥ 0,4).
static PHASE0: [[f64; 4]; 10] = [
    [0.85, 0.30, 0.25, 0.20], // c0 — a
    [0.82, 0.32, 0.28, 0.18], // c1 — a
    [0.88, 0.25, 0.30, 0.22], // c2 — a
    [0.80, 0.28, 0.26, 0.24], // c3 — a
    [0.30, 0.85, 0.28, 0.20], // c4 — b
    [0.25, 0.82, 0.32, 0.22], // c5 — b
    [0.28, 0.26, 0.86, 0.20], // c6 — c
    [0.30, 0.32, 0.83, 0.18], // c7 — c
    [0.25, 0.28, 0.22, 0.85], // c8 — d
    [0.28, 0.30, 0.24, 0.82], // c9 — d
];

/// θ(classe, bras) en **phase 1** (`S3` seulement) : les rôles s'inversent, `un`
/// devient trompeuse et `deux` utile.
static PHASE1: [[f64; 4]; 10] = [
    [0.28, 0.85, 0.30, 0.20], // c0 — b
    [0.27, 0.26, 0.85, 0.22], // c1 — c
    [0.30, 0.24, 0.26, 0.85], // c2 — d
    [0.85, 0.27, 0.25, 0.23], // c3 — a
    [0.85, 0.28, 0.26, 0.22], // c4 — a
    [0.26, 0.85, 0.30, 0.24], // c5 — b
    [0.28, 0.25, 0.85, 0.21], // c6 — c
    [0.26, 0.30, 0.23, 0.85], // c7 — d
    [0.28, 0.85, 0.24, 0.22], // c8 — b
    [0.30, 0.26, 0.85, 0.20], // c9 — c
];

/// θ(classe, bras) en **phase 2** (`S7` seulement) : chaque classe reçoit un
/// meilleur bras **inédit** (souvent jamais vu pour elle), choisi de sorte que les
/// cliques de `un` ET les groupes de `deux` deviennent hétérogènes — les deux
/// relations sont périmées, aucune ne prédit la nouvelle capacité.
static PHASE2: [[f64; 4]; 10] = [
    [0.85, 0.28, 0.25, 0.22], // c0 — a (inchangé)
    [0.28, 0.85, 0.26, 0.22], // c1 — b (inédit)
    [0.28, 0.24, 0.85, 0.22], // c2 — c (inédit)
    [0.24, 0.26, 0.22, 0.85], // c3 — d (inédit)
    [0.85, 0.28, 0.26, 0.22], // c4 — a (inédit)
    [0.28, 0.85, 0.24, 0.22], // c5 — b (inchangé)
    [0.85, 0.28, 0.26, 0.22], // c6 — a (inédit)
    [0.28, 0.85, 0.24, 0.22], // c7 — b (inédit)
    [0.26, 0.24, 0.85, 0.22], // c8 — c (inédit)
    [0.26, 0.24, 0.85, 0.22], // c9 — c (inédit)
];

/// Relation `un` : les classes de « a » forment une clique, idem b et c ; `c8` et
/// `c9` sont **isolées** — le cas « pas de relation pour cette classe » existe aussi
/// dans les scénarios relationnels.
static REL_UN: [(&str, &[&str]); 10] = [
    ("c0", &["c1", "c2", "c3"]),
    ("c1", &["c0", "c2", "c3"]),
    ("c2", &["c0", "c1", "c3"]),
    ("c3", &["c0", "c1", "c2"]),
    ("c4", &["c5"]),
    ("c5", &["c4"]),
    ("c6", &["c7"]),
    ("c7", &["c6"]),
    ("c8", &[]),
    ("c9", &[]),
];

/// Relation `deux` : les groupes de la phase 1 — trompeuse en phase 0, honnête en
/// phase 1.
static REL_DEUX: [(&str, &[&str]); 10] = [
    ("c0", &["c5", "c8"]),
    ("c1", &["c6", "c9"]),
    ("c2", &["c7"]),
    ("c3", &["c4"]),
    ("c4", &["c3"]),
    ("c5", &["c0", "c8"]),
    ("c6", &["c1", "c9"]),
    ("c7", &["c2"]),
    ("c8", &["c0", "c5"]),
    ("c9", &["c1", "c6"]),
];

/// Relation `trois` : une **deuxième** relation trompeuse en phase 0 (croise les
/// groupes d'une autre façon que `deux`). Toute classe a au moins un voisin, aucun
/// ne partage son meilleur bras.
static REL_TROIS: [(&str, &[&str]); 10] = [
    ("c0", &["c4", "c8"]),
    ("c1", &["c5", "c9"]),
    ("c2", &["c6"]),
    ("c3", &["c7"]),
    ("c4", &["c0"]),
    ("c5", &["c1"]),
    ("c6", &["c2"]),
    ("c7", &["c3"]),
    ("c8", &["c0"]),
    ("c9", &["c1"]),
];

const MASSE_MAX: f64 = 6.0;
const SHRINK: f64 = 6.0;
const TEMP: f64 = 3.0;
const TOURS: usize = 24;
const T_BASCULE: usize = 12;
const GRAINES: [u64; 3] = [0, 1, 2];
const POINTS: [usize; 5] = [0, 1, 2, 4, 8];

/// Facteurs d'oubli des deux horizons temporels : 0 = rapide, 1 = lent.
const RHO: [f64; 2] = [0.6, 0.95];
/// Taux d'apprentissage du *hedge* qui combine les horizons (H).
const ETA_H: f64 = 1.0;

fn idx_classe(c: &str) -> usize {
    CLASSES.iter().position(|x| *x == c).expect("classe connue")
}

fn idx_bras(b: &str) -> usize {
    BRAS.iter().position(|x| *x == b).expect("bras connu")
}

fn theta(phase: usize, classe: &str, bras: &str) -> f64 {
    let t = match phase {
        1 => &PHASE1,
        2 => &PHASE2,
        _ => &PHASE0,
    };
    t[idx_classe(classe)][idx_bras(bras)]
}

/// Le meilleur bras réel. Réservé à l'évaluateur et aux auto-contrôles.
fn meilleur_bras(phase: usize, classe: &str) -> &'static str {
    BRAS.iter()
        .copied()
        .max_by(|x, y| theta(phase, classe, x).total_cmp(&theta(phase, classe, y)))
        .expect("un bras")
}

/// `quatre` est **redondante** avec `un` : mêmes voisinages. Elle n'existe que pour
/// le scénario `S6`, et son identité avec `un` est ce que `l6_corpus_est_bien_forme`
/// vérifie.
fn voisins(rel: &str, classe: &str) -> &'static [&'static str] {
    let table = match rel {
        "un" => &REL_UN,
        "deux" => &REL_DEUX,
        "trois" => &REL_TROIS,
        "quatre" => &REL_UN,
        autre => panic!("relation inconnue : {autre}"),
    };
    table
        .iter()
        .find(|(c, _)| *c == classe)
        .map(|(_, v)| *v)
        .expect("classe présente dans la relation")
}

// ════════════════════════════════════════════════════════════════════════════
// 2. Tirages pré-générés — identiques pour toutes les politiques
// ════════════════════════════════════════════════════════════════════════════

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

/// Tirage Bernoulli de la `j`-ième occurrence du bras sur la classe. L'uniforme tiré
/// ne dépend **pas** de la phase : seule la comparaison au seuil change.
fn tirage(graine: u64, classe: &str, bras: &str, j: usize, phase: usize) -> bool {
    let mut etat = fnv1a(&format!("{graine}/{classe}/{bras}"));
    for _ in 0..=j {
        lcg(&mut etat);
    }
    let v = etat % 10_000;
    (v as f64) < theta(phase, classe, bras) * 10_000.0
}

fn id_de(graine: u64, classe: &str, tour: usize) -> u128 {
    ((graine as u128) << 96) | ((idx_classe(classe) as u128) << 64) | (tour as u128)
}

fn classe_de(id: u128) -> &'static str {
    CLASSES[((id >> 64) & 0xFFFF_FFFF) as usize]
}

// ════════════════════════════════════════════════════════════════════════════
// 3. Magasin : écriture, lecture stricte, graphe de voisinage
// ════════════════════════════════════════════════════════════════════════════

type Obs = (String, String, bool);
/// provider → (succès, essais)
type Comptes = HashMap<String, (f64, f64)>;

fn echapper(s: &str) -> String {
    s.replace('\\', "\\\\").replace('\'', "\\'")
}

fn run<F: std::future::Future<Output = T>, T>(f: F) -> T {
    block_on(f)
}

/// Lecture stricte : une réponse malformée ou possiblement tronquée est une panne,
/// jamais un résultat vide. (Motif repris de `l5_transfert_adaptatif::lire_lignes`.)
fn lire_lignes(store: &Arc<ObrainStore>, limite: usize) -> Vec<Obs> {
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

/// Le graphe des relations : un nœud `voisinage` par (relation, classe), l'arête
/// `VOISIN_<REL>` qui y mène depuis la classe, et une arête `MEMBRE` depuis chaque
/// voisin.
fn batir_graphe(store: &Arc<ObrainStore>, relations: &[&'static str]) {
    if relations.is_empty() {
        return;
    }
    let mut s = store.db().session();
    s.begin_transaction().expect("begin");
    let mut noeuds: HashMap<&'static str, obrain_common::types::NodeId> = HashMap::new();
    for c in CLASSES {
        let n = s.create_node(&["classe"]);
        s.set_node_property(n, "nom", Value::String(c.into()));
        noeuds.insert(c, n);
    }
    for rel in relations {
        let etiquette = format!("VOISIN_{}", rel.to_uppercase());
        for c in CLASSES {
            let v = s.create_node(&["voisinage"]);
            s.set_node_property(v, "nom", Value::String(format!("{rel}:{c}").into()));
            s.create_edge(noeuds[c], v, &etiquette);
            for p in voisins(rel, c) {
                s.create_edge(noeuds[p], v, "MEMBRE");
            }
        }
    }
    s.commit().expect("commit");
}

fn noms(store: &Arc<ObrainStore>, requete: String, quoi: &str) -> Vec<String> {
    let r = store.execute_query("gql", &requete, 1_000).expect(quoi);
    r.get("rows")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|l| {
            l.get(0)
                .and_then(|v| v.as_str())
                .unwrap_or_else(|| panic!("nom non textuel pour {quoi}"))
                .to_string()
        })
        .collect()
}

/// Voisins d'une classe pour une relation, relus **dans le graphe**.
fn lire_voisins(store: &Arc<ObrainStore>, rel: &str, classe: &str) -> Vec<&'static str> {
    let groupes = noms(
        store,
        format!(
            "MATCH (c:classe)-[:VOISIN_{}]->(v:voisinage) WHERE c.nom = '{}' RETURN v.nom AS nom",
            rel.to_uppercase(),
            echapper(classe)
        ),
        "voisinage",
    );
    let mut out = Vec::new();
    for g in groupes {
        for m in noms(
            store,
            format!(
                "MATCH (m:classe)-[:MEMBRE]->(v:voisinage) WHERE v.nom = '{}' RETURN m.nom AS nom",
                echapper(&g)
            ),
            "membres",
        ) {
            if let Some(c) = CLASSES.iter().find(|c| **c == m)
                && *c != classe
                && !out.contains(c)
            {
                out.push(*c);
            }
        }
    }
    out
}

// ════════════════════════════════════════════════════════════════════════════
// 4. Comptes, répartition de la masse, décision
// ════════════════════════════════════════════════════════════════════════════

fn comptes(lignes: &[Obs], autorisees: Option<&[&str]>) -> Comptes {
    let mut table: Comptes = HashMap::new();
    for (classe, provider, favorable) in lignes {
        if let Some(liste) = autorisees
            && !liste.contains(&classe.as_str())
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

/// Compose local (non plafonné) et transféré (plafonné), en paramètres Beta.
fn composites(local: &Comptes, transfere: &Comptes, plafond: f64) -> HashMap<String, (f64, f64)> {
    let masse: f64 = transfere.values().map(|(_, n)| n).sum();
    let lam = if masse > plafond {
        plafond / masse
    } else {
        1.0
    };
    BRAS.iter()
        .map(|b| {
            let (ls, ln) = local.get(*b).copied().unwrap_or((0.0, 0.0));
            let (ts, tn) = transfere.get(*b).copied().unwrap_or((0.0, 0.0));
            let (ts, tn) = (ts * lam, tn * lam);
            (b.to_string(), (1.0 + ls + ts, 1.0 + (ln - ls) + (tn - ts)))
        })
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

/// Thompson sampling sur le postérieur — le tirage dépend de `id`, donc de la graine
/// et du tour.
fn choisir(bras: &[ArmStats], id: u128) -> String {
    let mut meilleur: Option<(f64, &ArmStats)> = None;
    for (i, b) in bras.iter().enumerate() {
        let graine = seed_of(id) ^ ((i as u64).wrapping_add(1));
        let t = beta_draw(b.alpha, b.beta, graine);
        if meilleur.map(|(m, _)| t > m).unwrap_or(true) {
            meilleur = Some((t, b));
        }
    }
    meilleur
        .expect("au moins un bras")
        .1
        .key
        .provider_id
        .clone()
}

// ════════════════════════════════════════════════════════════════════════════
// 5. Les huit politiques
// ════════════════════════════════════════════════════════════════════════════

#[derive(Clone, Copy, PartialEq, Eq)]
enum Poids {
    /// C — poids identique pour toutes les relations.
    Fixe,
    /// D — poids appris par crédit cumulé (softmax).
    Appris,
    /// E — confiance absolue λ = 1/(1+contradictions).
    Lambda,
    /// F — crédit temporel multi-horizons, moyenne fixe.
    Temporel,
    /// G — λ ⊙ temporel (confiance × crédit).
    LambdaTemporel,
    /// H — combinaison adaptative des horizons (hedge).
    AdaptatifH,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Local,
    Global,
    Relationnel(Poids),
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum Politique {
    A,
    B,
    C,
    D,
    E,
    F,
    G,
    H,
}

const POLITIQUES: [(&str, Politique); 8] = [
    ("A local", Politique::A),
    ("B global", Politique::B),
    ("C relat.fixe", Politique::C),
    ("D relat.cumulé", Politique::D),
    ("E relat.λ", Politique::E),
    ("F relat.temporel", Politique::F),
    ("G relat.λ·t", Politique::G),
    ("H relat.horizons", Politique::H),
];

/// État de confiance d'une relation sur une classe. Un seul état porte toutes les
/// variantes : les champs inutilisés par une politique restent à zéro.
#[derive(Clone, Copy, Default)]
struct Etat {
    /// D : somme des δ (crédit cumulé).
    gain: f64,
    /// D/E : nombre de mises à jour.
    n: u32,
    /// E/G : nombre de tours où δ < 0.
    contradictions: u32,
    /// F/G/H : crédit temporel par horizon (0 = rapide, 1 = lent).
    q_h: [f64; 2],
    /// H : perte cumulée par horizon (hedge exponentiel).
    l_h: [f64; 2],
}

impl Etat {
    /// D : crédit moyen, zéro sans observation.
    fn q(&self) -> f64 {
        self.gain / (self.n as f64 + SHRINK)
    }

    /// E : confiance absolue, 1 sans contradiction, décroissante sinon.
    fn lambda(&self) -> f64 {
        1.0 / (1.0 + self.contradictions as f64)
    }

    /// F : moyenne fixe des horizons.
    fn q_temporel(&self) -> f64 {
        (self.q_h[0] + self.q_h[1]) / 2.0
    }

    /// H : combinaison adaptative — hedge exponentiel sur les horizons.
    fn q_adaptatif(&self) -> f64 {
        let e0 = (-ETA_H * self.l_h[0]).exp();
        let e1 = (-ETA_H * self.l_h[1]).exp();
        let tot = e0 + e1;
        let (a0, a1) = if tot > 0.0 {
            (e0 / tot, e1 / tot)
        } else {
            (0.5, 0.5)
        };
        a0 * self.q_h[0] + a1 * self.q_h[1]
    }

    /// Le scalaire de confiance affiché au diagnostic, selon la politique.
    fn confiance(&self, p: Poids) -> f64 {
        match p {
            Poids::Fixe => 1.0,
            Poids::Appris => self.q(),
            Poids::Lambda => self.lambda(),
            Poids::Temporel => self.q_temporel(),
            Poids::LambdaTemporel => self.lambda() * self.q_temporel(),
            Poids::AdaptatifH => self.q_adaptatif(),
        }
    }

    /// Absorbe le signal δ du tour, selon la politique.
    fn absorber(&mut self, delta: f64, p: Poids) {
        match p {
            Poids::Fixe => {}
            Poids::Appris => {
                self.gain += delta;
                self.n += 1;
            }
            Poids::Lambda => {
                self.n += 1;
                if delta < 0.0 {
                    self.contradictions += 1;
                }
            }
            Poids::Temporel => {
                for (h, rho) in RHO.iter().enumerate() {
                    self.q_h[h] = *rho * self.q_h[h] + (1.0 - *rho) * delta;
                }
            }
            Poids::LambdaTemporel => {
                for (h, rho) in RHO.iter().enumerate() {
                    self.q_h[h] = *rho * self.q_h[h] + (1.0 - *rho) * delta;
                }
                if delta < 0.0 {
                    self.contradictions += 1;
                }
            }
            Poids::AdaptatifH => {
                for (h, rho) in RHO.iter().enumerate() {
                    self.l_h[h] += (self.q_h[h] - delta).powi(2);
                    self.q_h[h] = *rho * self.q_h[h] + (1.0 - *rho) * delta;
                }
            }
        }
    }
}

/// Répartition de la masse entre relations, **normalisée à `n_rel`** : toutes les
/// variantes transfèrent la même masse que C, seule la provenance change.
fn poids_depuis(
    relations: &[(&'static str, &'static [&'static str])],
    mode: Mode,
    etats: &HashMap<&'static str, Etat>,
) -> HashMap<&'static str, f64> {
    let n_rel = relations.len() as f64;
    let mut w = HashMap::new();
    match mode {
        Mode::Relationnel(Poids::Fixe) => {
            for (rel, _) in relations {
                w.insert(*rel, 1.0);
            }
        }
        Mode::Relationnel(Poids::Appris) => {
            let exps: Vec<(&'static str, f64)> = relations
                .iter()
                .map(|(rel, _)| {
                    (
                        *rel,
                        (TEMP * etats.get(rel).map(|e| e.q()).unwrap_or(0.0)).exp(),
                    )
                })
                .collect();
            let total: f64 = exps.iter().map(|(_, e)| *e).sum();
            for (rel, e) in exps {
                w.insert(rel, if total > 0.0 { n_rel * e / total } else { 0.0 });
            }
        }
        Mode::Relationnel(Poids::Lambda) => {
            let lams: Vec<(&'static str, f64)> = relations
                .iter()
                .map(|(rel, _)| (*rel, etats.get(rel).map(|e| e.lambda()).unwrap_or(1.0)))
                .collect();
            let total: f64 = lams.iter().map(|(_, l)| *l).sum();
            for (rel, lam) in lams {
                w.insert(
                    rel,
                    if total > 0.0 {
                        n_rel * lam / total
                    } else {
                        0.0
                    },
                );
            }
        }
        Mode::Relationnel(Poids::Temporel) => {
            let exps: Vec<(&'static str, f64)> = relations
                .iter()
                .map(|(rel, _)| {
                    (
                        *rel,
                        (TEMP * etats.get(rel).map(|e| e.q_temporel()).unwrap_or(0.0)).exp(),
                    )
                })
                .collect();
            let total: f64 = exps.iter().map(|(_, e)| *e).sum();
            for (rel, e) in exps {
                w.insert(rel, if total > 0.0 { n_rel * e / total } else { 0.0 });
            }
        }
        Mode::Relationnel(Poids::LambdaTemporel) => {
            let exps: Vec<(&'static str, f64)> = relations
                .iter()
                .map(|(rel, _)| {
                    let e = etats.get(rel).unwrap();
                    (*rel, (TEMP * e.lambda() * e.q_temporel()).exp())
                })
                .collect();
            let total: f64 = exps.iter().map(|(_, e)| *e).sum();
            for (rel, e) in exps {
                w.insert(rel, if total > 0.0 { n_rel * e / total } else { 0.0 });
            }
        }
        Mode::Relationnel(Poids::AdaptatifH) => {
            let exps: Vec<(&'static str, f64)> = relations
                .iter()
                .map(|(rel, _)| {
                    (
                        *rel,
                        (TEMP * etats.get(rel).map(|e| e.q_adaptatif()).unwrap_or(0.0)).exp(),
                    )
                })
                .collect();
            let total: f64 = exps.iter().map(|(_, e)| *e).sum();
            for (rel, e) in exps {
                w.insert(rel, if total > 0.0 { n_rel * e / total } else { 0.0 });
            }
        }
        _ => {}
    }
    w
}

struct Banc {
    store: Arc<ObrainStore>,
    mode: Mode,
    /// classe → [(relation, voisins)], lu une fois dans le graphe.
    voisinages: HashMap<String, Vec<(&'static str, &'static [&'static str])>>,
    etat: Mutex<HashMap<(String, String), Etat>>,
    /// Comptes de voisinage du tour en cours, calculés par `arms` et réutilisés par
    /// `settle` : la prédiction jugée est bien celle d'avant l'observation.
    tour: Mutex<Option<(String, HashMap<&'static str, Comptes>)>>,
}

impl Banc {
    fn nouveau(store: Arc<ObrainStore>, mode: Mode, relations: &[&'static str]) -> Self {
        let mut voisinages = HashMap::new();
        if let Mode::Relationnel(_) = mode {
            for c in CLASSES {
                let mut v = Vec::new();
                for rel in relations {
                    let relus = lire_voisins(&store, rel, c);
                    assert_eq!(
                        relus,
                        voisins(rel, c),
                        "relation '{rel}', classe '{c}' : le graphe relu diffère de la table"
                    );
                    let statique: &'static [&'static str] = voisins(rel, c);
                    v.push((*rel, statique));
                }
                voisinages.insert(c.to_string(), v);
            }
        }
        Self {
            store,
            mode,
            voisinages,
            etat: Mutex::new(HashMap::new()),
            tour: Mutex::new(None),
        }
    }

    fn poids_classe(&self, classe: &str) -> HashMap<&'static str, f64> {
        let relations = self.voisinages.get(classe).cloned().unwrap_or_default();
        poids_depuis(&relations, self.mode, &self.etats_de(classe))
    }

    /// État de confiance par relation pour cette classe (vide au départ).
    fn etats_de(&self, classe: &str) -> HashMap<&'static str, Etat> {
        let etat = self.etat.lock().unwrap();
        self.voisinages
            .get(classe)
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .map(|(rel, _)| {
                (
                    rel,
                    etat.get(&(rel.to_string(), classe.to_string()))
                        .copied()
                        .unwrap_or_default(),
                )
            })
            .collect()
    }

    /// Comptes par relation pour cette classe, à partir des lignes données.
    fn comptes_par_relation(&self, classe: &str, lignes: &[Obs]) -> HashMap<&'static str, Comptes> {
        let mut m = HashMap::new();
        for (rel, vs) in self.voisinages.get(classe).cloned().unwrap_or_default() {
            if vs.is_empty() {
                continue;
            }
            m.insert(rel, comptes(lignes, Some(vs)));
        }
        m
    }

    /// Comptes du tour en cours ; à défaut de cache valide, relecture.
    fn comptes_du_tour(&self, classe: &str) -> HashMap<&'static str, Comptes> {
        {
            let garde = self.tour.lock().unwrap();
            if let Some((c, m)) = garde.as_ref()
                && c == classe
            {
                return m.clone();
            }
        }
        let lignes = lire_lignes(&self.store, 100_000);
        self.comptes_par_relation(classe, &lignes)
    }

    /// Diagnostic : (relation, confiance, poids, n) pour cette classe.
    fn diagnostic_de(&self, classe: &str) -> Vec<(&'static str, f64, f64, u32)> {
        let relations = self.voisinages.get(classe).cloned().unwrap_or_default();
        let poids = self.poids_classe(classe);
        let etats = self.etats_de(classe);
        let mode = self.mode;
        relations
            .iter()
            .map(|(rel, _)| {
                let e = etats.get(rel).copied().unwrap_or_default();
                let confiance = if let Mode::Relationnel(p) = mode {
                    e.confiance(p)
                } else {
                    0.0
                };
                (*rel, confiance, poids.get(rel).copied().unwrap_or(0.0), e.n)
            })
            .collect()
    }
}

#[async_trait(?Send)]
impl Learning for Banc {
    async fn arms(&self, classe: &str) -> Vec<ArmStats> {
        let lignes = lire_lignes(&self.store, 100_000);
        let local = comptes(&lignes, Some(&[classe]));
        let transfere = match self.mode {
            Mode::Local => Comptes::new(),
            Mode::Global => {
                let autres: Vec<&'static str> =
                    CLASSES.iter().copied().filter(|c| *c != classe).collect();
                comptes(&lignes, Some(&autres))
            }
            Mode::Relationnel(_) => {
                let par_relation = self.comptes_par_relation(classe, &lignes);
                let poids = self.poids_classe(classe);
                let masse_c: f64 = par_relation
                    .values()
                    .map(|c| c.values().map(|(_, n)| *n).sum::<f64>())
                    .sum();
                let mut pool: Comptes = Comptes::new();
                for (rel, c) in &par_relation {
                    let w = poids.get(rel).copied().unwrap_or(0.0);
                    if w <= 0.0 {
                        continue;
                    }
                    for (prov, (s, n)) in c {
                        let e = pool.entry(prov.clone()).or_insert((0.0, 0.0));
                        e.0 += w * s;
                        e.1 += w * n;
                    }
                }
                // Renormalisation sur la masse de C : seule la répartition change.
                let masse_d: f64 = pool.values().map(|(_, n)| *n).sum();
                if masse_d > 0.0 && masse_c > 0.0 {
                    let k = masse_c / masse_d;
                    if k != 1.0 {
                        for (s, n) in pool.values_mut() {
                            *s *= k;
                            *n *= k;
                        }
                    }
                }
                *self.tour.lock().unwrap() = Some((classe.to_string(), par_relation));
                pool
            }
        };
        let priors = composites(&local, &transfere, MASSE_MAX);
        bras_a_priori(classe, &priors)
    }

    async fn settle(&self, id: u128, served: Option<&Pick>, stop: &Stop, _code: Option<&str>) {
        let Some(p) = served else { return };
        let favorable = match stop {
            Stop::Done => true,
            Stop::Failed(_) => false,
            _ => return,
        };
        let classe = classe_de(id);

        // Jugement AVANT écriture, sur le bras réellement joué (hors politique) :
        // c'est le signal δ commun aux variantes D..H.
        if let Mode::Relationnel(poids) = self.mode {
            let par_relation = self.comptes_du_tour(classe);
            let mut etat = self.etat.lock().unwrap();
            for (rel, c) in par_relation {
                let Some((s, n)) = c.get(&p.provider_id).copied() else {
                    continue;
                };
                if n <= 0.0 {
                    continue;
                }
                let taux = ((s + 0.5) / (n + 1.0)).clamp(0.02, 0.98);
                let ll = if favorable {
                    taux.ln()
                } else {
                    (1.0 - taux).ln()
                };
                let delta = ll - 0.5f64.ln();
                etat.entry((rel.to_string(), classe.to_string()))
                    .or_default()
                    .absorber(delta, poids);
            }
        }

        ecrire_observation(&self.store, id, classe, &p.provider_id, &p.model, favorable);
    }
}

// ════════════════════════════════════════════════════════════════════════════
// 6. Scénarios et conducteur
// ════════════════════════════════════════════════════════════════════════════

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum Scenario {
    Utile,
    Trompeuse,
    Changeante,
    Concurrentes,
    PlusieursTrompeuses,
    Redondantes,
    CapacitesInedites,
    Absente,
}

const SCENARIOS: [(&str, Scenario); 8] = [
    ("S1 relation utile", Scenario::Utile),
    ("S2 trompeuse unique", Scenario::Trompeuse),
    ("S3 pertinence changeante", Scenario::Changeante),
    ("S4 concurrentes", Scenario::Concurrentes),
    ("S5 plusieurs trompeuses", Scenario::PlusieursTrompeuses),
    ("S6 redondantes", Scenario::Redondantes),
    ("S7 capacités inédites", Scenario::CapacitesInedites),
    ("S8 absence de relation", Scenario::Absente),
];

impl Scenario {
    fn relations(self) -> &'static [&'static str] {
        match self {
            Scenario::Utile => &["un"],
            Scenario::Trompeuse => &["deux"],
            Scenario::Changeante | Scenario::Concurrentes => &["un", "deux"],
            Scenario::PlusieursTrompeuses => &["deux", "trois"],
            Scenario::Redondantes => &["un", "quatre"],
            Scenario::CapacitesInedites => &["un", "deux"],
            Scenario::Absente => &[],
        }
    }

    fn bascule(self) -> Option<usize> {
        match self {
            Scenario::Changeante | Scenario::CapacitesInedites => Some(T_BASCULE),
            _ => None,
        }
    }

    /// Phase atteinte après la bascule : 1 (inversion des rôles) ou 2 (péremption).
    fn phase_apres(self) -> usize {
        match self {
            Scenario::Changeante => 1,
            Scenario::CapacitesInedites => 2,
            _ => 0,
        }
    }

    fn phase(self, tour: usize) -> usize {
        match self.bascule() {
            Some(t) if tour >= t => self.phase_apres(),
            _ => 0,
        }
    }
}

fn nom_scenario(s: Scenario) -> &'static str {
    SCENARIOS.iter().find(|(_, x)| *x == s).expect("scénario").0
}

fn nom_politique(p: Politique) -> &'static str {
    POLITIQUES
        .iter()
        .find(|(_, x)| *x == p)
        .expect("politique")
        .0
}

impl Politique {
    fn mode(self) -> Mode {
        match self {
            Politique::A => Mode::Local,
            Politique::B => Mode::Global,
            Politique::C => Mode::Relationnel(Poids::Fixe),
            Politique::D => Mode::Relationnel(Poids::Appris),
            Politique::E => Mode::Relationnel(Poids::Lambda),
            Politique::F => Mode::Relationnel(Poids::Temporel),
            Politique::G => Mode::Relationnel(Poids::LambdaTemporel),
            Politique::H => Mode::Relationnel(Poids::AdaptatifH),
        }
    }
}

fn dir_unique(etiquette: &str) -> std::path::PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    static N: AtomicU64 = AtomicU64::new(0);
    let d = std::env::temp_dir().join(format!(
        "l6_{etiquette}_{}_{}",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&d);
    d
}

type Nettoyage = (std::path::PathBuf, Arc<ObrainStore>);

fn instancier(scenario: Scenario, politique: Politique, graine: u64) -> (Arc<Banc>, Nettoyage) {
    let etiquette = format!(
        "{}_{}_{graine}",
        nom_scenario(scenario)
            .split_whitespace()
            .next()
            .unwrap_or("s"),
        politique as u8
    );
    let d = dir_unique(&etiquette);
    let store = ObrainStore::open(&d).expect("ouverture du store");
    let relations = scenario.relations();
    let mode = politique.mode();
    if let Mode::Relationnel(_) = mode {
        batir_graphe(&store, relations);
    }
    (
        Arc::new(Banc::nouveau(store.clone(), mode, relations)),
        (d, store),
    )
}

fn liberer(nettoyage: Nettoyage) {
    let (d, store) = nettoyage;
    store.commit_wal().expect("flush");
    drop(store);
    let _ = std::fs::remove_dir_all(&d);
}

/// Une trajectoire : par classe, la suite (bras choisi, issue).
type Traj = Vec<Vec<(String, bool)>>;

fn jouer(banc: &Banc, scenario: Scenario, graine: u64) -> Traj {
    let mut out: Traj = Vec::with_capacity(CLASSES.len());
    for classe in CLASSES {
        let mut traj = Vec::with_capacity(TOURS);
        let mut occurrences: HashMap<String, usize> = HashMap::new();
        for t in 0..TOURS {
            let phase = scenario.phase(t);
            let bras = run(banc.arms(classe));
            let id = id_de(graine, classe, t);
            let provider = choisir(&bras, id);
            let j = *occurrences.get(&provider).unwrap_or(&0);
            occurrences.insert(provider.clone(), j + 1);
            let ok = tirage(graine, classe, &provider, j, phase);
            let pick = Pick::new(provider.clone(), MODELE.to_string());
            let stop = if ok {
                Stop::Done
            } else {
                Stop::Failed("transport".into())
            };
            run(banc.settle(id, Some(&pick), &stop, None));
            traj.push((provider, ok));
        }
        out.push(traj);
    }
    out
}

// ════════════════════════════════════════════════════════════════════════════
// 7. Mesures
// ════════════════════════════════════════════════════════════════════════════

#[derive(Clone, Copy, Default)]
struct Mesures {
    choix_optimaux: f64,
    total: f64,
    succes_t0: f64,
    succes_k: [f64; POINTS.len()],
    regret: f64,
    regret_apres: f64,
    sous_opt_apres: f64,
    recup_taux: f64,
    recup_tour: f64,
}

impl Mesures {
    fn succes(&self) -> f64 {
        if self.total == 0.0 {
            0.0
        } else {
            self.choix_optimaux / self.total
        }
    }
}

fn mesurer(traj: &Traj, scenario: Scenario) -> Mesures {
    let n = CLASSES.len() as f64;
    let mut m = Mesures::default();
    let bascule = scenario.bascule();
    let mut recup_tours: Vec<f64> = Vec::new();
    let mut recup_n = 0.0;

    for (i, classe) in CLASSES.iter().enumerate() {
        let t = &traj[i];
        let mut regret = 0.0;
        let mut regret_apres = 0.0;
        let mut sous_opt_apres = 0.0;
        for (k, (provider, _issue)) in t.iter().enumerate() {
            let phase = scenario.phase(k);
            let best = meilleur_bras(phase, classe);
            let optimal = provider == best;
            let r = theta(phase, classe, best) - theta(phase, classe, provider);
            m.total += 1.0;
            if optimal {
                m.choix_optimaux += 1.0;
            }
            regret += r;
            if k == 0 && optimal {
                m.succes_t0 += 1.0;
            }
            if let Some(j) = POINTS.iter().position(|p| *p == k)
                && optimal
            {
                m.succes_k[j] += 1.0;
            }
            if let Some(b) = bascule
                && k >= b
            {
                regret_apres += r;
                if !optimal {
                    sous_opt_apres += 1.0;
                }
            }
        }
        m.regret += regret;
        if let Some(debut) = bascule {
            m.regret_apres += regret_apres;
            m.sous_opt_apres += sous_opt_apres;
            let rec = (debut..t.len()).find(|k| {
                (*k..t.len()).all(|q| t[q].0 == meilleur_bras(scenario.phase(q), classe))
            });
            if let Some(k) = rec {
                recup_n += 1.0;
                recup_tours.push(k as f64);
            }
        }
    }

    m.regret /= n;
    m.succes_t0 /= n;
    for s in m.succes_k.iter_mut() {
        *s /= n;
    }
    if bascule.is_some() {
        m.regret_apres /= n;
        m.sous_opt_apres /= n;
        m.recup_taux = recup_n / n;
        m.recup_tour = mediane(&recup_tours).unwrap_or(f64::NAN);
    }
    m
}

fn mediane(v: &[f64]) -> Option<f64> {
    if v.is_empty() {
        return None;
    }
    let mut s = v.to_vec();
    s.sort_by(|a, b| a.total_cmp(b));
    let n = s.len();
    Some(if n % 2 == 1 {
        s[n / 2]
    } else {
        (s[n / 2 - 1] + s[n / 2]) / 2.0
    })
}

fn fmt(v: f64, chiffres: usize) -> String {
    if v.is_nan() {
        "—".to_string()
    } else {
        format!("{v:.chiffres$}")
    }
}

// ════════════════════════════════════════════════════════════════════════════
// 8. Forme du corpus — rapide, sans magasin
// ════════════════════════════════════════════════════════════════════════════

#[test]
fn l6_corpus_est_bien_forme() {
    // Meilleur bras unique, avec une marge, dans les TROIS phases.
    for phase in 0..3 {
        for c in CLASSES {
            let mut vals: Vec<f64> = BRAS.iter().map(|b| theta(phase, c, b)).collect();
            vals.sort_by(|a, b| b.total_cmp(a));
            assert!(
                vals[0] - vals[1] >= 0.4,
                "phase {phase}, classe {c} : marge {:.3} < 0,4 — meilleur bras ambigu",
                vals[0] - vals[1]
            );
            assert_eq!(
                theta(phase, c, meilleur_bras(phase, c)),
                vals[0],
                "phase {phase}, classe {c} : `meilleur_bras` ne donne pas le maximum"
            );
        }
    }

    // S1 : `un` est UTILE en phase 0 — chaque voisin partage le meilleur bras.
    for c in CLASSES {
        let best = meilleur_bras(0, c);
        for v in voisins("un", c) {
            assert_eq!(
                meilleur_bras(0, v),
                best,
                "relation `un` en phase 0 : le voisin {v} de {c} ne partage pas son meilleur bras"
            );
        }
    }

    // S2/S5 : `deux` et `trois` sont TROMPEUSES en phase 0 — aucun voisin ne partage.
    for rel in ["deux", "trois"] {
        for c in CLASSES {
            let best = meilleur_bras(0, c);
            assert!(
                !voisins(rel, c).is_empty(),
                "relation `{rel}` : {c} doit avoir au moins un voisin pour être trompée"
            );
            for v in voisins(rel, c) {
                assert_ne!(
                    meilleur_bras(0, v),
                    best,
                    "relation `{rel}` en phase 0 : le voisin {v} de {c} partage son meilleur bras"
                );
            }
        }
    }

    // S3 : les rôles s'INVERSENT en phase 1 (`deux` utile, `un` trompeuse).
    for c in CLASSES {
        let best = meilleur_bras(1, c);
        for v in voisins("deux", c) {
            assert_eq!(
                meilleur_bras(1, v),
                best,
                "relation `deux` en phase 1 : le voisin {v} de {c} ne partage pas son meilleur bras"
            );
        }
        for v in voisins("un", c) {
            assert_ne!(
                meilleur_bras(1, v),
                best,
                "relation `un` en phase 1 : le voisin {v} de {c} partage son meilleur bras"
            );
        }
    }

    // S7 : en phase 2, les DEUX relations sont périmées — aucun voisin ne partage le
    // meilleur bras, ni pour `un`, ni pour `deux`. C'est ce qui distingue « capacités
    // inédites » (repli sur le local) de « pertinence changeante » (bascule d'une
    // relation à l'autre).
    for rel in ["un", "deux"] {
        for c in CLASSES {
            let best = meilleur_bras(2, c);
            for v in voisins(rel, c) {
                assert_ne!(
                    meilleur_bras(2, v),
                    best,
                    "relation `{rel}` en phase 2 : le voisin {v} de {c} partage son meilleur bras — la capacité n'est pas inédite"
                );
            }
        }
    }

    // Les relations sont des appartenances : symétriques, sans boucle.
    for rel in ["un", "deux", "trois", "quatre"] {
        for c in CLASSES {
            assert!(
                !voisins(rel, c).contains(&c),
                "relation `{rel}` : {c} est son propre voisin"
            );
            for v in voisins(rel, c) {
                assert!(
                    voisins(rel, v).contains(&c),
                    "relation `{rel}` : {c} cite {v} mais {v} ne cite pas {c}"
                );
            }
        }
    }

    // `quatre` est REDONDANTE avec `un` : mêmes voisinages, terme pour terme.
    for c in CLASSES {
        assert_eq!(
            voisins("quatre", c),
            voisins("un", c),
            "relation `quatre` : {c} ne réplique pas `un`"
        );
    }

    // `c8` et `c9` n'ont aucun voisin dans `un` (et `quatre`) : le cas « pas de
    // relation pour cette classe » existe aussi dans les scénarios relationnels.
    for c in ["c8", "c9"] {
        assert!(
            voisins("un", c).is_empty(),
            "{c} doit être isolée dans `un`"
        );
        assert!(
            !voisins("deux", c).is_empty(),
            "{c} doit avoir des voisins dans `deux`"
        );
        assert!(
            !voisins("trois", c).is_empty(),
            "{c} doit avoir des voisins dans `trois`"
        );
    }
}

// ════════════════════════════════════════════════════════════════════════════
// 9. Le banc
// ════════════════════════════════════════════════════════════════════════════

/// Diagnostic d'une politique relationnelle sur une classe :
/// (relation, confiance, poids, n).
type Diagnostic = Vec<(String, Vec<(&'static str, f64, f64, u32)>)>;

#[test]
fn l6_confiance_temporelle() {
    let mut trajs: HashMap<(Scenario, Politique, u64), Traj> = HashMap::new();
    let mut mesures: HashMap<(Scenario, Politique), Vec<Mesures>> = HashMap::new();
    let mut couts: HashMap<(Scenario, Politique), Duration> = HashMap::new();
    // Diagnostic (première graine) des politiques relationnelles apprenantes.
    let mut diag: HashMap<(Scenario, Politique), Diagnostic> = HashMap::new();

    for (_, scenario) in SCENARIOS {
        for (_, politique) in POLITIQUES {
            let debut = Instant::now();
            let mut par_graine = Vec::new();
            for graine in GRAINES {
                let (traj, diag_g0) = {
                    let (banc, nettoyage) = instancier(scenario, politique, graine);
                    let traj = jouer(&banc, scenario, graine);
                    let diag_g0 = if graine == GRAINES[0]
                        && matches!(politique.mode(), Mode::Relationnel(p) if p != Poids::Fixe)
                    {
                        Some(
                            CLASSES
                                .iter()
                                .map(|c| (c.to_string(), banc.diagnostic_de(c)))
                                .collect::<Vec<_>>(),
                        )
                    } else {
                        None
                    };
                    drop(banc);
                    liberer(nettoyage);
                    (traj, diag_g0)
                };
                if let Some(d) = diag_g0 {
                    diag.insert((scenario, politique), d);
                }
                par_graine.push(mesurer(&traj, scenario));
                trajs.insert((scenario, politique, graine), traj);
            }
            mesures.insert((scenario, politique), par_graine);
            couts.insert((scenario, politique), debut.elapsed());
        }
    }

    // ── Rapport ───────────────────────────────────────────────────────────
    let val = |s: Scenario, p: Politique, sel: &dyn Fn(&Mesures) -> f64| -> f64 {
        let v: Vec<f64> = mesures[&(s, p)].iter().map(sel).collect();
        mediane(&v).unwrap_or(f64::NAN)
    };
    let delta = |s: Scenario, p: Politique, sel: &dyn Fn(&Mesures) -> f64| -> f64 {
        let v: Vec<f64> = (0..GRAINES.len())
            .map(|i| sel(&mesures[&(s, p)][i]) - sel(&mesures[&(s, Politique::A)][i]))
            .collect();
        mediane(&v).unwrap_or(f64::NAN)
    };

    for (_, scenario) in SCENARIOS {
        let change = scenario.bascule().is_some();
        println!("===== L6 · {} =====", nom_scenario(scenario));
        println!(
            "  {: <17} {: >6} {: >6} {: >6} {: >7} {: >8} {: >11} {: >11} {: >13}",
            "politique",
            "t0",
            "k=4",
            "k=8",
            "succès",
            "regret",
            "Δregret / A",
            "sous-opt. t>",
            "récup."
        );
        for (_, p) in POLITIQUES {
            let t0 = val(scenario, p, &|m| m.succes_t0);
            let k4 = val(scenario, p, &|m| m.succes_k[3]);
            let k8 = val(scenario, p, &|m| m.succes_k[4]);
            let suc = val(scenario, p, &|m| m.succes());
            let reg = val(scenario, p, &|m| m.regret);
            let d = delta(scenario, p, &|m| m.regret);
            let (sous, recup) = if change {
                (
                    fmt(val(scenario, p, &|m| m.sous_opt_apres), 2),
                    format!(
                        "{:.0}% @ t={}",
                        val(scenario, p, &|m| m.recup_taux) * 100.0,
                        fmt(val(scenario, p, &|m| m.recup_tour), 0)
                    ),
                )
            } else {
                ("—".to_string(), "—".to_string())
            };
            println!(
                "  {: <17} {: >6} {: >6} {: >6} {: >7} {: >8} {: >+11} {: >11} {: >13}",
                nom_politique(p),
                fmt(t0, 2),
                fmt(k4, 2),
                fmt(k8, 2),
                fmt(suc, 3),
                fmt(reg, 2),
                d,
                sous,
                recup
            );
        }
    }
    println!("  (t0 / k=4 / k=8 : choix optimal après exactement 0, 4 et 8 observations ;");
    println!(
        "   succès : sur tous les tours ; Δregret/A : médiane par graine, > 0 = transfert NÉGATIF)"
    );

    // ── Coût réel mesuré ─────────────────────────────────────────────────
    println!("===== L6 · coût (temps écoulé, 3 graines) =====");
    let mut cout_total: HashMap<Politique, Duration> = HashMap::new();
    for ((_, p), d) in &couts {
        *cout_total.entry(*p).or_insert(Duration::ZERO) += *d;
    }
    for (_, p) in POLITIQUES {
        let total = cout_total.get(&p).copied().unwrap_or(Duration::ZERO);
        let par_scenario: Duration = couts
            .iter()
            .filter(|((_, pp), _)| *pp == p)
            .map(|(_, d)| *d)
            .sum();
        println!(
            "  {: <17} total {: >5.1}s   (~{: >4.0} ms/scénario)",
            nom_politique(p),
            total.as_secs_f64(),
            par_scenario.as_secs_f64() * 1000.0 / SCENARIOS.len() as f64
        );
    }
    println!(
        "  (toutes les politiques relationnelles font le même nombre de lectures GQL par tour :"
    );
    println!("   la différence de coût est du calcul en mémoire, pas des lectures)");

    // ── Diagnostic : ce que les politiques apprenantes ont appris ──────────
    println!("===== L6 · confiance apprise (graine 0) =====");
    for scenario in [
        Scenario::Concurrentes,
        Scenario::Changeante,
        Scenario::PlusieursTrompeuses,
        Scenario::CapacitesInedites,
    ] {
        println!("  · {} ·", nom_scenario(scenario));
        for p in [Politique::D, Politique::E, Politique::F, Politique::H] {
            let Some(classes) = diag.get(&(scenario, p)) else {
                continue;
            };
            let lignes: Vec<String> = classes
                .iter()
                .map(|(c, cr)| {
                    let detail = cr
                        .iter()
                        .map(|(rel, conf, w, _)| format!("{rel}: c={conf:+.2} w={w:.2}"))
                        .collect::<Vec<_>>()
                        .join("  ");
                    format!("{c} [{detail}]")
                })
                .collect();
            println!("    {: <17} {}", nom_politique(p), lignes.join("  "));
        }
    }
    println!("==================================================================");

    // ── INVARIANTS (assertés) ─────────────────────────────────────────────
    // 1. A ne lit aucune relation : sa trajectoire ne dépend pas des relations du
    //    scénario, tant que le monde (phase 0) et les tirages sont les mêmes.
    for graine in GRAINES {
        let reference = trajs[&(Scenario::Utile, Politique::A, graine)].clone();
        for scenario in [
            Scenario::Trompeuse,
            Scenario::Concurrentes,
            Scenario::PlusieursTrompeuses,
            Scenario::Redondantes,
            Scenario::Absente,
        ] {
            assert_eq!(
                reference,
                trajs[&(scenario, Politique::A, graine)],
                "INVARIANT : graine {graine} — la politique A change de trajectoire selon le scénario ({}) alors que le monde et les tirages sont les mêmes",
                nom_scenario(scenario)
            );
        }
    }

    // 2. Pas de relation ⇒ pas de transfert relationnel ⇒ C..H ≡ A (B en est exclu :
    //    son transfert est global, indépendant du graphe).
    for graine in GRAINES {
        let a = trajs[&(Scenario::Absente, Politique::A, graine)].clone();
        for p in [
            Politique::C,
            Politique::D,
            Politique::E,
            Politique::F,
            Politique::G,
            Politique::H,
        ] {
            assert_eq!(
                a,
                trajs[&(Scenario::Absente, p, graine)],
                "INVARIANT : graine {graine} — {} transfère alors qu'aucune relation n'existe",
                nom_politique(p)
            );
        }
    }

    // 3. Une seule relation ⇒ la normalisation à n_rel=1 vaut 1 pour tous ⇒ C..H ≡ C.
    for scenario in [Scenario::Utile, Scenario::Trompeuse] {
        for graine in GRAINES {
            let c = trajs[&(scenario, Politique::C, graine)].clone();
            for p in [
                Politique::D,
                Politique::E,
                Politique::F,
                Politique::G,
                Politique::H,
            ] {
                assert_eq!(
                    c,
                    trajs[&(scenario, p, graine)],
                    "INVARIANT : graine {graine} — {} diverge de C sur {} alors qu'il n'y a qu'une relation",
                    nom_politique(p),
                    nom_scenario(scenario)
                );
            }
        }
    }

    // 4. Relations redondantes identiques ⇒ signaux identiques ⇒ C..H ≡ C.
    for graine in GRAINES {
        let c = trajs[&(Scenario::Redondantes, Politique::C, graine)].clone();
        for p in [
            Politique::D,
            Politique::E,
            Politique::F,
            Politique::G,
            Politique::H,
        ] {
            assert_eq!(
                c,
                trajs[&(Scenario::Redondantes, p, graine)],
                "INVARIANT : graine {graine} — {} diverge de C sur des relations redondantes identiques",
                nom_politique(p)
            );
        }
    }

    // 5. État vide ⇒ pondération uniforme ⇒ le premier choix de E/F/G/H est celui de C.
    for scenario in [
        Scenario::Changeante,
        Scenario::Concurrentes,
        Scenario::PlusieursTrompeuses,
        Scenario::Redondantes,
        Scenario::CapacitesInedites,
    ] {
        for graine in GRAINES {
            let c = &trajs[&(scenario, Politique::C, graine)];
            for p in [
                Politique::D,
                Politique::E,
                Politique::F,
                Politique::G,
                Politique::H,
            ] {
                let q = &trajs[&(scenario, p, graine)];
                for i in 0..CLASSES.len() {
                    assert_eq!(
                        c[i][0].0,
                        q[i][0].0,
                        "INVARIANT : graine {graine}, {} — le premier choix de {} ({}) diffère de C ({}) sur {} : l'état n'est pas vide au départ",
                        nom_scenario(scenario),
                        nom_politique(p),
                        q[i][0].0,
                        c[i][0].0,
                        CLASSES[i]
                    );
                }
            }
        }
    }

    // ── HYPOTHÈSES (imprimées, jamais assertées) ──────────────────────────
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

    let regret = |s: Scenario, p: Politique| val(s, p, &|m: &Mesures| m.regret);
    let sous_opt = |s: Scenario, p: Politique| val(s, p, &|m: &Mesures| m.sous_opt_apres);
    let regret_apres = |s: Scenario, p: Politique| val(s, p, &|m: &Mesures| m.regret_apres);

    println!("===== L6 · hypothèses (verdicts, aucune assertion) =====");
    hypothese!(
        "H1 une relation utile transfère positivement",
        regret(Scenario::Utile, Politique::C) < regret(Scenario::Utile, Politique::A),
        format!(
            "S1 regret C {:.2} vs A {:.2} → {:+.2}",
            regret(Scenario::Utile, Politique::C),
            regret(Scenario::Utile, Politique::A),
            regret(Scenario::Utile, Politique::C) - regret(Scenario::Utile, Politique::A)
        )
    );
    hypothese!(
        "H2 une relation trompeuse produit un transfert NÉGATIF",
        regret(Scenario::Trompeuse, Politique::C) > regret(Scenario::Trompeuse, Politique::A),
        format!(
            "S2 regret C {:.2} vs A {:.2} → {:+.2}",
            regret(Scenario::Trompeuse, Politique::C),
            regret(Scenario::Trompeuse, Politique::A),
            regret(Scenario::Trompeuse, Politique::C) - regret(Scenario::Trompeuse, Politique::A)
        )
    );
    hypothese!(
        "H3 l'attribution par relation réduit le coût de la relation trompeuse",
        regret(Scenario::Concurrentes, Politique::D) < regret(Scenario::Concurrentes, Politique::C),
        format!(
            "S4 regret D {:.2} vs C {:.2} → {:+.2}",
            regret(Scenario::Concurrentes, Politique::D),
            regret(Scenario::Concurrentes, Politique::C),
            regret(Scenario::Concurrentes, Politique::D)
                - regret(Scenario::Concurrentes, Politique::C)
        )
    );
    hypothese!(
        "H4 (plusieurs trompeuses) la confiance λ atténue le double transfert négatif",
        regret(Scenario::PlusieursTrompeuses, Politique::E)
            < regret(Scenario::PlusieursTrompeuses, Politique::C),
        format!(
            "S5 regret E {:.2} vs C {:.2} → {:+.2}",
            regret(Scenario::PlusieursTrompeuses, Politique::E),
            regret(Scenario::PlusieursTrompeuses, Politique::C),
            regret(Scenario::PlusieursTrompeuses, Politique::E)
                - regret(Scenario::PlusieursTrompeuses, Politique::C)
        )
    );
    hypothese!(
        "H5 (plusieurs trompeuses) le crédit cumulé atténue aussi",
        regret(Scenario::PlusieursTrompeuses, Politique::D)
            < regret(Scenario::PlusieursTrompeuses, Politique::C),
        format!(
            "S5 regret D {:.2} vs C {:.2} → {:+.2}",
            regret(Scenario::PlusieursTrompeuses, Politique::D),
            regret(Scenario::PlusieursTrompeuses, Politique::C),
            regret(Scenario::PlusieursTrompeuses, Politique::D)
                - regret(Scenario::PlusieursTrompeuses, Politique::C)
        )
    );
    hypothese!(
        "H6 la redondance utile reste un transfert positif",
        regret(Scenario::Redondantes, Politique::C) < regret(Scenario::Redondantes, Politique::A),
        format!(
            "S6 regret C {:.2} vs A {:.2} → {:+.2}",
            regret(Scenario::Redondantes, Politique::C),
            regret(Scenario::Redondantes, Politique::A),
            regret(Scenario::Redondantes, Politique::C)
                - regret(Scenario::Redondantes, Politique::A)
        )
    );
    hypothese!(
        "H7 le crédit temporel récupère plus vite que le crédit cumulé",
        sous_opt(Scenario::Changeante, Politique::F) < sous_opt(Scenario::Changeante, Politique::D),
        format!(
            "S3 choix sous-optimaux après bascule : F {:.2} vs D {:.2}",
            sous_opt(Scenario::Changeante, Politique::F),
            sous_opt(Scenario::Changeante, Politique::D)
        )
    );
    hypothese!(
        "H8 le crédit temporel récupère, la confiance absolue non",
        sous_opt(Scenario::Changeante, Politique::F) < sous_opt(Scenario::Changeante, Politique::E),
        format!(
            "S3 choix sous-optimaux après bascule : F {:.2} vs E {:.2}",
            sous_opt(Scenario::Changeante, Politique::F),
            sous_opt(Scenario::Changeante, Politique::E)
        )
    );
    hypothese!(
        "H9 la combinaison adaptative des horizons ne dégrade pas la récupération",
        sous_opt(Scenario::Changeante, Politique::H)
            <= sous_opt(Scenario::Changeante, Politique::F),
        format!(
            "S3 choix sous-optimaux après bascule : H {:.2} vs F {:.2}",
            sous_opt(Scenario::Changeante, Politique::H),
            sous_opt(Scenario::Changeante, Politique::F)
        )
    );
    hypothese!(
        "H10 (capacités inédites) la confiance λ coupe le transfert périmé",
        regret_apres(Scenario::CapacitesInedites, Politique::E)
            < regret_apres(Scenario::CapacitesInedites, Politique::C),
        format!(
            "S7 regret après bascule E {:.2} vs C {:.2}",
            regret_apres(Scenario::CapacitesInedites, Politique::E),
            regret_apres(Scenario::CapacitesInedites, Politique::C)
        )
    );
    hypothese!(
        "H11 (capacités inédites) le crédit temporel coupe aussi le transfert périmé",
        regret_apres(Scenario::CapacitesInedites, Politique::F)
            < regret_apres(Scenario::CapacitesInedites, Politique::C),
        format!(
            "S7 regret après bascule F {:.2} vs C {:.2}",
            regret_apres(Scenario::CapacitesInedites, Politique::F),
            regret_apres(Scenario::CapacitesInedites, Politique::C)
        )
    );
    println!(
        "  → {refutees} hypothèse(s) réfutée(s) sur 11 (aucune n'invalide le test : un résultat négatif est un résultat)"
    );
}
