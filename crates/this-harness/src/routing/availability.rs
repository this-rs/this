//! La mémoire d'indisponibilité : quels MODÈLES le moteur vient de refuser, et pour combien de
//! temps.
//!
//! Un modèle qui répond « 404, je ne connais pas » ou « 503 » n'est pas une panne du tour : un
//! autre modèle de la même instance peut servir. Sans mémoire, chaque message retombait pourtant
//! sur le modèle cassé, qui refusait de nouveau, et la personne lisait la même erreur en boucle.
//!
//! Deux sortes d'indisponibilité, et elles ne se traitent pas pareil :
//!
//! - **passagère** (serveur en erreur, silence, quota) : le modèle est laissé de côté un temps
//!   borné, puis réessayé tout seul ;
//! - **complète** (le moteur ne connaît pas ce modèle) : aucun délai ne le fait revenir. Il reste
//!   de côté jusqu'à ce qu'on le remette — l'hôte change les réglages de l'instance
//!   ([`Availability::forget_provider`]), la personne le demande ([`Availability::clear`]), ou
//!   un tour le sert quand même ([`Availability::served`]).
//!
//! La mémoire est partageable (`Rc`) : donnée à plusieurs routeurs, un modèle refusé dans une
//! conversation l'est pour les autres. Elle ne lit aucune horloge : le harnais la fait avancer
//! ([`Availability::advance`]) avec la sienne avant chaque décision, ce qui la garde déterministe
//! pour les tests et utilisable là où il n'y a pas de `SystemTime` (wasm).
//!
//! Elle ne contourne jamais un choix de la personne : un modèle imposé ou réglé à la main n'est
//! pas filtré par le routeur, donc pas remplacé. Elle ne change jamais de fournisseur : une
//! indisponibilité écarte un modèle, jamais une instance.

use std::cell::{Cell, RefCell};

use serde::{Deserialize, Serialize};

use super::candidate::{Candidate, Pick, Unavailable, same_model};
use crate::wire::{ModelDown, Unavailability};

#[derive(Debug, Clone, PartialEq)]
enum Lasts {
    /// Jusqu'à cet instant de l'horloge du harnais.
    Until(f64),
    /// Jusqu'à ce qu'on le remette.
    Complete,
}

#[derive(Debug, Clone)]
struct Entry {
    provider_id: String,
    model: String,
    lasts: Lasts,
    reason: String,
}

/// Une indisponibilité complète, telle qu'un hôte la garde d'un lancement à l'autre.
///
/// Seules les complètes se persistent : une indisponibilité passagère n'a pas de sens après un
/// redémarrage (le serveur a pu revenir), tandis qu'un modèle que le moteur ne connaît pas ne
/// l'est pas moins demain.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Gone {
    pub provider_id: String,
    pub model: String,
    pub reason: String,
}

/// Ce que la mémoire sait d'un modèle, pour l'affichage.
#[derive(Debug, Clone, PartialEq)]
pub struct Standing {
    pub pick: Pick,
    /// Complète : ne revient pas d'elle-même.
    pub complete: bool,
    /// Passagère : dans combien de millisecondes il est réessayé. `None` pour une complète.
    pub back_in_ms: Option<u64>,
    pub reason: String,
}

/// Les modèles que le moteur a refusés, et jusqu'à quand.
#[derive(Debug, Default)]
pub struct Availability {
    now_ms: Cell<f64>,
    entries: RefCell<Vec<Entry>>,
}

impl Availability {
    pub fn new() -> Self {
        Self::default()
    }

    /// Fait avancer l'heure de la mémoire (celle de l'horloge du harnais). Jamais en arrière :
    /// une horloge qui recule ne ressuscite pas un modèle.
    pub fn advance(&self, now_ms: f64) {
        if now_ms.is_finite() && now_ms > self.now_ms.get() {
            self.now_ms.set(now_ms);
        }
    }

    /// Retient qu'un modèle est refusé. Une complète n'est jamais ramenée à une passagère : un
    /// modèle que le moteur ne connaît pas ne devient pas « passager » parce qu'un autre refus
    /// d'un autre ordre est venu après.
    pub fn mark(&self, pick: &Pick, down: &ModelDown) {
        let lasts = match down.kind {
            Unavailability::Complete => Lasts::Complete,
            Unavailability::Temporary { cooldown_ms } => {
                Lasts::Until(self.now_ms.get() + cooldown_ms as f64)
            }
        };
        let mut entries = self.entries.borrow_mut();
        let position = entries
            .iter()
            .position(|e| e.provider_id == pick.provider_id && same_model(&e.model, &pick.model));
        match position {
            Some(at) if entries[at].lasts == Lasts::Complete && lasts != Lasts::Complete => {}
            Some(at) => {
                entries[at].lasts = lasts;
                entries[at].reason = down.reason.clone();
            }
            None => entries.push(Entry {
                provider_id: pick.provider_id.clone(),
                model: pick.model.clone(),
                lasts,
                reason: down.reason.clone(),
            }),
        }
    }

