//! L'automate d'une négociation agent-à-agent : gestes **typés**, états, et
//! refus nommés.
//!
//! D3 demande « des messages typés avec automate d'états (proposer, accepter,
//! refuser, contre-proposer, annuler) — pas de texte libre ». Ce module est cet
//! automate, et **rien d'autre** : il ne touche ni au réseau ni au stockage. Il
//! décide, l'appelant transporte.
//!
//! La règle qui compte : un geste est refusé soit parce que l'état ne
//! l'autorise pas (rien à conclure, déjà en cours, négociation close), soit
//! parce que l'auteur sort de son rôle — on n'accepte pas sa propre
//! proposition, on n'annule pas celle d'un autre. Chaque refus est **nommé**
//! ([`Refus`]), jamais un booléen muet.

use crate::DeviceId;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

/// Le geste d'un agent. Sérialisé en `snake_case` (`"proposer"`, `"accepter"`…),
/// jamais en texte libre.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Geste {
    Proposer,
    Accepter,
    Refuser,
    ContreProposer,
    Annuler,
}

impl Geste {
    /// Lit un geste dans la charge d'un message : `{"geste": "accepter"}`.
    /// `None` si le champ manque, n'est pas une chaîne, ou n'est pas un geste
    /// connu — un texte libre n'est **pas** un geste.
    pub fn du_charge(charge: &Value) -> Option<Self> {
        let s = charge.get("geste")?.as_str()?;
        serde_json::from_value(Value::String(s.to_owned())).ok()
    }

    /// Nom sur le fil (identique au `snake_case` de la sérialisation).
    pub fn wire(self) -> &'static str {
        match self {
            Self::Proposer => "proposer",
            Self::Accepter => "accepter",
            Self::Refuser => "refuser",
            Self::ContreProposer => "contre_proposer",
            Self::Annuler => "annuler",
        }
    }
}

/// L'état d'une négociation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Etat {
    /// Aucune proposition encore : l'une ou l'autre partie peut ouvrir.
    Ouverte,
    /// Une proposition attend une réponse.
    Proposee,
    /// Une contre-proposition attend une réponse.
    ContreProposee,
    /// Conclue par un accord.
    Acceptee,
    /// Conclue par un refus.
    Refusee,
    /// Annulée par son proposant.
    Annulee,
}

impl Etat {
    /// Vrai quand la négociation ne peut plus bouger : accord, refus, annulation.
    pub fn est_conclue(self) -> bool {
        matches!(self, Etat::Acceptee | Etat::Refusee | Etat::Annulee)
    }
}

/// Pourquoi un geste est refusé. Nommé, pour que le refus soit diagnosticable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refus {
    /// L'auteur n'est pas l'une des deux parties de la négociation.
    PasUnePartie,
    /// Proposer alors qu'une proposition attend déjà une réponse.
    DejaEnCours,
    /// Conclure (accepter, refuser, contre-proposer, annuler) alors que rien
    /// n'est en cours.
    RienAConclure,
    /// La négociation est close : plus rien ne s'y fait.
    Conclue,
    /// Le geste n'est pas dans le rôle de l'auteur : accepter sa propre
    /// proposition, ou annuler celle d'un autre.
    PasVotreTour,
}

/// Une négociation entre deux appareils connus.
///
/// `partie_a` et `partie_b` sont fixes (les deux agents) ; `auteur` est qui a
/// fait la proposition **en cours** — il peut annuler, l'autre peut répondre.
#[derive(Debug, Clone)]
pub struct Negociation {
    id: Uuid,
    partie_a: DeviceId,
    partie_b: DeviceId,
    etat: Etat,
    auteur: Option<DeviceId>,
}

impl Negociation {
    pub fn new(id: Uuid, partie_a: DeviceId, partie_b: DeviceId) -> Self {
        Self {
            id,
            partie_a,
            partie_b,
            etat: Etat::Ouverte,
            auteur: None,
        }
    }

