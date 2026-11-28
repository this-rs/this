//! Le ledger natif est-il RÉELLEMENT atteignable depuis le consommateur ?
//!
//! `SubstrateAtomicEventLedger` n'est exporté que par
//! `#[cfg(all(feature = "engram", feature = "substrate"))]`. Déclarer `engram` ne suffit
//! donc pas : ce témoin échoue à la compilation si la combinaison n'est pas active, ce qui
//! est exactement le genre de manque qu'un `cargo tree` ne montre pas.
#![cfg(feature = "obrain-cognitive")]

/// La signature que le consommateur veut utiliser, écrite telle quelle : si le type n'est pas
/// exporté, ou si sa forme change, ce fichier ne compile plus.
#[test]
fn le_ledger_natif_est_exporte_par_la_combinaison_engram_substrate() {
    fn _accepte(_ledger: obrain_cognitive::engram::SubstrateAtomicEventLedger) {}
}
