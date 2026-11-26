# Port autonome K / OBRR / récupération vers `main` — verdict mesuré

**Date** : 26/11/2026 · **Toolchain** : `rustc 1.100.0 (0ef1bc386 2026-11-09)`, `cargo 1.100.0`
**Base** : `bcf89550` (`main` de `this-rs/this`, pin `obrain` = `7a795370`)
**Branche / worktree** : `port/k-obrr-recovery-main` sur `/Users/triviere/projects/this-rs/_wt-k-obrr-main`
**Sources des capacités** : `integ/abdk` (`b78aa8ea`, `2c986697`, `6c21ed40`) et `qualif/k-allocation` (`33eb9dd1`)

## Attestation d'arbre (les quatre vérifications, avant toute écriture)

```
git rev-parse --show-toplevel  -> /Users/triviere/projects/this-rs/_wt-k-obrr-main
realpath .                     -> /Users/triviere/projects/this-rs/_wt-k-obrr-main
git rev-parse --abbrev-ref HEAD-> port/k-obrr-recovery-main
git rev-parse HEAD             -> bcf895508aaa4502ab1df5082a80eb82619a755e
```

## 1. Inventaire mesuré : ce qui est autonome, ce qui ne l'est pas

Le pin de production est **`7a795370`** (`Cargo.toml` l. 126-144, six crates `obrain-*`).
`7a795370` est un **ancêtre strict** de `89d169d7` (`git merge-base 7a795370 89d169d7` = `7a795370`),
et de `dabcf91d`.

| capacité | dépendances réelles (relu) | verdict sur `7a795370` |
|---|---|---|
| allocation atomique de **K** (`src/autorite_k.rs`, `33eb9dd1`) | `std::fs` + `File::lock()` + `ecrire_durable` | **autonome** |
| **écriture durable** (`ecrire_durable`, `ecrire_durable_observe`, `temporaires_orphelins`) | `std::fs` uniquement | **autonome** |
| **codec OBRR** (`EnregistrementOriginal::{encoder,decoder}`, `33eb9dd1`/`6d7f9d4d`) | `obrain_common::types::NodeId` (présent au pin) | compilable, mais **format persistant sans récupération** → non porté (consigne) |
| **récupération / réconciliation** (`reconcilier`, `resoudre_operation`) | `ReferenceTentative`, `ResolutionTentative`, `IntegriteTentative`, `SubstrateStore::resoudre_reference` | **impossible sur ce pin** (§4) |
| fragment transactionnel (`EffetResolu.etat_courant`, `domaine`) | `dabcf91d` (pin expérimental) | **hors consigne**, non porté |

Le codec OBRR n'est pas porté pour une raison explicite, pas par oubli : la consigne dit
« ne pas fusionner de format persistant sans récupération fonctionnelle », et la seule
récupération fonctionnelle qu'il sert (`resoudre_operation` → `Attribution::resoudre`) est
précisément le chemin que le pin rend incompilable.

## 2. Ce qui est porté, et l'écart exact avec la source

