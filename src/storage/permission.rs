//! **Les permissions granulaires** — ce qu'un principal peut lire, écrire, et
//! surtout **tisser**.
//!
//! # Pourquoi une grammaire, et pas des chaînes libres
//!
//! Une permission `orders:write` dit « écrire des commandes » : utile, mais
//! grossier. Ce qu'une base de connaissance fait, c'est **tisser** : relier des
//! nœuds par des relations typées, dans des espaces nommés, sur des labels
//! précis. Donner ce droit « en bloc » oblige à choisir entre trop et trop peu.
//!
//! Une permission est donc **structurée** :
//!
//! ```text
//! famille ":" action [ ":" cible ]
//! ```
//!
//! | Permission | Ce qu'elle accorde |
//! |---|---|
//! | `graph:read` | lire le relief, la vue multi-échelle, l'identité |
//! | `node:read:Page` | lire les nœuds du label `Page` |
//! | `node:read:*` | lire les nœuds, quel que soit le label |
//! | `node:write:Lemma` | créer ou modifier des `Lemma` |
//! | `node:delete:*` | supprimer des nœuds |
//! | `edge:read:CITES` | lire les relations `CITES` |
//! | **`edge:create:CITES`** | **tisser** : créer des relations `CITES` |
//! | `edge:delete:*` | défaire des relations |
//! | `space:use:lab` | opérer dans l'espace nommé `lab` |
//! | `node:*` | toutes les actions de la famille `node` |
//! | `*` | tout (compatibilité avec le vocabulaire d'avant) |
//!
//! # La règle de correspondance, en une phrase
//!
//! Une permission **détenue** accorde une permission **demandée** si les
//! familles et les actions sont égales, et si la cible détenue est `*`, ou égale
//! à la cible demandée. Un `*` détenu accorde tout ; `famille:*` accorde toutes
//! les actions de cette famille.
//!
//! Ce qui n'est pas reconnu **n'accorde rien** : une permission mal orthographiée
//! est refusée, jamais interprétée au plus large.

use serde::{Deserialize, Serialize};

/// Les familles reconnues.
///
/// `echelle` (alias `scale`) vient de l'**interface** : un Canvas décrit ce qu'il
/// montre par une échelle de descente — `cosmos`, `monde`, `groupe`, `noeud`.
/// Donner un droit dans le vocabulaire de l'interface évite de tenir deux langues
/// pour la même chose.
pub const FAMILIES: [&str; 6] = ["graph", "node", "edge", "space", "echelle", "orders"];

/// **Les échelles de descente** — le vocabulaire de l'interface, devenu un droit.
///
/// | Échelle | Ce qu'elle montre |
/// |---|---|
/// | `cosmos` | l'ensemble, agrégé : des comptes, pas des nœuds |
/// | `monde` | le graphe entier |
/// | `groupe` | un groupe et ses sous-groupes |
/// | `noeud` | un nœud et son voisinage |
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Scale {
    /// L'ensemble, agrégé.
    Cosmos,
    /// Le graphe entier.
    Monde,
    /// Un groupe et ses sous-groupes.
    Groupe,
    /// Un nœud et son voisinage.
    Noeud,
}

impl Scale {
    /// Toutes les échelles, de la plus large à la plus fine.
    pub const ALL: [Scale; 4] = [Scale::Cosmos, Scale::Monde, Scale::Groupe, Scale::Noeud];

