#![cfg(all(feature = "obrain", feature = "objects-toolbox"))]
//! **Intégrité native Obrain — contre-épreuves de bout en bout.**
//!
//! Ces contre-épreuves ne récitent pas le contrat, elles essaient de le casser.
//! Chacune part des primitives réelles ([`ObrainStore`], le port [`Learning`] de
//! this-harness, le cycle `route → record → settle`) et vérifie **dans Obrain** —
//! par requête GQL, jamais par un compteur RAM — ce qui a réellement été persisté.
//!
//! Invariants mis à l'épreuve :
//!
//! 1. Une **formation** (proposition sans identité, `class: None`) n'est PAS une
//!    connaissance constituée : deux formations distinctes non constituées ne
//!    laissent **aucune observation cognitive commune** dans Obrain.
//! 2. La **constitution** d'une identité (`class: Some`) persiste une observation
//!    avec un **tuple complet** : `decision_id`, `identite`, `provider`, `modele`,
//!    `resultat`.
//! 3. L'attribution suit la **décision** (`Decision::class_key()`), jamais la
//!    dernière classe consultée : deux décisions entrelacées, réglées dans l'ordre
//!    inverse, restent chacune attribuée à sa propre identité.
//! 4. `record` identique = rejeu ; `record` divergent = conflit — ni double
//!    écriture, ni écrasement.
//! 5. Le rejeu complet `record → settle → record → settle` n'écrit qu'une fois.
//! 6. Un échec d'écriture n'est ni une création ni une perte silencieuse, et la
//!    récupération reprend sans maquiller la comptabilité.
//! 7. Un **résultat indéterminé** (lecture de déduplication en échec) n'est JAMAIS
//!    compté comme un effet confirmé.
//! 8. Fermeture / réouverture : les tuples complets survivent intacts.
//! 9. **H3** — une publication **réellement exécutée** dont la réponse se perd
//!    laisse un effet durable dans Obrain et un cycle local `Indetermine` ; K et O
//!    survivent à `settle`, une réception identique ne republie pas, une
//!    consultation ne crée rien, et seule une relecture concordante confirme —
//!    l'apprentissage étant compté une fois.
//! 10. **H3, second témoin** — une **absence définitivement attestée** par
//!     relecture libère la ré-publication, qui crée alors l'effet, une seule fois.
//! 11. **H3, preuves insuffisantes** — un effet présent mais **illisible** ne se
//!     confirme pas « par défaut » : le cycle reste indéterminé et la tentative
//!     reste en souffrance pour une mesure ultérieure.
//!
//! Le cas 9 n'est PAS le cas 7 : au cas 7 la lecture de déduplication échoue avant
//! toute écriture (Obrain vide), au cas 9 l'écriture a eu lieu et c'est la réponse
//! qui manque (Obrain porte l'observation). Les deux signatures sont opposées, et
//! les contre-épreuves les distinguent explicitement.
//!
//! Le nœud `observation` porte le **tuple complet**, avec `decision_id` comme
//! champ propre (pas seulement fondu dans une clé composite) : c'est lui que les
//! contre-épreuves relisent champ par champ.

use std::collections::HashMap;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use obrain_common::types::Value;
use this::storage::obrain::ObrainStore;
use this_agent::Stop;
use this_harness::attribution::{
    Attribution, Definition, Etat, Observation, Reception, Record, Resultat, RésolutionBackend,
    Tentative,
};
use this_harness::cognition::{LearnedClass, Prototype, SemanticEncoder, TaskRepr, classer};
use this_harness::routing::{
    ArmKey, ArmStats, Candidate, Capabilities, CostBasis, Decision, Learning, MemoryPool, Pick,
    PriorHints, RouterSpec, RoutingSettings, SequenceSeeds, TurnInput, TurnRouter,
};

fn executer<F: std::future::Future<Output = T>, T>(f: F) -> T {
    futures::executor::block_on(f)
}

/// Compteur pour noms de répertoires uniques (pas d'uuid en dev-deps).
struct Unique;
impl Unique {
    fn suivant() -> u64 {
        use std::sync::atomic::{AtomicU64, Ordering as O};
        static N: AtomicU64 = AtomicU64::new(0);
        N.fetch_add(1, O::Relaxed)
    }
}

fn dir(tag: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!(
        "integrite_{tag}_{}_{}",
        std::process::id(),
        Unique::suivant()
    ));
    let _ = std::fs::remove_dir_all(&d);
    d
}

/// Échappe une valeur interpolée dans une requête GQL. `execute_query` n'offre pas
/// de paramètres : l'échappement est la seule barrière, documentée comme telle.
fn echapper(s: &str) -> String {
    s.replace('\\', "\\\\").replace('\'', "\\'")
}

/// Un encodeur sans lexique : des vecteurs à deux dimensions, une par représentation.
struct Enc;

impl SemanticEncoder for Enc {
    fn encode(&self, text: &str) -> Option<Vec<f32>> {
        match text {
            "A" => Some(vec![1.0, 0.0]),
            "B" => Some(vec![0.0, 1.0]),
            _ => None,
        }
    }
}

