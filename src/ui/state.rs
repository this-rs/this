//! Le document d'état d'interface et son store.

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use tokio::sync::RwLock;

/// Identifiant de schéma du document — **versionné**, comme `RELIEF_SCHEMA`.
///
/// Un client qui lit ce document doit pouvoir constater à quelle forme il a
/// affaire ; sans numéro, une évolution de champ se découvre par un échec de
/// désérialisation chez l'appelant, c'est-à-dire trop tard et sans explication.
pub const UI_SCHEMA: &str = "this-rs/ui-state/1";

/// Le sujet visé, tel que l'interface le partage entre ses faces.
///
/// Les quatre champs viennent des atoms du tableau de bord (`subjectAtom`) :
/// une base, une entité, son libellé, et par quelle face le sujet a été posé
/// (traçabilité, jamais une priorité).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct UiSubject {
    #[serde(default)]
    pub store: String,
    #[serde(default)]
    pub qn: String,
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub via: String,
}

/// L'état d'interface d'une portée.
///
/// `#[serde(default)]` partout : un client d'une version antérieure doit pouvoir
/// écrire un document partiel sans être refusé, et un champ ajouté plus tard doit
/// se lire comme absent (défaut) plutôt que comme une erreur.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct UiState {
    /// Face courante : `cosmos`, `monde`, `graphe`.
    #[serde(default)]
    pub vue: String,
    /// Échelle de descente : `cosmos`, `monde`, `groupe`, `noeud`.
    #[serde(default)]
    pub echelle: String,
    /// Visibilité des éléments de chrome, par clé (`navigation`, `commandes`…).
    ///
    /// `BTreeMap` et non `HashMap` : l'ordre est stable dans les réponses, donc
    /// deux lectures successives du même document sont comparables à l'œil — ce
    /// qui n'est pas le cas d'une table de hachage.
    #[serde(default)]
    pub panneaux: BTreeMap<String, bool>,
    #[serde(default)]
    pub sujet: Option<UiSubject>,
}

impl Default for UiState {
    fn default() -> Self {
        // Le défaut est un état LISIBLE et cohérent, jamais vide : une interface
        // qui reçoit `{}` doit pouvoir s'afficher telle quelle.
        let mut panneaux = BTreeMap::new();
        panneaux.insert("navigation".to_string(), true);
        panneaux.insert("commandes".to_string(), true);
        panneaux.insert("reglages".to_string(), false);
        panneaux.insert("fil".to_string(), true);
        panneaux.insert("zoom".to_string(), true);
        Self {
            vue: "graphe".to_string(),
            echelle: "cosmos".to_string(),
            panneaux,
            sujet: None,
        }
    }
}

/// Store d'états d'interface, une entrée par portée.
#[derive(Debug)]
pub struct UiStateStore {
    etats: RwLock<HashMap<String, UiState>>,
}

impl UiStateStore {
    pub fn new() -> Self {
        Self {
            etats: RwLock::new(HashMap::new()),
        }
    }

    /// L'état d'une portée — le DÉFAUT si elle est inconnue, jamais une erreur.
    ///
    /// Une portée qui n'a rien écrit n'est pas une panne : c'est un client qui
    /// ouvre l'application pour la première fois. Rendre une erreur obligerait
    /// chaque appelant à traiter ce cas, et le premier qui l'oublierait
    /// afficherait un vide inexplicable.
    pub async fn get(&self, portee: &str) -> UiState {
        self.etats
            .read()
            .await
            .get(portee)
            .cloned()
            .unwrap_or_default()
    }

    /// Remplace l'état d'une portée et rend ce qui a été écrit.
    pub async fn update(&self, portee: impl Into<String>, etat: UiState) -> UiState {
        let mut etats = self.etats.write().await;
        etats.insert(portee.into(), etat.clone());
        etat
    }

    /// Nombre de portées connues (diagnostic : un store qui grossit sans fin se
    /// voit ici avant de devenir un problème de mémoire).
    pub async fn len(&self) -> usize {
        self.etats.read().await.len()
    }

    pub async fn is_empty(&self) -> bool {
        self.etats.read().await.is_empty()
    }
}

impl Default for UiStateStore {
    fn default() -> Self {
        Self::new()
    }
}
