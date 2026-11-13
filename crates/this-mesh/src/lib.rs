//! Couche de protocole du maillage des Ateliers, **indépendante du transport**.
//!
//! Aucune I/O : ce crate fixe ce qui doit être vrai quel que soit le transport
//! (iroh, quinn…) — l'enveloppe, les octets signés, le registre des pairs, la porte.
//!
//! Ordre de la porte ([`Gate::admettre`]) : taille → forme de l'enveloppe →
//! `de` = expéditeur → clé connue / révoquée → type connu → signature → rejeu.
//! La charge n'est jamais lue avant que la taille, l'identité et le type soient bons.

mod appairage;
mod envelope;
mod gate;
mod limits;
mod peers;
#[cfg(feature = "transport")]
pub mod transport;

pub use appairage::{Admission, Appairage, Rencontre, SAS_DIGITS, SAS_DOMAIN, SasAttendu, sas};
pub use envelope::{DeviceId, MESH_VERSION, Message, MessageType, SIGNING_DOMAIN, SignError};
pub use gate::{Gate, JournalEntry, Raison, Verdict};
pub use limits::Limits;
pub use peers::{PeerError, Peers};
