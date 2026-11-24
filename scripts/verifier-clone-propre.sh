#!/usr/bin/env bash
# Vérifie le « clone propre » de l'issue #289 : ce dépôt se résout et se compile
# SANS aucun checkout frère. obrain, this-vite et wami sont des dépendances git
# épinglées (Cargo.toml), récupérées par cargo — plus aucun `path = "../../…"`.
#
# Le test exporte l'arbre COMMITTÉ (`git archive HEAD`, donc sans `.git` ni
# fichiers non suivis — c'est exactement le contenu d'un clone propre) vers un
# répertoire temporaire profondément imbriqué, dont les voisins `../..`
# n'existent pas, puis exécute :
#
#   1. cargo metadata --no-deps                      → résolution sans chemin externe
#   2. cargo check -p this-rs --features frontend    → this-vite (git+rev) compile
#   3. (avec --obrain) cargo check -p this-rs --features obrain   → les six obrain-* compilent
#
# Réseau requis : cargo clone les dépôts publics obrain-db/obrain, this-rs/vite-rs
# et Lsh0x/wami depuis GitHub. Aucun chemin personnel, aucun secret. Aucune version
# flottante : les trois sont épinglés par `rev` (SHA complet).
#
# Usage :
#   scripts/verifier-clone-propre.sh           # metadata + frontend (rapide)
#   scripts/verifier-clone-propre.sh --obrain  # + compile de la feature obrain (lourd)

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

WITH_OBRAIN=0
case "${1:-}" in
  "") ;;
  --obrain) WITH_OBRAIN=1 ;;
  *) echo "usage: $0 [--obrain]" >&2; exit 2 ;;
esac

if ! git -C "$REPO_ROOT" rev-parse --git-dir >/dev/null 2>&1; then
  echo "erreur : $REPO_ROOT n'est pas un dépôt git" >&2
  exit 2
fi

# Répertoire temporaire dont AUCUN voisin (../..) n'existe : la profondeur est la
# garantie du test — cf. le piège du symlink /tmp/obrain décrit dans
# docs/publication-et-disposition.md (une mesure qui se résout sur un symlink
# préexistant est un faux positif partiel).
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT
DEST="$TMP/a/b/c/this"
mkdir -p "$DEST"

git -C "$REPO_ROOT" archive HEAD | tar -x -C "$DEST"
cd "$DEST"

echo "== 1/$( [ "$WITH_OBRAIN" = 1 ] && echo 3 || echo 2 ) — résolution (cargo metadata --no-deps) =="
cargo metadata --no-deps --format-version 1 > /dev/null
echo "OK : manifeste résolu sans checkout frère."

echo "== 2/$( [ "$WITH_OBRAIN" = 1 ] && echo 3 || echo 2 ) — feature frontend (this-vite git+rev) =="
cargo check -p this-rs --features frontend
echo "OK : this-rs --features frontend compile."

if [ "$WITH_OBRAIN" = 1 ]; then
  echo "== 3/3 — feature obrain (obrain-* git+rev) =="
  cargo check -p this-rs --features obrain
  echo "OK : this-rs --features obrain compile."
fi

echo
echo "Clone propre vérifié : aucune dépendance de chemin vers un dépôt voisin."
