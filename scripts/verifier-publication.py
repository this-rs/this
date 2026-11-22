#!/usr/bin/env python3
"""Les prérequis de paquetage du workspace, vérifiés sans réseau.

Deux choses, parce qu'elles se perdent à la main :

1. **La règle de cargo** : un paquet publiable ne peut pas dépendre d'un crate par
   `path` SANS `version`. `cargo package` refuse alors avec « all dependencies
   must have a version specified when packaging ». Cette règle ne se voit pas en
   développement (`path` gagne sur `version`), donc elle ne casse qu'au moment de
   publier — le pire moment pour la découvrir.
2. **L'ordre de publication**, calculé depuis le graphe des dépendances de
   workspace. L'écrire à la main dans un document le fait vieillir : un crate
   ajouté entre deux ne s'y trouvera pas.

Lecture seule : `cargo metadata --no-deps` (hors réseau), aucune écriture.

Usage :
    scripts/verifier-publication.py            # vérifie, sort 1 si violation
    scripts/verifier-publication.py --ordre    # imprime l'ordre calculé
"""

import json
import subprocess
import sys

# `publish = false` retire un paquet de la publication ; absent veut dire publiable.
def publiable(p):
    publish = p.get("publish")
    return publish is None or (isinstance(publish, list) and len(publish) > 0)


def main():
    sortie = subprocess.run(
        ["cargo", "metadata", "--no-deps", "--format-version", "1"],
        capture_output=True,
        text=True,
        check=True,
    )
    meta = json.loads(sortie.stdout)
    paquets = {p["name"]: p for p in meta["packages"]}
    membres = set(meta["workspace_members"])

    viol = []
    for p in meta["packages"]:
        if p["id"] not in membres or not publiable(p):
            continue
        for d in p["dependencies"]:
            if d.get("path") and d.get("req") in ("*", ""):
                viol.append((p["name"], d["name"], d["kind"] or "normal"))

    # Ordre : un crate doit être publié après ceux qu'il dépend dans le workspace.
    # Tri topologique sur les dépendances de workspace (toutes sortes).
    deps = {}
    for p in meta["packages"]:
        if p["id"] not in membres:
            continue
        deps[p["name"]] = sorted(
            {d["name"] for d in p["dependencies"] if d["name"] in paquets}
        )

    ordre, vus, en_cours = [], set(), set()

    def visite(n):
        if n in vus or n in en_cours:  # cycle : on ne boucle pas, cargo le refusera
            return
        en_cours.add(n)
        for d in deps.get(n, []):
            visite(d)
        en_cours.discard(n)
        vus.add(n)
        ordre.append(n)

    for n in sorted(deps):
        visite(n)

    if "--ordre" in sys.argv:
        for i, n in enumerate(ordre, 1):
            print(f"{i:2d}. {n}")
        return 0

    if viol:
        print("DÉPENDANCES `path` SANS `version` DANS UN PAQUET PUBLIABLE :", file=sys.stderr)
        for paquet, dep, genre in sorted(viol):
            print(f"  {paquet} -> {dep}  [{genre}]", file=sys.stderr)
        print(
            "\n`cargo package` refuse ces manifestes. Ajouter `version = \"…\"` "
            "(le `path` reste, il gagne en développement).",
            file=sys.stderr,
        )
        return 1

    print(f"OK : aucune dépendance `path` sans version dans les paquets publiables.")
    print("Ordre de publication calculé (voir --ordre) :")
    for i, n in enumerate(ordre, 1):
        print(f"{i:2d}. {n}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
