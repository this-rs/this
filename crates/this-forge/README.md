# this-forge

Le service qui compile du Rust en wasm pour des comptes : inscription sur invitation, quotas,
build dans un conteneur sans réseau, épreuve de l'ABI, signature Ed25519. API : §10 de
`docs/modules-wasm-abi.md`.

> **Bêta fermée. Un conteneur n'est PAS une frontière de sécurité pour des inconnus.**
> Le build tourne dans docker (sans réseau, racine en lecture seule, toutes capacités retirées,
> `no-new-privileges`, 6 CPU / 4 Go / 256 pids, 180 s) : cela borne un invité maladroit, pas un
> adversaire. Un échappement de conteneur (noyau partagé) donne l'hôte — et la clé de
> signature. Ni microVM, ni KMS. N'ouvrez la forge qu'à des personnes invitées que vous
> connaissez, sur une machine qui ne porte rien d'autre. Voir « Fragile » plus bas.

## Lancer

```sh
# 1. l'image de build (avec réseau, une fois) : toolchain 1.93 + cible wasm32 + dépendances vendorées
docker build -t this-forge-builder:1.93 -f crates/this-forge/docker/Dockerfile crates/this-forge/docker

# 2. une invitation (usage unique par défaut), puis le service
cargo run -p this-forge -- invite --data-dir ./forge-data --uses 1 --note "Aurélie"
cargo run -p this-forge -- serve  --data-dir ./forge-data --listen 127.0.0.1:8480
```

`--signing-key <chemin>` (défaut `<data-dir>/forge-signing.key`) : clé Ed25519 créée au premier
démarrage en mode `0600` ; un fichier plus permissif est refusé. La clé des JETONS est à côté
(`….jwt`) : deux clés, deux rôles — un jeton volé n'est pas une preuve de provenance de code.

## Tests

```sh
cargo test -p this-forge                       # vert sans docker (14 tests)
cargo test -p this-forge --test docker -- --ignored --nocapture --test-threads=1   # 7 tests, docker requis
```

## Arborescence

| fichier | rôle |
|---|---|
| `src/config.rs` | réglages (bornes, quotas, image, stockage) |
| `src/entities.rs` | `account`, `invitation`, `session`, `build` (`impl_data_entity!`) et le **module descripteur** `forge`, vérifié par `DescriptorSet` au démarrage |
| `src/store.rs` | un `DataService` du framework par entité : `InMemoryDataService` (tests) ou `LmdbDataService` |
| `src/auth.rs` | JWT Ed25519, argon2id, jetons de rafraîchissement (voir « Écarts ») |
| `src/sign.rs` | clés `0600`, JSON canonique, charge signée, empreinte des sources |
| `src/runner.rs` | `BuildRunner` : gabarit imposé, conteneur, diagnostics `--message-format=json` |
| `src/wasm_check.rs` | épreuve avant signature : aucun import, 4 exports aux signatures exactes (`wasmparser`) |
| `src/service.rs` | `Forge` : état, file d'attente (un build à la fois), reprise après redémarrage |
| `src/api.rs` | les routes `/v1/*` |
| `src/lib.rs` | `app()` : le routeur porté par `this::server::ServerBuilder` |
| `src/main.rs` | `serve` et `invite` |
| `docker/` | `Dockerfile` + **gabarit** (`template/Cargo.toml`, `Cargo.lock`) : la liste blanche |
| `tests/` | `api.rs` (sans docker), `docker.rs` (`#[ignore]`) |

## Ce que le client peut soumettre

Uniquement des fichiers `src/**/*.rs` (dont `src/lib.rs`). Le `Cargo.toml` et le `Cargo.lock` sont
ceux de `docker/template/` : c'est ce qui interdit `build.rs`, les proc-macros du client et toute
dépendance hors liste. **Liste blanche** : `serde` (derive), `serde_json`. Pour ajouter une
dépendance (ex. `this-module-sdk` quand il existera) : l'ajouter à `docker/template/Cargo.toml`,
régénérer `Cargo.lock`, reconstruire l'image (`cargo vendor` l'y embarque). Rien d'autre à changer.

## Écarts à « rien n'est réimplémenté »

* **Auth** : le STS (`this::core::auth::sts`) et l'annuaire argon2 sont derrière la feature
  `wami` (trois dépôts git, IAM complet). La forge a écrit elle-même, avec les crates que le
  framework utilise (`jsonwebtoken`, `ed25519-dalek`, `argon2`) : un JWT Ed25519 (15 min), des
  jetons de rafraîchissement à rotation (stockés hachés), argon2id aux paramètres par défaut du
  crate (OWASP). ~100 lignes dans `auth.rs`.
* **Garde d'autorisation du framework désarmée** (`without_authorization()`) : elle est par entité
  (pluriel enregistré) et la forge n'expose aucun CRUD générique. Chaque route authentifie elle-même
  (`api::Authed`). Les routes ne passent donc pas par `descriptor_policy`.
* **Le framework sert** : le serveur (`ServerBuilder`, santé, couches), le stockage, les entités et
  leur descripteur. Il ne sert **pas** les routes métier (`/v1/*`) : ce sont des routes axum
  écrites à la main, car ce ne sont pas du CRUD d'entités.

## API (résumé ; le détail est dans le rapport et `src/api.rs`)

Erreurs : `{"error":{"code":"…","message":"…"}}`. Codes : `invalid_invitation` 403, `email_taken` 409,
`weak_password`/`invalid_email`/`invalid_files`/`invalid_name`/`invalid_version`/`unsupported_toolchain` 400,
`unauthorized` 401, `not_found` 404, `not_ready`/`build_failed` 409,
`quota_builds_exceeded`/`quota_cpu_exceeded`/`too_many_pending` 429, `internal` 500.

## Fragile (assumé)

* Un conteneur n'est pas une frontière (voir l'avertissement en tête).
* `cpu_seconds_left` compte des **secondes de mur** de build, pas des secondes-CPU.
* Pas de limitation de débit sur `login`/`signup` (argon2 coûte ~50 ms, pas une protection).
* Le jeton d'accès survit au `logout` jusqu'à son expiration (15 min) : il est sans état.
* Recherches par `list()` + filtre : correct pour des dizaines de comptes, pas pour des milliers.
* Les artefacts sont lus en mémoire à chaque téléchargement (≤ 4 Mio).
* L'épreuve lit le binaire (imports/exports) mais ne l'**instancie** pas et n'appelle pas `manifest()` : la
  validité du descripteur rendu est vérifiée par l'hôte au chargement, pas par la forge.
* Les diagnostics d'un build script ou d'un échec docker sont la fin de `stderr` brute (`file: ""`).
* L'image est liée à l'architecture de la machine (arm64 ici) ; le `.wasm` n'en dépend pas.
