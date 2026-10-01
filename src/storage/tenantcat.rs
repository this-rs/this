//! Catalogue tenant Uuid ↔ u32 d'un store de données — `substrate.tenantcat`.
//!
//! Chantier NATIF-J4 (RFC principal-natif-v2 §5.1). Le jeton STS porte un
//! `tenant_id: Uuid` ; le substrat confine par un `u32` (colonne `tenant_id`
//! des records v3). Le dictionnaire qui les relie appartient **au store de
//! données**, pas à la passerelle : « le store qui porte les données porte la
//! table qui les interprète — un store copié reste interprétable seul »
//! (RFC, recommandation confirmée par le motif éprouvé des catalogues plats
//! `substrate.indexcat` / `substrate.proccat`).
//!
//! # Format
//!
//! `<store>/substrate.tenantcat` : texte UTF-8, une ligne par tenant,
//! `uuid \t u32 \t libellé`, lignes vides et `#commentaires` ignorés.
//! **Append-only** : aucune ligne n'est jamais retirée ni modifiée — une
//! révocation d'accès se fait dans l'annuaire d'identités, pas ici. Le
//! numéro attribué est `max(existants) + 1` (départ à 1) : un numéro n'est
//! **jamais réutilisé**, même si le RFC autorisait un jour un retrait.
//!
//! Réservations héritées du substrat (jalons 2-3) : `0` est le PUBLIC
//! (jamais attribuable à un Uuid), `u32::MAX` est la sentinelle A5 des
//! nœuds non migrés (jamais une portée de lecture).
//!
//! # Qui écrit, qui lit
//!
//! * **Écrit** hors requête par l'exploitant : `this-gateway tenantcat
//!   add <store-dir> <uuid> [libellé]`. La passerelle en marche monte les
//!   stores de données en LECTURE SEULE et n'écrit jamais ce fichier.
//! * **Lu** au montage par la passerelle (un tenant interné après le
//!   démarrage exige un redémarrage — limitation documentée du plan J4).
//!
//! Le code vit ici (passerelle) parce qu'elle est aujourd'hui l'unique
//! lectrice/écrivaine ; le format est celui du RFC, la graduation future
//! dans `obrain-substrate` (quand les writers du moteur apprendront le
//! tenant, jalon 5) ne changera pas les fichiers.

use std::collections::HashMap;
use std::io::Write;
use std::path::Path;

use anyhow::{Context, Result, anyhow, bail};
use uuid::Uuid;

/// Le nom du fichier catalogue, à la racine du répertoire du store.
pub const FICHIER_TENANTCAT: &str = "substrate.tenantcat";

/// La sentinelle A5 du substrat — jamais attribuable.
const SENTINELLE: u32 = u32::MAX;

/// Une ligne du catalogue : l'attribution d'un numéro interne à un tenant.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EntreeTenant {
    pub uuid: Uuid,
    pub numero: u32,
    pub libelle: String,
}

/// Le catalogue d'un store, chargé en mémoire. Immuable après chargement —
/// l'attribution ([`CatalogueTenants::attribuer`]) relit le fichier, ajoute
/// UNE ligne et rend la nouvelle entrée ; elle ne mute pas un catalogue
/// déjà chargé (la passerelle recharge au montage, jamais en vol).
#[derive(Debug, Default)]
pub struct CatalogueTenants {
    entrees: Vec<EntreeTenant>,
    par_uuid: HashMap<Uuid, u32>,
}

