//! # Les flux : ce que l'appareil émet, et ce qu'un module peut écouter
//!
//! Une capacité ([`crate::surface::Capability`]) dit « je peux faire X », et se
//! demande une fois. Un flux dit « X m'arrive, en continu » : la batterie qui
//! descend, le réseau qui tombe, le clavier qui s'ouvre. Les mêler ferait d'une
//! carte statique un bus d'événements : un flux a un état (ses abonnements, leurs
//! files, leurs seuils déjà franchis), une capacité n'en a pas.
//!
//! Ce module est **pur** : aucune horloge (le temps arrive dans [`Event::at_ms`]),
//! aucune file réelle, aucun thread. Il décrit le contrat et tient l'état des
//! abonnements. La coquille fournit les événements, et réveille un module quand
//! [`Diffuseur::pousser`] lui signale qu'il a quelque chose à lire.
//!
//! ## Pourquoi un fichier à part
//!
//! `surface.rs` est une carte sans état, relue pour décrire le harnais à l'agent.
//! Ici il y a des abonnements vivants, des files et des compteurs : un autre cycle
//! de vie. Les mettre dans `surface` obligerait `describe` à porter de l'état.
//!
//! ## Franchissement, pas niveau
//!
//! Un seuil ne se déclenche pas à chaque événement qui le dépasse : il se
//! déclenche quand la valeur **passe** du mauvais côté. Tant qu'elle reste du même
//! côté, rien. Elle ne se réarme qu'en revenant de l'autre côté. C'est ce qui rend
//! un flux utilisable par un agent : « batterie = 19 % » reçu 500 fois devient
//! « batterie a franchi 20 % vers le bas », une fois.
//!
//! - Premier échantillon : il compte comme un franchissement s'il est déjà du côté
//!   déclenché. Un module abonné à 19 % en reçoit l'information, pas le silence.
//! - Pas d'hystérésis : une valeur qui oscille autour du seuil déclenche à chaque
//!   aller-retour. Un module qui veut l'éviter choisit un seuil plus large.
//! - [`Filtre::Changed`] ne se déclenche pas au premier échantillon (rien à comparer).
//!
//! ## Rien n'est silencieux
//!
//! Un abonnement a une file bornée. Quand elle est pleine, on évince le plus
//! ancien (on garde l'état le plus récent) et on compte. Un anti-rebond
//! ([`Souscription::espacement_ms`]) compte de la même façon les événements voulus
//! qu'il retient. Un événement non voulu par le filtre n'est pas une perte : il
//! n'était pas demandé. Le compte remonte au module dans
//! [`Livraison::Perdus`], à la lecture suivante, avant les événements restants.
//!
//! ## Cycle de vie
//!
//! Un abonnement appartient à un [`Proprietaire`] (un module, et optionnellement
//! le run qui l'a lancé). Il meurt avec lui : [`Diffuseur::liberer_module`] à la
//! désinstallation, [`Diffuseur::terminer_run`] à la fin du run. Un abonnement
//! orphelin réveillerait un module déchargé : c'est pour l'exclure que le
//! propriétaire est obligatoire et qu'il n'existe pas d'abonnement anonyme.
//!
//! ## Donnée non fiable
//!
//! Un événement vient du système, ou du contenu d'une autre application (le
//! presse-papiers, le nom de l'application au premier plan). Ce n'est pas une
//! instruction. Les valeurs texte sortent de ce module brutes : l'hôte les passe
//! par [`crate::untrusted::wrap`] avant qu'elles n'atteignent un prompt. Ce module
//! ne les interprète jamais, il ne fait que les comparer.

use std::collections::{BTreeMap, VecDeque};

use serde::{Deserialize, Serialize};

use crate::surface::{Platform, Status};

/// Capacité de file par défaut d'un abonnement.
pub const CAPACITE_PAR_DEFAUT: usize = 64;

/// Le genre d'une valeur dans une charge.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Genre {
    Nombre,
    Texte,
    Booleen,
}

/// Une valeur d'événement. Un nombre est toujours fini : un `NaN` est refusé à l'entrée.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Valeur {
    Nombre(f64),
    Texte(String),
    Booleen(bool),
}

impl Valeur {
    fn genre(&self) -> Genre {
        match self {
            Valeur::Nombre(_) => Genre::Nombre,
            Valeur::Texte(_) => Genre::Texte,
            Valeur::Booleen(_) => Genre::Booleen,
        }
    }
}

