#!/bin/sh
# Banc de validation durabilité/cognition obrain — POSIX, Mac et Linux.
#
#   run.sh --repo <clone obrain> --rev <sha|ref> --artifacts <dir-exclusif> [--forced]
#
# --forced : dérive les 5 suites historiques en variantes OBSERVÉES.
#
# Garanties (chacune est vérifiée par bench/obrain/selftest.sh) :
#   - dépôt reconnu par `git rev-parse`, SHA complet résolu avant la campagne ;
#   - worktree isolé ; un `cd` impossible ARRÊTE la suite (jamais un faux "0 test") ;
#   - artefacts dans un répertoire EXCLUSIF : une collision est refusée, jamais écrasée ;
#   - compteurs séparés : passed / failed / ignored / filtered out / 0-test / erreur de compilation ;
#   - code de sortie non nul si une suite inattendue échoue ;
#   - INT/TERM : on arrête, on attend nos propres enfants, PUIS on nettoie ;
#   - on ne supprime que le worktree que CETTE campagne a créé ;
#   - aucun chemin $HOME codé en dur.
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
[ -n "$REPO" ] && [ -n "$REV" ] && [ -n "$ART" ] || {
  echo "usage: run.sh --repo <clone> --rev <sha> --artifacts <dir> [--forced]" >&2; exit 2; }

# --- dépôt reconnu par git, pas par une supposition sur .git ---
TOP=$(git -C "$REPO" rev-parse --show-toplevel 2>/dev/null) || {
  echo "--repo n'est pas un dépôt git : $REPO" >&2; exit 2; }
REPO=$(cd "$TOP" && pwd) || exit 2
command -v cargo >/dev/null 2>&1 || { echo "cargo absent du PATH" >&2; exit 2; }
FULL=$(git -C "$REPO" rev-parse --verify "$REV^{commit}" 2>/dev/null) || {
  echo "révision inconnue : $REV" >&2; exit 2; }

# --- répertoire d'artefacts EXCLUSIF : collision refusée ---
if [ -e "$ART" ]; then
  echo "artefacts déjà présents : $ART (une campagne ne réutilise pas le répertoire d'une autre)" >&2
  exit 3
fi
mkdir -p "$ART" || exit 3
ART=$(cd "$ART" && pwd) || exit 3

WT="$ART/wt"; GEN="$ART/generated"
CREATED_WT=0
ENFANTS=""
INTERROMPU=0

arret() { INTERROMPU=1; echo "interruption demandée — arrêt propre" >&2; }
trap 'arret' INT TERM
trap 'nettoyer' EXIT

nettoyer() {
  # on attend NOS enfants avant de toucher au worktree qu'on a créé
  for p in $ENFANTS; do wait "$p" 2>/dev/null; done
  if [ "$CREATED_WT" -eq 1 ]; then
    git -C "$REPO" worktree remove --force "$WT" >/dev/null 2>&1
  fi
}

git -C "$REPO" worktree add --detach "$WT" "$FULL" >> "$ART/contexte.txt" 2>&1 || {
  echo "worktree impossible" >&2; exit 4; }
CREATED_WT=1
mkdir -p "$GEN"

{
  echo "rev=$FULL"
  git -C "$REPO" log -1 --format='%H %ci %s' "$FULL"
  echo "artefacts=$ART"
  echo "forced=$FORCED"
  echo "rustc=$(rustc --version 2>/dev/null || echo absent)"
  echo "cargo=$(cargo --version 2>/dev/null || echo absent)"
  echo "os=$(uname -srm 2>/dev/null)"
  echo "cargo_target_dir=${CARGO_TARGET_DIR:-<défaut>}"
} > "$ART/contexte.txt"

T="$WT/crates/obrain-engine/tests"
: > "$ART/commands.txt"; : > "$ART/resume.txt"
PASSED=0; FAILED=0; IGNORED=0; FILTERED=0; SANS_TEST=0; COMPIL=0; SUITES_OK=0; SUITES_KO=0

