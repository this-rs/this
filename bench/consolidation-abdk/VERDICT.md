# Consolidation A/B/D/K — SHA commun + suites communes

## Identité

| objet | valeur |
|---|---|
| SHA mesuré (arbre commité) | `2c986697593d1a7c07b45a3bcde6f7a3581abacb` |
| branche | `integ/abdk` |
| worktree isolé | `/Users/triviere/projects/this-rs/_wt-abdk` |
| base commune des deux lignées | `a583633faf2a29c8a641cd4723788636dde84d49` |
| delta A/B/D réuni | `6c21ed402c7d789ca1e7328a38cd14b11d7dabe9` (`chore/convergence-abd`) |
| delta K réuni | `33eb9dd1c38e99a1f389adeb817e33456a46a326` + `66eb98e1e7ada4f584817b641e885b941ed33469` (`qualif/k-allocation`) |
| toolchain | rustc 1.100.0 (0ef1bc386 2026-11-09) |
| features | `obrain` |
| target dir | `_wt-abdk-target` (hors arbre) |
| checkout partagé `this` | NON utilisé ; `git status` vide avant et après |
| push | AUCUN |

Les deux lignées partageaient `a583633f` : aucun conflit TEXTUEL n'est apparu
(cherry-pick `EXIT=0` des deux commits). Le conflit était SÉMANTIQUE, et un seul.

## 1. Le conflit sémantique réel, et sa résolution

**Une seule définition de chaque chose survit.** Ce qui divergeait :

| grandeur | survit | écarté |
|---|---|---|
| codec OBRR | A/B/D : `ENREGISTREMENT_VERSION = 5`, `domaine: String` + `ecrivain: u64`, `encoder() -> Result` | le témoin K avait été écrit contre `V2`, champ unique `backend: u64`, `encoder() -> Vec<u8>` |
| allocation K | K : `AutoriteK` à verrou (une seule implémentation, `src/autorite_k.rs`) | A/B/D n'y touchait pas — aucun doublon |
| chaîne de récupération | A/B/D : `qualifier` → `relire_qualifie` → `RecordLegacy::resoudre_obligation` | aucune seconde chaîne |
| registre durable | `ecrire_durable` (`src/reconciliation.rs:620`), seule primitive, appelée par `autorite_k` | aucun registre ad hoc |

**Résolution retenue** : c'est le codec de A/B/D qui survit (c'est lui qui porte
le contrat corrigé du secondaire, `etat_courant` + identité de domaine), et le
témoin K est PORTÉ dessus — pas l'inverse. `backend` se scinde en `domaine` +
`ecrivain`, qui sont **deux grandeurs distinctes** (l'identité de base est
hors-bande et n'apparaît nulle part dans la référence ; `writer.id` n'est pas un
domaine — deux bases peuvent le partager). Fusionner ces deux-là aurait été la
faute exacte que le renommage corrige.

**Le témoin n'a pas été affaibli.** Ses assertions sont inchangées :
V1 `k = "7"` (chaîne) reste en `QuarantaineLegacy` et n'est **jamais** promu en
`K = 7` ; le codec courant refuse ces octets (`VersionInconnue(1)`) ; la
corruption d'un record courant reste un refus NOMMÉ (`VersionInconnue(99)`) ;
les octets V1 sont préservés VERBATIM au rejeu disque.

**Preuve structurelle de non-duplication** (`grep` sur `src/`) :
- un seul `pub fn emettre` (`src/autorite_k.rs:179`, délégué à `emettre_observe:190`) ;
- un seul `ENREGISTREMENT_MAGIC` / une seule version courante (`src/reconciliation.rs:55,62`) ;
- un seul `pub backend` restant, et c'est celui de `EnregistrementV1` (vue legacy, `:725`) — pas le record courant ;
- une seule `resoudre_obligation`, une seule `qualifier`, une seule `ecrire_durable`.

## 2. Deux lints HÉRITÉS, réparés (forme seulement)

`clippy -D warnings` échouait sur des défauts **hérités de `6c21ed40`**, pas
introduits par la fusion — vérifié identique ligne à ligne à `6c21ed40` :
`doc_lazy_continuation` (`tests/h3_v2_chaine_autorite.rs`) et
`redundant_field_names` ×3 (`tests/k_durable_autorite.rs`, `k: k` → `k`).
Corrections de forme, aucune logique de témoin modifiée.

## 3. Pin Obrain RÉEL (`cargo metadata`, pas la déclaration)

Déclaré dans `Cargo.toml` ET résolu (`cargo metadata --features obrain`,
`EXIT=0`) ET présent dans `Cargo.lock` — les trois concordent :

```
obrain-{engine,common,core,substrate,adapters,reactive} v0.0.1
source = git+file:///Users/triviere/projects/obrain/obrain-db?rev=dabcf91d38a6d779dea674b2f31007112971e5cf#dabcf91d...
```

**Écart de pin introduit par A/B/D** : la base `a583633f` pointait
`git+https://github.com/obrain-db/obrain` rev `89d169d7…` ; `6c21ed40` la
re-pointe sur `file:///Users/triviere/projects/obrain/obrain-db` rev `dabcf91d…`.
Les deux révisions sont les deux SEULS pins de l'arbre consolidé, ils sont
distincts, et le seul pin effectif est le LOCAL.

Mesuré sur le dépôt obrain local : `dabcf91d` = merge LOCAL de la branche
`exp/lot-de` ; **aucune branche de suivi distante du clone ne le contient**
(`git branch -r --contains dabcf91d` → vide). `git ls-remote` n'a pas pu être
exécuté (clé SSH indisponible dans cet environnement), donc l'absence DISTANTE
n'est pas qualifiée — seule l'absence du clone l'est.

