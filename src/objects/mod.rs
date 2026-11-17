//! Couche objets : blobs sur S3/MinIO/local (via `object_store`), isolés par tenant.
//!
//! Indépendante des backends d'entités (`storage`). Toute clé est préfixée par
//! `tenants/<tenant>/` et ne peut pas en sortir (`..` et `/` initial refusés).

use bytes::Bytes;
use futures::TryStreamExt;
use object_store::{ObjectStore, path::Path};
use std::sync::Arc;

/// Une entrée listée : clé relative au tenant.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entree {
    /// Clé SANS le préfixe `tenants/<tenant>/`.
    pub cle: String,
    pub taille: u64,
    pub modifie_ms: i64,
}

#[derive(Clone)]
pub struct Objets {
    store: Arc<dyn ObjectStore>,
}

fn verifier_tenant(tenant: &str) -> Result<(), String> {
    if tenant.is_empty() || tenant.contains('/') || tenant == ".." || tenant == "." {
        return Err(format!("tenant invalide : {tenant:?}"));
    }
    Ok(())
}

fn verifier_segments(cle: &str) -> Result<(), String> {
    if cle.starts_with('/') {
        return Err(format!("clé absolue refusée : {cle:?}"));
    }
    if cle.split('/').any(|s| s == "..") {
        return Err(format!("segment '..' refusé : {cle:?}"));
    }
    Ok(())
}

/// Chemin complet `tenants/<tenant>/<cle>`, garde d'isolation.
pub fn chemin(tenant: &str, cle: &str) -> Result<Path, String> {
    verifier_tenant(tenant)?;
    if cle.is_empty() {
        return Err("clé vide".into());
    }
    verifier_segments(cle)?;
    Path::parse(format!("tenants/{tenant}/{cle}")).map_err(|e| format!("clé invalide : {e}"))
}

impl Objets {
    pub fn new(store: Arc<dyn ObjectStore>) -> Self {
        Self { store }
    }

    pub async fn ecrire(&self, tenant: &str, cle: &str, octets: Bytes) -> Result<(), String> {
        let p = chemin(tenant, cle)?;
        self.store
            .put(&p, octets.into())
            .await
            .map(|_| ())
            .map_err(|e| format!("écriture de {cle} : {e}"))
    }

    pub async fn lire(&self, tenant: &str, cle: &str) -> Result<Bytes, String> {
        let p = chemin(tenant, cle)?;
        let r = self
            .store
            .get(&p)
            .await
            .map_err(|e| format!("lecture de {cle} : {e}"))?;
        r.bytes()
            .await
            .map_err(|e| format!("lecture de {cle} : {e}"))
    }

    pub async fn lister(&self, tenant: &str, prefixe: &str) -> Result<Vec<Entree>, String> {
        verifier_tenant(tenant)?;
        verifier_segments(prefixe)?;
        let base = format!("tenants/{tenant}");
        let complet = if prefixe.is_empty() {
            base.clone()
        } else {
            format!("{base}/{}", prefixe.trim_end_matches('/'))
        };
        let p = Path::parse(complet).map_err(|e| format!("préfixe invalide : {e}"))?;
        let metas: Vec<_> = self
            .store
            .list(Some(&p))
            .try_collect()
            .await
            .map_err(|e| format!("listage de {prefixe} : {e}"))?;
        let retire = format!("{base}/");
        let mut v: Vec<Entree> = metas
            .into_iter()
            .map(|m| {
                let loc = m.location.to_string();
                Entree {
                    cle: loc.strip_prefix(&retire).unwrap_or(&loc).to_string(),
                    taille: m.size as u64,
                    modifie_ms: m.last_modified.timestamp_millis(),
                }
            })
            .collect();
        v.sort_by(|a, b| a.cle.cmp(&b.cle));
        Ok(v)
    }

    pub async fn supprimer(&self, tenant: &str, cle: &str) -> Result<(), String> {
        let p = chemin(tenant, cle)?;
        self.store
            .delete(&p)
            .await
            .map_err(|e| format!("suppression de {cle} : {e}"))
    }

    pub async fn existe(&self, tenant: &str, cle: &str) -> Result<bool, String> {
        let p = chemin(tenant, cle)?;
        match self.store.head(&p).await {
            Ok(_) => Ok(true),
            Err(object_store::Error::NotFound { .. }) => Ok(false),
            Err(e) => Err(format!("existence de {cle} : {e}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use object_store::memory::InMemory;

    fn objets() -> Objets {
        Objets::new(Arc::new(InMemory::new()))
    }

    #[tokio::test]
    async fn ecrire_lire_aller_retour() {
        let o = objets();
        o.ecrire("t", "a/b.bin", Bytes::from_static(b"\x00\x01salut"))
            .await
            .unwrap();
        assert_eq!(
            o.lire("t", "a/b.bin").await.unwrap().as_ref(),
            b"\x00\x01salut"
        );
    }

    #[tokio::test]
    async fn isolation_tenant() {
        let o = objets();
        o.ecrire("A", "x", Bytes::from_static(b"1")).await.unwrap();
        assert!(o.lister("B", "").await.unwrap().is_empty());
        assert!(o.lire("B", "x").await.is_err());
        assert!(!o.existe("B", "x").await.unwrap());
    }

    #[tokio::test]
    async fn lister_donne_les_cles_relatives() {
        let o = objets();
        o.ecrire("t", "a/1.txt", Bytes::from_static(b"un"))
            .await
            .unwrap();
        o.ecrire("t", "a/2.txt", Bytes::from_static(b"deux"))
            .await
            .unwrap();
        o.ecrire("t", "b/3.txt", Bytes::from_static(b"x"))
            .await
            .unwrap();
        let l = o.lister("t", "a/").await.unwrap();
        let cles: Vec<_> = l.iter().map(|e| e.cle.as_str()).collect();
        assert_eq!(cles, ["a/1.txt", "a/2.txt"]);
        assert_eq!(l[1].taille, 4);
    }

    #[test]
    fn cle_avec_dotdot_refusee() {
        assert!(chemin("t", "../x").is_err());
        assert!(chemin("t", "a/../x").is_err());
        assert!(chemin("t", "/x").is_err());
        assert!(chemin("t", "").is_err());
        assert!(chemin("", "x").is_err());
        assert_eq!(chemin("t", "a/x").unwrap().to_string(), "tenants/t/a/x");
    }

    #[tokio::test]
    async fn supprimer_retire() {
        let o = objets();
        o.ecrire("t", "k", Bytes::from_static(b"v")).await.unwrap();
        assert!(o.existe("t", "k").await.unwrap());
        o.supprimer("t", "k").await.unwrap();
        assert!(!o.existe("t", "k").await.unwrap());
    }
}
