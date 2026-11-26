#![cfg(all(feature = "obrain", feature = "objects-toolbox"))]
//! **L5** — banc natif A/B/C/D : le transfert apprend-il *quelle relation* écouter ?
//!
//! ## La question, et pourquoi elle n'est pas déjà dans `l4_transfert_relationnel`
//!
//! `l4a` compare des **structures de partage** (classe, global, domaine, hiérarchie)
//! et une confiance adaptative **par classe** : λ = 1/(1+contradictions). Le défaut de
//! cette forme est qu'elle ne sait pas *attribuer* : quand deux relations alimentent la
//! même classe et que l'une ment, la contradiction est comptée globalement et la
//! confiance baisse pour toutes — y compris pour celle qui disait vrai.
//!
//! Ici la crédibilité est portée **par relation**, pas par classe. C'est la seule
//! différence entre C et D, et tout le reste du banc est construit pour que cette
//! différence soit le seul degré de liberté.
//!
//! ## Les quatre politiques
//!
//! | | transfert | masse |
//! |---|---|---|
//! | **A** | aucun — mémoire locale de la classe seule | — |
//! | **B** | toutes les observations des autres classes, brassées | plafonnée |
//! | **C** | les voisinages des relations, **poids fixe 1 par relation** | plafonnée |
//! | **D** | les mêmes voisinages, **poids appris par relation** | plafonnée |
//!
//! Les quatre partagent : même corpus, mêmes tirages pré-générés, même plafond de masse
//! (`MASSE_MAX`), même mémoire locale non plafonnée. **C et D ne diffèrent que par le
//! partage de la masse entre relations** : C répartit uniformément, D répartit selon la
//! crédibilité apprise. Le plafond étant appliqué après la répartition, les deux
//! transfèrent *exactement la même quantité* d'information — seule sa provenance change.
//! C'est ce qui rend la comparaison C/D interprétable, et c'est la faute de conception
//! la plus facile à commettre ici : si D avait en plus le droit de transférer moins, on
//! ne saurait pas si son gain vient de l'attribution ou du plafond.
//!
//! ## Ce que D apprend, et sur quel signal
//!
//! Pour chaque couple (classe, relation), un crédit accumule la **qualité prédictive**
//! de la relation sur le bras réellement joué :
//!
//! ```text
//! gain += [ y·ln p + (1−y)·ln(1−p) ] − ln(1/2)      p = taux du voisinage pour ce bras
//! q     = gain / (n + SHRINK)                        (0 sans aucune observation)
//! poids ∝ exp(TEMP · q)                              (softmax sur les relations)
//! ```
//!
//! Trois propriétés voulues : (1) à crédit vide q = 0 donc softmax **uniforme**, et D
//! démarre exactement comme C ; (2) le signal est disponible *hors politique* — on juge
//! la relation sur le bras joué, pas seulement quand on a suivi son conseil, ce qui évite
//! le trou classique « une relation trompeuse qu'on n'écoute jamais n'est jamais jugée » ;
//! (3) aucune lecture de `theta`, aucun lexique : uniquement des comptes (classe, bras,
//! issue).
//!
//! **Limite assumée du signal.** `q` mesure la *calibration* de la relation, pas sa
//! *véracité* : une relation honnête sur une classe difficile est moins bien notée qu'une
//! relation honnête sur une classe facile. Le banc est construit avec des écarts
//! (0,85 contre ~0,27) tels que ce biais ne peut pas inverser l'ordre, mais il existe et
//! il est rapporté plutôt que masqué.
//!
//! ## Les cinq situations exigées, et comment elles sont construites
//!
//! Le monde est une table θ(classe, bras) **explicite** — pas de générateur caché : le
//! corpus *est* la table, donc il est auditable et les assertions de forme ci-dessous
//! peuvent le vérifier.
//!
//! | situation | scénario | construction |
//! |---|---|---|
//! | relation utile | `S1` | une seule relation, `un`, dont chaque voisinage partage le meilleur bras de la classe |
//! | relation trompeuse | `S2` | une seule relation, `deux`, dont **aucun** voisin ne partage le meilleur bras de la classe |
//! | pertinence changeante | `S3` | les deux relations, et le monde **bascule** en phase 1 : `un` devient trompeuse, `deux` devient utile |
//! | deux concurrentes | `S4` | les deux relations, monde figé en phase 0 : `un` utile, `deux` trompeuse |
//! | absence de relation | `S5` | aucune relation dans le graphe |
//!
//! `S3` phase 0 ≡ `S4` par construction : les deux scénarios ne diffèrent que par la
//! bascule, ce qui isole l'effet du changement de pertinence.
//!
//! ## Invariants assertés / hypothèses imprimées
//!
//! Même doctrine que `l4a` — et elle n'est pas décorative : un banc qui assert ses
//! hypothèses refuse les résultats négatifs, et un banc qui n'asserte rien ne prouve
//! rien.
//!
//! *Asserté* (une violation est un défaut de code ou de corpus, jamais un résultat) :
//! forme du corpus (meilleur bras unique et margé, `un` homogène en phase 0, `deux`
//! systématiquement hétérogène en phase 0, l'inverse en phase 1) ; **A est identique dans
//! les cinq scénarios** (A ne lit aucune relation) ; **S5 : A ≡ C ≡ D** (pas de relation
//! ⇒ pas de transfert ⇒ aucune politique ne s'écarte du local) ; **une seule relation ⇒
//! C ≡ D** (le softmax à un élément vaut 1, donc D n'a rien à apprendre) ; **S3/S4 tour 0 :
//! le premier choix de D est celui de C** (crédit vide ⇒ répartition uniforme).
//!
//! *Imprimé avec un verdict, jamais asserté* : le transfert positif de `un`, le transfert
//! **négatif** de `deux`, le gain d'attribution de D sur C là où deux relations
//! coexistent, la conservation du transfert positif par D, et la récupération après
//! bascule. Le résultat peut être négatif : le rapport le dit, et une hypothèse réfutée
//! n'invalide pas le test.
//!
//! ## Bornes
//!
//! 10 classes × 24 tours × 3 graines × 4 politiques × 5 scénarios = 14 400 tours, un
//! magasin Obrain par (scénario, politique, graine) — 60 magasins, tous détruits en fin
//! de test. Les voisinages sont lus **une fois** par scénario et mis en cache : le graphe
//! ne change pas pendant un épisode, seule la table θ change (et elle n'est pas dans le
//! graphe). `settle` réutilise les comptes de voisinage calculés par `arms` du même tour :
//! rien n'est écrit entre les deux, donc la prédiction jugée est bien celle d'avant
//! l'observation.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use futures::executor::block_on;
use obrain_common::types::Value;
use this::storage::obrain::ObrainStore;
use this_agent::Stop;
use this_harness::routing::{ArmKey, ArmStats, Learning, Pick, beta_draw, seed_of};

