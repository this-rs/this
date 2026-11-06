//! # Le substrat de l'agent : ce qu'il sait, et ce qu'il peut faire
//!
//! Un harness décide et journalise ([`crate::graph`]) ; il lui manque deux
//! choses pour tourner : **le contexte** qu'on lui donne à lire, et **les
//! capacités** qu'on lui accorde. Les deux sont des données durables, au même
//! titre que sa composition — elles vivent donc dans le même graphe, avec les
//! mêmes conventions.
//!
//! ## Ce que « contexte » veut dire ici
//!
//! Pas un sac de texte : des **sections** typées, datées, priorisées, dont
//! chacune nomme d'où elle vient (`descriptor`, `registry`, `observation`, …).
//! Un assemblage qui ne sait pas dire d'où vient une phrase ne peut pas en
//! retirer une.
//!
//! ### Le budget, et l'honnêteté de la coupe
//!
//! Une fenêtre de contexte est finie : assembler, c'est **choisir**. Deux
//! erreurs sont faciles, et j'ai commis la première ailleurs :
//!
//! 1. **s'arrêter à la première section qui ne tient pas** — les suivantes
//!    disparaissent alors qu'elles tiendraient, et l'ordre de déclaration décide
//!    à la place de la priorité ;
//! 2. **couper en silence** — le modèle lit un contexte incomplet en croyant
//!    qu'il est complet.
//!
//! [`Context::render`] ne fait ni l'un ni l'autre : il parcourt **toutes** les
//! sections par priorité décroissante, garde celles qui tiennent, et **nomme
//! les omises** dans le texte rendu ([`Rendered::dropped`]). Une coupe qui ne
//! se dit pas est un mensonge ; une coupe qui se dit est une information.
//!
//! ## Ce que « capacités » veut dire ici — et pourquoi pas `Capability`
//!
//! `Capability` désigne déjà **trois** choses dans l'écosystème : une aptitude
//! du matériel (`this-platform`, `this-app`), et une permission de descripteur
//! (`this-descriptor`). Ce module ne parle ni de caméra ni de permission
//! d'entité : il parle de **ce qu'un agent a le droit d'invoquer**. Le mot est
//! donc [`Grant`], et la règle est celle de tout le reste : **fail-closed** —
//! un outil sans autorisation explicite est refusé, avec sa raison.

use serde::{Deserialize, Serialize};

pub use crate::graph::Node;

/// Label d'une section de contexte.
pub const CONTEXT: &str = "_context";
/// Label d'une autorisation d'outil.
pub const GRANT: &str = "_grant";

// ---- contexte ---------------------------------------------------------------

/// D'où vient une section, et donc ce qu'elle vaut.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SectionKind {
    /// Le texte de base du harness : l'identité, jamais coupée.
    Brief,
    /// Ce que le module déclare (entités, liens, erreurs métier).
    Descriptor,
    /// Ce que l'agent a installé (modules, versions, programmes).
    Registry,
    /// Un fait établi, vérifié.
    Fact,
    /// Ce qui a été observé pendant un run.
    Observation,
    /// Le journal : étapes passées, arrêts.
    Journal,
}

impl SectionKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            SectionKind::Brief => "brief",
            SectionKind::Descriptor => "descriptor",
            SectionKind::Registry => "registry",
            SectionKind::Fact => "fact",
            SectionKind::Observation => "observation",
            SectionKind::Journal => "journal",
        }
    }
}

/// Une section de contexte : un titre, un corps, une provenance, une priorité.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Section {
    /// Identité stable dans l'agent : re-écrire une section remplace, n'ajoute pas.
    pub id: String,
    pub kind: SectionKind,
    pub title: String,
    pub body: String,
    /// 0 = jamais omise tant que le budget le permet ; 255 = sacrifiable.
    pub priority: u8,
    /// Horodatage (ms) : départage à priorité égale, l'information récente d'abord.
    pub at_ms: u64,
    /// Qui l'a produite : `descriptor`, `registry`, `tool:install_module`, …
    pub source: String,
}