/// Un champ de charge : son nom, son genre, et éventuellement ses valeurs fermées.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Champ {
    pub nom: String,
    pub genre: Genre,
    /// Pour un texte à valeurs fermées (état réseau, angle) : hors de cette liste,
    /// l'événement est refusé. Un `None` laisse le texte libre (zone, nom d'application).
    pub choix: Option<Vec<String>>,
}

impl Champ {
    pub fn nombre(nom: &str) -> Self {
        Self {
            nom: nom.into(),
            genre: Genre::Nombre,
            choix: None,
        }
    }

    pub fn booleen(nom: &str) -> Self {
        Self {
            nom: nom.into(),
            genre: Genre::Booleen,
            choix: None,
        }
    }

    pub fn texte(nom: &str) -> Self {
        Self {
            nom: nom.into(),
            genre: Genre::Texte,
            choix: None,
        }
    }

    pub fn choix(nom: &str, valeurs: &[&str]) -> Self {
        Self {
            nom: nom.into(),
            genre: Genre::Texte,
            choix: Some(valeurs.iter().map(|v| v.to_string()).collect()),
        }
    }
}

/// La forme d'un événement : son nom et ses champs. Un événement doit porter
/// exactement ces champs, ni plus ni moins.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Charge {
    pub nom: String,
    pub champs: Vec<Champ>,
}

impl Charge {
    pub fn new(nom: &str, champs: Vec<Champ>) -> Self {
        Self {
            nom: nom.into(),
            champs,
        }
    }

    pub fn champ(&self, nom: &str) -> Option<&Champ> {
        self.champs.iter().find(|c| c.nom == nom)
    }
}

/// Ce que la source promet sur sa cadence. C'est un engagement de l'hôte : un
/// flux qui annonce `Throttled` et émet plus vite ment, et ça se voit aux pertes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "mode")]
pub enum Cadence {
    /// Émis seulement quand la valeur change.
    OnChange,
    /// Jamais deux émissions à moins de `min_ms` d'écart.
    Throttled { min_ms: u64 },
    /// Une émission par période, avec la valeur la plus récente.
    Sampled { period_ms: u64 },
}

/// Un flux du catalogue : son identifiant canonique, sa charge, sa cadence, et où
/// il existerait. Comme les capacités, `Status::Live` veut dire « ça tourne ».
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Stream {
    /// Identifiant stable que les modules demandent, ex. `battery.level`.
    pub id: String,
    pub charge: Charge,
    pub cadence: Cadence,
    /// Les plateformes visées. Une cible n'est pas une garantie : `Status` dit le reste.
    pub platforms: Vec<Platform>,
    pub status: Status,
}

impl Stream {
    pub fn new(
        id: &str,
        charge: Charge,
        cadence: Cadence,
        platforms: &[Platform],
        status: Status,
    ) -> Self {
        Self {
            id: id.into(),
            charge,
            cadence,
            platforms: platforms.to_vec(),
            status,
        }
    }
}

/// Un seuil, une égalité ou un changement, calculé côté hôte sur un champ.
///
/// Les variantes `Above`/`Below`/`Equals`/`Exit` sont **des franchissements** (voir
/// la doc du module). `Equals` sur un champ de zone est l'entrée dans la zone.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "filtre")]
pub enum Filtre {
    /// Franchit vers le haut : le champ numérique passe strictement au-dessus du seuil.
    Above { champ: String, seuil: f64 },
    /// Franchit vers le bas : le champ numérique passe strictement sous le seuil.
    Below { champ: String, seuil: f64 },
    /// Devient égal à la valeur (entrée de zone, bascule d'état).
    Equals { champ: String, valeur: Valeur },
    /// Cesse d'être égal à la valeur (sortie de zone).
    Exit { champ: String, valeur: Valeur },
    /// Diffère de l'échantillon précédent.
    Changed { champ: String },
}

impl Filtre {
    fn champ(&self) -> &str {
        match self {
            Filtre::Above { champ, .. }
            | Filtre::Below { champ, .. }
            | Filtre::Equals { champ, .. }
            | Filtre::Exit { champ, .. }
            | Filtre::Changed { champ } => champ,
        }
    }
}

