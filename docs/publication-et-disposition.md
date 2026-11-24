# Publier, et travailler sur plusieurs dépôts

Mesuré le 03/11/2026 avec `curl -H "User-Agent: …" https://crates.io/api/v1/crates/<crate>`.

## Où en est la publication

| Crate | Sur crates.io | Version locale |
|---|---|---|
| `this-descriptor` | **absent** | 0.0.1 |
| `this-rs` | publié 0.0.9 | 0.0.10 (non publiée) |
| `this-cli` | publié 0.0.4 | 0.0.5 |
| `this-vite` | publié 0.0.1 | 0.0.1 |
| `obrain-engine`, `obrain-core`, `obrain-common`, … | **absents** | 0.0.1 |

## Le blocage, et pourquoi il est unique

`this-cli/Cargo.toml` déclare :

```toml
this-descriptor = { version = "0.0.1", path = "../this/crates/this-descriptor" }
```

C'est déjà la forme correcte : `cargo publish` retire le `path` et ne garde que la `version`. Le `path` sert au développement local, la `version` à la publication. **Rien à changer dans les manifestes.**

Le seul problème est que **`this-descriptor` 0.0.1 n'existe pas sur crates.io**. Tant qu'il n'y est pas, `cargo install this-cli` depuis un checkout isolé échoue à la résolution : la version est introuvable.

`obrain-*` absents est un cas différent : ces crates sont derrière la feature `obrain`, explicitement hors du périmètre publiable pour l'instant (il faudrait les publier eux aussi, avec leurs propres dépendances). La feature `frontend`, elle, s'appuie sur `this-vite`, **déjà publié**.

## Ordre de publication

1. `this-descriptor` (aucune dépendance hors crates.io)
2. `this-rs` (dépend de `this-descriptor` par `version + path`)
3. `this-cli` (dépend de `this-descriptor`)
4. `this-examples` n'est pas publié : c'est un dépôt d'exemples.

Publier dans un autre ordre échoue : cargo refuse une dépendance `version` introuvable.

## La commande qui reste à lancer

```sh
cd this/crates/this-descriptor
cargo publish --dry-run   # vérifie l'empaquetage sans rien envoyer
cargo publish             # irréversible : seule une version yanked peut être retirée
```

`cargo publish` est **irréversible**. Il demande un jeton crates.io (`CARGO_REGISTRY_TOKEN`) que ce dépôt ne stocke pas. C'est donc une action humaine, à faire une fois.

### Le paquet est prêt (mesuré le 03/11/2026)

`cargo publish --dry-run` sur `crates/this-descriptor` va au bout :

```
Packaged 34 files, 409.5KiB (102.8KiB compressed)
Verifying this-descriptor v0.0.1
   Compiling this-descriptor v0.0.1 (.../slot2/package/this-descriptor-0.0.1)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 4.09s
   Uploading this-descriptor v0.0.1
warning: aborting upload due to dry run
```

La ligne qui compte est la recompilation dans `slot2/package/this-descriptor-0.0.1` : cargo a construit le paquet **tel qu'il serait publié**, hors de l'arborescence du dépôt. Le `--dry-run` s'arrête juste avant l'envoi. Il ne reste donc que l'upload, et il n'y a rien à corriger d'ici là.

## Travailler sur les trois dépôts

Les manifestes se résolvent par chemin relatif. La disposition attendue sur le disque :

```
<parent>/
├── this/              # framework + crates/this-descriptor
├── this-cli/          # path = ../this/crates/this-descriptor
└── this-examples/     # path = ../this
```

La feature optionnelle `obrain` ajoute un voisin :

```
<parent>/../obrain/obrain-db/   # obrain-engine, -core, -common, -substrate, -adapters, -reactive
```

La feature `frontend` n'en ajoute **plus** : `this-vite` est une dépendance git épinglée
(`this-rs/vite-rs`, dépôt public), récupérée par cargo comme n'importe quel crate.

