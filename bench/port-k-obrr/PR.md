## Objet

Porter sur `main` (`bcf89550`, pin `obrain` **`7a795370` inchangé**) les capacités opérationnelles
réellement autonomes de la lignée K/OBRR : **allocation atomique de K** et **écriture durable**.
Le reste (codec OBRR, récupération/réconciliation) est démontré **incompilable sur ce pin** — preuve
compilée jointe — et n'est donc pas porté.

## Ce qui est porté

| fichier | contenu | écart avec la source |
|---|---|---|
| `src/autorite_k.rs` | K = `u128`, verrou exclusif tenu de la lecture à la publication, relecture de l'état sous verrou, persistance durable **avant** publication, refus nommés (tronqué / nul / disparu / régressé / saturation) | **une ligne d'import** (`crate::durabilite`) |
| `src/durabilite.rs` | temporaire → octets → `fsync` fichier → `rename` → `fsync` répertoire ; `temporaires_orphelins` | copié **verbatim** de `6d7f9d4d:src/reconciliation.rs` l. 416-511 |
| `src/lib.rs` | `pub mod autorite_k;` `pub mod durabilite;` | **hors** de la feature `obrain` (aucune dépendance obrain) |
| `tests/k_collision_concurrente.rs` | 2 témoins multi-processus (frontière de processus réelle) | `#![cfg(feature = "obrain")]` retiré |
| `tests/k_allocation_atomique.rs` | 8 témoins | 9ᵉ (codec OBRR) retiré, et le fichier le dit |
| `tests/durabilite_primitive.rs` | 3 témoins de l'ordre d'écriture | **adapté** de `durabilite_obrr.rs::ecriture_durable_ordre_observe` (octets opaques au lieu d'un record OBRR) |

## Témoins — EXIT séparés

| suite | témoins | EXIT |
|---|---|---|
| `cargo test --lib autorite_k` | 5 | 0 |
| `cargo test --test durabilite_primitive` | 3 | 0 |
| `cargo test --test k_allocation_atomique` | 8 | 0 |
| `cargo test --test k_collision_concurrente` | 2 | 0 |
| `cargo test --lib` (non-régression) | 1116 | 0 |
| `cargo clippy --features obrain --lib --tests -- -D warnings` | — | 0 |
| `cargo fmt --check` | — | 0 |

## Contre-épreuves

1. **Discrimination** : le même binaire `k_collision_concurrente` exécuté sur l'implémentation de
   base (compteur lu à l'ouverture, sans verrou) rend **EXIT=101** — `COLLISION : les deux processus
   ont publié le même K (1 et 1)` et `20 K seulement sont distincts sur 80 émissions`. Il passe sur
   ce port. `src/autorite_k.rs` restauré, sha256 identique (`9897664f…`).
2. **Contrat natif manquant, compilé** : `src/reconciliation.rs` intégral injecté sur le pin rend
   E0432 (`ReferenceTentative`, `ResolutionTentative`, `IntegriteTentative`) et E0599
   (`SubstrateStore::resoudre_reference`). Ces symboles arrivent ensemble avec
   `obrain@b366a8c5`, hors de l'ascendance de `7a795370`.

## Ce que cette PR ne fait pas

- Elle ne relève **pas** le pin `obrain` (`7a795370` → `89d169d7` = 34 commits de substrat) :
  décision d'orchestrateur, pas effet de bord d'un port.
- Elle ne fusionne **pas** le codec OBRR : format persistant sans récupération fonctionnelle.
- Elle ne porte **pas** le fragment transactionnel (`EffetResolu.etat_courant`, `domaine`), qui exige
  le pin expérimental `dabcf91d`.

Verdict complet, avec le registre épistémique et les bornes : `bench/port-k-obrr/VERDICT.md`.
