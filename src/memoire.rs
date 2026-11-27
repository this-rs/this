//! Le service MÉMOIRE du consommateur : mémoriser un fait, le corriger, le rappeler.
//!
//! ## Ce que ce module est, et ce qu'il n'est pas
//!
//! Il ne réimplémente AUCUN mécanisme cognitif : la formation, la révision et le rappel
//! restent dans Obrain (`obrain-cognitive`). Ce module est le **service** qui les appelle
//! avec les garanties que le consommateur exige — et il porte les deux contrôles que le
//! trait `CognitiveStorage` ne permet pas d'exprimer (voir la décision 13bb4d7d).
//!
//! ## Le défaut du trait, mesuré à `b45ad200`
//!
//! `CognitiveStorage::create_node(&self, label, properties) -> NodeId` : **aucun `Result`**.
//! Le trait ne peut pas porter un échec d'écriture. Un service qui rendrait cet identifiant
//! tel quel annoncerait « enregistré » sur une écriture ratée — le pire résultat possible
//! pour une mémoire, parce que la personne CROIT avoir enregistré.
//!
//! D'où les deux contrôles, appliqués dans cet ordre après CHAQUE écriture :
//!
//! 1. **erreur hors bande** — l'adaptateur de l'application expose [`SanteEcriture`] ; si une
//!    erreur y est posée, on ne confirme pas, même si un identifiant est revenu ;
//! 2. **relecture** — on relit ce qu'on vient d'écrire (`query_nodes`) ; sans relecture
//!    conforme, on ne confirme pas non plus.
//!
//! Quand l'un des deux échoue, le verdict est [`Resultat::AConfirmer`] : ni « enregistré »
//! ni « perdu ». L'indétermination est conservée, et c'est la résolution native qui décide
//! d'une reprise — pas nous.
//!
//! ## Deux couches distinctes, à ne pas confondre
//!
//! - le **fait durable** : un nœud `:Fait` écrit, relu, corrigé — c'est ce module ;
//! - l'**engramme** : une trace formée par co-activation, avec son énergie et sa décroissance
//!   — c'est `EngramManager`, et il se branchera par-dessus, pas à la place.

use std::collections::HashMap;
use std::sync::Arc;

use obrain_cognitive::engram::traits::CognitiveStorage;
use obrain_common::types::Value;

/// Le label des faits mémorisés. Un seul label : les faits d'un projet se distinguent par
/// leur propriété `projet`, pas par un label par projet (un label est un schéma, pas une
/// donnée).
pub const LABEL_FAIT: &str = ":Fait";
/// La propriété qui porte le texte du fait.
pub const PROP_TEXTE: &str = "texte";
/// La propriété qui porte le projet auquel le fait se rattache.
pub const PROP_PROJET: &str = "projet";

/// Ce que l'application fournit pour que la mémoire sache qu'une écriture a échoué.
///
/// Le trait `CognitiveStorage` ne peut pas le dire : ses créations rendent un identifiant,
/// jamais une erreur. L'adaptateur natif de l'application tient donc le dernier échec et le
/// rend ici. Un adaptateur qui ne tient RIEN doit le dire en rendant `None` — jamais en
/// prétendant que tout va bien.
pub trait SanteEcriture {
    /// Ouvre une opération : l'adaptateur réserve désormais une place d'erreur pour CETTE
    /// opération, et cesse d'attribuer les échecs au dernier appel venu.
    ///
    /// Appelé par le service AVANT la mutation. Une implémentation qui ne fait rien ici
    /// (« drapeau global ») ne satisfait PAS ce contrat : sous concurrence, deux opérations
    /// simultanées partageraient la même erreur, et l'une des deux serait confirmée à tort
    /// ou refusée à tort. C'est ce que `deux_operations_concurrentes_ne_partagent_pas_leur_erreur`
    /// mesure.
    fn ouvre(&self, operation: Operation);

    /// L'échec de CETTE opération, s'il y en a un — jamais « le dernier échec en date ».
    fn erreur_de(&self, operation: Operation) -> Option<String>;
}

