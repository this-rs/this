//! Un CRDT minimal pour un espace partagé : un **ensemble d'articles à case**, celui
//! d'une liste de courses.
//!
//! Le choix est celui de l'ADR du chantier D, et il est petit exprès : un journal
//! d'opérations **idempotentes**, un LWW par propriété, « l'ajout gagne », et des
//! pierres tombales. Pas d'automerge ni de `yrs` : mesurés à ~290 Ko et ~227 Ko par
//! document pour un ensemble d'articles, ils apportent une édition collaborative riche
//! dont une liste de courses n'a aucun usage.
//!
//! # Pourquoi ça converge, et pourquoi ça se démontre
//!
//! Chaque article est réduit à trois champs, chacun gardé par un **maximum** :
//!
//! | Champ | Règle |
//! |---|---|
//! | `ajoute` | l'estampille la plus grande gagne — une ré-ajout plus tardif ressuscite |
//! | `texte` | LWW : la plus grande estampille gagne |
//! | `coche` | LWW : la plus grande estampille gagne |
//! | `retire` | la plus grande estampille gagne (pierre tombale, jamais effacée) |
//!
//! Un article est **vivant** quand son ajout est postérieur à son retrait. Fusionner
//! deux états, c'est prendre le maximum champ par champ. Or `max` est commutatif,
//! associatif et idempotent : deux appareils qui ont vu les mêmes opérations — dans
//! n'importe quel ordre, avec des doublons, ou en deux temps — arrivent au **même**
//! état. Ce ne sont pas les opérations qui convergent mais l'état, ce qui rend le
//! transport indifférent : un état perdu se redemande, un état rejoué ne change rien.
//!
//! # Ce que ce n'est pas
//!
//! - Pas un texte collaboratif : deux personnes n'écrivent pas le même article.
//! - Pas un journal d'opérations à réconciliation : les opérations n'ont pas d'identifiant
//!   propre parce qu'elles n'en ont pas besoin — rejouer est sans effet.
//! - Pas une politique de droits : *qui* peut retirer est décidé par l'appelant (le
//!   maillage), pas ici. Ce module ne connaît ni clé ni pair.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// Une estampille : une horloge logique, et l'appareil qui l'a émise.
///
/// L'appareil est dans la comparaison, et ce n'est pas un détail : deux appareils hors
/// ligne produisent la même horloge à partir de zéro. Sans le départage par appareil,
/// deux écritures concurrentes se départageraient au hasard de l'ordre d'arrivée — donc
/// différemment des deux côtés. Avec lui, l'ordre est **total** et identique partout.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Estampille {
    /// L'horloge logique de l'appareil émetteur (jamais l'heure du mur).
    pub horloge: u64,
    /// L'identifiant de l'appareil émetteur : le départage, quand les horloges sont égales.
    pub appareil: String,
}

impl Estampille {
    pub fn nouvelle(horloge: u64, appareil: impl Into<String>) -> Self {
        Self {
            horloge,
            appareil: appareil.into(),
        }
    }
}

/// Une opération sur la liste. Rejouable sans effet : c'est ce qui rend le transport
/// indifférent (un message perdu se renvoie, un message dupliqué ne change rien).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Operation {
    /// Ajouter un article, ou ré-ajouter un article retiré (si l'estampille est postérieure).
    Ajouter {
        id: String,
        texte: String,
        est: Estampille,
    },
    /// Cocher ou décocher.
    Cocher {
        id: String,
        coche: bool,
        est: Estampille,
    },
    /// Retirer (pierre tombale : l'article ne revient pas sans un ajout plus tardif).
    Retirer { id: String, est: Estampille },
}

/// L'état d'un article. Tous les champs sont des « derniers écrivains » — sauf la
/// présence, qui est `ajoute > retire`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct Article {
    /// `None` : on connaît l'article par une tombe ou un cochage, sans avoir vu son ajout.
    /// Ce n'est pas un cas tordu : c'est le cas NORMAL d'un retrait reçu avant l'ajout.
    #[serde(default)]
    ajoute: Option<Estampille>,
    #[serde(default)]
    texte: Option<(Estampille, String)>,
    #[serde(default)]
    coche: Option<(Estampille, bool)>,
    #[serde(default)]
    retire: Option<Estampille>,
}

