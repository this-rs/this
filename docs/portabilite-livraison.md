# Portabilité de livraison — inventaire des preuves (issue #289)

Ce document confronte les **6 cases d'acceptation** de #289 à ce qui est
**réellement mesuré** sur la branche. C'est un inventaire, pas une proposition :
une case est couverte quand une preuve littérale (fichier:ligne, nom de job,
commande rejouable avec son code de sortie) la tient ; sinon le manque est dit
tel quel.

Mesuré sur `main` = `787a988` (base de `fix/289-portabilite-livraison`). Chemins
relatifs à la racine du dépôt.

## Le constat de l'issue a changé depuis le 19/11

À l'ouverture, `Cargo.toml` déclarait obrain par `path = "../../obrain/obrain-db/crates/…"`
et this-vite par `path = "../../vite-rs"`. Trois commits fusionnés sur `main` ont
résolu le problème par le passage en **dépendances git épinglées** :

- `f7256fe` — `this-vite` → `git + rev` (`dcf233cd…`, dépôt public `this-rs/vite-rs`)
- `3091ab8` — les six `obrain-*` → `git + rev` (`f0d6e132…`, dépôt public `obrain-db/obrain`),
  et suppression de `scripts/bootstrap-dev.sh` (il n'y a plus rien à cloner)
- `4e0983d` — `wami` était déjà `git + rev` (`a4e1070…`)

La prémisse (« une arborescence locale imposée ») ne tient donc plus : plus aucun
chemin `../../` vers un dépôt voisin dans le manifeste (les seules occurrences de
chemins `../../…` dans `Cargo.toml` sont des **commentaires** : `../../banc-obrain-j4`
à la ligne 112 — historique — et `../../vite-rs` à la ligne 153 — l'exemple `[patch]`).

## Case 1 — build depuis un répertoire temporaire sans checkouts frères, bootstrap documenté, SHAs épinglés

**Verdict : couvert** (par construction, puis vérifié).

- `Cargo.toml:126-144` : les six crates `obrain-*` sont `git = "https://github.com/obrain-db/obrain"`
  avec `rev = "f0d6e132666d34d63824a46ad2a5119416b0afb4"` (SHA complet).
- `Cargo.toml:154` : `this-vite` est `git = "https://github.com/this-rs/vite-rs.git"` avec
  `rev = "dcf233cd154ed7416338c9141736ff9efa19f199"`.
- `Cargo.toml:161-163` : `wami`/`wami-core`/`wami-condition` en `git + rev = a4e1070…`.
- `cargo metadata --no-deps` → **code 0** : le manifeste se résout sans aucun voisin.
- `cargo check -p this-rs --features obrain` → **code 0** (les six crates compilent depuis git).
- `cargo check -p this-rs --features frontend` → **code 0** (`this-vite` compile depuis git).
- `scripts/verifier-clone-propre.sh` (ajouté sur cette branche) rejoue cette vérification
  depuis un répertoire temporaire dont les voisins n'existent pas, et documente que le
  réseau (GitHub, dépôts publics) est requis par cargo pour résoudre le manifeste.
- L'ancien `scripts/bootstrap-dev.sh` a été supprimé (`3091ab8`) : il n'y a plus de checkout
  frère à cloner. Pour développer obrain/vite-rs en local sans toucher le manifeste,
  `[patch]` dans `.cargo/config.toml` (documenté dans `docs/publication-et-disposition.md`).

## Case 2 — choix explicite : dépendances publiées vs sources épinglées ; overrides locaux seulement pour le développement

**Verdict : couvert.**

- `Cargo.toml:146-154` (vite-rs) : « `version` + `git` + `rev`, comme WAMI ci-dessous : le
  hash fixe le code, la `version` est ce que cargo résout à la PUBLICATION » — et l'exemple
  `[patch."https://github.com/this-rs/vite-rs.git"] this-vite = { path = "../../vite-rs" }`.
- `Cargo.toml:156-163` (WAMI) : « le hash fixe le code pendant le développement, la `version`
  est ce que cargo résout à la PUBLICATION (`path` et `git` sont retirés du manifeste empaqueté) ».
- `docs/publication-et-disposition.md`, section « Disposition des dossiers : plus aucun
  checkout frère » : tableau feature → source (git + SHA), et le `[patch]` pour développer
  en local.
- `release.yml`, job `publish-crate` (commentaire) : obrain reste hors de crates.io, c'est
  la contrainte qui bloque `cargo package`, pas un choix silencieux.

## Case 3 — CI teste les features réellement livrées (obrain, frontend…), pas seulement `default` ou `all`

**Verdict : couvert par cette branche** (était « partiel »).

Avant :

- le job `obrain` de `ci.yml` était **gaté sur un jeton** : sans `OBRAIN_TOKEN`, il
  n'exécutait rien (avertissement + `exit 0`). La feature `obrain` n'était donc pas
  compilée en CI.
- aucune compilation de la feature `frontend` (`all` ne l'inclut pas, `Cargo.toml`
  `[features]` : `frontend` est absent de `all`).

Après :

- `ci.yml`, job `obrain` : le clone privé `OBRAIN_TOKEN` est retiré (obrain est désormais
  une dépendance git publique) ; le job exécute `cargo check -p this-rs --features obrain`
  sans condition de secret.
- `ci.yml`, nouveau job `frontend` : `cargo check -p this-rs --features frontend`.
- les deux sont reliés au `gate` (`needs:`), avec la condition
  `if: needs.changes.outputs.code == 'true'` comme les autres jobs.

## Case 4 — `cargo package` + consommateur externe minimal ; liste des prérequis non publiés explicite

**Verdict : partiel** (le blocage est documenté, pas simulé).

- `cargo package -p this-rs` est **bloqué** : les six `obrain-*` sont absents de crates.io,
  et cargo refuse d'empaqueter un crate dont une dépendance optionnelle est absente du
  registre (mesuré, `release.yml` commentaire du job `publish-crate`, désactivé pour cette
  raison sous `vars.PUBLISH_CRATES_IO`).
- Liste des prérequis non publiés : `docs/publication-et-disposition.md`, tableau
  « Prérequis non publiés » — `obrain-*`, `this-agent`, `this-llm`, `this-harness` absents ;
  `wami` 0.21 / `wami-core` 0.21 / `wami-condition` 0.18.3 déclarés contre 0.18.0 / 0.19.0 /
  0.18.0 publiés.
- `scripts/verifier-publication.py` (exécutable, hors réseau) vérifie la règle « toute
  dépendance `path` d'un paquet publiable doit avoir une `version` » et **calcule** l'ordre
  de publication depuis le graphe (pour qu'il ne vieillisse pas dans un document).
- Paquetage prouvé : `this-descriptor` (`docs/publication-et-disposition.md`, 03/11) et
  `this-module-sdk` (même document, 21/11).
- **Reste ouvert** : un consommateur externe minimal qui dépend du crate publié. Ce qui
  exige une publication réelle sur crates.io (jeton, réversible seulement par yank) est
  hors de portée d'une branche locale.

## Case 5 — matrice OS/architecture, artefacts + sommes de contrôle ; installation, démarrage, mise à jour et retour arrière

**Verdict : ouvert** (partiellement couvert).

- Partiel : `ci.yml`, job `cross-platform` — `cargo build --features all,wami` et
  `cargo test --lib --features all,wami` sur `ubuntu-latest`, `windows-latest`, `macos-latest`.
- **Manque** : la matrice n'inclut pas `obrain` ni `frontend` (le correctif de la case 3 ne
  l'étend pas à Windows/macOS), et aucun artefact publié n'est accompagné d'une somme de
  contrôle. Installation, démarrage, mise à jour et retour arrière ne sont testés nulle part.
- `release.yml`, job `create-release` : `softprops/action-gh-release` génère les notes mais
  ne produit ni matrice d'artefacts ni checksums.

## Case 6 — aucun chemin personnel, checkout implicite ou secret requis dans l'artefact

**Verdict : couvert** (par construction).

- Aucun chemin `../../` ni `~/` dans les manifestes (seules des dépendances `git + rev`
  ou `path` internes au workspace, `crates/…`).
- Le job `obrain` n'exige plus de secret `OBRAIN_TOKEN` (obrain est public) : cette branche
  retire cette exigence.
- Le seul secret restant est `CARGO_TOKEN` pour `cargo publish` (`release.yml`) — il n'est
  pas dans l'artefact, c'est le jeton du publish, action humaine.

## Ce que la branche change

- `docs/portabilite-livraison.md` (ce document).
- `.github/workflows/ci.yml` : job `obrain` débarrassé du clone privé, nouveau job `frontend`,
  `gate` relié.
- `scripts/verifier-clone-propre.sh` : vérification « clone propre » rejouable.
- `README.md` : les deux mentions de `scripts/bootstrap-dev.sh` (supprimé) sont corrigées.