    /// Analyse une échelle, en acceptant les deux orthographes.
    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "cosmos" => Some(Scale::Cosmos),
            "monde" | "world" => Some(Scale::Monde),
            "groupe" | "group" => Some(Scale::Groupe),
            "noeud" | "nœud" | "node" => Some(Scale::Noeud),
            _ => None,
        }
    }

    /// Le nom publié.
    pub fn as_str(&self) -> &'static str {
        match self {
            Scale::Cosmos => "cosmos",
            Scale::Monde => "monde",
            Scale::Groupe => "groupe",
            Scale::Noeud => "noeud",
        }
    }

    /// Ce que cette échelle montre, en une phrase (pour un modèle).
    pub fn meaning(&self) -> &'static str {
        match self {
            Scale::Cosmos => "l'ensemble, agrégé : des comptes, pas des nœuds",
            Scale::Monde => "le graphe entier",
            Scale::Groupe => "un groupe et ses sous-groupes",
            Scale::Noeud => "un nœud et son voisinage",
        }
    }

    /// **La profondeur de descente correspondante**, et le plafond par niveau.
    ///
    /// Une échelle est une **intention**, une profondeur un **nombre** : ce
    /// tableau est le seul endroit qui traduit l'une en l'autre.
    pub fn descent(&self) -> (usize, usize) {
        match self {
            Scale::Cosmos => (1, 20),
            Scale::Monde => (3, 50),
            Scale::Groupe => (4, 100),
            Scale::Noeud => (1, 200),
        }
    }
}

/// Les actions reconnues, par famille.
///
/// `orders` est la famille historique du vocabulaire d'entité (`orders:write`) :
/// elle reste valide, sans cible.
pub fn known_action(family: &str, action: &str) -> bool {
    match family {
        "graph" => matches!(action, "read" | "write"),
        "node" => matches!(action, "read" | "write" | "delete"),
        // **Le tissage** : lire, créer, défaire une relation.
        "edge" => matches!(action, "read" | "create" | "delete"),
        "space" => matches!(action, "use" | "create" | "drop"),
        "echelle" | "scale" => matches!(action, "read"),
        // Familles libres du vocabulaire d'entité (`orders:write`) : on accepte
        // l'action telle quelle, la cible restant interdite.
        _ => !action.is_empty(),
    }
}

/// Une permission structurée.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Permission {
    /// La famille (`graph`, `node`, `edge`, `space`, ou une famille d'entité).
    pub family: String,
    /// L'action (`read`, `write`, `create`, `delete`, `use`…).
    pub action: String,
    /// La cible : un label, un type d'arête, un espace, un identifiant… ou `*`.
    pub target: Option<String>,
}

impl Permission {
    /// Analyse une permission. Rend `None` si elle n'est pas structurée ainsi —
    /// un appelant qui ne sait pas la lire ne doit **pas** l'accorder.
    pub fn parse(raw: &str) -> Option<Self> {
        let raw = raw.trim();
        if raw.is_empty() {
            return None;
        }
        if raw == "*" {
            return Some(Self {
                family: "*".to_string(),
                action: "*".to_string(),
                target: None,
            });
        }
        let mut parts = raw.split(':');
        let family = parts.next()?.trim();
        let action = parts.next()?.trim();
        if family.is_empty() || action.is_empty() {
            return None;
        }
        let target = parts.next().map(|t| t.trim().to_string());
        // Plus de trois segments : refusé (une cible contenant `:` est un
        // identifiant qualifié — on la recolle plutôt que de la perdre).
        let target = match (target, parts.next()) {
            (None, None) => None,
            (Some(t), None) => Some(t),
            (Some(t), Some(rest)) => Some(format!("{t}:{rest}")),
            (None, Some(_)) => return None,
        };
        if target.as_deref() == Some("") {
            return None;
        }
        Some(Self {
            family: family.to_string(),
            action: action.to_string(),
            target,
        })
    }

    /// Cette permission **détenue** accorde-t-elle `wanted` ?
    pub fn allows(&self, wanted: &Permission) -> bool {
        // `*` détenu : tout.
        if self.family == "*" && self.action == "*" {
            return true;
        }
        // Les familles sont comparées **exactement** : le cas `*`/`*` ci-dessus est
        // le seul qui accorde toutes les familles. La condition redondante qui
        // figurait ici — signalée par clippy — trahissait une intention floue :
        // il n'existe pas de « famille universelle avec une action précise ».
        if self.family != wanted.family {
            return false;
        }
        let action_ok = self.action == "*" || self.action == wanted.action;
        if !action_ok {
            return false;
        }
        match (&self.target, &wanted.target) {
            // Une permission sans cible ne couvre que les demandes sans cible :
            // `node:read` n'accorde pas `node:read:Page`.
            (None, None) => true,
            (Some(held), wanted) if held == "*" => true,
            (Some(held), Some(wanted)) => held == wanted,
            _ => false,
        }
    }

