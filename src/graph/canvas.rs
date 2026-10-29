//! **L'interface et les droits, dans la même langue.**
//!
//! # La découverte
//!
//! Un Canvas — l'état d'interface d'une application — décrit ce qu'il montre par
//! une **échelle de descente** (`cosmos`, `monde`, `groupe`, `noeud`) et par un
//! **sujet** (l'élément qu'il regarde). La lecture de graphe, elle, demandait une
//! profondeur et un identifiant. Les deux parlaient de la même chose sans le
//! savoir.
//!
//! Ce module les relie : quand un appelant demande `?echelle=canvas`, la lecture
//! prend **l'échelle de son propre état d'interface**, et `self` comme identifiant
//! prend **son propre sujet**. L'interface n'a plus à traduire : ce qu'un humain
//! voit à l'écran est ce que la lecture demande, et ce que ses droits permettent.
//!
//! # Une couture, pas une dépendance
//!
//! Le framework ne connaît **pas** l'implémentation de l'état d'interface : il
//! déclare [`CanvasState`], et l'application l'implémente — c'est ce qui permet à
//! un autre agent de faire évoluer l'interface sans que ce module change, et
//! inversement. Une implémentation en mémoire est fournie pour les tests et les
//! exemples.
//!
//! # Et les droits plafonnent
//!
//! L'échelle demandée par l'interface n'est pas un droit : elle est **plafonnée**
//! par le périmètre du demandeur. Un Canvas qui demande `noeud` alors que la
//! décision IAM s'arrête à `groupe` reçoit un refus, pas une vue tronquée en
//! silence — la même règle qu'ailleurs : jamais de coupe muette.

use std::collections::HashMap;
use std::sync::RwLock;

use crate::storage::permission::Scale;

/// **Ce qu'une lecture de graphe a besoin de savoir de l'état d'interface.**
///
/// Deux questions, pas plus : *à quelle échelle l'utilisateur regarde-t-il* et
/// *que regarde-t-il*. Un état d'interface qui répond à ces deux questions se
/// branche sans que le framework ait à connaître sa forme.
pub trait CanvasState: Send + Sync {
    /// **L'échelle déclarée** pour cette portée, si l'interface en déclare une.
    ///
    /// Une échelle inconnue ou absente rend `None` : la lecture retombe alors sur
    /// ses propres défauts, plutôt que d'inventer une intention.
    fn scale_of(&self, portee: &str) -> Option<Scale>;

    /// **Le sujet regardé** : `(store, qualified_name)`, tels que l'interface les
    /// nomme.
    fn subject_of(&self, portee: &str) -> Option<(String, String)>;
}

/// Ce qu'un état d'interface déclare d'une portée.
///
/// Une structure plutôt qu'un tuple imbriqué : `(Option<Scale>, Option<(String,
/// String)>)` se lit mal et se confond avec n'importe quoi d'autre.
#[derive(Debug, Default, Clone)]
struct Declaration {
    /// L'échelle déclarée, si l'interface en affiche une.
    echelle: Option<Scale>,
    /// Le sujet regardé, si l'interface en désigne un.
    sujet: Option<(String, String)>,
}

/// Une implémentation **en mémoire** : de quoi tester et démontrer la boucle
/// complète sans interface réelle.
#[derive(Debug, Default)]
pub struct SimpleCanvasState {
    etats: RwLock<HashMap<String, Declaration>>,
}

impl SimpleCanvasState {
    /// Un état d'interface vide.
    pub fn new() -> Self {
        Self::default()
    }

    /// Déclare l'échelle d'une portée.
    pub fn set_scale(&self, portee: &str, scale: Scale) {
        let mut etats = self.etats.write().expect("verrou");
        etats.entry(portee.to_string()).or_default().echelle = Some(scale);
    }

    /// Déclare le sujet d'une portée.
    pub fn set_subject(&self, portee: &str, store: &str, qn: &str) {
        let mut etats = self.etats.write().expect("verrou");
        etats.entry(portee.to_string()).or_default().sujet =
            Some((store.to_string(), qn.to_string()));
    }
}

impl CanvasState for SimpleCanvasState {
    fn scale_of(&self, portee: &str) -> Option<Scale> {
        self.etats
            .read()
            .expect("verrou")
            .get(portee)
            .and_then(|d| d.echelle)
    }

    fn subject_of(&self, portee: &str) -> Option<(String, String)> {
        self.etats
            .read()
            .expect("verrou")
            .get(portee)
            .and_then(|d| d.sujet.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Un état d'interface qui ne déclare rien n'invente rien.
    #[test]
    fn an_empty_canvas_declares_nothing() {
        let canvas = SimpleCanvasState::new();
        assert_eq!(canvas.scale_of("portee"), None);
        assert_eq!(canvas.subject_of("portee"), None);
    }

    /// L'échelle et le sujet d'**une** portée ne concernent pas les autres : ce
    /// qu'un utilisateur regarde n'est pas ce qu'un autre regarde.
    #[test]
    fn each_portee_has_its_own_view() {
        let canvas = SimpleCanvasState::new();
        canvas.set_scale("moi", Scale::Groupe);
        canvas.set_subject("moi", "xth", "aletheia/mathematiques");

        assert_eq!(canvas.scale_of("moi"), Some(Scale::Groupe));
        assert_eq!(
            canvas.subject_of("moi"),
            Some(("xth".to_string(), "aletheia/mathematiques".to_string()))
        );
        assert_eq!(canvas.scale_of("un-autre"), None);
        assert_eq!(canvas.subject_of("un-autre"), None);
    }
}
