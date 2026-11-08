# Déploiement de this-forge sur ffs-n2 (LAN, bêta)

État mesuré le 08/11/2026. Machine : `ssh ffs-n2` (192.168.1.102, x86_64, 16 cœurs, 109 Go),
utilisateur `triviere` **sans sudo sans mot de passe** : tout vit donc en espace utilisateur
(unité `systemd --user`, `loginctl` linger = yes, données sous `~/forge/`).

## Exploitation

| | |
|---|---|
| URL (LAN) | `http://192.168.1.102:9910/v1` |
| Port | 9910 (9901/9902 = llama-server, ne pas toucher) |
| Unité | `this-forge.service` (**utilisateur** : `systemctl --user … this-forge`) |
| Données | `/home/triviere/forge/data` (0700) : `db/`, `artifacts/`, `sources/`, `forge-signing.key`, `forge-signing.key.jwt` (0600) |
| Binaire | `~/forge/bin/this-forge` ; sources de build : `~/forge/src` |
| Journaux | `journalctl --user -u this-forge -f` |
| Comptes | `~/forge/comptes.txt` (0600) — mots de passe générés, à changer (voir « Manques ») |
| Sauvegardes | `~/forge/backup.sh` → `~/forge/backups/forge-<date>.tar.gz` (0600) |

Pas de redirection de port, pas de tunnel : joignable depuis le LAN uniquement.

## Installation (procédure exacte)

```sh
# 0. depuis le Mac : transférer l'arbre (sans target/ ni .git)
rsync -a --delete --exclude target/ --exclude .git/ <worktree>/ ffs-n2:forge/src/

# 1. sur n2 : la copie de build ne doit pas dépendre de dépôts voisins absents
#    (le Cargo.toml racine déclare des dépendances optionnelles par chemin hors dépôt :
#    ../../obrain, ../../vite-rs — cargo les charge même feature éteinte). On les retire
#    de la COPIE, jamais du dépôt :
cd ~/forge/src && sh crates/this-forge/deploy-strip-obrain.sh .

# 2. compiler en release (cible x86_64 native) et installer
cargo build --release -p this-forge
mkdir -p ~/forge/bin ~/forge/data && chmod 700 ~/forge/data
cp target/release/this-forge ~/forge/bin/

# 3. l'image de build (réseau, une fois ; embarque le SDK des modules : cf. docker/build-image.sh)
sh crates/this-forge/docker/build-image.sh            # → this-forge-builder:1.93

# 4. invitations PUIS service (LMDB : une invitation se crée avec le service arrêté ou avant)
~/forge/bin/this-forge invite --data-dir ~/forge/data --uses 1 --note "Prénom"

# 5. l'unité
cp crates/this-forge/deploy/this-forge.service ~/.config/systemd/user/
systemctl --user daemon-reload && systemctl --user enable --now this-forge
```

## Limites de ressources (pour ne pas affamer les llama-server)

Deux niveaux, parce que **le build s'exécute dans un conteneur lancé par le démon docker, hors
du cgroup du service** : les limites de l'unité ne le bornent pas.

* Unité (le service lui-même, léger) : `Nice=10`, `CPUQuota=400%`, `MemoryMax=4G`, `IOWeight=20`.
  En systemd **utilisateur**, les contrôleurs délégués sont `cpu memory pids` : `IOWeight` est
  accepté mais **sans effet** ici.
* Conteneur de build (là où la charge se produit) : `--cpus 4 --memory 3g --cpu-shares 128
  --blkio-weight 100` (réglages `serve --cpus/--memory/--cpu-shares/--blkio-weight`), plus
  `--pids-limit 256`, 180 s maximum, réseau coupé. `--cpu-shares 128` = poids 8× inférieur au
  défaut docker (1024) : le build cède dès que la machine est chargée.

## Mise à jour

```sh
rsync … (étape 0) ; sh crates/this-forge/deploy-strip-obrain.sh ~/forge/src
cd ~/forge/src && cargo build --release -p this-forge
systemctl --user stop this-forge && cp target/release/this-forge ~/forge/bin/ && systemctl --user start this-forge
# si docker/ ou le SDK ont changé : sh crates/this-forge/docker/build-image.sh
```
La clé de signature n'est jamais régénérée tant que `forge-signing.key` existe : l'empreinte
reste stable (vérifiée après redémarrage et après `kill -9`).

## Sauvegarde

`~/forge/backup.sh` arrête le service ~1 s (LMDB : pas de copie à chaud), archive tout le
répertoire de données (base, clés, artefacts) en `0600`, relance. Pas de cron. **La clé de
signature est dans l'archive** : l'archive se protège comme la clé. Restauration : arrêter le
service, vider `~/forge/data`, `tar -xzf … -C ~/forge/data`, redémarrer.

## Vérification de bout en bout

`tests/wasm_forge_artifact.rs` (dépôt racine) charge l'artefact **téléchargé de la forge** :
`FORGE_WASM=… FORGE_WASM_SHA256=… cargo test --features wasm-modules --test wasm_forge_artifact -- --ignored --nocapture`.

## Manques / fragile

* **Pas de route de changement de mot de passe** (`/v1/auth/*` : signup, login, refresh, logout).
  Les mots de passe générés restent donc ceux des comptes : manque à combler avant d'ouvrir à
  d'autres personnes.
* Reboot de n2 **non testé** (voir le rapport) : `Restart=always` est vérifié par `kill -9`,
  l'unité est `enabled` et le linger est actif, ce qui est la condition pour que l'unité
  utilisateur démarre sans session ouverte.
* Service en HTTP clair sur le LAN, sans TLS, sans limitation de débit sur `login`.
* Un conteneur n'est pas une frontière de sécurité (voir README).