Mesuré aussi : `Cargo.lock` est **ignoré par git** (`.gitignore:8`), donc la
résolution des dépendances ne fait pas partie du SHA. Le SHA n'atteste pas
l'arbre résolu ; il atteste les sources.

## 4. Suites exécutées ENSEMBLE, sur le même SHA, codes de sortie séparés

`bench/consolidation-abdk/suites.log` (arbre commité, `git status` propre hors journal) :

| suite | tests | EXIT |
|---|---|---|
| `reconciliation_composant` | 2 | 0 |
| `h3_v2_cycle_harness` | 5 | 0 |
| `h3_v2_chaine_autorite` | 7 | 0 |
| `k_durable_autorite` | 8 | 0 |
| `durabilite_obrr` | 15 | 0 |
| `k_allocation_atomique` | 9 | 0 |
| `k_collision_concurrente` | 2 | 0 |
| **total** | **48** | **0 partout** |

Plus, sur le même arbre : `clippy -p this-rs --features obrain --all-targets -- -D warnings`
→ `EXIT=0` ; `cargo test --lib` → **1364 passed, 0 failed, 1 ignored**, `EXIT=0`.

Note de lecture : les lignes « 1 passed; N filtered out » dans les journaux sont
les ré-exécutions du binaire par les témoins multi-PROCESSUS (relance de
soi-même en `--exact`), pas des tests supplémentaires.

## 5. Ce que ce SHA établit

- **DÉMONTRÉ** — chaque témoin discriminant échoue si sa propriété manque. En
  particulier `k_collision_concurrente` échouait sur `a583633f`
  (`bench/k-alloc/base-collision.log`, `EXIT=101`, K(a)=K(b)=1) et passe ici
  (K distincts) : l'allocation atomique est la cause. Le V1 `"7"` n'est pas
  promu : si le codec le réinterprétait, les octets V1 (longueur 1 + `0x37`)
  donneraient un `k` de 16 octets différent de 7, et l'assertion tomberait.
- **MESURÉ** — 48/48 tests verts + 1364 tests de lib + clippy, tous `EXIT=0`,
  toolchain 1.100.0, features `obrain`, sur `2c986697`.
- **RELU** — une seule implémentation par grandeur (sources nommées au §1) ;
  `domaine` est encodé mais JAMAIS vérifié depuis la référence.
- **INTERPRÉTÉ** — l'absence de conflit textuel signifie que les deux lignées
  n'ont jamais écrit dans les mêmes fichiers ; elle ne dit rien de leur
  compatibilité sémantique, qui a dû être établie à la main (c'est le §1).

## 6. Réserves NOMMÉES — ce que ce candidat ne prouve PAS

1. **Identité de domaine NON RÉSOLUE.** Le verrou d'`AutoriteK` est local au
   système de fichiers qui porte `<état>.verrou`. Il démontre l'atomicité entre
   instances et entre processus SUR CE FICHIER. Il ne démontre **aucune
   continuité après restauration** : un retour arrière du fichier d'état n'est
   pas détectable (seule une régression *dans un processus vivant* est refusée,
   via `dernier_emis`). Seul un témoin externe (journal d'émission, ancre hors
   du domaine) pourrait la constater — il n'existe pas ici.
2. **Contrôle de domaine hors-bande ≠ identité native backend.** `domaine` est
   une chaîne conservée par le consommateur et jamais vérifiée par
   `reconcilier` (indémontrable depuis la référence). Ce n'est donc PAS une
   preuve d'origine fournie par le backend, et cela ne doit jamais être présenté
   comme telle. Découverte rappelée : deux bases partageant `writer.id`
   produisent des `ReferenceTentative` identiques octet pour octet.
3. **Identité ENGAGÉE ≠ effet cognitif CONFIRMÉ.** Les suites montrent qu'une
   tentative est engagée, liée, résolue et que sa récupération est rejouable.
   Elles ne montrent pas qu'un effet cognitif a été produit dans le backend ni
   qu'il est observable. Aucun témoin de bout en bout ne le mesure ici.
4. **Pin non reproductible.** Le pin effectif est un `file://` sur un rev qui
   n'est sur aucune branche de suivi distante du clone (absence distante non
   qualifiée, SSH indisponible). Ce candidat est donc **expérimental et
   local** : il ne se rejoue pas sur une autre machine, et il ne change pas le
   pin de production.
5. **`Cargo.lock` hors SHA** (`.gitignore:8`) : la résolution des dépendances
   n'est pas attestée par le SHA.
6. **Autonomie des parties pures non extraite.** Le module `reconciliation.rs`
   importe les types substrate : « A/B/D purs this-rs » a été établi par
   SÉPARATION, jamais par extraction en module compilant sur le pin
   `7a795370`.
7. **Périmètre** : non mesurés ici — ScyllaDB, les autres backends, le
   chargeur wasm, et toute la matrice transactionnelle (5 configs × 5 suites).

## 7. Verdict

**CANDIDAT EXPÉRIMENTAL — consolidation A/B/D/K RÉUSSIE, réserves nommées.**

Une seule allocation K, un seul codec OBRR, une seule chaîne de récupération,
un seul registre durable : mesuré structurellement, et les 4 suites A/B/D et les
2 suites K passent ENSEMBLE sur le même SHA, `EXIT=0` partout, avec les deux
lints hérités réparés.

Ce verdict ne se transfère à aucun autre SHA. Il est prononcé sur
`2c986697593d1a7c07b45a3bcde6f7a3581abacb` exactement.
