#!/bin/bash
# Prépare le projet Xcode du banc et le rend capable d'exercer le matériel.
#
# Trois choses que le gabarit Tauri n'écrit pas, et sans lesquelles l'appareil se tait ou
# l'app ne lance pas. Chacune a été mesurée, pas supposée :
#
#  1. `ALWAYS_EMBED_SWIFT_STANDARD_LIBRARIES` — sans elle, le binaire embarque
#     `libswift_Concurrency.dylib` (concurrence rétro-déployée, active sous iOS 15) sans
#     `libswiftCore.dylib`, et dyld tue l'app au lancement :
#       Library not loaded: @rpath/libswiftCore.dylib … (not in dyld cache)
#  2. `IPHONEOS_DEPLOYMENT_TARGET` — les objets Rust sont compilés pour iOS 15.0. Un hôte qui
#     annonce 14.0 se lie en 14.0 et le lieur le dit.
#  3. Les **déclarations d'usage** (`NS*UsageDescription`) et les **habilitations** — le
#     gabarit n'en écrit AUCUNE (0 sur 18). Sans elles, iOS ne peut pas poser la question :
#     la caméra, la localisation, le Bluetooth, Face ID, la santé et le NFC répondent alors
#     « refusée » ou « indisponible » alors que rien n'a jamais été demandé. C'est le piège
#     qui fait accuser le pont à la place du manifeste.
#
# `cargo tauri ios build` ne régénère PAS le projet depuis `project.yml` : un réglage ajouté
# après `ios init` ne prend effet qu'après `xcodegen generate`. D'où ce script, et d'où le
# fait qu'il soit IDEMPOTENT : `reglages-appareil.py` remplace les blocs au lieu d'y ajouter
# des lignes, donc deux exécutions donnent le même fichier.
set -euo pipefail
cd "$(dirname "$0")"

# `xcodegen` s'arrête sur « Couldn't find current username » quand il ne peut pas résoudre
# l'utilisateur (shell outillé, HOME isolé) — et il sort alors en 1 en laissant le projet à
# moitié écrit. On lui donne l'identité qu'il cherche.
: "${USER:=$(id -un)}"
: "${LOGNAME:=$USER}"
export USER LOGNAME

cargo tauri ios init --ci
python3 reglages-appareil.py "$@"
xcodegen generate --spec gen/apple/project.yml

# Preuve, pas promesse : on relit ce que Xcode a réellement reçu.
echo "--- contrôle du projet généré ---"
DEMANDE_HABILITATIONS=0
for arg in "$@"; do [ "$arg" = "--habilitations" ] && DEMANDE_HABILITATIONS=1; done
export DEMANDE_HABILITATIONS
python3 - <<'PY'
import os
import plistlib
import pathlib
import sys

info = pathlib.Path("gen/apple/banc-appareil_iOS/Info.plist")
if info.exists():
    with info.open("rb") as f:
        cles = plistlib.load(f)
    usages = sorted(k for k in cles if k.endswith("UsageDescription"))
else:
    usages = []
    print("Info.plist pas encore généré (normal avant `cargo tauri ios build`)")

hab = pathlib.Path("gen/apple/banc-appareil_iOS/banc-appareil_iOS.entitlements")
droits = sorted(plistlib.load(hab.open("rb"))) if hab.exists() else []

print("déclarations d'usage : %d" % len(usages))
print("habilitations        : %d (%s)" % (len(droits), ", ".join(droits) or "aucune"))
if len(usages) < 16:
    sys.exit("ÉCHEC : %d déclarations d'usage, 16 attendues" % len(usages))
if os.environ.get("DEMANDE_HABILITATIONS") == "1" and len(droits) < 2:
    sys.exit("ÉCHEC : habilitations demandées mais %d reçue(s)" % len(droits))
print("projet généré : conforme")
PY
