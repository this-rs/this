//! **Représentation cognitive d'un tour — sans dictionnaire de mots-clés.**
//!
//! Invariant de conception : *aucune liste de mots, aucune expression régulière,
//! aucune règle lexicale ne décide de ce qu'une demande veut dire*. Un lexique est
//! un codage de langue, de domaine et de formulation ; il exclut par construction
//! tout ce qui n'a pas été écrit dedans (autres langues, paraphrases sans mot commun,
//! multimodal, domaines inédits).
//!
//! Ce module ne fournit que des **structures** : une représentation extensible, un
//! port d'encodage optionnel, et une règle de décision qui opère sur des **vecteurs
//! et des prototypes appris** — jamais sur des chaînes de caractères.
//!
//! ```text
//! perception → représentation → contextualisation → rappel → composition
//!            → décision → exécution → observation → apprentissage
//! ```
//!
//! Ce fichier couvre `représentation` et la règle `rappel → décision`. Les autres
//! étages vivent là où ils existent déjà (rappel : `recall`, décision : `routing`,
//! observation/apprentissage : `Learning`).
//!
//! ## Amorçage (pas de dépendance à un fournisseur)
//!
//! Si aucun encodeur n'est configuré, ou si aucun prototype n'est mémorisé, la
//! classification rend [`Uncertainty::Unknown`] et la classe
//! [`CLASSE_INCONNUE`]. C'est un état **explicite** : le routeur continue alors avec
//! sa politique neutre (capacités, disponibilité, contraintes objectives). Il n'y a
//! **aucun retour silencieux** à un mécanisme lexical — il n'en existe plus.

/// Ce qu'un tour apporte comme modalités.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Modalities {
    pub text: bool,
    pub images: bool,
    pub documents: bool,
}

impl Modalities {
    /// Les modalités d'un tour de conversation.
    pub fn of_turn(has_text: bool, has_images: bool) -> Self {
        Self {
            text: has_text,
            images: has_images,
            documents: false,
        }
    }
}

/// Ce que le système sait de sa propre compréhension.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Uncertainty {
    /// La représentation est proche d'un prototype connu.
    Established(f32),
    /// Elle n'est proche d'aucun prototype : une classe a été formée.
    Formed(f32),
    /// Aucune représentation disponible : on ne sait pas.
    Unknown,
}

impl Uncertainty {
    /// Vrai quand la décision repose sur une représentation qui existe.
    pub fn grounded(self) -> bool {
        !matches!(self, Uncertainty::Unknown)
    }
}

/// Contraintes objectives du tour — aucune n'est sémantique.
#[derive(Clone, Copy, Debug, Default)]
pub struct Constraints {
    pub context_need_tokens: u64,
    pub budget_remaining_usd: Option<f64>,
    pub needs_tools: bool,
    pub needs_images: bool,
    pub attempt: u32,
}

/// La représentation d'une tâche.
///
/// `embedding` vient de la couche perception (un encodeur configuré) ; `goals` est
/// ce que l'appelant a **déclaré**, jamais ce qu'un lexique a deviné.
#[derive(Clone, Debug, Default)]
pub struct TaskRepr {
    pub embedding: Option<Vec<f32>>,
    pub modalities: Modalities,
    pub constraints: Constraints,
    pub goals: Vec<String>,
}

impl TaskRepr {
    /// Une représentation sans vecteur : le cas d'amorçage.
    pub fn sans_encodeur(modalities: Modalities, constraints: Constraints) -> Self {
        Self {
            embedding: None,
            modalities,
            constraints,
            goals: Vec::new(),
        }
    }
}

/// Port d'encodage sémantique.
///
/// Le harness **n'impose aucun fournisseur** : pas d'implémentation par défaut, pas
/// de repli lexical. Une implémentation peut appeler un modèle local, un service
/// distant, ou Obrain ; elle rend `None` quand elle ne peut pas encoder, et le
/// système le dit (`Uncertainty::Unknown`).
pub trait SemanticEncoder {
    /// Le vecteur du texte, ou `None` si l'encodage n'est pas disponible.
    fn encode(&self, text: &str) -> Option<Vec<f32>>;
}

/// Une classe **apprise**. Son nom est opaque : il ne décrit pas un domaine.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct LearnedClass(pub String);