impl Section {
    /// Le nœud `_context` : `{agent}#{id}` — deux agents peuvent avoir une
    /// section de même id sans se marcher dessus.
    pub fn node(&self, agent: &str) -> Result<Node, String> {
        let data = serde_json::to_string(self).map_err(|e| format!("section: {e}"))?;
        Ok(Node {
            label: CONTEXT,
            props: vec![
                ("_uuid".to_string(), format!("{agent}#{}", self.id)),
                ("_agent".to_string(), agent.to_string()),
                ("_kind".to_string(), self.kind.as_str().to_string()),
                ("_priority".to_string(), self.priority.to_string()),
                ("_at".to_string(), self.at_ms.to_string()),
                ("_data".to_string(), data),
            ],
        })
    }
}

/// Le contexte d'un agent : un texte de base et des sections.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Context {
    pub agent: String,
    /// L'identité : émise telle quelle, jamais coupée, jamais priorisée.
    pub base: String,
    pub sections: Vec<Section>,
}

/// Le résultat d'un assemblage : ce qui est sorti, et ce qui a été laissé.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rendered {
    pub text: String,
    /// Identités des sections retenues, dans l'ordre d'émission.
    pub kept: Vec<String>,
    /// Identités des sections omises — nommées dans le texte aussi.
    pub dropped: Vec<String>,
}

impl Context {
    pub fn new(agent: impl Into<String>, base: impl Into<String>) -> Self {
        Self {
            agent: agent.into(),
            base: base.into(),
            sections: Vec::new(),
        }
    }

    /// Ajoute ou remplace une section (même `id` = même place).
    pub fn put(&mut self, section: Section) {
        match self
            .sections
            .iter_mut()
            .find(|existing| existing.id == section.id)
        {
            Some(slot) => *slot = section,
            None => self.sections.push(section),
        }
    }

    /// Assemble le contexte dans un budget de **caractères**.
    ///
    /// Les caractères sont l'unité honnête ici : un token n'a pas de définition
    /// stable d'un modèle à l'autre, et c'est l'hôte qui connaît le sien.
    ///
    /// Ordre : priorité croissante d'abord (0 avant 255), puis le plus récent.
    /// Toutes les sections sont essayées — une section trop grosse n'empêche
    /// pas les suivantes d'entrer.
    pub fn render(&self, budget_chars: usize) -> Rendered {
        let mut order: Vec<&Section> = self.sections.iter().collect();
        order.sort_by(|a, b| {
            a.priority
                .cmp(&b.priority)
                .then(b.at_ms.cmp(&a.at_ms))
                .then(a.id.cmp(&b.id))
        });

        let mut text = self.base.trim_end().to_string();
        let mut used = text.chars().count();
        let mut kept = Vec::new();
        let mut dropped = Vec::new();

        for section in order {
            let block = format!(
                "\n\n## {}\n{}\n",
                section.title.trim(),
                section.body.trim_end()
            );
            let cost = block.chars().count();
            if used + cost <= budget_chars {
                text.push_str(&block);
                used += cost;
                kept.push(section.id.clone());
            } else {
                dropped.push(section.id.clone());
            }
        }

        if !dropped.is_empty() {
            // La coupe se dit : ids, pas seulement un compte, sinon l'hôte ne
            // peut pas savoir *quoi* manque.
            let noms = dropped.join(", ");
            let ligne = format!(
                "\n\n---\n{} section(s) omise(s) faute de budget : {noms}\n",
                dropped.len()
            );
            text.push_str(&ligne);
        }

        Rendered {
            text,
            kept,
            dropped,
        }
    }

    /// Les nœuds `_context` de ce contexte.
    pub fn nodes(&self) -> Result<Vec<Node>, String> {
        self.sections
            .iter()
            .map(|section| section.node(&self.agent))
            .collect()
    }

    /// Relit un contexte depuis ses nœuds (l'agent et le texte de base sont
    /// fournis : ils ne sont pas dans les nœuds de section).
    pub fn from_nodes(agent: &str, base: &str, nodes: &[Node]) -> Result<Self, String> {
        let mut context = Context::new(agent, base);
        for node in nodes.iter().filter(|node| node.label == CONTEXT) {
            let data = node.get("_data").ok_or("section sans _data")?;
            let section: Section =
                serde_json::from_str(data).map_err(|e| format!("section illisible: {e}"))?;
            context.put(section);
        }
        Ok(context)
    }
}

// ---- autorisations ----------------------------------------------------------

