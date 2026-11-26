#![cfg(all(feature = "obrain", feature = "objects-toolbox"))]
//! **Concurrence G1-G5 — réservation, publication, autorités, annulation.**
//!
//! Flux C de la cognition : deux tâches concurrentes partageant l'autorité
//! d'attribution, des **interleavings déterministes** (canaux et barrières —
//! jamais de seuil de temps arbitraire), et un **comptage séparé** de cinq
//! grandeurs qui ne doivent pas se confondre :
//!
//! 1. **réservations** — `Attribution::reserver` (le droit unique de publier) ;
//! 2. **appels backend** — chaque lecture/écriture dans Obrain (GQL, session) ;
//! 3. **effets persistés** — le nombre de nœuds `observation` réellement
//!    présents dans Obrain, relus par GQL (jamais un compteur RAM) ;
//! 4. **confirmations** — `Attribution::confirmer` rendu `true` (état local) ;
//! 5. **apprentissages** — `record` reçu avec une identité constituée
//!    (`Record::Enregistre`), la contribution au cycle d'apprentissage.
//!
//! Le fil conducteur est la **distinction entre l'unicité locale de la
//! confirmation et l'unicité globale de l'effet** : une confirmation locale
//! (RAM) ne démontre pas un effet externe unique (Obrain). G2 le mesure en
//! deux nombres distincts ; G3 le mesure en montrant que le bail d'écriture est
//! ce qui refuse un écrivain périmé.
//!
//! Scénarios (chaque critère d'acceptation a son test) :
//!
//! - **G1** — une seule réservation réussit sur deux concurrentes (barrière) ;
//! - **G4** — une réponse périmée (ancienne génération) n'affecte pas la
//!   nouvelle génération (canaux) ;
//! - **G5** — annulation pendant l'engagement : l'effet est réconcilié, pas
//!   masqué (canaux) ;
//! - **G2** — deux publications du même effet logique : un seul effet global,
//!   deux confirmations locales ;
//! - **G3** — deux autorités indépendantes sur un backend partagé : descripteur
//!   non transférable + bail d'écriture qui refuse l'écrivain périmé.
//!
//! La base est `569e051c` (consolidé) : les primitives `Attribution` et le
//! fencing `writer_lease` y sont déjà en place ; ces contre-épreuves les
//! exercent en **concurrence réelle** (threads), là où les contre-épreuves
//! séquentielles de `integrite_obrain.rs` ne pouvaient pas les voir.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Barrier, Mutex, mpsc};
use std::thread;

use obrain_common::types::Value;
use this::storage::obrain::ObrainStore;
use this_harness::attribution::{
    Attribution, Cycle, Definition, Etat, Observation, Reception, Record, Resultat,
    RésolutionBackend, Tentative,
};

// ---------------------------------------------------------------------------
// Comptage séparé — les cinq grandeurs, en atomiques, partagées entre les
// tâches. Les « effets persistés » n'y figurent PAS : ils se mesurent dans
// Obrain par requête GQL, indépendamment de tout compteur RAM.
// ---------------------------------------------------------------------------

#[derive(Default)]
struct Compteurs {
    /// `reserver` appelé.
    reservations: AtomicU64,
    /// `reserver` a rendu `Some` (réservation gagnée).
    reservations_reussies: AtomicU64,
    /// Lecture ou écriture réelle dans Obrain (GQL ou session d'écriture).
    appels_backend: AtomicU64,
    /// `confirmer` rendu `true` — l'unicité LOCALE.
    confirmations: AtomicU64,
    /// `record` reçu avec identité constituée (`Enregistre`) — l'apprentissage.
    apprentissages: AtomicU64,
    /// `demander_annulation` rendu `true`.
    annulations: AtomicU64,
    /// Publication refusée (bail) ou transaction en échec.
    erreurs: AtomicU64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Bilan {
    reservations: u64,
    reservations_reussies: u64,
    appels_backend: u64,
    confirmations: u64,
    apprentissages: u64,
    annulations: u64,
    erreurs: u64,
}

impl Compteurs {
    fn bilan(&self) -> Bilan {
        Bilan {
            reservations: self.reservations.load(Ordering::Relaxed),
            reservations_reussies: self.reservations_reussies.load(Ordering::Relaxed),
            appels_backend: self.appels_backend.load(Ordering::Relaxed),
            confirmations: self.confirmations.load(Ordering::Relaxed),
            apprentissages: self.apprentissages.load(Ordering::Relaxed),
            annulations: self.annulations.load(Ordering::Relaxed),
            erreurs: self.erreurs.load(Ordering::Relaxed),
        }
    }
}

/// L'issue d'une publication d'effet, en cinq cas qu'il ne faut pas confondre.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Ecriture {
    /// Écrite (commit rendu) — le seul effet **confirmé** dans Obrain.
    Creee,
    /// Déduplication : même effet logique, contenu identique. Rien d'écrit.
    RejeuIdentique,
    /// Déduplication : même effet logique, contenu divergent. Rien d'écrit.
    Conflit,
    /// La lecture de déduplication a échoué : issue inconnue, jamais « absent ».
    Indetermine,
    /// Écriture refusée (bail périmé) ou transaction en échec.
    Erreur,
}