impl Article {
    /// Vivant : un ajout postérieur au retrait. Un ajout **rejoué** garde son estampille
    /// d'origine, donc il ne ressuscite pas un article retiré depuis.
    fn vivant(&self) -> bool {
        match (&self.ajoute, &self.retire) {
            (None, _) => false,
            (Some(_), None) => true,
            (Some(ajoute), Some(retire)) => ajoute > retire,
        }
    }

    /// Fusion champ par champ, par maximum. C'est la seule règle de ce module : tout le
    /// reste en découle.
    fn fusionner(&mut self, autre: &Article) {
        self.ajoute = max_option(self.ajoute.take(), autre.ajoute.clone());
        self.texte = match (self.texte.take(), &autre.texte) {
            (Some(a), Some(b)) => Some(if b.0 > a.0 { b.clone() } else { a }),
            (Some(a), None) => Some(a),
            (None, Some(b)) => Some(b.clone()),
            (None, None) => None,
        };
        self.coche = match (self.coche.take(), &autre.coche) {
            (Some(a), Some(b)) => Some(if b.0 > a.0 { b.clone() } else { a }),
            (Some(a), None) => Some(a),
            (None, Some(b)) => Some(b.clone()),
            (None, None) => None,
        };
        self.retire = match (self.retire.take(), &autre.retire) {
            (Some(a), Some(b)) => Some(a.max(b.clone())),
            (Some(a), None) => Some(a),
            (None, Some(b)) => Some(b.clone()),
            (None, None) => None,
        };
    }
}

/// Le maximum de deux estampilles dont l'une peut manquer.
fn max_option(a: Option<Estampille>, b: Option<Estampille>) -> Option<Estampille> {
    match (a, b) {
        (Some(a), Some(b)) => Some(a.max(b)),
        (Some(a), None) => Some(a),
        (None, Some(b)) => Some(b),
        (None, None) => None,
    }
}

/// Un article tel qu'il se lit : ce que l'écran affiche.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Ligne {
    pub id: String,
    pub texte: String,
    pub coche: bool,
}

/// La liste partagée. `Default` : vide.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Liste {
    articles: BTreeMap<String, Article>,
}

impl Liste {
    pub fn nouvelle() -> Self {
        Self::default()
    }

    /// Applique une opération. Idempotente : l'appliquer deux fois donne le même état.
    /// Rend `true` si l'état a changé — utile pour ne pas persister un rejeu.
    pub fn appliquer(&mut self, op: &Operation) -> bool {
        let avant = self.clone();
        match op {
            Operation::Ajouter { id, texte, est } => {
                // L'entrée peut déjà exister par sa tombe ou son cochage : on la complète.
                let entree = self.articles.entry(id.clone()).or_insert_with(|| Article {
                    ajoute: None,
                    texte: None,
                    coche: None,
                    retire: None,
                });
                if entree.ajoute.as_ref().is_none_or(|a| est > a) {
                    entree.ajoute = Some(est.clone());
                }
                if entree.texte.as_ref().is_none_or(|(a, _)| est > a) {
                    entree.texte = Some((est.clone(), texte.clone()));
                }
            }
            Operation::Cocher { id, coche, est } => {
                // Une case cochée sur un article que CE replica n'a pas encore vu doit
                // survivre : l'article existe ailleurs, et la fusion le réunira. Ne pas
                // l'enregistrer, c'est perdre l'écriture au premier échange.
                let entree = self.articles.entry(id.clone()).or_insert_with(|| Article {
                    ajoute: None,
                    texte: None,
                    coche: None,
                    retire: None,
                });
                if entree
                    .coche
                    .as_ref()
                    .is_none_or(|(actuelle, _)| est > actuelle)
                {
                    entree.coche = Some((est.clone(), *coche));
                }
            }
            Operation::Retirer { id, est } => {
                // Idem, et c'est le cas qui compte le plus : une tombe reçue avant l'ajout
                // doit RETENIR l'article, sinon la fusion le ferait revivre.
                let entree = self.articles.entry(id.clone()).or_insert_with(|| Article {
                    ajoute: None,
                    texte: None,
                    coche: None,
                    retire: None,
                });
                if entree.retire.as_ref().is_none_or(|actuelle| est > actuelle) {
                    entree.retire = Some(est.clone());
                }
            }
        }
        *self != avant
    }

