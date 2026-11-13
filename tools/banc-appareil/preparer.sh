#!/bin/bash
# Prépare le projet Xcode du banc : gabarit Tauri, puis les trois réglages que le gabarit
# n'écrit pas et sans lesquels un iPhone réel refuse de lancer l'app.
#
# Mesuré le 13/11/2026 sur l'iPhone 16 Pro Max : sans `ALWAYS_EMBED_SWIFT_STANDARD_LIBRARIES`,
# le binaire embarque `libswift_Concurrency.dylib` (concurrence rétro-déployée, active sous
# iOS 15) sans `libswiftCore.dylib`, et dyld tue l'app au lancement :
#   Library not loaded: @rpath/libswiftCore.dylib … (not in dyld cache)
#
# `cargo tauri ios build` ne régénère PAS le projet depuis `project.yml` : un réglage ajouté
# après `ios init` ne prend effet qu'après `xcodegen generate`. D'où ce script.
set -euo pipefail
cd "$(dirname "$0")"

cargo tauri ios init --ci

python3 - <<'PY'
import pathlib
p = pathlib.Path("gen/apple/project.yml")
t = p.read_text()
needle = "    settings:\n      base:\n"
if "ALWAYS_EMBED_SWIFT_STANDARD_LIBRARIES" not in t:
    assert needle in t, "gabarit inattendu : chercher 'settings: base:'"
    t = t.replace(needle, needle + """        # Le runtime Swift se COPIE dans l'app (voir preparer.sh).
        ALWAYS_EMBED_SWIFT_STANDARD_LIBRARIES: true
        # La borne basse du crate : les objets Rust sont compilés pour iOS 15.0.
        IPHONEOS_DEPLOYMENT_TARGET: "15.0"
        ARCHS: [arm64]
        ENABLE_BITCODE: false
""", 1)
    p.write_text(t)
    print("project.yml : trois réglages ajoutés")
else:
    print("project.yml : déjà réglé")
PY

xcodegen generate --spec gen/apple/project.yml
