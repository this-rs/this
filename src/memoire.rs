//! Le service MÉMOIRE du consommateur : mémoriser un fait, le corriger, le rappeler.
//!
//! ## Ce que ce module est, et ce qu'il n'est pas
//!
//! Il ne réimplémente AUCUN mécanisme cognitif : la formation, la révision et le rappel
//! restent dans Obrain. Ce module est le **service** qui appelle la MUTATION NATIVE du fait
//! (les clés `texte`/`projet`) par les primitives conditionnelles du substrat, et qui porte
//! les contrôles de périmètre.
//!
//! ## Deux couches distinctes, à ne pas confondre
//!
//! - le **fait durable** : un nœud `:Fait` dont `texte` et `projet` sont publiés par
//!   `capturer_precondition` / `preparer_transformation` (Strict) /
//!   `publier_si_precondition_intacte` / `resoudre_reference` — c'est ce module ;
//! - l'**engramme** : une trace formée par co-activation, avec son énergie — c'est
//!   `EngramManager`, et il se branchera par-dessus, pas à la place.
//!
//! ## Le discriminant de la référence
//!
//! La `ReferenceTentative` rendue par `creer`/`corriger` couvre les clés EFFECTIVEMENT
//! écrites (`texte`, `projet`), pas un `obs:*` d'un registre cognitif séparé. Une
//! publication `obs:*` réussie qui laisse `texte` inchangé n'est donc JAMAIS une
//! confirmation de la mutation du fait : la confirmation passe par la relecture de
//! `texte`/`projet`, et l'intégrité par `integrite()` de la résolution.

use std::sync::Arc;

use obrain_common::types::{NodeId, Value};
use obrain_core::graph::traits::{GraphStore, GraphStoreMut};
use obrain_substrate::{
    EtatPropriete, IntegriteTentative, IssuePublication, ModePublication, ReferenceTentative,
    ResolutionTentative, SubstrateStore,
};

/// Le label des faits mémorisés.
pub const LABEL_FAIT: &str = ":Fait";
/// La propriété qui porte le texte du fait.
pub const PROP_TEXTE: &str = "texte";
/// La propriété qui porte le projet auquel le fait se rattache.
pub const PROP_PROJET: &str = "projet";

/// Une mutation du fait publiée par le contrat natif : le nœud, la référence de SA
/// mutation (les clés écrites), et l'intégrité des effets.
#[derive(Debug, Clone)]
pub struct FaitPublie {
    /// L'identifiant du nœud.
    pub id: u64,
    /// La référence de cette mutation (clés `texte`/`projet`). `None` si l'adaptateur ne
    /// sait pas la produire (tests, adaptateurs non natifs).
    pub reference: Option<ReferenceTentative>,
    /// L'intégrité des effets de la publication.
    pub integrite: IntegriteTentative,
}

/// L'échec d'une mutation, distingué par ce qu'il permet de décider — et doté du contexte
/// nécessaire à une résolution.
#[derive(Debug, Clone)]
pub enum ErreurStockage {
    /// Refus AVANT mutation : fait absent, projet différent, entrée invalide. Rien n'a été
    /// écrit, et il n'y a pas d'indétermination.
    Refus(String),
    /// Indéterminé : erreur native, précondition en conflit, publication partielle. La
    /// résolution native décide d'une reprise.
    ///
    /// `id` et `reference` sont conservés dès qu'ils existent : un nœud créé (`id`) et une
    /// référence préparée (`reference`) ne sont PAS détruits par l'échec qui les suit —
    /// c'est ce qui permet au consommateur de retrouver la tentative exacte et de demander
    /// sa résolution, au lieu de republier aveuglément. Avant préparation, `reference` est
    /// `None` ; après, elle est conservée.
    AConfirmer {
        raison: String,
        /// L'identifiant du fait, si un nœud a été créé.
        id: Option<u64>,
        /// La référence de la mutation, si elle a été préparée.
        reference: Option<ReferenceTentative>,
    },
}