/// Ce qu'un module demande en s'abonnant.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Souscription {
    /// `None` : le module reçoit tout (au rythme de l'anti-rebond).
    pub filtre: Option<Filtre>,
    /// Écart minimal entre deux livraisons. Les événements voulus qui tombent
    /// dedans sont comptés comme perdus.
    pub espacement_ms: Option<u64>,
    /// Taille de la file. `None` : [`CAPACITE_PAR_DEFAUT`].
    pub capacite: Option<usize>,
}

/// Qui possède un abonnement : un module, et le run qui l'a créé s'il y en a un.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Proprietaire {
    pub module: String,
    pub run: Option<String>,
}

/// Identifiant d'un abonnement, valable tant qu'il vit.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct AbonnementId(pub u64);

/// Un événement entrant. `at_ms` est l'instant de la source, fourni par l'hôte.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Event {
    pub flux: String,
    pub at_ms: u64,
    pub champs: BTreeMap<String, Valeur>,
}

/// Ce qu'un module lit à son réveil.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Livraison {
    /// Un événement voulu, dans l'ordre d'arrivée.
    Evenement(Event),
    /// Des événements voulus n'ont pas été livrés depuis la lecture précédente.
    /// `topic` est l'identifiant du flux ; `count` ne compte que ce qui a été perdu.
    Perdus { topic: String, count: u64 },
}

/// Pourquoi une demande est refusée. Toujours un motif, jamais un silence.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Refus {
    FluxInconnu(String),
    ChampInconnu { flux: String, champ: String },
    ChampManquant { flux: String, champ: String },
    TypeIncompatible { champ: String, attendu: Genre },
    ValeurNonFinie { champ: String },
    HorsChoix { champ: String, valeur: String },
    CapaciteNulle,
}

impl std::fmt::Display for Refus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Refus::FluxInconnu(id) => write!(f, "flux inconnu : {id}"),
            Refus::ChampInconnu { flux, champ } => {
                write!(f, "le flux {flux} n'a pas de champ « {champ} »")
            }
            Refus::ChampManquant { flux, champ } => {
                write!(f, "événement {flux} sans le champ « {champ} »")
            }
            Refus::TypeIncompatible { champ, attendu } => {
                write!(f, "le champ « {champ} » attend {attendu:?}")
            }
            Refus::ValeurNonFinie { champ } => write!(f, "« {champ} » n'est pas un nombre fini"),
            Refus::HorsChoix { champ, valeur } => {
                write!(f, "« {valeur} » n'est pas une valeur de « {champ} »")
            }
            Refus::CapaciteNulle => write!(f, "une file de capacité 0 ne livre jamais rien"),
        }
    }
}

/// Le catalogue des flux, **tel qu'il est aujourd'hui** : tous `Missing`.
///
/// Aucun flux n'existe encore sur l'appareil. Le jour où un événement sort
/// réellement de la coquille, c'est ici que le flux passe à `Live`, et nulle part
/// ailleurs. Un flux déclaré `Live` qui n'émet rien est pire qu'un flux absent.
pub fn catalogue() -> Vec<Stream> {
    use Platform::*;
    vec![
        Stream::new(
            "battery.level",
            Charge::new(
                "batterie",
                vec![Champ::nombre("niveau"), Champ::booleen("en_charge")],
            ),
            // iOS notifie un changement de niveau (`batteryLevelDidChangeNotification`) : la
            // source émet au changement, elle n'échantillonne pas.
            Cadence::OnChange,
            &[Ios, Ipados, Android, Macos, Windows, Linux],
            Status::Missing,
        ),
        Stream::new(
            "network.state",
            Charge::new(
                "reseau",
                vec![Champ::choix("etat", &["en_ligne", "hors_ligne", "limite"])],
            ),
            Cadence::OnChange,
            &[Web, Ios, Ipados, Macos, Windows, Linux, Android],
            Status::Missing,
        ),
        Stream::new(
            "gesture.tap",
            Charge::new("geste", vec![Champ::nombre("x"), Champ::nombre("y")]),
            Cadence::OnChange,
            &[Web, Ios, Ipados, Android],
            Status::Missing,
        ),
        Stream::new(
            "keyboard.focus",
            Charge::new(
                "clavier",
                vec![Champ::booleen("visible"), Champ::nombre("hauteur")],
            ),
            Cadence::OnChange,
            &[Ios, Ipados, Android],
            Status::Missing,
        ),
        Stream::new(
            "orientation",
            Charge::new(
                "orientation",
                vec![Champ::choix(
                    "angle",
                    &[
                        "portrait",
                        "paysage_gauche",
                        "paysage_droit",
                        "portrait_inverse",
                    ],
                )],
            ),
            Cadence::OnChange,
            &[Ios, Ipados, Android],
            Status::Missing,
        ),
        Stream::new(
            "location.position",
            Charge::new(
                "position",
                vec![
                    Champ::nombre("lat"),
                    Champ::nombre("lon"),
                    Champ::texte("zone"),
                ],
            ),
            Cadence::Throttled { min_ms: 5_000 },
            &[Web, Ios, Ipados, Android, Macos, Windows, Linux],
            Status::Missing,
        ),
        Stream::new(
            "clipboard.change",
            Charge::new(
                "presse_papier",
                vec![Champ::choix("nature", &["texte", "image", "autre"])],
            ),
            Cadence::OnChange,
            &[Web, Macos, Windows, Linux],
            Status::Missing,
        ),
        Stream::new(
            "app.foreground",
            Charge::new("premier_plan", vec![Champ::texte("application")]),
            Cadence::OnChange,
            &[Ios, Ipados, Android, Macos, Windows, Linux],
            Status::Missing,
        ),
    ]
}

