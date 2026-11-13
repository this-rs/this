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

    /// La carte restreinte à une plateforme : l'agent ne doit entendre que ce qui existe sur
    /// l'appareil où il tourne, pas un catalogue toutes plateformes confondues. Un iPhone n'a
    /// pas à savoir qu'une capacité Mac existe, et inversement.
    pub fn for_platform(&self, platform: Platform) -> Surface {
        Surface {
            capabilities: self
                .capabilities
                .iter()
                .filter(|c| c.platforms.contains(&platform))
                .cloned()
                .collect(),
            slots: self.slots.clone(),
        }
    }

    /// La carte telle que l'agent la lit sur une plateforme donnée : filtrée, et préfixée du
    /// nom de l'appareil, pour que « disponible » veuille dire « disponible ICI ».
    /// Ajoute les entrées d'un hôte à la carte.
    ///
    /// `this-agent` décrit ce que le HARNAIS fournit (le tour d'outils : modules, graphe,
    /// mémoire, rangées, modèle). Ce qu'une coquille fournit en plus — sa fenêtre, son bac, ses
    /// notifications, son pont d'appareil — ne regarde qu'elle : c'est à l'hôte de le déclarer
    /// au lieu de le voir écrit en dur dans le framework.
    ///
    /// Une entrée de l'hôte portant un identifiant déjà présent **remplace** celle du framework :
    /// l'hôte sait mieux que le framework ce que sa coquille expose. Les emplacements se
    /// fusionnent de la même façon.
    pub fn with_host_entries(&self, host: Surface) -> Surface {
        let mut surface = self.clone();
        for entree in host.capabilities {
            match surface
                .capabilities
                .iter_mut()
                .find(|existante| existante.id == entree.id)
            {
                Some(existante) => *existante = entree,
                None => surface.capabilities.push(entree),
            }
        }
        for emplacement in host.slots {
            match surface
                .slots
                .iter_mut()
                .find(|existant| existant.id == emplacement.id)
            {
                Some(existant) => *existant = emplacement,
                None => surface.slots.push(emplacement),
            }
        }
        surface
    }

    /// La carte telle que CE build la peut : les entrées déclarées indisponibles passent en
    /// `Missing`, et leur titre porte la raison.
    ///
    /// Un identifiant inconnu est ignoré : un hôte qui déclare un manque sur une entrée
    /// supprimée depuis ne doit pas faire tomber la carte.
    pub fn with_limits(&self, limits: &HostLimits) -> Surface {
        let mut surface = self.clone();
        for entree in surface.capabilities.iter_mut() {
            if let Some((_, raison)) = limits.entries().iter().find(|(id, _)| *id == entree.id) {
                entree.status = Status::Missing;
                entree.title = format!("{} — indisponible sur ce build : {raison}", entree.title);
            }
        }
        surface
    }

    pub fn describe_for(&self, platform: Platform, budget_chars: usize) -> String {
        format!(
            "Plateforme : {}\n{}",
            platform.as_str(),
            self.for_platform(platform).describe(budget_chars)
        )
    }
}

/// Ce que le BUILD d'un hôte ne peut pas faire, alors que le pont, lui, sait le faire.
///
/// Un hôte n'est pas le pont. Il peut manquer une phrase dans son `Info.plist`, un entitlement
/// que son profil de signature ne porte pas, ou une permission que son manifeste oublie. La
/// carte décrit le PONT (`surface()`) ; [`Surface::with_limits`] la fait parler du BUILD.
///
/// Rien ici ne connaît un hôte en particulier : c'est l'hôte qui déclare ses manques, avec leur
/// raison. Un framework ne peut pas deviner ce qu'un profil de signature contient.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HostLimits {
    unavailable: Vec<(String, String)>,
}

impl HostLimits {
    pub fn new() -> Self {
        Self::default()
    }

    /// Déclare qu'une entrée de la carte n'est pas disponible dans ce build, et pourquoi.
    ///
    /// La raison est montrée à l'agent : « profil de signature sans l'entitlement HealthKit »
    /// lui dit qu'insister ne servira à rien, ce qu'un simple `not_authorized` ne dit pas.
    pub fn unavailable(mut self, id: &str, reason: &str) -> Self {
        self.unavailable.push((id.to_string(), reason.to_string()));
        self
    }

    pub fn is_empty(&self) -> bool {
        self.unavailable.is_empty()
    }

    /// Les entrées déclarées, dans l'ordre où l'hôte les a nommées.
    pub fn entries(&self) -> &[(String, String)] {
        &self.unavailable
    }

    /// Réunit deux déclarations de limites : celle de l'hôte, écrite à la main, et celle que
    /// l'appareil a **répondue**.
    ///
    /// L'ordre est celui de l'appel : ce que l'hôte sait de son propre build d'abord, ce que la
    /// mesure a trouvé ensuite. Un identifiant déjà déclaré garde sa première raison — celle de
    /// l'hôte est plus précise (« profil de signature sans l'entitlement HealthKit ») que celle
    /// du banc (« l'appareil ne fournit pas cette capacité »), et c'est celle-là qu'un humain
    /// doit lire.
    ///
    /// Pourquoi une fusion et pas un remplacement : un hôte a les deux à dire. L'Atelier déclare
    /// `device.health` et `device.nfc` à cause de son profil de signature, et l'appareil peut en
    /// plus ne pas avoir de podomètre. Remplacer l'une par l'autre perdrait la moitié de la
    /// vérité.
    pub fn and(mut self, autre: &HostLimits) -> Self {
        for (id, raison) in &autre.unavailable {
            if !self.unavailable.iter().any(|(connu, _)| connu == id) {
                self.unavailable.push((id.clone(), raison.clone()));
            }
        }
        self
    }

    /// Ce qu'un **appareil** a répondu quand on le lui a demandé.
    ///
    /// Prend le rapport du banc d'essai du pont (`tauri-plugin-this-device`, feature `banc`) et
    /// en tire les limites du build : une capacité dont l'état mesuré est `unsupported` n'existe
    /// pas sur cet appareil, et l'agent doit le lire au lieu de promettre.
    ///
    /// Pourquoi c'est ici et pas chez l'hôte : la forme du rapport est celle du contrat
    /// (`device_bridge`), donc sa lecture aussi. Un hôte qui écrirait cette conversion lui-même
    /// la réécrirait à chaque hôte, avec les mêmes clés devinées.
    ///
    /// **Un état de permission n'est pas une limite.** `denied` et `prompt` sont des décisions
    /// de la personne, réversibles dans les Réglages : les déclarer « indisponibles » ferait
    /// disparaître une capacité parce qu'on a répondu « plus tard » une fois. Seul
    /// `unsupported` — le matériel ou le système ne sait pas — est une limite.
    pub fn from_bench(report: &serde_json::Value) -> Self {
        let mut limits = Self::new();
        let Some(entries) = report.get("capacites").and_then(|value| value.as_array()) else {
            return limits;
        };
        for entry in entries {
            let (Some(capability), Some("unsupported")) = (
                entry.get("capacite").and_then(|value| value.as_str()),
                entry.get("etat").and_then(|value| value.as_str()),
            ) else {
                continue;
            };
            // L'identifiant de la carte est `device.<nom de fil>`, comme partout ailleurs.
            let id = format!("device.{capability}");
            if !limits.unavailable.iter().any(|(known, _)| *known == id) {
                limits = limits.unavailable(
                    &id,
                    "l'appareil ne fournit pas cette capacité (mesuré par le banc du pont)",
                );
            }
        }
        limits
    }
}

