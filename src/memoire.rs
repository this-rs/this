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
    /// Le dernier échec d'écriture non résolu, s'il y en a un.
    fn derniere_erreur(&self) -> Option<String>;
}

/// Une santé qui ne sait rien : `None` toujours. Réservé aux tests et aux adaptations qui
/// n'ont réellement aucun canal d'erreur — elle n'est PAS un « tout va bien » implicite :
/// c'est la relecture qui fait le travail.
pub struct SansSante;

impl SanteEcriture for SansSante {
    fn derniere_erreur(&self) -> Option<String> {
        None
    }
}

/// Le verdict d'une écriture. Trois états, jamais deux.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Resultat {
    /// Écrit ET relu : on peut le dire.
    Enregistre {
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
    /// Vrai seulement pour `Enregistre`. Une confirmation ne se déduit pas d'un identifiant.
    pub fn confirme(&self) -> bool {
        matches!(self, Resultat::Enregistre { .. })
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
}

impl Memoire {
    /// Un service sur un store déjà ouvert par l'application.
    pub fn new(stockage: Arc<dyn CognitiveStorage>, sante: Arc<dyn SanteEcriture>) -> Self {
        Self { stockage, sante }
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

        let id = self.stockage.create_node(LABEL_FAIT, &proprietes).0;

        // 1. L'erreur hors bande : elle a priorité sur l'identifiant, toujours.
        if let Some(erreur) = self.sante.derniere_erreur() {
            return Resultat::AConfirmer {
                raison: format!("écriture non confirmée par le store : {erreur}"),
            };
        }
        // 2. La relecture : un identifiant n'est pas une preuve, un fait relu en est une.
        if self.relire(id).is_none() {
            return Resultat::AConfirmer {
                raison: "le fait écrit n'a pas été relu dans le store".to_string(),
            };
        }
        Resultat::Enregistre { id }
    }

    /// Applique une correction au fait `id`. La relecture doit montrer le NOUVEAU texte.
    pub fn reviser(&self, id: u64, texte: &str) -> Resultat {
        let texte = texte.trim();
        if texte.is_empty() {
            return Resultat::Refuse {
                raison: "refusé : une correction vide n'est pas une correction".to_string(),
            };
        }
        if self.relire(id).is_none() {
            return Resultat::Refuse {
                raison: format!("refusé : le fait {id} n'existe pas dans le store"),
            };
        }

        let mut proprietes: HashMap<String, Value> = HashMap::new();
        proprietes.insert(PROP_TEXTE.to_string(), Value::from(texte));
        self.stockage
            .update_node(obrain_common::types::NodeId(id), &proprietes);

        if let Some(erreur) = self.sante.derniere_erreur() {
            return Resultat::AConfirmer {
                raison: format!("révision non confirmée par le store : {erreur}"),
            };
        }
        match self.relire(id) {
            Some(fait) if fait.texte == texte => Resultat::Enregistre { id },
            Some(_) => Resultat::AConfirmer {
                raison: "la relecture du fait ne montre pas la correction".to_string(),
            },
            None => Resultat::AConfirmer {
                raison: "le fait corrigé n'a pas été relu".to_string(),
            },
        }
    }

    /// Rappelle les faits qui recouvrent la question, les mieux classés d'abord.
    ///
    /// **Ce que fait la résolution, et ce qu'elle ne fait pas** : elle est LEXICALE (recouvrement
    /// de mots, mots vides ignorés). Elle ne prétend pas être une recherche sémantique ; elle
    /// est le chemin minimal qui va d'une QUESTION à des INDICES — `recall(&[NodeId], k)` prend
    /// des identifiants, pas du texte, et quelqu'un doit faire ce pont. Le jour où un index
    /// vectoriel est disponible, c'est cette fonction qui change, pas ses appelants.
    pub fn rappeler(&self, question: &str, k: usize) -> Vec<Rappel> {
        let mots = mots_de(question);
        if mots.is_empty() || k == 0 {
            return Vec::new();
        }
        let mut rappels: Vec<Rappel> = self
            .faits()
            .into_iter()
            .filter_map(|(id, texte)| {
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

    /// Tous les faits du store, `(id, texte)`. Lecture brute : la sélection est ailleurs.
    pub fn faits(&self) -> Vec<(u64, String)> {
        self.stockage
            .query_nodes(LABEL_FAIT, None)
            .into_iter()
            .filter_map(|noeud| {
                let texte = noeud
                    .properties
                    .get(PROP_TEXTE)
                    .and_then(|v| v.as_str())
                    .map(str::to_string)?;
                Some((noeud.id.0, texte))
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
            })
    }
}

/// Ce qu'on relit d'un fait : son texte courant. Rien d'autre n'est nécessaire pour décider.
struct Fait {
    texte: String,
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
    }

    impl StoreFaux {
        fn neuf() -> Self {
            Self {
                prochain: AtomicU64::new(1),
                nœuds: Mutex::new(Vec::new()),
                panne: AtomicBool::new(false),
            }
        }
        fn en_panne(&self, panne: bool) {
            self.panne.store(panne, Ordering::Relaxed);
        }
    }

    impl CognitiveStorage for StoreFaux {
        fn create_node(&self, label: &str, properties: &HashMap<String, Value>) -> NodeId {
            let id = NodeId(self.prochain.fetch_add(1, Ordering::Relaxed));
            if self.panne.load(Ordering::Relaxed) {
                // La panne : RIEN n'est écrit, et le trait ne peut pas le dire.
                return id;
            }
            self.nœuds.lock().unwrap().push(CognitiveNode {
                id,
                label: label.to_string(),
                properties: properties.clone(),
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

    /// Une santé qui relaie la panne du store — ce que ferait l'adaptateur natif.
    struct SanteDuFaux(Arc<StoreFaux>);

    impl SanteEcriture for SanteDuFaux {
        fn derniere_erreur(&self) -> Option<String> {
            if self.0.panne.load(Ordering::Relaxed) {
                Some("disque plein".to_string())
            } else {
                None
            }
        }
    }

    fn service() -> (Arc<StoreFaux>, Memoire) {
        let store = Arc::new(StoreFaux::neuf());
        let sante: Arc<dyn SanteEcriture> = Arc::new(SanteDuFaux(store.clone()));
        let stockage: Arc<dyn CognitiveStorage> = store.clone();
        let memoire = Memoire::new(stockage, sante);
        (store, memoire)
    }

    #[test]
    fn memoriser_ecrit_relit_et_confirme() {
        let (_, memoire) = service();
        let verdict = memoire.memoriser("this-rs", "le déploiement passe par Caddy");
        assert!(verdict.confirme(), "{verdict:?}");
        assert_eq!(memoire.faits().len(), 1);
        assert_eq!(memoire.faits()[0].1, "le déploiement passe par Caddy");
    }

    #[test]
    fn une_ecriture_ratee_n_est_jamais_annoncee_comme_enregistree() {
        let (store, memoire) = service();
        store.en_panne(true);
        let verdict = memoire.memoriser("this-rs", "un fait qui ne sera pas écrit");
        assert!(
            !verdict.confirme(),
            "un identifiant est revenu, mais rien n'est écrit"
        );
        match verdict {
            Resultat::AConfirmer { raison } => {
                assert!(raison.contains("non confirmée"), "{raison}")
            }
            autre => panic!("attendu AConfirmer, mesuré {autre:?}"),
        }
        store.en_panne(false);
        assert!(memoire.faits().is_empty(), "la panne n'a rien écrit");
    }

    #[test]
    fn une_relecture_vide_suffit_a_refuser_de_confirmer_meme_sans_erreur_hors_bande() {
        // La santé se tait, mais le store n'écrit pas : seule la relecture peut le voir.
        let (store, memoire) = service();
        store.en_panne(true);
        let memoire_muette = Memoire::new(store.clone(), Arc::new(SansSante));
        let verdict = memoire_muette.memoriser("this-rs", "fait fantôme");
        assert!(!verdict.confirme(), "{verdict:?}");
        assert!(memoire.faits().is_empty());
    }

    #[test]
    fn reviser_change_la_version_courante_et_le_rappel_sert_la_corrigee() {
        let (_, memoire) = service();
        let Resultat::Enregistre { id } = memoire.memoriser("this-rs", "le port est 8080") else {
            panic!("l'enregistrement doit réussir");
        };
        let verdict = memoire.reviser(id, "le port est 9090");
        assert!(verdict.confirme(), "{verdict:?}");

        let rappels = memoire.rappeler("quel est le port ?", 3);
        assert_eq!(rappels.len(), 1);
        assert_eq!(
            rappels[0].texte, "le port est 9090",
            "le rappel sert la version corrigée"
        );
    }

    #[test]
    fn reviser_un_fait_absent_est_refuse_et_ne_cree_rien() {
        let (_, memoire) = service();
        let verdict = memoire.reviser(4242, "un texte");
        assert!(matches!(verdict, Resultat::Refuse { .. }), "{verdict:?}");
        assert!(memoire.faits().is_empty());
    }

    #[test]
    fn une_question_sans_rapport_ne_rappelle_rien() {
        let (_, memoire) = service();
        memoire.memoriser("this-rs", "le déploiement passe par Caddy");
        assert!(
            memoire.rappeler("météo demain", 5).is_empty(),
            "aucun rapport, aucun rappel"
        );
    }

    #[test]
    fn plusieurs_souvenirs_contribuent_et_sont_classees() {
        let (_, memoire) = service();
        memoire.memoriser("this-rs", "le déploiement passe par Caddy sur le port 443");
        memoire.memoriser("this-rs", "le déploiement est signé par Developer ID");
        memoire.memoriser("autre", "la recette de la tarte tatin");
        let rappels = memoire.rappeler("comment se passe le déploiement ?", 5);
        assert_eq!(rappels.len(), 2, "{rappels:?}");
        assert!(rappels.iter().all(|r| r.texte.contains("déploiement")));
        assert!(
            rappels[0].score >= rappels[1].score,
            "le classement est décroissant"
        );
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
