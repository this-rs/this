#!/usr/bin/env bash
# bootstrap-dev.sh — prépare les checkouts FRÈRES que `this` exige pour ses
# feature optionnelle `obrain` (issue #289).
#
# `this/Cargo.toml` référence le dépôt voisin obrain par `path` :
#     obrain-engine = { path = "../../obrain/obrain-db/crates/obrain-engine", … }
# Il est OPTIONNEL : un build par défaut n'en a pas besoin. Mais `--features obrain`
# échoue si le dossier est absent. Ce script le clone au SHA épinglé, dans la
# disposition attendue :
#     <parent>/obrain/obrain-db/…     (dépôt obrain-db/obrain)
# où <parent> est DEUX niveaux au-dessus de `this/` : le dossier qui CONTIENT
# `this-rs/` (donc `this`) et `obrain/` côte à côte — c'est ce que dit le `../../`
# du manifeste.
#
# vite-rs n'est PLUS un checkout frère : `this-vite` est une dépendance git épinglée
# (Cargo.toml), cargo la récupère seul. Rien à cloner pour la feature `frontend`.
#
# Usage :
#     scripts/bootstrap-dev.sh                 # clone ce qui manque
#     scripts/bootstrap-dev.sh --force         # aligne aussi sur le SHA épinglé
#     OBRAIN_REV=<sha complet> scripts/bootstrap-dev.sh
#
# Ce script ne touche JAMAIS un checkout présent : s'il existe à un autre SHA,
# il le signale (une copie de travail peut être en cours) et ne l'écrase que
# sous --force. Aucun chemin personnel n'est écrit nulle part.
set -euo pipefail

THIS_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PARENT="$(cd "$THIS_DIR/../.." && pwd)"

OBRAIN_REV="${OBRAIN_REV:-f0d6e132666d34d63824a46ad2a5119416b0afb4}"
OBRAIN_URL="${OBRAIN_URL:-https://github.com/obrain-db/obrain.git}"

FORCE=0
for a in "$@"; do
  case "$a" in
    --force) FORCE=1 ;;
    -h|--help) sed -n '2,25p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'; exit 0 ;;
    *) echo "argument inconnu : $a" >&2; exit 2 ;;
  esac
done

# prepare <dossier> <url> <sha> <étiquette>
prepare() {
  local dir="$1" url="$2" rev="$3" nom="$4"
  if [ -d "$dir/.git" ]; then
    local actuel
    actuel="$(git -C "$dir" rev-parse --short HEAD 2>/dev/null || echo '?')"
    if [ "$actuel" = "$(echo "$rev" | cut -c1-7)" ] || git -C "$dir" merge-base --is-ancestor "$rev" HEAD 2>/dev/null; then
      echo "  OK    $nom présent, $actuel (épinglé $rev)"
    elif [ "$FORCE" = 1 ]; then
      echo "  FORCE $nom à $actuel → checkout $rev"
      git -C "$dir" fetch -q --depth 1 origin "$rev" && git -C "$dir" checkout -q FETCH_HEAD
    else
      echo "  NOTE  $nom présent à $actuel, différent de l'épingle $rev — laissé tel quel"
      echo "        (copie de travail possible ; --force pour aligner)"
    fi
    return 0
  fi
  if [ -e "$dir" ]; then
    echo "ERREUR : $dir existe mais n'est pas un dépôt git — à déplacer" >&2
    return 1
  fi
  echo "  CLONE $nom ($rev) → $dir"
  mkdir -p "$dir"
  git init -q "$dir"
  git -C "$dir" remote add origin "$url"
  git -C "$dir" fetch -q --depth 1 origin "$rev"
  git -C "$dir" checkout -q FETCH_HEAD
  echo "  OK    $nom à $(git -C "$dir" rev-parse --short HEAD)"
}

echo "Bootstrap des checkouts frères de this (parent : $PARENT)"
prepare "$PARENT/obrain/obrain-db" "$OBRAIN_URL" "$OBRAIN_REV" "obrain   "
echo
echo "Terminé. Construire avec les features optionnelles :"
echo "  cargo build --features obrain"
echo "Un build par défaut n'a besoin d'aucun de ces dossiers ; la feature frontend n'en a plus besoin non plus."
