#!/bin/sh
# Banc de validation durabilité/cognition obrain — POSIX, Mac et Linux.
#
#   run.sh --repo <clone obrain> --rev <sha|ref> --artifacts <dir> [--forced]
#
# --forced : dérive les 5 suites historiques en variantes OBSERVÉES
#            (open_substrate_observed), assertions inchangées.
#
# Propriétés garanties :
#   - dépôt source fourni, jamais deviné ; SHA complet résolu AVANT la campagne ;
#   - worktree isolé par campagne (aucun `git checkout` sur un dépôt partagé) ;
#   - répertoire d'artefacts unique (logs, commandes, diffs des variantes) ;
#   - vérification que les tests ont réellement tourné (> 0 exécuté) ;
#   - code de sortie global non nul si une suite inattendue échoue ;
#   - nettoyage du worktree et des fichiers générés ; seuls NOS enfants sont attendus ;
#   - aucun chemin de HOME codé en dur.
set -u

REPO=""; REV=""; ART=""; FORCED=0
while [ $# -gt 0 ]; do
  case "$1" in
    --repo) REPO="$2"; shift 2 ;;
    --rev) REV="$2"; shift 2 ;;
    --artifacts) ART="$2"; shift 2 ;;
    --forced) FORCED=1; shift ;;
    *) echo "argument inconnu : $1" >&2; exit 2 ;;
  esac
done
[ -n "$REPO" ] && [ -n "$REV" ] && [ -n "$ART" ] || { echo "usage: run.sh --repo <clone> --rev <sha> --artifacts <dir> [--forced]" >&2; exit 2; }
[ -d "$REPO/.git" ] || [ -d "$REPO/../.git" ] || { echo "--repo n'est pas un dépôt git : $REPO" >&2; exit 2; }
command -v cargo >/dev/null 2>&1 || { echo "cargo absent du PATH" >&2; exit 2; }

FULL=$(git -C "$REPO" rev-parse --verify "$REV^{commit}") || { echo "révision inconnue : $REV" >&2; exit 2; }
mkdir -p "$ART" || exit 2
WT="$ART/wt"
GEN="$ART/generated"
: > "$ART/commands.txt"

echo "rev=$FULL art=$ART forced=$FORCED" > "$ART/contexte.txt"
git -C "$REPO" log -1 --format='%H %ci %s' "$FULL" >> "$ART/contexte.txt"

# --- worktree isolé ---
git -C "$REPO" worktree remove --force "$WT" >/dev/null 2>&1
git -C "$REPO" worktree add --detach "$WT" "$FULL" >> "$ART/contexte.txt" 2>&1 || { echo "worktree impossible" >&2; exit 2; }
mkdir -p "$GEN"

nettoyer() {
  # on ne retire QUE notre worktree ; on ne touche pas aux processus d'autres travaux
  git -C "$REPO" worktree remove --force "$WT" >/dev/null 2>&1
  rm -f "$WT"/*_observee.rs 2>/dev/null
}
trap 'nettoyer' EXIT INT TERM

T="$WT/crates/obrain-engine/tests"

# --- variantes observées : générées ET vérifiées ---
deriver() { # $1=suite
  src="$T/$1.rs"; dst="$T/${1}_observee.rs"
  [ -f "$src" ] || { echo "suite absente : $1" >&2; return 1; }
  sed -e 's/ObrainDB::open(/ObrainDB::open_substrate_observed(/g' \
      -e 's/ObrainDB::new_in_memory()/ObrainDB::new_in_memory_observed()/g' "$src" > "$dst"
  # la transformation doit avoir mordu : une variante qui ne diffère pas ne prouve rien
  if cmp -s "$src" "$dst"; then
    echo "TRANSFORMATION SANS EFFET sur $1 (aucune ouverture à réécrire ?)" >&2
    return 1
  fi
  echo "# variante observée de $1" > "$GEN/${1}_observee.diff"
  diff -u "$src" "$dst" >> "$GEN/${1}_observee.diff"
  return 0
}

ECHECS=0; TOTAL=0
lancer() { # $1=nom du binaire, $2=features attendues
  echo "cargo test -p obrain-engine --features $2 --test $1" | tee -a "$ART/commands.txt"
  log="$ART/$1.log"
  ( cd "$WT" && cargo test -p obrain-engine --features "$2" --test "$1" -- --test-threads 1 ) > "$log" 2>&1
  code=$?
  ligne=$(grep -E "^test result:" "$log" | tail -1)
  execs=$(echo "$ligne" | sed -n 's/.* \([0-9][0-9]*\) passed.*/\1/p')
  execs=${execs:-0}
  ignorés=$(echo "$ligne" | sed -n 's/.* \([0-9][0-9]*\) ignored.*/\1/p')
  TOTAL=$((TOTAL + execs))
  if [ "$execs" -eq 0 ]; then
    echo "AUCUN TEST EXÉCUTÉ pour $1 — la suite n'a pas tourné" | tee -a "$ART/resume.txt"
    ECHECS=$((ECHECS + 1))
  elif [ "$code" -ne 0 ]; then
    echo "ÉCHEC $1 : $ligne" | tee -a "$ART/resume.txt"
    ECHECS=$((ECHECS + 1))
  else
    echo "ok   $1 : $ligne (ignorés=${ignorés:-0})" | tee -a "$ART/resume.txt"
  fi
}

: > "$ART/resume.txt"
echo "== révision $FULL ==" | tee -a "$ART/resume.txt"

# --- suites telles quelles ---
lancer durabilite_commit_kill9   "wal"
lancer ecriture_multi_processus  "wal"
lancer wal_recovery              "cypher"
lancer vector_dimension_native   "vector-index"
lancer vector_coherence_ecritures "vector-index"
lancer durabilite_cognitive_observee_kill9 "cognitive"
lancer cognitive_wiring_test     "cognitive"

# --- variantes observées forcées ---
if [ "$FORCED" -eq 1 ]; then
  echo "== variantes observées forcées ==" | tee -a "$ART/resume.txt"
  for s in durabilite_commit_kill9:wal ecriture_multi_processus:wal wal_recovery:cypher \
           vector_dimension_native:vector-index vector_coherence_ecritures:vector-index; do
    suite=${s%%:*}; feat=${s##*:}
    if deriver "$suite"; then
      lancer "${suite}_observee" "$feat,cognitive"
    else
      ECHECS=$((ECHECS + 1))
    fi
  done
fi

echo "== total tests exécutés : $TOTAL ; suites en échec : $ECHECS ==" | tee -a "$ART/resume.txt"
[ "$ECHECS" -eq 0 ] || exit 1
