//! Les classes d'effet d'une commande, et la table qui les DÉCLARE.
//!
//! Un shell est le premier outil dont la surface n'est pas finie : on ne peut pas énumérer
//! ce qu'il sait faire. Ce qui le rend rangeable, c'est de classer chaque PROGRAMME, en
//! code, une fois — jamais par un prompt, jamais par une heuristique sur la ligne.
//!
//! Deux règles portent tout le reste :
//!
//! 1. **Un programme absent de la table est INCONNU, donc refusé** ([`classer`] rend
//!    `None`). Il n'existe pas de classe « par défaut » : un défaut est une permission
//!    qu'on n'a pas écrite.
//! 2. **Un interpréteur est [`Classe::Privilege`]**, jamais moins. `sh -c`, `python -c`,
//!    `awk`, `find -exec` : le programme nommé n'est pas ce qui s'exécute. On ne devine pas
//!    ce que contient la chaîne — on refuse. Un besoin légitime de ligne interprétée
//!    s'exprime par un OUTIL à part, avec sa propre classe, pas en glissant `-c` dans un
//!    programme classé bas.
//!
//! La classe d'un programme ne dit rien du CHEMIN qu'il touche : `rm` est
//! [`Classe::EcritureEspace`] et peut pourtant viser `/etc/hosts`. C'est pourquoi
//! [`classe_de_chemin`] existe et pourquoi la classe effective d'un appel est le **maximum**
//! des deux ([`classe_effective`]).

use std::collections::BTreeMap;
use std::fmt;

use serde::{Deserialize, Serialize};

/// Ce qu'une commande fait au monde, du plus faible au plus engageant.
///
/// L'ordre de déclaration EST l'ordre de gravité (`Ord` dérivé) : `max` sur deux classes
/// rend la plus engageante. [`Classe::Privilege`] est dernière et n'est accordable à
/// personne.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Classe {
    /// Lire sans écrire : lister, afficher, chercher, comparer.
    Lecture,
    /// Écrire dans la racine de travail déclarée.
    EcritureEspace,
    /// Écrire hors de cette racine — une portée nommée doit la couvrir.
    EcritureHorsEspace,
    /// Sortir sur le réseau.
    Reseau,
    /// Vivre plus longtemps que l'appel : serveur, démon, veille.
    ProcessusLong,
    /// Changer les droits, l'identité, le démarrage, le noyau.
    ///
    /// **Jamais accordable** : aucun profil, aucun niveau, aucun octroi ne peut la contenir.
    /// Une classe qu'on ne peut pas donner empêche le modèle de négocier avec lui-même.
    Privilege,
}

impl Classe {
    /// Vrai si un octroi peut porter cette classe. `Privilege` ne le peut pas.
    pub fn accordable(self) -> bool {
        self != Classe::Privilege
    }

    /// Le nom stable, pour les refus et les journaux.
    pub fn nom(self) -> &'static str {
        match self {
            Classe::Lecture => "lecture",
            Classe::EcritureEspace => "ecriture_espace",
            Classe::EcritureHorsEspace => "ecriture_hors_espace",
            Classe::Reseau => "reseau",
            Classe::ProcessusLong => "processus_long",
            Classe::Privilege => "privilege",
        }
    }
}

impl fmt::Display for Classe {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.nom())
    }
}

/// Le moteur qui a exécuté — ou qui exécutera. Il est DÉCLARÉ, jamais déduit : un appelant
/// qui suppose « c'est un vrai shell » se trompe un jour sur trois.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Moteur {
    /// Un processus réel de la machine (`macOS`, `Linux`, `Windows`).
    Natif,
    /// Un jeu de commandes exécuté sous `wasmi`, dont le système de fichiers est le VFS.
    Wasm,
    /// Un pair, sous SON consentement.
    Distant,
}

impl fmt::Display for Moteur {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Moteur::Natif => "natif",
            Moteur::Wasm => "wasm",
            Moteur::Distant => "distant",
        })
    }
}