/// Traduit un échantillon du pont natif (`tauri-plugin-this-device`, `stream_start`) en
/// [`Event`] du catalogue, horodaté `at_ms` par l'hôte.
///
/// - `{"kind":"orientation","angle":…}` → `orientation`, `angle` (valeur fermée : un angle
///   hors des quatre choix est refusé par [`Diffuseur::pousser`], jamais transmis) ;
/// - `{"kind":"battery","level":0…1,"state":…}` → `battery.level`, `niveau` et `en_charge`
///   (`charging` ou `full`). Un niveau négatif (inconnu, ex. simulateur) n'est **pas** un
///   niveau : `None`, plutôt qu'un zéro inventé qui franchirait tous les seuils bas.
///
/// Aucun champ n'est du texte libre (deux nombres, un booléen, un choix fermé) : rien à
/// sceller par [`crate::untrusted::wrap`].
pub fn depuis_appareil(echantillon: &serde_json::Value, at_ms: u64) -> Option<Event> {
    let kind = echantillon.get("kind")?.as_str()?;
    let (flux, champs) = match kind {
        "orientation" => {
            let angle = echantillon.get("angle")?.as_str()?;
            (
                "orientation",
                BTreeMap::from([("angle".to_string(), Valeur::Texte(angle.into()))]),
            )
        }
        "battery" => {
            let niveau = echantillon.get("level")?.as_f64()?;
            if !(0.0..=1.0).contains(&niveau) {
                return None;
            }
            let etat = echantillon.get("state")?.as_str()?;
            (
                "battery.level",
                BTreeMap::from([
                    ("niveau".to_string(), Valeur::Nombre(niveau)),
                    (
                        "en_charge".to_string(),
                        Valeur::Booleen(matches!(etat, "charging" | "full")),
                    ),
                ]),
            )
        }
        _ => return None,
    };
    Some(Event {
        flux: flux.into(),
        at_ms,
        champs,
    })
}

/// État d'un filtre entre deux événements.
#[derive(Clone, Debug, Default, PartialEq)]
struct Etat {
    /// Côté du seuil ou de la valeur au dernier échantillon utile.
    cote: Option<bool>,
    /// Dernière valeur vue, pour [`Filtre::Changed`].
    precedent: Option<Valeur>,
}

/// Franchissement vers le côté déclenché : vrai une seule fois par entrée de côté.
fn franchit_vers(cote: bool, etat: &mut Etat) -> bool {
    let declenche = cote && etat.cote != Some(true);
    etat.cote = Some(cote);
    declenche
}

