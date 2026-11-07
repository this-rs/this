//! # Le socle : ce qu'un agent doit savoir avant d'écrire un module
//!
//! Un graphe vide ne rend pas un agent compétent, il le rend créatif — c'est-à-dire
//! approximatif. Ce module fournit les quelques leçons qui coûtent le plus cher à
//! réapprendre : le processus, les règles du descripteur, ce qu'un module peut et
//! ne peut pas faire.
//!
//! Ce ne sont pas des conseils : chaque leçon vient d'un échec **mesuré** dans ce
//! dépôt. La règle de version est née d'un module silencieusement ignoré ; la
//! règle `requires[]` d'un lien mort ; l'interdiction de promettre d'un agent qui
//! répondait « je ne peux pas » sur une capacité qui existait.
//!
//! ## Comment ça s'utilise
//!
//! ```ignore
//! let mut contexte = Context::new("atelier", consigne_du_harness);
//! for lecon in playbook::seed() {
//!     contexte.put(lecon);
//! }
//! let consigne = contexte.render(3000).text;
//! ```
//!
//! L'ordre compte : ces leçons ont une priorité **basse** (elles s'effacent devant
//! l'état présent — le module réellement installé, l'erreur en cours), mais elles
//! doivent toujours tenir dans le budget, sinon l'agent repart de zéro.

use crate::substrate::{Context, Section, SectionKind};

/// La version du socle. À incrémenter quand une leçon change de fond : un harnais
/// peut ainsi savoir qu'il tourne sur un socle plus ancien que le code.
pub const VERSION: u32 = 1;

/// Une leçon : un identifiant stable, un corps court, une source vérifiable.
fn lecon(id: &str, titre: &str, corps: &str, source: &str) -> Section {
    Section {
        id: format!("socle.{id}"),
        kind: SectionKind::Fact,
        title: titre.to_string(),
        body: corps.to_string(),
        // Priorité basse : devant l'état présent, derrière le travail en cours.
        priority: 40,
        at_ms: 0,
        source: format!("socle v{VERSION} · {source}"),
    }
}

/// Le minimum viable pour développer un module sans se tromper.
pub fn seed() -> Vec<Section> {
    vec![
        lecon(
            "processus",
            "Le processus d'un module",
            "Observer l'état réel (architecture, modules installés) → écrire le descripteur → \
             compiler le programme → installer → VÉRIFIER en relisant le module installé. \
             Une étape qu'on saute se paie deux fois : on croit avoir installé ce qu'on a écrit.",
            "mesuré : l'app affichait « installé » pour un module que le chargeur refusait",
        ),
        lecon(
            "version",
            "Un descripteur modifié exige une version",
            "Le magasin de modules utilise (nom, version) comme clé : un descripteur qui change \
             sans que la version change est SILENCIEUSEMENT ignoré — le magasin rend l'ancienne \
             version, sans erreur. Toute modification d'entité ou de lien impose d'incrémenter \
             la version.",
            "mesuré : desktop-modules est passé de 0.2.0 à 0.3.0 en ajoutant l'entité shipment",
        ),
        lecon(
            "requires",
            "Un lien vers un autre module doit être déclaré",
            "Si un module lie ses entités à celles d'un autre module, son descripteur doit \
             déclarer ce module dans requires[]. Sans cela le lien est refusé avec \
             `this.module.reference_unknown`.",
            "mesuré : shipping → catalog",
        ),
        lecon(
            "systeme",
            "Système ou utilisateur : ce n'est pas décoratif",
            "Un module système est fourni par l'app : il ne se retire pas et ne se désactive pas. \
             Un module écrit par l'agent appartient à l'utilisateur : il se retire. Se tromper \
             de famille, c'est soit empêcher l'utilisateur de défaire ce qu'il a fait, soit \
             laisser l'app se priver elle-même de son chat.",
            "mesuré : rien n'empêchait de désactiver le module de conversation",
        ),
        lecon(
            "emplacements",
            "Un module sans emplacement ne fait rien",
            "Le code d'un module ne s'exécute que dans les emplacements ouverts par l'app. \
             Aujourd'hui : l'écran du module, l'habillage (déclaratif), l'écran de réglages. \
             Pas encore : boot, message, composer, tool. Écrire un module markdown avant que \
             l'emplacement `message` existe produit du code mort — le dire, au lieu de le faire.",
            "mesuré : 0 occurrence de rendu markdown dans l'app",
        ),
        lecon(
            "capacites",
            "Ne promets pas ce qui n'existe pas",
            "La carte du harness (`surface()`) dit ce qui est disponible et ce qui ne l'est pas. \
             Avant d'annoncer une capacité, la lire. « Pas encore » est une réponse complète ; \
             inventer une raison de sécurité pour une absence est un mensonge.",
            "mesuré : un agent a répondu « pas accès au code des modules » pour une raison fausse",
        ),
        lecon(
            "verification",
            "La vérification fait partie du module",
            "Un module est fini quand on a relu ce qui est réellement installé, pas quand la \
             compilation a réussi. Si la relecture est impossible (emplacement absent, moteur \
             muet), le module n'est pas fini : il est en attente, et ça se dit.",
            "principe du dépôt : un magasin qui se corrige sans le dire est pire qu'une erreur",
        ),
    ]
}

