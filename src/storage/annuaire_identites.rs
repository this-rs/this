//! Annuaire d'identités persisté — chantier STS-PERSISTANT.
//!
//! Les utilisateurs de la passerelle cessent d'être des arguments de ligne de
//! commande en mémoire (mot de passe EN CLAIR dans un environnement systemd :
//! le défaut à tuer) : ils vivent dans un **store obrain dédié**, nœuds de
//! label [`LABEL_UTILISATEUR`] portant `{username, hash_argon2, user_id,
//! tenant_id, roles, created_at}`.
//!
//! # Mots de passe : argon2id, jamais de clair
//!
//! Le hachage est **argon2id** via [`Argon2::default()`], soit les paramètres
//! recommandés par l'OWASP Password Storage Cheat Sheet : v19, m=19456 KiB
//! (19 Mio), t=2 itérations, p=1 voie, sel aléatoire de 16 octets (OsRng),
//! sortie 32 octets, sérialisé au format PHC (`$argon2id$v=19$m=19456,t=2,
//! p=1$…`). Seule cette chaîne PHC est persistée — le clair ne touche jamais
//! le disque, et la vérification passe par [`PasswordVerifier`] (comparaison
//! à temps constant côté digest).
//!
//! # Verrou : un seul écrivain
//!
//! Le store est ouvert par [`ObrainStore::open`] — verrou **exclusif**. C'est
//! voulu : la passerelle en marche est LE processus qui détient les identités
//! (l'administration à chaud passe par ses routes HTTP `/auth/users`) ; les
//! sous-commandes CLI `user add/list/passwd` s'exécutent passerelle éteinte.
//! Un deuxième ouvreur échoue au verrou au lieu d'écrire dans le dos du
//! premier — c'est le comportement souhaité, documenté jusque dans les tests.

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, RwLock};

use anyhow::{Context, Result, anyhow, bail};
use argon2::Argon2;
use argon2::password_hash::{
    PasswordHash, PasswordHasher, PasswordVerifier, SaltString, rand_core::OsRng,
};
use obrain_common::types::{NodeId, Value as ObrainValue};
use uuid::Uuid;

use super::obrain::ObrainStore;

/// Le label des nœuds d'identité dans le store dédié.
pub const LABEL_UTILISATEUR: &str = "User";

/// Un hash argon2id « épouvantail » : vérifié quand l'utilisateur n'existe
/// pas, pour que « inconnu » et « mauvais mot de passe » coûtent le même
/// temps (pas d'énumération d'utilisateurs par chronométrage).
const HASH_EPOUVANTAIL: &str = "$argon2id$v=19$m=19456,t=2,p=1$c2VsLWVwb3V2YW50YWls$L1BOc9OO3XCcnGGSA9BhKgkyq/8k9zSjHLQC9ZmXFYU";

/// Un utilisateur tel que persisté — le champ `hash_argon2` est la chaîne
/// PHC complète, jamais le clair.
#[derive(Debug, Clone)]
pub struct Utilisateur {
    pub username: String,
    pub hash_argon2: String,
    pub user_id: Uuid,
    pub tenant_id: Uuid,
    pub roles: Vec<String>,
    /// RFC 3339, figé à la création.
    pub created_at: String,
}

/// L'annuaire : un `ObrainStore` inscriptible + un index `username → NodeId`
/// reconstruit à l'ouverture (les index obrain en mémoire ne survivent pas à
/// une réouverture — même contrat que `rebuild_index_for_label`).
pub struct AnnuaireIdentites {
    store: Arc<ObrainStore>,
    index: RwLock<HashMap<String, NodeId>>,
}

