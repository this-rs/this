//! # Les vues : où un module veut être dessiné, et quoi
//!
//! Un descripteur disait des **données** (entités, liens, politiques) et du **chrome** lourd
//! (menu, accent). Il ne disait rien de ce que le module veut *montrer*, donc une app devait
//! coder chaque écran à la main, et un module ne pouvait pas placer sa propre interface.
//!
//! Une vue comble ce trou sans toucher au langage de module : elle est **déclarative**, comme
//! le reste. Le module nomme une entité et un emplacement ; l'hôte dessine. Rien à exécuter,
//! donc rien à confiance : les mêmes garanties que le reste du descripteur.
//!
//! ```json
//! { "name": "conversations", "kind": "list", "surface": "sidebar",
//!   "entity": "conversation", "label": "titre", "subtitle": "updated_at" }
//! ```
//!
//! Ce que la vue ne dit PAS : comment dessiner. Une liste reste une liste. C'est la limite
//! assumée — un module qui veut un écran sur mesure n'a pas de vue sur mesure, il a une vue
//! d'un genre connu, rendue par l'hôte avec son style.

use serde::{Deserialize, Serialize};

/// Où la vue se dessine. Le nom est celui de l'emplacement, pas d'un composant : un hôte
/// web, mobile ou desktop peut décider que `sidebar` est un tiroir, une colonne ou un onglet.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Surface {
    /// Le menu latéral, quand il y en a un.
    Sidebar,
    /// L'écran principal : ce qu'on voit en ouvrant le module.
    #[default]
    Main,
    /// Un panneau : à côté du contenu principal, ou par-dessus.
    Panel,
    /// L'en-tête : une barre fine, pour un compteur ou un titre.
    Header,
}

/// Ce que la vue dessine. Chaque genre a une forme de rendu connue de l'hôte.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ViewKind {
    /// Les lignes d'une entité, une par ligne, cliquables.
    #[default]
    List,
    /// Une ligne, en détail.
    Detail,
    /// Un formulaire de création ou d'édition.
    Form,
    /// Des nombres : compteurs, totaux.
    Metrics,
    /// Le fil d'une conversation : les messages dans l'ordre, et la saisie.
    Chat,
}

/// Une vue déclarée par un module.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ViewDesc {
    /// Identifiant de la vue dans le module (`conversations`, `invoice_detail`).
    pub name: String,
    #[serde(default)]
    pub kind: ViewKind,
    #[serde(default)]
    pub surface: Surface,
    /// L'entité lue. Obligatoire pour `list`, `detail` et `form` ; absente pour `chat` et
    /// `metrics`, qui savent d'où vient leur contenu.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub entity: Option<String>,
    /// Le titre de la section, quand l'hôte en affiche un.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Le champ qui sert de libellé à une ligne (un titre, un nom).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// Le champ secondaire : une date, un statut, un compte.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subtitle: Option<String>,
    /// Le texte quand il n'y a rien à montrer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub empty: Option<String>,
    /// Le nombre de lignes demandé. L'hôte peut le réduire, jamais l'augmenter au-delà.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
}

impl ViewDesc {
    /// Vrai quand la vue lit les lignes d'une entité : `list`, `detail`, `form`.
    pub fn needs_entity(&self) -> bool {
        matches!(
            self.kind,
            ViewKind::List | ViewKind::Detail | ViewKind::Form
        )
    }

    /// Les champs que la vue nomme, pour que l'hôte sache quoi lire.
    pub fn fields(&self) -> impl Iterator<Item = &str> {
        self.label
            .iter()
            .chain(self.subtitle.iter())
            .map(String::as_str)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_view_defaults_to_a_main_list() {
        let view: ViewDesc = serde_json::from_str(r#"{"name":"invoices"}"#).unwrap();
        assert_eq!(view.kind, ViewKind::List);
        assert_eq!(view.surface, Surface::Main);
    }

    #[test]
    fn a_sidebar_list_round_trips() {
        let json = r#"{"name":"conversations","kind":"list","surface":"sidebar",
                       "entity":"conversation","label":"titre","subtitle":"updated_at",
                       "title":"Conversations","empty":"Aucune conversation","limit":30}"#;
        let view: ViewDesc = serde_json::from_str(json).unwrap();
        assert_eq!(view.surface, Surface::Sidebar);
        assert_eq!(view.entity.as_deref(), Some("conversation"));
        assert_eq!(view.label.as_deref(), Some("titre"));
        assert_eq!(view.limit, Some(30));
        let back = serde_json::to_string(&view).unwrap();
        let again: ViewDesc = serde_json::from_str(&back).unwrap();
        assert_eq!(again, view);
    }

    #[test]
    fn only_the_kinds_that_read_rows_need_an_entity() {
        assert!(
            ViewDesc {
                name: "a".into(),
                kind: ViewKind::List,
                surface: Surface::Main,
                entity: None,
                title: None,
                label: None,
                subtitle: None,
                empty: None,
                limit: None,
            }
            .needs_entity()
        );
        assert!(
            !ViewDesc {
                name: "b".into(),
                kind: ViewKind::Chat,
                surface: Surface::Main,
                entity: None,
                title: None,
                label: None,
                subtitle: None,
                empty: None,
                limit: None,
            }
            .needs_entity()
        );
    }

    #[test]
    fn an_unknown_surface_is_refused_not_guessed() {
        // Un hôte qui ne connaît pas l'emplacement dessinerait n'importe où : on refuse.
        assert!(serde_json::from_str::<ViewDesc>(r#"{"name":"x","surface":"toolbar"}"#).is_err());
    }
}
