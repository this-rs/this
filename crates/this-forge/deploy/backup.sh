#!/bin/sh
# Sauvegarde de la forge : base LMDB + clés (+ artefacts, sources). Arrête le service le
# temps de la copie (LMDB : une copie à chaud peut être incohérente). ~1 s.
# Usage : ~/forge/backup.sh [dossier de destination, défaut ~/forge/backups]
set -e
DATA="$HOME/forge/data"
DEST="${1:-$HOME/forge/backups}"
mkdir -p "$DEST"; chmod 700 "$DEST"
OUT="$DEST/forge-$(date +%Y%m%d-%H%M%S).tar.gz"
systemctl --user stop this-forge
trap 'systemctl --user start this-forge' EXIT
( umask 077; tar -C "$DATA" -czf "$OUT" . )
chmod 600 "$OUT"
echo "$OUT"