impl LearnedClass {
    /// Classe de l'incertitude — la seule qui soit nommée dans le code.
    pub fn inconnue() -> Self {
        Self(CLASSE_INCONNUE.to_string())
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Nom de la classe rendue quand aucune représentation n'est disponible.
pub const CLASSE_INCONNUE: &str = "repr.unknown";

/// Un prototype mémorisé : un vecteur, et la classe qu'il désigne.
#[derive(Clone, Debug)]
pub struct Prototype {
    pub class: LearnedClass,
    pub vector: Vec<f32>,
}

/// Le résultat d'une classification, avec son incertitude explicite.
#[derive(Clone, Debug)]
pub struct Classification {
    pub class: LearnedClass,
    pub uncertainty: Uncertainty,
    /// Le prototype le plus proche, s'il y en a un.
    pub nearest: Option<LearnedClass>,
    pub similarity: Option<f32>,
}

impl Classification {
    /// La seule classification qui ne repose sur aucune représentation.
    pub fn inconnue() -> Self {
        Self {
            class: LearnedClass::inconnue(),
            uncertainty: Uncertainty::Unknown,
            nearest: None,
            similarity: None,
        }
    }
}

/// Un vecteur est une représentation **valide** s'il est non vide et ne porte
/// aucun `NaN` ni infini. Une représentation invalide n'est jamais une
/// connaissance apprise : ni une similarité, ni une identité.
pub fn vecteur_valide(v: &[f32]) -> bool {
    !v.is_empty() && v.iter().all(|x| x.is_finite())
}

/// Similarité cosinus, ou `None` quand les deux vecteurs ne sont pas
/// **comparables** (dimensions différentes, ou représentation invalide : vide,
/// `NaN`, infini).
///
/// `Some(0.0)` signifie une orthogonalité (ou un vecteur nul) entre deux
/// représentations valides de même dimension — jamais une incompatibilité.
/// L'accumulation se fait en `f64` pour ne pas écraser les grandes amplitudes ni
/// les valeurs sous-normales ; la similarité est ramenée en `f32` en sortie.
pub fn cosinus(a: &[f32], b: &[f32]) -> Option<f32> {
    if a.len() != b.len() || !vecteur_valide(a) || !vecteur_valide(b) {
        return None;
    }
    let (mut dot, mut na, mut nb) = (0.0f64, 0.0f64, 0.0f64);
    for (x, y) in a.iter().zip(b.iter()) {
        dot += f64::from(*x) * f64::from(*y);
        na += f64::from(*x) * f64::from(*x);
        nb += f64::from(*y) * f64::from(*y);
    }
    if na == 0.0 || nb == 0.0 {
        return Some(0.0);
    }
    Some((dot / (na.sqrt() * nb.sqrt())) as f32)
}

/// **Poignée** (handle) dérivée de la projection quantifiée — PAS une identité
/// logique. Deux structures distinctes peuvent produire la même projection : un
/// hachage ne prouve rien (Obrain l'a mesuré). Cette poignée n'est qu'une clé
/// d'accélération / de rapprochement, jamais une preuve d'identité de la
/// connaissance, et elle ne garantit aucune stabilité quand l'encodeur change.
///
/// `quantification = 0` n'offre aucun signal exploitable : la poignée est
/// `None`, donc **aucune** classe n'est formée — jamais une classe commune à
/// toutes les connaissances nouvelles (une fusion interdite).
pub fn poignee_de_projection(vector: &[f32], quantification: u32) -> Option<LearnedClass> {
    if quantification == 0 {
        return None;
    }
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for x in vector {
        let q = (x * quantification as f32).round() as i64;
        for b in q.to_le_bytes() {
            h ^= b as u64;
            h = h.wrapping_mul(0x0100_0000_01b3);
        }
    }
    Some(LearnedClass(format!("repr.{h:016x}")))
}

/// Classe la représentation contre les prototypes mémorisés.
///
/// * pas de vecteur, ou vecteur **invalide** (`NaN`, infini, vide) →
///   [`Uncertainty::Unknown`], classe [`CLASSE_INCONNUE`] — rien n'est appris ;
/// * vecteur proche d'un prototype (`similarité ≥ seuil`) → la classe du prototype ;
/// * vecteur sans prototype assez proche → **formation** d'une nouvelle classe, dont
///   le nom est une *poignée* de projection (pas une identité logique). Aucun code
///   métier n'est modifié : la structure naît de la représentation.
pub fn classer(
    repr: &TaskRepr,
    prototypes: &[Prototype],
    seuil: f32,
    quantification: u32,
) -> Classification {
    let Some(vector) = repr.embedding.as_ref() else {
        return Classification::inconnue();
    };
    // Une représentation invalide ne forme rien : l'incertitude est dite.
    if !vecteur_valide(vector) {
        return Classification::inconnue();
    }
    let mut meilleur: Option<(f32, &Prototype)> = None;
    for p in prototypes {
        // Un prototype invalide ou de dimension différente n'est pas comparable :
        // ignoré, jamais assimilé à une similarité nulle.
        let Some(s) = cosinus(vector, &p.vector) else {
            continue;
        };
        if meilleur.map(|(m, _)| s > m).unwrap_or(true) {
            meilleur = Some((s, p));
        }
    }
    match meilleur {
        Some((s, p)) if s >= seuil => Classification {
            class: p.class.clone(),
            uncertainty: Uncertainty::Established(s),
            nearest: Some(p.class.clone()),
            similarity: Some(s),
        },
        Some((s, p)) => {
            // Formation d'une nouvelle classe : la poignée est une *proposition* de
            // rapprochement, pas une identité. Sans poignée (quantification=0),
            // rien n'est formé : l'incertitude est dite, aucune fusion.
            let Some(classe) = poignee_de_projection(vector, quantification) else {
                return Classification::inconnue();
            };
            Classification {
                class: classe,
                uncertainty: Uncertainty::Formed(s),
                nearest: Some(p.class.clone()),
                similarity: Some(s),
            }
        }
        None => {
            let Some(classe) = poignee_de_projection(vector, quantification) else {
                return Classification::inconnue();
            };
            Classification {
                class: classe,
                uncertainty: Uncertainty::Formed(0.0),
                nearest: None,
                similarity: None,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repr(v: Vec<f32>) -> TaskRepr {
        TaskRepr {
            embedding: Some(v),
            ..Default::default()
        }
    }

    #[test]
    fn sans_vecteur_l_incertitude_est_explicite() {
        let r = TaskRepr::sans_encodeur(Modalities::of_turn(true, false), Constraints::default());
        let c = classer(&r, &[], 0.9, 1000);
        assert_eq!(c.uncertainty, Uncertainty::Unknown);
        assert_eq!(c.class.as_str(), CLASSE_INCONNUE);
        assert!(!c.uncertainty.grounded());
    }

    #[test]
    fn un_vecteur_proche_rejoint_le_prototype_meme_sans_mot_commun() {
        let p = Prototype {
            class: LearnedClass("repr.a1".into()),
            vector: vec![1.0, 0.0, 0.0],
        };
        // Représentation quasi identique : aucun mot n'entre dans la décision.
        let c = classer(&repr(vec![0.98, 0.19, 0.0]), &[p], 0.9, 1000);
        assert_eq!(c.class.as_str(), "repr.a1");
        assert!(matches!(c.uncertainty, Uncertainty::Established(_)));
    }

    #[test]
    fn un_vecteur_eloigne_forme_une_classe_nouvelle() {
        let p = Prototype {
            class: LearnedClass("repr.a1".into()),
            vector: vec![1.0, 0.0, 0.0],
        };
        let c = classer(&repr(vec![0.0, 1.0, 0.0]), &[p], 0.9, 1000);
        assert_ne!(c.class.as_str(), "repr.a1");
        assert!(c.class.as_str().starts_with("repr."));
        assert!(matches!(c.uncertainty, Uncertainty::Formed(_)));
    }

    #[test]
    fn la_poignee_vient_du_vecteur_pas_du_texte() {
        let a = poignee_de_projection(&[0.1, 0.2, 0.3], 1000);
        let b = poignee_de_projection(&[0.1, 0.2, 0.3], 1000);
        let c = poignee_de_projection(&[0.1, 0.2, 0.4], 1000);
        assert_eq!(a, b);
        assert_ne!(a, c);
        // La poignée est une proposition de rapprochement, jamais une identité :
        // elle reste un hachage, sujet aux collisions.
        assert!(a.unwrap().as_str().starts_with("repr."));
    }

    #[test]
    fn la_similarite_ne_deborde_pas_et_distingue_l_incomparable() {
        // Vecteur nul valide : similarité nulle ordinaire.
        assert_eq!(cosinus(&[0.0, 0.0], &[1.0, 0.0]), Some(0.0));
        // Dimensions différentes : incomparable, PAS une similarité nulle.
        assert_eq!(cosinus(&[1.0], &[1.0, 0.0]), None, "dimensions différentes");
        // Vide : incomparable.
        assert_eq!(cosinus(&[], &[1.0, 0.0]), None);
    }

    #[test]
    fn une_representation_invalide_n_est_jamais_apprise() {
        // NaN, infini, vide : la classification rend l'inconnu, elle ne forme rien.
        for invalide in [
            vec![f32::NAN, 0.0],
            vec![f32::INFINITY, 0.0],
            vec![f32::NEG_INFINITY, 0.0],
            vec![],
        ] {
            let c = classer(&repr(invalide), &[], 0.9, 1000);
            assert_eq!(
                c.uncertainty,
                Uncertainty::Unknown,
                "une représentation invalide est inconnue, pas formée"
            );
            assert_eq!(c.class.as_str(), CLASSE_INCONNUE);
        }
        // Un prototype invalide n'est pas comparable : il ne matche jamais.
        let p = Prototype {
            class: LearnedClass("repr.a1".into()),
            vector: vec![f32::NAN, 0.0],
        };
        let c = classer(&repr(vec![1.0, 0.0]), &[p], 0.9, 1000);
        assert!(matches!(c.uncertainty, Uncertainty::Formed(_)));
        assert_ne!(c.class.as_str(), "repr.a1");
    }

    #[test]
    fn la_similarite_survit_aux_grandes_amplitudes() {
        // f32 accumulerait un débordement ; f64 ne déborde pas ici.
        let a = [1e30f32, 1e30f32];
        let b = [1e30f32, 1e30f32];
        assert_eq!(cosinus(&a, &b), Some(1.0));
        // Orthogonalité nette à grande amplitude.
        let c = [1e30f32, -1e30f32];
        let s = cosinus(&a, &c).unwrap();
        assert!(s.abs() < 1e-3, "{s}");
    }

    #[test]
    fn quantification_zero_ne_forme_aucune_classe_commune() {
        // Pas de signal exploitable : aucune poignée, donc AUCUNE classe formée.
        // Jamais une classe commune qui fusionnerait des connaissances distinctes.
        assert_eq!(poignee_de_projection(&[0.1, 0.2], 0), None);
        assert_eq!(poignee_de_projection(&[9.9, 8.8], 0), None);
        // Et `classer` rend l'inconnu au lieu de former une classe commune.
        let c = classer(&repr(vec![0.1, 0.2, 0.3]), &[], 0.9, 0);
        assert_eq!(c.uncertainty, Uncertainty::Unknown);
        assert_eq!(c.class.as_str(), CLASSE_INCONNUE);
    }

    #[test]
    fn la_projection_quantifiee_peut_collisionner_sans_etre_une_identite() {
        // v1 et v2 sont ORTHOGONAUX, donc des représentations distinctes.
        let v1 = vec![0.1f32, 0.0];
        let v2 = vec![0.0f32, 0.1];
        assert_eq!(cosinus(&v1, &v2), Some(0.0), "orthogonaux, pas identiques");
        // Mais à quantification=1, leurs composantes s'arrondissent pareil (0.1 → 0) :
        // la projection quantifiée est IDENTIQUE. C'est une collision d'indice, pas une
        // identité logique — deux connaissances distinctes ne doivent PAS fusionner.
        assert_eq!(
            poignee_de_projection(&v1, 1),
            poignee_de_projection(&v2, 1),
            "collision de projection : la poignée n'est pas une identité"
        );
    }

    #[test]
    fn la_saturation_i64_des_grandes_amplitudes_ne_cree_pas_d_identite() {
        // Une amplitude énorme quantifiée sature le `as i64` (débordement vers la valeur
        // extrême) : la poignée qui en sort n'a aucun sens comme identité. On vérifie
        // seulement qu'elle reste une poignée (Some), jamais une identité de connaissance.
        let grande = vec![f32::MAX, 0.0];
        let poignee = poignee_de_projection(&grande, 1);
        assert!(poignee.is_some());
        assert!(poignee.unwrap().as_str().starts_with("repr."));
    }
}