    /// Fusionne un état reçu. Commutative, associative, idempotente — c'est la propriété
    /// qui fait converger deux appareils quel que soit l'ordre d'arrivée.
    pub fn fusionner(&mut self, autre: &Liste) -> bool {
        let avant = self.clone();
        for (id, article) in &autre.articles {
            match self.articles.get_mut(id) {
                Some(mien) => mien.fusionner(article),
                None => {
                    self.articles.insert(id.clone(), article.clone());
                }
            }
        }
        *self != avant
    }

    /// Les articles vivants, triés par identifiant (donc dans le même ordre partout).
    pub fn lignes(&self) -> Vec<Ligne> {
        self.articles
            .iter()
            .filter(|(_, a)| a.vivant())
            .map(|(id, a)| Ligne {
                id: id.clone(),
                texte: a
                    .texte
                    .as_ref()
                    .map(|(_, texte)| texte.clone())
                    .unwrap_or_default(),
                coche: a.coche.as_ref().is_some_and(|(_, c)| *c),
            })
            .collect()
    }

    /// Combien d'articles vivants.
    pub fn len(&self) -> usize {
        self.lignes().len()
    }

    pub fn is_empty(&self) -> bool {
        self.articles.values().all(|a| !a.vivant())
    }

    /// La case d'un article vivant, ou `None` s'il n'existe pas (ou est retiré).
    pub fn est_coche(&self, id: &str) -> Option<bool> {
        self.articles
            .get(id)
            .filter(|a| a.vivant())
            .map(|a| a.coche.as_ref().is_some_and(|(_, c)| *c))
    }

    /// Un identifiant **libre** pour un nouvel article : `prefixe-<n>`, où `n` dépasse
    /// tout ce que cette liste connaît déjà sous ce préfixe. Deux appareils hors ligne
    /// produiront le même `n` — leurs articles porteront donc le même identifiant, et la
    /// fusion les réunira (même case, même texte s'ils sont identiques). C'est voulu :
    /// « deux fois du pain » est un article, pas deux.
    pub fn id_libre(&self, prefixe: &str) -> String {
        let debut = format!("{prefixe}-");
        let n = self
            .articles
            .keys()
            .filter_map(|id| id.strip_prefix(&debut)?.parse::<u64>().ok())
            .max()
            .unwrap_or(0);
        format!("{debut}{}", n + 1)
    }

    /// L'état complet, pour le transport : un texte JSON stable (clés triées).
    pub fn etat_json(&self) -> String {
        serde_json::to_string(self).expect("une liste est sérialisable")
    }

