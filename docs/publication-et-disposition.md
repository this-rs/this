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

Les features optionnelles ajoutent deux voisins :

```
<parent>/../banc-obrain-j4/   # obrain-engine, -core, -common, -substrate, -adapters, -reactive
<parent>/../vite-rs/          # this-vite (préférer la version publiée pour un usage externe)
```

La CI compense cette disposition dans chaque job (`this-cli/.github/workflows/ci.yml` place `this-rs/this` à `../this` ; `this-examples` utilise l'action composite `./.github/actions/this-dep`). Ce document est la version lisible de cette connaissance, qui n'existait que dans les workflows.
