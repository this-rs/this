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
use std::sync::atomic::{AtomicU64, Ordering};

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
    /// Identité de l'autorité émettrice : un descripteur émis par une instance
    /// d'`Attribution` ne peut pas résoudre un cycle d'une AUTRE instance.
    autorite: u64,
    decision_id: u128,
    generation: u64,
}

#[derive(Clone, Debug)]
pub struct Cycle {
    pub definition: Definition,
    pub observation: Option<Observation>,
    pub etat: Etat,
    /// Demande d'annulation, ORTHOGONALE à l'état de publication : une annulation
    /// demandée n'implique pas que l'effet externe n'existe pas.
    pub annulation_demandee: bool,
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

static PROCHAINE_AUTORITE: AtomicU64 = AtomicU64::new(1);

/// Issue du backend pour la résolution d'une publication indéterminée. La
/// tentative prouve une réservation ; SEULE cette résolution prouve l'effet.
pub enum RésolutionBackend {
    /// L'effet est attesté par le backend.
    EffetConfirme,
    /// L'absence d'effet est certaine et définitive.
    AbsenceDefinitive,
    /// Le backend ne peut pas encore trancher.
    Indetermine,
    /// Effet incompatible/conflictuel : l'effet existe mais ne correspond pas.
    Incompatible,
}

pub struct Attribution {
    cycles: HashMap<u128, Cycle>,
    autorite: u64,
}

impl Default for Attribution {
    fn default() -> Self {
        Self {
            cycles: HashMap::new(),
            autorite: PROCHAINE_AUTORITE.fetch_add(1, Ordering::Relaxed),
        }
    }
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
                    annulation_demandee: false,
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
            autorite: self.autorite,
            decision_id: id,
            generation: c.generation,
        })
    }

    /// Un descripteur est valide pour ce cycle s'il porte la même autorité ET la
    /// même décision ET la même génération.
    fn descripteur_valide(&self, id: u128, t: &Tentative) -> bool {
        t.autorite == self.autorite && t.decision_id == id
    }

    /// Confirme une tentative : le descripteur doit correspondre (décision + génération).
    /// Accepte `EnCours` et `Indetermine` (résolution après issue inconnue).
    pub fn confirmer(&mut self, id: u128, tentative: Tentative) -> bool {
        if !self.descripteur_valide(id, &tentative) {
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
        if !self.descripteur_valide(id, &tentative) {
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
        if !self.descripteur_valide(id, &tentative) {
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

    /// Abandon AVANT toute tentative (aucune écriture engagée). Terminal.
    /// Un abandon d'une tentative en cours ou indéterminée exige d'abord la
    /// résolution (`resoudre` avec `AbsenceDefinitive`) : une transition RAM ne
    /// peut pas masquer un effet externe déjà engagé.
    pub fn abandonner_sans_tentative(&mut self, id: u128) -> bool {
        match self.cycles.get_mut(&id) {
            Some(c) if c.etat == Etat::Ouvert || c.etat == Etat::Recu => {
                c.etat = Etat::Abandonne;
                true
            }
            _ => false,
        }
    }

    /// Résout une publication indéterminée selon le verdict du backend. Toute
    /// variante valide d'abord la tentative (autorité + décision + génération) et
    /// l'état du cycle — aucune ne répond positivement à une tentative étrangère.
    pub fn resoudre(
        &mut self,
        id: u128,
        tentative: Tentative,
        resolution: RésolutionBackend,
    ) -> bool {
        match resolution {
            RésolutionBackend::EffetConfirme => self.confirmer(id, tentative),
            RésolutionBackend::AbsenceDefinitive => self.echec_certain(id, tentative),
            RésolutionBackend::Indetermine => {
                if !self.descripteur_valide(id, &tentative) {
                    return false;
                }
                matches!(
                    self.cycles.get(&id).map(|c| &c.etat),
                    Some(Etat::EnCours) | Some(Etat::Indetermine)
                )
            }
            RésolutionBackend::Incompatible => {
                if !self.descripteur_valide(id, &tentative) {
                    return false;
                }
                if let Some(c) = self.cycles.get_mut(&id)
                    && (c.etat == Etat::EnCours || c.etat == Etat::Indetermine)
                {
                    c.etat = Etat::Indetermine; // reste observable, non résolu
                }
                false
            }
        }
    }

    /// Demande d'annulation (orthogonale à l'état de publication) : n'implique
    /// pas que l'effet externe n'existe pas.
    pub fn demander_annulation(&mut self, id: u128) -> bool {
        match self.cycles.get_mut(&id) {
            Some(c) if c.etat != Etat::Confirme && c.etat != Etat::Abandonne => {
                c.annulation_demandee = true;
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
        // Abandon d'une tentative engagée : exige la résolution (absence définitive),
        // puis l'abandon sans tentative. Pas de transition directe EnCours → Abandonne.
        assert!(
            !a.abandonner_sans_tentative(1),
            "pas d'abandon direct pendant EnCours"
        );
        assert!(a.resoudre(1, t, RésolutionBackend::AbsenceDefinitive));
        assert!(a.abandonner_sans_tentative(1));
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
    #[test]
    fn inter_instance_descripteur_non_transferable() {
        let mut a = Attribution::new();
        let mut b = Attribution::new();
        // Même identifiant de décision dans deux instances indépendantes.
        a.record(1, d());
        b.record(1, d());
        a.recevoir(1, obs("a", "m1", Resultat::Succes));
        b.recevoir(1, obs("a", "m1", Resultat::Succes));
        let ta = a.reserver(1).unwrap();
        let tb = b.reserver(1).unwrap();
        // Le descripteur de A ne résout PAS B, et inversement.
        assert!(!b.confirmer(1, ta), "descripteur inter-instance refusé");
        assert!(!a.confirmer(1, tb), "descripteur inter-instance refusé");
        assert!(a.confirmer(1, ta));
        assert!(b.confirmer(1, tb));
    }

    #[test]
    fn resoudre_selon_le_verdict_backend() {
        let mut a = Attribution::new();
        a.record(1, d());
        a.recevoir(1, obs("a", "m1", Resultat::Succes));
        let t = a.reserver(1).unwrap();
        a.indetermine(1, t);
        // Résolution positive : l'effet est attesté.
        assert!(a.resoudre(1, t, RésolutionBackend::EffetConfirme));
        assert_eq!(a.etat(1).unwrap().etat, Etat::Confirme);
    }

    #[test]
    fn resolution_negative_autorise_repubication() {
        let mut a = Attribution::new();
        a.record(1, d());
        a.recevoir(1, obs("a", "m1", Resultat::Succes));
        let t = a.reserver(1).unwrap();
        a.indetermine(1, t);
        assert!(a.resoudre(1, t, RésolutionBackend::AbsenceDefinitive));
        assert_eq!(a.etat(1).unwrap().etat, Etat::Recu);
    }

    #[test]
    fn annulation_demandee_puis_effet_decouvert_reconcilie() {
        let mut a = Attribution::new();
        a.record(1, d());
        a.recevoir(1, obs("a", "m1", Resultat::Succes));
        let t = a.reserver(1).unwrap();
        // L'annulation est demandée pendant la publication.
        assert!(a.demander_annulation(1));
        assert!(a.etat(1).unwrap().annulation_demandee);
        // Mais l'effet externe est découvert : il doit être réconcilié, pas masqué.
        assert!(a.confirmer(1, t));
        assert_eq!(a.etat(1).unwrap().etat, Etat::Confirme);
        assert!(
            a.etat(1).unwrap().observation.is_some(),
            "l'effet reste observable"
        );
    }
    #[test]
    fn resoudre_indetermine_avec_tentative_etrangere_refuse() {
        let mut a = Attribution::new();
        let mut b = Attribution::new();
        a.record(1, d());
        b.record(1, d());
        a.recevoir(1, obs("a", "m1", Resultat::Succes));
        b.recevoir(1, obs("a", "m1", Resultat::Succes));
        let ta = a.reserver(1).unwrap();
        let _tb = b.reserver(1).unwrap();
        a.indetermine(1, ta);
        // Une résolution Indetermine avec une tentative étrangère est REFUSÉE
        // (aucune variante ne contourne la validation commune).
        assert!(!a.resoudre(
            1,
            Tentative {
                autorite: 999,
                decision_id: 1,
                generation: 1
            },
            RésolutionBackend::Indetermine
        ));
        assert!(a.resoudre(1, ta, RésolutionBackend::Indetermine));
    }

    #[test]
    fn abandon_direct_pendant_indetermine_refuse() {
        let mut a = Attribution::new();
        a.record(1, d());
        a.recevoir(1, obs("a", "m1", Resultat::Succes));
        let t = a.reserver(1).unwrap();
        a.indetermine(1, t);
        // Pas d'abandon direct pendant Indetermine : il faut résoudre d'abord.
        assert!(
            !a.abandonner_sans_tentative(1),
            "abandon direct refusé pendant Indetermine"
        );
        assert_eq!(a.etat(1).unwrap().etat, Etat::Indetermine);
        // Résolution → absence définitive → abandon sans tentative.
        assert!(a.resoudre(1, t, RésolutionBackend::AbsenceDefinitive));
        assert!(a.abandonner_sans_tentative(1));
        assert_eq!(a.etat(1).unwrap().etat, Etat::Abandonne);
    }
}