/// L'identité d'une opération du service, unique par appel.
///
/// Elle n'est PAS un identifiant logique de reprise : c'est une clé de corrélation locale qui
/// permet d'attribuer une erreur à l'opération qui l'a produite. L'identité de reprise
/// (T3/OBRR) est une autre affaire, et ce service ne la fabrique pas.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Operation(pub u64);

/// Une santé qui ne sait rien : `None` toujours. Réservé aux tests et aux adaptations qui
/// n'ont réellement aucun canal d'erreur — elle n'est PAS un « tout va bien » implicite :
/// c'est la relecture qui fait le travail.
pub struct SansSante;

impl SanteEcriture for SansSante {
    fn ouvre(&self, _operation: Operation) {}
    fn erreur_de(&self, _operation: Operation) -> Option<String> {
        None
    }
}

/// Le verdict d'une écriture. Trois états, jamais deux.
///
/// **Ce que chaque verdict PROUVE, et ce qu'il ne prouve pas** — c'est la distinction que le
/// donneur d'ordre a exigée, et elle change le nom :
///
/// - [`Resultat::Ecrit`] : l'écriture a été **relue** dans le store. Cela prouve la
///   **VISIBILITÉ**. Cela ne prouve NI la durabilité (le store peut ne pas avoir fait son
///   `fsync`), NI l'engagement au sens OBRR (aucune référence native, aucune réconciliation).
///   Le nom est `Ecrit` et pas « enregistré » pour cette raison : une confirmation présentée
///   comme durable doit venir du contrat natif, pas d'une relecture.
/// - [`Resultat::AConfirmer`] : ni visible, ni perdu. L'indétermination est CONSERVÉE.
/// - [`Resultat::Refuse`] : rien n'a été tenté, ou la cible n'existe pas.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Resultat {
    /// Écrit ET relu — **visibilité**, pas durabilité (voir le doc du type).
    Ecrit {
        /// L'identifiant du nœud, tel que le store l'a rendu.
        id: u64,
    },
    /// Ni confirmé ni perdu : une erreur hors bande, ou une relecture qui ne trouve rien.
    AConfirmer {
        /// Ce qu'on sait, en clair, sans citer de contenu.
        raison: String,
    },
    /// Refus avant écriture : entrée vide, ou store en échec déclaré.
    Refuse {
        /// Le motif, en clair.
        raison: String,
    },
}

impl Resultat {
    /// Vrai seulement pour [`Resultat::Ecrit`]. Un identifiant ne suffit jamais.
    ///
    /// Le nom dit « visible », pas « durable » : pour annoncer une durabilité, il faut le
    /// verdict natif (T3 / OBRR), qui n'est pas rendu par ce service.
    pub fn visible(&self) -> bool {
        matches!(self, Resultat::Ecrit { .. })
    }
}

/// Un fait rappelé, avec le score qui a servi à le classer.
#[derive(Clone, Debug, PartialEq)]
pub struct Rappel {
    /// L'identifiant du fait.
    pub id: u64,
    /// Le texte du fait, tel qu'il est en base (donc la version COURANTE après correction).
    pub texte: String,
    /// Le recouvrement lexical avec la question, entre 0 et 1.
    pub score: f64,
}

/// Le service. Il ne possède pas le store : il l'emprunte, partagé.
pub struct Memoire {
    stockage: Arc<dyn CognitiveStorage>,
    sante: Arc<dyn SanteEcriture>,
    prochaine: std::sync::atomic::AtomicU64,
}

impl Memoire {
    /// Un service sur un store déjà ouvert par l'application.
    pub fn new(stockage: Arc<dyn CognitiveStorage>, sante: Arc<dyn SanteEcriture>) -> Self {
        Self {
            stockage,
            sante,
            prochaine: std::sync::atomic::AtomicU64::new(1),
        }
    }

