//! **Démonstration : la décision ne lit pas les mots.**
//!
//! Le banc fournit un [`SemanticEncoder`] qui **ne regarde pas le texte** : il rend un
//! vecteur déjà calculé, associé à la phrase entière. Ce n'est pas un détecteur — il
//! ne découpe rien, ne cherche aucun mot, n'obéit à aucune règle lexicale. Il simule
//! une couche perception qualifiée, et permet de démontrer ce qui compte :
//!
//! * la **même** demande, exprimée en français, anglais, allemand, arabe, japonais ou
//!   en paraphrase sans mot commun, tombe dans la **même** classe apprise ;
//! * une formulation qui **contient** le mot d'une famille mais dont la représentation
//!   appartient à une autre (négation, citation, vocabulaire trompeur) suit sa
//!   représentation, pas ses mots ;
//! * sans représentation, le système **dit** qu'il ne sait pas et route avec sa
//!   politique neutre — il n'existe plus de retour silencieux à un lexique.
//!
//! Ce que ce banc ne prouve pas : qu'un encodeur *réel* place bien ces phrases-là au
//! même endroit. C'est une question d'encodeur, pas de cette couche ; le contrat exige
//! seulement que la décision ne consomme que des vecteurs.

use std::collections::HashMap;
use std::rc::Rc;
use std::sync::{Mutex, RwLock};

use async_trait::async_trait;
use this_agent::Stop;
use this_harness::cognition::{
    CLASSE_INCONNUE, Classification, LearnedClass, Prototype, SemanticEncoder, TaskRepr,
    Uncertainty, classer,
};
use this_harness::routing::{
    ArmStats, Capabilities, CostBasis, Learning, MemoryPool, Pick, RouterSpec, RoutingSettings,
    SequenceSeeds, TurnInput, TurnRouter,
};

/// Vecteurs par FAMILLE DE SENS (pas par mot). Trois familles, trois directions.
const FAMILLE_DEBUG: [f32; 3] = [1.0, 0.0, 0.0];
const FAMILLE_PLAN: [f32; 3] = [0.0, 1.0, 0.0];
const FAMILLE_EXPLORE: [f32; 3] = [0.0, 0.0, 1.0];

fn v(base: [f32; 3], bruit: [f32; 3]) -> Vec<f32> {
    vec![base[0] + bruit[0], base[1] + bruit[1], base[2] + bruit[2]]
}

/// Encodeur du banc : rend le vecteur FOURNI pour cette phrase, sans lire ses mots.
/// Une phrase inconnue n'a pas de vecteur — exactement ce que fait un encodeur absent.
struct EncodeurDuBanc {
    vecteurs: HashMap<&'static str, Vec<f32>>,
}

impl SemanticEncoder for EncodeurDuBanc {
    fn encode(&self, text: &str) -> Option<Vec<f32>> {
        self.vecteurs.get(text).cloned()
    }
}

/// Le corpus : trois familles de sens, six langues, paraphrases, négation et pièges.
/// Chaque entrée est `(phrase, famille attendue)`.
fn corpus() -> Vec<(&'static str, [f32; 3])> {
    vec![
        // ── famille DEBUG, six langues ─────────────────────────────────────
        ("pourquoi le paiement échoue ?", FAMILLE_DEBUG),
        ("why does auth fail?", FAMILLE_DEBUG),
        ("warum schlägt die Zahlung fehl?", FAMILLE_DEBUG),
        ("لماذا يفشل الدفع؟", FAMILLE_DEBUG),
        ("支払いが失敗する理由は何ですか", FAMILLE_DEBUG),
        ("¿por qué falla el pago?", FAMILLE_DEBUG),
        // ── paraphrases sans mot commun, même famille ──────────────────────
        ("le service tombe à chaque requête", FAMILLE_DEBUG),
        ("ça ne marche pas du tout depuis ce matin", FAMILLE_DEBUG),
        ("it blows up every time I press the button", FAMILLE_DEBUG),
        // ── famille PLAN ───────────────────────────────────────────────────
        ("planifie un endpoint d'inscription", FAMILLE_PLAN),
        ("implement registration end to end", FAMILLE_PLAN),
        ("le projet de la nouvelle passerelle", FAMILLE_PLAN),
        ("mitte März den Umbau vorbereiten", FAMILLE_PLAN),
        // ── famille EXPLORE ────────────────────────────────────────────────
        ("comment fonctionne le cache ?", FAMILLE_EXPLORE),
        ("how does the API work?", FAMILLE_EXPLORE),
        ("was macht der Scheduler genau", FAMILLE_EXPLORE),
        // ── CONTRE-ÉPREUVES : les mots mentent, la représentation tranche ───
        // « bug » et « erreur » sont dans la phrase, mais la demande est un PLAN.
        (
            "ne corrige pas tout de suite ce bug : planifie d'abord la refonte",
            FAMILLE_PLAN,
        ),
        // citation : le mot est rapporté, ce n'est pas la demande.
        (
            "le ticket dit « please fix the error » mais je veux comprendre l'architecture",
            FAMILLE_EXPLORE,
        ),
    ]
}