/// La réponse à « cet agent peut-il invoquer cet outil ? ».
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Verdict {
    Allow,
    Refuse(String),
}

impl Verdict {
    pub fn is_allowed(&self) -> bool {
        matches!(self, Verdict::Allow)
    }
}

/// Une autorisation : un outil, et qui l'a accordée.
///
/// `source` n'est pas décoratif : « accordé par l'utilisateur », « accordé par
/// le descripteur du module » et « accordé par un module installé » n'ont pas le
/// même poids, et l'hôte doit pouvoir refuser la troisième catégorie.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Grant {
    pub tool: String,
    pub granted: bool,
    pub source: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

impl Grant {
    pub fn granted(tool: impl Into<String>, source: impl Into<String>) -> Self {
        Self {
            tool: tool.into(),
            granted: true,
            source: source.into(),
            reason: None,
        }
    }

    pub fn refused(
        tool: impl Into<String>,
        source: impl Into<String>,
        reason: impl Into<String>,
    ) -> Self {
        Self {
            tool: tool.into(),
            granted: false,
            source: source.into(),
            reason: Some(reason.into()),
        }
    }

    pub fn node(&self, agent: &str) -> Result<Node, String> {
        let data = serde_json::to_string(self).map_err(|e| format!("grant: {e}"))?;
        Ok(Node {
            label: GRANT,
            props: vec![
                ("_uuid".to_string(), format!("{agent}#{}", self.tool)),
                ("_agent".to_string(), agent.to_string()),
                ("_tool".to_string(), self.tool.clone()),
                ("_granted".to_string(), self.granted.to_string()),
                ("_source".to_string(), self.source.clone()),
                ("_data".to_string(), data),
            ],
        })
    }
}

/// L'ensemble des autorisations d'un agent.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Grants {
    pub grants: Vec<Grant>,
}

impl Grants {
    pub fn new() -> Self {
        Self::default()
    }

    /// Accorde (ou retire) — même outil, même place.
    pub fn put(&mut self, grant: Grant) {
        match self
            .grants
            .iter_mut()
            .find(|existing| existing.tool == grant.tool)
        {
            Some(slot) => *slot = grant,
            None => self.grants.push(grant),
        }
    }

    /// **Fail-closed** : sans autorisation explicite, l'outil est refusé, et le
    /// refus dit lequel et pourquoi.
    pub fn allows(&self, tool: &str) -> Verdict {
        match self.grants.iter().find(|grant| grant.tool == tool) {
            Some(grant) if grant.granted => Verdict::Allow,
            Some(grant) => Verdict::Refuse(
                grant
                    .reason
                    .clone()
                    .unwrap_or_else(|| format!("« {tool} » a été explicitement retiré")),
            ),
            None => Verdict::Refuse(format!("aucune autorisation pour « {tool} »")),
        }
    }

    pub fn nodes(&self, agent: &str) -> Result<Vec<Node>, String> {
        self.grants.iter().map(|grant| grant.node(agent)).collect()
    }

