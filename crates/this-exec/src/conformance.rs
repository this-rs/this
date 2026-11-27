//! Le banc qu'un moteur fait tourner — le même pour les trois.
//!
//! Un contrat partagé mérite un banc partagé : sans lui, chaque moteur invente sa propre
//! idée de « sortie conforme », et le troisième est toujours celui qui triche. Le banc ne
//! teste donc pas un moteur en particulier : il vérifie qu'une [`Sortie`] dit la vérité sur
//! elle-même — le moteur qui l'a produite, la classe effective, et la borne respectée.

use crate::classe::{Borne, Moteur, Sortie};

/// Les écarts entre ce qu'une `Sortie` PRÉTEND et ce qu'elle contient. Vide = conforme.
///
/// `attendu` est le moteur qu'on interroge : une sortie qui déclare un autre moteur est un
/// mensonge du banc, pas une variante acceptable.
pub fn verifier(sortie: &Sortie, borne: &Borne, attendu: Moteur) -> Vec<String> {
    let mut ecarts = Vec::new();

    if sortie.moteur != attendu {
        ecarts.push(format!(
            "le moteur est déclaré {} alors que le banc interroge {attendu}",
            sortie.moteur
        ));
    }
    if !sortie.classe.accordable() {
        ecarts.push(format!(
            "la classe {} n'est accordable à personne : aucune sortie ne devrait la porter",
            sortie.classe
        ));
    }

    let rendus = sortie.stdout.len() + sortie.stderr.len();
    if sortie.tronque {
        if rendus < borne.octets_sortie {
            ecarts.push(format!(
                "tronque annoncé mais {rendus} octets rendus pour une borne de {}",
                borne.octets_sortie
            ));
        }
    } else if rendus > borne.octets_sortie {
        ecarts.push(format!(
            "{rendus} octets rendus sans troncature annoncée, pour une borne de {}",
            borne.octets_sortie
        ));
    }

    ecarts
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::classe::Classe;

    fn sortie(moteur: Moteur, stdout: &str, tronque: bool) -> Sortie {
        Sortie {
            statut: 0,
            stdout: stdout.to_string(),
            stderr: String::new(),
            tronque,
            ms: 1,
            classe: Classe::Lecture,
            moteur,
        }
    }

    #[test]
    fn une_sortie_honnete_est_conforme() {
        let borne = Borne::new(1000, 16).unwrap();
        assert!(
            verifier(
                &sortie(Moteur::Natif, "court", false),
                &borne,
                Moteur::Natif
            )
            .is_empty()
        );
    }

    #[test]
    fn un_moteur_qui_se_declare_autre_est_un_ecart() {
        let borne = Borne::new(1000, 16).unwrap();
        let ecarts = verifier(&sortie(Moteur::Wasm, "x", false), &borne, Moteur::Natif);
        assert_eq!(ecarts.len(), 1, "{ecarts:?}");
    }

    #[test]
    fn depasser_la_borne_sans_le_dire_est_un_ecart() {
        let borne = Borne::new(1000, 4).unwrap();
        let ecarts = verifier(
            &sortie(Moteur::Natif, "beaucoup trop long", false),
            &borne,
            Moteur::Natif,
        );
        assert_eq!(ecarts.len(), 1, "{ecarts:?}");
    }

    #[test]
    fn annoncer_une_troncature_qui_n_a_pas_eu_lieu_est_un_ecart() {
        let borne = Borne::new(1000, 4096).unwrap();
        let ecarts = verifier(&sortie(Moteur::Natif, "court", true), &borne, Moteur::Natif);
        assert_eq!(ecarts.len(), 1, "{ecarts:?}");
    }

    #[test]
    fn une_sortie_en_privilege_est_un_ecart_meme_si_tout_le_reste_va_bien() {
        let borne = Borne::new(1000, 16).unwrap();
        let mut s = sortie(Moteur::Natif, "x", false);
        s.classe = Classe::Privilege;
        assert_eq!(verifier(&s, &borne, Moteur::Natif).len(), 1);
    }
}