    fn operation(&self) -> Operation {
        Operation(
            self.prochaine
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed),
        )
    }

    /// Enregistre un fait fourni par la personne, et ne le DIT que s'il a été relu.
    pub fn memoriser(&self, projet: &str, texte: &str) -> Resultat {
        let texte = texte.trim();
        let projet = projet.trim();
        if texte.is_empty() {
            return Resultat::Refuse {
                raison: "refusé : un fait vide n'est pas un fait".to_string(),
            };
        }
        if projet.is_empty() {
            return Resultat::Refuse {
                raison: "refusé : un fait sans projet ne se range nulle part".to_string(),
            };
        }

        let mut proprietes: HashMap<String, Value> = HashMap::new();
        proprietes.insert(PROP_TEXTE.to_string(), Value::from(texte));
        proprietes.insert(PROP_PROJET.to_string(), Value::from(projet));

        let operation = self.operation();
        self.sante.ouvre(operation);
        let id = self.stockage.create_node(LABEL_FAIT, &proprietes).0;

        // 1. L'erreur de CETTE opération : elle a priorité sur l'identifiant, toujours.
        if let Some(erreur) = self.sante.erreur_de(operation) {
            return Resultat::AConfirmer {
                raison: format!("écriture non confirmée par le store : {erreur}"),
            };
        }
        // 2. La relecture : un identifiant n'est pas une preuve. On vérifie le TEXTE et le
        //    PROJET relus, pas seulement la présence d'un nœud à cet identifiant — un store
        //    qui rend un id sans écrire, ou qui écrit dans un autre projet, doit tomber ici.
        match self.relire(id) {
            Some(fait) if fait.texte == texte && fait.projet == projet => {}
            Some(_) => {
                return Resultat::AConfirmer {
                    raison: "le fait relu ne correspond pas à ce qui a été demandé".to_string(),
                };
            }
            None => {
                return Resultat::AConfirmer {
                    raison: "le fait écrit n'a pas été relu dans le store".to_string(),
                };
            }
        }
        Resultat::Ecrit { id }
    }

    /// Applique une correction au fait `id`. La relecture doit montrer le NOUVEAU texte.
    pub fn reviser(&self, projet: &str, id: u64, texte: &str) -> Resultat {
        let texte = texte.trim();
        let projet = projet.trim();
        if texte.is_empty() {
            return Resultat::Refuse {
                raison: "refusé : une correction vide n'est pas une correction".to_string(),
            };
        }
        if projet.is_empty() {
            return Resultat::Refuse {
                raison: "refusé : une correction sans projet n'a pas de périmètre".to_string(),
            };
        }
        // Préserver le projet enregistré ne prouve pas que l'APPELANT y a droit : c'est ici
        // que le périmètre est vérifié, pas seulement au rappel.
        let Some(fait) = self.relire(id) else {
            return Resultat::Refuse {
                raison: format!("refusé : le fait {id} n'existe pas dans le store"),
            };
        };
        if fait.projet != projet {
            return Resultat::Refuse {
                raison: format!("refusé : le fait {id} n'appartient pas au projet « {projet} »"),
            };
        }

        let operation = self.operation();
        self.sante.ouvre(operation);
        let mut proprietes: HashMap<String, Value> = HashMap::new();
        proprietes.insert(PROP_TEXTE.to_string(), Value::from(texte));
        self.stockage
            .update_node(obrain_common::types::NodeId(id), &proprietes);

        if let Some(erreur) = self.sante.erreur_de(operation) {
            return Resultat::AConfirmer {
                raison: format!("révision non confirmée par le store : {erreur}"),
            };
        }
        match self.relire(id) {
            Some(fait) if fait.texte == texte && fait.projet == projet => Resultat::Ecrit { id },
            Some(_) => Resultat::AConfirmer {
                raison: "la relecture du fait ne montre pas la correction".to_string(),
            },
            None => Resultat::AConfirmer {
                raison: "le fait corrigé n'a pas été relu".to_string(),
            },
        }
    }

    /// Rappelle les faits du projet `projet` qui recouvrent la question, les mieux classés
    /// d'abord. Un projet vide ne rappelle rien (pas de périmètre implicite).
    ///
    /// **Ce que fait la résolution, et ce qu'elle ne fait pas** : elle est LEXICALE (recouvrement
    /// de mots, mots vides ignorés). Elle ne prétend pas être une recherche sémantique ; elle
    /// est le chemin minimal qui va d'une QUESTION à des INDICES — `recall(&[NodeId], k)` prend
    /// des identifiants, pas du texte, et quelqu'un doit faire ce pont. Le jour où un index
    /// vectoriel est disponible, c'est cette fonction qui change, pas ses appelants.
    pub fn rappeler(&self, projet: &str, question: &str, k: usize) -> Vec<Rappel> {
        let mots = mots_de(question);
        let projet = projet.trim();
        // Un projet vide ne rappelle RIEN : ce n'est pas « tous les projets ». Un périmètre
        // implicite est un élargissement silencieux — exactement ce que ce service refuse.
        if mots.is_empty() || k == 0 || projet.is_empty() {
            return Vec::new();
        }
        let mut rappels: Vec<Rappel> = self
            .faits()
            .into_iter()
            .filter(|(_, _, fait_projet)| fait_projet == projet)
            .filter_map(|(id, texte, _)| {
                let score = recouvrement(&mots, &mots_de(&texte));
                if score > 0.0 {
                    Some(Rappel { id, texte, score })
                } else {
                    None
                }
            })
            .collect();
        rappels.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then(a.id.cmp(&b.id))
        });
        rappels.truncate(k);
        rappels
    }

    /// Tous les faits du store, `(id, texte, projet)`. Lecture brute : la sélection est ailleurs.
    pub fn faits(&self) -> Vec<(u64, String, String)> {
        self.stockage
            .query_nodes(LABEL_FAIT, None)
            .into_iter()
            .filter_map(|noeud| {
                let texte = noeud
                    .properties
                    .get(PROP_TEXTE)
                    .and_then(|v| v.as_str())
                    .map(str::to_string)?;
                let projet = noeud
                    .properties
                    .get(PROP_PROJET)
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_string();
                Some((noeud.id.0, texte, projet))
            })
            .collect()
    }

    fn relire(&self, id: u64) -> Option<Fait> {
        // `query_nodes` ne filtre pas par id : on lit le label et on cherche l'id. Ce qui
        // compte est que la lecture passe par le STORE, pas par notre mémoire du dernier appel.
        self.stockage
            .query_nodes(LABEL_FAIT, None)
            .into_iter()
            .find(|noeud| noeud.id.0 == id)
            .map(|noeud| Fait {
                texte: noeud
                    .properties
                    .get(PROP_TEXTE)
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_string(),
                projet: noeud
                    .properties
                    .get(PROP_PROJET)
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_string(),
            })
    }
}