/// Mémoire de prototypes en RAM : ce que ferait un `Learning` qui les tient dans Obrain.
#[derive(Default)]
struct MemoirePrototypes {
    prototypes: RwLock<Vec<Prototype>>,
    /// Les classes que le ROUTEUR a demandées à l'apprentissage.
    classes_vues: Mutex<Vec<String>>,
    /// Nombre de classifications rendues inconnues.
    inconnues: Mutex<usize>,
}

impl MemoirePrototypes {
    fn classer(&self, repr: &TaskRepr) -> Classification {
        let c = classer(repr, &self.prototypes.read().unwrap(), 0.9, 1000);
        if !c.uncertainty.grounded() {
            *self.inconnues.lock().unwrap() += 1;
        }
        c
    }

    /// Mémorise un prototype (formation d'une classe si elle est nouvelle).
    fn apprendre(&self, classe: &LearnedClass, vector: &[f32]) {
        self.prototypes.write().unwrap().push(Prototype {
            class: classe.clone(),
            vector: vector.to_vec(),
        });
    }
}

#[async_trait(?Send)]
impl Learning for MemoirePrototypes {
    async fn classify(&self, repr: &TaskRepr) -> Classification {
        self.classer(repr)
    }
    async fn arms(&self, classe: &str) -> Vec<ArmStats> {
        self.classes_vues.lock().unwrap().push(classe.to_string());
        Vec::new()
    }
    async fn settle(&self, _id: u128, _served: Option<&Pick>, _stop: &Stop, _code: Option<&str>) {}
}

fn encodeur() -> Rc<EncodeurDuBanc> {
    let mut vecteurs = HashMap::new();
    for (i, (phrase, famille)) in corpus().into_iter().enumerate() {
        // Un bruit minuscule mais distinct par phrase : la représentation n'est pas
        // une égalité de chaînes, c'est une proximité de vecteurs.
        let bruit = [0.01 * (i % 3) as f32, 0.01 * (i % 2) as f32, 0.005];
        vecteurs.insert(phrase, v(famille, bruit));
    }
    Rc::new(EncodeurDuBanc { vecteurs })
}

#[test]
fn la_decision_ne_lit_pas_les_mots() {
    let enc = encodeur();
    let memoire = Rc::new(MemoirePrototypes::default());

    // Amorçage : un seul exemple par famille, dans UNE langue.
    for (exemple, famille) in [
        ("why does auth fail?", FAMILLE_DEBUG),
        ("implement registration end to end", FAMILLE_PLAN),
        ("how does the API work?", FAMILLE_EXPLORE),
    ] {
        let repr = TaskRepr {
            embedding: enc.encode(exemple),
            ..Default::default()
        };
        let c = memoire.classer(&repr);
        assert!(c.uncertainty.grounded());
        memoire.apprendre(&c.class, repr.embedding.as_ref().unwrap());
        let _ = famille;
    }

    // Chaque phrase du corpus est classée par SA REPRÉSENTATION.
    let mut par_famille: HashMap<String, Vec<String>> = HashMap::new();
    for (phrase, famille) in corpus() {
        let repr = TaskRepr {
            embedding: enc.encode(phrase),
            ..Default::default()
        };
        let c = memoire.classer(&repr);
        assert!(
            c.uncertainty.grounded(),
            "« {phrase} » n'a pas été classée : {c:?}"
        );
        par_famille
            .entry(c.class.as_str().to_string())
            .or_default()
            .push(phrase.to_string());

        // Le contrôle de sens : la classe rendue doit être celle de la FAMILLE,
        // c'est-à-dire la classe du prototype de cette famille.
        let classe_famille = memoire
            .prototypes
            .read()
            .unwrap()
            .iter()
            .filter(|p| {
                let s = this_harness::cognition::cosinus(&p.vector, &v(famille, [0.0; 3]));
                s > 0.9
            })
            .map(|p| p.class.clone())
            .next()
            .expect("un prototype pour la famille");
        assert_eq!(
            c.class, classe_famille,
            "« {phrase} » a été classée {c:?} alors que sa représentation appartient à {classe_famille:?}"
        );
    }

    // Trois familles → trois classes, pas plus : les six langues et les paraphrases
    // n'ont pas fabriqué de classes supplémentaires.
    assert_eq!(
        par_famille.len(),
        3,
        "les langues et paraphrases doivent se rejoindre, pas se multiplier : {par_famille:?}"
    );

    // Contre-épreuve explicite : la phrase qui contient « bug » n'est PAS en debug.
    let piege = TaskRepr {
        embedding: enc.encode("ne corrige pas tout de suite ce bug : planifie d'abord la refonte"),
        ..Default::default()
    };
    let classe_piege = memoire.classer(&piege).class;
    let debug = memoire
        .prototypes
        .read()
        .unwrap()
        .iter()
        .find(|p| this_harness::cognition::cosinus(&p.vector, &v(FAMILLE_DEBUG, [0.0; 3])) > 0.9)
        .map(|p| p.class.clone())
        .unwrap();
    assert_ne!(
        classe_piege, debug,
        "un lexique aurait classé cette phrase en débogage à cause du mot « bug »"
    );
}