// ════════════════════════════════════════════════════════════════════════════
// 1. Le monde — une table explicite, deux phases
// ════════════════════════════════════════════════════════════════════════════

const BRAS: [&str; 4] = ["a", "b", "c", "d"];
const MODELE: &str = "m";
const CLASSES: [&str; 10] = ["c0", "c1", "c2", "c3", "c4", "c5", "c6", "c7", "c8", "c9"];

/// θ(classe, bras) en **phase 0**. Chaque ligne a un maximum net : le meilleur bras est
/// unique et son écart au second vaut au moins 0,4 (vérifié par `l5_corpus_est_bien_forme`).
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

/// θ(classe, bras) en **phase 1** (`S3` seulement). Les meilleurs bras sont redistribués
/// de façon que la relation `un` devienne systématiquement trompeuse et `deux` utile.
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

/// Relation `un` : les quatre classes de « a » forment une clique, idem pour les paires
/// b et c ; `c8` et `c9` sont **isolées** (aucun voisin) — le cas « pas de relation pour
/// cette classe » est donc présent dans les scénarios relationnels aussi.
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

/// Relation `deux` : les groupes de la **phase 1** — donc systématiquement trompeuse en
/// phase 0 (aucun voisin ne partage le meilleur bras de la classe) et honnête en phase 1.
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

const MASSE_MAX: f64 = 6.0;
const SHRINK: f64 = 6.0;
const TEMP: f64 = 3.0;
const TOURS: usize = 24;
const T_BASCULE: usize = 12;
const GRAINES: [u64; 3] = [0, 1, 2];
const POINTS: [usize; 5] = [0, 1, 2, 4, 8];

