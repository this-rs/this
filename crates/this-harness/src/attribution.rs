//! **Primitive commune du cycle logique d'attribution** — RAM, indépendante du
//! backend de persistance. La durabilité et l'exactement-une-fois en concurrence
//! relèvent d'Obrain, pas de cette primitive.
//!
//! Contrat (corrigé) :
//! - `record` n'écrase jamais : divergence → `Conflit`, l'état d'origine est intact ;
//! - `recevoir` ne rouvre pas un cycle terminal, n'écrase pas une observation ;
//! - `confirmer` n'accepte qu'un cycle ayant une observation reçue ;
//! - `abandonner` est l'autre issue terminale ;
//! - le résultat d'action (`Resultat`) est distinct de l'issue de publication
//!   (confirmation / attente) : une écriture indéterminée ne clôt pas le cycle.

use std::collections::HashMap;
use std::collections::hash_map::Entry;

/// La définition d'une décision : ce qui doit rester identique pour un rejeu.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Definition {
    /// Identité cognitive constituée (`None` = pas de connaissance constituée).
    pub identite: Option<String>,
    /// Action PRÉVUE (la paire choisie par la décision).
    pub prevue: Option<(String, String)>,
    /// Action SERVIE (la paire réellement exécutée) — peut différer de `prevue`.
    pub servie: Option<(String, String)>,
}

/// Le résultat d'une ACTION — jamais confondu avec le succès de transport ni
/// avec l'issue de publication.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Resultat {
    Succes,
    Echec,
    /// Résultat d'action inconnu (pas de verdict fonctionnel).
    Inconnu,
}

/// Une observation reçue, pas encore confirmée.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Observation {
    pub fournisseur: String,
    pub modele: String,
    pub resultat: Resultat,
}

/// État du cycle d'attribution. `Confirme` et `Abandonne` sont terminaux.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Etat {
    /// Décision enregistrée, pas d'observation.
    Ouvert,
    /// Observation reçue, en attente de confirmation (ou d'abandon).
    Recu,
    /// Confirmation logique : terminal.
    Confirme,
    /// Abandon explicite : terminal.
    Abandonne,
}

#[derive(Clone, Debug)]
pub struct Cycle {
    pub definition: Definition,
    pub observation: Option<Observation>,
    pub etat: Etat,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Record {
    Enregistre,
    Rejeu,
    /// Définition divergente : refusée, l'état d'origine n'est PAS modifié.
    Conflit,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Reception {
    Recu,
    /// Même observation reçue une seconde fois : idempotent.
    Rejeu,
    /// Observation différente : refusée, l'observation d'origine est conservée.
    Divergente,
    /// L'observation ne correspond pas à l'action servie.
    Incompatible,
    /// Le cycle est terminal (confirmé ou abandonné) : refusé.
    Termine,
    /// Aucun `record` pour cet identifiant.
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

    /// Enregistre la définition d'une décision. Jamais d'écrasement : un identifiant
    /// présent avec une définition différente est un conflit SANS modifier l'état.
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
            Entry::Occupied(e) => {
                if e.get().definition == definition {
                    Record::Rejeu
                } else {
                    Record::Conflit
                }
            }
        }
    }

    /// Reçoit et conserve une observation, sans fermer le cycle. Ne rouvre jamais
    /// un cycle terminal, n'écrase jamais une observation existante.
    pub fn recevoir(&mut self, id: u128, observation: Observation) -> Reception {
        let Some(c) = self.cycles.get_mut(&id) else {
            return Reception::Absent;
        };
        if c.etat == Etat::Confirme || c.etat == Etat::Abandonne {
            return Reception::Termine;
        }
        // L'observation doit porter l'action effectivement servie (si connue).
        if let Some((p, m)) = &c.definition.servie
            && (&observation.fournisseur != p || &observation.modele != m)
        {
            return Reception::Incompatible;
        }
        match &c.observation {
            Some(existing) if existing == &observation => Reception::Rejeu,
            Some(_) => Reception::Divergente,
            None => {
                c.observation = Some(observation);
                c.etat = Etat::Recu;
                Reception::Recu
            }
        }
    }

    /// Confirme : n'accepte qu'un cycle ayant une observation reçue. Idempotent.
    pub fn confirmer(&mut self, id: u128) -> bool {
        match self.cycles.get_mut(&id) {
            Some(c) if c.etat == Etat::Recu => {
                c.etat = Etat::Confirme;
                true
            }
            _ => false,
        }
    }

    /// Abandon explicite : terminal.
    pub fn abandonner(&mut self, id: u128) -> bool {
        match self.cycles.get_mut(&id) {
            Some(c) if c.etat == Etat::Ouvert || c.etat == Etat::Recu => {
                c.etat = Etat::Abandonne;
                true
            }
            _ => false,
        }
    }