/// Une commande à exécuter. `programme` et `args` sont SÉPARÉS : rien n'est interprété.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Commande {
    /// Le programme, sans argument. Un chemin (`./build.sh`) donne un programme inconnu.
    pub programme: String,
    /// Les arguments, chacun déjà découpé par l'appelant.
    pub args: Vec<String>,
    /// Le répertoire de travail. Absent = la racine de travail déclarée.
    pub cwd: Option<String>,
    /// L'environnement explicite, jamais hérité du processus hôte.
    pub env: BTreeMap<String, String>,
}

impl Commande {
    pub fn new(programme: impl Into<String>) -> Self {
        Self {
            programme: programme.into(),
            ..Self::default()
        }
    }

    pub fn avec_args(mut self, args: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.args = args.into_iter().map(Into::into).collect();
        self
    }

    /// Le nom par lequel on classe : le dernier composant du chemin, sans répertoire.
    ///
    /// `"/usr/bin/git"` → `git`. `"./build.sh"` → `build.sh`, qui n'est pas dans la table et
    /// sera donc refusé : exécuter un fichier du dépôt est un acte, pas une lecture.
    pub fn nom_de_programme(&self) -> &str {
        self.programme.rsplit('/').next().unwrap_or(&self.programme)
    }
}

/// Les plafonds d'une exécution. Jamais nuls, jamais « illimités ».
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Borne {
    /// Durée murale maximale, en millisecondes.
    pub ms: u32,
    /// Octets rendus au plus, stdout et stderr confondus.
    pub octets_sortie: usize,
}

impl Borne {
    /// Une borne valide. Zéro ou une valeur absurde est un [`Refus`], jamais « sans limite ».
    pub fn new(ms: u32, octets_sortie: usize) -> Result<Self, Refus> {
        if ms == 0 {
            return Err(Refus::Borne("ms", "une durée nulle n'est pas une borne"));
        }
        if octets_sortie == 0 {
            return Err(Refus::Borne(
                "octets_sortie",
                "un plafond de sortie nul n'est pas une borne",
            ));
        }
        Ok(Self { ms, octets_sortie })
    }

    /// Le défaut d'un agent qui code : 30 s, 64 Kio.
    pub fn defaut() -> Self {
        Self {
            ms: 30_000,
            octets_sortie: 64 * 1024,
        }
    }
}

impl Default for Borne {
    fn default() -> Self {
        Self::defaut()
    }
}

/// Ce qu'une exécution a rendu. Le moteur et la classe y figurent : l'appelant n'a pas à
/// les supposer.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Sortie {
    /// Le code de sortie, tel que le moteur l'a vu. `-1` : le processus a été tué.
    pub statut: i32,
    pub stdout: String,
    pub stderr: String,
    /// Vrai si les octets rendus ont été coupés par la [`Borne`].
    pub tronque: bool,
    /// Durée murale mesurée, en millisecondes.
    pub ms: u64,
    /// La classe EFFECTIVE de l'appel (maximum du programme et des chemins touchés).
    pub classe: Classe,
    /// Le moteur qui a exécuté — jamais déduit par l'appelant.
    pub moteur: Moteur,
}

impl Sortie {
    /// Vrai si le moteur dit que la commande a réussi.
    pub fn succes(&self) -> bool {
        self.statut == 0
    }
}

/// Un refus, nommé. Il reste EN CLAIR dans la réponse au modèle : le harnais doit le lire.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Refus {
    /// Le programme n'est pas dans la table de classification.
    Inconnu(String),
    /// La classe existe mais n'est accordable à personne.
    NonAccordable(Classe),
    /// La cible sort de la portée déclarée.
    HorsPortee(String),
    /// Une borne n'est pas une borne. Le second champ dit pourquoi.
    Borne(&'static str, &'static str),
    /// Ce moteur ne peut pas exécuter ici (pas de processus, pas de pair, pas de wasmi).
    MoteurIndisponible(Moteur),
    /// Le programme n'a pas pu être lancé du tout (absent, non exécutable, répertoire
    /// disparu). Ce n'est pas un refus de droit : c'est un échec, et il se dit autrement.
    Lancement(String),
}

