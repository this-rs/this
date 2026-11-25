# Résultats du banc — observation cognitive d'obrain (validateur this-harness)

Révisions **figées** ; chaque résultat est attaché au SHA qu'il a testé.

## Campagne 1 — suites telles quelles (25/11/2026)

| Révision | 5 suites non observées | 2 suites observées dédiées |
|---|---|---|
| `main` **203f3cd1** | 44 tests, 0 échec | 6 tests, 0 échec |
| PR #414 **d275a99a** | 44 tests, 0 échec | 6 tests, 0 échec |

## Campagne 2 — observation FORCÉE sur les 5 suites historiques

Variantes générées par `bench-forced-obs.sh` : copies dérivées avec
`ObrainDB::open(` → `open_substrate_observed(` et `new_in_memory()` →
`new_in_memory_observed()`, **assertions inchangées**.

| Suite (observé forcé) | `main` 203f3cd1 | `d275a99a` |
|---|---|---|
| durabilite_commit_kill9_observee | **2 échecs / 8** | **2 échecs / 8** |
| ecriture_multi_processus_observee | 0/3 | 0/3 |
| wal_recovery_observee | 0/22 | 0/22 |
| vector_dimension_native_observee | 0/5 | 0/5 |
| vector_coherence_ecritures_observee | 0/6 | 0/6 |

**Verdict CORRIGÉ** : ces 2 échecs ne démontrent pas une perte de durabilité.
L'oracle de `durabilite_commit_kill9` (`oracle_apres_ack`) suppose un **écrivain
unique** — il compare `wal_octets_durables()` à `taille_wal()`. Installer
l'observateur invalide cette assimilation, et les deux tests échouent sur
`assert_aucune_violation` **avant le kill**.

## Campagne 3 — oracle corrigé, par transaction (obrain PR draft #417)

Instrumentation : `seal_commit(transaction_id)` publie la fin de la frame de CE
commit (le `(position, longueur)` d'`append_encoded` était jeté) ; stockage
thread-local clé par `TransactionId` ; aucun verrou ajouté.

| Contrôle | Attendu | Obtenu |
|---|---|---|
| A — observé, 20 commits | aucune violation | **0 violation** |
| B — `SyncMode::Never` (marqueur écrit, pas de fsync) | détecter | **`fin_commit=175 > durable=0`** |
| C — deux fils, un store chacun | frontières propres | croissantes, > 0 |

`cargo test -p obrain-engine --features cognitive --test frontiere_commit_observee` → 3 passed, 0 failed.

**Non couvert** : `kill -9` + récupération avec l'oracle corrigé, concurrence sur
un même WAL, group commit, changement d'ancre après checkpoint, multi-graphes.
