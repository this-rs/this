//! # Le harness, comme nœuds de graphe
//!
//! Un harness n'est pas seulement du code : c'est une **composition** (un
//! objectif, un brief, des emplacements tenus par des modules) et un **journal**
//! (un run, ses étapes, sa raison d'arrêt). Les deux sont des données — et des
//! données qui ont leur place dans le même graphe que le reste du travail,
//! comme les plans et les tâches d'un orchestrateur, ou la progression d'un
//! chantier lu en direct.
//!
//! Ce module ne connaît **ni obrain, ni le réseau, ni le DOM** : il projette des
//! structures vers un [`Node`] (un label + des propriétés textuelles) et
//! inversement. C'est ce qui permet au même modèle d'être écrit par un substrat
//! natif, par un wasm en mémoire ou par n'importe quel magasin qui sait ranger
//! un label et des paires clé/valeur.
//!
//! ## La convention, celle de this-rs
//!
//! Chaque nœud porte `_uuid` (son identité) et `_data` (l'objet entier, en
//! JSON). Les champs qu'on veut **interroger** sont recopiés à côté — un graphe
//! dans lequel on ne peut pas écrire `MATCH (h:_harness {_name: 'atelier'})`
//! n'est qu'un tas de JSON.
//!
//! | label | propriétés interrogeables |
//! |---|---|
//! | `_harness` | `_uuid`, `_name`, `_version`, `_fingerprint` |
//! | `_run` | `_uuid`, `_goal`, `_stop` |
//! | `_step` | `_uuid`, `_run`, `_index`, `_tool`, `_refused`, `_failed`, `_ok`, `_out_excerpt` |
//!
//! Les propriétés de **preuve** d'une étape (`_ok`, `_out_excerpt`, et
//! `_out_hash` posé par le magasin) sont calculées par le harness depuis
//! l'étape observée, jamais recopiées d'une affirmation du modèle.
//!
//! ## Ce que ça change, et ce que ça ne protège pas
//!
//! Un harness dans le graphe est un harness **que l'agent peut lire et
//! réécrire** : il peut proposer une autre politique, un autre brief, un autre
//! module d'outils. C'est exactement ce qu'on veut — et exactement ce qu'il faut
//! encadrer : `_fingerprint` détecte qu'une composition a changé, il ne prouve
//! pas qu'elle est autorisée. L'approbation (signature, revue, ancre de
//! confiance) reste au-dessus, dans l'hôte. Un agent qui s'accorde sa propre
//! politique n'a pas de limite.

use serde::{Deserialize, Serialize};

use crate::{Goal, Run, Step, Stop};

/// Label des compositions de harness.
pub const HARNESS: &str = "_harness";
/// Label d'un run.
pub const RUN: &str = "_run";
/// Label d'une étape de run.
pub const STEP: &str = "_step";

// ---- Types d'arêtes
//
// Le vocabulaire des relations, partagé par le framework et l'application
// Atelier (contrat : `atelier/GRAPHE.md`, section 1). Contrairement aux labels
// de nœuds, aucun préfixe `_` : un type d'arête est un verbe en majuscules.
//
// **Règle commune à toutes** : une arête `(from, type, to)` est unique. Écrire
// deux fois le même triplet ne crée pas une seconde arête : la première est
// mise à jour (ses propriétés) et son identifiant est rendu tel quel.

/// `_module` → `_entity` : le module déclare cette entité.
///
/// Écrite par l'indexeur quand un descripteur est validé. Unique par triplet.
pub const DECLARES: &str = "DECLARES";
/// `_entity` → `_entity` : un lien déclaré entre deux entités.
///
/// Porte `_name` (le nom du lien) et `_target`. Unique par triplet : deux liens
/// de noms différents entre les mêmes entités se distinguent par leurs
/// propriétés, jamais par un doublon d'arête.
pub const LINKS: &str = "LINKS";
/// `_module` → `_module` : le module en exige un autre (`requires[]`).
///
/// Sans elle, un lien vers une entité d'un autre module est refusé
/// (`this.module.reference_unknown`). Unique par triplet.
pub const REQUIRES: &str = "REQUIRES";
/// ligne → ligne : un champ `{"ref": …}` pointe vers une autre ligne.
///
/// Porte `_field` (le champ qui référence). Écrite à la création ou au patch de
/// la ligne. Unique par triplet.
pub const REFERENCES: &str = "REFERENCES";
/// `_run` → `_step` : l'étape appartient au run.
///
/// Porte `_index`, qui seul donne l'ordre : un graphe ne garantit aucun ordre de
/// lecture. Unique par triplet.
pub const HAS_STEP: &str = "HAS_STEP";
/// `_note` → n'importe quel nœud : de quoi parle la note.
///
/// C'est ce qui fait d'une mémoire autre chose qu'un journal : `recall` rend les
/// voisins des notes trouvées. Unique par triplet.
pub const ABOUT: &str = "ABOUT";
/// ligne (`message`) → ligne (`conversation`) : le message est dans la
/// conversation.
///
/// Unique par triplet. Un message n'est dans qu'une seule conversation.
pub const IN: &str = "IN";