impl AnnuaireIdentites {
    /// Ouvre (ou crée) le store d'identités à `chemin`, et recharge l'index.
    pub fn ouvrir(chemin: &Path) -> Result<Self> {
        if let Some(parent) = chemin.parent()
            && !parent.as_os_str().is_empty()
        {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("création de {}", parent.display()))?;
        }
        let store = ObrainStore::open(chemin)
            .with_context(|| format!("ouverture du store d'identités {}", chemin.display()))?;
        Self::depuis_store(store)
    }

    /// Construit l'annuaire sur un store déjà ouvert (couture de test).
    pub fn depuis_store(store: Arc<ObrainStore>) -> Result<Self> {
        let annuaire = Self {
            store,
            index: RwLock::new(HashMap::new()),
        };
        annuaire.recharger()?;
        Ok(annuaire)
    }

    /// Rebalaye les nœuds `User` du store et reconstruit l'index. Rend le
    /// nombre d'identités connues.
    pub fn recharger(&self) -> Result<usize> {
        let magasin = self.store.active_store();
        let session = self.store.session();
        let mut index = self
            .index
            .write()
            .map_err(|_| anyhow!("index d'identités : verrou empoisonné"))?;
        index.clear();
        for node_id in magasin.nodes_by_label(LABEL_UTILISATEUR) {
            if let Some(noeud) = session.get_node(node_id)
                && let Some(nom) = noeud
                    .get_property("username")
                    .and_then(|v| v.as_str().map(String::from))
            {
                index.insert(nom, node_id);
            }
        }
        Ok(index.len())
    }

    /// Le nombre d'identités connues.
    pub fn compte(&self) -> usize {
        self.index.read().map(|i| i.len()).unwrap_or(0)
    }

    /// Cherche un utilisateur par nom.
    pub fn chercher(&self, username: &str) -> Result<Option<Utilisateur>> {
        let node_id = {
            let index = self
                .index
                .read()
                .map_err(|_| anyhow!("index d'identités : verrou empoisonné"))?;
            match index.get(username) {
                Some(&nid) => nid,
                None => return Ok(None),
            }
        };
        self.lit_noeud(node_id).map(Some)
    }

    /// Tous les utilisateurs, triés par nom (sortie stable pour `user list`).
    pub fn lister(&self) -> Result<Vec<Utilisateur>> {
        let ids: Vec<NodeId> = {
            let index = self
                .index
                .read()
                .map_err(|_| anyhow!("index d'identités : verrou empoisonné"))?;
            index.values().copied().collect()
        };
        let mut tous = ids
            .into_iter()
            .map(|nid| self.lit_noeud(nid))
            .collect::<Result<Vec<_>>>()?;
        tous.sort_by(|a, b| a.username.cmp(&b.username));
        Ok(tous)
    }

    /// Crée un utilisateur : hache le mot de passe (argon2id), persiste le
    /// nœud, commit le WAL. Refuse un nom déjà pris.
    pub fn ajouter(
        &self,
        username: &str,
        mot_de_passe: &str,
        tenant_id: Uuid,
        roles: Vec<String>,
    ) -> Result<Utilisateur> {
        self.ajouter_avec_id(username, mot_de_passe, Uuid::new_v4(), tenant_id, roles)
    }

    /// ── LA MÊME CHOSE, AVEC UN `user_id` DÉJÀ CONNU (2026-09-20) ────────────
    ///
    /// Un appelant qui a **déjà matérialisé des décisions IAM** sous un
    /// identifiant fixe ne peut pas accepter un identifiant tiré au sort : ses
    /// politiques ne désigneraient plus personne. C'est le cas du chat mobile,
    /// dont les comptes portent des UID constants (`UID_TRIVIERE`,
    /// `UID_ACHAPTAL`) précisément parce que ses décisions sont écrites.
    ///
    /// L'aléa n'est pas ce qui protège ici : `user_id` est un identifiant
    /// **public**, et le secret est le mot de passe (argon2id).
    ///
    /// # Erreurs
    ///
    /// Comme [`Self::ajouter`] : champs vides, ou utilisateur déjà présent.
    pub fn ajouter_avec_id(
        &self,
        username: &str,
        mot_de_passe: &str,
        user_id: Uuid,
        tenant_id: Uuid,
        roles: Vec<String>,
    ) -> Result<Utilisateur> {
        if username.is_empty() || mot_de_passe.is_empty() {
            bail!("username et mot de passe ne peuvent pas être vides");
        }
        if self.chercher(username)?.is_some() {
            bail!("l'utilisateur `{username}` existe déjà dans l'annuaire");
        }
        self.store.check_write()?;

        let utilisateur = Utilisateur {
            username: username.to_string(),
            hash_argon2: hacher(mot_de_passe)?,
            user_id,
            tenant_id,
            roles,
            created_at: chrono::Utc::now().to_rfc3339(),
        };

        let roles_json = serde_json::to_string(&utilisateur.roles)?;
        let node_id = {
            let session = self.store.session();
            session.create_node_with_props(
                &[LABEL_UTILISATEUR],
                vec![
                    ("username", ObrainValue::from(utilisateur.username.as_str())),
                    (
                        "hash_argon2",
                        ObrainValue::from(utilisateur.hash_argon2.as_str()),
                    ),
                    (
                        "user_id",
                        ObrainValue::from(utilisateur.user_id.to_string().as_str()),
                    ),
                    (
                        "tenant_id",
                        ObrainValue::from(utilisateur.tenant_id.to_string().as_str()),
                    ),
                    ("roles", ObrainValue::from(roles_json.as_str())),
                    (
                        "created_at",
                        ObrainValue::from(utilisateur.created_at.as_str()),
                    ),
                ],
            )
        };
        self.store
            .commit_wal()
            .context("commit_wal après création d'utilisateur")?;

        self.index
            .write()
            .map_err(|_| anyhow!("index d'identités : verrou empoisonné"))?
            .insert(utilisateur.username.clone(), node_id);
        Ok(utilisateur)
    }

    /// Remplace le mot de passe (re-hache, persiste, commit).
    pub fn changer_mot_de_passe(&self, username: &str, nouveau: &str) -> Result<()> {
        if nouveau.is_empty() {
            bail!("le nouveau mot de passe ne peut pas être vide");
        }
        let node_id = {
            let index = self
                .index
                .read()
                .map_err(|_| anyhow!("index d'identités : verrou empoisonné"))?;
            *index
                .get(username)
                .ok_or_else(|| anyhow!("utilisateur `{username}` inconnu de l'annuaire"))?
        };
        self.store.check_write()?;
        let hash = hacher(nouveau)?;
        self.store.active_store().set_node_property(
            node_id,
            "hash_argon2",
            ObrainValue::from(hash.as_str()),
        );
        self.store
            .commit_wal()
            .context("commit_wal après changement de mot de passe")?;
        Ok(())
    }

    /// ── POSER LES RÔLES D'UN COMPTE (2026-09-21) ────────────────────────────
    ///
    /// L'administration des comptes a besoin de MODIFIER les rôles d'un
    /// utilisateur existant : c'est la différence entre « créer un compte avec
    /// ses droits » (une fois) et « confier un droit à quelqu'un qui est déjà
    /// là » (tous les jours).
    ///
    /// Les rôles SONT la liste des permissions portées par le compte : c'est le
    /// champ que `verifier_identite` fait entrer dans le jeton, et celui que
    /// l'appelant matérialise en décisions d'autorisation.
    ///
    /// # Erreurs
    ///
    /// Utilisateur inconnu, ou store non inscriptible.
    pub fn poser_roles(&self, username: &str, roles: Vec<String>) -> Result<()> {
        let node_id = {
            let index = self
                .index
                .read()
                .map_err(|_| anyhow!("index d'identités : verrou empoisonné"))?;
            *index
                .get(username)
                .ok_or_else(|| anyhow!("utilisateur `{username}` inconnu de l'annuaire"))?
        };
        self.store.check_write()?;
        let roles_json = serde_json::to_string(&roles)?;
        self.store.active_store().set_node_property(
            node_id,
            "roles",
            ObrainValue::from(roles_json.as_str()),
        );
        self.store
            .commit_wal()
            .context("commit_wal après changement de rôles")?;
        Ok(())
    }

    /// Vérifie un couple login/mot de passe. Rend l'utilisateur si — et
    /// seulement si — le hash argon2id correspond. Un utilisateur inconnu
    /// paie le même prix qu'un mauvais mot de passe (épouvantail).
    pub fn verifier(&self, username: &str, mot_de_passe: &str) -> Result<Option<Utilisateur>> {
        let candidat = self.chercher(username)?;
        let hash_str = candidat
            .as_ref()
            .map(|u| u.hash_argon2.as_str())
            .unwrap_or(HASH_EPOUVANTAIL);
        let hash = PasswordHash::new(hash_str)
            .map_err(|e| anyhow!("hash argon2 illisible pour `{username}` : {e}"))?;
        let valide = Argon2::default()
            .verify_password(mot_de_passe.as_bytes(), &hash)
            .is_ok();
        Ok(if valide { candidat } else { None })
    }
}

