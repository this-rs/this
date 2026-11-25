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
    RouterSpec, RoutingSettings, SequenceSeeds, TurnInput, TurnRouter,
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
struct ApprisObrain {
    store: Arc<ObrainStore>,
    scope: String,
    en_cours: Mutex<HashMap<u128, String>>, // decision_id -> classe
}

impl ApprisObrain {
    fn nouveau(store: Arc<ObrainStore>, scope: &str) -> Self {
        Self {
            store,
            scope: scope.to_string(),
            en_cours: Mutex::new(HashMap::new()),
        }
    }

    fn ecrire_observation(
        &self,
        decision: &str,
        classe: &str,
        provider: &str,
        modele: &str,
        favorable: bool,
    ) -> bool {
        let evenement = cle_evenement(&self.scope, decision);
        let existe = self
            .store
            .execute_query(
                "gql",
                &format!(
                    "MATCH (o:observation) WHERE o.evenement = '{}' RETURN o.evenement AS e",
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
            return false;
        }
        let mut s = self.store.db().session();
        s.begin_transaction().expect("begin");
        let n = s.create_node(&["observation"]);
        s.set_node_property(n, "evenement", Value::String(evenement.into()));
        s.set_node_property(n, "scope", Value::String(self.scope.clone().into()));
        s.set_node_property(n, "classe", Value::String(classe.into()));
        s.set_node_property(n, "provider", Value::String(provider.into()));
        s.set_node_property(n, "modele", Value::String(modele.into()));
        s.set_node_property(n, "favorable", Value::Bool(favorable));
        s.commit().expect("commit");
        true
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
        self.projection()
            .into_iter()
            .filter(|(c, _, _, _, _, _)| c == class)
            .map(|(c, p, m, a, b, _n)| ArmStats::fresh(ArmKey::new(c, p, m), a as f64, b as f64))
            .collect()
    }

    fn hints(&self) -> PriorHints {
        PriorHints::new()
    }

    async fn record(&self, d: &Decision) {
        if d.used.is_some() {
            self.en_cours
                .lock()
                .unwrap()
                .insert(d.id, d.signature.arm_key());
        }
    }

    async fn settle(&self, id: u128, served: Option<&Pick>, stop: &Stop, _code: Option<&str>) {
        let Some(classe) = self.en_cours.lock().unwrap().remove(&id) else {
            return;
        };
        let Some(p) = served else { return };
        let favorable = match stop {
            Stop::Done => true,
            Stop::Failed(_) => false,
            _ => return, // résultat inconnu : aucun bras nourri
        };
        self.ecrire_observation(
            &id.to_string(),
            &classe,
            &p.provider_id,
            &p.model,
            favorable,
        );
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

    let stats_avant;
    let sur_a_avant;
    let premier_id: String;
    {
        let store = ObrainStore::open(&d).expect("ouverture");
        let appris = Rc::new(ApprisObrain::nouveau(store.clone(), scope));
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

        let mut premier: Option<String> = None;
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
                if premier.is_none() {
                    premier = Some(dec.id.to_string());
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
        premier_id = premier.expect("au moins une décision écrite");
        stats_avant = appris.projection();
        assert!(
            !stats_avant.is_empty(),
            "des observations doivent avoir été écrites"
        );
        store.commit_wal().expect("flush");
    }

    // Réouverture : la projection doit être IDENTIQUE, reconstruite depuis les événements.
    let store2 = ObrainStore::open(&d).expect("réouverture");
    let relu = ApprisObrain::nouveau(store2.clone(), scope);
    let stats_apres = relu.projection();
    assert_eq!(
        stats_apres, stats_avant,
        "les statistiques doivent être reconstruites à l'identique après réouverture"
    );

    // Rejeu d'une observation déjà écrite : refusée, projection inchangée.
    let (c, p, m, ..) = stats_apres[0].clone();
    let avant_rejeu = relu.projection();
    assert!(
        !relu.ecrire_observation(&premier_id, &c, &p, &m, true),
        "un rejeu doit être refusé"
    );
    assert_eq!(relu.projection(), avant_rejeu, "aucun double comptage");

    // Le routeur a bien convergé avant la fermeture.
    assert!(
        sur_a_avant > 60,
        "le routeur doit converger vers a/m1 (sur_a={sur_a_avant}/120)"
    );
    drop(relu);
    drop(store2);
    let _ = std::fs::remove_dir_all(&d);
}