# Suite exécutée : parse passed/failed/ignored/filtered, distingue compilation et 0 test.
lancer() { # $1=suite  $2=features
  suite="$1"; feats="$2"
  cmd="cargo test -p obrain-engine --features $feats --test $suite -- --test-threads 1"
  printf '%s\n' "$cmd" >> "$ART/commands.txt"
  log="$ART/$suite.log"
  ( cd "$WT" && $cmd ) > "$log" 2>&1 &
  pid=$!
  ENFANTS="$ENFANTS $pid"
  wait "$pid"; code=$?
  ENFANTS=$(echo "$ENFANTS" | sed "s/ $pid//")

  ligne=$(grep -E "^test result:" "$log" | tail -1)
  if [ -z "$ligne" ]; then
    if grep -qE "^error(\[|:)" "$log"; then
      COMPIL=$((COMPIL + 1))
      printf 'ERREUR DE COMPILATION %s (voir %s)\n' "$suite" "$log" >> "$ART/resume.txt"
    else
      SANS_TEST=$((SANS_TEST + 1))
      printf 'AUCUN TEST EXÉCUTÉ %s (voir %s)\n' "$suite" "$log" >> "$ART/resume.txt"
    fi
    SUITES_KO=$((SUITES_KO + 1)); return 1
  fi
  p=$(printf '%s' "$ligne" | sed -n 's/.* \([0-9][0-9]*\) passed.*/\1/p'); p=${p:-0}
  f=$(printf '%s' "$ligne" | sed -n 's/.* \([0-9][0-9]*\) failed.*/\1/p'); f=${f:-0}
  i=$(printf '%s' "$ligne" | sed -n 's/.* \([0-9][0-9]*\) ignored.*/\1/p'); i=${i:-0}
  x=$(printf '%s' "$ligne" | sed -n 's/.* \([0-9][0-9]*\) filtered out.*/\1/p'); x=${x:-0}
  PASSED=$((PASSED+p)); FAILED=$((FAILED+f)); IGNORED=$((IGNORED+i)); FILTERED=$((FILTERED+x))
  if [ "$p" -eq 0 ] && [ "$f" -eq 0 ]; then
    SANS_TEST=$((SANS_TEST + 1))
    printf 'AUCUN TEST EXÉCUTÉ %s (%s)\n' "$suite" "$ligne" >> "$ART/resume.txt"
    SUITES_KO=$((SUITES_KO + 1)); return 1
  fi
  if [ "$f" -gt 0 ] || [ "$code" -ne 0 ]; then
    printf 'ÉCHEC %s : %s\n' "$suite" "$ligne" >> "$ART/resume.txt"
    SUITES_KO=$((SUITES_KO + 1)); return 1
  fi
  printf 'ok %s : %s\n' "$suite" "$ligne" >> "$ART/resume.txt"
  SUITES_OK=$((SUITES_OK + 1))
  return 0
}

deriver() { # $1=suite ; vérifie le nombre d'ouvertures RÉELLEMENT réécrites
  src="$T/$1.rs"; dst="$T/${1}_observee.rs"
  [ -f "$src" ] || { echo "suite absente : $1" >&2; return 1; }
  n=$(grep -cE 'ObrainDB::(open\(|new_in_memory\(\))' "$src" 2>/dev/null); n=${n:-0}
  sed -e 's/ObrainDB::open(/ObrainDB::open_substrate_observed(/g' \
      -e 's/ObrainDB::new_in_memory()/ObrainDB::new_in_memory_observed()/g' "$src" > "$dst"
  m=$(grep -cE 'ObrainDB::(open_substrate_observed\(|new_in_memory_observed\(\))' "$dst" 2>/dev/null); m=${m:-0}
  printf '%s\n' "# $1 : $n ouvertures attendues, $m réécrites" > "$GEN/${1}_observee.diff"
  diff -u "$src" "$dst" >> "$GEN/${1}_observee.diff" 2>&1
  [ "$n" -gt 0 ] && [ "$n" -eq "$m" ] || {
    echo "TRANSFORMATION INCOMPLÈTE sur $1 ($n attendues, $m réécrites)" >&2; return 1; }
  return 0
}

echo "== révision $FULL ==" >> "$ART/resume.txt"
for s in durabilite_commit_kill9:wal ecriture_multi_processus:wal wal_recovery:cypher \
         vector_dimension_native:vector-index vector_coherence_ecritures:vector-index \
         durabilite_cognitive_observee_kill9:cognitive cognitive_wiring_test:cognitive; do
  [ "$INTERROMPU" -eq 1 ] && break
  lancer "${s%%:*}" "${s##*:}" || :
done

if [ "$FORCED" -eq 1 ] && [ "$INTERROMPU" -eq 0 ]; then
  echo "== variantes observées forcées ==" >> "$ART/resume.txt"
  for s in durabilite_commit_kill9:wal ecriture_multi_processus:wal wal_recovery:cypher \
           vector_dimension_native:vector-index vector_coherence_ecritures:vector-index; do
    [ "$INTERROMPU" -eq 1 ] && break
    suite=${s%%:*}; feat=${s##*:}
    if deriver "$suite"; then
      lancer "${suite}_observee" "$feat,cognitive" || :
    else
      SUITES_KO=$((SUITES_KO + 1))
    fi
  done
fi

{
  echo "== bilan =="
  echo "passed=$PASSED failed=$FAILED ignored=$IGNORED filtered_out=$FILTERED"
  echo "suites ok=$SUITES_OK ko=$SUITES_KO sans_test=$SANS_TEST erreur_compilation=$COMPIL"
  echo "interrompu=$INTERROMPU"
} >> "$ART/resume.txt"

[ "$INTERROMPU" -eq 1 ] && exit 130
[ "$SUITES_KO" -eq 0 ] && [ "$SANS_TEST" -eq 0 ] && [ "$COMPIL" -eq 0 ] || exit 1
exit 0