/// Ce qu'on relit d'un fait : son texte courant. Rien d'autre n'est nécessaire pour décider.
struct Fait {
    texte: String,
    projet: String,
}

/// Les mots d'un texte : minuscules, alphanumériques, longueur ≥ 3, sans doublon.
fn mots_de(texte: &str) -> std::collections::BTreeSet<String> {
    texte
        .to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|mot| mot.chars().count() >= 3)
        .map(str::to_string)
        .collect()
}

/// Le recouvrement : part des mots de la question présents dans le fait.
fn recouvrement(
    question: &std::collections::BTreeSet<String>,
    fait: &std::collections::BTreeSet<String>,
) -> f64 {
    if question.is_empty() {
        return 0.0;
    }
    let communs = question.intersection(fait).count();
    communs as f64 / question.len() as f64
}

#[cfg(test)]
mod tests {
    use super::*;
    use obrain_cognitive::engram::traits::{CognitiveEdge, CognitiveFilter, CognitiveNode};
    use obrain_common::types::{EdgeId, NodeId};
    use std::collections::HashMap;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

    /// Un store en mémoire qui peut TOMBER : c'est le seul moyen de prouver que le service
    /// ne confirme pas quand l'écriture échoue.
    struct StoreFaux {
        prochain: AtomicU64,
        nœuds: Mutex<Vec<CognitiveNode>>,
        panne: AtomicBool,
        falsifie: AtomicBool,
    }

    impl StoreFaux {
        fn neuf() -> Self {
            Self {
                prochain: AtomicU64::new(1),
                nœuds: Mutex::new(Vec::new()),
                panne: AtomicBool::new(false),
                falsifie: AtomicBool::new(false),
            }
        }
        fn en_panne(&self, panne: bool) {
            self.panne.store(panne, Ordering::Relaxed);
        }
        fn falsifie(&self, falsifie: bool) {
            self.falsifie.store(falsifie, Ordering::Relaxed);
        }
    }