    pub fn etat(&self, id: u128) -> Option<&Cycle> {
        self.cycles.get(&id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn def(
        identite: Option<&str>,
        prevue: Option<(&str, &str)>,
        servie: Option<(&str, &str)>,
    ) -> Definition {
        Definition {
            identite: identite.map(str::to_string),
            prevue: prevue.map(|(p, m)| (p.to_string(), m.to_string())),
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

    fn d() -> Definition {
        def(Some("k1"), Some(("a", "m1")), Some(("a", "m1")))
    }

    #[test]
    fn a_recevoir_confirmer_puis_recevoir_ne_rouvre_pas() {
        let mut a = Attribution::new();
        a.record(1, d());
        assert_eq!(
            a.recevoir(1, obs("a", "m1", Resultat::Succes)),
            Reception::Recu
        );
        assert!(a.confirmer(1));
        // A — après confirmation, une nouvelle réception est refusée (Termine).
        assert_eq!(
            a.recevoir(1, obs("a", "m1", Resultat::Echec)),
            Reception::Termine
        );
        assert!(!a.confirmer(1), "pas de réouverture ni de re-confirmation");
        assert_eq!(
            a.etat(1).unwrap().observation.as_ref().unwrap().resultat,
            Resultat::Succes
        );
    }

    #[test]
    fn b_confirmer_sans_observation_refuse() {
        let mut a = Attribution::new();
        a.record(1, d());
        assert!(
            !a.confirmer(1),
            "Ouvert sans observation n'est pas confirmable"
        );
        assert_eq!(a.etat(1).unwrap().etat, Etat::Ouvert);
    }

    #[test]
    fn c_record_divergent_apres_confirmation_ne_modifie_pas() {
        let mut a = Attribution::new();
        a.record(1, d());
        a.recevoir(1, obs("a", "m1", Resultat::Succes));
        a.confirmer(1);
        // C — record divergent : conflit, mais l'état terminal est INTACT.
        assert_eq!(a.record(1, def(Some("k2"), None, None)), Record::Conflit);
        assert_eq!(a.etat(1).unwrap().etat, Etat::Confirme);
        assert_eq!(
            a.etat(1).unwrap().definition.identite.as_deref(),
            Some("k1")
        );
    }

    #[test]
    fn d_recevoir_apres_abandon_termine() {
        let mut a = Attribution::new();
        a.record(1, d());
        assert!(a.abandonner(1));
        assert_eq!(
            a.recevoir(1, obs("a", "m1", Resultat::Succes)),
            Reception::Termine
        );
        assert_eq!(a.etat(1).unwrap().etat, Etat::Abandonne);
    }

    #[test]
    fn e_observation_divergente_avant_confirmation_conservee() {
        let mut a = Attribution::new();
        a.record(1, d());
        a.recevoir(1, obs("a", "m1", Resultat::Succes));
        // E — observation divergente : refusée, la première est conservée.
        assert_eq!(
            a.recevoir(1, obs("a", "m1", Resultat::Echec)),
            Reception::Divergente
        );
        assert_eq!(
            a.etat(1).unwrap().observation.as_ref().unwrap().resultat,
            Resultat::Succes
        );
    }

    #[test]
    fn f_observation_incompatible_avec_action_servie() {
        let mut a = Attribution::new();
        a.record(1, d()); // servie = (a, m1)
        assert_eq!(
            a.recevoir(1, obs("b", "m2", Resultat::Succes)),
            Reception::Incompatible
        );
        assert!(a.etat(1).unwrap().observation.is_none());
    }

    #[test]
    fn g_deux_consolidations_refusees() {
        let mut a = Attribution::new();
        a.record(1, d());
        a.recevoir(1, obs("a", "m1", Resultat::Succes));
        assert!(a.confirmer(1));
        assert!(!a.confirmer(1), "deuxième consolidation refusée");
    }

    #[test]
    fn h_indetermination_conserve_l_observation_puis_resolution() {
        let mut a = Attribution::new();
        a.record(1, d());
        a.recevoir(1, obs("a", "m1", Resultat::Inconnu));
        // H — résultat d'action inconnu : le cycle reste Recu (pas confirmé),
        // l'observation n'est PAS écrasée par une résolution.
        assert_eq!(a.etat(1).unwrap().etat, Etat::Recu);
        a.confirmer(1);
        assert_eq!(a.etat(1).unwrap().etat, Etat::Confirme);
        assert_eq!(
            a.etat(1).unwrap().observation.as_ref().unwrap().resultat,
            Resultat::Inconnu
        );
    }

    #[test]
    fn entrelacees_reglement_inverse_tuples_exacts() {
        let mut a = Attribution::new();
        a.record(1, def(Some("k1"), None, Some(("a", "m1"))));
        a.record(2, def(Some("k2"), None, Some(("b", "m2"))));
        a.recevoir(1, obs("a", "m1", Resultat::Succes));
        a.recevoir(2, obs("b", "m2", Resultat::Echec));
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
}