    pub fn id(&self) -> Uuid {
        self.id
    }
    pub fn etat(&self) -> Etat {
        self.etat
    }
    pub fn est_conclue(&self) -> bool {
        self.etat.est_conclue()
    }
    /// L'auteur de la proposition en cours, s'il y en a une.
    pub fn auteur(&self) -> Option<&DeviceId> {
        self.auteur.as_ref()
    }

    /// Applique un geste de la part de `par` et rend le nouvel état, ou nomme
    /// le refus. **N'écrit rien** : l'appelant décide quoi faire d'un refus.
    pub fn appliquer(&mut self, geste: Geste, par: &DeviceId) -> Result<Etat, Refus> {
        if par != &self.partie_a && par != &self.partie_b {
            return Err(Refus::PasUnePartie);
        }
        match (self.etat, geste) {
            (Etat::Ouverte, Geste::Proposer) => {
                self.auteur = Some(par.clone());
                self.etat = Etat::Proposee;
                Ok(self.etat)
            }
            (Etat::Proposee | Etat::ContreProposee, geste) => {
                let par_proposant = self.auteur.as_ref() == Some(par);
                match geste {
                    Geste::Accepter if !par_proposant => self.etat = Etat::Acceptee,
                    Geste::Refuser if !par_proposant => self.etat = Etat::Refusee,
                    Geste::ContreProposer if !par_proposant => {
                        // La contre-proposition change d'auteur : c'est
                        // maintenant à l'autre de répondre.
                        self.auteur = Some(par.clone());
                        self.etat = Etat::ContreProposee;
                    }
                    Geste::Annuler if par_proposant => self.etat = Etat::Annulee,
                    // Proposer pendant qu'une proposition attend : refusé.
                    Geste::Proposer => return Err(Refus::DejaEnCours),
                    // Accepter/refuser/contre-proposer SA propre proposition,
                    // ou annuler celle d'un autre : hors rôle.
                    _ => return Err(Refus::PasVotreTour),
                }
                Ok(self.etat)
            }
            (Etat::Ouverte, _) => Err(Refus::RienAConclure),
            // Tout état conclusif : plus rien ne s'y fait.
            _ => Err(Refus::Conclue),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn dev(c: char) -> DeviceId {
        DeviceId::parse(&format!("device-0000000{c}")).expect("id")
    }
    fn a() -> DeviceId {
        dev('a')
    }
    fn b() -> DeviceId {
        dev('b')
    }
    fn nego() -> Negociation {
        Negociation::new(Uuid::nil(), a(), b())
    }

    #[test]
    fn proposer_ouvre_la_negociation() {
        let mut n = nego();
        assert_eq!(n.etat(), Etat::Ouverte);
        assert_eq!(n.appliquer(Geste::Proposer, &a()).unwrap(), Etat::Proposee);
        assert_eq!(n.auteur(), Some(&a()));
        assert!(!n.est_conclue());
    }

    #[test]
    fn l_autre_partie_accepte() {
        let mut n = nego();
        n.appliquer(Geste::Proposer, &a()).unwrap();
        assert_eq!(n.appliquer(Geste::Accepter, &b()).unwrap(), Etat::Acceptee);
        assert!(n.est_conclue());
    }

    #[test]
    fn l_autre_partie_refuse() {
        let mut n = nego();
        n.appliquer(Geste::Proposer, &a()).unwrap();
        assert_eq!(n.appliquer(Geste::Refuser, &b()).unwrap(), Etat::Refusee);
        assert!(n.est_conclue());
    }

    #[test]
    fn contre_proposition_puis_accord() {
        let mut n = nego();
        n.appliquer(Geste::Proposer, &a()).unwrap();
        // B contre-propose : l'auteur devient B, c'est à A de répondre.
        assert_eq!(
            n.appliquer(Geste::ContreProposer, &b()).unwrap(),
            Etat::ContreProposee
        );
        assert_eq!(n.auteur(), Some(&b()));
        // A accepte la contre-proposition.
        assert_eq!(n.appliquer(Geste::Accepter, &a()).unwrap(), Etat::Acceptee);
    }

    #[test]
    fn le_proposant_annule_sa_proposition() {
        let mut n = nego();
        n.appliquer(Geste::Proposer, &a()).unwrap();
        assert_eq!(n.appliquer(Geste::Annuler, &a()).unwrap(), Etat::Annulee);
    }

    #[test]
    fn accepter_sa_propre_proposition_est_refuse() {
        let mut n = nego();
        n.appliquer(Geste::Proposer, &a()).unwrap();
        assert_eq!(n.appliquer(Geste::Accepter, &a()), Err(Refus::PasVotreTour));
        // L'état n'a pas bougé.
        assert_eq!(n.etat(), Etat::Proposee);
    }

    #[test]
    fn annuler_la_proposition_d_un_autre_est_refuse() {
        let mut n = nego();
        n.appliquer(Geste::Proposer, &a()).unwrap();
        assert_eq!(n.appliquer(Geste::Annuler, &b()), Err(Refus::PasVotreTour));
    }

    #[test]
    fn proposer_pendant_une_proposition_est_refuse() {
        let mut n = nego();
        n.appliquer(Geste::Proposer, &a()).unwrap();
        assert_eq!(n.appliquer(Geste::Proposer, &b()), Err(Refus::DejaEnCours));
    }

    #[test]
    fn conclure_sans_proposition_est_refuse() {
        let mut n = nego();
        for g in [
            Geste::Accepter,
            Geste::Refuser,
            Geste::ContreProposer,
            Geste::Annuler,
        ] {
            assert_eq!(n.appliquer(g, &a()), Err(Refus::RienAConclure));
        }
    }

    #[test]
    fn rien_ne_bouge_apres_conclusion() {
        let mut n = nego();
        n.appliquer(Geste::Proposer, &a()).unwrap();
        n.appliquer(Geste::Accepter, &b()).unwrap();
        for g in [
            Geste::Proposer,
            Geste::Accepter,
            Geste::Refuser,
            Geste::ContreProposer,
            Geste::Annuler,
        ] {
            assert_eq!(n.appliquer(g, &a()), Err(Refus::Conclue));
            assert_eq!(n.appliquer(g, &b()), Err(Refus::Conclue));
        }
    }

    #[test]
    fn un_tiers_ne_peut_rien() {
        let mut n = nego();
        let tiers = dev('c');
        for g in [
            Geste::Proposer,
            Geste::Accepter,
            Geste::Refuser,
            Geste::ContreProposer,
            Geste::Annuler,
        ] {
            assert_eq!(n.appliquer(g, &tiers), Err(Refus::PasUnePartie));
        }
    }

    #[test]
    fn la_charge_type_le_geste_et_le_texte_libre_est_refuse() {
        assert_eq!(
            Geste::du_charge(&json!({"geste": "accepter"})),
            Some(Geste::Accepter)
        );
        assert_eq!(
            Geste::du_charge(&json!({"geste": "contre_proposer"})),
            Some(Geste::ContreProposer)
        );
        // Pas de texte libre : une phrase n'est pas un geste.
        assert_eq!(
            Geste::du_charge(&json!({"geste": "je veux bien merci"})),
            None
        );
        assert_eq!(Geste::du_charge(&json!({"texte": "accepter"})), None);
        assert_eq!(Geste::du_charge(&json!(42)), None);
    }

    #[test]
    fn le_nom_sur_le_fil_est_stable() {
        assert_eq!(Geste::Proposer.wire(), "proposer");
        assert_eq!(Geste::ContreProposer.wire(), "contre_proposer");
        assert_eq!(Geste::Annuler.wire(), "annuler");
    }
}