    impl CognitiveStorage for StoreFaux {
        fn create_node(&self, label: &str, properties: &HashMap<String, Value>) -> NodeId {
            let id = NodeId(self.prochain.fetch_add(1, Ordering::Relaxed));
            if self.panne.load(Ordering::Relaxed) {
                // La panne : RIEN n'est écrit, et le trait ne peut pas le dire.
                return id;
            }
            let mut ecrites = properties.clone();
            if self.falsifie.load(Ordering::Relaxed) {
                ecrites.insert(PROP_TEXTE.to_string(), Value::from("autre chose"));
            }
            self.nœuds.lock().unwrap().push(CognitiveNode {
                id,
                label: label.to_string(),
                properties: ecrites,
            });
            id
        }
        fn create_edge(&self, _: NodeId, _: NodeId, _: &str, _: &HashMap<String, Value>) -> EdgeId {
            EdgeId(0)
        }
        fn query_nodes(&self, label: &str, _: Option<&CognitiveFilter>) -> Vec<CognitiveNode> {
            self.nœuds
                .lock()
                .unwrap()
                .iter()
                .filter(|n| n.label == label)
                .cloned()
                .collect()
        }
        fn update_node(&self, id: NodeId, properties: &HashMap<String, Value>) {
            if self.panne.load(Ordering::Relaxed) {
                return;
            }
            for noeud in self.nœuds.lock().unwrap().iter_mut() {
                if noeud.id == id {
                    for (cle, valeur) in properties {
                        noeud.properties.insert(cle.clone(), valeur.clone());
                    }
                }
            }
        }
        fn delete_node(&self, _: NodeId) {}
        fn delete_edge(&self, _: EdgeId) {}
        fn query_edges(&self, _: NodeId, _: &str) -> Vec<CognitiveEdge> {
            Vec::new()
        }
    }

    /// Une santé qui réserve une place d'erreur PAR OPÉRATION — ce que doit faire l'adaptateur
    /// natif. Un drapeau global ne satisferait pas ce contrat (voir le test de concurrence).
    #[derive(Default)]
    struct SanteParOperation {
        panne: AtomicBool,
        ouverts: Mutex<Vec<Operation>>,
        erreurs: Mutex<HashMap<u64, String>>,
    }

    impl SanteParOperation {
        fn en_panne(&self, panne: bool) {
            self.panne.store(panne, Ordering::Relaxed);
        }
    }

    impl SanteEcriture for SanteParOperation {
        fn ouvre(&self, operation: Operation) {
            self.ouverts.lock().unwrap().push(operation);
            if self.panne.load(Ordering::Relaxed) {
                self.erreurs
                    .lock()
                    .unwrap()
                    .insert(operation.0, "disque plein".to_string());
            }
        }
        fn erreur_de(&self, operation: Operation) -> Option<String> {
            self.erreurs.lock().unwrap().get(&operation.0).cloned()
        }
    }

    fn service_avec_sante() -> (Arc<StoreFaux>, Arc<SanteParOperation>, Memoire) {
        let store = Arc::new(StoreFaux::neuf());
        let sante = Arc::new(SanteParOperation::default());
        let stockage: Arc<dyn CognitiveStorage> = store.clone();
        let memoire = Memoire::new(stockage, sante.clone());
        (store, sante, memoire)
    }

    fn service() -> (Arc<StoreFaux>, Memoire) {
        let (store, _sante, memoire) = service_avec_sante();
        (store, memoire)
    }

    #[test]
    fn memoriser_ecrit_relit_et_confirme() {
        let (_, memoire) = service();
        let verdict = memoire.memoriser("this-rs", "le déploiement passe par Caddy");
        assert!(verdict.visible(), "{verdict:?}");
        assert_eq!(memoire.faits().len(), 1);
        assert_eq!(memoire.faits()[0].1, "le déploiement passe par Caddy");
        assert_eq!(
            memoire.faits()[0].2,
            "this-rs",
            "le projet est attaché au fait"
        );
    }

