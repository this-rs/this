//! Profils d'accès : un bundle de grants, évalué **deny par défaut**, en Rust pur.
//!
//! Un profil dit ce qu'un pair peut faire : une liste de [`Grant`], chacun portant un
//! service, une opération, la portée où il s'applique et un quota optionnel. Rien de plus
//! — aucun stockage, aucune I/O réseau, jamais de panique. La règle du socle s'applique
//! telle quelle : ce qui n'est pas explicitement accordé est refusé.
//!
//! ## La portée n'était pas vérifiée — corrigé le 27/11/2026
//!
//! `Grant::couvre` ne comparait que `service` et `operation`. Un grant déclaré
//! `Portee::Nommee("famille")` autorisait donc **n'importe quelle** portée, y compris
//! `Portee::Tout` : la portée était décorative, et un profil ne pouvait pas être
//! « rétréci ». [`Grant::couvre`] prend maintenant la portée demandée, et
//! [`Portee::couvre`] dit la règle : `Tout` couvre tout, un nom ne couvre que lui-même.
//!
//! C'est ce qui rend l'**héritage** exprimable : [`Profil::contient`] (l'ordre partiel) et
//! [`Profil::intersection`] (le rétrécissement) sont les deux opérations dont un niveau
//! d'accès hérité a besoin. Sans elles, « le sous-agent ne peut pas dépasser son parent »
//! resterait une phrase.

use serde::{Deserialize, Serialize};

/// Le joker : un `service` ou une `operation` qui vaut `"*"` couvre n'importe quelle
/// valeur. C'est la seule forme de wildcard, et elle est toujours **explicite**.
pub const JOKER: &str = "*";

/// La portée d'un grant : tout ce que le service expose, ou un sous-ensemble nommé.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Portee {
    /// La totalité de ce que le service expose.
    Tout,
    /// Un sous-ensemble nommé (un salon, un dossier, un espace…).
    Nommee(String),
}

impl Portee {
    /// Vrai si cette portée couvre celle qui est demandée.
    ///
    /// `Tout` couvre tout ; un nom ne couvre QUE lui-même. Il n'y a pas de hiérarchie de
    /// noms : `Nommee("famille")` ne couvre pas `Nommee("famille/proche")` — une hiérarchie
    /// implicite serait un élargissement silencieux, et un élargissement silencieux est
    /// exactement ce que ce module existe pour empêcher.
    pub fn couvre(&self, demandee: &Portee) -> bool {
        match (self, demandee) {
            (Portee::Tout, _) => true,
            (Portee::Nommee(_), Portee::Tout) => false,
            (Portee::Nommee(a), Portee::Nommee(b)) => a == b,
        }
    }

    /// La portée commune, ou `None` si les deux n'ont rien en commun.
    pub fn intersection(&self, autre: &Portee) -> Option<Portee> {
        match (self, autre) {
            (Portee::Tout, autre) => Some(autre.clone()),
            (moi, Portee::Tout) => Some(moi.clone()),
            (Portee::Nommee(a), Portee::Nommee(b)) if a == b => Some(Portee::Nommee(a.clone())),
            _ => None,
        }
    }
}

/// Un droit accordé : sur quel `service`, quelle `operation`, dans quelle `portee`, et
/// avec éventuellement un `quota`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Grant {
    pub service: String,
    pub operation: String,
    pub portee: Portee,
    pub quota: Option<u64>,
}

impl Grant {
    /// Vrai si ce grant couvre le triplet `(service, operation, portee)` demandé.
    ///
    /// Un `grant.service` (resp. `grant.operation`) valant [`JOKER`] couvre n'importe
    /// quelle valeur — mais seulement dans ce sens : demander `"*"` n'est couvert que par
    /// un grant qui vaut lui aussi `"*"`. La portée suit [`Portee::couvre`].
    fn couvre(&self, service: &str, operation: &str, portee: &Portee) -> bool {
        let service_ok = self.service == JOKER || self.service == service;
        let operation_ok = self.operation == JOKER || self.operation == operation;
        service_ok && operation_ok && self.portee.couvre(portee)
    }