// ---------------------------------------------------------------------------
// Une autorité d'attribution : sa primitive RAM (Attribution) + son backend
// partagé (Arc<ObrainStore>) + ses compteurs partagés. Deux instances = deux
// autorités indépendantes (le descripteur Tentative n'est pas transférable).
// ---------------------------------------------------------------------------

struct Autorite {
    attr: Mutex<Attribution>,
    store: Arc<ObrainStore>,
    scope: String,
    compteurs: Arc<Compteurs>,
}

impl Autorite {
    fn nouvelle(store: Arc<ObrainStore>, scope: &str, compteurs: Arc<Compteurs>) -> Self {
        Self {
            attr: Mutex::new(Attribution::new()),
            store,
            scope: scope.to_string(),
            compteurs,
        }
    }

    fn record(&self, id: u128, definition: Definition) -> Record {
        let r = self.attr.lock().unwrap().record(id, definition);
        if r == Record::Enregistre {
            self.compteurs
                .apprentissages
                .fetch_add(1, Ordering::Relaxed);
        }
        r
    }

    fn recevoir(&self, id: u128, observation: Observation) -> Reception {
        self.attr.lock().unwrap().recevoir(id, observation)
    }

    fn reserver(&self, id: u128) -> Option<Tentative> {
        self.compteurs.reservations.fetch_add(1, Ordering::Relaxed);
        let t = self.attr.lock().unwrap().reserver(id);
        if t.is_some() {
            self.compteurs
                .reservations_reussies
                .fetch_add(1, Ordering::Relaxed);
        }
        t
    }

    fn confirmer(&self, id: u128, tentative: Tentative) -> bool {
        let ok = self.attr.lock().unwrap().confirmer(id, tentative);
        if ok {
            self.compteurs.confirmations.fetch_add(1, Ordering::Relaxed);
        }
        ok
    }

    fn echec_certain(&self, id: u128, tentative: Tentative) -> bool {
        self.attr.lock().unwrap().echec_certain(id, tentative)
    }

    fn resoudre(&self, id: u128, tentative: Tentative, resolution: RésolutionBackend) -> bool {
        self.attr
            .lock()
            .unwrap()
            .resoudre(id, tentative, resolution)
    }

    fn demander_annulation(&self, id: u128) -> bool {
        let ok = self.attr.lock().unwrap().demander_annulation(id);
        if ok {
            self.compteurs.annulations.fetch_add(1, Ordering::Relaxed);
        }
        ok
    }

    fn etat(&self, id: u128) -> Option<Cycle> {
        self.attr.lock().unwrap().etat(id).cloned()
    }

    // -- publication d'effet (la couche durable) --------------------------

