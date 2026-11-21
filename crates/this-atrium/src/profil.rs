//! Profils d'accès : un bundle de grants, évalué **deny par défaut**, en Rust pur.
//!
//! Un profil dit ce qu'un pair peut faire : une liste de [`Grant`], chacun portant un
//! service, une opération, la portée où il s'applique et un quota optionnel. Rien de plus
//! — aucun stockage, aucune I/O réseau, jamais de panique. La règle du socle s'applique
//! telle quelle : ce qui n'est pas explicitement accordé est refusé.

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
    /// Vrai si ce grant couvre le couple `(service, operation)` demandé.
    ///
    /// Un `grant.service` (resp. `grant.operation`) valant [`JOKER`] couvre n'importe
    /// quelle valeur — mais seulement dans ce sens : demander `"*"` n'est couvert que par
    /// un grant qui vaut lui aussi `"*"`.
    fn couvre(&self, service: &str, operation: &str) -> bool {
        let service_ok = self.service == JOKER || self.service == service;
        let operation_ok = self.operation == JOKER || self.operation == operation;
        service_ok && operation_ok
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

    /// Vrai si **au moins un** grant couvre le couple demandé.
    ///
    /// Deny par défaut : un profil sans grant, ou dont aucun grant ne couvre la demande,
    /// rend `false`.
    pub fn autorise(&self, service: &str, operation: &str) -> bool {
        self.grant(service, operation).is_some()
    }

    /// Le **premier** grant qui couvre la demande (dans l'ordre où les grants sont
    /// déclarés) — pour lire sa [`Portee`] et son `quota`. `None` si aucun ne couvre.
    pub fn grant(&self, service: &str, operation: &str) -> Option<&Grant> {
        self.grants.iter().find(|g| g.couvre(service, operation))
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

    fn profil(grants: Vec<Grant>) -> Profil {
        Profil::nouveau("test", grants)
    }

    #[test]
    fn un_grant_exact_autorise_le_couple_exact() {
        let p = profil(vec![grant("liste", "lire")]);
        assert!(p.autorise("liste", "lire"));
    }

    #[test]
    fn le_joker_sur_le_service_couvre_tout_service() {
        let p = profil(vec![grant(JOKER, "lire")]);
        assert!(p.autorise("liste", "lire"));
        assert!(p.autorise("photos", "lire"));
        // L'opération, elle, reste bornée : le joker ne portait que sur le service.
        assert!(!p.autorise("liste", "vider"));
    }

    #[test]
    fn le_joker_sur_l_operation_couvre_toute_operation() {
        let p = profil(vec![grant("liste", JOKER)]);
        assert!(p.autorise("liste", "lire"));
        assert!(p.autorise("liste", "vider"));
        assert!(!p.autorise("photos", "lire"));
    }

    #[test]
    fn un_profil_vide_refuse_tout() {
        let p = profil(Vec::new());
        assert!(!p.autorise("liste", "lire"));
        assert!(!p.autorise(JOKER, JOKER));
        assert_eq!(p.grant("liste", "lire"), None);
    }

    #[test]
    fn un_service_ou_une_operation_non_listee_est_refuse() {
        let p = profil(vec![grant("liste", "lire")]);
        assert!(!p.autorise("photos", "lire"), "service non listé");
        assert!(!p.autorise("liste", "vider"), "opération non listée");
        // Le joker n'est pas une demande « universelle » : il ne matche que le joker.
        assert!(!p.autorise(JOKER, JOKER));
    }

    #[test]
    fn grant_rend_la_portee_et_le_quota_du_bon_grant() {
        let p = profil(vec![
            Grant {
                service: "liste".to_string(),
                operation: "lire".to_string(),
                portee: Portee::Nommee("famille".to_string()),
                quota: Some(1_000),
            },
            Grant {
                service: "liste".to_string(),
                operation: "vider".to_string(),
                portee: Portee::Tout,
                quota: None,
            },
        ]);

        let g = p.grant("liste", "lire").expect("le grant « lire » existe");
        assert_eq!(g.portee, Portee::Nommee("famille".to_string()));
        assert_eq!(g.quota, Some(1_000));

        let g = p
            .grant("liste", "vider")
            .expect("le grant « vider » existe");
        assert_eq!(g.portee, Portee::Tout);
        assert_eq!(g.quota, None);

        assert!(p.grant("liste", "ecrire").is_none());
    }

    #[test]
    fn grant_rend_le_premier_grant_qui_couvre() {
        // Le wildcard est déclaré en premier : il gagne sur le grant exact qui suit.
        let p = profil(vec![
            Grant {
                service: JOKER.to_string(),
                operation: JOKER.to_string(),
                portee: Portee::Tout,
                quota: Some(5),
            },
            Grant {
                service: "liste".to_string(),
                operation: "lire".to_string(),
                portee: Portee::Nommee("proche".to_string()),
                quota: Some(99),
            },
        ]);

        let g = p.grant("liste", "lire").expect("couvert par le joker");
        assert_eq!(g.quota, Some(5));
        assert_eq!(g.portee, Portee::Tout);
    }

    #[test]
    fn un_profil_se_relit_apres_serde() {
        let p = profil(vec![Grant {
            service: "liste".to_string(),
            operation: JOKER.to_string(),
            portee: Portee::Nommee("famille".to_string()),
            quota: Some(42),
        }]);

        let json = serde_json::to_string(&p).unwrap();
        let relu: Profil = serde_json::from_str(&json).unwrap();
        assert_eq!(p, relu);
        assert!(relu.autorise("liste", "vider"));
    }
}
