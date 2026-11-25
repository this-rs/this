#!/bin/sh
# Banc comparatif : durabilité des 5 suites historiques (non observées) et des 2
# suites cognitives observées, sur une révision donnée d'obrain.
# usage : bench-observation.sh <rev> <etiquette>
set -u
REV="$1"; ETQ="$2"
export HOME=/Users/triviere RUSTUP_HOME=/Users/triviere/.rustup CARGO_HOME=/Users/triviere/.cargo
export PATH=/Users/triviere/.cargo/bin:/opt/homebrew/bin:/usr/local/bin:$PATH
cd /Users/triviere/projects/obrain/obrain-db || exit 1
git checkout -q "$REV" || { echo "checkout $REV échoué"; exit 2; }
LOG="/tmp/bench-$ETQ.log"
: > "$LOG"
echo "== révision $ETQ = $(git log -1 --format='%h %ci %s')" >> "$LOG"

# Groupe 1 : non observé (wal + cypher + vector-index)
for suite in durabilite_commit_kill9 ecriture_multi_processus wal_recovery vector_dimension_native vector_coherence_ecritures; do
  echo "### $suite (non observé)" >> "$LOG"
  cargo test -p obrain-engine --features wal,cypher,vector-index --test "$suite" >> "$LOG" 2>&1
  echo "### exit=$?" >> "$LOG"
done

# Groupe 2 : observé (cognitive)
for suite in durabilite_cognitive_observee_kill9 cognitive_wiring_test; do
  echo "### $suite (observé)" >> "$LOG"
  cargo test -p obrain-engine --features cognitive --test "$suite" >> "$LOG" 2>&1
  echo "### exit=$?" >> "$LOG"
done
echo "== FINI $ETQ ==" >> "$LOG"
