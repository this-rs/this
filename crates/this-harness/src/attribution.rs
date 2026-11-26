//! **Primitive commune du cycle logique d'attribution** — indépendante du backend
//! de persistance (RAM pour ce premier incrément).
//!
//! Le contrat : une décision enregistrée (`record`) porte une **définition**
//! (identité cognitive, fournisseur, modèle) qui ne peut être écrasée silencieusement ;
//! une observation reçue est **conservée** jusqu'à sa confirmation ; l'attribution
//! survit à une erreur récupérable et à une indétermination ; le cycle ne se clôt
//! que sur **confirmation** ou **abandon explicite**.
//!
//! La confirmation logique (`Confirme`) n'est PAS la durabilité : cette dernière
//! relève d'Obrain, pas de cette primitive.

use std::collections::HashMap;
use std::collections::hash_map::Entry;

/// La définition d'une décision : ce qui doit rester identique pour qu'un rejeu
/// soit reconnu (et divergent → conflit, sans écrasement).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Definition {
    /// Identité cognitive constituée (`None` = pas de connaissance constituée).
    pub identite: Option<String>,
    /// Paire effectivement servie (`(fournisseur, modèle)`).
    pub servie: Option<(String, String)>,
}

/// Un résultat observé — jamais confondu avec le succès de transport.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Resultat {
    Succes,
    Echec,
    /// Résultat inconnu (transport coupé, pas de verdict).
    Inconnu,
}

/// Une observation reçue, pas encore confirmée.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Observation {
    pub fournisseur: String,
    pub modele: String,
    pub resultat: Resultat,
}

/// L'état du cycle d'attribution d'une décision.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Etat {
    /// Décision enregistrée, pas d'observation.
    Ouvert,
    /// Observation reçue, en attente de confirmation (ou erreur/indétermination).
    EnAttente,
    /// Confirmation logique : observation consolidée, cycle terminal.
    Confirme,
    /// Abandon explicite : état terminal identifiable.
    Abandonne,
    /// Conflit : un `record` divergent a été détecté (la définition originale
    /// est conservée, l'intrus est refusé).
    Conflit,
}

#[derive(Clone, Debug)]
pub struct Cycle {
    pub definition: Definition,
    pub observation: Option<Observation>,
    pub etat: Etat,
}

/// Issue d'un `record`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Record {
    /// Première occurrence de cet identifiant.
    Enregistre,
    /// Même identifiant, même définition : rejeu idempotent.
    Rejeu,
    /// Même identifiant, définition divergente : refusé, l'original est conservé.
    Conflit,
}

/// Issue d'une réception d'observation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Reception {
    /// Observation reçue et conservée.
    Recu,
    /// Aucun `record` pour cet identifiant : no-op.
    Absent,
}

#[derive(Default)]
pub struct Attribution {
    cycles: HashMap<u128, Cycle>,
}

impl Attribution {
    pub fn new() -> Self {
        Self::default()
    }

    /// Enregistre la définition d'une décision. Jamais d'écrasement silencieux :
    /// un identifiant déjà présent avec une définition différente est un conflit.
    pub fn record(&mut self, id: u128, definition: Definition) -> Record {
        match self.cycles.entry(id) {
            Entry::Vacant(e) => {
                e.insert(Cycle {
                    definition,
                    observation: None,
                    etat: Etat::Ouvert,
                });
                Record::Enregistre
            }
            Entry::Occupied(mut e) => {
                if e.get().definition == definition {
                    Record::Rejeu
                } else {
                    // Marquer le conflit sans écraser la définition originale.
                    e.get_mut().etat = Etat::Conflit;
                    Record::Conflit
                }
            }
        }
    }

    /// Reçoit et conserve l'observation, sans fermer le cycle.
    pub fn recevoir(&mut self, id: u128, observation: Observation) -> Reception {
        match self.cycles.get_mut(&id) {
            None => Reception::Absent,
            Some(c) => {
                c.observation = Some(observation);
                c.etat = Etat::EnAttente;
                Reception::Recu
            }
        }
    }

    /// Confirme l'observation : cycle terminal. Idempotent.
    pub fn confirmer(&mut self, id: u128) -> bool {
        match self.cycles.get_mut(&id) {
            Some(c) if c.etat == Etat::EnAttente || c.etat == Etat::Ouvert => {
                c.etat = Etat::Confirme;
                true
            }
            Some(_) => false,
            None => false,
        }
    }

    /// Marque une indétermination : l'attribution et l'observation SONT conservées.
    pub fn indetermine(&mut self, id: u128) {
        if let Some(c) = self.cycles.get_mut(&id)
            && c.etat != Etat::Confirme
            && c.etat != Etat::Abandonne
        {
            c.etat = Etat::EnAttente;
        }
    }