    #[test]
    fn une_ecriture_ratee_n_est_jamais_annoncee_comme_enregistree() {
        let (store, sante, memoire) = service_avec_sante();
        store.en_panne(true);
        sante.en_panne(true);
        let verdict = memoire.memoriser("this-rs", "un fait qui ne sera pas écrit");
        assert!(
            !verdict.visible(),
            "un identifiant est revenu, mais rien n'est écrit"
        );
        match verdict {
            Resultat::AConfirmer { raison } => {
                assert!(raison.contains("non confirmée"), "{raison}")
            }
            autre => panic!("attendu AConfirmer, mesuré {autre:?}"),
        }
        store.en_panne(false);
        sante.en_panne(false);
        assert!(memoire.faits().is_empty(), "la panne n'a rien écrit");
    }

    #[test]
    fn une_relecture_vide_suffit_a_refuser_de_confirmer_meme_sans_erreur_hors_bande() {
        // La santé se tait, mais le store n'écrit pas : seule la relecture peut le voir.
        let (store, memoire) = service();
        store.en_panne(true);
        let memoire_muette = Memoire::new(store.clone(), Arc::new(SansSante));
        let verdict = memoire_muette.memoriser("this-rs", "fait fantôme");
        assert!(!verdict.visible(), "{verdict:?}");
        assert!(memoire.faits().is_empty());
    }

    #[test]
    fn reviser_change_la_version_courante_et_le_rappel_sert_la_corrigee() {
        let (_, memoire) = service();
        let Resultat::Ecrit { id } = memoire.memoriser("this-rs", "le port est 8080") else {
            panic!("l'enregistrement doit réussir");
        };
        let verdict = memoire.reviser("this-rs", id, "le port est 9090");
        assert!(verdict.visible(), "{verdict:?}");

        let rappels = memoire.rappeler("this-rs", "quel est le port ?", 3);
        assert_eq!(rappels.len(), 1);
        assert_eq!(
            rappels[0].texte, "le port est 9090",
            "le rappel sert la version corrigée"
        );
    }

    #[test]
    fn reviser_un_fait_absent_est_refuse_et_ne_cree_rien() {
        let (_, memoire) = service();
        let verdict = memoire.reviser("this-rs", 4242, "un texte");
        assert!(matches!(verdict, Resultat::Refuse { .. }), "{verdict:?}");
        assert!(memoire.faits().is_empty());
    }

    #[test]
    fn une_question_sans_rapport_ne_rappelle_rien() {
        let (_, memoire) = service();
        memoire.memoriser("this-rs", "le déploiement passe par Caddy");
        assert!(
            memoire.rappeler("this-rs", "météo demain", 5).is_empty(),
            "aucun rapport, aucun rappel"
        );
    }

    #[test]
    fn plusieurs_souvenirs_contribuent_et_sont_classees() {
        let (_, memoire) = service();
        memoire.memoriser("this-rs", "le déploiement passe par Caddy sur le port 443");
        memoire.memoriser("this-rs", "le déploiement est signé par Developer ID");
        memoire.memoriser("autre", "la recette de la tarte tatin");
        let rappels = memoire.rappeler("this-rs", "comment se passe le déploiement ?", 5);
        assert_eq!(rappels.len(), 2, "{rappels:?}");
        assert!(rappels.iter().all(|r| r.texte.contains("déploiement")));
        assert!(
            rappels[0].score >= rappels[1].score,
            "le classement est décroissant"
        );
    }

    #[test]
    fn deux_projets_avec_des_faits_proches_ne_fuient_pas() {
        // Le périmètre était ABSENT du rappel : la signature ne le montrait pas, et deux
        // projets décrivant la même chose se seraient mélangés. C'est le test qui le dit.
        let (_, memoire) = service();
        memoire.memoriser("this-rs", "le déploiement passe par Caddy");
        memoire.memoriser("atelier", "le déploiement passe par Caddy et par la forge");

        let cote_this = memoire.rappeler("this-rs", "le déploiement passe par quoi ?", 5);
        let cote_atelier = memoire.rappeler("atelier", "le déploiement passe par quoi ?", 5);

        assert_eq!(cote_this.len(), 1, "{cote_this:?}");
        assert_eq!(cote_atelier.len(), 1, "{cote_atelier:?}");
        assert!(cote_this[0].texte.contains("Caddy") && !cote_this[0].texte.contains("forge"));
        assert!(cote_atelier[0].texte.contains("forge"));
    }

