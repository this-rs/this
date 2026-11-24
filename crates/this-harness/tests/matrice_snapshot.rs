//! Épingle la matrice de `docs/harnais-reprise-durable.md` §1 : `Snapshot` est un état
//! de **conversation** (fil, file) et, depuis #287, de **révocations** (`revoked`). Il ne
//! porte JAMAIS d'octroi. Ajouter un champ (identité de run, étapes, effets inconnus…)
//! change ce que la reprise promet : ce test cesse alors de compiler et oblige à relire
//! la matrice avant de le corriger.

use this_harness::Snapshot;

#[test]
fn snapshot_reste_a_quatre_champs_tant_que_la_matrice_le_dit() {
    // Destructuring EXHAUSTIF, sans `..` : un quatrième champ est une erreur de compilation.
    let Snapshot {
        version,
        transcript,
        pending,
        revoked,
    } = Snapshot::default();
    let _ = (version, transcript, pending, revoked);
}