    /// Publie l'observation dans Obrain, avec déduplication par identité stable
    /// (`evenement = scope/decision_id`). Le contrôle d'entrée d'écriture
    /// (`check_write`) passe AVANT d'écrire : un écrivain périmé est refusé.
    fn publier(
        &self,
        decision_id: u128,
        identite: &str,
        fournisseur: &str,
        modele: &str,
        favorable: bool,
    ) -> Ecriture {
        if let Err(_refus) = self.store.check_write() {
            self.compteurs.erreurs.fetch_add(1, Ordering::Relaxed);
            return Ecriture::Erreur;
        }
        let evenement = format!("{}/{}", self.scope, decision_id);
        let existe = match self.lit_existence(&evenement) {
            Ok(e) => e,
            Err(_) => return Ecriture::Indetermine,
        };
        if existe {
            match self.lire_contenu(&evenement) {
                Some((i, p, m, f))
                    if i == identite && p == fournisseur && m == modele && f == favorable =>
                {
                    Ecriture::RejeuIdentique
                }
                _ => Ecriture::Conflit,
            }
        } else {
            match self.inscrire(
                decision_id,
                identite,
                fournisseur,
                modele,
                favorable,
                &evenement,
            ) {
                Ok(()) => Ecriture::Creee,
                Err(_) => {
                    self.compteurs.erreurs.fetch_add(1, Ordering::Relaxed);
                    Ecriture::Erreur
                }
            }
        }
    }

    /// Nombre d'effets réellement persistés, relu par GQL — la source
    /// indépendante de tout compteur RAM. (N'incrémente pas `appels_backend` :
    /// c'est la mesure, pas l'opération mesurée.)
    fn compter_effets(&self) -> usize {
        let r = self
            .store
            .execute_query(
                "gql",
                &format!(
                    "MATCH (o:observation) WHERE o.scope = '{}' RETURN o.decision_id AS d",
                    echapper(&self.scope)
                ),
                100_000,
            )
            .expect("comptage des effets");
        r.get("rows")
            .and_then(|v| v.as_array())
            .map(|a| a.len())
            .expect("réponse GQL sans `rows`")
    }

    fn lit_existence(&self, evenement: &str) -> Result<bool, String> {
        self.compteurs
            .appels_backend
            .fetch_add(1, Ordering::Relaxed);
        let r = self
            .store
            .execute_query(
                "gql",
                &format!(
                    "MATCH (o:observation) WHERE o.evenement = '{}' RETURN o.decision_id AS d",
                    echapper(evenement)
                ),
                1,
            )
            .map_err(|e| e.to_string())?;
        let rows = r
            .get("rows")
            .and_then(|v| v.as_array())
            .ok_or_else(|| "réponse sans `rows`".to_string())?;
        Ok(!rows.is_empty())
    }

    fn lire_contenu(&self, evenement: &str) -> Option<(String, String, String, bool)> {
        self.compteurs
            .appels_backend
            .fetch_add(1, Ordering::Relaxed);
        let r = self
            .store
            .execute_query(
                "gql",
                &format!(
                    "MATCH (o:observation) WHERE o.evenement = '{}' RETURN o.identite AS i, o.provider AS p, o.modele AS m, o.favorable AS f",
                    echapper(evenement)
                ),
                1,
            )
            .expect("relecture du contenu existant");
        let ligne = r.get("rows").and_then(|v| v.as_array())?.first()?.clone();
        let g = |i: usize| {
            ligne
                .get(i)
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string()
        };
        let f = ligne.get(3).and_then(|v| v.as_bool()).unwrap_or(false);
        Some((g(0), g(1), g(2), f))
    }

    #[allow(clippy::too_many_arguments)]
    fn inscrire(
        &self,
        decision_id: u128,
        identite: &str,
        fournisseur: &str,
        modele: &str,
        favorable: bool,
        evenement: &str,
    ) -> Result<(), String> {
        self.compteurs
            .appels_backend
            .fetch_add(1, Ordering::Relaxed);
        let mut s = self.store.db().session();
        s.begin_transaction()
            .map_err(|e| format!("begin : {e:?}"))?;
        let n = s.create_node(&["observation"]);
        s.set_node_property(
            n,
            "decision_id",
            Value::String(decision_id.to_string().into()),
        );
        s.set_node_property(n, "identite", Value::String(identite.into()));
        s.set_node_property(n, "provider", Value::String(fournisseur.into()));
        s.set_node_property(n, "modele", Value::String(modele.into()));
        s.set_node_property(n, "favorable", Value::Bool(favorable));
        s.set_node_property(n, "scope", Value::String(self.scope.clone().into()));
        s.set_node_property(n, "evenement", Value::String(evenement.into()));
        s.commit().map_err(|e| format!("commit : {e:?}"))
    }
}