    /// Un tour a SERVI ce modèle : rien ne le dit plus refusé. Rend vrai s'il l'était.
    pub fn served(&self, pick: &Pick) -> bool {
        self.clear(pick)
    }

    /// Remet un modèle (geste de la personne, ou « réessaye »). Rend vrai s'il était retenu.
    pub fn clear(&self, pick: &Pick) -> bool {
        let mut entries = self.entries.borrow_mut();
        let before = entries.len();
        entries
            .retain(|e| !(e.provider_id == pick.provider_id && same_model(&e.model, &pick.model)));
        entries.len() != before
    }

    /// Oublie tous les modèles d'une instance : ses réglages ont changé (adresse, clé), les refus
    /// d'avant ne disent plus rien. Rend combien ont été remis.
    pub fn forget_provider(&self, provider_id: &str) -> usize {
        let mut entries = self.entries.borrow_mut();
        let before = entries.len();
        entries.retain(|e| e.provider_id != provider_id);
        before - entries.len()
    }

    /// Oublie tout.
    pub fn clear_all(&self) {
        self.entries.borrow_mut().clear();
    }

    /// Pose [`Candidate::unavailable`] sur les candidats que la mémoire retient, après avoir oublié
    /// les passagères échues. Un fait posé par l'hôte n'est jamais écrasé.
    pub fn annotate(&self, pool: &mut [Candidate]) {
        let now = self.now_ms.get();
        let mut entries = self.entries.borrow_mut();
        entries.retain(|e| match e.lasts {
            Lasts::Until(at) => now < at,
            Lasts::Complete => true,
        });
        for facts in pool.iter_mut() {
            if facts.unavailable.is_some() {
                continue;
            }
            if let Some(entry) = entries
                .iter()
                .find(|e| e.provider_id == facts.provider_id && same_model(&e.model, &facts.model))
            {
                facts.unavailable = Some(Unavailable {
                    complete: entry.lasts == Lasts::Complete,
                    reason: entry.reason.clone(),
                });
            }
        }
    }

    /// Ce que la mémoire retient maintenant, pour l'affichage.
    pub fn standing(&self) -> Vec<Standing> {
        let now = self.now_ms.get();
        self.entries
            .borrow()
            .iter()
            .filter_map(|e| match e.lasts {
                Lasts::Until(at) if now >= at => None,
                Lasts::Until(at) => Some(Standing {
                    pick: Pick::new(&e.provider_id, &e.model),
                    complete: false,
                    back_in_ms: Some((at - now).max(0.0) as u64),
                    reason: e.reason.clone(),
                }),
                Lasts::Complete => Some(Standing {
                    pick: Pick::new(&e.provider_id, &e.model),
                    complete: true,
                    back_in_ms: None,
                    reason: e.reason.clone(),
                }),
            })
            .collect()
    }

    /// Les indisponibilités complètes, à garder d'un lancement à l'autre.
    pub fn gone(&self) -> Vec<Gone> {
        self.entries
            .borrow()
            .iter()
            .filter(|e| e.lasts == Lasts::Complete)
            .map(|e| Gone {
                provider_id: e.provider_id.clone(),
                model: e.model.clone(),
                reason: e.reason.clone(),
            })
            .collect()
    }