impl fmt::Display for Refus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Refus::Inconnu(programme) => write!(
                f,
                "refusé : « {programme} » n'est pas dans la table de classification"
            ),
            Refus::NonAccordable(classe) => {
                write!(f, "refusé : la classe {classe} n'est accordable à personne")
            }
            Refus::HorsPortee(chemin) => {
                write!(f, "refusé : {chemin} sort de la portée déclarée")
            }
            Refus::Borne(champ, motif) => write!(f, "refusé : borne « {champ} » — {motif}"),
            Refus::MoteurIndisponible(moteur) => {
                write!(f, "refusé : le moteur {moteur} est indisponible ici")
            }
            Refus::Lancement(motif) => {
                write!(f, "échec : le programme n'a pas pu être lancé — {motif}")
            }
        }
    }
}

impl std::error::Error for Refus {}

/// Une règle de la table : un programme, sa classe de base, et les arguments qui la MONTENT.
///
/// Une règle ne fait que monter : `sed` est [`Classe::Lecture`], `sed -i` devient
/// [`Classe::EcritureEspace`]. Aucun argument ne fait descendre une classe.
struct Regle {
    programme: &'static str,
    base: Classe,
    montent: &'static [(&'static str, Classe)],
}

/// La table, déclarée une fois. Volontairement courte : ce qui n'y est pas est refusé, et
/// ajouter un programme est une décision relue, pas un effet de bord.
const REGLES: &[Regle] = &[
    // Lire.
    Regle {
        programme: "ls",
        base: Classe::Lecture,
        montent: &[],
    },
    Regle {
        programme: "cat",
        base: Classe::Lecture,
        montent: &[],
    },
    Regle {
        programme: "head",
        base: Classe::Lecture,
        montent: &[],
    },
    Regle {
        programme: "tail",
        base: Classe::Lecture,
        montent: &[],
    },
    Regle {
        programme: "wc",
        base: Classe::Lecture,
        montent: &[],
    },
    Regle {
        programme: "grep",
        base: Classe::Lecture,
        montent: &[],
    },
    Regle {
        programme: "rg",
        base: Classe::Lecture,
        montent: &[],
    },
    Regle {
        programme: "stat",
        base: Classe::Lecture,
        montent: &[],
    },
    Regle {
        programme: "file",
        base: Classe::Lecture,
        montent: &[],
    },
    Regle {
        programme: "pwd",
        base: Classe::Lecture,
        montent: &[],
    },
    Regle {
        programme: "echo",
        base: Classe::Lecture,
        montent: &[],
    },
    Regle {
        programme: "which",
        base: Classe::Lecture,
        montent: &[],
    },
    Regle {
        programme: "basename",
        base: Classe::Lecture,
        montent: &[],
    },
    Regle {
        programme: "dirname",
        base: Classe::Lecture,
        montent: &[],
    },
    Regle {
        programme: "realpath",
        base: Classe::Lecture,
        montent: &[],
    },
    Regle {
        programme: "readlink",
        base: Classe::Lecture,
        montent: &[],
    },
    Regle {
        programme: "sha256sum",
        base: Classe::Lecture,
        montent: &[],
    },
    Regle {
        programme: "shasum",
        base: Classe::Lecture,
        montent: &[],
    },
    Regle {
        programme: "diff",
        base: Classe::Lecture,
        montent: &[],
    },
    Regle {
        programme: "sleep",
        base: Classe::Lecture,
        montent: &[],
    },
    Regle {
        programme: "printenv",
        base: Classe::Lecture,
        montent: &[],
    },
    // `sed` lit ; `-i` écrit sur place.
    Regle {
        programme: "sed",
        base: Classe::Lecture,
        montent: &[
            ("-i", Classe::EcritureEspace),
            ("--in-place", Classe::EcritureEspace),
        ],
    },
    // `git` écrit dans `.git` même pour `status` : sa base est donc une écriture.
    Regle {
        programme: "git",
        base: Classe::EcritureEspace,
        montent: &[
            ("push", Classe::Reseau),
            ("fetch", Classe::Reseau),
            ("pull", Classe::Reseau),
            ("clone", Classe::Reseau),
            ("ls-remote", Classe::Reseau),
            ("submodule", Classe::Reseau),
        ],
    },
    // `cargo` écrit `target/` ; les sous-commandes qui sortent sur le réseau montent.
    Regle {
        programme: "cargo",
        base: Classe::EcritureEspace,
        montent: &[
            ("publish", Classe::Reseau),
            ("install", Classe::Reseau),
            ("add", Classe::Reseau),
            ("update", Classe::Reseau),
            ("search", Classe::Reseau),
            ("login", Classe::Reseau),
            ("fetch", Classe::Reseau),
            ("doc", Classe::Reseau),
        ],
    },
    Regle {
        programme: "rustc",
        base: Classe::EcritureEspace,
        montent: &[],
    },
    Regle {
        programme: "rustfmt",
        base: Classe::EcritureEspace,
        montent: &[],
    },
    Regle {
        programme: "make",
        base: Classe::EcritureEspace,
        montent: &[],
    },
    Regle {
        programme: "cmake",
        base: Classe::EcritureEspace,
        montent: &[],
    },
    Regle {
        programme: "ninja",
        base: Classe::EcritureEspace,
        montent: &[],
    },
    Regle {
        programme: "cc",
        base: Classe::EcritureEspace,
        montent: &[],
    },
    Regle {
        programme: "gcc",
        base: Classe::EcritureEspace,
        montent: &[],
    },
    Regle {
        programme: "clang",
        base: Classe::EcritureEspace,
        montent: &[],
    },
    // Écrire.
    Regle {
        programme: "cp",
        base: Classe::EcritureEspace,
        montent: &[],
    },
    Regle {
        programme: "mv",
        base: Classe::EcritureEspace,
        montent: &[],
    },
    Regle {
        programme: "rm",
        base: Classe::EcritureEspace,
        montent: &[],
    },
    Regle {
        programme: "mkdir",
        base: Classe::EcritureEspace,
        montent: &[],
    },
    Regle {
        programme: "rmdir",
        base: Classe::EcritureEspace,
        montent: &[],
    },
    Regle {
        programme: "touch",
        base: Classe::EcritureEspace,
        montent: &[],
    },
    Regle {
        programme: "ln",
        base: Classe::EcritureEspace,
        montent: &[],
    },
    Regle {
        programme: "chmod",
        base: Classe::EcritureEspace,
        montent: &[],
    },
    Regle {
        programme: "tee",
        base: Classe::EcritureEspace,
        montent: &[],
    },
    Regle {
        programme: "zip",
        base: Classe::EcritureEspace,
        montent: &[],
    },
    Regle {
        programme: "tar",
        base: Classe::Lecture,
        montent: &[
            ("-x", Classe::EcritureEspace),
            ("-c", Classe::EcritureEspace),
            ("--extract", Classe::EcritureEspace),
            ("--create", Classe::EcritureEspace),
        ],
    },
    // Réseau.
    Regle {
        programme: "curl",
        base: Classe::Reseau,
        montent: &[],
    },
    Regle {
        programme: "wget",
        base: Classe::Reseau,
        montent: &[],
    },
    Regle {
        programme: "nc",
        base: Classe::Reseau,
        montent: &[],
    },
    Regle {
        programme: "ping",
        base: Classe::Reseau,
        montent: &[],
    },
    Regle {
        programme: "scp",
        base: Classe::Reseau,
        montent: &[],
    },
    Regle {
        programme: "rsync",
        base: Classe::Reseau,
        montent: &[],
    },
    Regle {
        programme: "npm",
        base: Classe::Reseau,
        montent: &[],
    },
    Regle {
        programme: "pnpm",
        base: Classe::Reseau,
        montent: &[],
    },
    Regle {
        programme: "yarn",
        base: Classe::Reseau,
        montent: &[],
    },
    Regle {
        programme: "pip",
        base: Classe::Reseau,
        montent: &[],
    },
    Regle {
        programme: "pip3",
        base: Classe::Reseau,
        montent: &[],
    },
    // Privilège : changement de droits, d'identité, de démarrage, ou exécution à distance.
    Regle {
        programme: "sudo",
        base: Classe::Privilege,
        montent: &[],
    },
    Regle {
        programme: "su",
        base: Classe::Privilege,
        montent: &[],
    },
    Regle {
        programme: "doas",
        base: Classe::Privilege,
        montent: &[],
    },
    Regle {
        programme: "chown",
        base: Classe::Privilege,
        montent: &[],
    },
    Regle {
        programme: "launchctl",
        base: Classe::Privilege,
        montent: &[],
    },
    Regle {
        programme: "systemctl",
        base: Classe::Privilege,
        montent: &[],
    },
    Regle {
        programme: "service",
        base: Classe::Privilege,
        montent: &[],
    },
    Regle {
        programme: "mount",
        base: Classe::Privilege,
        montent: &[],
    },
    Regle {
        programme: "umount",
        base: Classe::Privilege,
        montent: &[],
    },
    Regle {
        programme: "shutdown",
        base: Classe::Privilege,
        montent: &[],
    },
    Regle {
        programme: "reboot",
        base: Classe::Privilege,
        montent: &[],
    },
    Regle {
        programme: "defaults",
        base: Classe::Privilege,
        montent: &[],
    },
    Regle {
        programme: "ssh",
        base: Classe::Privilege,
        montent: &[],
    },
    Regle {
        programme: "docker",
        base: Classe::Privilege,
        montent: &[],
    },
];