    #[test]
    fn un_projet_vide_ne_rappelle_rien_pas_tous_les_projets() {
        let (_, memoire) = service();
        memoire.memoriser("this-rs", "le déploiement passe par Caddy");
        assert!(
            memoire.rappeler("", "le déploiement", 5).is_empty(),
            "un périmètre vide ne doit pas devenir « tous les projets »"
        );
    }

    #[test]
    fn une_correction_ne_change_pas_le_projet_du_fait() {
        let (_, memoire) = service();
        let Resultat::Ecrit { id } = memoire.memoriser("this-rs", "le port est 8080") else {
            panic!("l'écriture doit réussir");
        };
        assert!(memoire.reviser("this-rs", id, "le port est 9090").visible());
        let faits = memoire.faits();
        assert_eq!(faits[0].2, "this-rs", "le fait reste dans son projet");
        assert_eq!(faits[0].1, "le port est 9090");
    }

    #[test]
    fn deux_operations_ne_partagent_pas_leur_erreur() {
        // Une erreur attribuée à l'opération N ne doit pas contaminer l'opération N+1 :
        // sinon `SanteEcriture` est un drapeau global déguisé, et une écriture saine serait
        // refusée à tort (ou l'inverse) dès qu'un appel échoue quelque part.
        let (store, sante, memoire) = service_avec_sante();
        store.en_panne(true);
        sante.en_panne(true);
        let premiere = memoire.memoriser("this-rs", "un fait sous panne");
        assert!(!premiere.visible(), "{premiere:?}");

        store.en_panne(false);
        sante.en_panne(false);
        let seconde = memoire.memoriser("this-rs", "un fait sain");
        assert!(
            seconde.visible(),
            "l'erreur de l'opération précédente ne doit pas coller : {seconde:?}"
        );
    }

    #[test]
    fn une_correction_visant_un_autre_projet_est_refusee() {
        // Préserver le projet enregistré ne prouve pas que l'APPELANT y a droit.
        let (_, memoire) = service();
        let Resultat::Ecrit { id } = memoire.memoriser("this-rs", "le port est 8080") else {
            panic!("l'écriture doit réussir");
        };
        let verdict = memoire.reviser("atelier", id, "le port est 1");
        assert!(matches!(verdict, Resultat::Refuse { .. }), "{verdict:?}");
        assert_eq!(
            memoire.faits()[0].1,
            "le port est 8080",
            "rien n'a été corrigé"
        );
    }

    #[test]
    fn une_relecture_qui_ne_correspond_pas_au_texte_n_est_pas_confirmee() {
        // Un nœud relu au bon identifiant ne suffit pas : si le texte écrit n'est pas celui
        // demandé, l'écriture n'a pas fait ce qu'on croyait.
        let (store, memoire) = service();
        store.falsifie(true);
        let verdict = memoire.memoriser("this-rs", "le fait que j'ai demandé");
        assert!(!verdict.visible(), "{verdict:?}");
        match verdict {
            Resultat::AConfirmer { raison } => {
                assert!(raison.contains("ne correspond pas"), "{raison}")
            }
            autre => panic!("attendu AConfirmer, mesuré {autre:?}"),
        }
    }

    #[test]
    fn un_fait_vide_ou_sans_projet_est_refuse_avant_toute_ecriture() {
        let (store, memoire) = service();
        assert!(matches!(
            memoire.memoriser("this-rs", "   "),
            Resultat::Refuse { .. }
        ));
        assert!(matches!(
            memoire.memoriser("  ", "un fait"),
            Resultat::Refuse { .. }
        ));
        assert!(store.nœuds.lock().unwrap().is_empty(), "rien n'a été écrit");
    }
}