La CI compense cette disposition dans chaque job (`this-cli/.github/workflows/ci.yml` place `this-rs/this` à `../this` ; `this-examples` utilise l'action composite `./.github/actions/this-dep`). Ce document est la version lisible de cette connaissance, qui n'existait que dans les workflows.

## Clone propre : ce qui marche, ce qui casse (mesuré le 20/11/2026)

Mesuré depuis un clone neuf placé dans un répertoire dont les voisins `../..`
**n'existent pas** (ni `obrain/`, ni `vite-rs`). Toutes les commandes en premier
plan, chronométrées.

> **Portée de la mesure.** Le clone a été pris sur `main` à `08b0178`. Entre ce
> commit et `origin/main` (`bd387e8`), `Cargo.toml` ne change que les trois
> lignes WAMI : `git diff 08b0178 bd387e8 -- Cargo.toml` ne touche **aucune**
> ligne `path =`. La conclusion sur les dépendances de chemin vaut donc pour la
> pointe. Les versions WAMI, elles, ont été **re-vérifiées sur crates.io** après
> ce changement (tableau plus bas).

### Tableau des commandes

| Commande (dans le clone) | Verdict | Code | Temps | Note |
|---|---|---|---|---|
| `cargo metadata --no-deps` | **échec** | 101 | 0,05 s | manifeste non résolu |
| `cargo metadata --format-version 1` | **échec** | 101 | 0,30 s | échoue avant le registry |
| `cargo check -p this-harness` | **échec** | 101 | 0,22 s | le « cœur portable » n'y échappe pas |
| `bash .github/create-local-dep-stubs.sh` | OK | 0 | 3,1 s | 6 stubs obrain + 1 stub vite + clone wami |
| `cargo metadata --no-deps` (après stubs) | OK | 0 | 0,08 s | 19 membres résolus |
| `cargo check -p this-harness` (après stubs) | OK | 0 | 34 s | |
| `cargo check -p this-rs` (défaut) | OK | 0 | 38 s | in-memory seul |
| `cargo check --workspace` (défaut) | OK | 0 | 1 m 54 s | reprise ; un build concurrent occupait la machine, le temps à froid n'est pas chiffré |
| `cargo check -p this-rs --features obrain` | **échec** | 101 | 2 m 36 s | 98 erreurs : les stubs sont vides d'API |
| `cargo package -p this-harness --list` | OK | 0 | 2,1 s | 41 fichiers |
| `cargo package -p this-harness --no-verify --allow-dirty` | **échec** | 101 | 2,9 s | `no matching package named this-agent found` |

L'erreur du clone nu, telle quelle :

```
error: failed to load manifest for workspace member `…/this/.`
Caused by:
  failed to load manifest for dependency `obrain-adapters`
Caused by:
  failed to read `…/obrain/obrain-db/crates/obrain-adapters/Cargo.toml`
Caused by:
  No such file or directory (os error 2)
```

**Pourquoi.** Cargo lit les manifestes des dépendances `path` **même quand elles
sont `optional = true`** : `optional` décide de ce qui est COMPILÉ, pas de ce qui
est RÉSOLU. D'où le paradoxe mesuré : un build qui ne concerne pas obrain
(`-p this-harness`) échoue à cause d'obrain.

### Prérequis non publiés (crates.io, re-vérifié le 20/11/2026)

| Dépendance | Déclarée dans `Cargo.toml` | Sur crates.io | Statut |
|---|---|---|---|
| `this-descriptor` | 0.0.1 | 0.0.1 | publié |
| `this-vite` | 0.0.1 | 0.0.1 | publié |
| `obrain-engine`, `-common`, `-core`, `-substrate`, `-adapters`, `-reactive` | 0.0.1 (path) | absents | à publier (feature `obrain`) |
| `this-agent` | 0.0.1 (path) | absent | à publier |
| `this-llm` | 0.0.1 (path) | absent | à publier |
| `this-harness` | 0.0.1 (path) | absent | à publier |
| `wami` | **0.21** (git `a4e1070`) | 0.18.0 | version manquante |
| `wami-core` | **0.21** (git `a4e1070`) | 0.19.0 | version manquante |
| `wami-condition` | **0.18.3** (git `a4e1070`) | 0.18.0 | version manquante |

`cargo package` échoue sur `this-harness` ainsi :

```
error: failed to prepare local package for uploading
Caused by:
  no matching package named `this-agent` found
  location searched: crates.io index
```

`--no-verify` saute la recompilation mais **pas** le contrôle du registry : la
`version` d'un `path` doit exister sur crates.io pour que le paquet soit
préparable. `this-agent`, puis `this-llm` (deux `path` de `this-harness`), sont
les deux absents qui bloquent ; l'ordre de publication est donc
`this-llm` → `this-agent` → `this-harness`.

### Le vérificateur de prérequis (mesuré le 21/11/2026)

`scripts/verifier-publication.py` (lecture seule, hors réseau : `cargo metadata --no-deps`)
vérifie la règle que cargo applique **au paquetage** et que le développement ne montre
jamais — un paquet publiable ne peut pas dépendre d'un crate par `path` **sans** `version`
(« all dependencies must have a version specified when packaging »). Il calcule aussi
l'ordre de publication depuis le graphe, pour qu'il ne vieillisse pas dans un document.

Il a trouvé **trois** manifestes fautifs, pas un :

| Manifeste | Dépendance | Genre | Effet |
|---|---|---|---|
| `crates/this-harness` | `this-llm-providers` | dev | bloquant une fois la chaîne publiée |
| `crates/this-module-sdk` | `this-descriptor` | dev | **bloquait déjà** (le crate est publié en 0.0.1) |
| `crates/this-app` | `this-platform` | normal | bloquant dès que `this-app` est publié |

Les trois sont corrigés par `version = "0.0.1"` (le `path` reste : il gagne en
développement). Preuve que le correctif débloque quelque chose de **réel** et pas
seulement le vérificateur : `this-module-sdk`, dont toutes les autres dépendances sont
publiées, **paquette maintenant** —

```
Packaged 17 files, 95.5KiB (26.2KiB compressed)
```

alors qu'il échouait sur la règle ci-dessus. `this-harness`, lui, ne bloque plus que sur
`this-agent` absent de crates.io : c'est la chaîne de publication, donc une décision, pas
un défaut de manifeste.

Fermeture exacte de `this-harness` dans le workspace (calculée, pas recopiée) :
`this-agent` et `this-llm` (aucune dépendance de workspace) → `this-llm-providers` (dépend
de `this-llm`) → `this-harness`. Aucun ne dépend de `this-descriptor`, contrairement à ce
que laisserait croire l'ordre à quatre de la section « Ordre de publication », qui concerne
`this-cli`.

### Chemin de build minimal reproductible

```sh
git clone <this> this && cd this
bash .github/create-local-dep-stubs.sh   # stubs obrain + vite, clone wami (réseau, ~75 Mo)
cargo check -p this-harness              # le cœur portable
cargo check --workspace                  # défaut (in-memory)
```

Le script crée exactement six stubs `obrain-*` (un `Cargo.toml` 0.0.1 + un
`src/lib.rs` vide), un stub `this-vite`, et un clone `--depth 1` de `Lsh0x/wami`.

### Publié pour la distribution, `path` pour le développement

- **Distribution** : `this-descriptor 0.0.1` et `this-vite 0.0.1` sont sur
  crates.io ; toutes les autres dépendances viennent du registry. Le manifeste
  les déclare en `version + path`, forme correcte : `cargo publish` retire le
  `path` et garde la `version`.
- **Développement seulement** : les six `obrain-*` (feature `obrain`) et
  `this-vite` (feature `frontend`). Les stubs ne servent qu'à **charger le
  manifeste** ; leur API est vide — mesuré : `cargo check -p this-rs --features
  obrain` rend 98 erreurs (`SubstrateStore`, `GraphStoreMut` introuvables). La CI
  le sait déjà : elle emploie `--features all,wami`, jamais `--all-features`,
  pour ne pas compiler ce chemin sur stubs.
- **`wami`** : `git + rev` en développement, `version` seule à la publication ;
  0.21 / 0.21 / 0.18.3 ne sont pas encore sur crates.io.

### Piège de mesure (il a faussé la première tentative)

`/tmp/obrain` est un **lien symbolique préexistant** vers le checkout obrain. Un
clone posé à `/tmp/<x>/this` voit donc `../../obrain` se résoudre et échoue sur
`this-vite` au lieu d'`obrain-adapters` : les six dépendances obrain passent sans
que rien ne le signale. **Toute validation de clone propre doit se faire dans un
sous-répertoire dont les voisins n'existent pas**, sinon elle mesure un faux
positif partiel.

### Manques restants (non qualifiés)

- Le build **réel** de `obrain` et `frontend` avec les vrais checkouts : seule la
  rupture sur stubs vides est mesurée.
- Le build `--features wami` (fetch git) : non mesuré.
- `cargo check --workspace` **à froid** : non chiffré de façon fiable.
- ~~La dev-dependency `this-llm-providers` de `this-harness` est déclarée en `path`
  **sans** `version`~~ — **corrigé le 21/11/2026**, avec **deux** autres du même genre
  que la mesure de cette page n'avait pas vues (voir « Le vérificateur de prérequis »
  ci-dessus).
