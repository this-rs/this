#!/usr/bin/env python3
"""Réglages que le gabarit Tauri n'écrit pas, et sans lesquels le banc ment.

Le banc appareil exerce les capacités matérielles. Un iPhone refuse une capacité pour deux
raisons distinctes, et les confondre fait accuser le pont :

  1. **Pas de déclaration d'usage** (`NS*UsageDescription`) : le système ne PEUT pas poser la
     question, donc il n'y a jamais de demande, et la capacité répond « refusée » ou
     « indisponible » alors que rien n'a été demandé. Mesuré le 13/11/2026 : la caméra, la
     localisation, le Bluetooth, le mouvement, Face ID, la santé, le NFC, le réseau local, les
     contacts, le calendrier, les photos et la parole étaient tous dans ce cas — le gabarit
     généré par `cargo tauri ios init` ne contient AUCUNE clé d'usage (0 sur 18).
  2. **Pas d'habilitation** (`*.entitlements`) : la clé d'usage fait poser la question, mais
     HealthKit et le NFC exigent en plus une habilitation signée. Sans elle la lecture
     reste indisponible et le NFC répond `unsupported`, même permission accordée.

Ce script applique les deux au projet généré, de façon idempotente : il remplace le bloc
`properties:` au lieu d'y ajouter des lignes — deux exécutions donnent le même fichier.

Les déclarations d'usage sont toujours appliquées : elles ne cassent jamais la signature.
Les **habilitations**, non : un profil *wildcard* ne peut pas porter HealthKit ni le NFC, et
les activer par défaut ferait échouer la signature au lieu de réparer quoi que ce soit
(mesuré sur l'Atelier le 08/11/2026). Elles sont donc derrière `--habilitations`, à n'ouvrir
qu'après avoir créé l'App ID explicite dans le portail Apple avec les deux capacités cochées.
"""

import pathlib
import sys

PROJET = pathlib.Path("gen/apple/project.yml")
HABILITATIONS = pathlib.Path("gen/apple/banc-appareil_iOS/banc-appareil_iOS.entitlements")

# Une phrase par clé, et la phrase est ce que la personne lit dans la boîte de dialogue du
# système. Elle doit dire ce que le banc fait, pas flatter l'utilisateur.
USAGES = {
    "NSBluetoothAlwaysUsageDescription": "Le banc relève les appareils Bluetooth à portée pour vérifier le pont.",
    "NSCalendarsFullAccessUsageDescription": "Le banc lit les calendriers pour vérifier la lecture d'événements.",
    "NSCalendarsUsageDescription": "Le banc lit les calendriers pour vérifier la lecture d'événements.",
    "NSCameraUsageDescription": "Le banc prend une photo pour vérifier la capture.",
    "NSContactsUsageDescription": "Le banc lit les contacts pour vérifier la lecture du carnet.",
    "NSFaceIDUsageDescription": "Le banc demande une confirmation Face ID pour vérifier le pont.",
    "NSHealthShareUsageDescription": "Le banc lit le nombre de pas pour vérifier la lecture HealthKit.",
    "NSLocalNetworkUsageDescription": "Le banc découvre les services du réseau local pour vérifier le pont.",
    "NSLocationAlwaysAndWhenInUseUsageDescription": "Le banc relève une position pour vérifier la localisation.",
    "NSLocationWhenInUseUsageDescription": "Le banc relève une position pendant son usage, pour vérifier la localisation.",
    "NSMicrophoneUsageDescription": "Le banc enregistre un court échantillon pour vérifier le micro.",
    "NSMotionUsageDescription": "Le banc relève l'accéléromètre et le gyroscope pour vérifier le mouvement.",
    "NSPhotoLibraryUsageDescription": "Le banc lit la photothèque pour vérifier la sélection.",
    "NSPhotoLibraryAddUsageDescription": "Le banc écrit une photo d'essai pour vérifier l'enregistrement.",
    "NSSpeechRecognitionUsageDescription": "Le banc transcrit la parole pour vérifier la reconnaissance vocale.",
    "NFCReaderUsageDescription": "Le banc lit une étiquette NFC pour vérifier le pont.",
}

# Une liste de services Bonjour vide empêche la découverte : sans elle, le réseau local
# répond « refusé » même accordé.
SERVICES_BONJOUR = ["_this-device._tcp", "_http._tcp"]


