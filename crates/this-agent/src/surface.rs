//! # Ce que le harness EST, et ce qu'il PEUT
//!
//! Un harness qui ne connaît pas sa propre architecture invente ses réponses :
//! « je n'ai pas accès à ça » pour une capacité qui existe, « je peux » pour une
//! qui n'existe pas. Cette carte est la source unique de cette connaissance —
//! structurée, sérialisable, et **honnête** : chaque capacité dit son étage et
//! si elle est disponible *aujourd'hui*.
//!
//! ## Trois étages, une règle
//!
//! Une WebView ne peut rien faire qu'elle n'expose pas.
//!
//! | Étage | Qui l'exécute | C'est quoi |
//! |---|---|---|
//! | [`Tier::Webview`] | le module, seul | DOM, réseau, stockage, capteurs, média |
//! | [`Tier::Shell`] | la coquille | fenêtre, barre des tâches, notifications, fichiers, haptique, Live Activity |
//! | [`Tier::Native`] | un binaire compagnon | widgets, watchOS, CarPlay, App Intents — **qu'un module pilote, jamais ne fournit** |
//!
//! ## Pourquoi `Status`
//!
//! Une carte qui ne distingue pas le possible du livré est un catalogue
//! publicitaire. [`Status::Live`] = ça tourne dans l'app aujourd'hui ;
//! [`Status::Missing`] = c'est prévu, ce n'est pas fait, et la carte le dit.
//! C'est ce qui permet à l'agent de répondre « pas encore » au lieu d'inventer.

use serde::{Deserialize, Serialize};

/// L'étage d'une capacité.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Tier {
    /// Dans la page : le module suffit.
    Webview,
    /// Par la coquille : commande native + permission.
    Shell,
    /// Cible native séparée : le harness pilote, il ne fournit pas.
    Native,
}

impl Tier {
    pub fn as_str(&self) -> &'static str {
        match self {
            Tier::Webview => "webview",
            Tier::Shell => "coquille",
            Tier::Native => "natif",
        }
    }
}

/// Où une capacité existe.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Platform {
    Web,
    Ios,
    Ipados,
    Macos,
    Windows,
    Linux,
    Android,
    Watchos,
    Carplay,
    AndroidAuto,
}

impl Platform {
    pub fn as_str(&self) -> &'static str {
        match self {
            Platform::Web => "web",
            Platform::Ios => "ios",
            Platform::Ipados => "ipados",
            Platform::Macos => "macos",
            Platform::Windows => "windows",
            Platform::Linux => "linux",
            Platform::Android => "android",
            Platform::Watchos => "watchos",
            Platform::Carplay => "carplay",
            Platform::AndroidAuto => "android_auto",
        }
    }
}

/// Livré, ou seulement prévu.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    /// Ça tourne aujourd'hui.
    Live,
    /// C'est prévu, ce n'est pas fait.
    Missing,
}

/// Une capacité : ce que le harness peut faire, et à quel prix.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Capability {
    /// Identifiant stable, celui que les modules demandent.
    pub id: String,
    pub tier: Tier,
    pub title: String,
    /// La commande native exigée (étages coquille et natif), s'il y en a une.
    pub command: Option<String>,
    /// Les surfaces où elle existe réellement.
    pub platforms: Vec<Platform>,
    pub status: Status,
}

impl Capability {
    fn new(
        id: &str,
        tier: Tier,
        title: &str,
        command: Option<&str>,
        platforms: &[Platform],
        status: Status,
    ) -> Self {
        Self {
            id: id.to_string(),
            tier,
            title: title.to_string(),
            command: command.map(str::to_string),
            platforms: platforms.to_vec(),
            status,
        }
    }
}

/// Un emplacement de l'interface : là où un module peut intervenir.
///
/// Ce n'est pas un détail d'implémentation : un module sans emplacement ne peut
/// rien faire, quel que soit son code. « Mon module markdown ne marche pas » se
/// diagnostique ici, pas dans le module.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Slot {
    pub id: String,
    pub title: String,
    /// Ce que l'emplacement donne au module (le `ctx`).
    pub contract: String,
    pub status: Status,
}

impl Slot {
    fn new(id: &str, title: &str, contract: &str, status: Status) -> Self {
        Self {
            id: id.to_string(),
            title: title.to_string(),
            contract: contract.to_string(),
            status,
        }
    }
}

/// La carte complète : capacités et emplacements.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Surface {
    pub capabilities: Vec<Capability>,
    pub slots: Vec<Slot>,
}

impl Surface {
    pub fn capability(&self, id: &str) -> Option<&Capability> {
        self.capabilities.iter().find(|c| c.id == id)
    }