/// Les interpréteurs : le programme nommé n'est pas ce qui s'exécute. Toujours
/// [`Classe::Privilege`], donc jamais accordé.
const INTERPRETEURS: &[&str] = &[
    "sh", "bash", "zsh", "dash", "ksh", "fish", "env", "xargs", "awk", "gawk", "python", "python3",
    "perl", "ruby", "node", "deno", "bun", "eval", "exec", "nohup", "time", "command", "builtin",
    "source",
];

/// La classe d'une commande, ou `None` si le programme est inconnu — et un inconnu est
/// refusé, jamais rangé par défaut.
///
/// Ce que cette fonction ne dit PAS : la classe du CHEMIN visé. Voir [`classe_de_chemin`] et
/// [`classe_effective`].
pub fn classer(commande: &Commande) -> Option<Classe> {
    let nom = commande.nom_de_programme();
    if INTERPRETEURS.contains(&nom) {
        return Some(Classe::Privilege);
    }
    let regle = REGLES.iter().find(|r| r.programme == nom)?;
    let mut classe = regle.base;
    for arg in &commande.args {
        // Un marqueur qui n'est pas un drapeau (une sous-commande) doit correspondre au
        // PREMIER argument : `git push origin` monte, `git log --format=push` ne monte pas
        // par accident sur le mot.
        let est_drapeau = arg.starts_with('-');
        for (marqueur, monte) in regle.montent {
            let marqueur_est_drapeau = marqueur.starts_with('-');
            let correspond = if marqueur_est_drapeau {
                arg == marqueur || arg.starts_with(&format!("{marqueur}="))
            } else {
                !est_drapeau && commande.args.first() == Some(arg) && arg == marqueur
            };
            if correspond {
                classe = classe.max(*monte);
            }
        }
    }
    // Un interpréteur glissé dans les ARGUMENTS d'une commande classée basse est un
    // contournement : `find . -exec rm {} +`, `xargs rm`.
    if commande
        .args
        .iter()
        .any(|arg| arg == "-exec" || arg == "-execdir" || arg == "-delete")
    {
        classe = classe.max(Classe::Privilege);
    }
    Some(classe)
}