impl CatalogueTenants {
    /// Charge `<store_dir>/substrate.tenantcat`. Fichier absent = catalogue
    /// vide (store sans tenant interné : toute vue authentifiée retombe sur
    /// le public). Fichier PRÉSENT mais illisible ou incohérent = erreur —
    /// fail-closed : on ne monte pas un store dont on ne sait pas
    /// interpréter les tenants.
    pub fn charger(store_dir: &Path) -> Result<Self> {
        let chemin = store_dir.join(FICHIER_TENANTCAT);
        if !chemin.exists() {
            return Ok(Self::default());
        }
        let texte = std::fs::read_to_string(&chemin)
            .with_context(|| format!("lecture de {}", chemin.display()))?;

        let mut entrees = Vec::new();
        let mut par_uuid: HashMap<Uuid, u32> = HashMap::new();
        let mut numeros_vus: HashMap<u32, Uuid> = HashMap::new();
        for (i, ligne) in texte.lines().enumerate() {
            let n = i + 1;
            let ligne = ligne.trim();
            if ligne.is_empty() || ligne.starts_with('#') {
                continue;
            }
            let mut champs = ligne.splitn(3, '\t');
            let uuid: Uuid = champs
                .next()
                .ok_or_else(|| anyhow!("{}:{n} : ligne vide", chemin.display()))?
                .parse()
                .map_err(|e| anyhow!("{}:{n} : uuid illisible ({e})", chemin.display()))?;
            let numero: u32 = champs
                .next()
                .ok_or_else(|| anyhow!("{}:{n} : numéro manquant", chemin.display()))?
                .parse()
                .map_err(|e| anyhow!("{}:{n} : numéro illisible ({e})", chemin.display()))?;
            let libelle = champs.next().unwrap_or("").to_string();

            if numero == 0 {
                bail!(
                    "{}:{n} : le numéro 0 est le PUBLIC, jamais attribuable",
                    chemin.display()
                );
            }
            if numero == SENTINELLE {
                bail!(
                    "{}:{n} : u32::MAX est la sentinelle A5, jamais attribuable",
                    chemin.display()
                );
            }
            if let Some(deja) = par_uuid.get(&uuid) {
                bail!(
                    "{}:{n} : uuid {uuid} déjà interné sous le numéro {deja}",
                    chemin.display()
                );
            }
            if let Some(autre) = numeros_vus.get(&numero) {
                bail!(
                    "{}:{n} : numéro {numero} déjà attribué à {autre}",
                    chemin.display()
                );
            }
            par_uuid.insert(uuid, numero);
            numeros_vus.insert(numero, uuid);
            entrees.push(EntreeTenant {
                uuid,
                numero,
                libelle,
            });
        }
        Ok(Self { entrees, par_uuid })
    }

    /// Le numéro interne d'un tenant, s'il est interné dans CE store.
    /// `None` = pas de données tenantées ici : la passerelle sert la vue
    /// publique (tenant 0).
    pub fn resoudre(&self, uuid: &Uuid) -> Option<u32> {
        self.par_uuid.get(uuid).copied()
    }

    /// Toutes les attributions, dans l'ordre du fichier.
    pub fn entrees(&self) -> &[EntreeTenant] {
        &self.entrees
    }

    /// Le nombre de tenants internés.
    pub fn compte(&self) -> usize {
        self.entrees.len()
    }

    /// Attribue un numéro à `uuid` dans le catalogue de `store_dir` :
    /// relit le fichier, refuse un uuid déjà interné, choisit
    /// `max(existants) + 1` (départ à 1 — jamais 0, jamais la sentinelle),
    /// APPEND la ligne et fsync. Geste d'exploitant, hors requête —
    /// passerelle éteinte ou redémarrée ensuite pour qu'elle le voie.
    pub fn attribuer(store_dir: &Path, uuid: Uuid, libelle: &str) -> Result<EntreeTenant> {
        if libelle.contains('\t') || libelle.contains('\n') {
            bail!("le libellé ne peut contenir ni tabulation ni retour à la ligne");
        }
        let courant = Self::charger(store_dir)?;
        if let Some(numero) = courant.resoudre(&uuid) {
            bail!(
                "{uuid} est déjà interné sous le numéro {numero} — un numéro ne se réattribue pas"
            );
        }
        let prochain = courant
            .entrees
            .iter()
            .map(|e| e.numero)
            .max()
            .unwrap_or(0)
            .checked_add(1)
            .ok_or_else(|| anyhow!("catalogue plein : plus de numéro attribuable"))?;
        if prochain == SENTINELLE {
            bail!("catalogue plein : le prochain numéro serait la sentinelle A5");
        }

        let chemin = store_dir.join(FICHIER_TENANTCAT);
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&chemin)
            .with_context(|| format!("ouverture en append de {}", chemin.display()))?;
        writeln!(f, "{uuid}\t{prochain}\t{libelle}")
            .with_context(|| format!("écriture dans {}", chemin.display()))?;
        f.sync_all()
            .with_context(|| format!("fsync de {}", chemin.display()))?;