/// `_note` → `_step` : la note a été tirée de cette étape (sa preuve d'origine). Unique par triplet.
pub const DERIVED_FROM: &str = "DERIVED_FROM";
/// `_step` → n'importe quel nœud : l'étape a lu ce nœud (ce que l'outil a vu). Unique par triplet.
pub const SAW: &str = "SAW";
/// `_note` → `_note` : la note remplace une note antérieure, qui reste lisible. Unique par triplet.
pub const SUPERSEDES: &str = "SUPERSEDES";

/// Tous les types d'arêtes connus, dans l'ordre du contrat.
pub const EDGE_TYPES: [&str; 10] = [
    DECLARES,
    LINKS,
    REQUIRES,
    REFERENCES,
    HAS_STEP,
    ABOUT,
    IN,
    DERIVED_FROM,
    SAW,
    SUPERSEDES,
];

/// Longueur maximale (en caractères) de `_out_excerpt` sur un nœud `_step`.
pub const OUT_EXCERPT_CHARS: usize = 200;

/// Un nœud, tel qu'un magasin le range : un label et des propriétés textuelles.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Node {
    pub label: &'static str,
    pub props: Vec<(String, String)>,
}

impl Node {
    pub fn get(&self, key: &str) -> Option<&str> {
        self.props
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    }
}

/// L'emplacement d'une composition : quel module le tient, en quelle version.
///
/// `entry` est le point d'entrée **dans** le module (un programme, une fonction
/// exportée) : deux versions d'un module peuvent tenir le même emplacement avec
/// un code différent, et c'est la version qui le dit, pas le nom.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Slot {
    pub kind: SlotKind,
    pub module: String,
    pub version: String,
    pub entry: String,
}

/// Les quatre emplacements qu'un harness compose.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SlotKind {
    /// Ce qu'on ajoute au brief avant le premier tour.
    Brief,
    /// La décision « on continue, ou on s'arrête et pourquoi ».
    Policy,
    /// Les outils exposés au modèle.
    Tools,
    /// Ce qui observe et refuse (garde-fou), avant ou après une étape.
    Guard,
}

impl SlotKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            SlotKind::Brief => "brief",
            SlotKind::Policy => "policy",
            SlotKind::Tools => "tools",
            SlotKind::Guard => "guard",
        }
    }
}

/// La composition d'un harness : ce qui est **décidé**, pas ce qui est codé.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HarnessDoc {
    /// Identité stable : renommer ne change pas le harness, changer d'id si.
    pub id: String,
    pub name: String,
    /// Incrémentée à chaque composition acceptée. Deux versions, deux
    /// comportements — un run dit toujours sous quelle version il a tourné.
    pub version: u32,
    /// Le texte de base, avant les sections apportées par les emplacements.
    pub brief: String,
    pub slots: Vec<Slot>,
    pub max_tours: u32,
    pub max_relances: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub budget_ms: Option<u64>,
}

impl HarnessDoc {
    /// Le nœud `_harness`, avec les champs sur lesquels on veut pouvoir requêter.
    pub fn node(&self) -> Result<Node, String> {
        let data = serde_json::to_string(self).map_err(|e| format!("harness: {e}"))?;
        Ok(Node {
            label: HARNESS,
            props: vec![
                ("_uuid".to_string(), self.id.clone()),
                ("_name".to_string(), self.name.clone()),
                ("_version".to_string(), self.version.to_string()),
                ("_fingerprint".to_string(), self.fingerprint()),
                ("_data".to_string(), data),
            ],
        })
    }

