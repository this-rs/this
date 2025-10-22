#!/bin/bash
# Script pour créer une archive du projet This-RS

set -e

echo "📦 Création de l'archive This-RS..."
echo ""

# Aller au dossier parent
cd "$(dirname "$0")/.."

# Nom de l'archive
ARCHIVE_NAME="this-rs-v0.1.0.tar.gz"

# Créer l'archive
tar -czf "$ARCHIVE_NAME" \
    --exclude='target' \
    --exclude='.git' \
    --exclude='*.tar.gz' \
    this-rs/

# Vérifier la taille
SIZE=$(du -h "$ARCHIVE_NAME" | cut -f1)

echo "✅ Archive créée: $ARCHIVE_NAME"
echo "📊 Taille: $SIZE"
echo ""
echo "Pour extraire sur ta machine:"
echo "  tar -xzf $ARCHIVE_NAME"
echo "  cd this-rs"
echo "  cargo check"
echo ""
echo "Bon développement ! 🚀"