/// Hache un mot de passe en argon2id (paramètres par défaut du crate :
/// v19, m=19456 KiB, t=2, p=1 — recommandation OWASP), sel OsRng.
pub fn hacher(mot_de_passe: &str) -> Result<String> {
    let sel = SaltString::generate(&mut OsRng);
    Ok(Argon2::default()
        .hash_password(mot_de_passe.as_bytes(), &sel)
        .map_err(|e| anyhow!("hachage argon2id : {e}"))?
        .to_string())
}

impl AnnuaireIdentites {
    fn lit_noeud(&self, node_id: NodeId) -> Result<Utilisateur> {
        let session = self.store.session();
        let noeud = session
            .get_node(node_id)
            .ok_or_else(|| anyhow!("nœud d'identité {node_id:?} disparu"))?;
        let texte = |cle: &str| -> Result<String> {
            noeud
                .get_property(cle)
                .and_then(|v| v.as_str().map(String::from))
                .ok_or_else(|| anyhow!("nœud d'identité sans propriété `{cle}`"))
        };
        let roles: Vec<String> = serde_json::from_str(&texte("roles")?)
            .context("propriété `roles` illisible (JSON attendu)")?;
        Ok(Utilisateur {
            username: texte("username")?,
            hash_argon2: texte("hash_argon2")?,
            user_id: Uuid::parse_str(&texte("user_id")?)?,
            tenant_id: Uuid::parse_str(&texte("tenant_id")?)?,
            roles,
            created_at: texte("created_at")?,
        })
    }
}