// ---------------------------------------------------------------------------
// Petits outils
// ---------------------------------------------------------------------------

/// Échappe une valeur interpolée dans une requête GQL. `execute_query` n'offre
/// pas de paramètres : l'échappement est la seule barrière, documentée comme telle.
fn echapper(s: &str) -> String {
    s.replace('\\', "\\\\").replace('\'', "\\'")
}

/// Compteur pour noms de répertoires uniques (pas d'uuid en dev-deps).
struct Unique;
impl Unique {
    fn suivant() -> u64 {
        static N: AtomicU64 = AtomicU64::new(0);
        N.fetch_add(1, Ordering::Relaxed)
    }
}

fn dir(tag: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!(
        "concurrence_{tag}_{}_{}",
        std::process::id(),
        Unique::suivant()
    ));
    let _ = std::fs::remove_dir_all(&d);
    d
}

fn def(identite: &str, fournisseur: &str, modele: &str) -> Definition {
    Definition {
        identite: Some(identite.to_string()),
        prevue: Some((fournisseur.to_string(), modele.to_string())),
        servie: Some((fournisseur.to_string(), modele.to_string())),
    }
}

fn obs_succes(fournisseur: &str, modele: &str) -> Observation {
    Observation {
        fournisseur: fournisseur.to_string(),
        modele: modele.to_string(),
        resultat: Resultat::Succes,
    }
}

// ---------------------------------------------------------------------------
// G1 — une seule réservation réussit sur deux concurrentes
// ---------------------------------------------------------------------------

#[test]
fn g1_une_seule_reservation_reussit_sur_deux_concurrentes() {
    let store = ObrainStore::new_in_memory();
    let compteurs = Arc::new(Compteurs::default());
    let autorite = Arc::new(Autorite::nouvelle(
        store.clone(),
        "scope-g1",
        compteurs.clone(),
    ));

    // État initial posé HORS course : record + recevoir → Recu. Une seule
    // contribution d'apprentissage.
    let id = 7u128;
    assert_eq!(
        autorite.record(id, def("k1", "a", "m1")),
        Record::Enregistre
    );
    assert_eq!(
        autorite.recevoir(id, obs_succes("a", "m1")),
        Reception::Recu
    );

    // Deux tâches concurrentes, libérées par la MÊME barrière : elles tentent
    // `reserver` au même instant. Le mutex sérialise, l'état `EnCours` refuse
    // la seconde — exactement une gagne, quelle que soit l'ordre réel.
    let barriere = Arc::new(Barrier::new(2));
    let gagnes = Arc::new(Mutex::new(Vec::new()));
    let mut poignees = Vec::new();
    for _ in 0..2 {
        let a = autorite.clone();
        let b = barriere.clone();
        let g = gagnes.clone();
        poignees.push(thread::spawn(move || {
            b.wait();
            g.lock().unwrap().push(a.reserver(id).is_some());
        }));
    }
    for h in poignees {
        h.join().unwrap();
    }

    let nb_gagnees = gagnes.lock().unwrap().iter().filter(|x| **x).count();
    assert_eq!(
        nb_gagnees, 1,
        "une seule réservation réussit sur deux concurrentes"
    );

    let bilan = compteurs.bilan();
    assert_eq!(bilan.reservations, 2, "deux tentatives de réservation");
    assert_eq!(
        bilan.reservations_reussies, 1,
        "une seule réservation gagnée"
    );
    assert_eq!(
        bilan.apprentissages, 1,
        "une seule contribution d'apprentissage"
    );
    assert_eq!(bilan.confirmations, 0, "G1 ne publie ni ne confirme");
    assert_eq!(
        autorite.compter_effets(),
        0,
        "aucun effet persisté : G1 ne publie pas"
    );
}

// ---------------------------------------------------------------------------
// G4 — une réponse périmée n'affecte pas la nouvelle génération
// ---------------------------------------------------------------------------