    /// Abandon explicite : état terminal identifiable.
    pub fn abandonner(&mut self, id: u128) -> bool {
        match self.cycles.get_mut(&id) {
            Some(c) if c.etat != Etat::Confirme => {
                c.etat = Etat::Abandonne;
                true
            }
            _ => false,
        }
    }

    /// État courant du cycle d'une décision.
    pub fn etat(&self, id: u128) -> Option<&Cycle> {
        self.cycles.get(&id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn def(identite: Option<&str>, servie: Option<(&str, &str)>) -> Definition {
        Definition {
            identite: identite.map(str::to_string),
            servie: servie.map(|(p, m)| (p.to_string(), m.to_string())),
        }
    }

    fn obs(p: &str, m: &str, r: Resultat) -> Observation {
        Observation {
            fournisseur: p.to_string(),
            modele: m.to_string(),
            resultat: r,
        }
    }

    #[test]
    fn a_rejeu_et_b_conflit_sans_ecrasement() {
        let mut a = Attribution::new();
        // A — même id, même définition : rejeu.
        assert_eq!(
            a.record(1, def(Some("k1"), Some(("a", "m1")))),
            Record::Enregistre
        );
        assert_eq!(
            a.record(1, def(Some("k1"), Some(("a", "m1")))),
            Record::Rejeu
        );
        // B — même id, définition divergente : conflit, l'original est conservé.
        assert_eq!(
            a.record(1, def(Some("k2"), Some(("b", "m2")))),
            Record::Conflit
        );
        let c = a.etat(1).unwrap();
        assert_eq!(c.definition.identite.as_deref(), Some("k1"));
        assert_eq!(c.etat, Etat::Conflit);
    }

    #[test]
    fn c_settle_echoue_attribution_conservee_puis_d_confirme() {
        let mut a = Attribution::new();
        a.record(1, def(Some("k1"), Some(("a", "m1"))));
        assert_eq!(
            a.recevoir(1, obs("a", "m1", Resultat::Succes)),
            Reception::Recu
        );
        // C — erreur récupérable : l'attribution reste.
        a.indetermine(1);
        assert_eq!(a.etat(1).unwrap().etat, Etat::EnAttente);
        assert!(a.etat(1).unwrap().observation.is_some());
        // D — nouvelle tentative confirmée : observation consolidée.
        assert!(a.confirmer(1));
        assert_eq!(a.etat(1).unwrap().etat, Etat::Confirme);
    }

    #[test]
    fn e_indetermination_conserve_jusqu_a_resolution() {
        let mut a = Attribution::new();
        a.record(1, def(Some("k1"), Some(("a", "m1"))));
        a.recevoir(1, obs("a", "m1", Resultat::Inconnu));
        a.indetermine(1);
        assert_eq!(a.etat(1).unwrap().etat, Etat::EnAttente);
        a.confirmer(1);
        assert_eq!(a.etat(1).unwrap().etat, Etat::Confirme);
    }

    #[test]
    fn f_entrelacees_reglement_inverse_tuples_exacts() {
        let mut a = Attribution::new();
        a.record(1, def(Some("k1"), Some(("a", "m1"))));
        a.record(2, def(Some("k2"), Some(("b", "m2"))));
        a.recevoir(1, obs("a", "m1", Resultat::Succes));
        a.recevoir(2, obs("b", "m2", Resultat::Echec));
        // Règlement dans l'ordre inverse.
        a.confirmer(2);
        a.confirmer(1);
        assert_eq!(
            a.etat(1).unwrap().observation.as_ref().unwrap().resultat,
            Resultat::Succes
        );
        assert_eq!(
            a.etat(1).unwrap().definition.identite.as_deref(),
            Some("k1")
        );
        assert_eq!(
            a.etat(2).unwrap().observation.as_ref().unwrap().resultat,
            Resultat::Echec
        );
        assert_eq!(
            a.etat(2).unwrap().definition.identite.as_deref(),
            Some("k2")
        );
    }

    #[test]
    fn g_pas_de_double_consolidation_locale() {
        let mut a = Attribution::new();
        a.record(1, def(Some("k1"), Some(("a", "m1"))));
        a.recevoir(1, obs("a", "m1", Resultat::Succes));
        assert!(a.confirmer(1));
        assert!(!a.confirmer(1), "une seconde confirmation est refusée");
    }

    #[test]
    fn h_abandon_explicite_terminal_identifiable() {
        let mut a = Attribution::new();
        a.record(1, def(Some("k1"), Some(("a", "m1"))));
        assert!(a.abandonner(1));
        assert_eq!(a.etat(1).unwrap().etat, Etat::Abandonne);
    }

    #[test]
    fn pas_de_settle_sans_record() {
        let mut a = Attribution::new();
        assert_eq!(
            a.recevoir(7, obs("a", "m1", Resultat::Succes)),
            Reception::Absent
        );
        assert!(a.etat(7).is_none());
    }
}
