//! Couche objets : blobs sur S3/MinIO/local (via `object_store`), isolés par tenant.
//!
//! Indépendante des backends d'entités (`storage`). Toute clé est préfixée par
//! `tenants/<tenant>/` et ne peut pas en sortir (`..` et `/` initial refusés).

pub use bytes::Bytes;
use futures::TryStreamExt;
pub use object_store::signer::Signer;
use object_store::{ObjectStore, path::Path};
use std::sync::Arc;
use tokio::sync::broadcast;

/// Une entrée listée : clé relative au tenant.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entree {
    /// Clé SANS le préfixe `tenants/<tenant>/`.
    pub cle: String,
    pub taille: u64,
    pub modifie_ms: i64,
}

/// Ce qui est arrivé à un objet de l'espace.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EvenementObjet {
    pub tenant: String,
    /// Clé sans le préfixe `tenants/<tenant>/`.
    pub cle: String,
    pub genre: GenreObjet,
    /// Taille en octets (0 pour une suppression).
    pub taille: u64,
    /// Date de l'événement (ms depuis l'époque).
    pub a_ms: i64,
}

/// La nature d'un changement.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GenreObjet {
    Ecrit,
    Supprime,
}

/// L'accès visé par une URL présignée.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GenreUrl {
    Lire,
    Ecrire,
    Supprimer,
}

/// Combien d'événements non lus un abonné lent peut accumuler avant de perdre
/// les plus anciens : un abonné lent ne bloque jamais une écriture.
const CAPACITE_EVENEMENTS: usize = 256;

fn maintenant_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

#[derive(Clone)]
pub struct Objets {
    store: Arc<dyn ObjectStore>,
    /// Le signataire d'URL (S3/Azure/GCP), quand le store le permet. Le natif
    /// l'utilise pour des URLs présignées ; un store local n'en a pas.
    signer: Option<Arc<dyn Signer>>,
    diffusion: broadcast::Sender<EvenementObjet>,
}