/// Le stockage des faits, que l'adaptateur natif implémente sur le store déjà ouvert.
///
/// Le trait ne porte PAS de canal d'erreur hors bande : chaque opération rend son propre
/// `Result`, et c'est ce `Result` — pas un drapeau global — qui attribue l'échec à
/// l'opération qui l'a produit, y compris sous concurrence.
pub trait StockageFait: Send + Sync {
    /// Crée un fait (`texte` + `projet`) et publie ses propriétés par le contrat natif.
    fn creer(&self, projet: &str, texte: &str) -> Result<FaitPublie, ErreurStockage>;
    /// Corrige le `texte` d'un fait, `projet` ÉPINGLÉ dans la précondition.
    fn corriger(
        &self,
        id: u64,
        projet_attendu: &str,
        texte: &str,
    ) -> Result<FaitPublie, ErreurStockage>;
    /// Relit un fait. `Ok(None)` = absent ; `Ok(Some)` = présent ; `Err` = erreur de lecture
    /// ou contenu non conforme. Une erreur de lecture ne doit JAMAIS être confondue avec
    /// « absent ».
    fn lire(&self, id: u64) -> Result<Option<(String, String)>, ErreurStockage>;
    /// Tous les faits du store : `(id, texte, projet)`. `Err` = lecture impossible ou
    /// incomplète — ne jamais rendre « aucun fait » sur une lecture en échec.
    fn lister(&self) -> Result<Vec<(u64, String, String)>, ErreurStockage>;
    /// Résout une référence conservée contre l'état courant du store.
    fn resoudre(
        &self,
        reference: &ReferenceTentative,
    ) -> Result<ResolutionTentative, ErreurStockage>;
}

/// L'adaptateur de production : les primitives conditionnelles du substrat, sur le
/// `SubstrateStore` déjà ouvert et partagé par l'application.
///
/// L'adaptateur n'ouvre PAS une seconde base et ne ferme PAS la base : la fermeture
/// appartient au propriétaire applicatif. Il ne fabrique pas de second registre
/// d'engagement : la référence est celle du substrat, pour cette mutation.
pub struct AdaptateurNatif {
    store: Arc<SubstrateStore>,
}

impl AdaptateurNatif {
    /// Un adaptateur sur un store déjà ouvert. L'appelant (l'application) le fournit par
    /// `ObrainStore::db().substrate_handle()`.
    pub fn nouveau(store: Arc<SubstrateStore>) -> Self {
        Self { store }
    }
}

impl StockageFait for AdaptateurNatif {
    fn creer(&self, projet: &str, texte: &str) -> Result<FaitPublie, ErreurStockage> {
        let cible = self.store.create_node(&[LABEL_FAIT]);
        let pre = self
            .store
            .capturer_precondition(cible, &[PROP_TEXTE, PROP_PROJET])
            .map_err(|e| ErreurStockage::AConfirmer {
                raison: format!("capture : {e}"),
                id: Some(cible.0),
                reference: None,
            })?;
        let prep = self
            .store
            .preparer_transformation(
                cible,
                &[
                    (PROP_TEXTE, Value::from(texte)),
                    (PROP_PROJET, Value::from(projet)),
                ],
                &pre,
                ModePublication::Strict,
            )
            .map_err(|e| ErreurStockage::AConfirmer {
                raison: format!("préparation : {e}"),
                id: Some(cible.0),
                reference: None,
            })?;
        // À partir d'ici, la référence existe : elle est conservée quel que soit l'échec.
        let reference = prep.reference_tentative();
        let integrite = match self
            .store
            .publier_si_precondition_intacte(prep)
            .map_err(|e| ErreurStockage::AConfirmer {
                raison: format!("publication : {e}"),
                id: Some(cible.0),
                reference: Some(reference.clone()),
            })? {
            IssuePublication::PublieIntegrale => IntegriteTentative::Integrale,
            IssuePublication::PubliePartielle => IntegriteTentative::Partielle,
            IssuePublication::ConflitPrecondition => {
                return Err(ErreurStockage::AConfirmer {
                    raison: "précondition en conflit : un autre écrivain a intercalé une écriture"
                        .to_string(),
                    id: Some(cible.0),
                    reference: Some(reference),
                });
            }
        };
        Ok(FaitPublie {
            id: cible.0,
            reference: Some(reference),
            integrite,
        })
    }

