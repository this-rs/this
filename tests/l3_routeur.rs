#![cfg(all(feature = "obrain", feature = "objects-toolbox"))]
//! **L3 branché sur le port `Learning`** — un apprenant PERSISTANT dans Obrain,
//! piloté par le routeur natif, avant et après réouverture.
//!
//! Événements = source de vérité. `arms()` est une **projection recalculée** depuis
//! les observations du graphe, jamais un compteur opaque.
//!
//! Périmètre de concurrence : un seul écrivain, un seul processus, une seule
//! instance de store ouverte. Aucune garantie inter-thread/inter-processus n'est
//! revendiquée ici.
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use obrain_common::types::Value;
use this::storage::obrain::ObrainStore;
use this_agent::Stop;
use this_harness::routing::{
    ArmKey, ArmStats, Capabilities, CostBasis, Decision, Learning, MemoryPool, Pick, PriorHints,
    RouteWhy, RouterSpec, RoutingSettings, SequenceSeeds, TurnInput, TurnRouter,
};

fn executer<F: std::future::Future<Output = T>, T>(f: F) -> T {
    futures::executor::block_on(f)
}

/// Échappe une valeur pour une requête GQL interpolée.
/// `execute_query(lang, requete, limite)` n'offre pas de paramètres : l'échappement
/// est donc la seule barrière, et il est **documenté comme tel**.
fn echapper(s: &str) -> String {
    s.replace('\\', "\\\\").replace('\'', "\\'")
}