#[test]
fn g4_reponse_perimee_n_affecte_pas_la_nouvelle_generation() {
    let store = ObrainStore::new_in_memory();
    let compteurs = Arc::new(Compteurs::default());
    let autorite = Arc::new(Autorite::nouvelle(
        store.clone(),
        "scope-g4",
        compteurs.clone(),
    ));

    let id = 7u128;
    assert_eq!(
        autorite.record(id, def("k1", "a", "m1")),
        Record::Enregistre
    );
    assert_eq!(
        autorite.recevoir(id, obs_succes("a", "m1")),
        Reception::Recu
    );

    // Canaux : l'interleaving est imposé, aucun seuil de temps.
    let (tx_relache, rx_relache) = mpsc::channel(); // A → B : génération 1 relâchée
    let (tx_gen2, rx_gen2) = mpsc::channel(); // B → A : génération 2 réservée
    let (tx_perime, rx_perime) = mpsc::channel(); // A → B : réponse périmée tentée

    // Tâche A — l'écrivain de la génération 1, dont la réponse arrive trop tard.
    let poignee_a = {
        let autorite = autorite.clone();
        thread::spawn(move || {
            let t1 = autorite.reserver(id).expect("réservation génération 1");
            // Échec CERTAIN → la réservation est relâchée (Recu).
            assert!(autorite.echec_certain(id, t1), "relâche génération 1");
            tx_relache.send(()).unwrap();

            rx_gen2.recv().unwrap(); // la génération 2 est désormais réservée

            // La réponse périmée (génération 1) ne touche PAS la génération 2.
            assert!(!autorite.confirmer(id, t1), "confirmation périmée refusée");
            assert!(
                !autorite.resoudre(id, t1, RésolutionBackend::Incompatible),
                "résolution périmée refusée"
            );
            tx_perime.send(()).unwrap();
        })
    };

    // Tâche B — l'écrivain de la génération 2.
    let poignee_b = {
        let autorite = autorite.clone();
        thread::spawn(move || {
            rx_relache.recv().unwrap(); // génération 1 relâchée
            let t2 = autorite.reserver(id).expect("réservation génération 2");
            tx_gen2.send(()).unwrap();

            rx_perime.recv().unwrap(); // la réponse périmée a été tentée

            // La génération 2 est intacte : toujours EnCours, puis confirmée.
            assert!(matches!(
                autorite.etat(id).map(|c| c.etat),
                Some(Etat::EnCours)
            ));
            assert!(autorite.confirmer(id, t2), "génération 2 confirmée");
        })
    };

    poignee_a.join().unwrap();
    poignee_b.join().unwrap();

    assert_eq!(autorite.etat(id).unwrap().etat, Etat::Confirme);
    let bilan = compteurs.bilan();
    assert_eq!(bilan.reservations_reussies, 2, "deux générations réservées");
    assert_eq!(
        bilan.confirmations, 1,
        "seule la génération 2 est confirmée"
    );
    assert_eq!(autorite.compter_effets(), 0, "G4 ne publie pas d'effet");
}

// ---------------------------------------------------------------------------
// G5 — annulation pendant l'engagement : l'effet est réconcilié, pas masqué
// ---------------------------------------------------------------------------