    /// Relit ce que [`Self::gone`] avait rendu. Ne touche pas aux modèles déjà retenus.
    pub fn restore(&self, gone: impl IntoIterator<Item = Gone>) {
        for g in gone {
            self.mark(
                &Pick::new(&g.provider_id, &g.model),
                &ModelDown {
                    kind: Unavailability::Complete,
                    reason: g.reason,
                },
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::routing::candidate::Capabilities;

    fn pair(provider: &str, model: &str) -> Pick {
        Pick::new(provider, model)
    }

    fn cand(provider: &str, model: &str) -> Candidate {
        Candidate::new(
            provider,
            model,
            Capabilities {
                tools: true,
                images: false,
                reasoning: false,
                context_window: Some(1000),
            },
        )
    }

    fn temporary(ms: u64) -> ModelDown {
        ModelDown {
            kind: Unavailability::Temporary { cooldown_ms: ms },
            reason: "http 503".into(),
        }
    }

    fn complete() -> ModelDown {
        ModelDown {
            kind: Unavailability::Complete,
            reason: "http 404".into(),
        }
    }

    fn pool_after(board: &Availability) -> Vec<Candidate> {
        let mut pool = vec![cand("a", "m1"), cand("a", "m2"), cand("b", "m1")];
        board.annotate(&mut pool);
        pool
    }

    #[test]
    fn une_passagere_est_ecartee_puis_revient_d_elle_meme() {
        let board = Availability::new();
        board.advance(1_000.0);
        board.mark(&pair("a", "m1"), &temporary(5_000));

        let pool = pool_after(&board);
        assert_eq!(
            pool[0].unavailable,
            Some(Unavailable {
                complete: false,
                reason: "http 503".into()
            })
        );
        assert_eq!(pool[1].unavailable, None, "un autre modèle de l'instance");
        assert_eq!(pool[2].unavailable, None, "le même nom, une autre instance");

        board.advance(5_999.0);
        assert!(
            pool_after(&board)[0].unavailable.is_some(),
            "pas encore échue"
        );
        board.advance(6_000.0);
        assert_eq!(
            pool_after(&board)[0].unavailable,
            None,
            "échue : le modèle est réessayé"
        );
        assert!(board.standing().is_empty(), "et la mémoire l'a oubliée");
    }

    #[test]
    fn une_complete_ne_revient_pas_d_elle_meme() {
        let board = Availability::new();
        board.mark(&pair("a", "m1"), &complete());
        board.advance(10_000_000_000.0);
        assert_eq!(
            pool_after(&board)[0].unavailable,
            Some(Unavailable {
                complete: true,
                reason: "http 404".into()
            })
        );
    }

    #[test]
    fn une_complete_n_est_jamais_ramenee_a_une_passagere() {
        let board = Availability::new();
        board.mark(&pair("a", "m1"), &complete());
        board.mark(&pair("a", "m1"), &temporary(1));
        board.advance(1_000_000.0);
        assert!(pool_after(&board)[0].unavailable.as_ref().unwrap().complete);
    }

    #[test]
    fn une_horloge_qui_recule_ne_ressuscite_pas_un_modele() {
        let board = Availability::new();
        board.advance(10_000.0);
        board.mark(&pair("a", "m1"), &temporary(1_000));
        board.advance(11_000.0);
        board.advance(0.0);
        board.advance(f64::NAN);
        assert_eq!(pool_after(&board)[0].unavailable, None, "échue à 11 000");
    }

    #[test]
    fn la_meme_paire_sous_deux_ecritures_du_nom_est_une_seule() {
        // `m` et `m[1m]` sont le même modèle pour le routeur.
        let board = Availability::new();
        board.mark(&pair("a", "m1"), &temporary(5_000));
        let mut pool = vec![cand("a", "m1[1m]")];
        board.annotate(&mut pool);
        assert!(pool[0].unavailable.is_some());
    }

    #[test]
    fn servi_remet_le_modele() {
        let board = Availability::new();
        board.mark(&pair("a", "m1"), &complete());
        assert!(board.served(&pair("a", "m1")));
        assert_eq!(pool_after(&board)[0].unavailable, None);
        assert!(!board.served(&pair("a", "m1")), "rien à remettre");
    }

    #[test]
    fn changer_les_reglages_d_une_instance_oublie_tous_ses_modeles() {
        let board = Availability::new();
        board.mark(&pair("a", "m1"), &complete());
        board.mark(&pair("a", "m2"), &temporary(5_000));
        board.mark(&pair("b", "m1"), &complete());
        assert_eq!(board.forget_provider("a"), 2);
        let pool = pool_after(&board);
        assert_eq!(pool[0].unavailable, None);
        assert_eq!(pool[1].unavailable, None);
        assert!(pool[2].unavailable.is_some(), "l'autre instance reste");
    }

    #[test]
    fn un_fait_pose_par_l_hote_n_est_pas_ecrase() {
        let board = Availability::new();
        board.mark(&pair("a", "m1"), &temporary(5_000));
        let mut hote = cand("a", "m1");
        hote.unavailable = Some(Unavailable {
            complete: true,
            reason: "hôte".into(),
        });
        let mut pool = vec![hote];
        board.annotate(&mut pool);
        assert_eq!(pool[0].unavailable.as_ref().unwrap().reason, "hôte");
    }

    #[test]
    fn seules_les_completes_se_gardent_d_un_lancement_a_l_autre() {
        let board = Availability::new();
        board.mark(&pair("a", "m1"), &complete());
        board.mark(&pair("a", "m2"), &temporary(5_000));
        let gone = board.gone();
        assert_eq!(
            gone,
            vec![Gone {
                provider_id: "a".into(),
                model: "m1".into(),
                reason: "http 404".into()
            }]
        );

        let neuf = Availability::new();
        neuf.restore(gone);
        let pool = pool_after(&neuf);
        assert!(pool[0].unavailable.as_ref().unwrap().complete);
        assert_eq!(pool[1].unavailable, None, "la passagère n'a pas survécu");
    }

    #[test]
    fn standing_dit_dans_combien_de_temps_le_modele_revient() {
        let board = Availability::new();
        board.advance(1_000.0);
        board.mark(&pair("a", "m1"), &temporary(5_000));
        board.mark(&pair("a", "m2"), &complete());
        let mut standing = board.standing();
        standing.sort_by(|x, y| x.pick.model.cmp(&y.pick.model));
        assert_eq!(standing[0].back_in_ms, Some(5_000));
        assert!(!standing[0].complete);
        assert_eq!(standing[1].back_in_ms, None);
        assert!(standing[1].complete);
    }
}