/// La classe d'un chemin, vue depuis une racine de travail.
///
/// Dedans → [`Classe::EcritureEspace`] ; dehors, ou au-dessus de la racine par `..` →
/// [`Classe::EcritureHorsEspace`]. La normalisation est PURE (aucun appel système) : un
/// chemin qui s'échappe est reconnu sans toucher au disque. La résolution des liens
/// symboliques, elle, appartient au moteur, qui seul a le disque — le contrat dit la règle,
/// le moteur fait l'appel.
pub fn classe_de_chemin(racine: &str, chemin: &str) -> Classe {
    match normaliser(racine, chemin) {
        Some(_) => Classe::EcritureEspace,
        None => Classe::EcritureHorsEspace,
    }
}

/// La classe effective d'un appel : le maximum de la commande et de ses chemins.
pub fn classe_effective(racine: &str, commande: &Commande, chemins: &[&str]) -> Option<Classe> {
    let mut classe = classer(commande)?;
    for chemin in chemins {
        classe = classe.max(classe_de_chemin(racine, chemin));
    }
    Some(classe)
}

/// Résout `chemin` relativement à `racine`, sans toucher au disque. `None` si le résultat
/// sort de la racine.
fn normaliser(racine: &str, chemin: &str) -> Option<String> {
    let racine = simplifier(racine, "")?;
    let combine = if chemin.starts_with('/') {
        chemin.to_string()
    } else {
        format!("{racine}/{chemin}")
    };
    let resolu = simplifier(&combine, "")?;
    if resolu == racine || resolu.starts_with(&format!("{racine}/")) {
        Some(resolu)
    } else {
        None
    }
}

