#!/bin/sh
# À lancer DANS la copie de build sur la machine cible : le Cargo.toml racine déclare des
# dépendances optionnelles (feature `obrain`) par chemin hors dépôt, que cargo charge même
# si la feature est éteinte. La forge n'en a pas besoin : on les retire de la COPIE.
set -e
cd "${1:-.}"
sed -i -E '/^obrain-(engine|common|core|substrate|adapters|reactive) = /d' Cargo.toml
sed -i -E '/^this-vite = /d' Cargo.toml
sed -i -E 's/^frontend = \[.*\]$/frontend = []/' Cargo.toml
sed -i -E 's/^obrain = \[.*\]$/obrain = []/' Cargo.toml