/// L'annuaire EST le vérificateur d'identités du STS : quand il est branché,
/// le store fait foi — l'ancien annuaire en mémoire (clair) ne sert plus.
#[cfg(feature = "wami")]
impl crate::core::auth::sts::AnnuaireVerification for AnnuaireIdentites {
    fn verifier_identite(
        &self,
        username: &str,
        mot_de_passe: &str,
    ) -> Option<crate::core::auth::sts::IdentiteVerifiee> {
        match self.verifier(username, mot_de_passe) {
            Ok(Some(u)) => Some(crate::core::auth::sts::IdentiteVerifiee {
                user_id: u.user_id,
                tenant_id: u.tenant_id,
                roles: u.roles,
            }),
            Ok(None) => None,
            Err(e) => {
                tracing::warn!(username, "vérification d'identité en échec : {e}");
                None
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn annuaire_temporaire() -> (tempfile::TempDir, AnnuaireIdentites) {
        let dir = tempfile::tempdir().expect("tempdir");
        let annuaire =
            AnnuaireIdentites::ouvrir(&dir.path().join("identites.obrain")).expect("ouvrir");
        (dir, annuaire)
    }

    #[test]
    fn ajouter_puis_verifier() {
        let (_dir, annuaire) = annuaire_temporaire();
        let tenant = Uuid::new_v4();
        let cree = annuaire
            .ajouter("alice", "tres-secret", tenant, vec!["lecteur".into()])
            .expect("ajouter");

        // Le hash est bien de l'argon2id au format PHC — jamais le clair.
        assert!(cree.hash_argon2.starts_with("$argon2id$v=19$"));
        assert!(!cree.hash_argon2.contains("tres-secret"));

        let ok = annuaire.verifier("alice", "tres-secret").expect("verifier");
        assert_eq!(ok.as_ref().map(|u| u.tenant_id), Some(tenant));
        assert_eq!(ok.unwrap().roles, vec!["lecteur".to_string()]);

        assert!(annuaire.verifier("alice", "faux").unwrap().is_none());
        assert!(
            annuaire
                .verifier("inconnue", "tres-secret")
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn doublon_refuse() {
        let (_dir, annuaire) = annuaire_temporaire();
        annuaire
            .ajouter("bob", "mdp", Uuid::new_v4(), vec![])
            .unwrap();
        let err = annuaire
            .ajouter("bob", "autre", Uuid::new_v4(), vec![])
            .unwrap_err();
        assert!(err.to_string().contains("existe déjà"));
    }

    #[test]
    fn changer_mot_de_passe_invalide_l_ancien() {
        let (_dir, annuaire) = annuaire_temporaire();
        annuaire
            .ajouter("carol", "ancien", Uuid::new_v4(), vec![])
            .unwrap();
        annuaire.changer_mot_de_passe("carol", "nouveau").unwrap();
        assert!(annuaire.verifier("carol", "ancien").unwrap().is_none());
        assert!(annuaire.verifier("carol", "nouveau").unwrap().is_some());

        // Le hash a changé et reste de l'argon2id.
        let u = annuaire.chercher("carol").unwrap().unwrap();
        assert!(u.hash_argon2.starts_with("$argon2id$"));
    }

    #[test]
    fn les_identites_survivent_a_une_reouverture() {
        let dir = tempfile::tempdir().unwrap();
        let chemin = dir.path().join("identites.obrain");
        let tenant = Uuid::new_v4();
        {
            let annuaire = AnnuaireIdentites::ouvrir(&chemin).unwrap();
            annuaire
                .ajouter("durable", "mdp-durable", tenant, vec!["admin".into()])
                .unwrap();
        } // drop → verrou relâché, WAL committé

        let rouvert = AnnuaireIdentites::ouvrir(&chemin).expect("réouverture");
        assert_eq!(rouvert.compte(), 1);
        let u = rouvert
            .verifier("durable", "mdp-durable")
            .unwrap()
            .expect("l'utilisateur survit à la réouverture");
        assert_eq!(u.tenant_id, tenant);
        assert_eq!(u.roles, vec!["admin".to_string()]);
    }

    #[test]
    fn l_epouvantail_est_un_hash_valide() {
        // Si ce hash devenait illisible, `verifier` sur un inconnu rendrait
        // Err au lieu de None — l'épouvantail doit rester analysable.
        assert!(PasswordHash::new(HASH_EPOUVANTAIL).is_ok());
    }
}
