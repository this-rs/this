//! Les niveaux d'accès : quatre barreaux, chacun strictement inclus dans le suivant.
//!
//! Un niveau est un **bundle nommé**, pas un interrupteur : « monte d'un cran » doit
//! vouloir dire quelque chose, et « fais redescendre le sous-agent » aussi. Ce module porte
//! la table et la règle d'inclusion ; il ne porte aucun octroi — accorder est le métier de
//! la porte (`desktop_consent`), et un fichier de table n'accorde rien.
//!
//! Trois choses y sont DÉCLARÉES, jamais déduites :
//!
//! - les **outils** de harnais qu'un niveau ouvre (`fs_write`, `fs_patch`, `sh_job`…) ;
//! - les **classes** de commande qu'il autorise (voir [`crate::Classe`]) ;
//! - l'**étendue** des chemins qu'il couvre ([`Etendue`]).
//!
//! L'inclusion se démontre par [`Niveau::inclus`], et le test
//! `les_niveaux_sont_emboites` la vérifie sur les quatre : si quelqu'un ajoute un outil au
//! mauvais barreau, le test tombe. C'est la seule façon d'empêcher qu'une échelle devienne
//! une collection de cases.
//!
//! `Classe::Privilege` n'apparaît dans AUCUN barreau : un niveau qui la contiendrait
//! promettrait ce que rien ne peut accorder.

use crate::classe::Classe;

/// Un niveau d'accès, du plus faible au plus engageant.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Niveau {
    /// Lire : rien ne change.
    Lecture,
    /// Écrire dans la racine de travail déclarée. Le défaut d'un agent qui code.
    Espace,
    /// Écrire hors de la racine (dossier de la personne) et sortir sur le réseau.
    Hote,
    /// Tout, sauf ce qui n'est accordable à personne.
    Complet,
}

/// Jusqu'où les chemins d'un niveau peuvent aller.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Etendue {
    /// La racine de travail déclarée, et rien au-dessus.
    Racine,
    /// Le dossier de la personne, moins les zones sensibles (la porte les nomme).
    DossierPersonnel,
    /// La machine.
    Machine,
}

/// Ce qu'un niveau ouvre. Table DÉCLARÉE, une ligne par barreau, sans calcul.
pub struct Droits {
    /// Les outils de harnais ouverts à ce niveau.
    pub outils: &'static [&'static str],
    /// Les classes de commande autorisées à ce niveau.
    pub classes: &'static [Classe],
    /// L'étendue des chemins couverte.
    pub etendue: Etendue,
}

/// Lecture seule. Aucun octet du monde ne change.
pub const LECTURE: Droits = Droits {
    outils: &[
        "fs_ls",
        "fs_read",
        "fs_grep",
        "fs_glob",
        "vcs_log",
        "vcs_diff",
        "architecture",
    ],
    classes: &[Classe::Lecture],
    etendue: Etendue::Racine,
};

/// Écrire dans la racine. Le défaut d'un agent qui code.
pub const ESPACE: Droits = Droits {
    outils: &[
        "fs_ls",
        "fs_read",
        "fs_grep",
        "fs_glob",
        "vcs_log",
        "vcs_diff",
        "architecture",
        "fs_write",
        "fs_edit",
        "fs_rm",
        "fs_mv",
        "fs_patch",
        "vcs_restore",
    ],
    classes: &[Classe::Lecture, Classe::EcritureEspace],
    etendue: Etendue::Racine,
};

/// Écrire hors racine dans une portée nommée, et sortir sur le réseau.
pub const HOTE: Droits = Droits {
    outils: &[
        "fs_ls",
        "fs_read",
        "fs_grep",
        "fs_glob",
        "vcs_log",
        "vcs_diff",
        "architecture",
        "fs_write",
        "fs_edit",
        "fs_rm",
        "fs_mv",
        "fs_patch",
        "vcs_restore",
    ],
    classes: &[
        Classe::Lecture,
        Classe::EcritureEspace,
        Classe::EcritureHorsEspace,
        Classe::Reseau,
    ],
    etendue: Etendue::DossierPersonnel,
};

/// Tout, sauf ce qui n'est accordable à personne : les processus longs arrivent ici.
pub const COMPLET: Droits = Droits {
    outils: &[
        "fs_ls",
        "fs_read",
        "fs_grep",
        "fs_glob",
        "vcs_log",
        "vcs_diff",
        "architecture",
        "fs_write",
        "fs_edit",
        "fs_rm",
        "fs_mv",
        "fs_patch",
        "vcs_restore",
        "sh_exec",
        "sh_job",
    ],
    classes: &[
        Classe::Lecture,
        Classe::EcritureEspace,
        Classe::EcritureHorsEspace,
        Classe::Reseau,
        Classe::ProcessusLong,
    ],
    etendue: Etendue::Machine,
};