def scalaire(valeur):
    """Rend une valeur YAML sûre pour ce fichier (booléens et chaînes uniquement)."""
    if isinstance(valeur, bool):
        return "true" if valeur else "false"
    texte = str(valeur)
    return '"%s"' % texte if any(c in texte for c in ':#"') else texte


def reglages_de_base():
    """Les réglages de compilation, écrits une seule fois chacun."""
    return {
        "ALWAYS_EMBED_SWIFT_STANDARD_LIBRARIES": True,
        "IPHONEOS_DEPLOYMENT_TARGET": "15.0",
        "ARCHS": "[arm64]",
        "ENABLE_BITCODE": False,
    }


def lire_bloc(lignes, debut, indent):
    """Lit le bloc YAML indenté qui suit `debut`.

    Rend (groupes, fin) : chaque groupe est une entrée du bloc — une clé avec ses lignes
    filles (une liste imbriquée, par exemple), ou une ligne de commentaire. On garde les
    groupes au lieu de réécrire le bloc entier, parce que le gabarit porte des clés et des
    commentaires qui doivent survivre (CFBundle*, orientations, UILaunchStoryboardName).
    """
    prefixe = " " * indent
    groupes, fin, courant = [], debut + 1, []
    while fin < len(lignes):
        ligne = lignes[fin]
        if not ligne.strip() or not ligne.startswith(prefixe):
            break
        profondeur = len(ligne) - len(ligne.lstrip())
        if profondeur == indent:
            if courant:
                groupes.append(courant)
            courant = [ligne]
        else:
            courant.append(ligne)
        fin += 1
    if courant:
        groupes.append(courant)
    return groupes, fin


def cle_de(groupe):
    """La clé d'un groupe, ou None pour un commentaire."""
    tete = groupe[0].strip()
    if tete.startswith("#") or ":" not in tete:
        return None
    return tete.split(":", 1)[0]


def fusionner(lignes, index_bloc, indent, voulues):
    """Fusionne `voulues` (liste ordonnée de groupes) dans le bloc existant.

    Une clé déjà présente est REMPLACÉE sur place : c'est ce qui rend le script idempotent
    et supprime les doublons qu'un patch successif a laissés. Les clés absentes sont
    ajoutées, les clés et commentaires étrangers sont conservés tels quels.
    """
    groupes, fin = lire_bloc(lignes, index_bloc, indent)
    voulues_par_cle = {cle_de(g): g for g in voulues if cle_de(g)}
    vues, resultat = set(), []
    for groupe in groupes:
        cle = cle_de(groupe)
        if cle is None:
            resultat.append(groupe)
        elif cle in voulues_par_cle:
            if cle not in vues:
                resultat.append(voulues_par_cle[cle])
                vues.add(cle)
            # doublon : abandonné, c'est le but
        else:
            resultat.append(groupe)
    ajoutees = 0
    for groupe in voulues:
        cle = cle_de(groupe)
        if cle and cle not in vues:
            resultat.append(groupe)
            ajoutees += 1
    aplati = [l for groupe in resultat for l in groupe]
    return lignes[:index_bloc + 1] + aplati + lignes[fin:], len(groupes), ajoutees


IND = 8


