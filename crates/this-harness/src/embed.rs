//! # L'embedding — un port, à côté du rappel
//!
//! Le rappel ([`crate::Recall`]) est lexical : des ancres (`#kind:uuid`,
//! chemins, outils) et l'état du graphe. Ici, l'hôte peut **ajouter** un
//! espace vectoriel : le port [`Embed`] vectorise des textes, et le harnais
//! n'en fait rien lui-même — aucune entrée/sortie, comme partout ailleurs.
//!
//! **Règle : deux espaces vectoriels différents ne se mélangent pas.** Le port
//! déclare son estampille ([`EmbeddingModel`] : le modèle et sa dimension) et
//! le consommateur la compare : un vecteur d'une dimension autre que celle
//! annoncée est **refusé nommément**, jamais comparé en silence (un cosinus
//! entre deux espaces ne veut rien dire).
//!
//! Sans implémentation branchée, rien ne change : [`SansEmbed`] est le défaut,
//! et [`vectoriser`] rend [`EmbedError::Unsupported`] **sans appeler** `embed`
//! — le consommateur retombe sur le lexique seul, exactement comme avant.

use async_trait::async_trait;

/// L'estampille d'un espace vectoriel : le modèle qui produit les vecteurs et
/// leur dimension. Deux estampilles différentes = deux espaces incomparables :
/// on ne mélange jamais leurs vecteurs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EmbeddingModel {
    pub nom: String,
    pub dimension: usize,
}

/// La raison pour laquelle un embedding n'a pas eu lieu.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EmbedError {
    /// Aucune implémentation branchée : le port par défaut ne fait rien, et le
    /// consommateur retombe sur le lexique seul.
    Unsupported,
    /// La dimension rendue diffère de celle annoncée par l'estampille.
    Dimension { attendue: usize, recue: usize },
    /// L'implémentation a échoué (réseau, modèle…) : la raison est propagée,
    /// pas avalée.
    Failed(String),
}

impl std::fmt::Display for EmbedError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unsupported => f.write_str("embedding non branché : aucune implémentation"),
            Self::Dimension { attendue, recue } => write!(
                f,
                "l'implémentation a annoncé {attendue} dimension(s) mais a rendu un vecteur de {recue} dimension(s)"
            ),
            Self::Failed(raison) => write!(f, "embedding : {raison}"),
        }
    }
}

impl std::error::Error for EmbedError {}

/// Le port que l'hôte implémente : vectoriser des textes dans UN espace annoncé.
///
/// `?Send`, comme [`crate::Recall`] et les autres ports : le même trait doit
/// vivre dans une page (`!Send`) et dans une coquille native.
///
/// L'implémentation HTTP n'est **pas** ici : le crate n'ouvre aucune connexion.
/// L'hôte branche son transport (`reqwest` en natif, `fetch` en wasm) et rend
/// ce trait.
#[async_trait(?Send)]
pub trait Embed {
    /// L'estampille de l'espace : le modèle et sa dimension. `None` = pas
    /// branché ([`SansEmbed`]), et `embed` n'est alors jamais appelé.
    fn modele(&self) -> Option<&EmbeddingModel>;

    /// Vectorise un lot de textes : un vecteur par texte, dans l'ordre.
    async fn embed(&self, textes: &[String]) -> Result<Vec<Vec<f32>>, EmbedError>;
}

/// Le défaut : aucune implémentation. `embed` n'est jamais appelé (voir
/// [`vectoriser`], qui s'arrête à l'estampille absente) et, appelé de force, il
/// répond [`EmbedError::Unsupported`] sans toucher au réseau.
#[derive(Clone, Copy, Debug, Default)]
pub struct SansEmbed;

#[async_trait(?Send)]
impl Embed for SansEmbed {
    fn modele(&self) -> Option<&EmbeddingModel> {
        None
    }

    async fn embed(&self, _textes: &[String]) -> Result<Vec<Vec<f32>>, EmbedError> {
        Err(EmbedError::Unsupported)
    }
}

/// La règle de dimension, en fonction **pure** : chaque vecteur du lot doit avoir la
/// dimension annoncée par l'estampille.
///
/// Synchrone, sans `async` et sans réseau : elle vit donc aussi bien du côté d'une
/// page que d'une coquille native dont le futur doit être `Send` — le port est
/// `?Send`, cette règle ne l'est pas. Le lot est vérifié **entier** : un seul
/// vecteur fautif, même loin du premier, refuse le lot, et les deux dimensions
/// sont nommées ([`EmbedError::Dimension`]).
///
/// C'est le SEUL endroit où la règle est écrite : [`vectoriser`] l'applique, et
/// un hôte qui vectorise sans passer par le port (coquille native) l'appelle
/// directement au lieu de la réécrire.
pub fn verifier(attendue: &EmbeddingModel, recues: &[Vec<f32>]) -> Result<(), EmbedError> {
    for vecteur in recues {
        if vecteur.len() != attendue.dimension {
            return Err(EmbedError::Dimension {
                attendue: attendue.dimension,
                recue: vecteur.len(),
            });
        }
    }
    Ok(())
}

/// Vectorise via le port, en confrontant le résultat à l'estampille annoncée.
///
/// C'est le consommateur : sans estampille (`modele() == None`), `embed` n'est
/// **pas** appelé et l'on rend [`EmbedError::Unsupported`] — le consommateur
/// retombe sur le lexique seul. Une dimension rendue différente de l'annonce
/// est refusée en nommant les deux, jamais comparée en silence. Un échec de
/// l'implémentation est propagé, pas avalé.
pub async fn vectoriser(embed: &dyn Embed, textes: &[String]) -> Result<Vec<Vec<f32>>, EmbedError> {
    let modele = embed.modele().ok_or(EmbedError::Unsupported)?;
    let vecteurs = embed.embed(textes).await?;
    // La règle n'est pas réécrite ici : `verifier` la porte, et un hôte qui
    // vectorise hors du port (futur `Send`) l'appelle sans la dupliquer.
    verifier(modele, &vecteurs)?;
    Ok(vecteurs)
}

/// Le vecteur dense d'une requête, ou `None` : sans implémentation branchée,
/// le consommateur garde le lexique seul (`(None, None)`), et `embed` n'est
/// jamais appelé. Un échec d'une implémentation branchée (réseau, dimension)
/// retombe aussi sur le lexique et rend la raison pour le journal — jamais une
/// erreur fatale.
pub async fn vecteur_dense(embed: &dyn Embed, texte: &str) -> (Option<Vec<f32>>, Option<String>) {
    match vectoriser(embed, &[texte.to_string()]).await {
        Ok(mut vecteurs) => (vecteurs.pop(), None),
        Err(EmbedError::Unsupported) => (None, None),
        Err(raison) => (None, Some(raison.to_string())),
    }
}