    /// Relit une composition depuis un nœud.
    pub fn from_node(node: &Node) -> Result<Self, String> {
        if node.label != HARNESS {
            return Err(format!("attendu {HARNESS}, obtenu {}", node.label));
        }
        let data = node.get("_data").ok_or("nœud _harness sans _data")?;
        serde_json::from_str(data).map_err(|e| format!("harness illisible: {e}"))
    }

    /// Empreinte du contenu : **détecte** qu'une composition a changé.
    ///
    /// FNV-1a sur le JSON sérialisé, sans dépendance. Ce n'est pas une preuve
    /// d'autorisation — deux contenus différents ont deux empreintes, mais
    /// n'importe qui peut en produire une. L'approbation vit dans l'hôte.
    pub fn fingerprint(&self) -> String {
        let data = serde_json::to_string(self).unwrap_or_default();
        let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
        for octet in data.as_bytes() {
            hash ^= u64::from(*octet);
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
        format!("{hash:016x}")
    }

    /// L'emplacement d'un genre donné, s'il est tenu.
    pub fn slot(&self, kind: SlotKind) -> Option<&Slot> {
        self.slots.iter().find(|slot| slot.kind == kind)
    }
}

/// Projette un run et ses étapes en nœuds.
///
/// Un seul nœud `_run` (l'objectif, la raison d'arrêt) et un nœud `_step` par
/// étape. `run_id` est fourni par l'appelant : un `Run` n'a pas d'identité
/// propre, et en inventer une ici rendrait deux écritures du même run
/// indistinguables.
pub fn run_nodes(run: &Run, run_id: &str) -> Result<Vec<Node>, String> {
    let data = serde_json::to_string(run).map_err(|e| format!("run: {e}"))?;
    let mut nodes = vec![Node {
        label: RUN,
        props: vec![
            ("_uuid".to_string(), run_id.to_string()),
            ("_goal".to_string(), run.goal.text.clone()),
            (
                "_stop".to_string(),
                run.stop
                    .as_ref()
                    .map(Stop::code)
                    .unwrap_or("running")
                    .to_string(),
            ),
            ("_tours".to_string(), run.tours.to_string()),
            ("_data".to_string(), data),
        ],
    }];
    for step in &run.steps {
        nodes.push(step_node(step, run_id));
    }
    Ok(nodes)
}

/// La sortie de l'outil telle que l'étape l'a gardée : `resume`.
///
/// C'est le texte rendu par l'outil (ou, à défaut, le motif du refus / la cause
/// de l'échec, cf. [`Step::refused`] et [`Step::failed`]). Attention : la boucle
/// de `this-harness` le tronque déjà à 200 caractères avant de l'enregistrer ;
/// ce qui n'est pas dans l'étape n'existe pas pour le graphe.
pub fn step_output(step: &Step) -> &str {
    &step.resume
}

/// L'étape a-t-elle réussi ? Ni refusée, ni échouée.
pub fn step_ok(step: &Step) -> bool {
    !step.refused && step.failure.is_none()
}

/// Le début de la sortie, borné à [`OUT_EXCERPT_CHARS`] caractères (jamais
/// coupé au milieu d'un caractère).
pub fn step_out_excerpt(step: &Step) -> String {
    step_output(step).chars().take(OUT_EXCERPT_CHARS).collect()
}

/// Un nœud d'étape. `_index` porte l'ordre : un graphe ne garantit aucun ordre
/// de lecture, et un journal d'étapes qui se relit mélangé n'est pas un journal.
///
/// `_ok` et `_out_excerpt` sont dérivés de l'étape elle-même ; `_out_hash`
/// (empreinte de la sortie) est posé par le magasin, qui a la fonction de hachage.
pub fn step_node(step: &Step, run_id: &str) -> Node {
    let data = serde_json::to_string(step).unwrap_or_default();
    Node {
        label: STEP,
        props: vec![
            ("_uuid".to_string(), format!("{run_id}#{}", step.index)),
            ("_run".to_string(), run_id.to_string()),
            ("_index".to_string(), step.index.to_string()),
            ("_tool".to_string(), step.tool.clone()),
            ("_refused".to_string(), step.refused.to_string()),
            ("_failed".to_string(), step.failure.is_some().to_string()),
            ("_ms".to_string(), step.ms.to_string()),
            ("_ok".to_string(), step_ok(step).to_string()),
            ("_out_excerpt".to_string(), step_out_excerpt(step)),
            ("_data".to_string(), data),
        ],
    }
}

/// Relit un run depuis ses nœuds.
///
/// Les étapes sont **réordonnées par `_index`**, pas par ordre d'arrivée : un
/// magasin qui rend ses nœuds dans un autre ordre doit produire le même run.
pub fn run_from_nodes(nodes: &[Node]) -> Result<Option<Run>, String> {
    let Some(head) = nodes.iter().find(|node| node.label == RUN) else {
        return Ok(None);
    };
    let data = head.get("_data").ok_or("nœud _run sans _data")?;
    let mut run: Run = serde_json::from_str(data).map_err(|e| format!("run illisible: {e}"))?;
    let mut steps: Vec<Step> = nodes
        .iter()
        .filter(|node| node.label == STEP)
        .map(|node| {
            let raw = node.get("_data").ok_or("nœud _step sans _data")?;
            serde_json::from_str(raw).map_err(|e| format!("étape illisible: {e}"))
        })
        .collect::<Result<_, _>>()?;
    steps.sort_by_key(|step| step.index);
    // Le nœud de tête fait foi pour ce qui n'est pas l'étape ; les étapes
    // viennent des nœuds, et elles doivent être les mêmes.
    if !steps.is_empty() {
        run.steps = steps;
    }
    Ok(Some(run))
}

impl Stop {
    /// Le code stable d'un arrêt, pour une propriété de graphe.
    pub fn code(&self) -> &'static str {
        match self {
            Stop::Done => "done",
            Stop::Budget => "budget",
            Stop::Refused(_) => "refused",
            Stop::Failed(_) => "failed",
            Stop::Cancelled => "cancelled",
            Stop::Interrupted => "interrupted",
        }
    }
}

