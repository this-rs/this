//! Raccordement de l'autorité K durable au port [`this_harness::routing::Seeds`].
//!
//! Le harnais (`this-harness`) ne tire pas lui-même l'identité de ses décisions :
//! le trait [`this_harness::routing::Seeds`] injecte un `u128` par décision
//! ([`Seeds::next_id`](this_harness::routing::Seeds::next_id)), qui devient à la
//! fois l'`id` d'une [`Decision`](this_harness::routing::Decision) et sa graine.
//! L'implémentation en mémoire ([`SequenceSeeds`](this_harness::routing::SequenceSeeds))
//! sert aux tests et à un rejeu ; elle n'est ni durable, ni atomique entre processus.
//!
//! Ce module raccorde l'**unique** autorité K de `this` ([`crate::autorite_k::AutoriteK`])
//! à ce port : chaque `next_id` alloue un K **u128**, **atomique** (verrou exclusif
//! tenu de la lecture à la publication) et **durable** (persisté AVANT d'être rendu,
//! via [`crate::durabilite::ecrire_durable`]). C'est le même code que le port
//! `feat(k)` (#390), exposé sous le trait que le routeur consomme réellement.
//!
//! ## Ce que ce raccordement n'ajoute pas
//!
//! - **Aucune seconde autorité** : [`SeedsDurables`] ne fait que déléguer à
//!   [`AutoriteK`], l'UNIQUE définition de K. Deux instances sur le MÊME fichier
//!   d'état restent une seule autorité (le verrou du fichier les sérialise).
//! - **Aucun second mécanisme de persistance** : la persistance est celle
//!   d'[`AutoriteK`] (`ecrire_durable`), pas un nouveau magasin. Les observations,
//!   tentatives et obligations du harnais, elles, continuent de passer par leur
//!   mécanisme propre ([`Learning`](this_harness::routing::Learning), Obrain GQL) —
//!   ce module ne s'y substitue pas.
//! - **Aucun contrat Obrain** : [`AutoriteK`] ne dépend que de `std::fs` et
//!   d'[`ecrire_durable`] ; le trait `Seeds` est celui du harnais. Aucun
//!   `ReferenceTentative`/`ResolutionTentative`/`IntegriteTentative` n'est requis.

use std::path::PathBuf;
use std::sync::Mutex;

use crate::autorite_k::AutoriteK;

/// Le [`this_harness::routing::Seeds`] d'un serveur, adossé à l'autorité K durable.
///
/// Le [`Mutex`] ne fait que permettre `next_id(&self)` sur un [`AutoriteK`] dont
/// l'émission exige `&mut self` ; il n'est PAS l'autorité : la sérialisation
/// entre processus est le verrou POSIX tenu par [`AutoriteK`] sur son fichier
/// d'état. Deux [`SeedsDurables`] sur le même chemin sont donc UNE seule autorité.
#[derive(Debug)]
pub struct SeedsDurables {
    autorite: Mutex<AutoriteK>,
}

impl SeedsDurables {
    /// Ouvre (ou crée durablement, sous verrou) l'autorité K au chemin donné.
    ///
    /// Un état présent mais illisible / corrompu est REFUSÉ nommément — jamais de
    /// réinitialisation silencieuse (un K émis doit rester distinct à jamais).
    pub fn ouvrir(chemin: PathBuf) -> std::io::Result<Self> {
        Ok(Self {
            autorite: Mutex::new(AutoriteK::ouvrir(chemin)?),
        })
    }

    /// L'état durable RÉEL du compteur, relu sous verrou — pour l'observation et
    /// les témoins, jamais pour allouer (l'allocation relit l'état sous verrou à
    /// chaque émission, cf. [`AutoriteK::emettre`]).
    pub fn prochain_durable(&self) -> std::io::Result<u128> {
        self.autorite
            .lock()
            .map_err(|_| std::io::Error::other("autorité K : verrou empoisonné"))?
            .prochain_durable()
    }
}

impl this_harness::routing::Seeds for SeedsDurables {
    fn next_id(&self) -> u128 {
        // Le trait `Seeds` n'offre aucun canal d'erreur : un état K corrompu, nul,
        // disparu sous verrou, RÉGRESSÉ ou saturé est un refus NOMMÉ (panique),
        // jamais une réinitialisation silencieuse ni un repli sur un K faux.
        self.autorite
            .lock()
            .expect("autorité K : verrou empoisonné")
            .emettre()
            .expect("autorité K : refus nommé (corruption, saturation, régression)")
    }
}