/// Clé d'événement NON AMBIGUË : chaque champ est préfixé par sa longueur.
fn cle_evenement(scope: &str, decision: &str) -> String {
    format!("{}:{}|{}:{}", scope.len(), scope, decision.len(), decision)
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

/// Apprenant persistant : écrit une observation par `settle`, relit les
/// statistiques par projection GQL.
/// Résultat d'une tentative d'écriture d'observation.
/// **Bilan exhaustif par décision** : chaque décision tombe dans EXACTEMENT un seau.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
struct Compteurs {
    /// `record` appelé avec une paire servie.
    decisions: u64,
    /// `record` appelé sans paire (décision consultative).
    consultatives: u64,
    /// `settle` reçu sans `record` correspondant (identifiant absent de `en_cours`).
    id_absent: u64,
    /// `settle` reçu avec `served = None` (aucun moteur servi).
    served_none: u64,
    /// `settle` reçu avec un `Stop` non exploitable (Interrupted, Budget, …).
    stop_non_exploitable: u64,
    /// Observation effectivement écrite.
    creees: u64,
    rejeux: u64,
    conflits: u64,
    /// Échec d'écriture (persistance) : le contrôle d'entrée d'écriture a refusé,
    /// ou la transaction a échoué. **Rien n'a été écrit** — ce n'est pas une
    /// création, ce n'est pas un rejeu, et la décision ne doit pas disparaître
    /// silencieusement de la comptabilité.
    erreurs: u64,
}

#[derive(Debug, PartialEq, Eq)]
enum Ecriture {
    Creee,
    RejeuIdentique,
    Conflit,
    /// L'écriture a été refusée ou a échoué : comptée en `erreurs`, jamais en `creees`.
    Erreur,
}

struct ApprisObrain {
    store: Arc<ObrainStore>,
    scope: String,
    /// Identité de SESSION, durable et indépendante des graines d'exploration :
    /// deux exécutions du même plan produisent des identifiants différents.
    session: String,
    candidats: Vec<(String, String)>,
    en_cours: Mutex<HashMap<u128, String>>, // decision_id -> classe
    compteurs: Mutex<Compteurs>,
}

impl ApprisObrain {
    fn nouveau(store: Arc<ObrainStore>, scope: &str, candidats: Vec<(String, String)>) -> Self {
        let session = Self::allouer_session(&store, scope);
        Self {
            store,
            scope: scope.to_string(),
            session,
            candidats,
            en_cours: Mutex::new(HashMap::new()),
            compteurs: Mutex::new(Compteurs::default()),
        }
    }

    /// Alloue un numéro de session DURABLE par scope : `max(numero) + 1`.
    /// L'identité d'un événement ne dépend donc PAS des graines du routeur.
    fn allouer_session(store: &Arc<ObrainStore>, scope: &str) -> String {
        let r = store
            .execute_query(
                "gql",
                &format!(
                    "MATCH (s:session) WHERE s.scope = '{}' RETURN s.numero AS n",
                    echapper(scope)
                ),
                100_000,
            )
            .expect("lecture des sessions")
            .get("rows")
            .and_then(|v| v.as_array())
            .cloned()
            .expect("réponse GQL sans `rows`");
        let max = r
            .iter()
            .filter_map(|l| l.get(0).and_then(|v| v.as_i64()))
            .max()
            .unwrap_or(0);
        let numero = max + 1;
        let mut s = store.db().session();
        s.begin_transaction().expect("begin");
        let n = s.create_node(&["session"]);
        s.set_node_property(n, "scope", Value::String(scope.into()));
        s.set_node_property(n, "numero", Value::Int64(numero));
        s.commit().expect("commit session");
        numero.to_string()
    }

    fn compteurs(&self) -> Compteurs {
        *self.compteurs.lock().unwrap()
    }

    /// Compte INDÉPENDANT des identités réellement persistées (requête GQL).
    fn identites_persistees(&self) -> usize {
        self.store
            .execute_query(
                "gql",
                &format!(
                    "MATCH (o:observation) WHERE o.scope = '{}' RETURN o.evenement AS e",
                    echapper(&self.scope)
                ),
                100_000,
            )
            .expect("comptage des identités")
            .get("rows")
            .and_then(|v| v.as_array())
            .map(|a| a.len())
            .expect("réponse GQL sans `rows`")
    }

    fn ecrire_observation(
        &self,
        decision: &str,
        classe: &str,
        provider: &str,
        modele: &str,
        favorable: bool,
    ) -> Ecriture {
        // **Le contrôle d'entrée d'écriture**, à passer avant d'écrire par nos propres
        // moyens : c'est l'usage documenté de `ObrainStore::check_write` pour un
        // écrivain qui n'est pas un service de données. Un écrivain périmé (bail
        // repris par un autre) est refusé au lieu d'écrire dans son dos.
        if let Err(refus) = self.store.check_write() {
            eprintln!("observation NON écrite (écriture refusée) : {refus}");
            self.compteurs.lock().unwrap().erreurs += 1;
            return Ecriture::Erreur;
        }
        let evenement = cle_evenement(&format!("{}/{}", self.scope, self.session), decision);
        let existe = self
            .store
            .execute_query(
                "gql",
                &format!(
                    "MATCH (o:observation) WHERE o.evenement = '{}' RETURN o.provider AS p, o.modele AS m, o.favorable AS f, o.classe AS c",
                    echapper(&evenement)
                ),
                1,
            )
            .expect("lecture de déduplication")
            .get("rows")
            .and_then(|v| v.as_array())
            .map(|a| !a.is_empty())
            .expect("réponse GQL sans `rows` : on ne l'assimile PAS à une absence");
        if existe {
            // Identifiant déjà présent : comparer le CONTENU pour distinguer un rejeu
            // identique d'un conflit (même identité, contenu incompatible).
            let ligne = self
                .store
                .execute_query(
                    "gql",
                    &format!(
                        "MATCH (o:observation) WHERE o.evenement = '{}' RETURN o.provider AS p, o.modele AS m, o.favorable AS f, o.classe AS c",
                        echapper(&evenement)
                    ),
                    1,
                )
                .expect("relecture de l'observation existante")
                .get("rows")
                .and_then(|v| v.as_array())
                .cloned()
                .expect("réponse GQL sans `rows`")
                .into_iter()
                .next()
                .expect("ligne attendue");
            let sp = ligne.get(0).and_then(|v| v.as_str()).unwrap_or_default();
            let sm = ligne.get(1).and_then(|v| v.as_str()).unwrap_or_default();
            let sf = ligne.get(2).and_then(|v| v.as_bool()).unwrap_or(false);
            let sc = ligne.get(3).and_then(|v| v.as_str()).unwrap_or_default();
            if sp == provider && sm == modele && sf == favorable && sc == classe {
                self.compteurs.lock().unwrap().rejeux += 1;
                return Ecriture::RejeuIdentique;
            }
            self.compteurs.lock().unwrap().conflits += 1;
            return Ecriture::Conflit;
        }
        let ecriture = (|| -> Result<(), String> {
            let mut s = self.store.db().session();
            s.begin_transaction()
                .map_err(|e| format!("begin : {e:?}"))?;
            let n = s.create_node(&["observation"]);
            s.set_node_property(n, "evenement", Value::String(evenement.into()));
            s.set_node_property(n, "scope", Value::String(self.scope.clone().into()));
            s.set_node_property(n, "classe", Value::String(classe.into()));
            s.set_node_property(n, "provider", Value::String(provider.into()));
            s.set_node_property(n, "modele", Value::String(modele.into()));
            s.set_node_property(n, "favorable", Value::Bool(favorable));
            s.commit().map_err(|e| format!("commit : {e:?}"))?;
            Ok(())
        })();
        match ecriture {
            Ok(()) => {
                self.compteurs.lock().unwrap().creees += 1;
                Ecriture::Creee
            }
            // Une panne de persistance est une DONNÉE, pas un arrêt du monde : elle
            // est comptée, nommée, et l'observation n'est pas déclarée créée.
            Err(panne) => {
                eprintln!("observation NON écrite (échec de transaction) : {panne}");
                self.compteurs.lock().unwrap().erreurs += 1;
                Ecriture::Erreur
            }
        }
    }

    /// Projection : (classe, provider, model) -> (alpha, beta, n), a priori 1/1.
    fn projection(&self) -> Vec<(String, String, String, u32, u32, u32)> {
        let r = self
            .store
            .execute_query(
                "gql",
                &format!(
                    "MATCH (o:observation) WHERE o.scope = '{}' RETURN o.classe AS c, o.provider AS p, o.modele AS m, o.favorable AS f",
                    echapper(&self.scope)
                ),
                100_000,
            )
            .expect("projection GQL");
        assert!(
            r.get("truncated").and_then(|v| v.as_bool()) != Some(true),
            "réponse GQL TRONQUÉE : la projection serait fausse"
        );
        let cols: Vec<String> = r
            .get("columns")
            .and_then(|c| c.as_array())
            .expect("réponse GQL sans `columns`")
            .iter()
            .map(|v| v.as_str().expect("colonne non textuelle").to_string())
            .collect();
        let idx = |nom: &str| {
            cols.iter()
                .position(|c| c == nom)
                .unwrap_or_else(|| panic!("colonne {nom} absente"))
        };
        let (ic, ip, im, iff) = (idx("c"), idx("p"), idx("m"), idx("f"));
        let mut t: std::collections::BTreeMap<(String, String, String), (u32, u32, u32)> =
            Default::default();
        for l in r
            .get("rows")
            .and_then(|v| v.as_array())
            .expect("réponse GQL sans `rows`")
        {
            let g = |i: usize| {
                l.get(i)
                    .and_then(|v| v.as_str())
                    .unwrap_or_else(|| panic!("colonne {i} non textuelle"))
                    .to_string()
            };
            let fav = l
                .get(iff)
                .and_then(|v| v.as_bool())
                .expect("colonne `f` non booléenne");
            let e = t.entry((g(ic), g(ip), g(im))).or_insert((1, 1, 0));
            e.2 += 1;
            if fav {
                e.0 += 1;
            } else {
                e.1 += 1;
            }
        }
        t.into_iter()
            .map(|((c, p, m), (a, b, n))| (c, p, m, a, b, n))
            .collect()
    }
}

#[async_trait(?Send)]
impl Learning for ApprisObrain {
    async fn arms(&self, class: &str) -> Vec<ArmStats> {
        // Parité avec L2 : TOUS les candidats sont présents, à l'a priori neutre 1/1
        // s'ils n'ont pas encore été observés. `fresh` met `n = 0` : on restaure le
        // compteur d'observations effectif depuis la projection.
        let projection = self.projection();
        let mut stats: Vec<ArmStats> = Vec::new();
        for (p, m) in &self.candidats {
            let (a, b, n) = projection
                .iter()
                .find(|(c, pp, mm, _, _, _)| c == class && pp == p && mm == m)
                .map(|(_, _, _, a, b, n)| (*a as f64, *b as f64, *n as u64))
                .unwrap_or((1.0, 1.0, 0));
            let mut st = ArmStats::fresh(ArmKey::new(class, p.clone(), m.clone()), a, b);
            st.n = n;
            stats.push(st);
        }
        stats
    }

    fn hints(&self) -> PriorHints {
        PriorHints::new()
    }

    async fn record(&self, d: &Decision) {
        let mut c = self.compteurs.lock().unwrap();
        if d.used.is_some() {
            c.decisions += 1;
            self.en_cours.lock().unwrap().insert(d.id, d.class_key());
        } else {
            c.consultatives += 1;
        }
    }

    async fn settle(&self, id: u128, served: Option<&Pick>, stop: &Stop, _code: Option<&str>) {
        let Some(classe) = self.en_cours.lock().unwrap().remove(&id) else {
            self.compteurs.lock().unwrap().id_absent += 1;
            return;
        };
        let Some(p) = served else {
            self.compteurs.lock().unwrap().served_none += 1;
            return;
        };
        let favorable = match stop {
            Stop::Done => true,
            Stop::Failed(_) => false,
            _ => {
                self.compteurs.lock().unwrap().stop_non_exploitable += 1;
                return; // résultat inconnu : aucun bras nourri
            }
        };
        match self.ecrire_observation(
            &id.to_string(),
            &classe,
            &p.provider_id,
            &p.model,
            favorable,
        ) {
            Ecriture::Creee | Ecriture::RejeuIdentique => {}
            // La décision EST réglée (le `settle` est arrivé) ; c'est l'écriture qui a
            // échoué. Elle est comptée en `erreurs`, donc la comptabilité reste
            // exhaustive, et on ne la maquille ni en création ni en rejeu.
            Ecriture::Erreur => {}
            Ecriture::Conflit => {
                // Même identité, contenu différent : ce n'est pas un rejeu, c'est un
                // conflit. On le voit, on ne l'avale pas.
                panic!("conflit d'observation : même identité, contenu incompatible");
            }
        }
    }
}

fn parametres() -> RoutingSettings {
    RoutingSettings {
        exploration_epsilon: 0.1,
        ..Default::default()
    }
}

#[test]
fn l3_routeur_natif_avant_et_apres_reouverture() {
    let d = std::env::temp_dir().join(format!("l3r_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    let scope = "conv-l3";
    let candidats: Vec<(String, String)> = vec![
        ("a".to_string(), "m1".to_string()),
        ("b".to_string(), "m2".to_string()),
    ];

    let stats_avant;
    let sur_a_avant;
    let premier_id: String;
    {
        let store = ObrainStore::open(&d).expect("ouverture");
        let appris = Rc::new(ApprisObrain::nouveau(
            store.clone(),
            scope,
            candidats.clone(),
        ));
        let spec = RouterSpec::new(
            Rc::new(MemoryPool::new(vec![
                candidat("a", "m1"),
                candidat("b", "m2"),
            ])),
            Rc::new(SequenceSeeds::from(1u128)),
            "a".to_string(),
            "m1".to_string(),
        )
        .learning(appris.clone())
        .auto()
        .settings(parametres());
        let routeur = TurnRouter::new(spec);

        // tirages pré-générés par (tour, candidat) : indépendants de la stratégie
        let mut rng = 1u64;
        let mut tirage = Vec::new();
        for _ in 0..120 {
            let mut l = [false, false];
            for (c, case) in l.iter_mut().enumerate() {
                rng = rng
                    .wrapping_mul(6364136223846793005)
                    .wrapping_add(1442695040888963407);
                *case = (rng % 100) < if c == 0 { 80 } else { 20 };
            }
            tirage.push(l);
        }

        // Première décision réellement APPLIQUÉE (donc première observation
        // écrite), avec son identité et son contenu : c'est elle qu'on rejouera.
        // (identifiant, classe, fournisseur, modèle, favorable)
        let mut premiere: Option<(String, String, String, String, bool)> = None;
        sur_a_avant = {
            let mut n = 0usize;
            for ligne in tirage.iter() {
                let routed = executer(routeur.route(TurnInput {
                    text: "hello",
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
                let idx = if used.provider_id == "a" { 0 } else { 1 };
                let ok = ligne[idx];
                if idx == 0 {
                    n += 1;
                }
                if premiere.is_none() && dec.applied {
                    premiere = Some((
                        dec.id.to_string(),
                        dec.signature.arm_key(),
                        used.provider_id.clone(),
                        used.model.clone(),
                        ok,
                    ));
                }
                executer(routeur.record(&dec));
                let stop = if ok {
                    Stop::Done
                } else {
                    Stop::Failed("transport".to_string())
                };
                executer(routeur.settle(&stop, None, true));
            }
            n
        };
        let (id_p, classe_p, prov_p, mod_p, fav_p) =
            premiere.expect("au moins une décision appliquée");
        premier_id = id_p.clone();

        // ── Rejeu dans la SESSION ORIGINALE ──────────────────────────────────
        // Même store, même apprenant, même routeur, même session durable : on
        // rejoue une décision DÉJÀ écrite, avec un contenu identique. C'est le cas
        // que la continuité après fermeture/réouverture ne mesure pas — là, la
        // session change et tout redevient une expérience nouvelle.
        let projection_avant_rejeu = appris.projection();
        let compteurs_avant_rejeu = appris.compteurs();
        assert_eq!(
            appris.ecrire_observation(&id_p, &classe_p, &prov_p, &mod_p, fav_p),
            Ecriture::RejeuIdentique,
            "même session + même identité + même contenu => rejeu RECONNU"
        );
        assert_eq!(
            appris.projection(),
            projection_avant_rejeu,
            "un rejeu ne modifie pas la projection"
        );
        let compteurs_rejeu = appris.compteurs();
        assert_eq!(
            compteurs_rejeu.rejeux,
            compteurs_avant_rejeu.rejeux + 1,
            "le rejeu est compté comme rejeu"
        );
        assert_eq!(
            compteurs_rejeu.creees, compteurs_avant_rejeu.creees,
            "…et il ne crée rien"
        );
        // Même identité, contenu incompatible : ce n'est pas un rejeu.
        assert_eq!(
            appris.ecrire_observation(&id_p, &classe_p, &prov_p, &mod_p, !fav_p),
            Ecriture::Conflit,
            "même identité + contenu différent => conflit, dans la session aussi"
        );

        stats_avant = appris.projection();
        assert_eq!(
            stats_avant, projection_avant_rejeu,
            "ni le rejeu ni le conflit ne changent la projection"
        );
        assert!(
            !stats_avant.is_empty(),
            "des observations doivent avoir été écrites"
        );
        store.commit_wal().expect("flush");
    }

    // Réouverture : la projection doit être IDENTIQUE, reconstruite depuis les événements.
    let store2 = ObrainStore::open(&d).expect("réouverture");
    let relu = ApprisObrain::nouveau(store2.clone(), scope, candidats.clone());
    let stats_apres = relu.projection();
    assert_eq!(
        stats_apres, stats_avant,
        "les statistiques doivent être reconstruites à l'identique après réouverture"
    );

    // Après réouverture, le learner ouvre une NOUVELLE session : réécrire le même
    // identifiant de décision est une expérience NOUVELLE (pas un rejeu) — c'est la
    // sémantique voulue, et l'assertion le dit.
    let (c, p, m, ..) = stats_apres[0].clone();
    let avant = relu.projection();
    assert_eq!(
        relu.ecrire_observation(&premier_id, &c, &p, &m, true),
        Ecriture::Creee,
        "session différente => expérience nouvelle"
    );
    assert_ne!(relu.projection(), avant, "la nouvelle observation compte");

    // Rejeu VÉRITABLE : même session, même contenu => identique, sans double comptage.
    let apres = relu.projection();
    assert_eq!(
        relu.ecrire_observation(&premier_id, &c, &p, &m, true),
        Ecriture::RejeuIdentique,
        "même session + même contenu => rejeu identique"
    );
    assert_eq!(relu.projection(), apres, "aucun double comptage");
    // Conflit : même session, même identité, contenu incompatible.
    assert_eq!(
        relu.ecrire_observation(&premier_id, &c, &p, &m, false),
        Ecriture::Conflit,
        "même identité + contenu différent => conflit, pas rejeu"
    );

    // Le routeur a bien convergé avant la fermeture.
    assert!(
        sur_a_avant > 60,
        "le routeur doit converger vers a/m1 (sur_a={sur_a_avant}/120)"
    );
    drop(relu);
    drop(store2);
    let _ = std::fs::remove_dir_all(&d);
}

// ── Continuité : L2 en mémoire vs L3 continu vs L3 avec coupure ───────────────

/// Témoin L2 : même sémantique Beta, en mémoire seulement.
type CleBras = (String, String, String);
type StatsBras = (f64, f64, u64);

#[derive(Default)]
struct ApprisMemoire {
    bras: Mutex<HashMap<CleBras, StatsBras>>,
    en_cours: Mutex<HashMap<u128, String>>,
    candidats: Vec<(String, String)>,
    /// Nombre de décisions effectivement reçues par `record` : la parité des
    /// statistiques ne se lit pas seulement bras par bras, elle se lit aussi sur
    /// le flux de décisions qui les a nourries.
    decisions: Mutex<u64>,
}

impl ApprisMemoire {
    /// La table entière, triée — comparable à `ApprisObrain::projection`.
    fn table(&self) -> Vec<(String, String, String, f64, f64, u64)> {
        let m = self.bras.lock().unwrap();
        let mut v: Vec<(String, String, String, f64, f64, u64)> = m
            .iter()
            .map(|((c, p, mo), (a, b, n))| (c.clone(), p.clone(), mo.clone(), *a, *b, *n))
            .collect();
        v.sort_by(|x, y| (&x.0, &x.1, &x.2).cmp(&(&y.0, &y.1, &y.2)));
        v
    }
}

#[async_trait(?Send)]
impl Learning for ApprisMemoire {
    async fn arms(&self, class: &str) -> Vec<ArmStats> {
        let m = self.bras.lock().unwrap();
        self.candidats
            .iter()
            .map(|(p, mm)| {
                let (a, b, n) = m
                    .get(&(class.to_string(), p.clone(), mm.clone()))
                    .copied()
                    .unwrap_or((1.0, 1.0, 0));
                let mut st = ArmStats::fresh(ArmKey::new(class, p.clone(), mm.clone()), a, b);
                st.n = n;
                st
            })
            .collect()
    }

    fn hints(&self) -> PriorHints {
        PriorHints::new()
    }

    async fn record(&self, d: &Decision) {
        if d.used.is_some() {
            *self.decisions.lock().unwrap() += 1;
            self.en_cours.lock().unwrap().insert(d.id, d.class_key());
        }
    }

    async fn settle(&self, id: u128, served: Option<&Pick>, stop: &Stop, _c: Option<&str>) {
        let Some(classe) = self.en_cours.lock().unwrap().remove(&id) else {
            return;
        };
        let Some(p) = served else { return };
        let (da, db) = match stop {
            Stop::Done => (1.0, 0.0),
            Stop::Failed(_) => (0.0, 1.0),
            _ => return,
        };
        let mut m = self.bras.lock().unwrap();
        let e = m
            .entry((classe, p.provider_id.clone(), p.model.clone()))
            .or_insert((1.0, 1.0, 0));
        e.0 += da;
        e.1 += db;
        e.2 += 1;
    }
}

fn tirages(graine: u64, n: usize) -> Vec<[bool; 2]> {
    let mut rng = graine;
    let mut v = Vec::with_capacity(n);
    for _ in 0..n {
        let mut l = [false, false];
        for (c, case) in l.iter_mut().enumerate() {
            rng = rng
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            *case = (rng % 100) < if c == 0 { 80 } else { 20 };
        }
        v.push(l);
    }
    v
}

fn routeur_pour(learning: Rc<dyn Learning>, graine: u128) -> TurnRouter {
    let spec = RouterSpec::new(
        Rc::new(MemoryPool::new(vec![
            candidat("a", "m1"),
            candidat("b", "m2"),
        ])),
        Rc::new(SequenceSeeds::from(graine)),
        "a".to_string(),
        "m1".to_string(),
    )
    .learning(learning)
    .auto()
    .settings(parametres());
    TurnRouter::new(spec)
}

/// Ce qu'une moitié du parcours a produit, **décision par décision**.
///
/// Le compte des décisions vient du côté ROUTEUR ; la ventilation des non
/// appliquées vient de la cause qu'il nomme (`RouteWhy`). C'est une source
/// indépendante du `Compteurs` de l'apprentissage : comparer les deux est la
/// contre-épreuve qui dit si une décision « non réglée » l'est pour la raison
/// qu'on croit.
#[derive(Debug, Default)]
struct Tour {
    /// Tours qui ont produit une décision AVEC une paire `used` — exactement ce
    /// que voit `Learning::record`.
    decisions: usize,
    /// Décisions que le routeur a APPLIQUÉES : autant de `settle` qui arriveront
    /// à l'apprentissage.
    appliquees: usize,
    /// Décisions NON appliquées, comptées par cause nommée par le routeur.
    non_appliquees: std::collections::BTreeMap<&'static str, usize>,
}

impl Tour {
    /// Attendu : une décision non appliquée n'ouvre aucune attente, donc son
    /// `settle` n'atteint jamais l'apprentissage.
    fn non_reglees_attendues(&self) -> usize {
        self.decisions - self.appliquees
    }

    fn ventilation(&self) -> String {
        if self.non_appliquees.is_empty() {
            return "aucune".to_string();
        }
        self.non_appliquees
            .iter()
            .map(|(cause, n)| format!("{cause}={n}"))
            .collect::<Vec<_>>()
            .join(" + ")
    }
}

/// Joue `draws` et rend la trajectoire des paires choisies.
fn jouer(
    routeur: &TurnRouter,
    draws: &[[bool; 2]],
    depart: usize,
) -> (Vec<(String, String)>, Tour) {
    let mut trajet = Vec::new();
    let mut tour = Tour::default();
    for ligne in draws.iter().skip(depart) {
        let routed = executer(routeur.route(TurnInput {
            text: "hello",
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
        let idx = if used.provider_id == "a" { 0 } else { 1 };
        trajet.push((used.provider_id.clone(), used.model.clone()));
        tour.decisions += 1;
        if dec.applied {
            tour.appliquees += 1;
        } else {
            *tour.non_appliquees.entry(routed.why.code()).or_insert(0) += 1;
        }
        executer(routeur.record(&dec));
        let stop = if ligne[idx] {
            Stop::Done
        } else {
            Stop::Failed("transport".to_string())
        };
        executer(routeur.settle(&stop, None, true));
    }
    (trajet, tour)
}

#[test]
fn l3_continuite_trajectoires_l2_l3_et_coupure() {
    let base = std::env::temp_dir().join(format!("l3c_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let draws = tirages(7, 80);
    let coupe = 40;

    // L2 : témoin en mémoire
    let memo = Rc::new(ApprisMemoire {
        candidats: vec![("a".into(), "m1".into()), ("b".into(), "m2".into())],
        ..Default::default()
    });
    let r2 = routeur_pour(memo.clone(), 7);
    let (trajet_l2, tour_l2) = jouer(&r2, &draws, 0);

    // L3 continu
    let d1 = base.join("continu");
    let s1 = ObrainStore::open(&d1).expect("ouverture L3");
    let l1 = Rc::new(ApprisObrain::nouveau(
        s1.clone(),
        "conv-cont",
        vec![("a".into(), "m1".into()), ("b".into(), "m2".into())],
    ));
    let r3 = routeur_pour(l1.clone(), 7);
    let (trajet_l3, tour_l3) = jouer(&r3, &draws, 0);

    // L3 avec fermeture / réouverture à mi-parcours
    let d2 = base.join("coupe");
    let mut trajet_coupe;
    {
        let s = ObrainStore::open(&d2).expect("ouverture coupure");
        let l = Rc::new(ApprisObrain::nouveau(
            s.clone(),
            "conv-coupe",
            vec![("a".into(), "m1".into()), ("b".into(), "m2".into())],
        ));
        let r = routeur_pour(l.clone(), 7);
        trajet_coupe = jouer(&r, &draws[..coupe], 0).0;
        s.commit_wal().expect("flush");
    }
    {
        let s = ObrainStore::open(&d2).expect("réouverture coupure");
        let l = Rc::new(ApprisObrain::nouveau(
            s.clone(),
            "conv-coupe",
            vec![("a".into(), "m1".into()), ("b".into(), "m2".into())],
        ));
        // Le ROUTEUR est recréé (état et graines réinitialisés) : seules les
        // STATISTIQUES persistent. C'est le point que ce test mesure.
        let r = routeur_pour(l.clone(), 7);
        trajet_coupe.extend(jouer(&r, &draws, coupe).0);
        s.commit_wal().expect("flush");
    }

    println!(
        "trajectoires — L2 {} · L3 {} · coupure {}",
        trajet_l2.len(),
        trajet_l3.len(),
        trajet_coupe.len()
    );
    // PARITÉ : L2 en mémoire et L3 persistant doivent décider identiquement.
    assert_eq!(
        trajet_l2, trajet_l3,
        "L2 et L3 doivent produire la MÊME trajectoire à graines et tirages identiques"
    );

    // ── Parité des STATISTIQUES, et pas seulement du total ───────────────────
    // La trajectoire dit qu'ils DÉCIDENT pareil ; elle ne dit pas qu'ils ont
    // APPRIS pareil. On compare donc la table entière : pour chaque bras
    // (classe, fournisseur, modèle), Alpha, Beta et n — puis le flux de
    // décisions qui l'a nourrie.
    let decisions_l2 = *memo.decisions.lock().unwrap();
    let decisions_l3 = l1.compteurs().decisions;
    assert_eq!(
        decisions_l2, decisions_l3,
        "L2 et L3 doivent recevoir le même nombre de décisions (reçu par `record`)"
    );

    let mut table_l2: std::collections::BTreeMap<(String, String, String), (f64, f64, u64)> =
        Default::default();
    for (c, p, m, a, b, n) in memo.table() {
        table_l2.insert((c, p, m), (a, b, n));
    }
    let table_l3: std::collections::BTreeMap<(String, String, String), (f64, f64, u64)> = l1
        .projection()
        .into_iter()
        .map(|(c, p, m, a, b, n)| ((c, p, m), (a as f64, b as f64, n as u64)))
        .collect();
    assert!(
        !table_l3.is_empty(),
        "la projection L3 doit contenir au moins un bras observé"
    );
    assert_eq!(
        table_l2, table_l3,
        "parité L2/L3 sur TOUS les bras : mêmes Alpha, mêmes Beta, mêmes n"
    );

    // Les catégories nommées une par une, pour que l'égalité soit lisible et pas
    // seulement globale.
    let totaux = |t: &std::collections::BTreeMap<(String, String, String), (f64, f64, u64)>| {
        t.values()
            .fold((0.0f64, 0.0f64, 0u64), |(sa, sb, sn), (a, b, n)| {
                (sa + a, sb + b, sn + n)
            })
    };
    let (alpha_l2, beta_l2, n_l2) = totaux(&table_l2);
    let (alpha_l3, beta_l3, n_l3) = totaux(&table_l3);
    println!(
        "stats — bras={} · Alpha {alpha_l2} vs {alpha_l3} · Beta {beta_l2} vs {beta_l3} · \
         n {n_l2} vs {n_l3} · décisions {decisions_l2} vs {decisions_l3}",
        table_l2.len()
    );
    assert_eq!(
        (alpha_l2, beta_l2, n_l2),
        (alpha_l3, beta_l3, n_l3),
        "totaux"
    );
    // Contrôle interne : chaque bras part de l'a priori neutre 1/1, et chaque
    // observation réglée ajoute 1 à Alpha (favorable) ou à Beta (défavorable).
    // Donc Alpha + Beta = 2 × (nombre de bras) + n.
    let bras = table_l2.len() as f64;
    assert_eq!(
        alpha_l2 + beta_l2,
        2.0 * bras + n_l2 as f64,
        "a priori 1/1 par bras + un bilan par observation"
    );
    // Une décision NON appliquée (exploration refusée) n'ouvre aucune attente :
    // elle est comptée, mais elle ne nourrit aucun bras. L'écart est donc
    // exactement le nombre de décisions non réglées, et il doit être le même des
    // deux côtés.
    assert_eq!(
        decisions_l2 as u64 - n_l2,
        tour_l2.non_reglees_attendues() as u64,
        "L2 : décisions - observations = décisions non réglées attendues"
    );
    assert_eq!(
        decisions_l3 as u64 - n_l3,
        tour_l3.non_reglees_attendues() as u64,
        "L3 : décisions - observations = décisions non réglées attendues"
    );

    // La coupure peut diverger (routeur recréé) : on le rapporte, sans l'exiger.
    let divergences = trajet_coupe
        .iter()
        .zip(trajet_l3.iter())
        .filter(|(a, b)| a != b)
        .count();
    println!(
        "divergences après coupure : {divergences}/{}",
        trajet_l3.len()
    );

    let _ = std::fs::remove_dir_all(&base);
}

/// **Contre-épreuve de reprise** : les observations d'après-réouverture sont-elles
/// réellement conservées, ou silencieusement prises pour des rejeux ?
#[test]
fn l3_reprise_conserve_les_nouvelles_observations() {
    let d = std::env::temp_dir().join(format!("l3n_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    let scope = "conv-reprise";
    let cands = vec![
        ("a".to_string(), "m1".to_string()),
        ("b".to_string(), "m2".to_string()),
    ];
    let draws = tirages(3, 80);
    let coupe = 40;

    let compteurs_avant;
    let tour_avant;
    {
        let store = ObrainStore::open(&d).expect("ouverture");
        let l = Rc::new(ApprisObrain::nouveau(store.clone(), scope, cands.clone()));
        let r = routeur_pour(l.clone(), 3);
        let (_, t) = jouer(&r, &draws[..coupe], 0);
        tour_avant = t;
        compteurs_avant = l.compteurs();
        println!(
            "avant fermeture : créées={} rejeux={} conflits={}",
            compteurs_avant.creees, compteurs_avant.rejeux, compteurs_avant.conflits
        );
        store.commit_wal().expect("flush");
    }
    let store = ObrainStore::open(&d).expect("réouverture");
    let l = Rc::new(ApprisObrain::nouveau(store.clone(), scope, cands.clone()));
    let r = routeur_pour(l.clone(), 3); // routeur RECRÉÉ (graines réinitialisées)
    let (_, tour_apres) = jouer(&r, &draws, coupe);
    let compteurs_apres = l.compteurs();
    let stats = l.projection();
    let n_total: u32 = stats.iter().map(|(_, _, _, _, _, n)| *n).sum();
    // Comptage INDÉPENDANT des identités persistées (requête GQL, pas la somme des n).
    let identites = l.identites_persistees();
    store.commit_wal().expect("flush");

    let creees_total = compteurs_avant.creees as usize + compteurs_apres.creees as usize;
    println!(
        "après reprise : créées={} rejeux={} conflits={} | observations distinctes={} somme_n={}",
        compteurs_apres.creees,
        compteurs_apres.rejeux,
        compteurs_apres.conflits,
        creees_total,
        n_total
    );
    // La ligne ci-dessus affichait `40 + creees` : 40 est le nombre de TIRAGES de la
    // première moitié, pas le nombre d'observations qu'elle a créées (34). Le total
    // annoncé valait donc 74 quand la vérité était 68 — une erreur d'affichage qui
    // contredisait l'assertion juste en dessous. On somme maintenant les deux
    // moitiés, et c'est la même valeur que le comptage indépendant.
    assert_eq!(
        creees_total, identites,
        "l'affichage `observations distinctes` doit être le comptage réel"
    );

    // Deux sources INDÉPENDANTES expliquent qu'une décision ne produise pas
    // d'observation :
    //  (a) le ROUTEUR ne l'a pas appliquée (`applied == false`) : il n'ouvre
    //      aucune attente, donc son `settle` n'atteint jamais l'apprentissage ;
    //  (b) l'apprentissage l'a réglée mais la persistance a échoué (`erreurs`).
    // Le bilan ci-dessous confronte les deux : ce que le routeur dit avoir
    // refusé (attendu) doit égaler ce que l'apprentissage n'a jamais réglé
    // (obtenu), et chaque refus doit avoir une CAUSE NOMMÉE — pas un total.
    let bilan = |role: &str, c: &Compteurs, tour: &Tour| -> usize {
        let comptabilise = c.id_absent
            + c.served_none
            + c.stop_non_exploitable
            + c.creees
            + c.rejeux
            + c.conflits
            + c.erreurs;
        println!(
            "{role} : décisions={} consultatives={} | id_absent={} served_none={} \
             stop_non_exploitable={} creees={} rejeux={} conflits={} erreurs={} \
             | comptabilisé={comptabilise}",
            tour.decisions,
            c.consultatives,
            c.id_absent,
            c.served_none,
            c.stop_non_exploitable,
            c.creees,
            c.rejeux,
            c.conflits,
            c.erreurs
        );
        let jamais_regles = tour.decisions.saturating_sub(comptabilise as usize);
        let attendu = tour.non_reglees_attendues();
        println!(
            "{role} : NON RÉGLÉES obtenu={jamais_regles} attendu={attendu} \
             | ventilation : {}",
            tour.ventilation()
        );
        assert_eq!(
            comptabilise as usize + jamais_regles,
            tour.decisions,
            "{role} : la comptabilité doit être exhaustive (réglées + jamais réglées = décisions)"
        );
        // Le compte obtenu est-il celui que le routeur ANNONCE ?
        assert_eq!(
            jamais_regles, attendu,
            "{role} : l'apprentissage doit ne rien avoir réglé exactement pour les \
             décisions que le routeur n'a pas appliquées"
        );
        // La ventilation couvre-t-elle le compte ? (somme des causes = attendu)
        assert_eq!(
            tour.non_appliquees.values().sum::<usize>(),
            attendu,
            "{role} : chaque décision non réglée doit tomber dans une cause"
        );
        // Et la cause est NOMMÉE : si une autre apparaît, le test le dit au lieu
        // de la diluer dans un total.
        assert!(
            tour.non_appliquees
                .keys()
                .all(|cause| *cause == RouteWhy::ExplorationNeverMoves.code()),
            "{role} : cause non réglée inattendue — {:?}",
            tour.non_appliquees
        );
        jamais_regles
    };
    let jamais_avant = bilan("AVANT  ", &compteurs_avant, &tour_avant);
    let jamais_apres = bilan("APRÈS  ", &compteurs_apres, &tour_apres);
    println!(
        "TOTAL décisions non réglées = {jamais_avant} (avant) + {jamais_apres} (après) = {}",
        jamais_avant + jamais_apres
    );
    println!("identités persistées (requête indépendante) = {identites}");

    // Le comptage indépendant doit égaler les créations comptabilisées.
    assert_eq!(
        identites,
        compteurs_avant.creees as usize + compteurs_apres.creees as usize,
        "les identités persistées doivent égaler les observations créées"
    );
    assert!(
        compteurs_avant.creees as usize <= tour_avant.decisions,
        "on ne peut pas créer plus d'observations que de décisions"
    );
    assert!(
        compteurs_apres.creees as usize <= tour_apres.decisions,
        "idem après reprise"
    );
    assert_eq!(
        compteurs_apres.rejeux, 0,
        "aucune observation prise pour un rejeu"
    );
    assert_eq!(compteurs_apres.conflits, 0, "aucun conflit d'identité");
    assert_eq!(
        n_total,
        compteurs_avant.creees as u32 + compteurs_apres.creees as u32,
        "somme des n = toutes les observations créées (aucune perdue)"
    );

    let _ = std::fs::remove_dir_all(&d);
}

/// **Le compteur `erreurs` n'est pas décoratif** : une écriture refusée y atterrit,
/// n'est comptée ni comme création ni comme rejeu, et ne laisse **rien** dans le
/// store.
///
/// La panne n'est pas simulée : on arme le bail d'écriture du store puis on le
/// périme (`fence_write_lease_for_test`), exactement comme si un autre écrivain
/// avait repris le shard. C'est le chemin `ObrainStore::check_write` que doit
/// passer tout écrivain qui n'est pas un service de données — et l'apprenant en
/// est un.
#[test]
fn l3_echec_d_ecriture_atterrit_dans_erreurs() {
    use this::storage::writer_lease::FileAuthority;

    let d = std::env::temp_dir().join(format!("l3e_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    let store = ObrainStore::open(&d).expect("ouverture");
    let appris = ApprisObrain::nouveau(
        store.clone(),
        "conv-erreurs",
        vec![("a".to_string(), "m1".to_string())],
    );
    let authority = FileAuthority::new(d.as_path());
    store
        .enable_write_fencing_with(&authority)
        .expect("bail d'écriture armé");

    // Témoin : avec le bail, l'écriture passe.
    assert_eq!(
        appris.ecrire_observation("dec-temoin", "chat.general", "a", "m1", true),
        Ecriture::Creee,
        "le détenteur du bail écrit"
    );

    // Un autre écrivain a repris le shard : celui-ci est PÉRIMÉ.
    store.fence_write_lease_for_test(2);
    assert_eq!(
        appris.ecrire_observation("dec-refusee", "chat.general", "a", "m1", true),
        Ecriture::Erreur,
        "un écrivain périmé n'écrit pas"
    );
    let apres_refus = appris.compteurs();
    assert_eq!(apres_refus.erreurs, 1, "l'échec est compté dans `erreurs`");
    assert_eq!(
        apres_refus.creees, 1,
        "l'échec n'est PAS compté comme une création"
    );
    assert_eq!(apres_refus.rejeux, 0, "ni comme un rejeu");
    assert_eq!(
        appris.identites_persistees(),
        1,
        "le refus n'a laissé aucune identité dans le store"
    );

    // Le bail est rendu : l'écriture redevient possible. Le compteur n'est pas un
    // cimetière — un échec passé reste compté, et la tentative rattrapée passe.
    store.release_write_fencing();
    assert_eq!(
        appris.ecrire_observation("dec-refusee", "chat.general", "a", "m1", true),
        Ecriture::Creee,
        "bail rendu : la même écriture passe"
    );
    let apres_reprise = appris.compteurs();
    assert_eq!(apres_reprise.erreurs, 1, "l'échec passé reste compté");
    assert_eq!(apres_reprise.creees, 2, "la tentative rattrapée crée");
    assert_eq!(appris.identites_persistees(), 2);

    store.commit_wal().expect("flush");
    let _ = std::fs::remove_dir_all(&d);
}