def patch_project_yml():
    lignes = PROJET.read_text().split("\n")

    # 1. Le bloc `settings: base:`. On fusionne, et on ajoute aux clés déjà là les quatre
    # réglages que le gabarit n'écrit pas.
    reglages = [
        ["        # Le runtime Swift se COPIE dans l'app (voir preparer.sh)."],
        ["        # Sans cette ligne, dyld tue l'app : Library not loaded: @rpath/libswiftCore.dylib"],
        ["        ALWAYS_EMBED_SWIFT_STANDARD_LIBRARIES: true"],
        ["        # La borne basse du crate : les objets Rust sont compilés pour iOS 15.0."],
        ['        IPHONEOS_DEPLOYMENT_TARGET: "15.0"'],
        ["        ARCHS: [arm64]"],
        ["        ENABLE_BITCODE: false"],
    ]
    for i, ligne in enumerate(lignes):
        if ligne.strip() == "base:" and "settings" in lignes[i - 1]:
            lignes, avant, ajoutees = fusionner(lignes, i, IND, reglages)
            print("settings: base -> %d entrée(s) lue(s), %d ajoutée(s)" % (avant, ajoutees))
            break
    else:
        sys.exit("gabarit inattendu : bloc 'settings: base:' introuvable")

    # 2. Le bloc `properties:` de la cible. C'est LUI qui produit l'Info.plist : une clé
    # absente d'ici n'existe pas pour iOS, donc la capacité ne peut jamais poser la question.
    proprietes = [
        ["        # Déclarations d'usage : sans elles, iOS ne peut pas poser la question,"],
        ["        # donc la capacité répond « refusée » alors que rien n'a été demandé."],
    ]
    for cle, phrase in sorted(USAGES.items()):
        proprietes.append(['        %s: "%s"' % (cle, phrase)])
    # Sans liste de services, la découverte Bonjour échoue même accordée : les deux entrées
    # sont celles que le pont annonce réellement.
    proprietes.append(["        NSBonjourServices:"] + ['          - "%s"' % s for s in SERVICES_BONJOUR])
    proprietes.append(["        NSSupportsLiveActivities: true"])
    for i, ligne in enumerate(lignes):
        if ligne.strip() == "properties:":
            lignes, avant, ajoutees = fusionner(lignes, i, IND, proprietes)
            print("properties -> %d entrée(s) lue(s), %d clé(s) d'usage ajoutée(s)"
                  % (avant, len(USAGES)))
            break
    else:
        sys.exit("gabarit inattendu : bloc 'properties:' introuvable")

    texte = "\n".join(lignes)
    texte = texte.replace("deploymentTarget:\n    iOS: 14.0", 'deploymentTarget:\n    iOS: "15.0"')
    PROJET.write_text(texte)


def patch_habilitations(lignes):
    """Déclare les habilitations DANS le gabarit, pas dans un fichier en aval.

    `xcodegen` régénère le fichier `.entitlements` depuis `project.yml` : écrire le fichier
    en aval est inutile, il est écrasé (mesuré : `entitlements: []` après génération).

    HealthKit et le NFC ne s'ouvrent pas avec une phrase d'usage : il faut une habilitation
    SIGNÉE. Mais une habilitation ne peut pas vivre sur un profil *wildcard* : mesuré le
    08/11/2026 sur l'Atelier, la signature échoue —
      Provisioning profile "iOS Team Provisioning Profile: *" doesn't include the HealthKit
      capability
    — donc activer ces deux capacités par défaut CASSE le build au lieu de le réparer.

    D'où l'opt-in. Par défaut le script n'écrit que les déclarations d'usage, qui ne cassent
    jamais la signature. `--habilitations` ajoute les deux capacités, à n'activer qu'après
    avoir créé l'App ID explicite dans le portail Apple avec HealthKit et NFC Tag Reading
    cochés. C'est la seule façon d'obtenir `granted` en santé et autre chose que
    `unsupported` en NFC.
    """
    for i, ligne in enumerate(lignes):
        if ligne.strip() == "entitlements:":
            groupe = ["      properties:",
                      "        com.apple.developer.healthkit: true",
                      "        com.apple.developer.healthkit.access: []",
                      "        com.apple.developer.nfc.readersession.formats:",
                      "          - NDEF",
                      "          - TAG"]
            lignes, avant, ajoutees = fusionner(lignes, i, 6, [groupe])
            print("entitlements -> %d entrée(s) lue(s), %d ajoutée(s) (healthkit, nfc)"
                  % (avant, ajoutees))
            return lignes
    sys.exit("gabarit inattendu : bloc 'entitlements:' introuvable")


def main():
    if not PROJET.exists():
        sys.exit("project.yml absent : lancer `cargo tauri ios init --ci` d'abord")
    avec_habilitations = "--habilitations" in sys.argv[1:]
    patch_project_yml()
    if avec_habilitations:
        lignes = PROJET.read_text().split("\n")
        PROJET.write_text("\n".join(patch_habilitations(lignes)))
    else:
        print("entitlements -> non (santé et NFC resteront indisponibles ; "
              "voir --habilitations)" )
    print("réglages appareil appliqués")


if __name__ == "__main__":
    main()