    /// Vrai si ce grant couvre ENTIÈREMENT cet autre grant — service, opération, portée et
    /// quota compris.
    ///
    /// C'est la brique de l'inclusion : un octroi plus large couvre l'octroi plus étroit,
    /// et un quota plus généreux couvre un quota plus serré.
    fn couvre_grant(&self, autre: &Grant) -> bool {
        // Un JOKER dans `autre` n'est couvert que par un JOKER : on ne peut pas promettre
        // « tout » quand on ne détient qu'un nom.
        let service_ok = self.service == JOKER || self.service == autre.service;
        let operation_ok = self.operation == JOKER || self.operation == autre.operation;
        let quota_ok = match (self.quota, autre.quota) {
            (None, _) => true,
            (Some(_), None) => false,
            (Some(mien), Some(sien)) => mien >= sien,
        };
        service_ok && operation_ok && self.portee.couvre(&autre.portee) && quota_ok
    }
}

/// Un profil d'accès : un nom et les grants qu'il accorde.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Profil {
    pub nom: String,
    pub grants: Vec<Grant>,
}

impl Profil {
    /// Un profil nommé et ses grants.
    pub fn nouveau(nom: impl Into<String>, grants: Vec<Grant>) -> Self {
        Self {
            nom: nom.into(),
            grants,
        }
    }

    /// Vrai si **au moins un** grant couvre le triplet demandé.
    ///
    /// Deny par défaut : un profil sans grant, ou dont aucun grant ne couvre la demande,
    /// rend `false`.
    pub fn autorise(&self, service: &str, operation: &str, portee: &Portee) -> bool {
        self.grant(service, operation, portee).is_some()
    }

    /// Le **premier** grant qui couvre la demande (dans l'ordre où les grants sont
    /// déclarés) — pour lire sa [`Portee`] et son `quota`. `None` si aucun ne couvre.
    pub fn grant(&self, service: &str, operation: &str, portee: &Portee) -> Option<&Grant> {
        self.grants
            .iter()
            .find(|g| g.couvre(service, operation, portee))
    }

    /// Vrai si ce profil **contient** l'autre : tout ce que l'autre accorde, celui-ci
    /// l'accorde aussi.
    ///
    /// C'est l'ordre partiel dont dépend un héritage : `parent.contient(&enfant)` est la
    /// preuve que l'enfant ne dépasse pas son parent. Un profil vide est contenu par tout
    /// le monde ; deux profils qui ne se recouvrent pas ne se contiennent pas.
    pub fn contient(&self, autre: &Profil) -> bool {
        autre
            .grants
            .iter()
            .all(|g| self.grants.iter().any(|mien| mien.couvre_grant(g)))
    }

    /// Le rétrécissement : ce que les deux accordent à la fois.
    ///
    /// Chaque grant du résultat est l'intersection d'un grant de `self` et d'un grant de
    /// `autre` qui se recouvrent — service, opération, portée (par [`Portee::intersection`])
    /// et quota (le plus serré). Deux grants dont les portées nommées diffèrent ne donnent
    /// rien : il n'y a pas de hiérarchie de noms à inventer.
    ///
    /// Le nom du résultat est `self.nom ∩ autre.nom` : un profil rétréci n'est pas un
    /// profil qui existait avant, et le nom le dit.
    pub fn intersection(&self, autre: &Profil) -> Profil {
        let mut grants: Vec<Grant> = Vec::new();
        for mien in &self.grants {
            for sien in &autre.grants {
                let (Some(service), Some(operation), Some(portee)) = (
                    intersection_simple(&mien.service, &sien.service),
                    intersection_simple(&mien.operation, &sien.operation),
                    mien.portee.intersection(&sien.portee),
                ) else {
                    continue;
                };
                let quota = match (mien.quota, sien.quota) {
                    (Some(a), Some(b)) => Some(a.min(b)),
                    (Some(a), None) => Some(a),
                    (None, Some(b)) => Some(b),
                    (None, None) => None,
                };
                grants.push(Grant {
                    service,
                    operation,
                    portee,
                    quota,
                });
            }
        }
        Profil::nouveau(format!("{}∩{}", self.nom, autre.nom), grants)
    }
}