#[test]
fn sans_representation_l_incertitude_est_dite_et_le_routeur_route_quand_meme() {
    let memoire = Rc::new(MemoirePrototypes::default());
    // Aucun encodeur : chaque tour est explicitement inconnu.
    let pool = Rc::new(MemoryPool::new(vec![this_harness::routing::Candidate {
        cost_basis: CostBasis::Subscription,
        healthy: Some(true),
        ..this_harness::routing::Candidate::new(
            "a",
            "m1",
            Capabilities {
                tools: true,
                images: false,
                reasoning: false,
                context_window: Some(200_000),
            },
        )
    }]));
    let routeur = TurnRouter::new(
        RouterSpec::new(
            pool,
            Rc::new(SequenceSeeds::from(3)),
            "a".to_string(),
            "m1".to_string(),
        )
        .learning(memoire.clone())
        .auto()
        .settings(RoutingSettings {
            exploration_epsilon: 0.0,
            ..Default::default()
        }),
    );

    for texte in [
        "pourquoi le paiement échoue ?",
        "لماذا يفشل الدفع؟",
        "支払いが失敗する理由は何ですか",
    ] {
        let routed = futures::executor::block_on(routeur.route(TurnInput {
            text: texte,
            has_images: false,
            hints: Default::default(),
            honored: true,
        }));
        // Le routeur décide quand même : politique neutre, aucune exception.
        assert!(
            routed.decision.is_some(),
            "le routeur doit décider ({texte})"
        );
    }

    // L'apprentissage a été consulté avec la classe EXPLICITE de l'incertitude,
    // jamais avec une catégorie lexicale.
    let vues = memoire.classes_vues.lock().unwrap().clone();
    assert!(
        !vues.is_empty(),
        "l'apprentissage doit être consulté même sans représentation"
    );
    for classe in &vues {
        assert_eq!(
            classe, CLASSE_INCONNUE,
            "sans encodeur, aucune classe lexicale ne doit être inventée (vu : {classe})"
        );
    }
    assert!(
        *memoire.inconnues.lock().unwrap() > 0,
        "l'incertitude doit être comptée, pas silencieuse"
    );
}

#[test]
fn avec_encodeur_la_classe_apprise_remplace_la_metadonnee() {
    let enc = encodeur();
    let memoire = Rc::new(MemoirePrototypes::default());
    // Un prototype de la famille PLAN.
    let exemple = TaskRepr {
        embedding: enc.encode("implement registration end to end"),
        ..Default::default()
    };
    let c = memoire.classer(&exemple);
    memoire.apprendre(&c.class, exemple.embedding.as_ref().unwrap());

    let pool = Rc::new(MemoryPool::new(vec![this_harness::routing::Candidate {
        cost_basis: CostBasis::Subscription,
        healthy: Some(true),
        ..this_harness::routing::Candidate::new(
            "a",
            "m1",
            Capabilities {
                tools: true,
                images: false,
                reasoning: false,
                context_window: Some(200_000),
            },
        )
    }]));
    let routeur = TurnRouter::new(
        RouterSpec::new(
            pool,
            Rc::new(SequenceSeeds::from(5)),
            "a".to_string(),
            "m1".to_string(),
        )
        .learning(memoire.clone())
        .encoder(enc.clone())
        .auto()
        .settings(RoutingSettings::default()),
    );

    // Une phrase d'une AUTRE langue, même sens : elle doit rejoindre la classe apprise.
    let _ = futures::executor::block_on(routeur.route(TurnInput {
        text: "planifie un endpoint d'inscription",
        has_images: false,
        hints: Default::default(),
        honored: true,
    }));
    let vues = memoire.classes_vues.lock().unwrap().clone();
    assert!(!vues.is_empty());
    for classe in &vues {
        assert!(
            classe.starts_with("repr."),
            "la classe transmise à l'apprentissage doit venir de la représentation, pas d'une métadonnée ({classe})"
        );
        assert_ne!(
            classe, CLASSE_INCONNUE,
            "un prototype existe : la classe est connue"
        );
        assert!(
            !classe.starts_with("chat."),
            "aucune catégorie lexicale ne doit atteindre l'apprentissage ({classe})"
        );
    }
    let _ = Uncertainty::Unknown;
    let _ = CLASSE_INCONNUE;
}