    pub fn slot(&self, id: &str) -> Option<&Slot> {
        self.slots.iter().find(|s| s.id == id)
    }

    pub fn live(&self) -> impl Iterator<Item = &Capability> {
        self.capabilities
            .iter()
            .filter(|c| c.status == Status::Live)
    }

    pub fn missing(&self) -> impl Iterator<Item = &Capability> {
        self.capabilities
            .iter()
            .filter(|c| c.status == Status::Missing)
    }

    /// La carte en clair, dans un budget de caractères.
    ///
    /// Même règle que le contexte : on n'annonce jamais une coupe en la taisant.
    /// Une carte tronquée sans le dire ferait croire que le reste n'existe pas.
    pub fn describe(&self, budget_chars: usize) -> String {
        let mut sortie = String::from("Tes capacités (étage · état) :\n");
        let mut tronque = 0usize;
        for tier in [Tier::Webview, Tier::Shell, Tier::Native] {
            let lignes: Vec<String> = self
                .capabilities
                .iter()
                .filter(|c| c.tier == tier)
                .map(|c| {
                    format!(
                        "  · {} — {}{} [{}]",
                        c.id,
                        c.title,
                        c.command
                            .as_ref()
                            .map(|cmd| format!(" (commande {cmd})"))
                            .unwrap_or_default(),
                        match c.status {
                            Status::Live => "disponible",
                            Status::Missing => "pas encore",
                        }
                    )
                })
                .collect();
            if lignes.is_empty() {
                continue;
            }
            let bloc = format!("[{}]\n{}\n", tier.as_str(), lignes.join("\n"));
            if sortie.chars().count() + bloc.chars().count() > budget_chars {
                tronque += lignes.len();
                continue;
            }
            sortie.push_str(&bloc);
        }
        let manquants: Vec<&str> = self
            .slots
            .iter()
            .filter(|s| s.status == Status::Missing)
            .map(|s| s.id.as_str())
            .collect();
        if !manquants.is_empty() {
            sortie.push_str(&format!(
                "\nEmplacements pas encore ouverts : {}\n",
                manquants.join(", ")
            ));
        }
        if tronque > 0 {
            sortie.push_str(&format!(
                "\n{tronque} capacité(s) omise(s) faute de budget.\n"
            ));
        }
        sortie
    }
}