    /// Relit un état reçu. Un état illisible est refusé : il vient d'un pair.
    pub fn depuis_etat(texte: &str) -> Option<Self> {
        serde_json::from_str(texte).ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: &str = "appareil-a";
    const B: &str = "appareil-b";

    fn ajouter(id: &str, texte: &str, horloge: u64, appareil: &str) -> Operation {
        Operation::Ajouter {
            id: id.into(),
            texte: texte.into(),
            est: Estampille::nouvelle(horloge, appareil),
        }
    }

    fn cocher(id: &str, coche: bool, horloge: u64, appareil: &str) -> Operation {
        Operation::Cocher {
            id: id.into(),
            coche,
            est: Estampille::nouvelle(horloge, appareil),
        }
    }

    fn retirer(id: &str, horloge: u64, appareil: &str) -> Operation {
        Operation::Retirer {
            id: id.into(),
            est: Estampille::nouvelle(horloge, appareil),
        }
    }

    fn textes(liste: &Liste) -> Vec<String> {
        liste.lignes().into_iter().map(|l| l.texte).collect()
    }

    #[test]
    fn deux_appareils_hors_ligne_fusionnent_leurs_ajouts() {
        let mut a = Liste::nouvelle();
        let mut b = Liste::nouvelle();
        a.appliquer(&ajouter("pain-1", "pain", 1, A));
        a.appliquer(&ajouter("lait-1", "lait", 2, A));
        b.appliquer(&ajouter("pain-1", "pain", 1, B));
        b.appliquer(&ajouter("cafe-1", "café", 3, B));

        let mut reunion = a.clone();
        reunion.fusionner(&b);
        let mut autre_ordre = b.clone();
        autre_ordre.fusionner(&a);

        assert_eq!(
            reunion, autre_ordre,
            "l'ordre de fusion ne doit rien changer"
        );
        // Triés par identifiant : `cafe-1`, `lait-1`, `pain-1`.
        assert_eq!(textes(&reunion), ["café", "lait", "pain"]);
        assert_eq!(reunion.len(), 3);
    }

    #[test]
    fn un_cochage_concurrent_converge_des_deux_cotes() {
        let mut a = Liste::nouvelle();
        let mut b = Liste::nouvelle();
        a.appliquer(&ajouter("pain-1", "pain", 1, A));
        b.fusionner(&a);

        // Concurrents, même horloge : c'est l'appareil qui départage, pas l'ordre d'arrivée.
        a.appliquer(&cocher("pain-1", true, 5, A));
        b.appliquer(&cocher("pain-1", false, 5, B));

        let mut reunion = a.clone();
        reunion.fusionner(&b);
        let mut autre = b.clone();
        autre.fusionner(&a);
        assert_eq!(reunion, autre);
        // `appareil-b` > `appareil-a` : son cochage l'emporte, partout.
        assert_eq!(reunion.est_coche("pain-1"), Some(false));
    }

    #[test]
    fn une_suppression_ne_ressuscite_pas_apres_fusion() {
        let mut a = Liste::nouvelle();
        a.appliquer(&ajouter("pain-1", "pain", 1, A));
        a.appliquer(&retirer("pain-1", 2, A));

        // `b` ne connaît que l'ajout : il fusionne sans faire revenir l'article.
        let mut b = Liste::nouvelle();
        b.appliquer(&ajouter("pain-1", "pain", 1, A));
        let mut reunion = b.clone();
        reunion.fusionner(&a);

        assert!(reunion.lignes().is_empty(), "la pierre tombale doit gagner");
        assert_eq!(
            reunion.est_coche("pain-1"),
            None,
            "un retiré n'a plus de case"
        );
    }

    /// Le cas qui a mordu : la tombe arrive AVANT l'ajout qu'elle annule.
    ///
    /// Un replica qui reçoit `retirer` sans avoir jamais vu `ajouter` ne peut pas jeter
    /// l'opération « parce que l'article n'existe pas » : l'article existe ailleurs, et la
    /// fusion le ferait revivre. La tombe est donc RETENUE, seule.
    #[test]
    fn une_tombe_recue_avant_l_ajout_annule_quand_meme() {
        let mut avec_seulement_la_tombe = Liste::nouvelle();
        avec_seulement_la_tombe.appliquer(&retirer("pain-1", 2, A));
        assert!(avec_seulement_la_tombe.lignes().is_empty());

        let mut avec_l_article = Liste::nouvelle();
        avec_l_article.appliquer(&ajouter("pain-1", "pain", 1, A));

        let mut reunion = avec_l_article.clone();
        reunion.fusionner(&avec_seulement_la_tombe);
        assert!(
            reunion.lignes().is_empty(),
            "la tombe doit annuler l'ajout plus ancien"
        );

        // L'ordre inverse donne le même résultat.
        let mut autre = avec_seulement_la_tombe.clone();
        autre.fusionner(&avec_l_article);
        assert_eq!(reunion, autre);
    }

    #[test]
    fn un_ajout_plus_tardif_ressuscite_un_article_retire() {
        let mut liste = Liste::nouvelle();
        liste.appliquer(&ajouter("pain-1", "pain", 1, A));
        liste.appliquer(&retirer("pain-1", 2, A));
        assert!(liste.is_empty());
        // Ré-ajout après le retrait : c'est un ajout, il gagne.
        liste.appliquer(&ajouter("pain-1", "pain de mie", 3, A));
        assert_eq!(liste.len(), 1);
        assert_eq!(liste.lignes()[0].texte, "pain de mie");
    }

    #[test]
    fn rejouer_une_operation_ne_change_rien() {
        let mut liste = Liste::nouvelle();
        let op = ajouter("pain-1", "pain", 1, A);
        assert!(liste.appliquer(&op), "le premier passage change l'état");
        assert!(!liste.appliquer(&op), "un rejeu ne change rien");
        let apres = liste.clone();
        assert!(!liste.appliquer(&op));
        assert_eq!(liste, apres);

        let coche = cocher("pain-1", true, 2, A);
        assert!(liste.appliquer(&coche));
        assert!(
            !liste.appliquer(&coche),
            "un rejeu de cochage ne change rien"
        );
    }

    #[test]
    fn un_cochage_plus_ancien_n_ecrase_pas_un_plus_recent() {
        let mut liste = Liste::nouvelle();
        liste.appliquer(&ajouter("pain-1", "pain", 1, A));
        liste.appliquer(&cocher("pain-1", true, 9, A));
        liste.appliquer(&cocher("pain-1", false, 4, B)); // arrive en retard
        assert_eq!(liste.est_coche("pain-1"), Some(true));
    }

    #[test]
    fn une_case_sur_un_article_inconnu_ne_le_fait_pas_apparaitre() {
        let mut liste = Liste::nouvelle();
        liste.appliquer(&cocher("fantome-1", true, 3, A));
        assert!(liste.lignes().is_empty(), "sans ajout, rien ne s'affiche");
        // Mais l'écriture est RETENUE : le jour où l'ajout arrive, elle est là.
        liste.appliquer(&ajouter("fantome-1", "fantôme", 1, A));
        assert_eq!(liste.est_coche("fantome-1"), Some(true));
    }

    #[test]
    fn la_fusion_est_associative_sur_trois_etats() {
        let mut a = Liste::nouvelle();
        a.appliquer(&ajouter("pain-1", "pain", 1, A));
        let mut b = Liste::nouvelle();
        b.appliquer(&ajouter("cafe-1", "café", 2, B));
        b.appliquer(&retirer("pain-1", 4, B));
        let mut c = Liste::nouvelle();
        c.appliquer(&ajouter("pain-1", "pain", 1, A));
        c.appliquer(&cocher("cafe-1", true, 7, A));

        // (a∪b)∪c, (a∪c)∪b, a∪(b∪c) : même résultat.
        let mut g1 = a.clone();
        g1.fusionner(&b);
        g1.fusionner(&c);
        let mut g2 = a.clone();
        g2.fusionner(&c);
        g2.fusionner(&b);
        let mut bc = b.clone();
        bc.fusionner(&c);
        let mut g3 = a.clone();
        g3.fusionner(&bc);

        assert_eq!(g1, g2);
        assert_eq!(g1, g3);
        assert_eq!(textes(&g1), vec!["café".to_string()]);
        assert_eq!(g1.est_coche("cafe-1"), Some(true));
    }

    #[test]
    fn l_etat_fait_l_aller_retour_par_json() {
        let mut liste = Liste::nouvelle();
        liste.appliquer(&ajouter("pain-1", "pain", 1, A));
        liste.appliquer(&cocher("pain-1", true, 2, A));
        let texte = liste.etat_json();
        let relu = Liste::depuis_etat(&texte).expect("l'état se relit");
        assert_eq!(relu, liste);
        assert_eq!(
            relu.etat_json(),
            texte,
            "même état, même texte (clés triées)"
        );
    }

    #[test]
    fn un_etat_illisible_est_refuse() {
        assert!(Liste::depuis_etat("{pas du json").is_none());
    }

    #[test]
    fn l_identifiant_libre_depasse_ce_que_la_liste_connait() {
        let mut liste = Liste::nouvelle();
        assert_eq!(liste.id_libre("pain"), "pain-1");
        liste.appliquer(&ajouter("pain-1", "pain", 1, A));
        assert_eq!(liste.id_libre("pain"), "pain-2");
        // Un identifiant d'un autre préfixe ne compte pas.
        liste.appliquer(&ajouter("cafe-9", "café", 1, A));
        assert_eq!(liste.id_libre("pain"), "pain-2");
    }

    #[test]
    fn le_meme_identifiant_ecrit_deux_fois_est_un_seul_article() {
        // Deux appareils hors ligne créent « pain-1 » chacun : la fusion les réunit, et
        // c'est le texte du plus tardif qui reste — pas deux lignes identiques.
        let mut a = Liste::nouvelle();
        a.appliquer(&ajouter("pain-1", "pain", 1, A));
        let mut b = Liste::nouvelle();
        b.appliquer(&ajouter("pain-1", "pain", 1, B));
        let mut reunion = a.clone();
        reunion.fusionner(&b);
        assert_eq!(reunion.len(), 1);
        assert_eq!(reunion.lignes()[0].texte, "pain");
    }
}