pub mod televersement;
pub use televersement::Televersement;

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
        let (diffusion, _) = broadcast::channel(CAPACITE_EVENEMENTS);
        Self {
            store,
            signer: None,
            diffusion,
        }
    }

    /// Comme [`Objets::new`], mais avec le **signataire** du même store S3 —
    /// l'hôte peut alors obtenir des URLs présignées (`signataire()` + `chemin`).
    pub fn avec_signature(store: Arc<dyn ObjectStore>, signer: Arc<dyn Signer>) -> Self {
        let (diffusion, _) = broadcast::channel(CAPACITE_EVENEMENTS);
        Self {
            store,
            signer: Some(signer),
            diffusion,
        }
    }

    /// Le signataire, si le store en a un (S3/Azure/GCP). `None` en local/mémoire.
    pub fn signataire(&self) -> Option<Arc<dyn Signer>> {
        self.signer.clone()
    }

    /// Un espace rangé sur le **disque local**, sous `dir`.
    ///
    /// Même API, même isolement par tenant — utile quand il n'y a pas de S3
    /// (le poste, l'appareil). Le natif peut l'utiliser sans config réseau ;
    /// S3 reste un autre `ObjectStore` passé à [`Objets::new`].
    pub fn local(dir: impl AsRef<std::path::Path>) -> Result<Self, String> {
        let dir = dir.as_ref();
        // `LocalFileSystem` exige une racine existante (il la canonicalise).
        std::fs::create_dir_all(dir)
            .map_err(|e| format!("dossier objets {} : {e}", dir.display()))?;
        let store = object_store::local::LocalFileSystem::new_with_prefix(dir)
            .map_err(|e| format!("dossier objets invalide : {e}"))?;
        Ok(Self::new(Arc::new(store)))
    }

    /// S'abonner au flux des changements de l'espace (écrit / supprimé).
    ///
    /// Chaque abonné reçoit **tous** les tenants : c'est à lui de filtrer par
    /// `tenant`. Un message émis avant l'abonnement n'est pas rejoué (diffusion
    /// « à partir de maintenant »).
    pub fn abonner(&self) -> broadcast::Receiver<EvenementObjet> {
        self.diffusion.subscribe()
    }

    pub async fn ecrire(&self, tenant: &str, cle: &str, octets: Bytes) -> Result<(), String> {
        let p = chemin(tenant, cle)?;
        let taille = octets.len() as u64;
        self.store
            .put(&p, octets.into())
            .await
            .map_err(|e| format!("écriture de {cle} : {e}"))?;
        let _ = self.diffusion.send(EvenementObjet {
            tenant: tenant.to_string(),
            cle: cle.to_string(),
            genre: GenreObjet::Ecrit,
            taille,
            a_ms: maintenant_ms(),
        });
        Ok(())
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
            .map_err(|e| format!("suppression de {cle} : {e}"))?;
        let _ = self.diffusion.send(EvenementObjet {
            tenant: tenant.to_string(),
            cle: cle.to_string(),
            genre: GenreObjet::Supprime,
            taille: 0,
            a_ms: maintenant_ms(),
        });
        Ok(())
    }

    pub async fn existe(&self, tenant: &str, cle: &str) -> Result<bool, String> {
        let p = chemin(tenant, cle)?;
        match self.store.head(&p).await {
            Ok(_) => Ok(true),
            Err(object_store::Error::NotFound { .. }) => Ok(false),
            Err(e) => Err(format!("existence de {cle} : {e}")),
        }
    }

    /// Démarre un téléversement **multipart** de `cle`. On pousse des parties
    /// (`Televersement::partie`), puis `finir` scelle — ou `annuler` libère.
    pub async fn televerser(&self, tenant: &str, cle: &str) -> Result<Televersement, String> {
        let p = chemin(tenant, cle)?;
        let inner = self
            .store
            .put_multipart(&p)
            .await
            .map_err(|e| format!("début de téléversement de {cle} : {e}"))?;
        Ok(Televersement::nouveau(
            inner,
            self.diffusion.clone(),
            tenant.to_string(),
            cle.to_string(),
        ))
    }

    /// Téléverse un **fichier** par parties (multipart). Chaque partie fait
    /// `taille_part` octets (défaut 8 Mio, minimum 5 Mio comme S3). Rend le
    /// nombre d'octets envoyés. C'est le chemin d'un gros fichier sur mobile :
    /// rien n'est chargé d'un coup, la mémoire reste bornée à une partie.
    pub async fn televerser_chemin(
        &self,
        tenant: &str,
        cle: &str,
        chemin: impl AsRef<std::path::Path>,
        taille_part: Option<usize>,
    ) -> Result<u64, String> {
        use tokio::io::AsyncReadExt;
        let chemin = chemin.as_ref();
        let part = taille_part.unwrap_or(8 * 1024 * 1024).max(5 * 1024 * 1024);
        let mut fichier = tokio::fs::File::open(chemin)
            .await
            .map_err(|e| format!("ouverture de {} : {e}", chemin.display()))?;
        let mut tel = self.televerser(tenant, cle).await?;
        let mut tampon = vec![0u8; part];
        loop {
            let n = fichier
                .read(&mut tampon)
                .await
                .map_err(|e| format!("lecture de {} : {e}", chemin.display()))?;
            if n == 0 {
                break;
            }
            tel.partie(Bytes::copy_from_slice(&tampon[..n])).await?;
        }
        let total = tel.octets_ecrits();
        tel.finir().await?;
        Ok(total)
    }
}

#[cfg(feature = "s3")]
impl Objets {
    /// Un espace rangé dans un **bucket S3/**MinIO** (ou compatible). Même API et même
    /// isolement par tenant que le local. `endpoint` vide : AWS par défaut.
    pub fn s3(
        bucket: &str,
        endpoint: &str,
        region: &str,
        access: &str,
        secret: &str,
    ) -> Result<Self, String> {
        use object_store::aws::AmazonS3Builder;
        let mut b = AmazonS3Builder::new()
            .with_bucket_name(bucket)
            .with_access_key_id(access)
            .with_secret_access_key(secret)
            .with_allow_http(true);
        if !region.trim().is_empty() {
            b = b.with_region(region);
        }
        if !endpoint.trim().is_empty() {
            b = b.with_endpoint(endpoint);
        }
        let store = b.build().map_err(|e| format!("configuration S3 : {e}"))?;
        // Le store S3 signe aussi : URLs présignées disponibles (feature `presign`).
        Ok(Self::avec_signature(
            Arc::new(store.clone()),
            Arc::new(store),
        ))
    }
}

