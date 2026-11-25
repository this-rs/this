#!/bin/sh
# Force l'observation sur les 5 suites historiques (copies dérivées, assertions intactes)
# et rejoue sur <rev>. Usage: bench-forced-obs.sh <rev> <etiquette>
set -u
REV="$1"; ETQ="$2"
export HOME=/Users/triviere RUSTUP_HOME=/Users/triviere/.rustup CARGO_HOME=/Users/triviere/.cargo
export PATH=/Users/triviere/.cargo/bin:/opt/homebrew/bin:/usr/local/bin:$PATH
cd /Users/triviere/projects/obrain/obrain-db || exit 1
git checkout -q --detach "$REV" || { echo "checkout $REV échoué"; exit 2; }
T=crates/obrain-engine/tests
LOG="/tmp/fobs-$ETQ.log"; : > "$LOG"
echo "== révision $ETQ = $(git log -1 --format='%h %s')" >> "$LOG"

gen() { # $1 = suite, $2 = feature(s) d'origine
  src="$T/$1.rs"; dst="$T/${1}_observee.rs"
  sed -e 's/ObrainDB::open(/ObrainDB::open_substrate_observed(/g' \
      -e 's/ObrainDB::new_in_memory()/ObrainDB::new_in_memory_observed()/g' "$src" > "$dst"
  # marqueur de provenance (ne touche aucune assertion)
  { echo "// Dérivé de $1.rs par le banc du validateur this-harness : OUVERTURE OBSERVÉE forcée."
    echo "#![cfg(feature = \"cognitive\")]"
    cat "$dst"; } > "$dst.tmp" && mv "$dst.tmp" "$dst"
}

for s in durabilite_commit_kill9:wal ecriture_multi_processus:wal wal_recovery:cypher vector_dimension_native:vector-index vector_coherence_ecritures:vector-index; do
  suite="${s%%:*}"; feat="${s##*:}"
  gen "$suite"
  echo "### ${suite}_observee (observé)" >> "$LOG"
  cargo test -p obrain-engine --features "$feat,cognitive" --test "${suite}_observee" >> "$LOG" 2>&1
  echo "### exit=$?" >> "$LOG"
done
echo "== FINI $ETQ ==" >> "$LOG"