    fn corriger(
        &self,
        id: u64,
        projet_attendu: &str,
        texte: &str,
    ) -> Result<FaitPublie, ErreurStockage> {
        let cible = NodeId(id);
        let pre = self
            .store
            .capturer_precondition(cible, &[PROP_TEXTE, PROP_PROJET])
            .map_err(|e| ErreurStockage::AConfirmer {
                raison: format!("capture : {e}"),
                id: Some(id),
                reference: None,
            })?;
        // Le projet AUTORISÉ est comparé au projet CAPTURÉ par la précondition (même lecture).
        match pre.valeur_capturee(PROP_PROJET) {
            Some(v) if v.as_str() == Some(projet_attendu) => {}
            Some(_) => {
                return Err(ErreurStockage::Refus(format!(
                    "refusé : le fait {id} n'appartient pas au projet « {projet_attendu} »"
                )));
            }
            None => {
                return Err(ErreurStockage::Refus(format!(
                    "refusé : le fait {id} n'existe pas ou n'a pas de projet"
                )));
            }
        }
        let prep = self
            .store
            .preparer_transformation(
                cible,
                &[(PROP_TEXTE, Value::from(texte))],
                &pre,
                ModePublication::Strict,
            )
            .map_err(|e| ErreurStockage::AConfirmer {
                raison: format!("préparation : {e}"),
                id: Some(id),
                reference: None,
            })?;
        let reference = prep.reference_tentative();
        let integrite = match self
            .store
            .publier_si_precondition_intacte(prep)
            .map_err(|e| ErreurStockage::AConfirmer {
                raison: format!("publication : {e}"),
                id: Some(id),
                reference: Some(reference.clone()),
            })? {
            IssuePublication::PublieIntegrale => IntegriteTentative::Integrale,
            IssuePublication::PubliePartielle => IntegriteTentative::Partielle,
            IssuePublication::ConflitPrecondition => {
                return Err(ErreurStockage::AConfirmer {
                    raison: "précondition en conflit".to_string(),
                    id: Some(id),
                    reference: Some(reference),
                });
            }
        };
        Ok(FaitPublie {
            id,
            reference: Some(reference),
            integrite,
        })
    }

    fn lire(&self, id: u64) -> Result<Option<(String, String)>, ErreurStockage> {
        let cible = NodeId(id);
        let texte = match self
            .store
            .lire_etat_propriete(cible, PROP_TEXTE)
            .map_err(|e| ErreurStockage::AConfirmer {
                raison: format!("lecture de `texte` : {e}"),
                id: Some(id),
                reference: None,
            })? {
            EtatPropriete::Presente { valeur, .. } => match valeur.as_str() {
                Some(s) => s.to_string(),
                None => {
                    return Err(ErreurStockage::AConfirmer {
                        raison: format!("le fait {id} porte un `texte` non conforme"),
                        id: Some(id),
                        reference: None,
                    });
                }
            },
            EtatPropriete::RessourceInexistante
            | EtatPropriete::Absente
            | EtatPropriete::Supprimee(_) => return Ok(None),
        };
        let projet = match self
            .store
            .lire_etat_propriete(cible, PROP_PROJET)
            .map_err(|e| ErreurStockage::AConfirmer {
                raison: format!("lecture de `projet` : {e}"),
                id: Some(id),
                reference: None,
            })? {
            EtatPropriete::Presente { valeur, .. } => {
                valeur.as_str().unwrap_or_default().to_string()
            }
            _ => String::new(),
        };
        Ok(Some((texte, projet)))
    }

    fn lister(&self) -> Result<Vec<(u64, String, String)>, ErreurStockage> {
        self.store
            .nodes_by_label(LABEL_FAIT)
            .into_iter()
            .map(|id| match self.lire(id.0)? {
                Some((t, p)) => Ok((id.0, t, p)),
                None => Ok((id.0, String::new(), String::new())),
            })
            .collect()
    }

    fn resoudre(
        &self,
        reference: &ReferenceTentative,
    ) -> Result<ResolutionTentative, ErreurStockage> {
        self.store
            .resoudre_reference(reference)
            .map_err(|e| ErreurStockage::AConfirmer {
                raison: format!("résolution : {e}"),
                id: None,
                reference: None,
            })
    }
}

/// Le verdict d'une écriture. Ce que chaque état PROUVE :
///
/// - [`Resultat::Ecrit`] : publié avec effets **intégraux** ET relu (visibilité). La
///   référence de la mutation est rendue pour une éventuelle reprise. Cela ne prouve PAS
///   la durabilité (le `fsync` est l'affaire de `commit_wal`, pas de la publication).
/// - [`Resultat::AConfirmer`] : indéterminé (erreur native, publication partielle, ou
///   relecture non conforme). L'indétermination est conservée, AVEC l'identifiant et la
///   référence de la tentative quand ils existent.
/// - [`Resultat::Refuse`] : rien n'a été tenté (entrée vide, fait absent, autre projet).
#[derive(Debug, Clone)]
pub enum Resultat {
    Ecrit {
        /// L'identifiant du nœud.
        id: u64,
        /// La référence de la mutation, si l'adaptateur la produit.
        reference: Option<ReferenceTentative>,
    },
    AConfirmer {
        raison: String,
        /// L'identifiant du fait, si un nœud a été créé.
        id: Option<u64>,
        /// La référence de la mutation, si elle a été préparée.
        reference: Option<ReferenceTentative>,
    },
    Refuse {
        raison: String,
    },
}

