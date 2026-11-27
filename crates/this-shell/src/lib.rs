//! # Le contrat d'exécution d'un this-rs app — ce qu'une commande EST, avant qui l'exécute
//!
//! Un shell est le premier outil dont la surface n'est pas finie. Ce crate ne l'ouvre pas :
//! il le **borne**, et il dit l'ordre dans lequel les capacités d'un agent se composent.
//!
//! Ce crate est **pur** : aucun processus, aucun système de fichiers, aucun réseau. Il
//! compile pour `wasm32-unknown-unknown`, ce qui est exactement ce qui permet au même
//! contrat de servir un noyau natif, une page, et un pair distant.
//!
//! ## L'ordre — cinq étages, du plus stable au plus engageant
//!
//! 1. **Droit** — une porte (`desktop_consent`), pas trois : capacité nommée, portée,
//!    TTL, révocation qui interrompt, et un selftest qui dit l'état sans forcer.
//! 2. **Lieu** — le VFS (`etag` + `Precondition`) est la seule adresse autoritative.
//!    L'arbre du système est un **second montage, déclaré** ; jamais les deux dans un même
//!    appel, jamais l'un présenté pour l'autre.
//! 3. **Geste** — quatre : voir · changer un texte (ancre exacte + `etag`) · créer,
//!    déplacer, supprimer · exécuter. Un geste qui n'entre dans aucune de ces quatre cases
//!    n'est pas un geste de plus, c'est une erreur de conception.
//! 4. **Moteur** — un seul port ([`Executeur`]), trois implémentations qui se DÉCLARENT
//!    ([`Moteur`]) : `Natif` (un processus réel), `Wasm` (un jeu de commandes sous `wasmi`
//!    dont les fichiers sont le VFS — le « shell sans shell » des architectures qui n'en
//!    ont pas), `Distant` (un pair, sous SON consentement). L'appelant n'a pas le droit de
//!    supposer lequel il a parlé.
//! 5. **Chaîne d'approvisionnement** — la forge construit, signe, installe. Elle n'exécute
//!    pas, et le shell n'installe pas : ce sont deux métiers, et les confondre ferait de
//!    l'ancre de confiance une décoration.
//!
//! ## La matrice — les « non » sont l'architecture
//!
//! | de ↓ / vers → | VFS | hôte (OS) | module wasm | forge | appareil | shell |
//! |---|---|---|---|---|---|---|
//! | outil `fs_*` | oui (`IfMatch`) | non | — | — | — | — |
//! | shell natif | oui | oui (classé + delta) | **non** | **non** | non | — |
//! | shell wasm | oui | non | **non** | **non** | non | — |
//! | module wasm | via son ABI bornée | non | — | non | oui, consentie | **non** (sauf `ShellExec`) |
//! | forge | module signé (chemin unique) | non | non | — | non | non |
//! | capacité appareil | blobs/fichiers | fichier déposé | non | non | — | non |
//!
//! Trois bornes portent cette matrice, et chacune est un test, pas un commentaire :
//!
//! - **R1** — le shell n'écrit jamais dans une zone signée (`/modules/*/*/program.wasm`,
//!   manifeste). Le magasin de modules re-signe ce qu'il range avec la clé de l'appareil :
//!   un shell qui écrirait là fabriquerait un module signé par l'appareil à partir d'octets
//!   quelconques.
//! - **R2** — la forge ne décide pas de l'exécution : construire → vérifier l'ancre →
//!   charger, sans raccourci.
//! - **R3** — un module n'obtient pas de geste système sans capacité nommée, et pas de
//!   shell sans `ShellExec` consentie (portée = noms de commandes).
//!
//! ## Ce que ce crate n'est PAS
//!
//! - Pas un terminal : un agent qui code n'a pas besoin d'un PTY, et un PTY ouvre la
//!   question du rendu et de l'attente. L'exécution est en tuyaux, sans interaction.
//! - Pas un interpréteur : [`classe::classer`] refuse tout interpréteur, donc `sh -c` n'est
//!   pas une porte dérobée. Une ligne interprétée légitime est un OUTIL à part, avec sa
//!   propre classe.
//! - Pas un compilateur (c'est la forge), pas une porte vers `sudo` (`Privilege` n'est
//!   accordable à personne).
//!
//! ## Registre
//!
//! Les affirmations ci-dessus sont **relues** dans le code de l'Atelier et du harnais le
//! 27/11/2026 (`fs_agent.rs`, `cloison.rs`, `services_appareil.rs`, `desktop_consent.rs`,
//! `ports.rs`) ; la matrice est **démontrée** cellule par cellule dans `atelier/tests/` ;
//! et « wasmi compile pour wasm32 » reste **non qualifié** tant que ce n'est pas mesuré.

use async_trait::async_trait;

pub mod classe;
pub mod conformance;

pub use classe::{
    Borne, Classe, Commande, Moteur, Refus, Sortie, classe_de_chemin, classe_effective, classer,
};

/// Le port d'exécution. L'hôte en fournit une implémentation par moteur disponible.
///
/// `&self` : un moteur n'a pas d'état de tour à lui. `?Send` : le harnais vit sur un seul
/// fil et une page n'a pas de `Send` — imposer `Send` fermerait la porte au seul moteur qui
/// tourne partout.
///
/// Contrat que TOUTE implémentation doit tenir, et que [`conformance::verifier`] vérifie :
///
/// 1. la [`Borne`] est respectée, et une troncature est DITE (`tronque`) ;
/// 2. `moteur()` est celui qu'on déclare dans la [`Sortie`], pas un autre ;
/// 3. la classe effective est rendue dans la [`Sortie`] (maximum du programme et des
///    chemins), jamais la classe du seul programme ;
/// 4. un dépassement de temps rend une `Sortie` avec un statut négatif et `ms` mesuré —
///    jamais une erreur qui efface ce qui a été produit ;
/// 5. un refus est un [`Refus`] nommé, jamais une `Sortie` vide qui se lit comme un succès.
#[async_trait(?Send)]
pub trait Executeur {
    /// Le moteur que cette implémentation sert. Déclaré, jamais déduit par l'appelant.
    fn moteur(&self) -> Moteur;

    /// Exécute, borné. Rend ce qui a été produit, ou un refus nommé.
    async fn executer(&self, commande: &Commande, borne: &Borne) -> Result<Sortie, Refus>;
}