/// Aplatit `.` et `..`. `None` si `..` remonte au-dessus de la racine du système — un tel
/// chemin n'a pas de sens et n'est pas « presque » dans la portée.
fn simplifier(chemin: &str, _: &str) -> Option<String> {
    let mut parts: Vec<&str> = Vec::new();
    for part in chemin.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop()?;
            }
            autre => parts.push(autre),
        }
    }
    Some(format!("/{}", parts.join("/")))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c(programme: &str, args: &[&str]) -> Commande {
        Commande::new(programme).avec_args(args.iter().copied())
    }

    #[test]
    fn un_programme_inconnu_n_a_pas_de_classe_donc_est_refuse() {
        assert_eq!(classer(&c("cargo-mutants", &[])), None);
        assert_eq!(
            classer(&c("./build.sh", &[])),
            None,
            "un fichier du dépôt n'est pas un programme classé"
        );
    }

    #[test]
    fn un_interpreteur_est_toujours_privilege() {
        for nom in ["sh", "bash", "python3", "awk", "eval", "xargs", "env"] {
            assert_eq!(classer(&c(nom, &[])), Some(Classe::Privilege), "{nom}");
            assert_eq!(
                classer(&c(nom, &["-c", "rm -rf /"])),
                Some(Classe::Privilege),
                "{nom} -c"
            );
        }
    }

    #[test]
    fn find_exec_est_un_interpreteur_deguise() {
        assert_eq!(classer(&c("rg", &[])), Some(Classe::Lecture));
        // `find` n'est pas dans la table : inconnu, donc refusé — c'est encore plus sûr.
        assert_eq!(classer(&c("find", &[".", "-exec", "rm", "{}", "+"])), None);
    }

    #[test]
    fn un_argument_fait_monter_une_classe_jamais_descendre() {
        assert_eq!(classer(&c("sed", &["-n", "1,5p"])), Some(Classe::Lecture));
        assert_eq!(
            classer(&c("sed", &["-i", "s/a/b/"])),
            Some(Classe::EcritureEspace)
        );
        assert_eq!(
            classer(&c("git", &["status"])),
            Some(Classe::EcritureEspace)
        );
        assert_eq!(
            classer(&c("git", &["push", "origin", "main"])),
            Some(Classe::Reseau)
        );
        assert_eq!(
            classer(&c("cargo", &["test"])),
            Some(Classe::EcritureEspace)
        );
        assert_eq!(classer(&c("cargo", &["publish"])), Some(Classe::Reseau));
    }

    #[test]
    fn un_mot_de_sous_commande_dans_une_option_ne_monte_pas() {
        // `git log --format=push` ne doit pas devenir réseau parce que « push » apparaît.
        assert_eq!(
            classer(&c("git", &["log", "--format=push"])),
            Some(Classe::EcritureEspace)
        );
    }

    #[test]
    fn un_chemin_dedans_est_une_ecriture_et_un_chemin_dehors_en_est_une_autre() {
        let racine = "/Users/moi/projet";
        assert_eq!(
            classe_de_chemin(racine, "src/main.rs"),
            Classe::EcritureEspace
        );
        assert_eq!(
            classe_de_chemin(racine, "/Users/moi/projet/src/main.rs"),
            Classe::EcritureEspace
        );
        assert_eq!(
            classe_de_chemin(racine, "/etc/hosts"),
            Classe::EcritureHorsEspace
        );
        assert_eq!(
            classe_de_chemin(racine, "../../etc/hosts"),
            Classe::EcritureHorsEspace
        );
        // Le préfixe ne suffit pas : `/Users/moi/projet-2` n'est pas dans `/Users/moi/projet`.
        assert_eq!(
            classe_de_chemin(racine, "/Users/moi/projet-2/x"),
            Classe::EcritureHorsEspace
        );
    }

    #[test]
    fn la_classe_effective_prend_le_maximum_de_la_commande_et_des_chemins() {
        let racine = "/Users/moi/projet";
        // `rm` dans la racine : écriture espace.
        assert_eq!(
            classe_effective(racine, &c("rm", &["src/x.rs"]), &["src/x.rs"]),
            Some(Classe::EcritureEspace)
        );
        // `rm` hors racine : c'est le chemin qui commande.
        assert_eq!(
            classe_effective(racine, &c("rm", &["/etc/hosts"]), &["/etc/hosts"]),
            Some(Classe::EcritureHorsEspace)
        );
        // `cat` (lecture) SUR un chemin hors racine reste une lecture : lire n'écrit rien.
        assert_eq!(
            classe_effective(racine, &c("cat", &["/etc/hosts"]), &[]),
            Some(Classe::Lecture)
        );
    }

    #[test]
    fn privilege_n_est_accordable_a_personne() {
        assert!(!Classe::Privilege.accordable());
        for classe in [
            Classe::Lecture,
            Classe::EcritureEspace,
            Classe::EcritureHorsEspace,
            Classe::Reseau,
            Classe::ProcessusLong,
        ] {
            assert!(classe.accordable(), "{classe}");
        }
    }

    #[test]
    fn une_borne_nulle_est_un_refus_pas_une_absence_de_borne() {
        assert!(Borne::new(0, 1024).is_err());
        assert!(Borne::new(1000, 0).is_err());
        assert_eq!(
            Borne::new(1000, 1024).unwrap(),
            Borne {
                ms: 1000,
                octets_sortie: 1024
            }
        );
    }

    #[test]
    fn l_ordre_des_classes_est_l_ordre_de_gravite() {
        assert!(Classe::Lecture < Classe::EcritureEspace);
        assert!(Classe::EcritureEspace < Classe::EcritureHorsEspace);
        assert!(Classe::EcritureHorsEspace < Classe::Reseau);
        assert!(Classe::Reseau < Classe::ProcessusLong);
        assert!(Classe::ProcessusLong < Classe::Privilege);
    }

    #[test]
    fn le_nom_du_programme_est_le_dernier_composant() {
        assert_eq!(Commande::new("/usr/bin/git").nom_de_programme(), "git");
        assert_eq!(Commande::new("./build.sh").nom_de_programme(), "build.sh");
    }

    #[test]
    fn un_refus_se_lit_en_francais_et_commence_par_refuse() {
        let motifs = [
            Refus::Inconnu("truc".into()),
            Refus::NonAccordable(Classe::Privilege),
            Refus::HorsPortee("/etc".into()),
            Refus::Borne("ms", "nulle"),
            Refus::MoteurIndisponible(Moteur::Distant),
        ];
        for refus in motifs {
            let texte = refus.to_string();
            assert!(texte.starts_with("refusé : "), "{texte}");
        }
    }
}