/// Vrai si l'événement passe le filtre, en mettant l'état à jour.
fn passe(filtre: &Filtre, etat: &mut Etat, champs: &BTreeMap<String, Valeur>) -> bool {
    let Some(valeur) = champs.get(filtre.champ()) else {
        // Champ absent : l'événement ne dit rien de ce champ, l'état ne bouge pas.
        return false;
    };
    match filtre {
        Filtre::Above { seuil, .. } => match valeur {
            Valeur::Nombre(x) => franchit_vers(*x > *seuil, etat),
            _ => false,
        },
        Filtre::Below { seuil, .. } => match valeur {
            Valeur::Nombre(x) => franchit_vers(*x < *seuil, etat),
            _ => false,
        },
        Filtre::Equals { valeur: cible, .. } => franchit_vers(valeur == cible, etat),
        Filtre::Exit { valeur: cible, .. } => {
            let dehors = valeur != cible;
            let sort = dehors && etat.cote == Some(true);
            etat.cote = Some(!dehors);
            sort
        }
        Filtre::Changed { .. } => {
            let change = etat.precedent.as_ref().is_some_and(|p| p != valeur);
            etat.precedent = Some(valeur.clone());
            change
        }
    }
}

/// Un abonnement vivant : son propriétaire, son filtre, sa file, ses compteurs.
#[derive(Debug)]
struct Abonnement {
    id: AbonnementId,
    proprietaire: Proprietaire,
    flux: String,
    filtre: Option<Filtre>,
    espacement_ms: Option<u64>,
    capacite: usize,
    etat: Etat,
    file: VecDeque<Event>,
    perdus: u64,
    dernier_envoi: Option<u64>,
}

/// Le diffuseur : le catalogue, et les abonnements vivants.
///
/// Sans horloge ni thread. L'hôte appelle [`Diffuseur::pousser`] à chaque
/// événement, lit les réveils qu'il renvoie, puis [`Diffuseur::lire`].
#[derive(Debug)]
pub struct Diffuseur {
    catalogue: Vec<Stream>,
    abonnements: Vec<Abonnement>,
    suivant: u64,
}

impl Default for Diffuseur {
    fn default() -> Self {
        Self::new()
    }
}

impl Diffuseur {
    pub fn new() -> Self {
        Self {
            catalogue: catalogue(),
            abonnements: Vec::new(),
            suivant: 0,
        }
    }

    pub fn flux(&self, id: &str) -> Option<&Stream> {
        self.catalogue.iter().find(|s| s.id == id)
    }

    pub fn abonnements_actifs(&self) -> usize {
        self.abonnements.len()
    }

    /// Crée un abonnement. Le filtre est vérifié contre la charge du flux : un champ
    /// inconnu ou de mauvais genre se refuse ici, pas à la première livraison.
    pub fn abonner(
        &mut self,
        proprietaire: Proprietaire,
        flux_id: &str,
        options: Souscription,
    ) -> Result<AbonnementId, Refus> {
        let charge = self
            .flux(flux_id)
            .ok_or_else(|| Refus::FluxInconnu(flux_id.into()))?
            .charge
            .clone();
        let capacite = options.capacite.unwrap_or(CAPACITE_PAR_DEFAUT);
        if capacite == 0 {
            return Err(Refus::CapaciteNulle);
        }
        if let Some(filtre) = &options.filtre {
            verifier_filtre(&charge, flux_id, filtre)?;
        }
        let id = AbonnementId(self.suivant);
        self.suivant += 1;
        self.abonnements.push(Abonnement {
            id,
            proprietaire,
            flux: flux_id.into(),
            filtre: options.filtre,
            espacement_ms: options.espacement_ms,
            capacite,
            etat: Etat::default(),
            file: VecDeque::new(),
            perdus: 0,
            dernier_envoi: None,
        });
        Ok(id)
    }

    /// Entre un événement. Renvoie les abonnements qui ont maintenant à lire :
    /// l'hôte ne réveille que ceux-là.
    pub fn pousser(&mut self, evenement: Event) -> Result<Vec<AbonnementId>, Refus> {
        let charge = self
            .flux(&evenement.flux)
            .ok_or_else(|| Refus::FluxInconnu(evenement.flux.clone()))?
            .charge
            .clone();
        verifier_evenement(&charge, &evenement)?;

        let mut reveils = Vec::new();
        for ab in self
            .abonnements
            .iter_mut()
            .filter(|a| a.flux == evenement.flux)
        {
            let voulu = match &ab.filtre {
                None => true,
                Some(filtre) => passe(filtre, &mut ab.etat, &evenement.champs),
            };
            if !voulu {
                continue;
            }
            if let (Some(espacement), Some(dernier)) = (ab.espacement_ms, ab.dernier_envoi)
                && evenement.at_ms.saturating_sub(dernier) < espacement
            {
                ab.perdus += 1;
                continue;
            }
            if ab.file.len() == ab.capacite {
                // On évince le plus ancien : la file garde l'état le plus récent.
                ab.file.pop_front();
                ab.perdus += 1;
            }
            ab.file.push_back(evenement.clone());
            ab.dernier_envoi = Some(evenement.at_ms);
            reveils.push(ab.id);
        }
        Ok(reveils)
    }