#[test]
fn g5_annulation_pendant_engagement_reconciliee() {
    let store = ObrainStore::new_in_memory();
    let compteurs = Arc::new(Compteurs::default());
    let autorite = Arc::new(Autorite::nouvelle(
        store.clone(),
        "scope-g5",
        compteurs.clone(),
    ));

    let id = 7u128;
    assert_eq!(
        autorite.record(id, def("k1", "a", "m1")),
        Record::Enregistre
    );
    assert_eq!(
        autorite.recevoir(id, obs_succes("a", "m1")),
        Reception::Recu
    );

    let (tx_engage, rx_engage) = mpsc::channel(); // éditeur → annuleur : engagé
    let (tx_annule, rx_annule) = mpsc::channel(); // annuleur → éditeur : annulation demandée

    // Tâche éditeur : réserve, puis découvre l'effet pendant l'engagement.
    let editeur = {
        let autorite = autorite.clone();
        thread::spawn(move || {
            let t = autorite.reserver(id).expect("engagement");
            tx_engage.send(()).unwrap();
            rx_annule.recv().unwrap(); // l'annulation a été demandée pendant l'engagement
            // L'effet externe est découvert : il est réconcilié, pas masqué.
            assert!(autorite.confirmer(id, t), "effet découvert et réconcilié");
            assert!(
                autorite.etat(id).unwrap().annulation_demandee,
                "l'annulation reste enregistrée"
            );
        })
    };

    // Tâche annuleur : demande l'annulation pendant que l'éditeur est engagé.
    let annuleur = {
        let autorite = autorite.clone();
        thread::spawn(move || {
            rx_engage.recv().unwrap();
            assert!(
                autorite.demander_annulation(id),
                "annulation demandée pendant EnCours"
            );
            tx_annule.send(()).unwrap();
        })
    };

    editeur.join().unwrap();
    annuleur.join().unwrap();

    let cycle = autorite.etat(id).unwrap();
    assert_eq!(
        cycle.etat,
        Etat::Confirme,
        "l'effet est réconcilié, pas masqué"
    );
    assert!(cycle.annulation_demandee, "l'annulation est enregistrée");
    assert!(cycle.observation.is_some(), "l'effet reste observable");

    let bilan = compteurs.bilan();
    assert_eq!(bilan.annulations, 1);
    assert_eq!(bilan.confirmations, 1);
    assert_eq!(
        autorite.compter_effets(),
        0,
        "G5 n'exerce pas la couche durable"
    );
}

// ---------------------------------------------------------------------------
// G2 — deux publications du même effet logique : 1 effet global, 2 confirmations
// ---------------------------------------------------------------------------

#[test]
fn g2_deux_publications_du_meme_effet_logique() {
    let d = dir("g2");
    let store = ObrainStore::open(&d).expect("ouverture");
    let scope = "scope-g2";
    let compteurs = Arc::new(Compteurs::default());

    // Deux autorités INDÉPENDANTES, même backend, même effet logique (même
    // scope + même decision_id → même `evenement`).
    let a = Arc::new(Autorite::nouvelle(store.clone(), scope, compteurs.clone()));
    let b = Arc::new(Autorite::nouvelle(store.clone(), scope, compteurs.clone()));
    let id = 7u128;

    for autorite in [&a, &b] {
        assert_eq!(
            autorite.record(id, def("k1", "a", "m1")),
            Record::Enregistre
        );
        assert_eq!(
            autorite.recevoir(id, obs_succes("a", "m1")),
            Reception::Recu
        );
    }

    // Interleaving contrôlé : A publie (crée) avant que B publie (déduplique).
    let (tx_a, rx_a) = mpsc::channel();

    let t_a = {
        let a = a.clone();
        thread::spawn(move || {
            let t = a.reserver(id).expect("réservation A");
            assert_eq!(a.publier(id, "k1", "a", "m1", true), Ecriture::Creee);
            assert!(a.confirmer(id, t), "confirmation locale A");
            tx_a.send(()).unwrap();
        })
    };
    let t_b = {
        let b = b.clone();
        thread::spawn(move || {
            rx_a.recv().unwrap(); // A a fini : l'effet est en place
            let t = b.reserver(id).expect("réservation B");
            assert_eq!(
                b.publier(id, "k1", "a", "m1", true),
                Ecriture::RejeuIdentique
            );
            assert!(b.confirmer(id, t), "confirmation locale B");
        })
    };
    t_a.join().unwrap();
    t_b.join().unwrap();

    // L'unicité GLOBALE (1 effet persisté, relu par GQL) est distincte de
    // l'unicité LOCALE (2 confirmations, une par autorité).
    assert_eq!(a.compter_effets(), 1, "un seul effet global");
    let bilan = compteurs.bilan();
    assert_eq!(bilan.confirmations, 2, "deux confirmations locales");
    assert_eq!(
        bilan.apprentissages, 2,
        "deux contributions d'apprentissage"
    );
    // A : lit_existence + inscrire = 2 ; B : lit_existence + lire_contenu = 2.
    assert_eq!(
        bilan.appels_backend, 4,
        "deux lectures de dédup + une écriture + une relecture"
    );
    assert_eq!(bilan.erreurs, 0);

    store.commit_wal().expect("flush");
    drop(a);
    drop(b);
    drop(store);
    let _ = std::fs::remove_dir_all(&d);
}