impl Resultat {
    pub fn visible(&self) -> bool {
        matches!(self, Resultat::Ecrit { .. })
    }
}

/// Un fait rappelé, avec le score qui a servi à le classer.
#[derive(Clone, Debug, PartialEq)]
pub struct Rappel {
    pub id: u64,
    pub texte: String,
    pub score: f64,
}

/// Le service. Il ne possède pas le store : il l'emprunte, partagé.
pub struct Memoire {
    stockage: Arc<dyn StockageFait>,
}

impl Memoire {
    pub fn new(stockage: Arc<dyn StockageFait>) -> Self {
        Self { stockage }
    }

    fn a_confirmer(
        raison: impl Into<String>,
        id: Option<u64>,
        reference: Option<ReferenceTentative>,
    ) -> Resultat {
        Resultat::AConfirmer {
            raison: raison.into(),
            id,
            reference,
        }
    }

    /// Enregistre un fait fourni par la personne, et ne le DIT que s'il a été publié
    /// intégralement ET relu.
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

        let publie = match self.stockage.creer(projet, texte) {
            Ok(p) => p,
            Err(ErreurStockage::Refus(raison)) => return Resultat::Refuse { raison },
            Err(ErreurStockage::AConfirmer {
                raison,
                id,
                reference,
            }) => return Self::a_confirmer(raison, id, reference),
        };

