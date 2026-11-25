#!/bin/sh
# Auto-contrôles du banc, avec un FAUX cargo : aucun obrain compilé, aucun réseau.
# Vérifie les comportements élémentaires avant toute campagne réelle.
set -u
ICI=$(cd "$(dirname "$0")" && pwd)
BANC="$ICI/run.sh"
TRAVAIL=$(mktemp -d "${TMPDIR:-/tmp}/selftest-banc.XXXXXX") || exit 2
trap 'rm -rf "$TRAVAIL"' EXIT
ECHECS=0
verifier() { # $1=libellé $2=attendu $3=obtenu
  if [ "$2" = "$3" ]; then printf 'ok   %s (%s)\n' "$1" "$3"
  else printf 'ÉCHEC %s : attendu=%s obtenu=%s\n' "$1" "$2" "$3"; ECHECS=$((ECHECS+1)); fi
}

# --- faux dépôt obrain : 5 suites avec des ouvertures à réécrire ---
DEPOT="$TRAVAIL/faux-depot"; mkdir -p "$DEPOT/crates/obrain-engine/tests"
for s in durabilite_commit_kill9 ecriture_multi_processus wal_recovery vector_dimension_native vector_coherence_ecritures; do
  printf 'fn t() { let _ = ObrainDB::open("x"); }\n' > "$DEPOT/crates/obrain-engine/tests/$s.rs"
done
git -C "$DEPOT" init -q && git -C "$DEPOT" add -A \
  && git -C "$DEPOT" -c user.name=t -c user.email=t@t commit -qm "faux dépôt"

# --- faux cargo : sortie contrôlée par nom de suite ---
BIN="$TRAVAIL/bin"; mkdir -p "$BIN"
cat > "$BIN/cargo" <<'FAUX'
#!/bin/sh
suite=""
prev=""
for a in "$@"; do
  [ "$prev" = "--test" ] && suite="$a"
  prev="$a"
done
case "$suite" in
  durabilite_commit_kill9)         echo "test result: ok. 8 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out" ;;
  ecriture_multi_processus)        echo "test result: FAILED. 1 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out"; exit 101 ;;
  wal_recovery)                    echo "test result: FAILED. 0 passed; 3 failed; 0 ignored; 0 measured; 0 filtered out"; exit 101 ;;
  vector_dimension_native)         echo "test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 5 filtered out" ;;
  vector_coherence_ecritures)      echo "error[E0425]: cannot find value"; exit 101 ;;
  *)                               echo "test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out" ;;
esac
exit 0
FAUX
chmod +x "$BIN/cargo"
export PATH="$BIN:$PATH"

# --- 1..5 : une campagne qui doit tout classer correctement ---
A1="$TRAVAIL/a1"
"$BANC" --repo "$DEPOT" --rev HEAD --artifacts "$A1" >"$TRAVAIL/a1.out" 2>&1
verifier "campagne en échec => exit 1" "1" "$?"
verifier "suite réussie classée ok" "1" "$(grep -c '^ok durabilite_commit_kill9' "$A1/resume.txt")"
verifier "échec partiel distingué" "1" "$(grep -c '^ÉCHEC ecriture_multi_processus' "$A1/resume.txt")"
verifier "échec total distingué"   "1" "$(grep -c '^ÉCHEC wal_recovery' "$A1/resume.txt")"
verifier "zéro test distingué"     "1" "$(grep -c '^AUCUN TEST EXÉCUTÉ vector_dimension_native' "$A1/resume.txt")"
verifier "erreur de compilation distinguée" "1" "$(grep -c '^ERREUR DE COMPILATION vector_coherence_ecritures' "$A1/resume.txt")"
verifier "compteurs séparés"       "1" "$(grep -c 'passed=13 failed=4 ignored=0 filtered_out=5' "$A1/resume.txt")"
verifier "commande complète journalisée (7 suites)" "7" "$(grep -c -- '--test-threads 1' "$A1/commands.txt")"
verifier "worktree nettoyé"        "absent" "$([ -d "$A1/wt" ] && echo présent || echo absent)"

# --- 6 : collision de répertoire d'artefacts ---
"$BANC" --repo "$DEPOT" --rev HEAD --artifacts "$A1" >"$TRAVAIL/a2.out" 2>&1
verifier "collision d'artefacts refusée (exit 3)" "3" "$?"

# --- 7 : interruption contrôlée ---
A3="$TRAVAIL/a3"
"$BANC" --repo "$DEPOT" --rev HEAD --artifacts "$A3" --forced >"$TRAVAIL/a3.out" 2>&1 &
PID=$!
sleep 0.3; kill -TERM "$PID" 2>/dev/null; wait "$PID"; code=$?
verifier "interruption => exit 130" "130" "$code"
verifier "artefacts conservés après interruption" "oui" "$([ -f "$A3/resume.txt" ] && echo oui || echo non)"
verifier "worktree nettoyé après interruption" "absent" "$([ -d "$A3/wt" ] && echo présent || echo absent)"

# --- variantes : la transformation doit être vérifiée ---
A4="$TRAVAIL/a4"
"$BANC" --repo "$DEPOT" --rev HEAD --artifacts "$A4" --forced >"$TRAVAIL/a4.out" 2>&1
verifier "variantes générées et comptées" "1" "$(grep -c '1 ouvertures attendues, 1 réécrites' "$A4/generated/durabilite_commit_kill9_observee.diff" 2>/dev/null || echo 0)"

echo
echo "== auto-contrôles en échec : $ECHECS =="
[ "$ECHECS" -eq 0 ] || exit 1
