//! Thèmes : la table qui associe un nom de thème à un [`Profil`] d'accès.
//!
//! Un « thème » est le nom sous lequel une application offre un espace (famille, travail,
//! amis…). Cette table est le seul endroit où un nom de thème devient un ensemble de
//! droits. Un thème inconnu ne porte **aucun** droit : deny par défaut, en Rust pur.

use crate::profil::Profil;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// La table des profils d'accès, indexée par nom de thème.
///
/// `BTreeMap` plutôt qu'un `HashMap` : l'ordre est stable, donc la sérialisation et les
/// tests le sont aussi.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Themes {
    profils: BTreeMap<String, Profil>,
}

impl Themes {
    /// Une table vide : aucun thème, donc aucun droit.
    pub fn nouveau() -> Self {
        Self::default()
    }

    /// Associe `profil` au thème `theme`, en remplaçant le profil déjà associé s'il existe.
    pub fn definir(&mut self, theme: &str, profil: Profil) {
        self.profils.insert(theme.to_string(), profil);
    }

    /// Le profil associé à `theme`, ou `None` si le thème est inconnu.
    pub fn profil_de(&self, theme: &str) -> Option<&Profil> {
        self.profils.get(theme)
    }

    /// Vrai si le thème `theme` accorde `(service, operation)`.
    ///
    /// Deny par défaut, à deux étages : thème inconnu → `false` ; thème connu mais aucun
    /// grant du profil ne couvre la demande → `false`.
    pub fn autorise(&self, theme: &str, service: &str, operation: &str) -> bool {
        match self.profil_de(theme) {
            Some(profil) => profil.autorise(service, operation),
            None => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::profil::{Grant, JOKER, Portee};

    /// Un profil « famille » qui accorde `liste/lire` et rien d'autre.
    fn profil_famille() -> Profil {
        Profil::nouveau(
            "famille",
            vec![Grant {
                service: "liste".to_string(),
                operation: "lire".to_string(),
                portee: Portee::Nommee("famille".to_string()),
                quota: Some(500),
            }],
        )
    }

    #[test]
    fn un_theme_connu_autorise_ce_que_son_profil_accorde() {
        let mut themes = Themes::nouveau();
        themes.definir("famille", profil_famille());

        assert!(themes.autorise("famille", "liste", "lire"));
        assert_eq!(
            themes.profil_de("famille").map(|p| p.nom.as_str()),
            Some("famille")
        );
    }

    #[test]
    fn un_theme_connu_refuse_ce_que_son_profil_n_accorde_pas() {
        let mut themes = Themes::nouveau();
        themes.definir("famille", profil_famille());

        assert!(!themes.autorise("famille", "liste", "vider"));
        assert!(!themes.autorise("famille", "photos", "lire"));
    }

    #[test]
    fn un_theme_inconnu_refuse_tout() {
        let mut themes = Themes::default();
        themes.definir("famille", profil_famille());

        assert!(!themes.autorise("amis", "liste", "lire"));
        assert!(!themes.autorise("amis", JOKER, JOKER));
        assert_eq!(themes.profil_de("amis"), None);
    }

    #[test]
    fn une_table_vide_refuse_tout() {
        let themes = Themes::nouveau();
        assert!(!themes.autorise("famille", "liste", "lire"));
    }

    #[test]
    fn definir_remplace_le_profil_du_meme_theme() {
        let mut themes = Themes::nouveau();
        themes.definir("famille", profil_famille());
        themes.definir(
            "famille",
            Profil::nouveau(
                "famille-v2",
                vec![Grant {
                    service: JOKER.to_string(),
                    operation: JOKER.to_string(),
                    portee: Portee::Tout,
                    quota: None,
                }],
            ),
        );

        assert_eq!(
            themes.profil_de("famille").map(|p| p.nom.as_str()),
            Some("famille-v2")
        );
        assert!(themes.autorise("famille", "liste", "vider"));
    }
}