        if publie.integrite != IntegriteTentative::Integrale {
            return Self::a_confirmer(
                "publication partielle : tous les effets ne sont pas engagés",
                Some(publie.id),
                publie.reference,
            );
        }
        match self.stockage.lire(publie.id) {
            Ok(Some((t, p))) if t == texte && p == projet => Resultat::Ecrit {
                id: publie.id,
                reference: publie.reference,
            },
            Ok(Some(_)) => Self::a_confirmer(
                "le fait relu ne correspond pas à ce qui a été demandé",
                Some(publie.id),
                publie.reference,
            ),
            Ok(None) => Self::a_confirmer(
                "le fait écrit n'a pas été relu dans le store",
                Some(publie.id),
                publie.reference,
            ),
            Err(ErreurStockage::AConfirmer { raison, .. }) => {
                Self::a_confirmer(raison, Some(publie.id), publie.reference)
            }
            Err(ErreurStockage::Refus(raison)) => Resultat::Refuse { raison },
        }
    }

    /// Applique une correction au fait `id`, dans le périmètre `projet`.
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

        let publie = match self.stockage.corriger(id, projet, texte) {
            Ok(p) => p,
            Err(ErreurStockage::Refus(raison)) => return Resultat::Refuse { raison },
            Err(ErreurStockage::AConfirmer {
                raison,
                id: fid,
                reference,
            }) => return Self::a_confirmer(raison, fid, reference),
        };

        if publie.integrite != IntegriteTentative::Integrale {
            return Self::a_confirmer("publication partielle", Some(publie.id), publie.reference);
        }
        match self.stockage.lire(id) {
            Ok(Some((t, p))) if t == texte && p == projet => Resultat::Ecrit {
                id,
                reference: publie.reference,
            },
            Ok(Some(_)) => Self::a_confirmer(
                "la relecture du fait ne montre pas la correction",
                Some(id),
                publie.reference,
            ),
            Ok(None) => Self::a_confirmer(
                "le fait corrigé n'a pas été relu",
                Some(id),
                publie.reference,
            ),
            Err(ErreurStockage::AConfirmer { raison, .. }) => {
                Self::a_confirmer(raison, Some(id), publie.reference)
            }
            Err(ErreurStockage::Refus(raison)) => Resultat::Refuse { raison },
        }
    }

    /// Rappelle les faits du projet `projet` qui recouvrent la question, les mieux classés
    /// d'abord. Un projet vide ne rappelle rien (pas de périmètre implicite).
    ///
    /// `Err` = lecture impossible ou incomplète : un rappel ne doit jamais rendre « aucun
    /// souvenir » sur une lecture en échec.
    ///
    /// La résolution est LEXICALE (recouvrement de mots) : c'est le chemin minimal qui va
    /// d'une QUESTION à des INDICES — `recall(&[NodeId], k)` prend des identifiants, pas du
    /// texte. Le rappel COGNITIF natif se branche par-dessus, pas à la place.
    pub fn rappeler(
        &self,
        projet: &str,
        question: &str,
        k: usize,
    ) -> Result<Vec<Rappel>, ErreurStockage> {
        let mots = mots_de(question);
        let projet = projet.trim();
        if mots.is_empty() || k == 0 || projet.is_empty() {
            return Ok(Vec::new());
        }
        let mut rappels: Vec<Rappel> = self
            .stockage
            .lister()?
            .into_iter()
            .filter(|(_, _, p)| p == projet)
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
        Ok(rappels)
    }

    /// Tous les faits du store, `(id, texte, projet)`. `Err` = lecture impossible.
    pub fn faits(&self) -> Result<Vec<(u64, String, String)>, ErreurStockage> {
        self.stockage.lister()
    }

    /// Résout une référence conservée contre l'état courant du store. C'est le chemin de
    /// reprise après fermeture/réouverture : la référence rendue par `memoriser`/`reviser`
    /// est conservée par l'appelant, et résolue ici après réouverture.
    pub fn resoudre(
        &self,
        reference: &ReferenceTentative,
    ) -> Result<ResolutionTentative, ErreurStockage> {
        self.stockage.resoudre(reference)
    }
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
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

    struct StoreFaux {
        prochain: AtomicU64,
        faits: Mutex<Vec<(u64, String, String)>>,
        panne: AtomicBool,
        falsifie: AtomicBool,
        fantome: AtomicBool,
        lecture_en_panne: AtomicBool,
    }

    impl StoreFaux {
        fn neuf() -> Self {
            Self {
                prochain: AtomicU64::new(1),
                faits: Mutex::new(Vec::new()),
                panne: AtomicBool::new(false),
                falsifie: AtomicBool::new(false),
                fantome: AtomicBool::new(false),
                lecture_en_panne: AtomicBool::new(false),
            }
        }
        fn en_panne(&self, panne: bool) {
            self.panne.store(panne, Ordering::Relaxed);
        }
        fn falsifie(&self, falsifie: bool) {
            self.falsifie.store(falsifie, Ordering::Relaxed);
        }
        fn fantome(&self, fantome: bool) {
            self.fantome.store(fantome, Ordering::Relaxed);
        }
        fn lecture_en_panne(&self, panne: bool) {
            self.lecture_en_panne.store(panne, Ordering::Relaxed);
        }
    }

    impl StockageFait for StoreFaux {
        fn creer(&self, projet: &str, texte: &str) -> Result<FaitPublie, ErreurStockage> {
            let id = self.prochain.fetch_add(1, Ordering::Relaxed);
            if self.panne.load(Ordering::Relaxed) {
                return Err(ErreurStockage::AConfirmer {
                    raison: "disque plein".to_string(),
                    id: Some(id),
                    reference: None,
                });
            }
            if self.fantome.load(Ordering::Relaxed) {
                return Ok(FaitPublie {
                    id,
                    reference: None,
                    integrite: IntegriteTentative::Integrale,
                });
            }
            let ecrit = if self.falsifie.load(Ordering::Relaxed) {
                "autre chose".to_string()
            } else {
                texte.to_string()
            };
            self.faits
                .lock()
                .unwrap()
                .push((id, ecrit, projet.to_string()));
            Ok(FaitPublie {
                id,
                reference: None,
                integrite: IntegriteTentative::Integrale,
            })
        }

        fn corriger(
            &self,
            id: u64,
            projet_attendu: &str,
            texte: &str,
        ) -> Result<FaitPublie, ErreurStockage> {
            if self.panne.load(Ordering::Relaxed) {
                return Err(ErreurStockage::AConfirmer {
                    raison: "disque plein".to_string(),
                    id: Some(id),
                    reference: None,
                });
            }
            let mut faits = self.faits.lock().unwrap();
            let Some((_, _, projet_actuel)) = faits.iter().find(|(fid, _, _)| *fid == id) else {
                return Err(ErreurStockage::Refus(format!(
                    "refusé : le fait {id} n'existe pas ou n'a pas de projet"
                )));
            };
            if projet_actuel != projet_attendu {
                return Err(ErreurStockage::Refus(format!(
                    "refusé : le fait {id} n'appartient pas au projet « {projet_attendu} »"
                )));
            }
            for (fid, t, _) in faits.iter_mut() {
                if *fid == id {
                    *t = texte.to_string();
                }
            }
            Ok(FaitPublie {
                id,
                reference: None,
                integrite: IntegriteTentative::Integrale,
            })
        }

        fn lire(&self, id: u64) -> Result<Option<(String, String)>, ErreurStockage> {
            if self.lecture_en_panne.load(Ordering::Relaxed) {
                return Err(ErreurStockage::AConfirmer {
                    raison: "lecture impossible".to_string(),
                    id: Some(id),
                    reference: None,
                });
            }
            Ok(self
                .faits
                .lock()
                .unwrap()
                .iter()
                .find(|(fid, _, _)| *fid == id)
                .map(|(_, t, p)| (t.clone(), p.clone())))
        }

        fn lister(&self) -> Result<Vec<(u64, String, String)>, ErreurStockage> {
            if self.lecture_en_panne.load(Ordering::Relaxed) {
                return Err(ErreurStockage::AConfirmer {
                    raison: "lecture impossible".to_string(),
                    id: None,
                    reference: None,
                });
            }
            Ok(self.faits.lock().unwrap().clone())
        }

        fn resoudre(
            &self,
            _reference: &ReferenceTentative,
        ) -> Result<ResolutionTentative, ErreurStockage> {
            Err(ErreurStockage::AConfirmer {
                raison: "résolution non implémentée dans le faux store".to_string(),
                id: None,
                reference: None,
            })
        }
    }

    fn service() -> (Arc<StoreFaux>, Memoire) {
        let store = Arc::new(StoreFaux::neuf());
        let stockage: Arc<dyn StockageFait> = store.clone();
        let memoire = Memoire::new(stockage);
        (store, memoire)
    }

    #[test]
    fn memoriser_ecrit_relit_et_confirme() {
        let (_, memoire) = service();
        let verdict = memoire.memoriser("this-rs", "le déploiement passe par Caddy");
        assert!(verdict.visible(), "{verdict:?}");
        let faits = memoire.faits().unwrap();
        assert_eq!(faits.len(), 1);
        assert_eq!(faits[0].1, "le déploiement passe par Caddy");
        assert_eq!(faits[0].2, "this-rs", "le projet est attaché");
    }

    #[test]
    fn une_ecriture_ratee_n_est_jamais_annoncee_comme_enregistree() {
        let (store, memoire) = service();
        store.en_panne(true);
        let verdict = memoire.memoriser("this-rs", "un fait qui ne sera pas écrit");
        assert!(!verdict.visible(), "{verdict:?}");
        match verdict {
            Resultat::AConfirmer {
                raison,
                id,
                reference,
            } => {
                assert!(raison.contains("disque plein"), "{raison}");
                assert!(id.is_some(), "l'identifiant du nœud créé est conservé");
                assert!(reference.is_none(), "pas de référence avant préparation");
            }
            autre => panic!("attendu AConfirmer, mesuré {autre:?}"),
        }
        store.en_panne(false);
        assert!(
            memoire.faits().unwrap().is_empty(),
            "la panne n'a rien écrit"
        );
    }

    #[test]
    fn une_publication_obs_reussie_qui_laisse_le_texte_inchange_ne_confirme_pas() {
        let (store, memoire) = service();
        store.fantome(true);
        let verdict = memoire.memoriser("this-rs", "fait fantôme");
        assert!(!verdict.visible(), "{verdict:?}");
        assert!(memoire.faits().unwrap().is_empty(), "rien n'a été écrit");
    }

    #[test]
    fn une_relecture_qui_ne_correspond_pas_au_texte_n_est_pas_confirmee() {
        let (store, memoire) = service();
        store.falsifie(true);
        let verdict = memoire.memoriser("this-rs", "le fait que j'ai demandé");
        assert!(!verdict.visible(), "{verdict:?}");
        match verdict {
            Resultat::AConfirmer { raison, .. } => {
                assert!(raison.contains("ne correspond pas"), "{raison}")
            }
            autre => panic!("attendu AConfirmer, mesuré {autre:?}"),
        }
    }

    #[test]
    fn une_erreur_de_lecture_n_est_pas_un_resultat_aucun_souvenir() {
        // Une lecture en échec ne doit PAS produire « aucun souvenir » : le rappel signale
        // l'impossibilité de lire, au lieu de rendre silencieusement une liste vide.
        let (store, memoire) = service();
        store.lecture_en_panne(true);
        let resultat = memoire.rappeler("this-rs", "déploiement", 5);
        assert!(
            resultat.is_err(),
            "une lecture impossible doit être une erreur, mesuré {resultat:?}"
        );
    }

    #[test]
    fn reviser_change_la_version_courante_et_le_rappel_sert_la_corrigee() {
        let (_, memoire) = service();
        let Resultat::Ecrit { id, .. } = memoire.memoriser("this-rs", "le port est 8080") else {
            panic!("l'enregistrement doit réussir");
        };
        let verdict = memoire.reviser("this-rs", id, "le port est 9090");
        assert!(verdict.visible(), "{verdict:?}");

        let rappels = memoire
            .rappeler("this-rs", "quel est le port ?", 3)
            .unwrap();
        assert_eq!(rappels.len(), 1);
        assert_eq!(
            rappels[0].texte, "le port est 9090",
            "le rappel sert la correction"
        );
    }

    #[test]
    fn reviser_un_fait_absent_est_refuse_et_ne_cree_rien() {
        let (_, memoire) = service();
        let verdict = memoire.reviser("this-rs", 4242, "un texte");
        assert!(matches!(verdict, Resultat::Refuse { .. }), "{verdict:?}");
        assert!(memoire.faits().unwrap().is_empty());
    }

    #[test]
    fn une_question_sans_rapport_ne_rappelle_rien() {
        let (_, memoire) = service();
        memoire.memoriser("this-rs", "le déploiement passe par Caddy");
        assert!(
            memoire
                .rappeler("this-rs", "météo demain", 5)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn plusieurs_souvenirs_contribuent_et_sont_classees() {
        let (_, memoire) = service();
        memoire.memoriser("this-rs", "le déploiement passe par Caddy sur le port 443");
        memoire.memoriser("this-rs", "le déploiement est signé par Developer ID");
        memoire.memoriser("autre", "la recette de la tarte tatin");
        let rappels = memoire
            .rappeler("this-rs", "comment se passe le déploiement ?", 5)
            .unwrap();
        assert_eq!(rappels.len(), 2, "{rappels:?}");
        assert!(rappels.iter().all(|r| r.texte.contains("déploiement")));
        assert!(rappels[0].score >= rappels[1].score);
    }

    #[test]
    fn deux_projets_avec_des_faits_proches_ne_fuient_pas() {
        let (_, memoire) = service();
        memoire.memoriser("this-rs", "le déploiement passe par Caddy");
        memoire.memoriser("atelier", "le déploiement passe par Caddy et par la forge");

        let cote_this = memoire
            .rappeler("this-rs", "le déploiement passe par quoi ?", 5)
            .unwrap();
        let cote_atelier = memoire
            .rappeler("atelier", "le déploiement passe par quoi ?", 5)
            .unwrap();

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
            memoire
                .rappeler("", "le déploiement", 5)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn une_correction_ne_change_pas_le_projet_du_fait() {
        let (_, memoire) = service();
        let Resultat::Ecrit { id, .. } = memoire.memoriser("this-rs", "le port est 8080") else {
            panic!("l'écriture doit réussir");
        };
        assert!(memoire.reviser("this-rs", id, "le port est 9090").visible());
        let faits = memoire.faits().unwrap();
        assert_eq!(faits[0].2, "this-rs", "le fait reste dans son projet");
        assert_eq!(faits[0].1, "le port est 9090");
    }

    #[test]
    fn une_correction_visant_un_autre_projet_est_refusee() {
        let (_, memoire) = service();
        let Resultat::Ecrit { id, .. } = memoire.memoriser("this-rs", "le port est 8080") else {
            panic!("l'écriture doit réussir");
        };
        let verdict = memoire.reviser("atelier", id, "le port est 1");
        assert!(matches!(verdict, Resultat::Refuse { .. }), "{verdict:?}");
        assert_eq!(
            memoire.faits().unwrap()[0].1,
            "le port est 8080",
            "rien n'a été corrigé"
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
        assert!(store.faits.lock().unwrap().is_empty(), "rien n'a été écrit");
    }
}