impl Niveau {
    /// Le nom stable, pour les refus et les journaux.
    pub fn nom(self) -> &'static str {
        match self {
            Niveau::Lecture => "lecture",
            Niveau::Espace => "espace",
            Niveau::Hote => "hote",
            Niveau::Complet => "complet",
        }
    }

    /// Les droits de ce niveau.
    pub fn droits(self) -> &'static Droits {
        match self {
            Niveau::Lecture => &LECTURE,
            Niveau::Espace => &ESPACE,
            Niveau::Hote => &HOTE,
            Niveau::Complet => &COMPLET,
        }
    }

    /// Vrai si cet outil est ouvert à ce niveau.
    pub fn permet_outil(self, outil: &str) -> bool {
        self.droits().outils.contains(&outil)
    }

    /// Vrai si cette classe est autorisée à ce niveau.
    ///
    /// `Privilege` rend TOUJOURS faux, quel que soit le niveau : c'est une propriété de la
    /// classe ([`Classe::accordable`]), pas une case à cocher dans un barreau.
    pub fn permet_classe(self, classe: Classe) -> bool {
        classe.accordable() && self.droits().classes.contains(&classe)
    }

    /// Vrai si `self` est **inclus** dans `autre` : tout ce que `self` ouvre, `autre`
    /// l'ouvre aussi.
    pub fn inclus(self, autre: Niveau) -> bool {
        if self > autre {
            return false;
        }
        let (moi, lui) = (self.droits(), autre.droits());
        moi.etendue <= lui.etendue
            && moi.outils.iter().all(|o| lui.outils.contains(o))
            && moi.classes.iter().all(|c| lui.classes.contains(c))
    }

    /// Le niveau qui ouvre exactement cet outil. `None` si aucun.
    pub fn pour_outil(outil: &str) -> Option<Niveau> {
        [
            Niveau::Lecture,
            Niveau::Espace,
            Niveau::Hote,
            Niveau::Complet,
        ]
        .into_iter()
        .find(|n| n.permet_outil(outil))
    }

    /// Le niveau qui autorise cette classe. `None` pour `Privilege`.
    pub fn pour_classe(classe: Classe) -> Option<Niveau> {
        [
            Niveau::Lecture,
            Niveau::Espace,
            Niveau::Hote,
            Niveau::Complet,
        ]
        .into_iter()
        .find(|n| n.permet_classe(classe))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TOUS: [Niveau; 4] = [
        Niveau::Lecture,
        Niveau::Espace,
        Niveau::Hote,
        Niveau::Complet,
    ];

    #[test]
    fn les_niveaux_sont_emboites() {
        // L'échelle est un emboîtement, pas une collection de cases. Ce test tombe si
        // quelqu'un ajoute un outil au mauvais barreau, ou retire une classe d'un barreau
        // supérieur.
        for (i, bas) in TOUS.iter().enumerate() {
            for haut in &TOUS[i..] {
                assert!(
                    bas.inclus(*haut),
                    "{} devrait être inclus dans {}",
                    bas.nom(),
                    haut.nom()
                );
            }
            for bas2 in TOUS.iter().take(i) {
                assert!(
                    !bas.inclus(*bas2),
                    "{} ne doit pas être inclus dans {}",
                    bas.nom(),
                    bas2.nom()
                );
            }
        }
    }

    #[test]
    fn privilege_n_est_ouvert_a_aucun_niveau() {
        for niveau in TOUS {
            assert!(
                !niveau.permet_classe(Classe::Privilege),
                "{} ne doit pas ouvrir Privilege",
                niveau.nom()
            );
        }
        assert_eq!(Niveau::pour_classe(Classe::Privilege), None);
    }

    #[test]
    fn ecrire_demande_l_espace_et_lire_la_lecture() {
        assert!(Niveau::Lecture.permet_outil("fs_read"));
        assert!(!Niveau::Lecture.permet_outil("fs_write"));
        assert!(Niveau::Espace.permet_outil("fs_write"));
        assert!(Niveau::Espace.permet_outil("fs_patch"));
        assert!(!Niveau::Espace.permet_outil("sh_job"));
        assert_eq!(Niveau::pour_outil("fs_edit"), Some(Niveau::Espace));
        assert_eq!(Niveau::pour_outil("outil_qui_n_existe_pas"), None);
    }

    #[test]
    fn executer_une_commande_de_lecture_ne_demande_pas_l_espace() {
        assert!(Niveau::Lecture.permet_classe(Classe::Lecture));
        assert!(!Niveau::Lecture.permet_classe(Classe::EcritureEspace));
        assert!(Niveau::Espace.permet_classe(Classe::EcritureEspace));
        assert!(
            !Niveau::Espace.permet_classe(Classe::Reseau),
            "le réseau demande l'hôte"
        );
        assert!(Niveau::Hote.permet_classe(Classe::Reseau));
        assert!(!Niveau::Hote.permet_classe(Classe::ProcessusLong));
        assert!(Niveau::Complet.permet_classe(Classe::ProcessusLong));
        assert_eq!(Niveau::pour_classe(Classe::Reseau), Some(Niveau::Hote));
    }

    #[test]
    fn les_etendues_ne_descendent_jamais() {
        assert_eq!(Niveau::Lecture.droits().etendue, Etendue::Racine);
        assert_eq!(Niveau::Espace.droits().etendue, Etendue::Racine);
        assert_eq!(Niveau::Hote.droits().etendue, Etendue::DossierPersonnel);
        assert_eq!(Niveau::Complet.droits().etendue, Etendue::Machine);
    }

    #[test]
    fn aucun_outil_n_est_ouvert_par_deux_niveaux_non_adjacents_sans_l_etre_au_milieu() {
        // Un outil ouvert à `Lecture` doit l'être partout ; un outil propre à `Complet` ne
        // doit pas apparaître à `Espace`.
        for niveau in TOUS {
            for outil in niveau.droits().outils {
                let premier = Niveau::pour_outil(outil).expect("un outil ouvert a un niveau");
                assert!(
                    premier <= niveau,
                    "{outil} apparaît à {} mais son premier niveau est {}",
                    niveau.nom(),
                    premier.nom()
                );
            }
        }
    }
}