/// Un objectif, pour les nœuds de run.
impl Goal {
    /// Texte de l'objectif tel qu'il s'écrit dans le graphe.
    pub fn label(&self) -> &str {
        &self.text
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc() -> HarnessDoc {
        HarnessDoc {
            id: "h-atelier".into(),
            name: "atelier".into(),
            version: 3,
            brief: "tu es l'atelier".into(),
            slots: vec![Slot {
                kind: SlotKind::Policy,
                module: "harness-default".into(),
                version: "0.1.0".into(),
                entry: "policy".into(),
            }],
            max_tours: 12,
            max_relances: 3,
            budget_ms: Some(900_000),
        }
    }

    #[test]
    fn une_composition_fait_un_noeud_interrogeable() {
        let node = doc().node().unwrap();
        assert_eq!(node.label, HARNESS);
        // Ce qu'on veut pouvoir requêter est là, en clair.
        assert_eq!(node.get("_name"), Some("atelier"));
        assert_eq!(node.get("_version"), Some("3"));
        assert!(node.get("_fingerprint").is_some());
        assert!(node.get("_data").is_some());
    }

    #[test]
    fn une_composition_se_relit_a_l_identique() {
        let original = doc();
        let back = HarnessDoc::from_node(&original.node().unwrap()).unwrap();
        assert_eq!(back, original);
        assert_eq!(
            back.slot(SlotKind::Policy).unwrap().module,
            "harness-default"
        );
        assert!(
            back.slot(SlotKind::Tools).is_none(),
            "un emplacement vide n'est pas un emplacement"
        );
    }

    #[test]
    fn changer_la_composition_change_l_empreinte() {
        let mut autre = doc();
        assert_eq!(
            autre.fingerprint(),
            doc().fingerprint(),
            "même contenu, même empreinte"
        );
        autre.version = 4;
        assert_ne!(autre.fingerprint(), doc().fingerprint());
        // L'empreinte ne dépend pas de l'ordre des propriétés du nœud.
        let mut node = doc().node().unwrap();
        node.props.reverse();
        assert_eq!(
            HarnessDoc::from_node(&node).unwrap().fingerprint(),
            doc().fingerprint()
        );
    }

    #[test]
    fn un_run_devient_un_noeud_plus_une_etape_par_etape() {
        let mut run = Run::new(Goal::new("installe le module conversation"), 6);
        run.record(Step::tool("install_module", 1840).state("a", "b"));
        run.record(Step::tool("list_modules", 3).refused("même version"));
        let nodes = run_nodes(&run, "run-1").unwrap();

        assert_eq!(nodes.len(), 3, "un run + deux étapes");
        assert_eq!(nodes[0].label, RUN);
        assert_eq!(
            nodes[0].get("_goal"),
            Some("installe le module conversation")
        );
        assert_eq!(nodes[0].get("_stop"), Some("running"));
        assert_eq!(nodes[2].get("_tool"), Some("list_modules"));
        assert_eq!(nodes[2].get("_refused"), Some("true"));
        assert_eq!(nodes[2].get("_index"), Some("1"));
    }

    #[test]
    fn un_run_se_relit_meme_si_les_noeuds_arrivent_melanges() {
        let mut run = Run::new(Goal::new("range"), 6);
        run.record(Step::tool("a", 1).state("x", "y"));
        run.record(Step::tool("b", 2).state("y", "z"));
        run.record(Step::tool("c", 3).state("z", "w"));
        run.after_turn(false, true);

        let mut nodes = run_nodes(&run, "run-2").unwrap();
        // Un magasin ne garantit aucun ordre : on mélange exprès.
        nodes.reverse();
        let back = run_from_nodes(&nodes).unwrap().unwrap();
        let tools: Vec<&str> = back.steps.iter().map(|s| s.tool.as_str()).collect();
        assert_eq!(
            tools,
            ["a", "b", "c"],
            "l'ordre vient de _index, pas de la lecture"
        );
        assert_eq!(back.goal.text, "range");
        assert_eq!(back.tours, run.tours);
    }

    #[test]
    fn un_arret_se_lit_en_clair_dans_le_graphe() {
        let mut run = Run::new(Goal::new("boucle"), 1);
        run.after_turn(false, true);
        let stop = run.finish(Stop::Refused("même version".into()));
        let nodes = run_nodes(&run, "run-3").unwrap();
        assert_eq!(
            nodes[0].get("_stop"),
            Some(Stop::Refused("même version".into()).code())
        );
        assert_eq!(stop.code(), "refused");
    }

    #[test]
    fn les_types_d_aretes_suivent_le_contrat_et_restent_distincts() {
        assert_eq!(
            EDGE_TYPES,
            [
                "DECLARES",
                "LINKS",
                "REQUIRES",
                "REFERENCES",
                "HAS_STEP",
                "ABOUT",
                "IN",
                "DERIVED_FROM",
                "SAW",
                "SUPERSEDES"
            ]
        );
        let uniques: std::collections::BTreeSet<_> = EDGE_TYPES.iter().collect();
        assert_eq!(uniques.len(), EDGE_TYPES.len());
        // Un type d'arête n'est pas un label de nœud : pas de `_` en tête.
        for t in EDGE_TYPES {
            assert!(!t.starts_with('_'), "{t} ressemble à un label réservé");
            assert_eq!(t, t.to_uppercase());
        }
    }

    #[test]
    fn une_etape_porte_ses_preuves_calculees() {
        let ok = step_node(&Step::tool("lire", 2).resume("x".repeat(500)), "r");
        assert_eq!(ok.get("_ok"), Some("true"));
        assert_eq!(
            ok.get("_out_excerpt").map(|s| s.chars().count()),
            Some(OUT_EXCERPT_CHARS),
            "l'extrait est borné"
        );
        let refus = step_node(&Step::tool("ecrire", 1).refused("même version"), "r");
        assert_eq!(refus.get("_ok"), Some("false"));
        assert_eq!(refus.get("_out_excerpt"), Some("même version"));
        let echec = step_node(&Step::tool("ecrire", 1).failed("disque plein"), "r");
        assert_eq!(echec.get("_ok"), Some("false"));
        // Un extrait multi-octets ne se coupe pas au milieu d'un caractère.
        let accents = step_node(&Step::tool("t", 1).resume("é".repeat(300)), "r");
        assert_eq!(
            accents.get("_out_excerpt"),
            Some("é".repeat(OUT_EXCERPT_CHARS).as_str())
        );
    }

    #[test]
    fn un_noeud_qui_n_est_pas_un_harness_est_refuse() {
        let node = Node {
            label: RUN,
            props: vec![("_data".to_string(), "{}".to_string())],
        };
        assert!(HarnessDoc::from_node(&node).is_err());
    }
}