- Matrice OS/architecture, artefacts, sommes de contrôle, installation,
  démarrage, mise à jour et retour arrière : rien de tout cela n'est mesuré ici.

## Disposition des dossiers : les checkouts frères (bootstrap)

`this/Cargo.toml` référence un dépôt **voisin** par `path`, pour la feature optionnelle `obrain` :

| Feature | Dépendances | Où |
|---|---|---|
| `obrain` | `obrain-engine`, `obrain-core`, `obrain-substrate`, … | `../../obrain/obrain-db/crates/…` (checkout frère) |
| `frontend` | `this-vite` | dépendance **git épinglée** (`this-rs/vite-rs`), aucun checkout |

Un build **par défaut** n'a besoin d'aucun voisin. `--features obrain` échoue si le dossier d'obrain manque — mesuré le 21/11 : `cargo metadata --features obrain` depuis un clone sans voisins rend **code 101**. `--features frontend` n'a plus ce défaut depuis que `this-vite` est épinglé par `git` + `rev` (même motif que WAMI : le hash fixe le code, la `version` est celle résolue à la publication).

Pour travailler sur vite-rs **en local** sans toucher le manifeste, dans `.cargo/config.toml` :

```toml
[patch."https://github.com/this-rs/vite-rs.git"]
this-vite = { path = "../../vite-rs" }
```

**`scripts/bootstrap-dev.sh`** clone `obrain` au SHA épinglé, dans la disposition attendue, à côté de `this/` :

- `obrain` → `../obrain/obrain-db/` (dépôt `obrain-db/obrain`).

Il est **idempotent**, ne touche **jamais** un checkout présent à un autre SHA (une copie de travail peut être en cours — `--force` pour aligner), et n'écrit **aucun chemin personnel**. Les épingles sont des **SHA complets** (un SHA abrégé échoue au `git fetch --depth 1`), surchargeables par `OBRAIN_REV` :

- obrain `f0d6e132666d34d63824a46ad2a5119416b0afb4`

`vite-rs` n'est plus cloné : son SHA `dcf233cd154ed7416338c9141736ff9efa19f199` est épinglé dans `Cargo.toml` (`rev`).

Cela couvre le premier critère de #289 (« build depuis un répertoire temporaire sans checkouts frères, par un bootstrap documenté et SHAs épinglés »). La matrice OS/architecture, les artefacts, sommes de contrôle et l'installation/mise à jour/retour arrière restent hors de ce document.