/// La carte réelle du harness, telle qu'elle est **aujourd'hui**.
///
/// Chaque entrée est une affirmation vérifiable : `Status::Live` veut dire que le
/// code existe et tourne ; `Status::Missing` veut dire qu'il n'existe pas. Le
/// jour où une capacité est livrée, c'est ici — et nulle part ailleurs — qu'elle
/// change d'état.
pub fn surface() -> Surface {
    use Platform::*;
    Surface {
        capabilities: vec![
            // ---- étage page
            Capability::new(
                "ui.render",
                Tier::Webview,
                "dessiner dans un emplacement",
                None,
                &[Web, Ios, Ipados, Macos, Windows, Linux, Android],
                Status::Live,
            ),
            Capability::new(
                "ui.markdown",
                Tier::Webview,
                "rendre du markdown dans une bulle",
                None,
                &[Web, Ios, Ipados, Macos, Windows, Linux, Android],
                Status::Missing,
            ),
            Capability::new(
                "store.local",
                Tier::Webview,
                "lire et écrire le stockage de l'app",
                None,
                &[Web, Ios, Ipados, Macos, Windows, Linux, Android],
                Status::Live,
            ),
            Capability::new(
                "http.fetch",
                Tier::Webview,
                "appeler un service HTTP",
                None,
                &[Web, Ios, Ipados, Macos, Windows, Linux, Android],
                Status::Live,
            ),
            Capability::new(
                "fs.index",
                Tier::Webview,
                "lister le VFS (artefacts compilés)",
                None,
                &[Web, Ios, Ipados, Macos, Windows, Linux, Android],
                Status::Missing,
            ),
            // ---- étage coquille
            Capability::new(
                "app.boot",
                Tier::Shell,
                "intervenir au démarrage, et bloquer",
                None,
                &[Ios, Ipados, Macos, Windows, Linux, Android],
                Status::Missing,
            ),
            Capability::new(
                "window.control",
                Tier::Shell,
                "taille, plein écran, focus",
                Some("window.*"),
                &[Macos, Windows, Linux],
                Status::Live,
            ),
            Capability::new(
                "chrome.read",
                Tier::Shell,
                "lire l'habillage posé par la platforme",
                Some("plugin:this-shell|env"),
                &[Ios, Ipados, Macos, Windows, Linux, Android],
                Status::Live,
            ),
            Capability::new(
                "tray",
                Tier::Shell,
                "icône et menu de zone de notification",
                Some("tray.*"),
                &[Macos, Windows, Linux],
                Status::Missing,
            ),
            Capability::new(
                "notify",
                Tier::Shell,
                "notifier l'utilisateur",
                Some("notification.*"),
                &[Ios, Ipados, Macos, Windows, Linux, Android],
                Status::Missing,
            ),
            Capability::new(
                "haptics",
                Tier::Shell,
                "retour haptique",
                Some("haptics.*"),
                &[Ios, Android],
                Status::Missing,
            ),
            Capability::new(
                "share",
                Tier::Shell,
                "feuille de partage du système",
                Some("share.*"),
                &[Ios, Ipados, Macos, Android],
                Status::Missing,
            ),
            Capability::new(
                "deep_link",
                Tier::Shell,
                "ouvrir l'app sur une adresse",
                Some("deeplink.*"),
                &[Ios, Ipados, Macos, Windows, Linux, Android],
                Status::Missing,
            ),
            Capability::new(
                "device.events",
                Tier::Shell,
                "clavier, gestes, batterie, réseau, orientation",
                Some("device.*"),
                &[Ios, Ipados, Macos, Windows, Linux, Android],
                Status::Missing,
            ),
            Capability::new(
                "background",
                Tier::Shell,
                "travail en arrière-plan et resynchronisation",
                Some("background.*"),
                &[Ios, Android, Macos, Windows, Linux],
                Status::Missing,
            ),
            Capability::new(
                "activity",
                Tier::Shell,
                "Live Activity / Dynamic Island",
                Some("activity.*"),
                &[Ios],
                Status::Missing,
            ),
            Capability::new(
                "model.call",
                Tier::Shell,
                "appeler un modèle via le relais",
                Some("chat_stream / model_detect"),
                &[Ios, Ipados, Macos, Windows, Linux],
                Status::Live,
            ),
            Capability::new(
                "file.pick",
                Tier::Shell,
                "choisir un fichier ou une photo",
                Some("dialog.*"),
                &[Ios, Ipados, Macos, Windows, Linux, Android],
                Status::Missing,
            ),
            // ---- étage natif
            Capability::new(
                "widget",
                Tier::Native,
                "widget d'écran d'accueil",
                None,
                &[Ios, Ipados, Macos, Android],
                Status::Missing,
            ),
            Capability::new(
                "watch",
                Tier::Native,
                "application montre compagnon",
                None,
                &[Watchos],
                Status::Missing,
            ),
            Capability::new(
                "carplay",
                Tier::Native,
                "interface CarPlay (entitlement Apple)",
                None,
                &[Carplay],
                Status::Missing,
            ),
            Capability::new(
                "android_auto",
                Tier::Native,
                "interface Android Auto",
                None,
                &[AndroidAuto],
                Status::Missing,
            ),
            Capability::new(
                "intents",
                Tier::Native,
                "Raccourcis / Siri / App Intents",
                None,
                &[Ios, Ipados, Macos],
                Status::Missing,
            ),
        ],
        slots: vec![
            Slot::new(
                "module.screen",
                "l'écran du module",
                "ctx.el, ctx.module",
                Status::Live,
            ),
            Slot::new(
                "chrome",
                "habillage statique (barre latérale, accent, champ)",
                "déclaratif, sans code",
                Status::Live,
            ),
            Slot::new(
                "boot",
                "au démarrage, avant le premier tour",
                "ctx.app, ctx.store — peut bloquer",
                Status::Missing,
            ),
            Slot::new(
                "message",
                "le corps d'une bulle",
                "ctx.contenu, ctx.role — retourne du texte ou un rendu",
                Status::Missing,
            ),
            Slot::new(
                "composer",
                "le champ de saisie",
                "ctx.el, ctx.envoyer",
                Status::Missing,
            ),
            Slot::new(
                "tool",
                "l'affichage d'un appel d'outil",
                "ctx.outil, ctx.arguments, ctx.resultat",
                Status::Missing,
            ),
            Slot::new(
                "settings",
                "un écran de réglages",
                "ctx.el, ctx.provider",
                Status::Live,
            ),
        ],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn la_carte_ne_promet_rien_quelle_ne_livre() {
        let carte = surface();
        // Ce qui est mesuré comme absent doit être dit absent.
        assert_eq!(
            carte.capability("ui.markdown").unwrap().status,
            Status::Missing
        );
        assert_eq!(carte.capability("ui.render").unwrap().status, Status::Live);
        assert_eq!(carte.slot("boot").unwrap().status, Status::Missing);
        assert_eq!(carte.slot("module.screen").unwrap().status, Status::Live);
    }

    #[test]
    fn une_capacite_de_coquille_dit_sa_commande() {
        let carte = surface();
        for capacite in carte
            .capabilities
            .iter()
            .filter(|c| c.tier == Tier::Shell && c.status == Status::Live)
        {
            assert!(
                capacite.command.is_some(),
                "{} est de l'étage coquille et doit nommer sa commande",
                capacite.id
            );
        }
    }

    #[test]
    fn le_natif_n_est_jamais_une_commande_de_coquille() {
        let carte = surface();
        for capacite in carte.capabilities.iter().filter(|c| c.tier == Tier::Native) {
            // Une cible native n'est pas une commande : elle se pilote par un
            // contrat, elle ne s'invoque pas.
            assert!(
                capacite.command.is_none(),
                "{} est natif, pas une commande",
                capacite.id
            );
        }
    }

    #[test]
    fn la_carte_se_dit_dans_un_budget_et_avoue_ses_coupes() {
        let carte = surface();
        let large = carte.describe(100_000);
        assert!(large.contains("ui.markdown"));
        assert!(large.contains("pas encore"));
        assert!(!large.contains("omise"));

        let serre = carte.describe(200);
        assert!(
            serre.contains("omise"),
            "une carte tronquée doit dire qu'elle l'est"
        );
    }

    #[test]
    fn chaque_capacite_a_un_identifiant_unique_et_au_moins_une_plateforme() {
        let carte = surface();
        let mut vus = std::collections::BTreeSet::new();
        for capacite in &carte.capabilities {
            assert!(
                vus.insert(capacite.id.clone()),
                "identifiant en double : {}",
                capacite.id
            );
            assert!(
                !capacite.platforms.is_empty(),
                "{} n'existe sur aucune plateforme",
                capacite.id
            );
        }
    }

    #[test]
    fn les_etages_sont_ordonnes_dans_le_rendu() {
        let texte = surface().describe(100_000);
        let page = texte.find("[webview]").unwrap();
        let coquille = texte.find("[coquille]").unwrap();
        let natif = texte.find("[natif]").unwrap();
        assert!(page < coquille && coquille < natif);
    }
}

#[cfg(test)]
mod couverture {
    use super::*;

    #[test]
    fn les_noms_d_etage_et_de_plateforme_sont_tous_couverts() {
        // Une capacité qui nomme mal son étage est illisible dans le prompt.
        assert_eq!(Tier::Webview.as_str(), "webview");
        assert_eq!(Tier::Shell.as_str(), "coquille");
        assert_eq!(Tier::Native.as_str(), "natif");
        for (plateforme, nom) in [
            (Platform::Web, "web"),
            (Platform::Ios, "ios"),
            (Platform::Ipados, "ipados"),
            (Platform::Macos, "macos"),
            (Platform::Windows, "windows"),
            (Platform::Linux, "linux"),
            (Platform::Android, "android"),
            (Platform::Watchos, "watchos"),
            (Platform::Carplay, "carplay"),
            (Platform::AndroidAuto, "android_auto"),
        ] {
            assert_eq!(plateforme.as_str(), nom);
        }
    }

    #[test]
    fn les_accesseurs_rendent_none_sur_un_identifiant_absent() {
        let carte = surface();
        assert!(carte.capability("n.existe.pas").is_none());
        assert!(carte.slot("n.existe.pas").is_none());
        assert!(carte.capability("ui.render").is_some());
        assert!(carte.slot("message").is_some());
    }

    #[test]
    fn la_carte_se_partitionne_entre_livre_et_attendu() {
        let carte = surface();
        let livres = carte.live().count();
        let attendus = carte.missing().count();
        assert_eq!(livres + attendus, carte.capabilities.len());
        assert!(
            livres > 0 && attendus > 0,
            "les deux familles doivent exister"
        );
    }

    #[test]
    fn un_budget_nul_tronque_tout_et_le_dit() {
        // Le cas extrême : rien ne tient. La carte doit rester utilisable et
        // annoncer qu'elle est vide plutôt que de rendre une chaîne trompeuse.
        let texte = surface().describe(0);
        assert!(texte.contains("omise"));
        assert!(!texte.contains("[webview]"));
    }

    #[test]
    fn les_emplacements_manquants_sont_nommes_meme_sans_capacite() {
        let texte = surface().describe(100_000);
        assert!(texte.contains("Emplacements pas encore ouverts"));
        assert!(texte.contains("boot") && texte.contains("message"));
    }
}