fn candidat(p: &str, m: &str) -> Candidate {
    Candidate {
        cost_basis: CostBasis::Subscription,
        healthy: Some(true),
        ..Candidate::new(
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

/// Bilan d'une tentative d'écriture d'observation — **exhaustif**, chaque tentative
/// tombe dans EXACTEMENT un seau. C'est lui qui porte la règle « ne pas compter une
/// tentative comme un effet confirmé » : une création n'est déclarée que sur commit
/// rendu ; tout le reste est nommé et compté à part.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
struct Compteurs {
    /// `record` reçu avec une paire servie.
    decisions: u64,
    /// `record` reçu avec une paire servie mais `class_key()` absent : une
    /// formation non constituée, donc rien à attribuer.
    formations: u64,
    /// `record` reçu sans paire (décision consultative).
    consultatives: u64,
    /// Observation effectivement écrite (commit rendu).
    creees: u64,
    /// Même définition re-`record`ée, ou même contenu réécrit : idempotent.
    rejeux: u64,
    /// Définition ou contenu divergent, refusé sans écrasement.
    conflits: u64,
    /// Lecture de déduplication en échec : issue indéterminée.
    indetermines: u64,
    /// Écriture refusée (bail) ou transaction en échec.
    erreurs: u64,
    /// `settle` reçu sans `record` correspondant.
    id_absent: u64,
    /// `settle` reçu avec `served = None`.
    served_none: u64,
    /// `settle` reçu avec un `Stop` non exploitable (Interrupted, Budget…).
    stop_non_exploitable: u64,
    /// Publication **réellement exécutée** dont le VERDICT n'est jamais parvenu au
    /// cycle local (perte de réponse entre le commit et l'enregistrement du
    /// verdict). À ne pas confondre avec `indetermines`, qui compte une lecture de
    /// déduplication en échec **avant** toute écriture : ici l'effet peut exister.
    reponses_perdues: u64,
    /// Résolutions qui ont CONFIRMÉ l'effet sur preuve suffisante (relecture
    /// concordante). C'est le compteur d'apprentissage : il vaut 1 par cycle, jamais
    /// davantage.
    confirmations: u64,
    /// Résolutions qui ont attesté une ABSENCE définitive (la tentative est
    /// terminée et n'a rien commité) : la ré-publication redevient licite.
    absences_resolues: u64,
    /// Résolutions restées SANS PREUVE : relecture en échec, contenu divergent, ou
    /// verdict refusé par la primitive. Le cycle reste indéterminé, rien ne se
    /// confirme.
    resolutions_sans_preuve: u64,
}

/// Le verdict qu'une **relecture d'Obrain** a réellement rendu. Il ne dit que ce
/// que la mesure a montré — jamais ce qu'on espérait trouver.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Verdict {
    /// L'effet est présent et identique au tuple attendu (K, O).
    EffetAtteste,
    /// L'effet est absent, et l'absence est certaine : la tentative est TERMINÉE
    /// (sa transaction a rendu), elle ne peut plus commiter après coup.
    AbsenceAttestee,
    /// Un contenu est présent sous la même clé, mais il diverge du tuple attendu.
    ContenuDivergent,
    /// La relecture elle-même a échoué : on ne sait pas. Ni présent ni absent.
    Illisible,
}

/// L'issue d'une tentative d'écriture, en quatre cas qu'il ne faut pas confondre.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Ecriture {
    /// Écrite (commit rendu) — le seul effet **confirmé**.
    Creee,
    /// Déduplication : identifiant présent, contenu identique. Rien d'écrit.
    RejeuIdentique,
    /// Déduplication : identifiant présent, contenu divergent. Rien d'écrit.
    Conflit,
    /// La lecture de déduplication a échoué : on ne sait pas si l'identifiant
    /// existe. On n'assimile PAS l'échec à « absent » (qui écrirait un doublon).
    Indetermine,
    /// Écriture refusée ou transaction en échec : aucun effet confirmé.
    Erreur,
}

/// Un apprenant persistant qui distingue formation et constitution, et persiste
/// des tuples complets dans Obrain.
struct Apprenant {
    store: Arc<ObrainStore>,
    scope: String,
    /// Prototypes constitués : ce qui fait d'une représentation une identité.
    prototypes: Vec<Prototype>,
    seuil: f32,
    quantification: u32,
    /// Le cycle logique d'attribution (RAM) : `record` / `recevoir` / `reserver`
    /// / `confirmer` / `echec_certain` / `indetermine` / `resoudre`. La durabilité est la responsabilité
    /// d'Obrain, pas de cette primitive.
    attribution: Mutex<Attribution>,
    compteurs: Mutex<Compteurs>,
    /// Seam de test, explicitement nommé : simule une panne de lecture de la
    /// déduplication (I/O, verrou empoisonné). Rend `Ecriture::Indetermine` au
    /// lieu d'assimiler l'échec à « absent ».
    lecture_en_echec: AtomicBool,
    /// Seam de test, explicitement nommé, pour H3 : la publication est
    /// **réellement exécutée** (le commit Obrain a lieu, l'effet est durable),
    /// mais son verdict ne rejoint jamais le cycle local — la réponse est perdue
    /// entre le commit et l'enregistrement. C'est la panne du processus qui meurt
    /// après avoir écrit : l'effet existe et personne ne le sait. Le cycle reste
    /// `Indetermine`, et il faudra RELIRE Obrain pour trancher.
    reponse_perdue: AtomicBool,
    /// Descripteurs des tentatives dont la réponse s'est perdue : ce qui rend une
    /// résolution ultérieure légitime (autorité + décision + génération). Sans eux,
    /// aucune relecture ne pourrait être appliquée au bon cycle.
    tentatives_en_souffrance: Mutex<HashMap<u128, Tentative>>,
}

impl Apprenant {
    fn nouveau(store: Arc<ObrainStore>, scope: &str, prototypes: Vec<Prototype>) -> Self {
        Self {
            store,
            scope: scope.to_string(),
            prototypes,
            seuil: 0.9,
            quantification: 1000,
            attribution: Mutex::new(Attribution::new()),
            compteurs: Mutex::new(Compteurs::default()),
            lecture_en_echec: AtomicBool::new(false),
            reponse_perdue: AtomicBool::new(false),
            tentatives_en_souffrance: Mutex::new(HashMap::new()),
        }
    }

    fn compteurs(&self) -> Compteurs {
        *self.compteurs.lock().unwrap()
    }

    /// Compte INDÉPENDANT des observations réellement présentes (requête GQL),
    /// jamais un compteur RAM.
    fn compter_observations(&self) -> usize {
        self.store
            .execute_query(
                "gql",
                &format!(
                    "MATCH (o:observation) WHERE o.scope = '{}' RETURN o.decision_id AS d",
                    echapper(&self.scope)
                ),
                100_000,
            )
            .expect("comptage des observations")
            .get("rows")
            .and_then(|v| v.as_array())
            .map(|a| a.len())
            .expect("réponse GQL sans `rows`")
    }

    fn compter_par_identite(&self, identite: &str) -> usize {
        self.store
            .execute_query(
                "gql",
                &format!(
                    "MATCH (o:observation) WHERE o.scope = '{}' AND o.identite = '{}' RETURN o.decision_id AS d",
                    echapper(&self.scope),
                    echapper(identite)
                ),
                100_000,
            )
            .expect("comptage par identité")
            .get("rows")
            .and_then(|v| v.as_array())
            .map(|a| a.len())
            .expect("réponse GQL sans `rows`")
    }

    /// Relit **le tuple complet** de chaque observation, trié : c'est la source
    /// contre laquelle les contre-épreuves mesurent l'intégrité.
    fn tuples(&self) -> Vec<(String, String, String, String, bool)> {
        let r = self
            .store
            .execute_query(
                "gql",
                &format!(
                    "MATCH (o:observation) WHERE o.scope = '{}' RETURN o.decision_id AS d, o.identite AS i, o.provider AS p, o.modele AS m, o.favorable AS f",
                    echapper(&self.scope)
                ),
                100_000,
            )
            .expect("lecture des tuples");
        let cols: Vec<String> = r
            .get("columns")
            .and_then(|c| c.as_array())
            .expect("réponse sans `columns`")
            .iter()
            .map(|v| v.as_str().expect("colonne non textuelle").to_string())
            .collect();
        let idx = |nom: &str| {
            cols.iter()
                .position(|c| c == nom)
                .unwrap_or_else(|| panic!("colonne {nom} absente : {cols:?}"))
        };
        let (id, ii, ip, im, iff) = (idx("d"), idx("i"), idx("p"), idx("m"), idx("f"));
        let mut out = Vec::new();
        for ligne in r
            .get("rows")
            .and_then(|v| v.as_array())
            .expect("réponse sans `rows`")
        {
            let g = |i: usize| {
                ligne
                    .get(i)
                    .and_then(|v| v.as_str())
                    .unwrap_or_else(|| panic!("colonne {i} non textuelle"))
                    .to_string()
            };
            let fav = ligne
                .get(iff)
                .and_then(|v| v.as_bool())
                .expect("colonne `f` non booléenne");
            out.push((g(id), g(ii), g(ip), g(im), fav));
        }
        out.sort();
        out
    }

    /// Lit si l'identifiant existe. Une lecture en échec est une ERREUR : l'appelant
    /// la traite en `Indetermine`, jamais en « absent ».
    fn lit_existence(&self, evenement: &str) -> Result<bool, String> {
        if self.lecture_en_echec.load(Ordering::Relaxed) {
            return Err("lecture simulée en échec (I/O, verrou empoisonné)".to_string());
        }
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
            .map_err(|e| format!("requête de déduplication en échec : {e}"))?;
        let rows = r.get("rows").and_then(|v| v.as_array()).ok_or_else(|| {
            "réponse sans `rows` : on ne l'assimile PAS à une absence".to_string()
        })?;
        Ok(!rows.is_empty())
    }

    /// Relit le contenu d'une observation existante, pour distinguer un rejeu
    /// identique d'un conflit.
    fn lire_contenu(&self, evenement: &str) -> Option<(String, String, String, bool)> {
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
        let ligne = r
            .get("rows")
            .and_then(|v| v.as_array())
            .cloned()
            .expect("réponse sans `rows`")
            .into_iter()
            .next()?;
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

    fn inscrire(
        &self,
        decision_id: &str,
        identite: &str,
        provider: &str,
        modele: &str,
        favorable: bool,
        evenement: &str,
    ) -> Result<(), String> {
        let mut s = self.store.db().session();
        s.begin_transaction()
            .map_err(|e| format!("begin : {e:?}"))?;
        let n = s.create_node(&["observation"]);
        s.set_node_property(n, "decision_id", Value::String(decision_id.into()));
        s.set_node_property(n, "identite", Value::String(identite.into()));
        s.set_node_property(n, "provider", Value::String(provider.into()));
        s.set_node_property(n, "modele", Value::String(modele.into()));
        s.set_node_property(n, "favorable", Value::Bool(favorable));
        s.set_node_property(n, "scope", Value::String(self.scope.clone().into()));
        s.set_node_property(n, "evenement", Value::String(evenement.into()));
        s.commit().map_err(|e| format!("commit : {e:?}"))
    }

    /// Publie l'observation courante dans Obrain, avec déduplication par identité
    /// stable (`evenement = scope/decision_id`). Le contrôle d'entrée d'écriture
    /// (`check_write`) est passé AVANT d'écrire : un écrivain périmé est refusé.
    /// C'est l'effet durable du cycle logique, jamais appelé directement par les
    /// contre-épreuves — toujours depuis [`Self::publier_cycle`].
    fn publier(
        &self,
        decision_id: &str,
        identite: &str,
        provider: &str,
        modele: &str,
        favorable: bool,
    ) -> Ecriture {
        if let Err(refus) = self.store.check_write() {
            eprintln!("observation NON écrite (écriture refusée) : {refus}");
            self.compteurs.lock().unwrap().erreurs += 1;
            return Ecriture::Erreur;
        }
        let evenement = format!("{}/{}", self.scope, decision_id);
        let existe = match self.lit_existence(&evenement) {
            Ok(e) => e,
            Err(e) => {
                eprintln!("observation INDÉTERMINÉE (lecture de déduplication en échec) : {e}");
                self.compteurs.lock().unwrap().indetermines += 1;
                return Ecriture::Indetermine;
            }
        };
        if existe {
            match self.lire_contenu(&evenement) {
                Some((i, p, m, f))
                    if i == identite && p == provider && m == modele && f == favorable =>
                {
                    self.compteurs.lock().unwrap().rejeux += 1;
                    Ecriture::RejeuIdentique
                }
                _ => {
                    self.compteurs.lock().unwrap().conflits += 1;
                    Ecriture::Conflit
                }
            }
        } else {
            match self.inscrire(
                decision_id,
                identite,
                provider,
                modele,
                favorable,
                &evenement,
            ) {
                Ok(()) => {
                    self.compteurs.lock().unwrap().creees += 1;
                    Ecriture::Creee
                }
                Err(panne) => {
                    eprintln!("observation NON écrite (échec de transaction) : {panne}");
                    self.compteurs.lock().unwrap().erreurs += 1;
                    Ecriture::Erreur
                }
            }
        }
    }

    /// Publication du cycle logique d'attribution : `reserver` (un seul écrivain),
    /// écrire l'effet durable, puis `confirmer` (succès) ou `echec_recuperable`
    /// (échec récupérable). Le descripteur de tentative lie la publication à la
    /// génération réservée : un résultat tardif d'une ancienne tentative ne peut
    /// pas confirmer une tentative différente.
    fn publier_cycle(&self, id: u128, attr: &mut Attribution) {
        let (identite, observation) = {
            let Some(cycle) = attr.etat(id) else { return };
            let Some(identite) = cycle.definition.identite.clone() else {
                return;
            };
            let Some(observation) = cycle.observation.clone() else {
                return;
            };
            (identite, observation)
        };
        let Some(tentative) = attr.reserver(id) else {
            return;
        };
        let ecriture = self.publier(
            &id.to_string(),
            &identite,
            &observation.fournisseur,
            &observation.modele,
            observation.resultat == Resultat::Succes,
        );
        if self.reponse_perdue.load(Ordering::Relaxed) {
            // **H3 — la réponse se perd.** L'écriture a EU LIEU (ou a échoué : on
            // ne le sait pas non plus), mais son verdict n'atteint jamais le cycle.
            // Le cycle ne peut donc RIEN affirmer : il reste `Indetermine`, et le
            // descripteur de tentative est conservé pour une résolution ultérieure.
            // Rien n'est confirmé, rien n'est compté comme effet — et surtout,
            // aucune ré-publication n'est possible tant que rien n'a été mesuré.
            self.compteurs.lock().unwrap().reponses_perdues += 1;
            let bascule = attr.indetermine(id, tentative);
            debug_assert!(bascule, "EnCours → Indetermine doit réussir");
            self.tentatives_en_souffrance
                .lock()
                .unwrap()
                .insert(id, tentative);
            return;
        }
        match ecriture {
            Ecriture::Creee | Ecriture::RejeuIdentique => {
                // Création ou rejeu strictement identique : effet attesté.
                self.confirmer_et_compter(attr, id, tentative);
            }
            Ecriture::Conflit => {
                // Un conflit N'EST PAS une confirmation : l'observation n'est pas
                // consolidée comme preuve d'apprentissage. On marque l'effet
                // incompatible (observable, non résolu).
                attr.resoudre(id, tentative, RésolutionBackend::Incompatible);
            }
            Ecriture::Erreur => {
                // Échec CERTAIN : ré-publication autorisée.
                attr.echec_certain(id, tentative);
            }
            Ecriture::Indetermine => {
                // Issue INCONNUE : exige une résolution, jamais une ré-publication.
                // Le descripteur est conservé : c'est lui qui rend la résolution
                // applicable au bon cycle (autorité + décision + génération).
                attr.indetermine(id, tentative);
                self.tentatives_en_souffrance
                    .lock()
                    .unwrap()
                    .insert(id, tentative);
            }
        }
    }

    /// Confirme une tentative et compte l'apprentissage **une seule fois**.
    /// C'est le SEUL point de comptage : ni la réception, ni la consultation, ni une
    /// seconde résolution ne peuvent incrémenter deux fois pour le même cycle —
    /// `Attribution::confirmer` refuse tout état déjà terminal.
    fn confirmer_et_compter(&self, attr: &mut Attribution, id: u128, tentative: Tentative) -> bool {
        if attr.confirmer(id, tentative) {
            self.compteurs.lock().unwrap().confirmations += 1;
            true
        } else {
            false
        }
    }

    /// **Résolution native** d'une publication dont la réponse s'est perdue : on
    /// RELIT Obrain — la seule source de vérité — et on applique au cycle le
    /// verdict que la mesure a rendu.
    ///
    /// K (`definition.identite`) et O (`observation`) sont relus dans le cycle :
    /// ils ont survécu à `settle`, et c'est exactement pour cela qu'on les garde —
    /// sans eux la relecture ne saurait pas quel tuple attendre.
    ///
    /// Aucune supposition n'est possible : une relecture en échec rend `Illisible`,
    /// donc une résolution `Indetermine`. Ni « présent » (qui confirmerait un effet
    /// jamais lu), ni « absent » (qui ré-publierait sur un doublon).
    ///
    /// La tentative reste « en souffrance » tant que la résolution n'a pas tranché :
    /// une mesure ultérieure, meilleure, reste donc licite.
    fn resoudre_depuis_obrain(&self, id: u128) -> Option<Verdict> {
        let tentative = self.tentatives_en_souffrance.lock().unwrap().remove(&id)?;
        // K et O, relus dans le cycle — pas reconstruits, pas supposés.
        let (identite, fournisseur, modele, favorable) = {
            let attr = self.attribution.lock().unwrap();
            let cycle = attr.etat(id)?;
            let identite = cycle.definition.identite.clone()?;
            let observation = cycle.observation.clone()?;
            (
                identite,
                observation.fournisseur,
                observation.modele,
                observation.resultat == Resultat::Succes,
            )
        };
        let evenement = format!("{}/{}", self.scope, id);
        // C'est CETTE lecture qui prouve — et elle seule.
        let verdict = match self.lit_existence(&evenement) {
            Err(detail) => {
                eprintln!("résolution SANS PREUVE (relecture en échec) : {detail}");
                Verdict::Illisible
            }
            // Absence : la tentative est terminée, sa transaction a rendu, elle ne
            // peut plus commiter après coup. L'absence est donc définitive pour
            // CETTE tentative — c'est ce que le bail d'écriture (un seul écrivain)
            // garantit.
            Ok(false) => Verdict::AbsenceAttestee,
            Ok(true) => match self.lire_contenu(&evenement) {
                Some((i, p, m, f))
                    if i == identite && p == fournisseur && m == modele && f == favorable =>
                {
                    Verdict::EffetAtteste
                }
                _ => Verdict::ContenuDivergent,
            },
        };
        let resolution = match verdict {
            Verdict::EffetAtteste => RésolutionBackend::EffetConfirme,
            Verdict::AbsenceAttestee => RésolutionBackend::AbsenceDefinitive,
            Verdict::ContenuDivergent => RésolutionBackend::Incompatible,
            Verdict::Illisible => RésolutionBackend::Indetermine,
        };
        let mut attr = self.attribution.lock().unwrap();
        // Ce que la mesure autorise, et RIEN de plus.
        let decisive = match verdict {
            Verdict::EffetAtteste => self.confirmer_et_compter(&mut attr, id, tentative),
            Verdict::AbsenceAttestee => {
                let applique = attr.resoudre(id, tentative, resolution);
                if applique {
                    self.compteurs.lock().unwrap().absences_resolues += 1;
                }
                applique
            }
            Verdict::ContenuDivergent | Verdict::Illisible => {
                // Un contenu divergent laisse le cycle observable mais NON confirmé ;
                // une relecture en échec ne tranche pas. Dans les deux cas on ne
                // confirme RIEN : la tentative reste en souffrance.
                attr.resoudre(id, tentative, resolution);
                false
            }
        };
        if !decisive {
            self.compteurs.lock().unwrap().resolutions_sans_preuve += 1;
            self.tentatives_en_souffrance
                .lock()
                .unwrap()
                .insert(id, tentative);
        }
        Some(verdict)
    }
}

#[async_trait(?Send)]
impl Learning for Apprenant {
    async fn classify(&self, repr: &TaskRepr) -> this_harness::cognition::Classification {
        classer(repr, &self.prototypes, self.seuil, self.quantification)
    }

    async fn arms(&self, class: &str) -> Vec<ArmStats> {
        vec![
            ArmStats::fresh(ArmKey::new(class, "a", "m1"), 1.0, 1.0),
            ArmStats::fresh(ArmKey::new(class, "b", "m2"), 1.0, 1.0),
        ]
    }

    fn hints(&self) -> PriorHints {
        PriorHints::new()
    }

    async fn record(&self, d: &Decision) {
        if d.used.is_none() {
            self.compteurs.lock().unwrap().consultatives += 1;
            return;
        }
        self.compteurs.lock().unwrap().decisions += 1;
        match d.class_key() {
            Some(k) => {
                let definition = Definition {
                    identite: Some(k),
                    prevue: d
                        .chosen
                        .as_ref()
                        .map(|p| (p.provider_id.clone(), p.model.clone())),
                    servie: d
                        .used
                        .as_ref()
                        .map(|p| (p.provider_id.clone(), p.model.clone())),
                };
                match self.attribution.lock().unwrap().record(d.id, definition) {
                    Record::Enregistre => {}
                    Record::Rejeu => {
                        self.compteurs.lock().unwrap().rejeux += 1;
                    }
                    Record::Conflit => {
                        self.compteurs.lock().unwrap().conflits += 1;
                    }
                }
            }
            None => {
                // Formation non constituée : aucune identité à attribuer. C'est le
                // cas qui garantit « 0 observation cognitive commune ».
                self.compteurs.lock().unwrap().formations += 1;
            }
        }
    }

    async fn settle(&self, id: u128, served: Option<&Pick>, stop: &Stop, _code: Option<&str>) {
        let Some(p) = served else {
            self.compteurs.lock().unwrap().served_none += 1;
            return;
        };
        let favorable = match stop {
            Stop::Done => true,
            Stop::Failed(_) => false,
            _ => {
                self.compteurs.lock().unwrap().stop_non_exploitable += 1;
                return;
            }
        };
        let observation = Observation {
            fournisseur: p.provider_id.clone(),
            modele: p.model.clone(),
            resultat: if favorable {
                Resultat::Succes
            } else {
                Resultat::Echec
            },
        };
        let mut attr = self.attribution.lock().unwrap();
        match attr.recevoir(id, observation) {
            Reception::Absent => {
                self.compteurs.lock().unwrap().id_absent += 1;
            }
            Reception::Termine => {
                // Cycle déjà terminal (confirmé ou abandonné) : rien à publier.
            }
            Reception::Rejeu => {
                // Observation déjà reçue, cycle non terminal (`Recu` après un échec
                // récupérable) : nouvelle tentative de publication, idempotente.
                self.publier_cycle(id, &mut attr);
            }
            Reception::Divergente | Reception::Incompatible => {
                self.compteurs.lock().unwrap().conflits += 1;
            }
            Reception::Recu => {
                self.publier_cycle(id, &mut attr);
            }
        }
    }
}

fn routeur_pour(apprenant: Rc<Apprenant>) -> TurnRouter {
    let spec = RouterSpec::new(
        Rc::new(MemoryPool::new(vec![
            candidat("a", "m1"),
            candidat("b", "m2"),
        ])),
        Rc::new(SequenceSeeds::from(1u128)),
        "a".to_string(),
        "m1".to_string(),
    )
    .learning(apprenant.clone())
    .encoder(Rc::new(Enc))
    .auto()
    .settings(RoutingSettings {
        exploration_epsilon: 0.0,
        ..Default::default()
    });
    TurnRouter::new(spec)
}

fn tour(texte: &str) -> TurnInput<'_> {
    TurnInput {
        text: texte,
        has_images: false,
        hints: Default::default(),
        honored: true,
    }
}

// ---------------------------------------------------------------------------
// 1. Deux formations distinctes non constituées → 0 observation commune
// ---------------------------------------------------------------------------

#[test]
fn deux_formations_distinctes_non_constituees_laissent_obrain_vide() {
    let d = dir("formations");
    let store = ObrainStore::open(&d).expect("ouverture");
    // Aucun prototype : toute représentation valide est une formation (class: None).
    let apprenant = Rc::new(Apprenant::nouveau(
        store.clone(),
        "scope-formations",
        vec![],
    ));
    let routeur = routeur_pour(apprenant.clone());

    let mut projections = Vec::new();
    for texte in ["A", "B"] {
        let routed = executer(routeur.route(tour(texte)));
        let dec = routed.decision.clone().expect("décision exigée");
        assert!(dec.applied, "décision appliquée ({texte})");
        assert!(
            dec.cognitive_class.is_none(),
            "aucune identité constituée ({texte})"
        );
        let repr = TaskRepr {
            embedding: Enc.encode(texte),
            ..Default::default()
        };
        let c = executer(apprenant.classify(&repr));
        let f = c.formation.as_ref().expect("proposition de formation");
        projections.push(f.projection.as_str().to_string());

        executer(routeur.record(&dec));
        executer(routeur.settle(&Stop::Done, None, true));
    }

    // Les deux formations sont RÉELLEMENT distinctes : la contre-épreuve n'est pas
    // vide (deux fois la même projection figée ne prouverait rien).
    assert_ne!(
        projections[0], projections[1],
        "deux formations distinctes doivent produire deux projections distinctes"
    );

    // Rien dans Obrain : `class_key()` est None, `record` n'attribue rien, `settle`
    // n'écrit rien. Vérifié par requête GQL, pas par un compteur RAM.
    assert_eq!(
        apprenant.compter_observations(),
        0,
        "aucune observation cognitive commune ne doit être écrite"
    );
    let c = apprenant.compteurs();
    assert_eq!(
        c.formations, 2,
        "deux décisions reçues sans identité constituée"
    );
    assert_eq!(c.creees, 0, "aucune création");

    store.commit_wal().expect("flush");
    drop(routeur);
    drop(apprenant);
    drop(store);
    let _ = std::fs::remove_dir_all(&d);
}

// ---------------------------------------------------------------------------
// 2. Constitution → observation persistée, tuple complet
// ---------------------------------------------------------------------------

#[test]
fn une_identite_constituee_persiste_un_tuple_complet() {
    let d = dir("constitution");
    let store = ObrainStore::open(&d).expect("ouverture");
    let apprenant = Rc::new(Apprenant::nouveau(
        store.clone(),
        "scope-const",
        vec![Prototype {
            class: LearnedClass("repr.K1".into()),
            vector: vec![1.0, 0.0],
        }],
    ));
    let routeur = routeur_pour(apprenant.clone());

    let routed = executer(routeur.route(tour("A")));
    let dec = routed.decision.clone().expect("décision exigée");
    assert!(dec.applied);
    assert_eq!(
        dec.cognitive_class.as_deref(),
        Some("repr.K1"),
        "identité constituée"
    );
    let decision_id = dec.id.to_string();
    let used = dec.used.clone().expect("paire servie");

    executer(routeur.record(&dec));
    executer(routeur.settle(&Stop::Done, None, true));

    // Un tuple COMPLET : decision_id, identite, provider, modele, resultat.
    let tuples = apprenant.tuples();
    assert_eq!(tuples.len(), 1, "une seule observation");
    assert_eq!(
        tuples[0],
        (
            decision_id,
            "repr.K1".to_string(),
            used.provider_id,
            used.model,
            true,
        ),
        "tuple complet persisté"
    );

    store.commit_wal().expect("flush");
    drop(routeur);
    drop(apprenant);
    drop(store);
    let _ = std::fs::remove_dir_all(&d);
}

// ---------------------------------------------------------------------------
// 3. Décisions entrelacées réglées dans l'ordre inverse
// ---------------------------------------------------------------------------

#[test]
fn attribution_suit_la_decision_pas_la_derniere_classe_consultee() {
    let d = dir("attribution");
    let store = ObrainStore::open(&d).expect("ouverture");
    let apprenant = Rc::new(Apprenant::nouveau(
        store.clone(),
        "scope-attr",
        vec![
            Prototype {
                class: LearnedClass("repr.K1".into()),
                vector: vec![1.0, 0.0],
            },
            Prototype {
                class: LearnedClass("repr.K2".into()),
                vector: vec![0.0, 1.0],
            },
        ],
    ));
    let routeur = routeur_pour(apprenant.clone());

    let d1 = executer(routeur.route(tour("A"))).decision.clone().unwrap();
    let d2 = executer(routeur.route(tour("B"))).decision.clone().unwrap();
    assert_eq!(d1.cognitive_class.as_deref(), Some("repr.K1"));
    assert_eq!(d2.cognitive_class.as_deref(), Some("repr.K2"));

    executer(routeur.record(&d1));
    executer(routeur.record(&d2));

    // Consultations intermédiaires : la « dernière classe vue » devient K1 (le
    // piège d'un canal latéral). L'attribution doit suivre LA DÉCISION, pas ce
    // dernier passage.
    executer(apprenant.arms("repr.K2"));
    executer(apprenant.arms("repr.K1"));

    // Réglées dans l'ordre INVERSE : D2 puis D1.
    executer(apprenant.settle(d2.id, d2.used.as_ref(), &Stop::Done, None));
    executer(apprenant.settle(d1.id, d1.used.as_ref(), &Stop::Done, None));

    assert_eq!(
        apprenant.compter_par_identite("repr.K1"),
        1,
        "D1 doit être attribuée à K1"
    );
    assert_eq!(
        apprenant.compter_par_identite("repr.K2"),
        1,
        "D2 doit être attribuée à K2"
    );
    assert_eq!(apprenant.compter_observations(), 2);

    store.commit_wal().expect("flush");
    drop(routeur);
    drop(apprenant);
    drop(store);
    let _ = std::fs::remove_dir_all(&d);
}

// ---------------------------------------------------------------------------
// 4. record identique = rejeu ; record divergent = conflit
// ---------------------------------------------------------------------------

#[test]
fn record_identique_est_un_rejeu_et_record_divergent_est_un_conflit() {
    let d = dir("rejeu-conflit");
    let store = ObrainStore::open(&d).expect("ouverture");
    let apprenant = Rc::new(Apprenant::nouveau(
        store.clone(),
        "scope-rc",
        vec![Prototype {
            class: LearnedClass("repr.K1".into()),
            vector: vec![1.0, 0.0],
        }],
    ));
    let routeur = routeur_pour(apprenant.clone());

    let dec = executer(routeur.route(tour("A"))).decision.clone().unwrap();
    assert_eq!(dec.cognitive_class.as_deref(), Some("repr.K1"));
    let pick = dec.used.clone().unwrap();

    // Premier cycle complet : record → settle (créé, effet confirmé).
    executer(apprenant.record(&dec));
    executer(apprenant.settle(dec.id, Some(&pick), &Stop::Done, None));
    assert_eq!(apprenant.compter_observations(), 1);

    // record identique : rejeu, pas de double écriture.
    executer(apprenant.record(&dec));

    // record divergent (même identifiant, identité différente) : conflit, sans
    // écrasement de l'état d'origine.
    let mut divergent = dec.clone();
    divergent.cognitive_class = Some("repr.K2".into());
    executer(apprenant.record(&divergent));

    let tuples = apprenant.tuples();
    assert_eq!(tuples.len(), 1, "ni double écriture ni écrasement");
    assert_eq!(
        tuples[0],
        (
            dec.id.to_string(),
            "repr.K1".to_string(),
            pick.provider_id,
            pick.model,
            true,
        ),
        "le premier tuple est conservé"
    );
    let c = apprenant.compteurs();
    assert_eq!(c.creees, 1);
    assert_eq!(c.rejeux, 1);
    assert_eq!(c.conflits, 1);

    store.commit_wal().expect("flush");
    drop(routeur);
    drop(apprenant);
    drop(store);
    let _ = std::fs::remove_dir_all(&d);
}

// ---------------------------------------------------------------------------
// 5. Rejeu complet record → settle → record → settle
// ---------------------------------------------------------------------------

#[test]
fn rejeu_complet_record_settle_record_settle_n_ecrit_qu_une_fois() {
    let d = dir("rejeu-cycle");
    let store = ObrainStore::open(&d).expect("ouverture");
    let apprenant = Rc::new(Apprenant::nouveau(
        store.clone(),
        "scope-rejeu",
        vec![Prototype {
            class: LearnedClass("repr.K1".into()),
            vector: vec![1.0, 0.0],
        }],
    ));
    let routeur = routeur_pour(apprenant.clone());
    let dec = executer(routeur.route(tour("A"))).decision.clone().unwrap();
    let pick = dec.used.clone().unwrap();

    // Premier cycle complet : record → settle (écrit).
    executer(apprenant.record(&dec));
    executer(apprenant.settle(dec.id, Some(&pick), &Stop::Done, None));
    assert_eq!(apprenant.compter_observations(), 1);

    // Rejeu complet du MÊME identifiant : record → settle ne réécrit pas.
    executer(apprenant.record(&dec));
    executer(apprenant.settle(dec.id, Some(&pick), &Stop::Done, None));
    assert_eq!(
        apprenant.compter_observations(),
        1,
        "le rejeu complet n'écrit qu'une fois"
    );

    let c = apprenant.compteurs();
    assert_eq!(c.creees, 1);
    assert_eq!(c.rejeux, 1, "le second passage est reconnu comme rejeu");

    store.commit_wal().expect("flush");
    drop(routeur);
    drop(apprenant);
    drop(store);
    let _ = std::fs::remove_dir_all(&d);
}

// ---------------------------------------------------------------------------
// 6. Échec d'écriture puis récupération
// ---------------------------------------------------------------------------

#[test]
fn echec_d_ecriture_puis_recuperation() {
    use this::storage::writer_lease::FileAuthority;

    let d = dir("echec-recuperation");
    let store = ObrainStore::open(&d).expect("ouverture");
    let apprenant = Rc::new(Apprenant::nouveau(
        store.clone(),
        "scope-echec",
        vec![Prototype {
            class: LearnedClass("repr.K1".into()),
            vector: vec![1.0, 0.0],
        }],
    ));
    let routeur = routeur_pour(apprenant.clone());
    let authority = FileAuthority::new(d.as_path());
    store
        .enable_write_fencing_with(&authority)
        .expect("bail armé");

    // Deux décisions distinctes, même identité constituée.
    let dec1 = executer(routeur.route(tour("A"))).decision.clone().unwrap();
    let dec2 = executer(routeur.route(tour("A"))).decision.clone().unwrap();
    let pick1 = dec1.used.clone().unwrap();
    let pick2 = dec2.used.clone().unwrap();

    // Témoin : le détenteur du bail écrit.
    executer(apprenant.record(&dec1));
    executer(apprenant.settle(dec1.id, Some(&pick1), &Stop::Done, None));
    assert_eq!(apprenant.compter_observations(), 1);

    // Un autre écrivain a repris le shard : celui-ci est périmé.
    store.fence_write_lease_for_test(2);
    executer(apprenant.record(&dec2));
    executer(apprenant.settle(dec2.id, Some(&pick2), &Stop::Done, None));

    // L'échec n'est PAS compté comme une création, et ne laisse rien dans Obrain.
    assert_eq!(
        apprenant.compter_observations(),
        1,
        "le refus n'a rien écrit"
    );
    let c = apprenant.compteurs();
    assert_eq!(c.creees, 1);
    assert_eq!(c.erreurs, 1, "l'échec est compté dans `erreurs`");

    // Récupération : le bail est rendu, la même publication passe.
    store.release_write_fencing();
    executer(apprenant.settle(dec2.id, Some(&pick2), &Stop::Done, None));
    assert_eq!(apprenant.compter_observations(), 2);
    let c2 = apprenant.compteurs();
    assert_eq!(c2.creees, 2, "la tentative rattrapée crée");
    assert_eq!(c2.erreurs, 1, "l'échec passé reste compté");

    store.commit_wal().expect("flush");
    drop(routeur);
    drop(apprenant);
    drop(store);
    let _ = std::fs::remove_dir_all(&d);
}

// ---------------------------------------------------------------------------
// 7. Résultat indéterminé : jamais un effet confirmé
// ---------------------------------------------------------------------------

#[test]
fn resultat_indetermine_n_est_pas_un_effet_confirme() {
    let d = dir("indetermine");
    let store = ObrainStore::open(&d).expect("ouverture");
    let apprenant = Rc::new(Apprenant::nouveau(
        store.clone(),
        "scope-indet",
        vec![Prototype {
            class: LearnedClass("repr.K1".into()),
            vector: vec![1.0, 0.0],
        }],
    ));
    let routeur = routeur_pour(apprenant.clone());
    let dec = executer(routeur.route(tour("A"))).decision.clone().unwrap();
    let pick = dec.used.clone().unwrap();

    // La lecture de déduplication échoue : l'issue est INDÉTERMINÉE. On ne peut
    // pas savoir si l'identifiant existe, et on ne prétend ni « absent » (qui
    // écrirait un doublon) ni « présent ».
    apprenant.lecture_en_echec.store(true, Ordering::Relaxed);
    executer(apprenant.record(&dec));
    executer(apprenant.settle(dec.id, Some(&pick), &Stop::Done, None));
    apprenant.lecture_en_echec.store(false, Ordering::Relaxed);

    // L'indétermination n'est PAS un effet confirmé : rien d'écrit, rien compté
    // comme création.
    assert_eq!(
        apprenant.compter_observations(),
        0,
        "aucune observation écrite"
    );
    let c = apprenant.compteurs();
    assert_eq!(c.creees, 0, "l'indétermination n'est pas une création");
    assert_eq!(c.indetermines, 1, "…mais elle est nommée");

    // La lecture est rétablie : SANS résolution explicite, une publication
    // indéterminée ne se rejoue pas (le cycle reste Indetermine, en attente du
    // verdict du backend — pas de ré-publication aveugle).
    executer(apprenant.settle(dec.id, Some(&pick), &Stop::Done, None));
    assert_eq!(
        apprenant.compter_observations(),
        0,
        "pas de ré-publication aveugle sans résolution"
    );

    store.commit_wal().expect("flush");
    drop(routeur);
    drop(apprenant);
    drop(store);
    let _ = std::fs::remove_dir_all(&d);
}

// ---------------------------------------------------------------------------
// 8. Fermeture / réouverture : les tuples complets survivent
// ---------------------------------------------------------------------------

#[test]
fn fermeture_reouverture_conserve_les_tuples_complets() {
    let d = dir("fermeture");
    let scope = "scope-ferm";
    let prototypes = vec![Prototype {
        class: LearnedClass("repr.K1".into()),
        vector: vec![1.0, 0.0],
    }];
    let attendu;

    {
        let store = ObrainStore::open(&d).expect("ouverture");
        let apprenant = Rc::new(Apprenant::nouveau(store.clone(), scope, prototypes.clone()));
        let routeur = routeur_pour(apprenant.clone());

        // Deux décisions distinctes, deux résultats différents : des tuples
        // complets et distinguables par `decision_id` et `resultat`.
        let dec1 = executer(routeur.route(tour("A"))).decision.clone().unwrap();
        let dec2 = executer(routeur.route(tour("A"))).decision.clone().unwrap();
        let pick1 = dec1.used.clone().unwrap();
        let pick2 = dec2.used.clone().unwrap();

        executer(apprenant.record(&dec1));
        executer(apprenant.settle(dec1.id, Some(&pick1), &Stop::Done, None));
        executer(apprenant.record(&dec2));
        executer(apprenant.settle(dec2.id, Some(&pick2), &Stop::Failed("panne".into()), None));

        attendu = apprenant.tuples();
        assert_eq!(attendu.len(), 2);
        store.commit_wal().expect("flush");
        drop(routeur);
        drop(apprenant);
        drop(store);
    }

    {
        let store = ObrainStore::open(&d).expect("réouverture");
        let apprenant = Rc::new(Apprenant::nouveau(store.clone(), scope, prototypes.clone()));
        assert_eq!(
            apprenant.tuples(),
            attendu,
            "les tuples complets survivent à la réouverture"
        );
        assert_eq!(apprenant.compter_observations(), 2);
        drop(apprenant);
        drop(store);
    }

    let _ = std::fs::remove_dir_all(&d);
}

// ---------------------------------------------------------------------------
// 9. H3 — publication RÉELLE, perte de réponse, résolution par mesure
// ---------------------------------------------------------------------------

/// **H3 n'est pas le cas 7.** Au cas 7
/// (`resultat_indetermine_n_est_pas_un_effet_confirme`), la lecture de
/// déduplication échoue **avant** toute écriture : Obrain reste vide. Ici, l'effet
/// durable est **réellement commité dans Obrain**, et c'est la RÉPONSE du commit
/// qui se perd — la panne du processus qui meurt après avoir écrit. Les deux
/// signatures observables sont opposées : 1 observation dans Obrain avec un cycle
/// local indéterminé (cas 9), contre 0 observation et un cycle indéterminé (cas 7).
///
/// Ce que H3 vérifie, dans l'ordre :
///   a. l'effet persistant existe — lu en GQL, jamais par un compteur RAM ;
///   b. le cycle local reste `Indetermine`, K et O conservés au-delà de `settle` ;
///   c. une réception identique ne republie pas et ne confirme pas ;
///   d. une consultation ne crée aucun effet et ne confirme pas ;
///   e. une résolution positive confirme, sur preuve RELUE et non sur espoir ;
///   f. l'apprentissage est compté une seule fois.
#[test]
fn h3_publication_reelle_perte_de_reponse_puis_resolution_par_mesure() {
    let d = dir("h3-perte-reponse");
    let scope = "scope-h3";
    let store = ObrainStore::open(&d).expect("ouverture");
    let apprenant = Rc::new(Apprenant::nouveau(
        store.clone(),
        scope,
        vec![Prototype {
            class: LearnedClass("repr.K1".into()),
            vector: vec![1.0, 0.0],
        }],
    ));
    let routeur = routeur_pour(apprenant.clone());
    let dec = executer(routeur.route(tour("A"))).decision.clone().unwrap();
    assert_eq!(dec.cognitive_class.as_deref(), Some("repr.K1"));
    let pick = dec.used.clone().expect("paire servie");
    let id = dec.id;
    let tuple_attendu = (
        id.to_string(),
        "repr.K1".to_string(),
        pick.provider_id.clone(),
        pick.model.clone(),
        true,
    );

    // --- Publication RÉELLE dont la réponse se perd -------------------------
    executer(apprenant.record(&dec));
    apprenant.reponse_perdue.store(true, Ordering::Relaxed);
    executer(apprenant.settle(id, Some(&pick), &Stop::Done, None));
    apprenant.reponse_perdue.store(false, Ordering::Relaxed);

    // (a) L'EFFET PERSISTANT EXISTE — mesuré dans Obrain.
    assert_eq!(
        apprenant.tuples(),
        vec![tuple_attendu.clone()],
        "l'écriture a bien eu lieu : le tuple complet est durable"
    );

    // (b) LE CYCLE LOCAL RESTE INDÉTERMINÉ, et K / O survivent à `settle`.
    {
        let attr = apprenant.attribution.lock().unwrap();
        let cycle = attr.etat(id).expect("cycle conservé");
        assert_eq!(
            cycle.etat,
            Etat::Indetermine,
            "le verdict n'est pas arrivé : issue inconnue, pas confirmée"
        );
        assert_eq!(
            cycle.definition.identite.as_deref(),
            Some("repr.K1"),
            "K conservée au-delà de settle"
        );
        let o = cycle
            .observation
            .as_ref()
            .expect("O conservée au-delà de settle");
        assert_eq!(o.fournisseur, pick.provider_id);
        assert_eq!(o.modele, pick.model);
        assert_eq!(o.resultat, Resultat::Succes);
    }
    let c = apprenant.compteurs();
    assert_eq!(
        (c.creees, c.reponses_perdues, c.confirmations),
        (1, 1, 0),
        "écrit une fois, réponse perdue, rien de confirmé"
    );
    {
        let mut attr = apprenant.attribution.lock().unwrap();
        assert!(
            attr.reserver(id).is_none(),
            "aucune ré-publication tant que rien n'a été mesuré"
        );
    }

    // (c) RÉCEPTION IDENTIQUE NE REPUBLIE PAS. La même observation est relivrée
    // (livraison « au moins une fois ») : ce n'est PAS une preuve d'effet.
    executer(apprenant.settle(id, Some(&pick), &Stop::Done, None));
    assert_eq!(
        apprenant.compter_observations(),
        1,
        "pas de seconde écriture"
    );
    assert_eq!(apprenant.compteurs().creees, 1);
    assert_eq!(
        apprenant.attribution.lock().unwrap().etat(id).unwrap().etat,
        Etat::Indetermine,
        "une réception n'est pas une preuve : le cycle reste indéterminé"
    );

    // (d) CONSULTATION NE CRÉE PAS D'EFFET — sur le MÊME identifiant.
    executer(apprenant.arms("repr.K1"));
    // Décision CONSULTATIVE : la paire n'est pas servie. `Decision` est
    // `non_exhaustive`, donc on part de la décision réelle pour en retirer la paire.
    let mut consultative = dec.clone();
    consultative.used = None;
    executer(apprenant.record(&consultative));
    assert_eq!(
        apprenant.compter_observations(),
        1,
        "une consultation n'écrit rien"
    );
    assert_eq!(apprenant.compteurs().consultatives, 1);
    assert_eq!(
        apprenant.attribution.lock().unwrap().etat(id).unwrap().etat,
        Etat::Indetermine,
        "une consultation ne confirme pas"
    );

    // (e) RÉSOLUTION POSITIVE : les preuves sont suffisantes (effet présent ET
    // identique). C'est la relecture qui confirme, pas le temps qui passe.
    assert_eq!(
        apprenant.resoudre_depuis_obrain(id),
        Some(Verdict::EffetAtteste)
    );
    assert_eq!(
        apprenant.attribution.lock().unwrap().etat(id).unwrap().etat,
        Etat::Confirme
    );
    assert_eq!(apprenant.compter_observations(), 1, "résoudre n'écrit pas");

    // (f) L'APPRENTISSAGE EST COMPTÉ UNE SEULE FOIS.
    assert_eq!(apprenant.compteurs().confirmations, 1);
    assert_eq!(
        apprenant.resoudre_depuis_obrain(id),
        None,
        "plus de tentative en souffrance : rien à résoudre"
    );
    assert_eq!(
        apprenant.compteurs().confirmations,
        1,
        "une seconde résolution ne double pas la comptabilité"
    );
    // Et un rejeu complet après confirmation n'ajoute rien.
    executer(apprenant.record(&dec));
    executer(apprenant.settle(id, Some(&pick), &Stop::Done, None));
    assert_eq!(apprenant.compter_observations(), 1);
    assert_eq!(apprenant.compteurs().confirmations, 1);
    assert_eq!(apprenant.tuples(), vec![tuple_attendu]);

    store.commit_wal().expect("flush");
    drop(routeur);
    drop(apprenant);
    drop(store);
    let _ = std::fs::remove_dir_all(&d);
}

// ---------------------------------------------------------------------------
// 10. H3, second témoin — reprise après absence DÉFINITIVEMENT attestée
// ---------------------------------------------------------------------------

/// La perte de réponse ne condamne pas le cycle : quand la relecture ATTESTE une
/// absence, la ré-publication redevient licite et crée réellement l'effet.
///
/// L'absence est dite « définitive » pour une raison vérifiable, pas par confort :
/// la tentative est TERMINÉE (sa transaction a rendu — ici l'écriture a été refusée
/// d'entrée par un bail périmé), elle ne peut donc plus commiter après coup, et le
/// bail d'écriture garantit un seul écrivain. Une absence mesurée dans ces
/// conditions engage l'avenir de CETTE tentative.
#[test]
fn h3_reprise_apres_absence_definitivement_attestee() {
    use this::storage::writer_lease::FileAuthority;

    let d = dir("h3-absence");
    let scope = "scope-h3-abs";
    let store = ObrainStore::open(&d).expect("ouverture");
    let apprenant = Rc::new(Apprenant::nouveau(
        store.clone(),
        scope,
        vec![Prototype {
            class: LearnedClass("repr.K1".into()),
            vector: vec![1.0, 0.0],
        }],
    ));
    let routeur = routeur_pour(apprenant.clone());
    let authority = FileAuthority::new(d.as_path());
    store
        .enable_write_fencing_with(&authority)
        .expect("bail armé");

    let dec = executer(routeur.route(tour("A"))).decision.clone().unwrap();
    let pick = dec.used.clone().expect("paire servie");
    let id = dec.id;

    // Un autre écrivain a repris le shard : cette publication sera refusée POUR SÛR.
    store.fence_write_lease_for_test(2);

    // Publication tentée, réponse perdue : on ignore ce qui s'est passé.
    executer(apprenant.record(&dec));
    apprenant.reponse_perdue.store(true, Ordering::Relaxed);
    executer(apprenant.settle(id, Some(&pick), &Stop::Done, None));
    apprenant.reponse_perdue.store(false, Ordering::Relaxed);

    assert_eq!(apprenant.compter_observations(), 0, "rien n'a été écrit");
    let c = apprenant.compteurs();
    assert_eq!((c.erreurs, c.reponses_perdues, c.creees), (1, 1, 0));
    assert_eq!(
        apprenant.attribution.lock().unwrap().etat(id).unwrap().etat,
        Etat::Indetermine,
        "issue inconnue : on ne conclut rien"
    );

    // La RELECTURE atteste l'absence : rien sous la clé, et plus rien ne peut y
    // apparaître puisque la tentative est terminée.
    assert_eq!(
        apprenant.resoudre_depuis_obrain(id),
        Some(Verdict::AbsenceAttestee)
    );
    let c = apprenant.compteurs();
    assert_eq!(
        (c.absences_resolues, c.confirmations),
        (1, 0),
        "une absence attestée ne confirme RIEN"
    );
    assert_eq!(
        apprenant.attribution.lock().unwrap().etat(id).unwrap().etat,
        Etat::Recu,
        "la ré-publication redevient licite"
    );

    // Le bail est rendu : la reprise écrit réellement, et une seule fois.
    store.release_write_fencing();
    executer(apprenant.settle(id, Some(&pick), &Stop::Done, None));

    assert_eq!(
        apprenant.compter_observations(),
        1,
        "la reprise crée l'effet"
    );
    assert_eq!(
        apprenant.tuples(),
        vec![(
            id.to_string(),
            "repr.K1".to_string(),
            pick.provider_id.clone(),
            pick.model.clone(),
            true,
        )]
    );
    let c = apprenant.compteurs();
    assert_eq!(
        (c.creees, c.confirmations, c.erreurs),
        (1, 1, 1),
        "une création, un apprentissage, l'échec passé toujours compté"
    );
    assert_eq!(
        apprenant.attribution.lock().unwrap().etat(id).unwrap().etat,
        Etat::Confirme
    );

    // Une dernière livraison identique ne change plus rien.
    executer(apprenant.settle(id, Some(&pick), &Stop::Done, None));
    assert_eq!(apprenant.compter_observations(), 1);
    assert_eq!(apprenant.compteurs().confirmations, 1);

    store.commit_wal().expect("flush");
    drop(routeur);
    drop(apprenant);
    drop(store);
    let _ = std::fs::remove_dir_all(&d);
}

// ---------------------------------------------------------------------------
// 11. H3, preuves INSUFFISANTES — effet présent mais illisible
// ---------------------------------------------------------------------------

/// L'effet existe dans Obrain, mais la relecture qui devait l'attester échoue.
/// Rien ne se confirme alors « par défaut » : l'issue reste indéterminée et
/// l'apprentissage n'est pas compté. Le cycle n'est pas perdu pour autant — la
/// tentative reste en souffrance, et une mesure ultérieure, réussie, tranchera.
///
/// Signature, encore une fois opposée au cas 7 : ici Obrain contient l'observation
/// (1 ligne) et le cycle reste indéterminé ; au cas 7 Obrain est vide et le cycle
/// reste indéterminé. Le cas 11 n'est donc pas une variante du cas 7 — c'est le
/// cas où l'ÉCRITURE a eu lieu et où la LECTURE manque.
#[test]
fn h3_effet_present_mais_illisible_ne_confirme_pas() {
    let d = dir("h3-illisible");
    let scope = "scope-h3-ill";
    let store = ObrainStore::open(&d).expect("ouverture");
    let apprenant = Rc::new(Apprenant::nouveau(
        store.clone(),
        scope,
        vec![Prototype {
            class: LearnedClass("repr.K1".into()),
            vector: vec![1.0, 0.0],
        }],
    ));
    let routeur = routeur_pour(apprenant.clone());
    let dec = executer(routeur.route(tour("A"))).decision.clone().unwrap();
    let pick = dec.used.clone().expect("paire servie");
    let id = dec.id;

    // Publication réelle, réponse perdue.
    executer(apprenant.record(&dec));
    apprenant.reponse_perdue.store(true, Ordering::Relaxed);
    executer(apprenant.settle(id, Some(&pick), &Stop::Done, None));
    apprenant.reponse_perdue.store(false, Ordering::Relaxed);
    assert_eq!(apprenant.compter_observations(), 1, "l'effet est bien là");

    // La relecture de résolution échoue : preuve insuffisante.
    apprenant.lecture_en_echec.store(true, Ordering::Relaxed);
    assert_eq!(
        apprenant.resoudre_depuis_obrain(id),
        Some(Verdict::Illisible),
        "une relecture en échec ne prétend ni présent ni absent"
    );
    apprenant.lecture_en_echec.store(false, Ordering::Relaxed);

    assert_eq!(
        apprenant.attribution.lock().unwrap().etat(id).unwrap().etat,
        Etat::Indetermine,
        "rien ne se confirme sans preuve"
    );
    let c = apprenant.compteurs();
    assert_eq!(
        (
            c.confirmations,
            c.absences_resolues,
            c.resolutions_sans_preuve
        ),
        (0, 0, 1),
        "aucune résolution décisive, et cela se voit dans la comptabilité"
    );
    assert_eq!(
        apprenant.compter_observations(),
        1,
        "l'effet reste dans Obrain"
    );

    // La mesure redevient disponible : elle tranche, et l'apprentissage est compté
    // une seule fois.
    assert_eq!(
        apprenant.resoudre_depuis_obrain(id),
        Some(Verdict::EffetAtteste)
    );
    assert_eq!(
        apprenant.attribution.lock().unwrap().etat(id).unwrap().etat,
        Etat::Confirme
    );
    assert_eq!(apprenant.compteurs().confirmations, 1);
    assert_eq!(
        apprenant.resoudre_depuis_obrain(id),
        None,
        "plus rien à résoudre : la comptabilité ne peut pas doubler"
    );

    store.commit_wal().expect("flush");
    drop(routeur);
    drop(apprenant);
    drop(store);
    let _ = std::fs::remove_dir_all(&d);
}