/// L'intersection de deux noms de service (ou d'opération), avec le joker explicite.
fn intersection_simple(a: &str, b: &str) -> Option<String> {
    match (a, b) {
        (JOKER, autre) | (autre, JOKER) => Some(autre.to_string()),
        (x, y) if x == y => Some(x.to_string()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn grant(service: &str, operation: &str) -> Grant {
        Grant {
            service: service.to_string(),
            operation: operation.to_string(),
            portee: Portee::Tout,
            quota: None,
        }
    }

    fn nomme(service: &str, operation: &str, portee: &str) -> Grant {
        Grant {
            service: service.to_string(),
            operation: operation.to_string(),
            portee: Portee::Nommee(portee.to_string()),
            quota: None,
        }
    }

    fn profil(grants: Vec<Grant>) -> Profil {
        Profil::nouveau("test", grants)
    }

    #[test]
    fn un_grant_exact_autorise_le_couple_exact() {
        let p = profil(vec![grant("liste", "lire")]);
        assert!(p.autorise("liste", "lire", &Portee::Tout));
    }

    #[test]
    fn le_joker_sur_le_service_couvre_tout_service() {
        let p = profil(vec![grant(JOKER, "lire")]);
        assert!(p.autorise("liste", "lire", &Portee::Tout));
        assert!(p.autorise("photos", "lire", &Portee::Tout));
        // L'opération, elle, reste bornée : le joker ne portait que sur le service.
        assert!(!p.autorise("liste", "vider", &Portee::Tout));
    }

    #[test]
    fn le_joker_sur_l_operation_couvre_toute_operation() {
        let p = profil(vec![grant("liste", JOKER)]);
        assert!(p.autorise("liste", "lire", &Portee::Tout));
        assert!(p.autorise("liste", "vider", &Portee::Tout));
        assert!(!p.autorise("photos", "lire", &Portee::Tout));
    }

    #[test]
    fn un_profil_vide_refuse_tout() {
        let p = profil(Vec::new());
        assert!(!p.autorise("liste", "lire", &Portee::Tout));
        assert!(!p.autorise(JOKER, JOKER, &Portee::Tout));
        assert_eq!(p.grant("liste", "lire", &Portee::Tout), None);
    }

    #[test]
    fn un_service_ou_une_operation_non_listee_est_refuse() {
        let p = profil(vec![grant("liste", "lire")]);
        assert!(
            !p.autorise("photos", "lire", &Portee::Tout),
            "service non listé"
        );
        assert!(
            !p.autorise("liste", "vider", &Portee::Tout),
            "opération non listée"
        );
        // Le joker n'est pas une demande « universelle » : il ne matche que le joker.
        assert!(!p.autorise(JOKER, JOKER, &Portee::Tout));
    }

    #[test]
    fn la_portee_est_verifiee_elle_aussi() {
        // Le défaut mesuré le 27/11 : cette assertion ÉCHOUAIT avant la correction.
        let p = profil(vec![nomme("liste", "lire", "famille")]);
        assert!(p.autorise("liste", "lire", &Portee::Nommee("famille".into())));
        assert!(
            !p.autorise("liste", "lire", &Portee::Nommee("travail".into())),
            "un nom ne couvre pas un autre nom"
        );
        assert!(
            !p.autorise("liste", "lire", &Portee::Tout),
            "un nom ne couvre jamais « tout »"
        );
        // Et l'inverse est vrai : « tout » couvre un nom.
        let tout = profil(vec![grant("liste", "lire")]);
        assert!(tout.autorise("liste", "lire", &Portee::Nommee("famille".into())));
    }

    #[test]
    fn un_nom_ne_couvre_pas_un_nom_qui_le_prefixe() {
        // Pas de hiérarchie implicite : elle serait un élargissement silencieux.
        assert!(!Portee::Nommee("famille".into()).couvre(&Portee::Nommee("famille/proche".into())));
        assert!(!Portee::Nommee("famille/proche".into()).couvre(&Portee::Nommee("famille".into())));
    }

    #[test]
    fn grant_rend_la_portee_et_le_quota_du_bon_grant() {
        let mut lire = nomme("liste", "lire", "famille");
        lire.quota = Some(1_000);
        let p = profil(vec![lire, grant("liste", "vider")]);

        let g = p
            .grant("liste", "lire", &Portee::Nommee("famille".into()))
            .expect("le grant « lire » existe");
        assert_eq!(g.portee, Portee::Nommee("famille".to_string()));
        assert_eq!(g.quota, Some(1_000));

        let g = p
            .grant("liste", "vider", &Portee::Tout)
            .expect("le grant « vider » existe");
        assert_eq!(g.portee, Portee::Tout);
        assert_eq!(g.quota, None);

        assert!(p.grant("liste", "ecrire", &Portee::Tout).is_none());
    }

    #[test]
    fn grant_rend_le_premier_grant_qui_couvre() {
        // Le wildcard est déclaré en premier : il gagne sur le grant exact qui suit.
        let mut large = grant(JOKER, JOKER);
        large.quota = Some(5);
        let mut exact = nomme("liste", "lire", "proche");
        exact.quota = Some(99);
        let p = profil(vec![large, exact]);

        let g = p
            .grant("liste", "lire", &Portee::Nommee("proche".into()))
            .expect("couvert par le joker");
        assert_eq!(g.quota, Some(5));
        assert_eq!(g.portee, Portee::Tout);
    }

    #[test]
    fn un_profil_se_relit_apres_serde() {
        let mut g = nomme("liste", JOKER, "famille");
        g.quota = Some(42);
        let p = profil(vec![g]);

        let json = serde_json::to_string(&p).unwrap();
        let relu: Profil = serde_json::from_str(&json).unwrap();
        assert_eq!(p, relu);
        assert!(relu.autorise("liste", "vider", &Portee::Nommee("famille".into())));
    }

    // ---- L'ordre partiel et le rétrécissement : ce dont un héritage a besoin ----------

    #[test]
    fn un_profil_plus_large_contient_un_profil_plus_etroit() {
        let large = profil(vec![grant("liste", JOKER)]);
        let etroit = profil(vec![grant("liste", "lire")]);
        assert!(large.contient(&etroit));
        assert!(
            !etroit.contient(&large),
            "un JOKER ne s'obtient pas d'un nom"
        );
    }

    #[test]
    fn tout_profil_contient_le_profil_vide_et_le_vide_ne_contient_que_lui() {
        let vide = profil(Vec::new());
        let plein = profil(vec![grant("liste", "lire")]);
        assert!(plein.contient(&vide));
        assert!(!vide.contient(&plein));
        assert!(vide.contient(&vide));
    }

    #[test]
    fn une_portee_nommee_ne_contient_ni_tout_ni_un_autre_nom() {
        let famille = profil(vec![nomme("liste", "lire", "famille")]);
        let tout = profil(vec![grant("liste", "lire")]);
        let travail = profil(vec![nomme("liste", "lire", "travail")]);
        assert!(tout.contient(&famille));
        assert!(!famille.contient(&tout));
        assert!(!famille.contient(&travail));
        assert!(!travail.contient(&famille));
    }

    #[test]
    fn un_quota_plus_serre_est_contient_par_un_quota_plus_large() {
        let mut large = grant("liste", "lire");
        large.quota = Some(100);
        let mut serre = grant("liste", "lire");
        serre.quota = Some(10);
        assert!(profil(vec![large.clone()]).contient(&profil(vec![serre.clone()])));
        assert!(!profil(vec![serre]).contient(&profil(vec![large])));
    }

    #[test]
    fn le_retrecissement_garde_ce_que_les_deux_accordent() {
        let parent = profil(vec![
            grant("liste", JOKER),
            nomme("espace", "lire", "famille"),
            grant("modules", "lister"),
        ]);
        let demande = profil(vec![
            grant("liste", "lire"),
            nomme("espace", "lire", "travail"),
            grant("forge", "publier"),
        ]);
        let enfant = parent.intersection(&demande);

        // Gardé : le service que les deux accordent.
        assert!(enfant.autorise("liste", "lire", &Portee::Tout));
        // Perdu : la portée « travail » n'était pas détenue par le parent.
        assert!(!enfant.autorise("espace", "lire", &Portee::Nommee("travail".into())));
        // Perdu : le parent ne détenait pas la forge.
        assert!(!enfant.autorise("forge", "publier", &Portee::Tout));
        // Le nom dit qu'il est le produit de deux.
        assert_eq!(enfant.nom, "test∩test");
    }

    #[test]
    fn le_retrecissement_est_idempotent_et_borne_par_le_parent() {
        let parent = profil(vec![grant("liste", "lire")]);
        let enfant = parent.intersection(&parent);
        assert!(parent.contient(&enfant));
        assert!(
            enfant.contient(&parent),
            "l'intersection d'un profil avec lui-même est lui-même"
        );
    }

    #[test]
    fn un_enfant_ne_depasse_jamais_son_parent_quel_que_soit_le_jeu() {
        // Le contrôle qui compte : pour un profil demandé PLUS LARGE que le parent,
        // l'intersection ne dépasse pas le parent.
        let parent = profil(vec![grant("liste", "lire")]);
        let demande = profil(vec![grant(JOKER, JOKER), grant("forge", "publier")]);
        let enfant = parent.intersection(&demande);
        assert!(
            parent.contient(&enfant),
            "l'enfant doit rester dans l'enveloppe du parent"
        );
        assert!(!enfant.autorise("forge", "publier", &Portee::Tout));
        assert!(!enfant.autorise("photos", "lire", &Portee::Tout));
    }
}