fn idx_classe(c: &str) -> usize {
    CLASSES.iter().position(|x| *x == c).expect("classe connue")
}

fn idx_bras(b: &str) -> usize {
    BRAS.iter().position(|x| *x == b).expect("bras connu")
}

fn theta(phase: usize, classe: &str, bras: &str) -> f64 {
    let t = if phase == 0 { &PHASE0 } else { &PHASE1 };
    t[idx_classe(classe)][idx_bras(bras)]
}

/// Le meilleur bras réel. Réservé à l'évaluateur et aux auto-contrôles : aucune
/// politique ne l'appelle.
fn meilleur_bras(phase: usize, classe: &str) -> &'static str {
    BRAS.iter()
        .copied()
        .max_by(|x, y| theta(phase, classe, x).total_cmp(&theta(phase, classe, y)))
        .expect("un bras")
}

fn voisins(rel: &str, classe: &str) -> &'static [&'static str] {
    let table = match rel {
        "un" => &REL_UN,
        "deux" => &REL_DEUX,
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

/// Tirage Bernoulli de la `j`-ième occurrence du bras sur la classe. L'uniforme tiré ne
/// dépend **pas** de la phase : seule la comparaison au seuil change, donc une bascule de
/// régime garde le même hasard et ne fait varier que le monde.
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

/// Lecture stricte : une réponse malformée ou possiblement tronquée est une panne, jamais
/// un résultat vide. (Motif repris de `l4_transfert_relationnel::lire_lignes`.)
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
/// voisin. La lecture se fait donc en deux sauts, exactement comme le domaine de `l4a`.
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
    // Un voisinage peut légitimement être vide (classe isolée) : `rows` absent et
    // `rows: []` disent la même chose. Ce qui n'est pas toléré, c'est une relecture qui
    // ne corresponde pas à la table — c'est `Banc::nouveau` qui l'assure.
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

/// Voisins d'une classe pour une relation, relus **dans le graphe** (pas dans la table
/// statique) : le banc vérifie ainsi que ce qu'il construit est bien ce qu'il relit.
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
///
/// Le plafond porte sur la masse **totale** transférée : c'est lui qui garantit que C et
/// D transfèrent la même quantité, et donc que seule la *répartition* les distingue.
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

/// Répartition de la masse entre relations.
///
/// C (`Fixe`) : 1 par relation. D (`Appris`) : `n_rel × softmax(TEMP · q)` — la somme
/// vaut `n_rel` dans les deux cas, donc **la masse transférée est identique** et le
/// plafond mord exactement pareil. À crédit vide, `q = 0` partout et le softmax est
/// uniforme : D vaut C, terme pour terme.
fn poids_depuis(
    relations: &[(&'static str, &'static [&'static str])],
    mode: Mode,
    q: &HashMap<&'static str, f64>,
) -> HashMap<&'static str, f64> {
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
                .map(|(rel, _)| (*rel, (TEMP * q.get(rel).copied().unwrap_or(0.0)).exp()))
                .collect();
            let total: f64 = exps.iter().map(|(_, e)| *e).sum();
            let n_rel = relations.len() as f64;
            for (rel, e) in exps {
                w.insert(rel, if total > 0.0 { n_rel * e / total } else { 0.0 });
            }
        }
        _ => {}
    }
    w
}

/// Thompson sampling sur le postérieur — le tirage dépend de `id`, donc de la graine et du
/// tour : deux politiques qui ont les mêmes postérieurs font le même choix.
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
// 5. Les quatre politiques
// ════════════════════════════════════════════════════════════════════════════

#[derive(Clone, Copy, PartialEq, Eq)]
enum Poids {
    /// C — poids identique pour toutes les relations.
    Fixe,
    /// D — poids appris depuis les conséquences observées.
    Appris,
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
}

const POLITIQUES: [(&str, Politique); 4] = [
    ("A local", Politique::A),
    ("B global", Politique::B),
    ("C relat.fixe", Politique::C),
    ("D relat.appris", Politique::D),
];

/// Crédit d'une relation sur une classe : qualité prédictive accumulée.
#[derive(Clone, Copy, Default)]
struct Credit {
    gain: f64,
    n: u32,
}

impl Credit {
    /// `q` vaut 0 sans aucune observation — donc softmax uniforme, donc D ≡ C au départ.
    fn q(&self) -> f64 {
        self.gain / (self.n as f64 + SHRINK)
    }
}

struct Banc {
    store: Arc<ObrainStore>,
    mode: Mode,
    /// classe → [(relation, voisins)], lu une fois dans le graphe.
    voisinages: HashMap<String, Vec<(&'static str, &'static [&'static str])>>,
    credit: Mutex<HashMap<(String, String), Credit>>,
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
                    // La relecture doit coïncider avec la table : sinon le banc mesurerait
                    // autre chose que ce qu'il croit avoir construit.
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
            credit: Mutex::new(HashMap::new()),
            tour: Mutex::new(None),
        }
    }

    /// Poids de chaque relation pour cette classe. C : 1 partout. D : softmax des crédits.
    fn poids(&self, classe: &str) -> HashMap<&'static str, f64> {
        let relations = self.voisinages.get(classe).cloned().unwrap_or_default();
        poids_depuis(&relations, self.mode, &self.credibilite(classe))
    }

    /// Crédibilité `q` par relation pour cette classe (0 partout sans observation).
    fn credibilite(&self, classe: &str) -> HashMap<&'static str, f64> {
        let credit = self.credit.lock().unwrap();
        self.voisinages
            .get(classe)
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .map(|(rel, _)| {
                let q = credit
                    .get(&(rel.to_string(), classe.to_string()))
                    .map(|c| c.q())
                    .unwrap_or(0.0);
                (rel, q)
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

    /// Comptes du tour en cours ; à défaut de cache valide, relecture (plus lente mais
    /// correcte).
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

    /// Poids appris de chaque relation, pour le diagnostic du rapport.
    ///
    /// Ne rappelle jamais `poids` sous le verrou du crédit : `std::sync::Mutex` n'est pas
    /// réentrant, et c'est le genre d'erreur qui se manifeste par un test suspendu, pas
    /// par un message.
    fn credit_de(&self, classe: &str) -> Vec<(&'static str, f64, f64, u32)> {
        let relations = self.voisinages.get(classe).cloned().unwrap_or_default();
        let q = self.credibilite(classe);
        let w = poids_depuis(&relations, self.mode, &q);
        let n = {
            let credit = self.credit.lock().unwrap();
            relations
                .iter()
                .map(|(rel, _)| {
                    (
                        *rel,
                        credit
                            .get(&(rel.to_string(), classe.to_string()))
                            .map(|c| c.n)
                            .unwrap_or(0),
                    )
                })
                .collect::<HashMap<&'static str, u32>>()
        };
        relations
            .iter()
            .map(|(rel, _)| {
                (
                    *rel,
                    q.get(rel).copied().unwrap_or(0.0),
                    w.get(rel).copied().unwrap_or(0.0),
                    n.get(rel).copied().unwrap_or(0),
                )
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
                let poids = self.poids(classe);
                // Masse de référence : celle de C, poids 1 partout.
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
                // Renormalisation sur la masse de C : D ne transfère ni plus ni moins, il
                // transfère *ailleurs*. Sans ce facteur, une relation sans données (une
                // classe isolée dans cette relation) ferait mécaniquement transférer moins
                // à D qu'à C — et un « gain » de D pourrait n'être qu'un transfert plus
                // faible, pas une meilleure attribution.
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

        // Jugement AVANT écriture : la relation prédisait-elle l'issue de CE bras ?
        // (hors politique : on juge sur le bras joué, pas seulement sur son conseil)
        if self.mode == Mode::Relationnel(Poids::Appris) {
            let par_relation = self.comptes_du_tour(classe);
            let mut credit = self.credit.lock().unwrap();
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
                let e = credit
                    .entry((rel.to_string(), classe.to_string()))
                    .or_default();
                e.gain += ll - 0.5f64.ln();
                e.n += 1;
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
    Absente,
}

const SCENARIOS: [(&str, Scenario); 5] = [
    ("S1 relation utile", Scenario::Utile),
    ("S2 relation trompeuse", Scenario::Trompeuse),
    ("S3 pertinence changeante", Scenario::Changeante),
    ("S4 deux concurrentes", Scenario::Concurrentes),
    ("S5 absence de relation", Scenario::Absente),
];

impl Scenario {
    fn relations(self) -> &'static [&'static str] {
        match self {
            Scenario::Utile => &["un"],
            Scenario::Trompeuse => &["deux"],
            Scenario::Changeante | Scenario::Concurrentes => &["un", "deux"],
            Scenario::Absente => &[],
        }
    }

    fn bascule(self) -> Option<usize> {
        match self {
            Scenario::Changeante => Some(T_BASCULE),
            _ => None,
        }
    }

    fn phase(self, tour: usize) -> usize {
        match self.bascule() {
            Some(t) if tour >= t => 1,
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
        }
    }
}

fn dir_unique(etiquette: &str) -> std::path::PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    static N: AtomicU64 = AtomicU64::new(0);
    let d = std::env::temp_dir().join(format!(
        "l5_{etiquette}_{}_{}",
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
    // Le graphe n'est bâti que pour les politiques qui le lisent : c'est aussi une preuve
    // que A et B ne dépendent pas des relations.
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
    /// Choix optimal (au sens de θ) sur l'ensemble des tours et des classes.
    choix_optimaux: f64,
    total: f64,
    /// Choix optimal au tour 0, par classe.
    succes_t0: f64,
    /// Choix optimal au tour `POINTS[j]`.
    succes_k: [f64; POINTS.len()],
    /// Regret cumulé, moyenné par classe.
    regret: f64,
    /// Regret cumulé après la bascule, moyenné par classe (`S3` seulement).
    regret_apres: f64,
    /// Nombre de choix sous-optimaux après la bascule, moyenné par classe.
    sous_opt_apres: f64,
    /// Classes dont les tours post-bascule finissent durablement optimaux.
    recup_taux: f64,
    /// Médiane du premier tour durablement optimal, parmi les classes récupérées.
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

/// Vérifie que le corpus est bien celui que les cinq situations décrivent. Un corpus
/// malformé rendrait le banc muet (toutes les politiques identiques) sans qu'aucun test ne
/// rougisse — c'est précisément ce que cette vérification empêche.
#[test]
fn l5_corpus_est_bien_forme() {
    // Meilleur bras unique, avec une marge : le regret n'est pas du bruit d'arrondi.
    for phase in 0..2 {
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

    // S1 : `un` est UTILE en phase 0 — chaque voisin partage le meilleur bras de la classe.
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

    // S2 : `deux` est TROMPEUSE en phase 0 — aucun voisin ne partage le meilleur bras.
    for c in CLASSES {
        let best = meilleur_bras(0, c);
        assert!(
            !voisins("deux", c).is_empty(),
            "relation `deux` : {c} doit avoir au moins un voisin pour être trompée"
        );
        for v in voisins("deux", c) {
            assert_ne!(
                meilleur_bras(0, v),
                best,
                "relation `deux` en phase 0 : le voisin {v} de {c} partage son meilleur bras — la relation n'est pas trompeuse"
            );
        }
    }

    // S3 : les rôles s'INVERSENT en phase 1.
    for c in CLASSES {
        let best = meilleur_bras(1, c);
        for v in voisins("deux", c) {
            assert_eq!(
                meilleur_bras(1, v),
                best,
                "relation `deux` en phase 1 : le voisin {v} de {c} ne partage pas son meilleur bras — elle devrait être utile"
            );
        }
        for v in voisins("un", c) {
            assert_ne!(
                meilleur_bras(1, v),
                best,
                "relation `un` en phase 1 : le voisin {v} de {c} partage son meilleur bras — elle devrait être trompeuse"
            );
        }
    }

    // Les relations sont des appartenances : symétriques, sans boucle.
    for rel in ["un", "deux"] {
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

    // `c8` et `c9` n'ont aucun voisin dans `un` : le cas « pas de relation pour cette
    // classe » existe aussi à l'intérieur des scénarios relationnels.
    for c in ["c8", "c9"] {
        assert!(
            voisins("un", c).is_empty(),
            "{c} doit être isolée dans `un`"
        );
        assert!(
            !voisins("deux", c).is_empty(),
            "{c} doit avoir des voisins dans `deux`"
        );
    }
}

// ════════════════════════════════════════════════════════════════════════════
// 9. Le banc
// ════════════════════════════════════════════════════════════════════════════

#[test]
fn l5_transfert_adaptatif() {
    let mut trajs: HashMap<(Scenario, Politique, u64), Traj> = HashMap::new();
    let mut mesures: HashMap<(Scenario, Politique), Vec<Mesures>> = HashMap::new();
    // Magnitude médiane de |q| à la fin d'un épisode de D — mesure la DILUTION du crédit.
    let mut dilution: HashMap<Scenario, Vec<f64>> = HashMap::new();
    for (_, scenario) in SCENARIOS {
        for (_, politique) in POLITIQUES {
            let mut par_graine = Vec::new();
            for graine in GRAINES {
                let (traj, credits) = {
                    let (banc, nettoyage) = instancier(scenario, politique, graine);
                    let traj = jouer(&banc, scenario, graine);
                    let credits = if politique == Politique::D {
                        CLASSES
                            .iter()
                            .map(|c| (*c, banc.credit_de(c)))
                            .collect::<Vec<_>>()
                    } else {
                        Vec::new()
                    };
                    drop(banc);
                    liberer(nettoyage);
                    (traj, credits)
                };
                par_graine.push(mesurer(&traj, scenario));
                if politique == Politique::D {
                    let qs: Vec<f64> = credits
                        .iter()
                        .flat_map(|(_, cr)| {
                            cr.iter()
                                .filter(|(_, _, _, n)| *n > 0)
                                .map(|(_, q, _, _)| q.abs())
                        })
                        .collect();
                    dilution
                        .entry(scenario)
                        .or_default()
                        .push(mediane(&qs).unwrap_or(f64::NAN));
                }
                if graine == GRAINES[0] && politique == Politique::D {
                    println!(
                        "  · crédits appris · {} · graine {graine}",
                        nom_scenario(scenario)
                    );
                    for (c, cr) in &credits {
                        let detail = cr
                            .iter()
                            .map(|(rel, q, w, n)| format!("{rel}: q={q:+.3} w={w:.3} n={n}"))
                            .collect::<Vec<_>>()
                            .join("  ");
                        println!("      {c: <3} {detail}");
                    }
                }
                trajs.insert((scenario, politique, graine), traj);
            }
            mesures.insert((scenario, politique), par_graine);
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
    // Part des décisions où C et D diffèrent. Sans cette colonne, deux politiques qui ont
    // la même médiane passeraient pour identiques alors qu'elles choisissent autrement —
    // et l'inverse (médianes différentes pour des trajectoires identiques) est impossible.
    let divergence = |s: Scenario| -> f64 {
        let v: Vec<f64> = GRAINES
            .iter()
            .map(|g| {
                let c = &trajs[&(s, Politique::C, *g)];
                let d = &trajs[&(s, Politique::D, *g)];
                let mut diff = 0.0;
                let mut tot = 0.0;
                for (tc, td) in c.iter().zip(d.iter()) {
                    for (a, b) in tc.iter().zip(td.iter()) {
                        tot += 1.0;
                        if a.0 != b.0 {
                            diff += 1.0;
                        }
                    }
                }
                diff / tot
            })
            .collect();
        mediane(&v).unwrap_or(f64::NAN)
    };

    for (_, scenario) in SCENARIOS {
        let change = scenario.bascule().is_some();
        println!("===== L5 · {} =====", nom_scenario(scenario));
        println!(
            "  {: <15} {: >6} {: >6} {: >6} {: >7} {: >8} {: >11} {: >11} {: >11} {: >7}",
            "politique",
            "t0",
            "k=4",
            "k=8",
            "succès",
            "regret",
            "Δregret / A",
            "sous-opt. t>",
            "récup.",
            "C↔D"
        );
        let div = divergence(scenario);
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
                "  {: <15} {: >6} {: >6} {: >6} {: >7} {: >8} {: >+11} {: >11} {: >11} {: >7}",
                nom_politique(p),
                fmt(t0, 2),
                fmt(k4, 2),
                fmt(k8, 2),
                fmt(suc, 3),
                fmt(reg, 2),
                d,
                sous,
                recup,
                fmt(div, 3)
            );
        }
    }
    println!("  (t0 / k=4 / k=8 : choix optimal après exactement 0, 4 et 8 observations ;");
    println!(
        "   succès : sur tous les tours ; Δregret/A : médiane par graine, > 0 = transfert NÉGATIF)"
    );
    println!("==================================================================");

    // ── Diagnostic : où C et D divergent-ils RÉELLEMENT ? ──────────────────
    // `C↔D` dans les tableaux est une médiane : elle peut valoir 0 sans que les poids
    // appris soient restés uniformes. Le compte par graine, lui, ne ment pas.
    println!("===== L5 · divergences C↔D (décisions différentes / total, par graine) =====");
    for (_, scenario) in SCENARIOS {
        let compte: Vec<String> = GRAINES
            .iter()
            .map(|g| {
                let c = &trajs[&(scenario, Politique::C, *g)];
                let d = &trajs[&(scenario, Politique::D, *g)];
                let mut diff = 0;
                let mut tot = 0;
                for (tc, td) in c.iter().zip(d.iter()) {
                    for (a, b) in tc.iter().zip(td.iter()) {
                        tot += 1;
                        if a.0 != b.0 {
                            diff += 1;
                        }
                    }
                }
                format!("{diff}/{tot}")
            })
            .collect();
        println!("  {: <26} {}", nom_scenario(scenario), compte.join("   "));
    }
    println!("  |q| médian du crédit appris à la fin (D) : combien le crédit ose s'écarter de 0");
    for (_, scenario) in SCENARIOS {
        let v = dilution.get(&scenario).cloned().unwrap_or_default();
        println!(
            "    {: <26} {}",
            nom_scenario(scenario),
            fmt(mediane(&v).unwrap_or(f64::NAN), 3)
        );
    }
    println!("==================================================================");

    // ── INVARIANTS (assertés) ─────────────────────────────────────────────
    // 1. A ne lit aucune relation : sa trajectoire ne dépend pas des relations du
    //    scénario. Les quatre scénarios comparés partagent le MÊME monde (phase 0) et les
    //    mêmes tirages ; S3 en est exclu — non parce que A y lirait une relation, mais
    //    parce que son monde bascule, donc ses issues observées diffèrent. Ce que S3
    //    démontre est vérifié par `l5_corpus_est_bien_forme` : les rôles des relations
    //    s'y inversent réellement.
    for graine in GRAINES {
        let reference = trajs[&(Scenario::Utile, Politique::A, graine)].clone();
        for scenario in [
            Scenario::Trompeuse,
            Scenario::Concurrentes,
            Scenario::Absente,
        ] {
            assert_eq!(
                reference,
                trajs[&(scenario, Politique::A, graine)],
                "INVARIANT : graine {graine} — la politique A change de trajectoire selon le scénario ({}) alors que le monde et les tirages sont les mêmes : elle lit une relation, ce qu'elle ne doit pas faire",
                nom_scenario(scenario)
            );
        }
    }

    // 2. Pas de relation ⇒ pas de transfert relationnel ⇒ C et D ne s'écartent pas du
    //    local. B en est exclu : son transfert est GLOBAL, il ne dépend d'aucune relation
    //    et doit donc continuer de transférer — c'est même la preuve qu'il ne lit pas le
    //    graphe. C'est bien pour cela que `S5 B` vaut `S1 B` et diffère de `A`.
    for graine in GRAINES {
        let a = trajs[&(Scenario::Absente, Politique::A, graine)].clone();
        for p in [Politique::C, Politique::D] {
            assert_eq!(
                a,
                trajs[&(Scenario::Absente, p, graine)],
                "INVARIANT : graine {graine} — {} transfère alors qu'aucune relation n'existe",
                nom_politique(p)
            );
        }
    }

    // 3. Une seule relation ⇒ le softmax à un élément vaut 1 ⇒ D n'a rien à apprendre.
    for scenario in [Scenario::Utile, Scenario::Trompeuse] {
        for graine in GRAINES {
            assert_eq!(
                trajs[&(scenario, Politique::C, graine)],
                trajs[&(scenario, Politique::D, graine)],
                "INVARIANT : graine {graine} — C et D divergent sur {} alors qu'il n'y a qu'une relation",
                nom_scenario(scenario)
            );
        }
    }

    // 4. Crédit vide ⇒ répartition uniforme ⇒ D choisit comme C au premier tour.
    for scenario in [Scenario::Changeante, Scenario::Concurrentes] {
        for graine in GRAINES {
            let c = &trajs[&(scenario, Politique::C, graine)];
            let d = &trajs[&(scenario, Politique::D, graine)];
            for i in 0..CLASSES.len() {
                assert_eq!(
                    c[i][0].0,
                    d[i][0].0,
                    "INVARIANT : graine {graine}, {} — le premier choix de D ({}) diffère de C ({}) sur {} : le crédit n'est pas vide au départ",
                    nom_scenario(scenario),
                    d[i][0].0,
                    c[i][0].0,
                    CLASSES[i]
                );
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

    println!("===== L5 · hypothèses (verdicts, aucune assertion) =====");
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
        "H4 D conserve le transfert positif au lieu de tout couper",
        regret(Scenario::Concurrentes, Politique::D) < regret(Scenario::Concurrentes, Politique::A),
        format!(
            "S4 regret D {:.2} vs A {:.2} (si D ≥ A, son gain sur C vient d'avoir tué le transfert)",
            regret(Scenario::Concurrentes, Politique::D),
            regret(Scenario::Concurrentes, Politique::A)
        )
    );
    hypothese!(
        "H5 D est plus souvent optimal que C après 8 observations",
        val(Scenario::Concurrentes, Politique::D, &|m: &Mesures| m
            .succes_k[4])
            >= val(Scenario::Concurrentes, Politique::C, &|m: &Mesures| m
                .succes_k[4]),
        format!(
            "S4 k=8 D {:.3} vs C {:.3}",
            val(Scenario::Concurrentes, Politique::D, &|m: &Mesures| m
                .succes_k[4]),
            val(Scenario::Concurrentes, Politique::C, &|m: &Mesures| m
                .succes_k[4])
        )
    );
    hypothese!(
        "H6 après bascule, D se trompe moins souvent que C",
        sous_opt(Scenario::Changeante, Politique::D) < sous_opt(Scenario::Changeante, Politique::C),
        format!(
            "S3 choix sous-optimaux après t={T_BASCULE} : D {:.2} vs C {:.2} (A {:.2})",
            sous_opt(Scenario::Changeante, Politique::D),
            sous_opt(Scenario::Changeante, Politique::C),
            sous_opt(Scenario::Changeante, Politique::A)
        )
    );
    hypothese!(
        "H7 le regret post-bascule de D est inférieur à celui de C",
        val(Scenario::Changeante, Politique::D, &|m: &Mesures| m
            .regret_apres)
            < val(Scenario::Changeante, Politique::C, &|m: &Mesures| m
                .regret_apres),
        format!(
            "S3 regret après bascule D {:.2} vs C {:.2} (A {:.2})",
            val(Scenario::Changeante, Politique::D, &|m: &Mesures| m
                .regret_apres),
            val(Scenario::Changeante, Politique::C, &|m: &Mesures| m
                .regret_apres),
            val(Scenario::Changeante, Politique::A, &|m: &Mesures| m
                .regret_apres)
        )
    );
    hypothese!(
        "H8 l'attribution reste payante quand la pertinence change",
        regret(Scenario::Changeante, Politique::D) < regret(Scenario::Changeante, Politique::C),
        format!(
            "S3 regret D {:.2} vs C {:.2} — l'inertie du crédit peut coûter plus que l'uniforme",
            regret(Scenario::Changeante, Politique::D),
            regret(Scenario::Changeante, Politique::C)
        )
    );
    hypothese!(
        "H9 le crédit cumulé est DILUÉ par le changement de pertinence",
        mediane(&dilution[&Scenario::Changeante]).unwrap_or(f64::NAN)
            < mediane(&dilution[&Scenario::Concurrentes]).unwrap_or(f64::NAN),
        format!(
            "|q| médian final : S3 {:.3} vs S4 {:.3} — un crédit sans oubli moyenne les deux régimes et revient vers 0, donc vers l'uniforme, donc vers C",
            mediane(&dilution[&Scenario::Changeante]).unwrap_or(f64::NAN),
            mediane(&dilution[&Scenario::Concurrentes]).unwrap_or(f64::NAN)
        )
    );
    println!(
        "  → {refutees} hypothèse(s) réfutée(s) sur 9 (aucune n'invalide le test : un résultat négatif est un résultat)"
    );
}