        Ok(EntreeTenant {
            uuid,
            numero: prochain,
            libelle: libelle.to_string(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn absent_est_vide() {
        let dir = tempfile::tempdir().unwrap();
        let cat = CatalogueTenants::charger(dir.path()).unwrap();
        assert_eq!(cat.compte(), 0);
        assert_eq!(cat.resoudre(&Uuid::new_v4()), None);
    }

    #[test]
    fn attribution_stable_et_monotone() {
        let dir = tempfile::tempdir().unwrap();
        let (a, b) = (Uuid::new_v4(), Uuid::new_v4());
        let ea = CatalogueTenants::attribuer(dir.path(), a, "tenant A").unwrap();
        let eb = CatalogueTenants::attribuer(dir.path(), b, "").unwrap();
        assert_eq!(ea.numero, 1, "l'attribution démarre à 1 — 0 est le public");
        assert_eq!(eb.numero, 2);

        let cat = CatalogueTenants::charger(dir.path()).unwrap();
        assert_eq!(cat.resoudre(&a), Some(1));
        assert_eq!(cat.resoudre(&b), Some(2));
        assert_eq!(cat.resoudre(&Uuid::new_v4()), None);
    }

    #[test]
    fn double_attribution_refusee() {
        let dir = tempfile::tempdir().unwrap();
        let a = Uuid::new_v4();
        CatalogueTenants::attribuer(dir.path(), a, "premier").unwrap();
        let err = CatalogueTenants::attribuer(dir.path(), a, "second").unwrap_err();
        assert!(err.to_string().contains("déjà interné"));
    }

    #[test]
    fn zero_et_sentinelle_refuses_au_chargement() {
        let dir = tempfile::tempdir().unwrap();
        let chemin = dir.path().join(FICHIER_TENANTCAT);
        std::fs::write(&chemin, format!("{}\t0\tpublic-vole\n", Uuid::new_v4())).unwrap();
        assert!(
            CatalogueTenants::charger(dir.path())
                .unwrap_err()
                .to_string()
                .contains("PUBLIC")
        );

        std::fs::write(
            &chemin,
            format!("{}\t{}\tsentinelle\n", Uuid::new_v4(), u32::MAX),
        )
        .unwrap();
        assert!(
            CatalogueTenants::charger(dir.path())
                .unwrap_err()
                .to_string()
                .contains("sentinelle")
        );
    }

    #[test]
    fn doublons_refuses_au_chargement() {
        let dir = tempfile::tempdir().unwrap();
        let chemin = dir.path().join(FICHIER_TENANTCAT);
        let u = Uuid::new_v4();
        std::fs::write(&chemin, format!("{u}\t1\tun\n{u}\t2\tdeux\n")).unwrap();
        assert!(
            CatalogueTenants::charger(dir.path())
                .unwrap_err()
                .to_string()
                .contains("déjà interné")
        );

        let (x, y) = (Uuid::new_v4(), Uuid::new_v4());
        std::fs::write(&chemin, format!("{x}\t3\tx\n{y}\t3\ty\n")).unwrap();
        assert!(
            CatalogueTenants::charger(dir.path())
                .unwrap_err()
                .to_string()
                .contains("déjà attribué")
        );
    }

    #[test]
    fn commentaires_et_lignes_vides_ignores() {
        let dir = tempfile::tempdir().unwrap();
        let u = Uuid::new_v4();
        std::fs::write(
            dir.path().join(FICHIER_TENANTCAT),
            format!("# catalogue d'essai\n\n{u}\t7\tlibellé avec espaces\n"),
        )
        .unwrap();
        let cat = CatalogueTenants::charger(dir.path()).unwrap();
        assert_eq!(cat.resoudre(&u), Some(7));
        assert_eq!(cat.entrees()[0].libelle, "libellé avec espaces");
    }

    #[test]
    fn apres_un_max_l_attribution_continue_au_dela() {
        // Jamais de réutilisation : le prochain numéro est max+1 même si des
        // numéros intermédiaires n'existent pas.
        let dir = tempfile::tempdir().unwrap();
        let u = Uuid::new_v4();
        std::fs::write(
            dir.path().join(FICHIER_TENANTCAT),
            format!("{u}\t41\tancien\n"),
        )
        .unwrap();
        let e = CatalogueTenants::attribuer(dir.path(), Uuid::new_v4(), "suivant").unwrap();
        assert_eq!(e.numero, 42);
    }
}