    /// La forme canonique.
    pub fn to_key(&self) -> String {
        match &self.target {
            Some(target) => format!("{}:{}:{}", self.family, self.action, target),
            None => format!("{}:{}", self.family, self.action),
        }
    }
}

/// **Ce qu'un principal a le droit de faire** — construit depuis les permissions
/// de sa décision IAM.
///
/// C'est la pièce qui donne une vraie granularité : au lieu de « peut lire le
/// graphe », on porte une **liste** de permissions structurées, et chaque lecture
/// demande « ai-je le droit sur *ce* label, *ce* type d'arête, *cet* espace ».
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReadScope {
    permissions: Vec<Permission>,
}

impl ReadScope {
    /// Un périmètre qui **n'accorde rien** : le défaut d'un principal sans
    /// décision.
    pub fn none() -> Self {
        Self::default()
    }

    /// Un périmètre qui **accorde tout** — réservé à la compatibilité (un
    /// appelant qui n'a pas de décision, ou une lecture hors IAM).
    pub fn all() -> Self {
        Self {
            permissions: vec![Permission {
                family: "*".to_string(),
                action: "*".to_string(),
                target: None,
            }],
        }
    }

    /// Construit le périmètre depuis des permissions brutes (celles d'une
    /// décision IAM).
    pub fn from_permissions<'a>(raw: impl IntoIterator<Item = &'a str>) -> Self {
        Self {
            permissions: raw.into_iter().filter_map(Permission::parse).collect(),
        }
    }

    /// Combien de permissions lisibles.
    pub fn len(&self) -> usize {
        self.permissions.len()
    }

    /// Aucune permission ?
    pub fn is_empty(&self) -> bool {
        self.permissions.is_empty()
    }

    /// Le périmètre accorde-t-il tout ?
    pub fn is_unrestricted(&self) -> bool {
        self.permissions
            .iter()
            .any(|p| p.family == "*" && p.action == "*")
    }

    /// **Le droit de lire des nœuds** d'un label donné.
    pub fn may_read_node(&self, label: &str) -> bool {
        self.may("node", "read", Some(label))
    }

    /// **Le droit d'écrire** des nœuds d'un label.
    pub fn may_write_node(&self, label: &str) -> bool {
        self.may("node", "write", Some(label))
    }

    /// **Le droit de lire des relations** d'un type (le tissage, en lecture).
    pub fn may_read_edge(&self, edge_type: &str) -> bool {
        self.may("edge", "read", Some(edge_type))
    }

    /// **Le droit de TISSER** : créer une relation de ce type.
    pub fn may_create_edge(&self, edge_type: &str) -> bool {
        self.may("edge", "create", Some(edge_type))
    }

    /// Le droit d'opérer dans un espace nommé.
    pub fn may_use_space(&self, space: &str) -> bool {
        self.may("space", "use", Some(space))
    }

    /// **Le droit de lire à une échelle** — le vocabulaire de l'interface.
    pub fn may_read_scale(&self, scale: Scale) -> bool {
        // `echelle` et `scale` sont la même famille : on accepte les deux
        // orthographes plutôt que d'imposer la nôtre.
        self.may("echelle", "read", Some(scale.as_str()))
            || self.may("scale", "read", Some(scale.as_str()))
    }

    /// **Le plafond d'échelle** : ce périmètre permet-il de lire *jusqu'à*
    /// cette échelle ?
    ///
    /// Les échelles vont du plus large au plus fin (`cosmos` < `monde` <
    /// `groupe` < `noeud`). Un droit sur `echelle:read:groupe` accorde donc le
    /// cosmos, le monde et le groupe — mais **pas** le détail d'un nœud : on
    /// donne « jusqu'où » on peut descendre, pas un cran isolé.
    pub fn allows_scale(&self, scale: Scale) -> bool {
        if self.is_unrestricted() {
            return true;
        }
        self.finest_scale().is_some_and(|finest| scale <= finest)
    }

    /// L'échelle la plus fine que ce périmètre autorise.
    pub fn finest_scale(&self) -> Option<Scale> {
        Scale::ALL
            .iter()
            .copied()
            .filter(|s| self.may_read_scale(*s))
            .max()
    }

    /// Le droit de lire le relief de la base.
    pub fn may_read_graph(&self) -> bool {
        self.may("graph", "read", None)
    }

    fn may(&self, family: &str, action: &str, target: Option<&str>) -> bool {
        let wanted = Permission {
            family: family.to_string(),
            action: action.to_string(),
            target: target.map(str::to_string),
        };
        self.permissions.iter().any(|held| held.allows(&wanted))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scope(raw: &[&str]) -> ReadScope {
        ReadScope::from_permissions(raw.iter().copied())
    }

    #[test]
    fn a_permission_is_parsed_into_family_action_target() {
        let p = Permission::parse("node:read:Page").expect("lisible");
        assert_eq!(p.family, "node");
        assert_eq!(p.action, "read");
        assert_eq!(p.target.as_deref(), Some("Page"));
        assert_eq!(p.to_key(), "node:read:Page");

        let bare = Permission::parse("graph:read").expect("lisible");
        assert_eq!(bare.target, None);

        // Une cible qualifiée garde ses `:`.
        let qualified = Permission::parse("node:read:Label:sub").expect("lisible");
        assert_eq!(qualified.target.as_deref(), Some("Label:sub"));
    }

    /// Ce qui n'est pas structuré **n'accorde rien** : une faute de frappe ne
    /// doit jamais devenir un droit.
    #[test]
    fn a_malformed_permission_grants_nothing() {
        for bad in ["", "   ", "node", ":read", "node:", "node:read:"] {
            assert!(
                Permission::parse(bad).is_none(),
                "`{bad}` doit être refusée, jamais interprétée"
            );
            assert_eq!(
                scope(&[bad]).len(),
                0,
                "`{bad}` ne doit pas produire de droit"
            );
        }
    }

    #[test]
    fn the_matching_rule_is_exact_and_wildcards_are_explicit() {
        // Exact.
        let s = scope(&["node:read:Page"]);
        assert!(s.may_read_node("Page"));
        assert!(
            !s.may_read_node("Lemma"),
            "un autre label n'est pas couvert"
        );

        // Cible `*` : tous les labels, cette action-là seulement.
        let s = scope(&["node:read:*"]);
        assert!(s.may_read_node("Page") && s.may_read_node("Lemma"));
        assert!(!s.may_write_node("Page"), "l'écriture n'est pas accordée");

        // `famille:*` : toutes les actions de la famille.
        let s = scope(&["node:*:Page"]);
        assert!(s.may_read_node("Page") && s.may_write_node("Page"));
        assert!(!s.may_read_edge("CITES"), "une autre famille reste fermée");

        // `*` : tout.
        let s = ReadScope::all();
        assert!(s.is_unrestricted() && s.may_read_node("N'importe") && s.may_create_edge("X"));
    }

    /// **Le tissage se donne séparément de la lecture** : c'est le cœur de la
    /// granularité demandée.
    #[test]
    fn weaving_is_a_permission_of_its_own() {
        let reader = scope(&["graph:read", "node:read:*", "edge:read:CITES"]);
        assert!(reader.may_read_graph());
        assert!(reader.may_read_node("Page"));
        assert!(reader.may_read_edge("CITES"));
        assert!(
            !reader.may_create_edge("CITES"),
            "lire les relations n'est PAS le droit de les tisser"
        );
        assert!(
            !reader.may_read_edge("CONTAINS"),
            "un autre type reste fermé"
        );

        let weaver = scope(&["edge:create:CITES"]);
        assert!(weaver.may_create_edge("CITES"));
        assert!(
            !weaver.may_read_node("Page"),
            "tisser une relation ne donne pas le droit de lire les nœuds"
        );
    }

    /// Une permission **sans cible** ne couvre pas les demandes **avec** cible :
    /// `node:read` est plus étroit que `node:read:*`, et il faut le dire.
    #[test]
    fn a_permission_without_a_target_does_not_cover_targeted_reads() {
        let s = scope(&["node:read"]);
        assert!(
            !s.may_read_node("Page"),
            "`node:read` ne dit pas QUEL label : il ne peut pas l'accorder"
        );
    }

    /// Un périmètre sans permission n'accorde rien — le défaut d'un principal
    /// sans décision est le refus.
    #[test]
    fn an_empty_scope_grants_nothing() {
        let s = ReadScope::none();
        assert!(s.is_empty() && !s.is_unrestricted());
        assert!(!s.may_read_graph() && !s.may_read_node("Page") && !s.may_create_edge("CITES"));
    }

    /// **Le vocabulaire du Canvas devient un droit** : on donne *jusqu'où* un
    /// principal peut descendre, et cette intention se traduit en profondeur.
    #[test]
    fn the_canvas_scales_are_rights_and_carry_an_intention() {
        // Les quatre échelles de l'interface, et leur sens.
        for (raw, scale) in [
            ("cosmos", Scale::Cosmos),
            ("monde", Scale::Monde),
            ("groupe", Scale::Groupe),
            ("noeud", Scale::Noeud),
        ] {
            assert_eq!(Scale::parse(raw), Some(scale));
            assert!(
                !scale.meaning().is_empty(),
                "chaque échelle dit ce qu'elle montre"
            );
        }
        assert_eq!(Scale::parse("group"), Some(Scale::Groupe), "alias anglais");
        assert_eq!(
            Scale::parse("n'importe"),
            None,
            "une échelle inconnue est refusée"
        );

        // Un droit sur `groupe` : on voit jusqu'au groupe, pas le détail d'un nœud.
        let s = scope(&["echelle:read:groupe"]);
        assert!(
            s.allows_scale(Scale::Cosmos),
            "le plus large est toujours permis"
        );
        assert!(s.allows_scale(Scale::Monde));
        assert!(s.allows_scale(Scale::Groupe));
        assert!(
            !s.allows_scale(Scale::Noeud),
            "le détail d'un nœud n'est pas accordé par un droit sur le groupe"
        );
        assert_eq!(s.finest_scale(), Some(Scale::Groupe));

        // Un droit sur `noeud` donne tout ce qui est plus large.
        let s = scope(&["scale:read:noeud"]);
        assert!(s.allows_scale(Scale::Cosmos) && s.allows_scale(Scale::Noeud));

        // Sans droit d'échelle : rien n'est lisible à aucune échelle.
        let s = scope(&["node:read:Page"]);
        assert!(!s.allows_scale(Scale::Cosmos) && s.finest_scale().is_none());

        // Et une échelle se traduit en profondeur — le seul endroit qui le fait.
        assert_eq!(Scale::Groupe.descent(), (4, 100));
        assert!(Scale::Noeud.descent().1 > Scale::Cosmos.descent().1);
    }

    #[test]
    fn family_and_action_names_are_checked() {
        assert!(known_action("node", "read"));
        assert!(known_action("edge", "create"));
        assert!(known_action("graph", "read"));
        assert!(!known_action("edge", "weave"), "l'action doit exister");
        // Les familles libres (vocabulaire d'entité) restent acceptées.
        assert!(known_action("orders", "write"));
    }
}