    /// Lit ce qui est en attente. Le compte de pertes, s'il y en a, vient en tête
    /// et se remet à zéro. `None` si l'abonnement n'existe plus (libéré).
    pub fn lire(&mut self, id: AbonnementId) -> Option<Vec<Livraison>> {
        let ab = self.abonnements.iter_mut().find(|a| a.id == id)?;
        let mut sortie = Vec::new();
        if ab.perdus > 0 {
            sortie.push(Livraison::Perdus {
                topic: ab.flux.clone(),
                count: ab.perdus,
            });
            ab.perdus = 0;
        }
        sortie.extend(ab.file.drain(..).map(Livraison::Evenement));
        Some(sortie)
    }

    /// Libère tous les abonnements d'un module (désinstallation, déchargement).
    /// Renvoie le nombre d'abonnements libérés.
    pub fn liberer_module(&mut self, module: &str) -> usize {
        self.liberer(|p| p.module == module)
    }

    /// Libère les abonnements créés par un run terminé.
    pub fn terminer_run(&mut self, run: &str) -> usize {
        self.liberer(|p| p.run.as_deref() == Some(run))
    }

    fn liberer(&mut self, pour: impl Fn(&Proprietaire) -> bool) -> usize {
        let avant = self.abonnements.len();
        self.abonnements.retain(|a| !pour(&a.proprietaire));
        avant - self.abonnements.len()
    }
}

fn verifier_filtre(charge: &Charge, flux: &str, filtre: &Filtre) -> Result<(), Refus> {
    let nom = filtre.champ();
    let champ = charge.champ(nom).ok_or_else(|| Refus::ChampInconnu {
        flux: flux.into(),
        champ: nom.into(),
    })?;
    match filtre {
        Filtre::Above { seuil, .. } | Filtre::Below { seuil, .. } => {
            if champ.genre != Genre::Nombre {
                return Err(Refus::TypeIncompatible {
                    champ: nom.into(),
                    attendu: Genre::Nombre,
                });
            }
            if !seuil.is_finite() {
                return Err(Refus::ValeurNonFinie { champ: nom.into() });
            }
        }
        Filtre::Equals { valeur, .. } | Filtre::Exit { valeur, .. } => {
            verifier_valeur(champ, valeur)?;
        }
        Filtre::Changed { .. } => {}
    }
    Ok(())
}

fn verifier_evenement(charge: &Charge, ev: &Event) -> Result<(), Refus> {
    for (nom, valeur) in &ev.champs {
        let champ = charge.champ(nom).ok_or_else(|| Refus::ChampInconnu {
            flux: ev.flux.clone(),
            champ: nom.clone(),
        })?;
        verifier_valeur(champ, valeur)?;
    }
    for champ in &charge.champs {
        if !ev.champs.contains_key(&champ.nom) {
            return Err(Refus::ChampManquant {
                flux: ev.flux.clone(),
                champ: champ.nom.clone(),
            });
        }
    }
    Ok(())
}

fn verifier_valeur(champ: &Champ, valeur: &Valeur) -> Result<(), Refus> {
    if valeur.genre() != champ.genre {
        return Err(Refus::TypeIncompatible {
            champ: champ.nom.clone(),
            attendu: champ.genre,
        });
    }
    match valeur {
        Valeur::Nombre(x) if !x.is_finite() => Err(Refus::ValeurNonFinie {
            champ: champ.nom.clone(),
        }),
        Valeur::Texte(t) => match &champ.choix {
            Some(choix) if !choix.contains(t) => Err(Refus::HorsChoix {
                champ: champ.nom.clone(),
                valeur: t.clone(),
            }),
            _ => Ok(()),
        },
        _ => Ok(()),
    }
}
