//! **Primitive commune du cycle logique d'attribution** — RAM, indépendante du
//! backend de persistance. La durabilité et l'exactement-une-fois en concurrence
//! relèvent d'Obrain, pas de cette primitive.
//!
//! Un cycle : `Ouvert` (record) → `Recu` (observation reçue) → `EnCours`
//! (tentative réservée) → `Confirme` (terminal) ou `Abandonne` (terminal). Une
//! publication d'issue inconnue passe par `Indetermine`, qui **exige** une
//! résolution auprès du backend (jamais de ré-publication aveugle).

use std::collections::HashMap;
use std::collections::hash_map::Entry;

/// La définition d'une décision : ce qui doit rester identique pour un rejeu.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Definition {
    pub identite: Option<String>,
    /// Action PRÉVUE (choisie), peut différer de l'action servie.
    pub prevue: Option<(String, String)>,
    /// Action SERVIE (réellement exécutée).
    pub servie: Option<(String, String)>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Resultat {
    Succes,
    Echec,
    /// Résultat d'action inconnu (pas de verdict fonctionnel) — distinct de
    /// l'issue de publication.
    Inconnu,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Observation {
    pub fournisseur: String,
    pub modele: String,
    pub resultat: Resultat,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Etat {
    Ouvert,
    Recu,
    /// Tentative de publication réservée.
    EnCours,
    /// Publication d'issue INCONNUE : à résoudre auprès du backend, pas de
    /// ré-publication aveugle.
    Indetermine,
    Confirme,
    Abandonne,
}

/// Descripteur **opaque** de tentative : lié à la décision, à son observation et
/// à sa génération. Seule [`Attribution::reserver`] peut en produire un ; les
/// champs sont privés, on ne peut pas en fabriquer un arbitraire. Un descripteur
/// d'une décision ne peut pas confirmer une autre.
#[derive(Clone, Copy, Debug)]
pub struct Tentative {
    decision_id: u128,
    generation: u64,
}

#[derive(Clone, Debug)]
pub struct Cycle {
    pub definition: Definition,
    pub observation: Option<Observation>,
    pub etat: Etat,
    generation: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Record {
    Enregistre,
    Rejeu,
    Conflit,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Reception {
    Recu,
    Rejeu,
    Divergente,
    Incompatible,
    Termine,
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

    pub fn record(&mut self, id: u128, definition: Definition) -> Record {
        match self.cycles.entry(id) {
            Entry::Vacant(e) => {
                e.insert(Cycle {
                    definition,
                    observation: None,
                    etat: Etat::Ouvert,
                    generation: 0,
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

    pub fn recevoir(&mut self, id: u128, observation: Observation) -> Reception {
        let Some(c) = self.cycles.get_mut(&id) else {
            return Reception::Absent;
        };
        if c.etat == Etat::Confirme || c.etat == Etat::Abandonne {
            return Reception::Termine;
        }
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

    /// Réserve une tentative : `Recu → EnCours`. Un seul écrivain à la fois.
    pub fn reserver(&mut self, id: u128) -> Option<Tentative> {
        let c = self.cycles.get_mut(&id)?;
        if c.etat != Etat::Recu {
            return None;
        }
        c.generation = c.generation.checked_add(1)?; // débordement → refus
        c.etat = Etat::EnCours;
        Some(Tentative {
            decision_id: id,
            generation: c.generation,
        })
    }

    /// Confirme une tentative : le descripteur doit correspondre (décision + génération).
    /// Accepte `EnCours` et `Indetermine` (résolution après issue inconnue).
    pub fn confirmer(&mut self, id: u128, tentative: Tentative) -> bool {
        if tentative.decision_id != id {
            return false;
        }
        match self.cycles.get_mut(&id) {
            Some(c)
                if (c.etat == Etat::EnCours || c.etat == Etat::Indetermine)
                    && c.generation == tentative.generation =>
            {
                c.etat = Etat::Confirme;
                true
            }
            _ => false,
        }
    }

    /// Échec CERTAIN de la publication : relâche la réservation (ré-publication
    /// autorisée). Accepte `EnCours` et `Indetermine` (le backend a tranché « non écrit »).
    pub fn echec_certain(&mut self, id: u128, tentative: Tentative) -> bool {
        if tentative.decision_id != id {
            return false;
        }
        match self.cycles.get_mut(&id) {
            Some(c)
                if (c.etat == Etat::EnCours || c.etat == Etat::Indetermine)
                    && c.generation == tentative.generation =>
            {
                c.etat = Etat::Recu;
                true
            }
            _ => false,
        }
    }

    /// Publication d'issue INCONNUE : `EnCours → Indetermine`. Exige une résolution
    /// (confirmer ou echec_certain auprès du backend), jamais une ré-publication.
    pub fn indetermine(&mut self, id: u128, tentative: Tentative) -> bool {
        if tentative.decision_id != id {
            return false;
        }
        match self.cycles.get_mut(&id) {
            Some(c) if c.etat == Etat::EnCours && c.generation == tentative.generation => {
                c.etat = Etat::Indetermine;
                true
            }
            _ => false,
        }
    }

    /// Abandon résolu d'une tentative (l'écriture est tranchée). Terminal.
    pub fn abandonner(&mut self, id: u128, tentative: Tentative) -> bool {
        if tentative.decision_id != id {
            return false;
        }
        match self.cycles.get_mut(&id) {
            Some(c)
                if (c.etat == Etat::EnCours || c.etat == Etat::Indetermine)
                    && c.generation == tentative.generation =>
            {
                c.etat = Etat::Abandonne;
                true
            }
            _ => false,
        }
    }

    /// Abandon avant toute tentative (aucune écriture engagée). Terminal.
    pub fn abandonner_sans_tentative(&mut self, id: u128) -> bool {
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

    fn ouvrir_confirmer(a: &mut Attribution, id: u128) -> Tentative {
        let t = a.reserver(id).expect("réservation");
        assert!(a.confirmer(id, t));
        t
    }

    #[test]
    fn le_descripteur_est_lie_a_la_decision() {
        let mut a = Attribution::new();
        a.record(1, d());
        a.record(2, d());
        a.recevoir(1, obs("a", "m1", Resultat::Succes));
        a.recevoir(2, obs("a", "m1", Resultat::Succes));
        let t1 = a.reserver(1).unwrap();
        let t2 = a.reserver(2).unwrap();
        // Le descripteur de A ne confirme PAS B, et inversement.
        assert!(!a.confirmer(2, t1), "descripteur croisé refusé");
        assert!(!a.confirmer(1, t2), "descripteur croisé refusé");
        assert!(a.confirmer(1, t1));
        assert!(a.confirmer(2, t2));
    }

    #[test]
    fn indetermine_exige_resolution_pas_de_republication_aveugle() {
        let mut a = Attribution::new();
        a.record(1, d());
        a.recevoir(1, obs("a", "m1", Resultat::Succes));
        let t = a.reserver(1).unwrap();
        assert!(a.indetermine(1, t), "publication d'issue inconnue");
        assert_eq!(a.etat(1).unwrap().etat, Etat::Indetermine);
        // Pas de ré-publication aveugle : reserver est refusé pendant Indetermine.
        assert!(
            a.reserver(1).is_none(),
            "pas de ré-publication sans résolution"
        );
        // Résolution : le backend dit « écrit » → confirmation, contenu intact.
        assert!(a.confirmer(1, t));
        assert_eq!(
            a.etat(1).unwrap().observation.as_ref().unwrap().resultat,
            Resultat::Succes
        );
    }

    #[test]
    fn echec_certain_autorise_nouvelle_tentative() {
        let mut a = Attribution::new();
        a.record(1, d());
        a.recevoir(1, obs("a", "m1", Resultat::Succes));
        let t1 = a.reserver(1).unwrap();
        assert!(a.echec_certain(1, t1));
        assert_eq!(a.etat(1).unwrap().etat, Etat::Recu);
        let t2 = a.reserver(1).unwrap();
        assert!(a.confirmer(1, t2));
    }

    #[test]
    fn reponse_tardive_apres_abandon_resolu() {
        let mut a = Attribution::new();
        a.record(1, d());
        a.recevoir(1, obs("a", "m1", Resultat::Succes));
        let t = a.reserver(1).unwrap();
        assert!(a.abandonner(1, t), "abandon résolu");
        assert_eq!(a.etat(1).unwrap().etat, Etat::Abandonne);
        // Une réponse tardive de la tentative abandonnée ne ressuscite rien.
        assert!(!a.confirmer(1, t), "réponse tardive après abandon refusée");
        assert_eq!(a.etat(1).unwrap().etat, Etat::Abandonne);
    }

    #[test]
    fn abandon_sans_tentative_terminal() {
        let mut a = Attribution::new();
        a.record(1, d());
        a.recevoir(1, obs("a", "m1", Resultat::Succes));
        assert!(a.abandonner_sans_tentative(1));
        assert_eq!(a.etat(1).unwrap().etat, Etat::Abandonne);
    }

    #[test]
    fn rejeu_et_conflit() {
        let mut a = Attribution::new();
        assert_eq!(a.record(1, d()), Record::Enregistre);
        assert_eq!(a.record(1, d()), Record::Rejeu);
        assert_eq!(a.record(1, def(Some("k2"), None, None)), Record::Conflit);
        assert_eq!(
            a.etat(1).unwrap().definition.identite.as_deref(),
            Some("k1")
        );
    }

    #[test]
    fn b_confirmer_sans_observation_refuse() {
        let mut a = Attribution::new();
        a.record(1, d());
        assert!(a.reserver(1).is_none());
    }

    #[test]
    fn c_record_divergent_apres_confirmation_ne_modifie_pas() {
        let mut a = Attribution::new();
        a.record(1, d());
        a.recevoir(1, obs("a", "m1", Resultat::Succes));
        ouvrir_confirmer(&mut a, 1);
        assert_eq!(a.record(1, def(Some("k2"), None, None)), Record::Conflit);
        assert_eq!(a.etat(1).unwrap().etat, Etat::Confirme);
    }

    #[test]
    fn d_recevoir_apres_abandon_termine() {
        let mut a = Attribution::new();
        a.record(1, d());
        a.abandonner_sans_tentative(1);
        assert_eq!(
            a.recevoir(1, obs("a", "m1", Resultat::Succes)),
            Reception::Termine
        );
    }

    #[test]
    fn e_observation_divergente_conservee() {
        let mut a = Attribution::new();
        a.record(1, d());
        a.recevoir(1, obs("a", "m1", Resultat::Succes));
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
    fn f_incompatible_avec_action_servie() {
        let mut a = Attribution::new();
        a.record(1, d());
        assert_eq!(
            a.recevoir(1, obs("b", "m2", Resultat::Succes)),
            Reception::Incompatible
        );
    }

    #[test]
    fn g_deux_reservations_concurrentes_refusees() {
        let mut a = Attribution::new();
        a.record(1, d());
        a.recevoir(1, obs("a", "m1", Resultat::Succes));
        let t = a.reserver(1).unwrap();
        assert!(a.reserver(1).is_none());
        assert!(a.confirmer(1, t));
    }

    #[test]
    fn entrelacees_reglement_inverse_tuples_exacts() {
        let mut a = Attribution::new();
        a.record(1, def(Some("k1"), None, Some(("a", "m1"))));
        a.record(2, def(Some("k2"), None, Some(("b", "m2"))));
        a.recevoir(1, obs("a", "m1", Resultat::Succes));
        a.recevoir(2, obs("b", "m2", Resultat::Echec));
        ouvrir_confirmer(&mut a, 2);
        ouvrir_confirmer(&mut a, 1);
        assert_eq!(
            a.etat(1).unwrap().observation.as_ref().unwrap().resultat,
            Resultat::Succes
        );
        assert_eq!(
            a.etat(2).unwrap().observation.as_ref().unwrap().resultat,
            Resultat::Echec
        );
    }
}