    pub fn from_nodes(nodes: &[Node]) -> Result<Self, String> {
        let mut grants = Grants::new();
        for node in nodes.iter().filter(|node| node.label == GRANT) {
            let data = node.get("_data").ok_or("autorisation sans _data")?;
            let grant: Grant =
                serde_json::from_str(data).map_err(|e| format!("autorisation illisible: {e}"))?;
            grants.put(grant);
        }
        Ok(grants)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn section(id: &str, priority: u8, at_ms: u64, body: char, len: usize) -> Section {
        Section {
            id: id.into(),
            kind: SectionKind::Fact,
            title: format!("titre {id}"),
            body: std::iter::repeat_n(body, len).collect(),
            priority,
            at_ms,
            source: "test".into(),
        }
    }

    #[test]
    fn le_brief_passe_avant_tout_et_la_priorite_decide_ensuite() {
        let mut ctx = Context::new("atelier", "tu es l'atelier");
        ctx.put(section("basse", 200, 2, 'b', 10));
        ctx.put(section("haute", 10, 1, 'h', 10));
        let out = ctx.render(10_000);
        assert_eq!(
            out.kept,
            ["haute", "basse"],
            "priorité d'abord, puis le reste"
        );
        assert!(out.text.starts_with("tu es l'atelier"));
    }

    #[test]
    fn a_priori_te_egale_le_plus_recent_gagne() {
        let mut ctx = Context::new("a", "base");
        ctx.put(section("vieux", 50, 1_000, 'v', 5));
        ctx.put(section("neuf", 50, 2_000, 'n', 5));
        assert_eq!(ctx.render(10_000).kept, ["neuf", "vieux"]);
    }

    #[test]
    fn une_grosse_section_n_empeche_pas_les_suivantes_d_entrer() {
        // Le défaut que ce test interdit : s'arrêter au premier qui ne tient pas.
        let mut ctx = Context::new("a", "base");
        ctx.put(section("enorme", 0, 0, 'x', 5_000));
        ctx.put(section("petite", 255, 0, 'p', 5));
        let out = ctx.render(1_000);
        assert_eq!(out.kept, ["petite"], "la petite tient, l'énorme non");
        assert_eq!(out.dropped, ["enorme"]);
    }

    #[test]
    fn une_coupe_se_dit_et_nomme_ce_qui_manque() {
        let mut ctx = Context::new("a", "base");
        ctx.put(section("gardee", 1, 0, 'g', 10));
        ctx.put(section("perdue", 200, 0, 'p', 400));
        let out = ctx.render(100);
        assert!(!out.dropped.is_empty());
        assert!(
            out.text.contains("perdue"),
            "le texte doit nommer la section omise, pas seulement la compter"
        );
        assert!(out.text.contains("omise"));
    }

    #[test]
    fn un_outil_non_autorise_est_refuse_et_le_dit() {
        let mut grants = Grants::new();
        grants.put(Grant::granted("install_module", "user"));
        assert!(grants.allows("install_module").is_allowed());

        // Fail-closed : inconnu = refusé, avec le nom de l'outil.
        match grants.allows("rm_rf") {
            Verdict::Refuse(raison) => assert!(raison.contains("rm_rf")),
            Verdict::Allow => panic!("un outil inconnu ne doit jamais passer"),
        }
    }

    #[test]
    fn un_retrait_explicite_porte_sa_raison() {
        let mut grants = Grants::new();
        grants.put(Grant::refused(
            "run_program",
            "descriptor",
            "module non signé",
        ));
        match grants.allows("run_program") {
            Verdict::Refuse(raison) => assert_eq!(raison, "module non signé"),
            Verdict::Allow => panic!("un retrait explicite doit tenir"),
        }
    }

    #[test]
    fn les_sections_se_relisent_depuis_leurs_noeuds() {
        let mut ctx = Context::new("atelier", "base");
        ctx.put(section("s1", 10, 1_000, 'a', 20));
        ctx.put(section("s2", 20, 2_000, 'b', 20));
        let nodes = ctx.nodes().unwrap();
        let back = Context::from_nodes("atelier", "base", &nodes).unwrap();
        let mut ids: Vec<&str> = back.sections.iter().map(|s| s.id.as_str()).collect();
        ids.sort();
        assert_eq!(ids, ["s1", "s2"]);
    }

    #[test]
    fn deux_agents_peuvent_avoir_la_meme_section_sans_se_marcher_dessus() {
        let mut a = Context::new("atelier", "base");
        a.put(section("s1", 10, 1_000, 'a', 20));
        let mut b = Context::new("autre", "base");
        b.put(section("s1", 10, 1_000, 'a', 20));
        assert_ne!(
            a.nodes().unwrap()[0].get("_uuid"),
            b.nodes().unwrap()[0].get("_uuid"),
            "l'identité d'une section inclut son agent"
        );
    }

    #[test]
    fn les_autorisations_se_relisent_depuis_leurs_noeuds() {
        let mut grants = Grants::new();
        grants.put(Grant::granted("prompt_agent", "user"));
        grants.put(Grant::refused(
            "index_repository",
            "descriptor",
            "lecture seule",
        ));
        let nodes = grants.nodes("atelier").unwrap();
        let back = Grants::from_nodes(&nodes).unwrap();
        assert!(back.allows("prompt_agent").is_allowed());
        assert!(!back.allows("index_repository").is_allowed());
        assert!(!back.allows("inconnu").is_allowed());
    }
}