#[cfg(feature = "presign")]
impl Objets {
    /// Une URL **présignée** pour `cle` : un tiers sans identifiants S3 peut
    /// lire/écrire/supprimer pendant `secs`. Rend `Err` si le store ne signe pas
    /// (local/mémoire) — un presign exige un backend cloud.
    ///
    /// POURQUOI une feature : `object_store` pin `reqwest 0.12` pour signer, et
    /// l'hôte a souvent `reqwest 0.13`. Sans `presign`, aucune des deux piles
    /// n'est dupliquée ; avec, l'app choisit de payer la seconde.
    pub async fn url_presignee(
        &self,
        genre: GenreUrl,
        tenant: &str,
        cle: &str,
        secs: u64,
    ) -> Result<String, String> {
        let signer = self
            .signer
            .as_ref()
            .ok_or_else(|| "ce store ne signe pas (local/mémoire)".to_string())?;
        let p = chemin(tenant, cle)?;
        let method = match genre {
            GenreUrl::Lire => reqwest_presign::Method::GET,
            GenreUrl::Ecrire => reqwest_presign::Method::PUT,
            GenreUrl::Supprimer => reqwest_presign::Method::DELETE,
        };
        let url = signer
            .signed_url(method, &p, std::time::Duration::from_secs(secs))
            .await
            .map_err(|e| format!("presign de « {cle} » : {e}"))?;
        Ok(url.to_string())
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

    #[tokio::test]
    async fn local_range_sur_le_disque() {
        let dir = std::env::temp_dir().join(format!("objets-local-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let o = Objets::local(&dir).expect("store local");
        o.ecrire("alice", "a/b.txt", bytes::Bytes::from_static(b"x"))
            .await
            .unwrap();
        assert_eq!(&o.lire("alice", "a/b.txt").await.unwrap()[..], b"x");
        assert!(o.existe("alice", "a/b.txt").await.unwrap());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn televersement_multipart_reconstitue_l_objet() {
        let o = Objets::new(Arc::new(InMemory::new()) as Arc<dyn ObjectStore>);
        let mut rx = o.abonner();
        let mut t = o.televerser("alice", "gros.bin").await.unwrap();
        t.partie(Bytes::from_static(b"aaa")).await.unwrap();
        t.partie(Bytes::from_static(b"bbb")).await.unwrap();
        assert_eq!(t.octets_ecrits(), 6);
        t.finir().await.unwrap();
        assert_eq!(&o.lire("alice", "gros.bin").await.unwrap()[..], b"aaabbb");
        let e = rx.recv().await.unwrap();
        assert_eq!(e.genre, GenreObjet::Ecrit);
        assert_eq!(e.taille, 6);
        assert_eq!(e.cle, "gros.bin");
    }

    #[tokio::test]
    async fn televersement_annule_n_ecrit_rien() {
        let o = Objets::new(Arc::new(InMemory::new()) as Arc<dyn ObjectStore>);
        let mut t = o.televerser("alice", "jete.bin").await.unwrap();
        t.partie(Bytes::from_static(b"x")).await.unwrap();
        t.annuler().await.unwrap();
        assert!(!o.existe("alice", "jete.bin").await.unwrap());
    }

    #[tokio::test]
    async fn televerser_chemin_envoie_un_fichier_par_parties() {
        let dir = std::env::temp_dir().join(format!("objets-tel-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("f.bin");
        let contenu: Vec<u8> = (0..6 * 1024 * 1024).map(|i| (i % 251) as u8).collect();
        std::fs::write(&f, &contenu).unwrap();
        let o = Objets::local(dir.join("store")).unwrap();
        let total = o
            .televerser_chemin("alice", "gros.bin", &f, Some(5 * 1024 * 1024))
            .await
            .unwrap();
        assert_eq!(total as usize, contenu.len());
        assert_eq!(o.lire("alice", "gros.bin").await.unwrap().to_vec(), contenu);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(feature = "presign")]
    #[tokio::test]
    async fn presign_refuse_sans_signataire() {
        let o = Objets::new(Arc::new(InMemory::new()) as Arc<dyn ObjectStore>);
        let r = o.url_presignee(GenreUrl::Lire, "alice", "x.json", 60).await;
        let e = r.expect_err("un store local ne signe pas");
        assert!(e.contains("ne signe pas"), "{e}");
    }
}

#[cfg(feature = "objects-toolbox")]
pub mod toolbox;