| fichier | origine | écart |
|---|---|---|
| `src/autorite_k.rs` | `33eb9dd1:src/autorite_k.rs` (396 l.) | **une ligne** : `use crate::reconciliation::ecrire_durable` → `use crate::durabilite::ecrire_durable` |
| `src/durabilite.rs` | `6d7f9d4d:src/reconciliation.rs` l. 416-511, **verbatim** | extrait (même code, même ordre, mêmes commentaires) + en-tête de module propre |
| `src/lib.rs` | `bcf89550:src/lib.rs` | ajoute `pub mod autorite_k;` et `pub mod durabilite;`, **sans `cfg(feature = "obrain")`** |
| `tests/k_collision_concurrente.rs` | `33eb9dd1` | **une ligne** : `#![cfg(feature = "obrain")]` retiré (le fichier n'utilise que `this::autorite_k`) |
| `tests/k_allocation_atomique.rs` | `33eb9dd1` | 8 témoins sur 9 + imports + en-tête ; la section 7 (`obrr_v1_string_jamais_reinterprete_en_u128`) est **retirée et le dit** |
| `tests/durabilite_primitive.rs` | **adapté** de `durabilite_obrr.rs::ecriture_durable_ordre_observe` (`6d7f9d4d`) | la source fabriquait ses octets via le codec OBRR ; ici octets opaques. La propriété mesurée est la même (l'ORDRE des cinq étapes) |

Pourquoi `durabilite` est un module séparé : `AutoriteK` n'a besoin que de cette primitive, alors que
`reconciliation.rs` la portait dans le même fichier que le codec et la réconciliation. Extraire la
primitive est ce qui rend **K autonome** sans tirer le format persistant. Aucune seconde
implémentation n'est créée : le bloc est copié à l'identique.

## 3. Témoins exécutés (EXIT séparés)

Répertoire de travail Cargo propre : `CARGO_TARGET_DIR=/Users/triviere/projects/this-rs/_wt-k-obrr-target`.

| # | commande | témoins | EXIT | log |
|---|---|---|---|---|
| T1 | `cargo test --lib autorite_k -- --test-threads=1` | 5 | **0** | `t1-lib-unitaire.log` |
| T2 | `cargo test --test durabilite_primitive -- --test-threads=1` | 3 | **0** | `t2-durabilite-primitive.log` |
| T3 | `cargo test --test k_allocation_atomique -- --test-threads=1` | 8 | **0** | `t3-k-allocation.log` |
| T4 | `cargo test --test k_collision_concurrente -- --test-threads=1` | 2 | **0** | `t4-k-collision.log` |
| T5 | `cargo test --lib -- --test-threads=1` (non-régression) | 1116 | **0** | `t5-regression-lib.log` |
| — | `cargo check --features obrain --lib` | — | **0** | `check-features-obrain.log` |
| — | `cargo clippy --features obrain --lib --tests -- -D warnings` | — | **0** | `clippy.log` |
| — | `cargo fmt --check` | — | **0** | `fmt.log` |

Les deux témoins multi-processus traversent une vraie frontière de processus
(`std::process::Command` sur `current_exe()` + `--exact`), pas un close/open.

Les trois binaires ont aussi été rejoués en **parallélisme par défaut** (comme la CI, sans
`--test-threads=1`) : tous EXIT=0 — `par-*.log`.

## 4. Contre-épreuves

### 4.a Le témoin de collision DISCRIMINE — démontré

Même binaire de test, mêmes commandes, implémentation `K` remplacée par celle de la base
`6d7f9d4d` (compteur lu à l'OUVERTURE, sans verrou) :

```
cargo test --test k_collision_concurrente -- --test-threads=1   -> EXIT=101
thread 'collision_concurrente_interleaving_deterministe' panicked:
  assertion `left == right` failed: COLLISION : les deux processus ont publié le même K (1 et 1) ; l'état durable vaut 2 au lieu de 3
    left: [1, 1]
   right: [1, 2]
thread 'contention_multiprocessus_sans_doublon' panicked:
  DOUBLONS : 20 K seulement sont distincts sur 80 émissions
```

Log : `c1-contre-epreuve-base-lotG.log`. Le fichier `src/autorite_k.rs` a ensuite été restauré —
sha256 identique avant/après (`9897664f…`).

### 4.b Le contrat natif manquant est COMPILÉ, pas supposé

`src/reconciliation.rs` (intégral, `6d7f9d4d:src/reconciliation.rs`, 838 l.) a été injecté tel quel
avec `#[cfg(feature = "obrain")] pub mod reconciliation;`, puis :

```
cargo check --features obrain --lib   -> EXIT=101
error[E0432]: unresolved imports `obrain_substrate::IntegriteTentative`,
              `obrain_substrate::ReferenceTentative`, `obrain_substrate::ResolutionTentative`
  --> src/reconciliation.rs:26:5
error[E0599]: no method named `resoudre_reference` found for reference `&SubstrateStore`
  --> src/reconciliation.rs:828:14
error[E0599]: no method named `generation` / `decision_id` / `autorite` found for `&Tentative`
```

Log : `p1-sonde-obrr-pin-7a795370.log`. La sonde a été retirée (`src/reconciliation.rs` supprimé,
`src/lib.rs` restauré) ; `git status` ne la montre plus.

## 5. Le contrat natif manquant, nommé

Dans `obrain-substrate` au pin `7a795370`, **absent** de la racine du crate :

- `ReferenceTentative` — la référence engageante `(auteur, horloge, cible, effets)`, et son codec
  versionné (`REFERENCE_TENTATIVE_MAGIC`, `REFERENCE_TENTATIVE_VERSION`) ;
- `ResolutionTentative` (+ `ResolutionEffet`) — le résultat d'une résolution avec son tuple relu ;
- `IntegriteTentative` — `Integrale` / `Partielle` / `Indeterminee` ;
- `SubstrateStore::reference_tentative()` — produire la référence depuis le store ;
- `SubstrateStore::resoudre_reference()` — résoudre une référence contre l'état persisté.

Ces symboles sont introduits **ensemble** par un seul commit, `b366a8c5`
*(« feat(substrate): ReferenceTentative V2 — codec versionné, résolution multi-effets, refus différé nommé »)*,
qui est dans `7a795370..89d169d7` et **hors** de l'ascendance de `7a795370` :

```
git log --oneline -S "pub struct ReferenceTentative" 7a795370..89d169d7 -- crates/obrain-substrate/src/store.rs
  -> b366a8c5 ...
```

Deux dépendances distinctes, à ne pas confondre :

1. **native (obrain)** : les cinq symboles ci-dessus ⇒ exige un pin ≥ `89d169d7` ;
2. **locale (ce dépôt)** : `Tentative::{autorite,generation,decision_id}` et `Attribution::autorite()`
   (27 lignes, `crates/this-harness/src/attribution.rs`) — portables telles quelles, mais **sans objet**
   tant que la dépendance native manque.

Le pin de production n'est **pas** modifié par ce port : relever `obrain` de `7a795370` à `89d169d7`
est une décision d'orchestrateur (34 commits de substrat), pas un effet de bord d'un port.

## 6. Ce qui n'est PAS qualifié ici

- **Survie à une coupure d'alimentation** : `fsync` en est la condition nécessaire, pas la preuve.
- **Unicité globale de K** : K n'est unique que DANS son domaine (le fichier d'état) ; deux
  fichiers = deux autorités qui peuvent émettre le même K (témoin `domaine_canonique_…`, qui mesure
  cette non-unicité au lieu de la masquer).
- **Restauration d'un snapshot du fichier d'état** : une régression n'est refusée que dans un
  processus vivant ; un retour arrière du fichier entre deux processus n'est pas détectable.
- **Identité de domaine backend** : `writer.id` ne discrimine pas les bases (mesure obrain
  `62ecd940`) ; `ReferenceTentative` ne constitue pas sa propre preuve d'origine.
- **Récupération / réconciliation** : non portée — c'est le §5.
- **`tests/k_durable_autorite.rs`, `tests/durabilite_obrr.rs`, `tests/h3_v2_*`,
  `tests/reconciliation_composant.rs`** : non portés (même dépendance native).

## 7. Registre épistémique

| énoncé | registre |
|---|---|
| `obrain-substrate@7a795370` n'a pas `ReferenceTentative`/`ResolutionTentative`/`IntegriteTentative`/`resoudre_reference` | **Démontré** (compilation, `p1-…log`, E0432 + E0599) |
| ces symboles viennent de `b366a8c5`, absent de l'ascendance de `7a795370` | **Relu** (`git log -S` dans `obrain-db`, source nommée) |
| l'ancienne allocation de K produit des collisions (K identiques, 20/80 distincts) | **Démontré** (contre-épreuve 4.a, EXIT=101) |
| la nouvelle allocation n'en produit pas sous contention réelle (4 processus × 20) | **Démontré** (T4, EXIT=0 ; échoue sur la base) |
| l'ordre `verrou → lecture → persistance → publication` | **Démontré** (T1, `EtapeEmission` observé ; rouge sur la base par construction) |
| l'ordre des cinq étapes d'écriture durable (octets `fsync`és AVANT rename, répertoire APRÈS) | **Démontré** (T2) |
| K survit à la mort du processus | **Mesuré** (réouverture : compteur repris ; K brûlé non réattribué) — pas une preuve de survie à une panne |
| le codec OBRR + réconciliation compilent sur `89d169d7` | **Non mesuré ici** — mesure antérieure sur un autre SHA (`ceb706ff`, `be4b4288`), non transférable |