/// Le budget en caractères dans lequel la carte d'un appareil doit tenir.
///
/// C'est ce que `capacites` passe à [`Surface::describe_for`]. Il est ici, et non chez
/// l'appelant, pour qu'une capacité ajoutée se heurte à un test plutôt qu'à une troncature
/// silencieuse — le défaut d'origine : la carte ne contenait pas l'appareil parce que rien
/// ne mesurait ce qu'elle contenait.
///
/// Mesuré le 08/11/2026 après le chantier appareil, carte entière : **7 907 caractères sur
/// iOS** (30 entrées d'appareil), 5 557 sur macOS, 664 en navigateur. L'ancien budget de
/// 8 000 était à 93 caractères de la troncature : un ajout et la moitié de l'appareil
/// disparaissait sans le dire. Relevé à 12 000, soit la place d'une poignée d'entrées —
/// et le test ci-dessous échouera avant que la marge ne soit mangée.
pub const BUDGET_CARTE: usize = 12_000;

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
                "modules",
                Tier::Webview,
                "installer, relire, lister et exécuter des modules (entités, programmes, chrome). Un programme peut appeler l'appareil et le réseau (device, device_int, device_emit, http_int, http_emit) : il s'exécute alors dans la coquille, et le consentement est demandé au nom du module, une fois par capacité",
                None,
                &[Web, Ios, Ipados, Macos, Windows, Linux, Android],
                Status::Live,
            ),
            Capability::new(
                "ui.render",
                Tier::Webview,
                "rendre un écran (via le code du module, pas par toi)",
                None,
                &[Web, Ios, Ipados, Macos, Windows, Linux, Android],
                Status::Live,
            ),
            Capability::new(
                "ui.markdown",
                Tier::Webview,
                "rendre du markdown dans une bulle (titres, listes, citations, code, tableaux)",
                None,
                &[Web, Ios, Ipados, Macos, Windows, Linux, Android],
                Status::Live,
            ),
            Capability::new(
                "store.local",
                Tier::Webview,
                "stocker dans la page (via le code du module)",
                None,
                &[Web, Ios, Ipados, Macos, Windows, Linux, Android],
                Status::Live,
            ),
            Capability::new(
                "http.fetch",
                Tier::Webview,
                "appeler HTTP (via le code du module)",
                None,
                &[Web, Ios, Ipados, Macos, Windows, Linux, Android],
                Status::Live,
            ),
            Capability::new(
                "fs.index",
                Tier::Webview,
                "lister les fichiers du VFS de la coquille (artefacts compilés, modules)",
                None,
                &[Web, Ios, Ipados, Macos, Windows, Linux, Android],
                Status::Live,
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
                None,
                &[Macos, Windows, Linux],
                Status::Missing,
            ),
            Capability::new(
                "chrome.read",
                Tier::Shell,
                "lire l'habillage posé par la platforme",
                Some("get_chrome / set_chrome"),
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
                "notifier la personne : poster ou retirer une notification locale",
                Some("notify (notificationsPost / notificationsCancel)"),
                &[Ios, Ipados, Macos, Windows, Linux, Android],
                Status::Live,
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
                "ouvrir la feuille de partage du système (texte, lien, image, fichier de l'app)",
                Some("shareText / shareItem"),
                &[Ios, Ipados, Macos, Android],
                Status::Live,
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
            // ---- état système : lectures passives, sans consentement
            //
            // Modèle, batterie, réseau, langue, écran, volume : ce que la coquille sait de
            // l'appareil SANS demander de permission ni porter d'entitlement. Chemin distinct
            // du pont `device.*` (consenti) : une commande unique `system_state`, et chaque
            // méthode est un sélecteur Swift du plugin (`ios/Sources/system_state.swift`).
            Capability::new(
                "device_info",
                Tier::Shell,
                "appareil : modèle, système et version",
                Some("system_state (deviceInfo)"),
                &[Ios, Ipados],
                Status::Live,
            ),
            Capability::new(
                "battery",
                Tier::Shell,
                "niveau et état de charge de la batterie",
                Some("system_state (batteryLevel)"),
                &[Ios, Ipados],
                Status::Live,
            ),
            Capability::new(
                "network_state",
                Tier::Shell,
                "connectivité réseau : connecté ou non, et le type d'interface",
                Some("system_state (networkState)"),
                &[Ios, Ipados],
                Status::Live,
            ),
            Capability::new(
                "locale",
                Tier::Shell,
                "langue et région du système",
                Some("system_state (locale)"),
                &[Ios, Ipados],
                Status::Live,
            ),
            Capability::new(
                "screen_state",
                Tier::Shell,
                "luminosité et échelle de l'écran",
                Some("system_state (screenState)"),
                &[Ios, Ipados],
                Status::Live,
            ),
            // Le volume système n'est PAS lisible sur iOS sans session audio active : la
            // méthode `volume` du pont le refuse honnêtement (`not_supported`). La capacité
            // reste donc `Missing`, avec la raison — une information, pas un échec.
            Capability::new(
                "volume",
                Tier::Shell,
                "volume du système — indisponible sur iOS : aucune lecture du volume système sans session audio active",
                Some("system_state (volume)"),
                &[Ios, Ipados],
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
                "Live Activity / Dynamic Island : démarrer, mettre à jour, terminer",
                Some(
                    "liveActivityStart / liveActivityUpdate / liveActivityEnd / liveActivityStatus",
                ),
                &[Ios, Ipados],
                Status::Live,
            ),
            Capability::new(
                "model.call",
                Tier::Shell,
                "appeler le modèle (le harnais le fait pour toi, tu n'as pas d'outil)",
                Some("chat_stream"),
                &[Ios, Ipados, Macos, Windows, Linux],
                Status::Live,
            ),
            Capability::new(
                "graph",
                Tier::Shell,
                "lire et écrire des nœuds et des arêtes du graphe de l'app",
                Some("graph_put / graph_link / graph_query"),
                &[Ios, Ipados, Macos],
                Status::Live,
            ),
            Capability::new(
                "memory",
                Tier::Shell,
                "notes durables de l'agent, retrouvées par le graphe",
                Some("remember / recall / forget"),
                &[Ios, Ipados, Macos],
                Status::Live,
            ),
            Capability::new(
                "rows",
                Tier::Shell,
                "lignes des entités déclarées par les modules, persistées dans le graphe",
                Some("row_list / row_create / row_patch / row_remove"),
                &[Ios, Ipados, Macos],
                Status::Live,
            ),
            Capability::new(
                "file.pick",
                Tier::Shell,
                "choisir un fichier ou une photo dans le sélecteur système, et relire un fichier choisi",
                Some("filesPick / filesRead / photosPick"),
                &[Ios, Ipados, Macos, Windows, Linux, Android],
                Status::Live,
            ),
            // Ce qui reste VRAI sur les autres systèmes, et qu'il faut dire plutôt que de
            // laisser un vide : le pont natif n'existe que pour iOS, iPadOS et une partie de
            // macOS. Sans cette entrée, la carte d'un Android ne parlait pas du tout de
            // l'appareil — l'agent ne pouvait pas distinguer « pas de pont » de « je ne sais
            // pas », et la seule réponse honnête (« il n'y a pas de pont ici ») n'était écrite
            // nulle part.
            Capability::new(
                "device",
                Tier::Native,
                "appareil : aucun pont natif sur cette plateforme — les capacités d'appareil existent sur iOS/iPadOS et, en partie, sur macOS",
                None,
                &[Android, Windows, Linux, Web],
                Status::Missing,
            ),
            // ---- étage coquille : l'appareil
            //
            // Une entrée par capacité du pont natif (`this_platform::capability::Capability::ALL`),
            // identifiant `device.<nom de fil>`, et une commande qui nomme les méthodes
            // réellement exposées par `this_platform::device_bridge::METHODS`.
            //
            // POURQUOI CE BLOC EXISTE. La carte ne parlait pas de l'appareil du tout : ni
            // contacts, ni calendrier, ni santé, ni Bluetooth. L'agent, qui lit cette carte
            // (`capacites`), répondait donc « pas encore » pour des capacités livrées depuis
            // la mise en place du pont — et la personne concluait que rien ne marchait.
            //
            // `Platform` n'est pas décoratif : une famille que `macos/Sources/dispatch.swift`
            // ne traite pas (santé, mouvement, NFC, gyroscope, activité en direct) n'est pas
            // annoncée sur macOS. Vérifié par `tauri-plugin-this-device` :
            // `tests/surface_parity.rs` compare cette liste à `METHODS`, et
            // `tests/swift_parity.rs` compare `METHODS` aux sélecteurs natifs.
            //
            // CE QUI RESTE PARTIEL, et qui le dit : le sélecteur de fichiers n'a pas de mode
            // écriture (on écrit dans le conteneur), et la santé ne s'écrit pas encore.
            // Les autres profondeurs manquantes, mesurées :
            // localisation (flux continu, adresse), parole (dictée live), notifications
            // (planifier, lister), mouvement (distance, étages), santé (écriture, workouts),
            // fichiers (écrire dans le sélecteur).
            //
            // Une capacité peut être plus profonde sur un appareil que sur l'autre : le
            // Bluetooth se connecte sur iPhone et se contente de scanner sur Mac. D'où les
            // entrées `device.<nom>.<aspect>`, que `tests/surface_parity.rs` accepte comme
            // faisant partie de la capacité `device.<nom>`.
            Capability::new(
                "device.notifications",
                Tier::Shell,
                "notifications locales : état, demande d'autorisation, poster, retirer",
                Some(
                    "notificationsStatus / notificationsRequest / notificationsPost / notificationsCancel",
                ),
                &[Ios, Ipados, Macos],
                Status::Live,
            ),
            Capability::new(
                "device.notifications.schedule",
                Tier::Shell,
                "PLANIFIER une notification (1 s à 30 jours) et lire celles qui attendent",
                Some("notificationsSchedule / notificationsPending"),
                // macOS depuis le 13/11/2026 : UserNotifications offre le déclencheur de temps
                // et la liste des attentes, les mêmes API que sur iOS. La carte ne le disait pas
                // parce que le pont ne les routait pas.
                &[Ios, Ipados, Macos],
                Status::Live,
            ),
            Capability::new(
                "device.camera",
                Tier::Shell,
                "caméra : état de la permission et demande ; la capture est sous device.camera.capture",
                Some("captureStatus / captureRequest"),
                &[Ios, Ipados, Macos, Linux],
                Status::Live,
            ),
            Capability::new(
                "device.microphone",
                Tier::Shell,
                "microphone : état de la permission et demande ; l'enregistrement est sous device.microphone.record",
                Some("captureStatus / captureRequest"),
                &[Ios, Ipados, Macos, Linux],
                Status::Live,
            ),
            // Le rattachement est porté par le préfixe : `device.camera.capture` est un aspect
            // de `device.camera`, pas une capacité de plus.
            Capability::new(
                "device.camera.capture",
                Tier::Shell,
                "prendre une photo (JPEG borné en pixels et en qualité)",
                Some("cameraCapturePhoto"),
                &[Ios, Ipados],
                Status::Live,
            ),
            Capability::new(
                "device.camera.video",
                Tier::Shell,
                "filmer un clip (1 à 30 s) : le fichier va dans le conteneur de l'app",
                Some("cameraRecordVideo"),
                &[Ios, Ipados],
                Status::Live,
            ),
            Capability::new(
                "device.microphone.record",
                Tier::Shell,
                "enregistrer une note audio bornée, écrite dans un fichier de l'app",
                Some("microphoneStart / microphoneStop"),
                &[Ios, Ipados],
                Status::Live,
            ),
            Capability::new(
                "device.location",
                Tier::Shell,
                "état de la permission, demande d'autorisation, position ponctuelle",
                // `locationStatus` : l'ÉTAT, qui manquait — sans lui le pont répondait
                // `unsupported` pour une capacité que l'appareil a.
                Some("locationStatus / locationRequest / locationSample"),
                &[Ios, Ipados, Macos],
                Status::Live,
            ),
            // Ce que Linux sait de la localisation : la position, sans l'ÉTAT (GeoClue2 n'a pas
            // d'autorisation à lire, il en a une à demander) ni flux. Une entrée à part plutôt
            // qu'ajouter Linux à `device.location`, qui promet `locationStatus`.
            Capability::new(
                "device.location.fix",
                Tier::Shell,
                "demander l'autorisation et lire une position ponctuelle (sans état ni flux)",
                Some("locationRequest / locationSample"),
                &[Linux],
                Status::Live,
            ),
            // Le flux de positions : iOS seulement (le pont macOS n'a que le point).
            Capability::new(
                "device.location.stream",
                Tier::Shell,
                "démarrer un flux de positions, l'arrêter, relire ce qui a été reçu",
                Some("locationStreamStart / locationStreamStop / locationRecent"),
                &[Ios, Ipados],
                Status::Live,
            ),
            Capability::new(
                "device.location_always",
                Tier::Shell,
                "localisation en arrière-plan",
                Some("locationStatus / locationRequest"),
                &[Ios, Ipados],
                Status::Live,
            ),
            Capability::new(
                "device.clipboard",
                Tier::Shell,
                "lire et écrire du TEXTE dans le presse-papiers",
                Some("clipboardRead / clipboardWrite"),
                &[Ios, Ipados, Macos],
                Status::Live,
            ),
            // Un lien ou une image dans le presse-papiers. Porté sur macOS le 08/11.
            Capability::new(
                "device.clipboard.media",
                Tier::Shell,
                "lire et écrire un LIEN ou une IMAGE dans le presse-papiers (4 Mio au plus)",
                Some("clipboardRead / clipboardWrite"),
                &[Ios, Ipados, Macos],
                Status::Live,
            ),
            Capability::new(
                "device.share",
                Tier::Shell,
                "ouvrir la feuille de partage système avec du texte",
                Some("shareText"),
                &[Ios, Ipados, Macos],
                Status::Live,
            ),
            // Le partage d'un lien, d'une image ou d'un fichier. Sur macOS le picker
            // (`NSSharingServicePicker`) répond « présenté », pas « partagé » : macOS n'a pas de
            // callback de fermeture fiable, et le code le dit au lieu de le laisser croire.
            Capability::new(
                "device.share.item",
                Tier::Shell,
                "partager un lien, une image ou un fichier de l'app",
                Some("shareItem"),
                &[Ios, Ipados, Macos],
                Status::Live,
            ),
            Capability::new(
                "device.live_activity",
                Tier::Shell,
                "activité en direct : démarrer, mettre à jour, terminer",
                Some(
                    "liveActivityStatus / liveActivityStart / liveActivityUpdate / liveActivityEnd",
                ),
                &[Ios, Ipados],
                Status::Live,
            ),
            Capability::new(
                "device.contacts",
                Tier::Shell,
                "carnet d'adresses : lister (borné) et demander l'autorisation",
                Some("contactsStatus / contactsRequest / contactsList"),
                &[Ios, Ipados, Macos],
                Status::Live,
            ),
            // Chercher, lire un contact précis et en créer un. Porté sur macOS le 08/11 :
            // `macos/Sources/contacts.swift` implémente les trois.
            Capability::new(
                "device.contacts.more",
                Tier::Shell,
                "chercher un contact par nom, en lire un par son identifiant, en créer un",
                Some("contactsGet / contactsSearch / contactsCreate"),
                &[Ios, Ipados, Macos],
                Status::Live,
            ),
            Capability::new(
                "device.photos",
                Tier::Shell,
                "photothèque : état de la permission et demande (la lecture est sous device.photos.read)",
                Some("photosStatus / photosRequest"),
                &[Ios, Ipados, Macos],
                Status::Live,
            ),
            Capability::new(
                "device.photos.read",
                Tier::Shell,
                "lire la photothèque : métadonnées d'une fenêtre, puis les octets d'UNE photo (bornés en pixels)",
                Some("photosList / photosRead"),
                &[Ios, Ipados, Macos],
                Status::Live,
            ),
            Capability::new(
                "device.photos_pick",
                Tier::Shell,
                "choisir des photos dans le sélecteur système et recevoir leurs octets",
                Some("photosPick"),
                &[Ios, Ipados, Macos],
                Status::Live,
            ),
            Capability::new(
                "device.files_pick",
                Tier::Shell,
                "choisir des fichiers, puis les relire par signet (bookmark)",
                Some("filesPick / filesRead"),
                &[Ios, Ipados, Macos],
                Status::Live,
            ),
            // Écrire un fichier produit par l'app : iOS seulement.
            Capability::new(
                "device.files_pick.write",
                Tier::Shell,
                "écrire un fichier produit par l'app, puis le partager",
                Some("filesWrite"),
                &[Ios, Ipados, Macos],
                Status::Live,
            ),
            Capability::new(
                "device.bluetooth",
                Tier::Shell,
                "Bluetooth : état de la permission et SCAN des appareils à portée",
                Some("status / bluetoothScan"),
                &[Ios, Ipados, Macos, Linux],
                Status::Live,
            ),
            Capability::new(
                "device.bluetooth.connect",
                Tier::Shell,
                "se connecter à un périphérique, lire ses services, lire et écrire une caractéristique, s'abonner à ses notifications (valeurs tirées, pas poussées)",
                Some(
                    "bluetoothState / bluetoothConnect / bluetoothDisconnect / bluetoothServices / bluetoothRead / bluetoothWrite / bluetoothSubscribe / bluetoothNotifications",
                ),
                // iOS d'abord : le pont macOS ne sait toujours que scanner (`dispatch.swift`
                // répond `not_supported` à ces huit méthodes, et la carte ne les annonce donc
                // pas sur un Mac).
                &[Ios, Ipados],
                Status::Live,
            ),
            Capability::new(
                "device.gyroscope",
                Tier::Shell,
                "un échantillon du gyroscope (aucun flux continu)",
                Some("status / gyroscopeSample"),
                &[Ios, Ipados, Linux],
                Status::Live,
            ),
            Capability::new(
                "device.motion_activity",
                Tier::Shell,
                "pas, activité physique, DISTANCE et ÉTAGES sur une fenêtre bornée",
                Some(
                    "motionActivityStatus / motionActivityRequest / motionStepsSample / motionDistance",
                ),
                &[Ios, Ipados],
                Status::Live,
            ),
            Capability::new(
                "device.calendar",
                Tier::Shell,
                "calendrier : lire une fenêtre d'événements et en créer un",
                Some("calendarStatus / calendarRequest / calendarEvents / calendarCreateEvent"),
                &[Ios, Ipados, Macos],
                Status::Live,
            ),
            // Les calendriers eux-mêmes, la modification et la suppression — portés sur macOS.
            Capability::new(
                "device.calendar.edit",
                Tier::Shell,
                "lister les calendriers et leur droit d'écriture, modifier et supprimer un événement",
                Some("calendarCalendars / calendarUpdateEvent / calendarDeleteEvent"),
                &[Ios, Ipados, Macos],
                Status::Live,
            ),
            // L'ÉCRITURE N'EST PAS EXPOSÉE, décision du 08/11/2026. `healthRequest` ne demande
            // aujourd'hui que la lecture (`toShare: []`), et c'est volontaire : HealthKit ne
            // permet pas de savoir ensuite ce que l'app a écrit, donc une écriture décidée par
            // un modèle dans une boucle d'outils serait irréversible ET non attribuable. La
            // personne lirait « 12 000 pas » sans pouvoir dire d'où ça vient. Si l'écriture
            // devient nécessaire, elle passe par une capacité distincte avec son propre
            // consentement et une confirmation à chaque geste — pas par un argument oublié.
            Capability::new(
                "device.health",
                Tier::Shell,
                "santé : LECTURE seule — une mesure (pas, fréquence cardiaque, masse) sur 24 h au plus, et les séances sur un an au plus ; l'écriture n'est pas exposée",
                Some("healthStatus / healthRequest / healthQuantitySample / healthWorkouts"),
                &[Ios, Ipados],
                Status::Live,
            ),
            Capability::new(
                "device.face_id",
                Tier::Shell,
                "confirmer une action par Face ID / Touch ID",
                Some("faceIdStatus / faceIdConfirm"),
                &[Ios, Ipados, Macos],
                Status::Live,
            ),
            Capability::new(
                "device.speech",
                Tier::Shell,
                "transcrire un fichier audio (1 à 60 s), sur l'appareil — pour une dictée : microphoneStart, puis microphoneStop, puis ce fichier",
                Some("speechStatus / speechRequest / speechTranscribeFile"),
                &[Ios, Ipados, Macos],
                Status::Live,
            ),
            Capability::new(
                "device.nfc",
                Tier::Shell,
                "lire une étiquette NFC, et lui ÉCRIRE un texte",
                Some("nfcStatus / nfcReadOnce / nfcWrite"),
                &[Ios, Ipados],
                Status::Live,
            ),
            Capability::new(
                "device.local_network",
                Tier::Shell,
                "voir les services Bonjour du réseau local (_http._tcp)",
                Some("localNetworkStatus / localNetworkBrowse"),
                &[Ios, Ipados, Macos],
                Status::Live,
            ),
            // Résoudre un service en hôte et port : iOS seulement (le pont macOS ne l'a pas).
            Capability::new(
                "device.local_network.resolve",
                Tier::Shell,
                "résoudre un service trouvé en hôte et port",
                Some("localNetworkResolve"),
                &[Ios, Ipados],
                Status::Live,
            ),
            // ---- étage coquille : le matériel et le système PAS ENCORE LIVRÉS
            //
            // Suite du chantier appareil : la carte nomme ce qu'un module POURRA appeler, et
            // tout est `Missing` parce que rien ne le sert aujourd'hui. Chaque titre dit
            // POURQUOI (permission du système, entitlement du profil, ou pont natif à écrire) —
            // c'est ce qui permet de la faire passer `Live` une par une en nommant le vrai
            // obstacle, au lieu d'inventer une commande qui n'existe pas.
            //
            // Aucune n'est `Live` (le marquer serait un mensonge). Ce qui existe DÉJÀ — caméra,
            // micro, Bluetooth, NFC, gyroscope, localisation, presse-papiers, sélecteur de
            // fichiers, partage, biométrie, contacts, calendrier, photos, santé, parole, pas et
            // fréquence cardiaque — reste sous `device.*` ou son nom court ; on ne le duplique
            // pas ici sous un second nom.
            Capability::new(
                "audio_playback",
                Tier::Shell,
                "lire un son : aucun pont natif n'expose la lecture audio",
                None,
                &[Ios, Ipados, Macos, Android, Windows, Linux],
                Status::Missing,
            ),
            Capability::new(
                "video_playback",
                Tier::Shell,
                "lire une vidéo : aucun pont natif n'expose la lecture vidéo",
                None,
                &[Ios, Ipados, Macos, Android, Windows, Linux],
                Status::Missing,
            ),
            Capability::new(
                "wifi",
                Tier::Shell,
                "état et contrôle du Wi-Fi : pas de pont (permission réseau à exposer)",
                None,
                &[Ios, Ipados, Macos, Android, Windows, Linux],
                Status::Missing,
            ),
            Capability::new(
                "uwb",
                Tier::Shell,
                "bande ultra-large : entitlement UWB et pont à écrire (iPhone)",
                None,
                &[Ios],
                Status::Missing,
            ),
            Capability::new(
                "cellular_info",
                Tier::Shell,
                "opérateur et signal cellulaires : pas de pont (CoreTelephony)",
                None,
                &[Ios, Ipados, Android],
                Status::Missing,
            ),
            Capability::new(
                "accelerometer",
                Tier::Shell,
                "accéléromètre : pas de pont (seul le gyroscope est exposé)",
                None,
                &[Ios, Ipados, Android],
                Status::Missing,
            ),
            Capability::new(
                "magnetometer",
                Tier::Shell,
                "boussole : pas de pont",
                None,
                &[Ios, Ipados, Android],
                Status::Missing,
            ),
            Capability::new(
                "barometer",
                Tier::Shell,
                "pression atmosphérique : pas de pont",
                None,
                &[Ios, Ipados, Android],
                Status::Missing,
            ),
            Capability::new(
                "proximity",
                Tier::Shell,
                "détecteur de proximité : pas de pont",
                None,
                &[Ios, Ipados, Android],
                Status::Missing,
            ),
            Capability::new(
                "ambient_light",
                Tier::Shell,
                "luminosité ambiante : pas de pont",
                None,
                &[Ios, Ipados, Android],
                Status::Missing,
            ),
            Capability::new(
                "print",
                Tier::Shell,
                "imprimer via le système : pas de pont",
                None,
                &[Ios, Ipados, Macos, Android, Windows, Linux],
                Status::Missing,
            ),
            Capability::new(
                "document_scan",
                Tier::Shell,
                "scanner un document (Vision) : pas de pont",
                None,
                &[Ios, Ipados],
                Status::Missing,
            ),
            Capability::new(
                "notification_push",
                Tier::Shell,
                "notifications distantes (APNs/FCM) : entitlement et pont à écrire",
                None,
                &[Ios, Ipados, Android],
                Status::Missing,
            ),
            Capability::new(
                "badge",
                Tier::Shell,
                "pastille sur l'icône de l'app : pas de pont",
                None,
                &[Ios, Ipados, Macos, Android],
                Status::Missing,
            ),
            Capability::new(
                "keychain",
                Tier::Shell,
                "trousseau de clés : pas de pont (entitlement Keychain)",
                None,
                &[Ios, Ipados, Macos],
                Status::Missing,
            ),
            Capability::new(
                "secure_storage",
                Tier::Shell,
                "stockage chiffré persistant : pas de pont",
                None,
                &[Ios, Ipados, Macos, Android, Windows, Linux],
                Status::Missing,
            ),
            Capability::new(
                "text_to_speech",
                Tier::Shell,
                "synthèse vocale : pas de pont",
                None,
                &[Ios, Ipados, Macos, Android, Windows, Linux],
                Status::Missing,
            ),
            Capability::new(
                "ar_session",
                Tier::Shell,
                "session de réalité augmentée (ARKit) : entitlement et pont à écrire",
                None,
                &[Ios, Ipados],
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
            carte.capability("device.files_pick").unwrap().status,
            Status::Live
        );
        // La vidéo est arrivée : elle n'est plus annoncée manquante.
        assert_eq!(
            carte.capability("device.camera.video").unwrap().status,
            Status::Live
        );
        assert_eq!(
            carte.capability("device.files_pick").unwrap().status,
            Status::Live
        );
        // La vidéo est arrivée : elle n'est plus annoncée manquante.
        assert_eq!(
            carte.capability("device.camera.video").unwrap().status,
            Status::Live
        );
        assert_eq!(carte.capability("ui.render").unwrap().status, Status::Live);
        // Une commande qui n'existe pas n'a pas le droit de se dire disponible.
        assert_eq!(
            carte.capability("window.control").unwrap().status,
            Status::Missing
        );
        assert_eq!(carte.capability("haptics").unwrap().status, Status::Missing);
        // Le pouvoir premier de l'agent est dit, pas supposé.
        assert_eq!(carte.capability("modules").unwrap().status, Status::Live);
        assert_eq!(carte.slot("boot").unwrap().status, Status::Missing);
        assert_eq!(carte.slot("module.screen").unwrap().status, Status::Live);
    }

    /// Le rapport du banc devient les limites du build : c'est le seul chemin où ce que
    /// l'appareil a **répondu** arrive dans la carte que lit l'agent, sans qu'un développeur
    /// recopie une liste à la main.
    #[test]
    fn ce_que_l_appareil_a_repondu_devient_les_limites_du_build() {
        let rapport = serde_json::json!({
            "kind": "banc",
            "capacites": [
                {"capacite": "health", "etat": "unsupported"},
                {"capacite": "nfc", "etat": "unsupported"},
                {"capacite": "contacts", "etat": "granted"},
                // Une décision de la personne n'est PAS une limite : elle se change dans les
                // Réglages, et la carte ne doit pas effacer la capacité pour autant.
                {"capacite": "photos", "etat": "denied"},
                {"capacite": "calendar", "etat": "prompt"}
            ]
        });
        let limites = HostLimits::from_bench(&rapport);
        assert_eq!(limites.entries().len(), 2, "{:?}", limites.entries());
        let carte = surface().with_limits(&limites);
        for id in ["device.health", "device.nfc"] {
            assert_eq!(
                carte.capability(id).unwrap().status,
                Status::Missing,
                "{id}"
            );
        }
        for id in ["device.contacts", "device.photos", "device.calendar"] {
            assert_eq!(carte.capability(id).unwrap().status, Status::Live, "{id}");
        }
    }

    /// Un rapport absent, vide ou d'une autre forme ne déclare rien : une mesure qu'on ne sait
    /// pas lire ne doit pas se traduire par « tout est indisponible ».
    /// Les deux sources de limites se réunissent sans se perdre : celle de l'hôte reste en
    /// tête avec sa raison, celle de la mesure complète.
    #[test]
    fn les_limites_de_l_hote_et_celles_de_l_appareil_s_additionnent() {
        let hote = HostLimits::new()
            .unavailable(
                "device.health",
                "profil de signature sans l'entitlement HealthKit",
            )
            .unavailable(
                "device.nfc",
                "profil de signature sans l'entitlement NFC Tag Reading",
            );
        let rapport = serde_json::json!({
            "capacites": [
                // Déjà déclarée par l'hôte : la raison de l'hôte gagne.
                {"capacite": "health", "etat": "unsupported"},
                // Nouvelle : c'est l'appareil qui la révèle.
                {"capacite": "motion_activity", "etat": "unsupported"}
            ]
        });
        let fusion = hote.and(&HostLimits::from_bench(&rapport));
        let ids: Vec<&str> = fusion.entries().iter().map(|(id, _)| id.as_str()).collect();
        assert_eq!(
            ids,
            vec!["device.health", "device.nfc", "device.motion_activity"]
        );
        assert_eq!(
            fusion.entries()[0].1,
            "profil de signature sans l'entitlement HealthKit",
            "la raison de l'hôte doit rester, pas celle du banc"
        );
        let carte = surface().with_limits(&fusion);
        for id in ["device.health", "device.nfc", "device.motion_activity"] {
            assert_eq!(
                carte.capability(id).unwrap().status,
                Status::Missing,
                "{id}"
            );
        }
    }

    /// Fusionner avec une déclaration vide ne change rien, et deux fois la même limite ne se
    /// duplique pas.
    #[test]
    fn une_fusion_ne_duplique_jamais_une_limite() {
        let seule = HostLimits::new().unavailable("device.nfc", "pas d'entitlement");
        assert_eq!(seule.clone().and(&HostLimits::new()).entries().len(), 1);
        let deux_fois = seule.clone().and(&seule);
        assert_eq!(deux_fois.entries().len(), 1);
        assert_eq!(deux_fois.entries()[0].1, "pas d'entitlement");
    }

    #[test]
    fn un_rapport_illisible_ne_declare_aucune_limite() {
        for rapport in [
            serde_json::json!({}),
            serde_json::json!({"capacites": []}),
            serde_json::json!({"capacites": "oui"}),
        ] {
            assert!(HostLimits::from_bench(&rapport).is_empty(), "{rapport}");
        }
    }

    /// Deux fois la même capacité dans le rapport (deux passages, deux versions du banc) ne
    /// produit qu'une entrée : la carte n'a pas à répéter une raison.
    #[test]
    fn une_capacite_absente_deux_fois_ne_declare_qu_une_limite() {
        let rapport = serde_json::json!({
            "capacites": [
                {"capacite": "nfc", "etat": "unsupported"},
                {"capacite": "nfc", "etat": "unsupported"}
            ]
        });
        assert_eq!(HostLimits::from_bench(&rapport).entries().len(), 1);
    }

    #[test]
    fn un_hote_peut_declarer_ce_que_son_build_ne_peut_pas() {
        // Le pont sait lire la santé ; le BUILD peut ne pas avoir l'entitlement HealthKit. La
        // carte doit dire lequel des deux ment — et le dire à l'agent, pas seulement au
        // développeur : « indisponible sur ce build : <raison> ».
        let limites = HostLimits::new()
            .unavailable(
                "device.health",
                "profil de signature sans l'entitlement HealthKit",
            )
            .unavailable("device.nfc", "profil de signature sans l'entitlement NFC")
            // Un identifiant qui n'existe pas ne doit rien casser.
            .unavailable("device.teleport", "n'existe pas");
        assert!(!limites.is_empty());
        assert_eq!(limites.entries().len(), 3);

        let bridee = surface().with_limits(&limites);
        let sante = bridee.capability("device.health").unwrap();
        assert_eq!(sante.status, Status::Missing);
        assert!(
            sante.title.contains("indisponible sur ce build"),
            "{}",
            sante.title
        );
        assert!(sante.title.contains("HealthKit"), "{}", sante.title);

        // Ce qui n'est pas déclaré ne bouge pas.
        let bluetooth = bridee.capability("device.bluetooth").unwrap();
        assert_eq!(bluetooth.status, Status::Live);
        assert!(!bluetooth.title.contains("indisponible"));

        // Et la carte d'origine n'est pas modifiée : `with_limits` rend une copie.
        assert_eq!(
            surface().capability("device.health").unwrap().status,
            Status::Live
        );

        // Le rendu d'un appareil porte la raison.
        let texte = bridee.describe_for(Platform::Ios, usize::MAX);
        assert!(texte.contains("indisponible sur ce build"), "{texte}");
    }

    #[test]
    fn un_hote_ajoute_ses_propres_entrees_sans_toucher_au_framework() {
        // Une coquille sait des choses que le cadre ignore (son bac, sa fenêtre). Elle les
        // déclare, et le framework n'a pas à les connaître.
        let hote = Surface {
            capabilities: vec![
                Capability::new(
                    "pastebin",
                    Tier::Shell,
                    "le presse-papiers de cet hôte",
                    Some("paste_read / paste_write"),
                    &[Platform::Macos],
                    Status::Live,
                ),
                // Un identifiant déjà connu : l'hôte REMPLACE l'entrée du framework.
                Capability::new(
                    "ui.render",
                    Tier::Webview,
                    "le rendu de cet hôte (le sien, pas celui du framework)",
                    Some("screen_show"),
                    &[Platform::Web],
                    Status::Live,
                ),
            ],
            slots: Vec::new(),
        };
        let fusion = surface().with_host_entries(hote);
        assert!(fusion.capability("pastebin").is_some());
        let rendu = fusion.capability("ui.render").unwrap();
        assert_eq!(rendu.command.as_deref(), Some("screen_show"));
        assert_ne!(
            surface()
                .capability("ui.render")
                .unwrap()
                .command
                .as_deref(),
            Some("screen_show")
        );
        // Le reste de la carte est intact.
        assert_eq!(
            fusion.capability("device.bluetooth").unwrap().status,
            Status::Live
        );
        // Et la carte du framework n'a pas bougé.
        assert!(surface().capability("pastebin").is_none());
    }

    #[test]
    fn sans_limite_la_carte_est_celle_du_pont() {
        let aucune = HostLimits::new();
        assert!(aucune.is_empty());
        assert_eq!(
            surface().with_limits(&aucune).capabilities,
            surface().capabilities
        );
    }

    /// Les six lectures d'état système existent, au bon étage et sur la bonne plateforme.
    /// Cinq sont `Live` (le pont les rend) ; `volume` reste `Missing` parce qu'iOS ne lit pas
    /// le volume système sans session audio active — la raison est dans le titre, pas tuée.
    #[test]
    fn les_six_lectures_d_etat_systeme_sont_dans_la_carte() {
        let carte = surface();
        for id in [
            "device_info",
            "battery",
            "network_state",
            "locale",
            "screen_state",
        ] {
            let c = carte
                .capability(id)
                .unwrap_or_else(|| panic!("{id} absent de la carte"));
            assert_eq!(c.tier, Tier::Shell, "{id}");
            assert_eq!(c.status, Status::Live, "{id}");
            assert!(c.command.is_some(), "{id} doit nommer sa commande");
            assert!(c.platforms.contains(&Platform::Ios), "{id}");
        }
        let volume = carte.capability("volume").unwrap();
        assert_eq!(volume.tier, Tier::Shell);
        assert_eq!(volume.status, Status::Missing);
        assert!(
            volume.title.contains("indisponible sur iOS"),
            "{}",
            volume.title
        );
        // Chacune nomme `system_state` et son sélecteur natif.
        for (id, methode) in [
            ("device_info", "deviceInfo"),
            ("battery", "batteryLevel"),
            ("network_state", "networkState"),
            ("locale", "locale"),
            ("screen_state", "screenState"),
            ("volume", "volume"),
        ] {
            let commande = carte.capability(id).unwrap().command.clone().unwrap();
            assert!(
                commande.contains("system_state") && commande.contains(methode),
                "{id} : commande « {commande} » ne nomme ni system_state ni {methode}"
            );
        }
        // La carte iOS reste dans le budget annoncé malgré les six entrées de plus.
        let texte = carte.describe_for(Platform::Ios, usize::MAX);
        assert!(
            texte.chars().count() <= BUDGET_CARTE,
            "ios : {} caractères, budget {BUDGET_CARTE}",
            texte.chars().count()
        );
    }

    #[test]
    fn la_carte_d_un_appareil_tient_dans_le_budget_annonce() {
        // Une carte plus longue que le budget est tronquée par `describe`, et l'agent perd
        // des capacités sans le savoir dans le feu de l'action. Le budget est donc mesuré ici.
        for plateforme in [Platform::Ios, Platform::Macos, Platform::Web] {
            let texte = surface().describe_for(plateforme, usize::MAX);
            assert!(
                texte.chars().count() <= BUDGET_CARTE,
                "{:?} : {} caractères, budget {BUDGET_CARTE}",
                plateforme,
                texte.chars().count()
            );
        }
    }

    #[test]
    fn la_carte_dit_l_appareil_et_ses_limites() {
        // Mesuré le 08/11/2026 : la carte ne nommait AUCUNE capacité d'appareil. L'agent
        // répondait « pas encore » pour les contacts ou le Bluetooth, qui existaient.
        let carte = surface();
        for id in [
            "device.contacts",
            "device.calendar",
            "device.health",
            "device.bluetooth",
            "device.nfc",
            "device.location",
            "device.speech",
            "device.face_id",
            "device.photos_pick",
            "device.files_pick",
            "device.clipboard",
            "device.share",
            "device.notifications",
            "device.live_activity",
            "device.motion_activity",
            "device.gyroscope",
            "device.local_network",
            "device.camera",
            "device.microphone",
            "device.photos",
            "device.location_always",
        ] {
            let c = carte
                .capability(id)
                .unwrap_or_else(|| panic!("{id} absent de la carte"));
            assert_eq!(c.tier, Tier::Shell, "{id}");
            assert_eq!(c.status, Status::Live, "{id}");
            assert!(c.command.is_some(), "{id} doit nommer ses méthodes");
            assert!(c.platforms.contains(&Platform::Ios), "{id}");
        }
        // Une capacité partielle le dit : `device.photos` ne porte que la permission, la
        // lecture est sous `device.photos.read`. Et la santé dit qu'elle est en LECTURE seule.
        let photos = carte.capability("device.photos").unwrap();
        assert!(
            photos.title.contains("device.photos.read"),
            "{}",
            photos.title
        );
        let health = carte.capability("device.health").unwrap();
        assert!(health.title.contains("LECTURE seule"), "{}", health.title);
        // Et la capture, qui vient d'arriver, est annoncée avec ses méthodes.
        let capture = carte.capability("device.camera.capture").unwrap();
        assert_eq!(capture.status, Status::Live);
        assert!(
            capture
                .command
                .as_deref()
                .unwrap()
                .contains("cameraCapturePhoto")
        );
        // Ce qui est plus profond qu'avant est dit disponible, avec ses méthodes.
        let bt = carte.capability("device.bluetooth.connect").unwrap();
        assert_eq!(bt.status, Status::Live);
        let commande = bt.command.clone().unwrap();
        for methode in ["bluetoothConnect", "bluetoothServices", "bluetoothWrite"] {
            assert!(commande.contains(methode), "{methode} absent de {commande}");
        }
        // Et ce qui n'existe pas du tout reste dit absent, à côté de ce qui existe.
        assert_eq!(
            carte.capability("device.files_pick").unwrap().status,
            Status::Live
        );
        // La vidéo est arrivée : elle n'est plus annoncée manquante.
        assert_eq!(
            carte.capability("device.camera.video").unwrap().status,
            Status::Live
        );
    }

    #[test]
    fn sur_un_systeme_sans_pont_la_carte_dit_pourquoi() {
        // Un vide n'est pas une réponse : l'agent doit pouvoir distinguer « pas de pont ici »
        // de « je ne sais pas ». Mesuré le 08/11/2026 : la carte d'un Android ne nommait
        // aucune capacité d'appareil, sans un mot pour dire que c'était voulu.
        // Linux n'y est plus depuis le 13/11/2026 : son backend porte le scan Bluetooth, l'état
        // de la caméra et du micro, le gyroscope et la position ponctuelle. Il a son propre
        // contrôle, juste après.
        for plateforme in [Platform::Android, Platform::Windows, Platform::Web] {
            let texte = surface().describe_for(plateforme, usize::MAX);
            assert!(
                texte.contains("device\\n") || texte.contains("· device "),
                "{:?} ne dit rien de l'appareil : {texte}",
                plateforme
            );
            // Et elle ne promet AUCUNE méthode d'appareil sur ces systèmes.
            for interdit in [
                "device.contacts",
                "device.bluetooth",
                "device.camera.capture",
            ] {
                assert!(
                    !texte.contains(interdit),
                    "{:?} annonce {interdit}",
                    plateforme
                );
            }
        }
        // Linux a un pont PARTIEL, et c'est ce qu'il doit dire : le scan mais pas la connexion,
        // l'état de la caméra mais pas la capture, la position mais ni l'état ni le flux.
        //
        // Le contrôle porte sur les ENTRÉES, pas sur le texte rendu : la description de
        // `device.camera` dit « la capture est sous device.camera.capture », donc chercher ce
        // nom dans la prose trouve toujours quelque chose — et ne prouve rien.
        let linux = surface().for_platform(Platform::Linux);
        for attendu in [
            "device.bluetooth",
            "device.camera",
            "device.microphone",
            "device.gyroscope",
            "device.location.fix",
        ] {
            assert_eq!(
                linux.capability(attendu).map(|entry| entry.status),
                Some(Status::Live),
                "Linux ne nomme pas {attendu}, que son backend porte"
            );
        }
        for interdit in [
            "device.bluetooth.connect",
            "device.camera.capture",
            "device.camera.video",
            "device.location.stream",
            "device.location",
            "device.contacts",
            "device.calendar",
        ] {
            assert_ne!(
                linux.capability(interdit).map(|entry| entry.status),
                Some(Status::Live),
                "Linux annonce {interdit}, que son backend ne porte pas"
            );
        }

        // iOS, lui, les annonce toutes.
        let ios = surface().describe_for(Platform::Ios, usize::MAX);
        assert!(ios.contains("device.bluetooth"));
        assert!(ios.contains("device.camera.capture"));
    }

    #[test]
    fn l_appareil_est_filtre_par_plateforme() {
        let carte = surface();
        let iphone = carte.describe_for(Platform::Ios, 100_000);
        let mac = carte.describe_for(Platform::Macos, 100_000);
        // Ce que le pont macOS ne traite pas (`macos/Sources/dispatch.swift`).
        for absent_sur_mac in [
            "device.health",
            "device.nfc",
            "device.motion_activity",
            "device.gyroscope",
            "device.live_activity",
        ] {
            assert!(
                iphone.contains(absent_sur_mac),
                "{absent_sur_mac} sur iPhone"
            );
            assert!(!mac.contains(absent_sur_mac), "{absent_sur_mac} sur Mac");
        }
        // Et ce qu'il traite y est.
        for present_sur_mac in ["device.contacts", "device.bluetooth", "device.clipboard"] {
            assert!(mac.contains(present_sur_mac), "{present_sur_mac} sur Mac");
        }
    }

    #[test]
    fn une_plateforme_ne_voit_que_ses_capacites() {
        let carte = surface();
        let iphone = carte.describe_for(Platform::Ios, 100_000);
        let mac = carte.describe_for(Platform::Macos, 100_000);
        // window.control n'est que macOS/Windows/Linux : un iPhone ne doit pas l'entendre.
        assert!(!iphone.contains("window.control"));
        assert!(mac.contains("window.control"));
        // widget est natif, iOS compris : il apparaît sur l'iPhone.
        assert!(iphone.contains("widget"));
        // la carte dit sur quelle plateforme elle parle.
        assert!(iphone.starts_with("Plateforme : ios"));
    }

    #[test]
    fn la_carte_dit_que_l_agent_a_un_graphe_une_memoire_et_des_lignes() {
        let texte = surface().describe(100_000);
        for id in ["graph", "memory", "rows"] {
            assert!(
                texte.contains(&format!("· {id} —")),
                "{id} absent de la carte"
            );
            let c = surface().capability(id).cloned().unwrap();
            assert_eq!(c.tier, Tier::Shell);
            assert_eq!(c.status, Status::Live);
            assert!(c.platforms.contains(&Platform::Ios));
            assert!(c.platforms.contains(&Platform::Macos));
        }
        assert!(
            texte.contains("graph_put")
                && texte.contains("graph_link")
                && texte.contains("graph_query")
        );
        assert!(texte.contains("remember") && texte.contains("recall") && texte.contains("forget"));
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
        // Le compte est exact, pas « au moins » : une capacité ajoutée ou retirée sans le
        // dire se voit ici, avant que la carte ne mente à l'agent.
        // 55 depuis que Linux nomme ce qu'il porte (scan Bluetooth, état caméra/micro,
        // gyroscope, position ponctuelle) : quatre entrées gagnent `Linux`, une naît
        // (`device.location.fix`). Une entrée ajoutée sans le dire se voit ici.
        assert_eq!(livres, 55, "nombre de capacités Live");
        assert_eq!(attendus, 32, "nombre de capacités Missing");
        // Aucun identifiant en double : un module ne doit jamais hésiter entre deux entrées.
        let mut vus = std::collections::BTreeSet::new();
        for capacite in &carte.capabilities {
            assert!(
                vus.insert(capacite.id.clone()),
                "identifiant en double : {}",
                capacite.id
            );
        }
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
