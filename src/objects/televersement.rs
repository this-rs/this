//! Téléversement **multipart** : écrire un objet en parties successives.
//!
//! POURQUOI : un fichier de 200 Mio sur un téléphone ne tient pas forcément en
//! mémoire, et une requête unique géante se rejoue entièrement au moindre
//! incident réseau. `put_multipart` découpe côté store (S3) : chaque partie
//! (sauf la dernière) fait au moins 5 Mio chez la plupart des fournisseurs.
//! On pousse, on scelle, ou on abandonne — jamais les trois.

use super::{EvenementObjet, GenreObjet};
use bytes::Bytes;
use object_store::MultipartUpload;
use tokio::sync::broadcast;

/// Un téléversement en cours. Tout est consommé par `finir`/`annuler`.
pub struct Televersement {
    inner: Box<dyn MultipartUpload>,
    diffusion: broadcast::Sender<EvenementObjet>,
    tenant: String,
    cle: String,
    total: u64,
}

impl Televersement {
    pub(crate) fn nouveau(
        inner: Box<dyn MultipartUpload>,
        diffusion: broadcast::Sender<EvenementObjet>,
        tenant: String,
        cle: String,
    ) -> Self {
        Self {
            inner,
            diffusion,
            tenant,
            cle,
            total: 0,
        }
    }

    /// Pousse une partie. L'ordre est celui des appels.
    pub async fn partie(&mut self, octets: Bytes) -> Result<(), String> {
        let n = octets.len() as u64;
        self.inner
            .put_part(octets.into())
            .await
            .map_err(|e| format!("partie de « {} » : {e}", self.cle))?;
        self.total += n;
        Ok(())
    }

    /// Nombre d'octets poussés jusqu'ici.
    pub fn octets_ecrits(&self) -> u64 {
        self.total
    }

    /// Scelle l'objet : à partir d'ici il est lisible. Émet `Écrit`.
    pub async fn finir(mut self) -> Result<(), String> {
        self.inner
            .complete()
            .await
            .map_err(|e| format!("fin de « {} » : {e}", self.cle))?;
        let _ = self.diffusion.send(EvenementObjet {
            tenant: self.tenant,
            cle: self.cle,
            genre: GenreObjet::Ecrit,
            taille: self.total,
            a_ms: super::maintenant_ms(),
        });
        Ok(())
    }

    /// Abandonne : rien n'est écrit, le store libère les parties.
    pub async fn annuler(mut self) -> Result<(), String> {
        self.inner
            .abort()
            .await
            .map_err(|e| format!("abandon de « {} » : {e}", self.cle))
    }
}