/// La version du socle sous laquelle un run a tourné.
///
/// Écrite dans la source de chaque leçon (`socle v1 · …`), elle ne servait qu'à
/// lire. La rendre interrogeable permet à un hôte de dire « ce run a tourné sous
/// le socle v1 » — sans quoi on ne sait jamais quelle connaissance a produit quel
/// résultat, exactement le défaut qu'on corrige pour les modules.
pub fn version() -> u32 {
    VERSION
}

/// Le socle assemblé dans un budget : ce qui est retenu, ce qui est laissé dehors.
pub fn brief(base: &str, budget_chars: usize) -> crate::substrate::Rendered {
    let mut contexte = Context::new("atelier", base);
    for lecon in seed() {
        contexte.put(lecon);
    }
    contexte.render(budget_chars)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn le_socle_couvre_les_defauts_qui_coutent_cher() {
        let ids: Vec<String> = seed().into_iter().map(|section| section.id).collect();
        for attendu in [
            "socle.processus",
            "socle.version",
            "socle.requires",
            "socle.systeme",
            "socle.emplacements",
            "socle.capacites",
            "socle.verification",
        ] {
            assert!(
                ids.iter().any(|id| id == attendu),
                "leçon absente : {attendu}"
            );
        }
    }

    #[test]
    fn chaque_lecon_nomme_sa_source() {
        for section in seed() {
            assert!(
                section.source.starts_with("socle v"),
                "{} n'a pas de provenance",
                section.id
            );
            assert!(!section.body.trim().is_empty(), "{} est vide", section.id);
        }
    }

    #[test]
    fn le_socle_passe_devant_rien_mais_tient_entier_dans_un_budget_raisonnable() {
        let rendu = brief("tu es l'atelier", 4000);
        assert!(
            rendu.dropped.is_empty(),
            "le socle doit tenir entier à 4000"
        );
        assert_eq!(rendu.kept.len(), seed().len());
        assert!(rendu.text.starts_with("tu es l'atelier"));
    }

    #[test]
    fn une_lecon_perdue_se_dit() {
        let rendu = brief("base", 300);
        assert!(!rendu.dropped.is_empty());
        assert!(rendu.text.contains("omise"), "la coupe doit être nommée");
    }

    #[test]
    fn chaque_lecon_porte_la_version_courante() {
        // Si une leçon garde « socle v1 » alors que VERSION passe à 2, le run
        // mentirait sur la connaissance qu'il a utilisée.
        for section in seed() {
            assert!(
                section.source.contains(&format!("socle v{VERSION}")),
                "{} n'annonce pas la version courante : {}",
                section.id,
                section.source
            );
        }
        assert_eq!(version(), VERSION);
    }

    #[test]
    fn les_lecons_sont_stables_et_versionnees() {
        assert_eq!(VERSION, 1);
        let a: Vec<String> = seed().into_iter().map(|s| s.id).collect();
        let b: Vec<String> = seed().into_iter().map(|s| s.id).collect();
        assert_eq!(a, b, "l'ordre du socle doit être déterministe");
    }
}
