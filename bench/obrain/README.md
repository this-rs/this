# Banc d'observation cognitive obrain (validateur this-harness)

Rejoue les suites de durabilité d'obrain en mode NON observé et OBSERVÉ, sur
une révision donnée, pour trancher « l'observation interfère-t-elle avec le
WAL/checkpoint/leases/récupération ? ».

## Usage

```sh
bench/obrain/bench-observation.sh <rev> <étiquette>
```

`<rev>` est un commit/réf du dépôt obrain-db ; le script `git checkout` dessus,
compile puis exécute 7 suites et écrit le log dans `/tmp/bench-<étiquette>.log`.

## Suites

- NON observé (features `wal,cypher,vector-index`) :
  `durabilite_commit_kill9`, `ecriture_multi_processus`, `wal_recovery`,
  `vector_dimension_native`, `vector_coherence_ecritures`.
- OBSERVÉ (feature `cognitive`) :
  `durabilite_cognitive_observee_kill9`, `cognitive_wiring_test`.

## Limite connue

Les 5 suites historiques tournent ici en mode NON observé (l'observation est
opt-in par ouverture `open_substrate_observed`). La matrice complète « avec
observation forcée » est un chantier séparé (tâche « Terminer la matrice
transactionnelle »).