// ---------------------------------------------------------------------------
// G3 — deux autorités indépendantes sur un backend partagé
// ---------------------------------------------------------------------------

#[test]
fn g3_deux_autorites_independantes_sur_un_backend_partage() {
    use this::storage::writer_lease::FileAuthority;

    let d = dir("g3");
    let store = ObrainStore::open(&d).expect("ouverture");
    let scope = "scope-g3";
    let compteurs = Arc::new(Compteurs::default());

    // Le bail d'écriture est armé : c'est LUI qui impose l'unicité de
    // l'écrivain sur le backend partagé (opt-in, `FileAuthority`).
    let authority = FileAuthority::new(d.as_path());
    store
        .enable_write_fencing_with(&authority)
        .expect("bail armé");

    // Deux autorités indépendantes : chacune son cycle, son descripteur.
    let a = Arc::new(Autorite::nouvelle(store.clone(), scope, compteurs.clone()));
    let b = Arc::new(Autorite::nouvelle(store.clone(), scope, compteurs.clone()));
    let id_a = 7u128;
    let id_b = 8u128;
    assert_eq!(a.record(id_a, def("k1", "a", "m1")), Record::Enregistre);
    assert_eq!(a.recevoir(id_a, obs_succes("a", "m1")), Reception::Recu);
    assert_eq!(b.record(id_b, def("k2", "b", "m2")), Record::Enregistre);
    assert_eq!(b.recevoir(id_b, obs_succes("b", "m2")), Reception::Recu);

    let ta = a.reserver(id_a).expect("réservation A");
    let tb = b.reserver(id_b).expect("réservation B");

    // Le descripteur est lié à son autorité : A ne confirme pas B, et inversement.
    assert!(
        !b.confirmer(id_b, ta),
        "descripteur inter-autorité refusé (A → B)"
    );
    assert!(
        !a.confirmer(id_a, tb),
        "descripteur inter-autorité refusé (B → A)"
    );

    // Chaque autorité publie SON effet sur le backend partagé : deux effets
    // distincts, tous deux écrits (le bail est détenu par le store partagé).
    assert_eq!(a.publier(id_a, "k1", "a", "m1", true), Ecriture::Creee);
    assert_eq!(b.publier(id_b, "k2", "b", "m2", true), Ecriture::Creee);
    assert!(a.confirmer(id_a, ta));
    assert!(b.confirmer(id_b, tb));
    assert_eq!(a.compter_effets(), 2, "deux effets distincts persistés");

    // Reprise du shard par un tiers : l'écrivain périmé est refusé par le bail.
    // Une confirmation locale ne suffit PAS à écrire — c'est le fencing qui
    // tranche au niveau global.
    store.fence_write_lease_for_test(2);
    let id_c = 9u128;
    assert_eq!(a.record(id_c, def("k1", "a", "m1")), Record::Enregistre);
    assert_eq!(a.recevoir(id_c, obs_succes("a", "m1")), Reception::Recu);
    let _tc = a.reserver(id_c).expect("réservation C");
    assert_eq!(
        a.publier(id_c, "k1", "a", "m1", true),
        Ecriture::Erreur,
        "écrivain périmé refusé par le bail"
    );
    // L'échec n'est pas un effet, et le cycle reste EnCours (réservé, non résolu).
    assert_eq!(a.compter_effets(), 2, "aucun nouvel effet persisté");
    assert!(matches!(a.etat(id_c).map(|c| c.etat), Some(Etat::EnCours)));
    // `_tc` est la tentative encore en vol, non résolue : c'est le point du scénario.

    let bilan = compteurs.bilan();
    assert_eq!(
        bilan.reservations_reussies, 3,
        "trois réservations (A, B, C)"
    );
    assert_eq!(bilan.confirmations, 2, "A et B confirmées, C refusée");
    assert_eq!(
        bilan.erreurs, 1,
        "la publication périmée est un échec nommé"
    );
    assert_eq!(
        bilan.apprentissages, 3,
        "trois contributions d'apprentissage"
    );

    store.commit_wal().expect("flush");
    drop(a);
    drop(b);
    drop(store);
    let _ = std::fs::remove_dir_all(&d);
}
