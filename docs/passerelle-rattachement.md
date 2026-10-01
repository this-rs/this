# Déclarer la passerelle : rattacher des bases et des projets existants

Ce document décrit le **fichier de déclaration** de la passerelle IAM
(`this-gateway`) et le rattachement d'un projet qui existe déjà. Il remplace
la ligne `ExecStart` interminable de l'unité systemd : une structure qu'on
relit, qu'on versionne et qu'on fait évoluer.

> ### ⚠️ Ce que l'instance EN SERVICE ne fait pas encore (constat du 2026-09-14)
>
> L'unité `this-rs-gateway.service` (port 8144) exécute
> `~/sauvegardes-bascule-v3/this-rs-gateway.v3-j4`, écrit à **10:55** — un
> binaire **antérieur à tout ce chapitre**. Mesuré dans le binaire :
> `/auth/verify` **0** occurrence, `x-this-subject` **0**, `/services/` **0**,
> `--config` **0**, `prevol` **0**, `plan` **0** ; seul `/stores/` y est.
> Vérifié en ligne : `GET http://192.168.1.103:8144/auth/verify` rend **404**
> alors que le runbook généré par le binaire courant exige **401**.
>
> Autrement dit : **la production sert les bases en lecture seule** (STS, IAM,
> `/stores/{nom}`) et **rien** du rattachement — ni la porte `/auth/verify`, ni
> les services montés, ni la déclaration. `plan`, `prevol`, `basculer.sh` et la
> configuration par fichier existent dans ce dépôt, sont testés, mais **ne sont
> pas déployés**. Ne pas lire ce document comme la description de ce qui tourne :
> c'est la description de ce qu'on **déploie**.
>
> Toutes les mesures des §5 et §8 ont été prises sur une passerelle **locale**
> (ports 8088/8089, binaire de ce dépôt), jamais sur celle qui tourne.

---

## 1. Le fichier

```yaml
addr: 192.168.1.103:8144
keys: ~/.config/this-rs-gateway/cles        # gateway.key.pem / gateway.pub.pem
identites: ~/.local/share/this-rs-gateway/identites.obrain
issuer: obrain-n3

stores:                                     # rattacher une BASE
  - name: runtime
    path: ~/.local/share/obrain-harness/data/runtime-dev.obrain
    tenant: c90599a3-f1b6-43f1-a5c8-03b7b645f378
  - name: linux
    path: /tmp/linux-full3.obrain
    tenant: public                           # anonyme, en GET/HEAD seulement

services:                                   # rattacher un PROJET existant
  - name: viz
    upstream: http://192.168.1.103:8432
    deny: ["/api/query"]                     # refus explicites, avant tout
```

`this-gateway --config deploy.yaml` suffit. Les options de ligne de commande
restent acceptées **en complément** : un scalaire donné au CLI gagne sur le
fichier, les `--store` s'ajoutent à ceux de la déclaration.

**Les chemins.** Un `~/` initial est **développé** en `$HOME` dans `keys`,
`identites` et `stores[].path` — un seul `~`, celui de l'utilisateur courant :
`~autre/…` n'est pas deviné (on ne choisit pas le domicile de quelqu'un
d'autre), et sans `HOME` la déclaration est **refusée** plutôt que lue avec un
`~` littéral, qui désignerait un répertoire nommé `~` et ferait accuser le
store.

> **En production, écrivez des chemins ABSOLUS.** Le développement de `~` est
> récent : l'ancien binaire le prenait au pied de la lettre, donc une
> déclaration en `~` refuserait de démarrer *pendant un retour arrière* — le
> moment précis où l'on a besoin qu'elle fonctionne. La déclaration en service
> (`~/.config/this-rs-gateway/deploy.yaml`) est en absolu pour cette raison, et
> `prevol` constate les deux formats.

**Une déclaration refusée ne démarre pas** — clé inconnue (faute de frappe),
`upstream` sans `http(s)`, `mount` réservé (`/auth/…`, `/stores/…`, `/health`),
`anonymous: true` **et** `tenant:` (contradictoire), `deny` sans `/`, deux
services sur le même `mount`, store sans `tenant`, `tenant` illisible. C'est le
même parti pris que partout ailleurs : le refus par défaut, jusque dans la
configuration.

---

## 2. `stores:` — rattacher une base

| Clé | Sens |
|---|---|
| `name` | le segment de chemin : la base est servie sous `/stores/{name}` |
| `path` | le répertoire du store obrain, ouvert **en lecture seule** (verrou partagé : la passerelle coexiste avec le viz et le relief) |
| `tenant` | `<uuid>` — seul ce tenant lit — ou `public` — lisible par tout authentifié, et par l'**anonyme en GET/HEAD seulement** |

---

## 3. `services:` — rattacher un projet qui existe déjà

Le projet n'est **pas réécrit** : la passerelle le joint par HTTP et le remet
sous sa garde.

| Clé | Défaut | Sens |
|---|---|---|
| `name` | requis | nom court, utilisé dans le `mount` par défaut |
| `upstream` | requis | `http://…` / `https://…` — où vit le projet |
| `mount` | `/services/{name}` | préfixe monté (interdit : `/auth/…`, `/stores/…`, `/health`, `/healthz`, `/`) |
| `deny` | vide | chemins refusés **à travers la passerelle** (exact ou préfixe de segment) |
| `tenant` | aucun | réserve le service à ce tenant |
| `anonymous` | `false` | **sans jeton** — en lecture GET/HEAD seulement |

### Ce que le projet rattaché reçoit

L'identité **vérifiée** du principal, en en-têtes — les mêmes noms que
`/auth/verify`, une seule convention :

```
x-this-subject: <user_id | service_name | admin_id>
x-this-tenant:  <tenant_id>
x-this-roles:   <rôle1,rôle2>
```

**Un en-tête d'identité envoyé par le client est écrasé**, jamais transmis :
sinon n'importe qui choisirait son tenant. Et les en-têtes hop-by-hop, `host`,
`content-length` ne traversent pas (le proxy ne réécrit pas la réponse de
l'amont : `content-length` est recalculé par le serveur).

Conséquence à honorer côté projet : **ces en-têtes ne valent que parce que la
requête vient de la passerelle.** Le projet rattaché ne doit donc être joignable
**que** par elle.

### Le refus déclaré, et sa portée

`deny: ["/api/query"]` refuse `/api/query` **et** `/api/query/plan`, jamais
`/api/querylog` : une règle ne refuse pas plus large que ce qu'elle écrit.

C'est la sortie de secours prévue pour le viz : son `/api/query` accepte
aujourd'hui des requêtes **sans authentification** (vérifié : `POST` direct →
`400`), et la lecture brute d'un store doit passer par le gateway gardé, pas par
l'API ouverte du tableau de bord.

---

## 4. La chaîne de garde (de l'extérieur vers l'intérieur)

1. **le cookie devient un en-tête** — `POST /auth/token` pose `this_token`
   (`HttpOnly`, `SameSite=Lax`), parce qu'un navigateur ne peut pas poser
   `Authorization` sur une navigation ;
2. **authentification** (`WamiAuthProvider`, Ed25519) ;
3. **la porte** — hors chemins publics, pas de principal ⇒ **401**, sauf lecture
   anonyme d'un store `tenant=public` ou d'un service `anonymous: true` ;
4. **la garde** — store ou service : le tenant du jeton doit être propriétaire
   (`403` sinon) ;
5. **les refus déclarés** du service, puis la transmission.

---

## 5. Vérifier un rattachement

```sh
# 1. Le service est gardé : sans jeton, refus.
curl -s -o /dev/null -w '%{http_code}\n' http://127.0.0.1:8144/services/viz/        # 401

# 2. Avec un jeton, le projet répond à travers la passerelle.
curl -s -c /tmp/c.txt -X POST http://127.0.0.1:8144/auth/token \
     -H 'content-type: application/json' \
     -d '{"username":"…","password":"…"}' > /dev/null
curl -s -b /tmp/c.txt -o /dev/null -w '%{http_code}\n' http://127.0.0.1:8144/services/viz/   # 200

# 3. Le chemin refusé est refusé, même avec un jeton valide.
curl -s -b /tmp/c.txt -X POST http://127.0.0.1:8144/services/viz/api/query \
     -H 'content-type: application/json' -d '{"query":"RETURN 1"}'              # 403

# 4. La règle ne déborde pas.
curl -s -b /tmp/c.txt -o /dev/null -w '%{http_code}\n' \
     http://127.0.0.1:8144/services/viz/api/querylog                            # 200
```

Mesuré le 2026-09-14, contre le viz de n3 (`192.168.1.103:8432`) rattaché par une
passerelle d'essai locale : `401` sans jeton, `200` avec (le tableau de bord,
986 octets), `403` sur `/api/query` — alors qu'en direct le viz est public et
accepte `POST /api/query` sans authentification.

---

---

## 6. Le plan et les artefacts : `plan --config … --out DIR`

Le générateur **n'exécute rien**. Il écrit des fichiers relus par un opérateur,
chacun portant son état en tête — « préparé, non déployé » — et un `RUNBOOK.md`
qui donne l'ordre des gestes, la vérification et le retrait.

```sh
this-gateway plan --config deploy.yaml --out /tmp/plan --bin /opt/this-rs/this-gateway
this-gateway plan --config deploy.yaml --json      # le plan, sans écrire
```

| Artefact | Ce qu'il est |
|---|---|
| `this-gateway.service` | l'unité systemd **utilisateur** (l'unité déployée aujourd'hui s'appelle encore `this-rs-gateway.service` : la bascule la renomme, et le prévol le signale) : `ExecStart=… --config <déclaration>`. La ligne interminable disparaît ; aucun mot de passe n'y entre (les identités vivent dans le store d'identités) ; `LimitNOFILE` y est posé (une vue confinée par `(store, tenant)` tient ~28 descripteurs) |
| `caddy/<hôte>.caddy` | le bloc **à ajouter** au Caddyfile existant de la façade (chemin A) |
| `caddy/Caddyfile` | la variante **hôte** complète (chemin B), options globales + un bloc par site |
| `systemd/this-rs-tunnel-<hôte>.service` | le tunnel ssh, quand `tunnel: true` |
| `dns.txt` | les enregistrements **désirés** (réutilise `this::dns::desired_records`) : A/AAAA/CNAME seulement, jamais MX, TXT, NS ni SRV |
| `basculer.sh` | **la bascule, avec retour arrière ÉCRIT** : installe l'unité, la démarre, attend la santé, n'arrête l'ancienne qu'**après**, et **relance l'ancienne si la nouvelle ne répond pas**. `--sec` affiche ce qu'il ferait sans rien exécuter. Il ne touche ni les clés ni le store d'identités |
| `RUNBOOK.md` | état, ordre des gestes, vérifications avec les codes attendus, retrait, et ce que les artefacts ne font pas |

### Deux façons d'exposer un site rattaché

| `mode` | Ce que Caddy fait | Quand |
|---|---|---|
| `via_gateway` *(défaut)* | `reverse_proxy` vers la passerelle : **une seule route**, aucune décision de sécurité dans Caddy | dès que la passerelle sait joindre le projet |
| `forward_auth` | Caddy joint le projet en direct, après `forward_auth` vers `/auth/verify` (`copy_headers x-this-subject x-this-tenant x-this-roles`), **et** repose les refus déclarés en `respond … 403` | le proxy de la passerelle ne convient pas (volumes, protocoles) |

Dans les deux cas, **le tunnel joint la passerelle**, jamais chaque projet : un
seul chemin à connaître, et atteindre un projet n'ouvre plus un second tunnel.

### Ce qui a été vérifié sur les artefacts

Le 2026-09-14, pour la topologie de n3 (passerelle `192.168.1.103:8144`, viz
rattaché, `ffs.space` en `via_gateway`, `viz.ffs.space` en `forward_auth` +
tunnel) :

```sh
caddy adapt --config caddy/Caddyfile --adapter caddyfile   # exit 0, aucun avertissement
caddy fmt --diff caddy/Caddyfile                           # aucun écart
```

La configuration adaptée contient bien `forward_auth` → `/auth/verify`, le
`reverse_proxy` vers la passerelle, celui vers le viz en direct, et le refus
403. Une exposition incomplète (pas d'`acme_email`, site pointant un service
inexistant, `tunnel: true` sans `tunnel_host`, `record_type: TXT`, hôte avec un
schéma) **refuse la génération**.

---

---

## 7. Le prévol : `prevol --config … [--ssh user@hôte]`

Avant de déployer, constater — **sans rien modifier**. Le prévol exécute le
**même** script en local (`bash -s`) ou sur la cible (`ssh … bash -s`) : la
distance ne change pas ce qu'on vérifie, et il n'y a qu'un seul texte de vérité.

```sh
this-gateway prevol --config deploy.yaml --ssh triviere@192.168.1.103 \
    --bin /home/triviere/projects/this-rs/target/release/this-gateway
```

Deux constats se lisent avant tous les autres :

| Constat | Ce qu'un `MANQUE` veut dire |
|---|---|
| `cles` | la paire Ed25519 est absente : un redémarrage en **génère une nouvelle** et **invalide tous les jetons émis** — tout le monde est déconnecté |
| `identites` | le store d'identités est absent : les comptes sont à recréer, **plus personne ne se connecte** |

Les autres : `binaire` (existe et exécutable sur la cible), `store:<nom>` (la
passerelle refuse de démarrer avec une base déclarée et absente), `port` (libre =
première installation ; tenu par la passerelle = **redémarrage**, quelques
secondes d'interruption ; tenu par autre chose = `MANQUE`), `unite` (active ou
non, et si elle est **encore pilotée par la ligne de commande**),
`service:<nom>` (le code HTTP de l'amont, vu depuis la cible), `chaine`
(`cargo` présent), `moteur` (le checkout obrain), `tunnel:<hôte>` (la cible du
tunnel accepte la clé en `BatchMode` — un tunnel sans clé meurt en boucle).

Le code de sortie est **1** dès qu'un constat est `MANQUE` : un prévol vert veut
dire quelque chose, un prévol rouge arrête un déploiement automatique.

`--json` rend le même constat pour une machine.

### Mesuré sur n3 (2026-09-14, lecture seule)

| constat | verdict | preuve |
|---|---|---|
| binaire | OK | `/home/triviere/projects/this-rs-passerelle/target/release/this-rs-gateway` (ancien nom) |
| cles | OK | les deux PEM sont là : **les jetons émis survivent au redémarrage** |
| identites | OK | `~/.local/share/this-rs-gateway/identites.obrain` |
| store:runtime | OK | `~/.local/share/obrain-harness/data/runtime-dev.obrain` |
| store:linux | OK | `/tmp/linux-full3.obrain` |
| port | OK | `8144` déjà tenu par la passerelle : **REDÉMARRAGE**, interruption de quelques secondes |
| unite | ATTENTION | active, **encore pilotée par la ligne de commande** — ce redémarrage bascule sur la déclaration |
| service:viz | OK | `http://192.168.1.103:8432` répond `200` |
| chaine | OK | `cargo` présent : le binaire peut être construit sur la cible |
| moteur | OK | `~/projects/banc-obrain-j4` |
| tunnel:viz.ffs.space | OK | `triviere@192.168.1.103` joignable sans mot de passe |

---

---

## 8. La répétition avec un vrai Caddy (2026-09-14)

`caddy adapt` valide la **syntaxe** ; il ne dit rien de ce qui se passe à
l'exécution. La répétition a donc fait tourner un **Caddy réel** (le binaire de
`ffs-space-front`, v2.11.4) devant une passerelle locale attachant le **vrai viz
de n3**, sur deux sites : `http://localhost:8088` en `forward_auth` et
`http://localhost:8089` en `via_gateway` (adaptation locale : `auto_https off`,
ports hauts, logs dans un dossier temporaire — l'ACME ne peut pas être exercé
sans 443 ni DNS public, et c'est la seule chose que la répétition ne couvre pas).

| Vérification | `forward_auth` | `via_gateway` |
|---|---|---|
| sans jeton `GET /` | **401** | **401** |
| login à travers la façade `POST /auth/token` | **200** + cookie | **200** |
| avec cookie `GET /` | **200** (986 octets du viz) | **200** (986 octets) |
| avec cookie `POST /api/query` | **403** (refus de la **façade**) | **403** (refus de la **passerelle**) |
| cookie forgé `GET /` | **401** | — |
| `GET /api/querylog` (la règle ne déborde pas) | **404** — comme le viz en direct, donc c'est bien l'amont | — |
| `GET /api/query/plan` (sous-chemin de la règle) | **403** | — |

### Les deux défauts que la répétition a trouvés — et que la relecture n'avait pas vus

1. **En `forward_auth`, aucune route ne menait au login.** `forward_auth`
   s'appliquait à *tout*, `/auth/token` compris : un navigateur n'obtenait
   **jamais** son cookie de session. Une garde sans porte d'entrée n'est pas une
   garde, c'est une fermeture. Le bloc porte désormais une route explicite
   (`@passerelle path /auth/* /auth /health /healthz` → passerelle) **avant** la
   porte.
2. **En `via_gateway`, la racine du site allait au 404 de la passerelle** : `/`
   n'est pas une route de la passerelle (elle sert `/stores/…`, `/services/…`,
   `/auth/…`, `/health`). La page d'accueil d'un site exposé était donc un 404,
   la garde fonctionnant par ailleurs. Le bloc **préfixe** maintenant vers le
   montage du service (`rewrite * /services/<nom>{uri}`).

Dans les deux cas, les directives sont enveloppées dans un bloc `route { }` :
Caddy réordonne les directives nues selon son ordre interne, et ici **l'ordre
est porteur de sens** (le login d'abord, le refus ensuite, la porte après, le
projet en dernier).

### Le nom : `this-gateway`

Le binaire s'appelle **`this-gateway`** (et l'unité `this-gateway.service`).
L'unité déployée aujourd'hui sur n3 s'appelle encore `this-rs-gateway.service` :
la bascule la renomme, et le prévol le **signale** (`unite` → `ATTENTION`)
plutôt que d'annoncer une première installation. Les chemins qui portent des
**données** ne changent pas — `~/.config/this-rs-gateway/cles` et
`~/.local/share/this-rs-gateway/identites.obrain` : les renommer ferait générer
une nouvelle paire de clés et **déconnecterait tout le monde**.

---

### Ce que le proxy porte — et ce qu'il ne porte pas

| Cas | Verdict | Preuve |
|---|---|---|
| réponse ordinaire | portée | les matrices ci-dessus |
| **flux (SSE, long polling, flux infini)** | **porté, transmis et non tamponné** | mesuré le 2026-09-14 : TTFB **0,005 s** à travers la passerelle contre **0,701 s** au total (le second événement n'est écrit qu'à 700 ms) — l'ancienne version, qui attendait `reponse.bytes()`, aurait **figé** le client sans lever d'erreur. Verrouillé par un test qui lit la réponse en TCP brut |
| **WebSocket / `Upgrade`** | **NON porté** | mesuré : une demande d'upgrade traverse et rend un `200` ordinaire, jamais un `101` — le proxy est un client HTTP, il ne peut pas tunneliser une connexion mise à niveau |

**Conséquence pratique** : un projet qui parle WebSocket se rattache en mode
`forward_auth` (Caddy joint le projet en direct et porte le WebSocket
nativement), pas en `via_gateway`. Le dire ici évite de le découvrir en
production.

---

---

## 9. La bascule, répétée sur la machine de production (2026-09-14)

**Correction d'un fait que j'avais mal établi** : `192.168.1.103` (que j'appelais
« n3 » comme une autre machine) **est cette machine** — `hostname` rend `n3`,
`enp2s0` porte `192.168.1.103/24`, et le `MainPID` de `this-rs-gateway.service`
est le même en local qu'à travers `ssh`. La production tourne donc **ici**, en
unité utilisateur : la bascule est un geste **local**, sans copie de binaire ni
étape distante. Tout ce que j'ai mesuré « à distance » était en lecture seule —
mais la distinction poste/n3 était fausse, et elle change la nature du dernier
acte : il est plus simple qu'annoncé.

### Les deux chemins, mesurés

| Chemin | Ce qui a été fait | Résultat |
|---|---|---|
| **nominal** | unité « ancienne » active, puis `basculer.sh` | unité installée, **ancienne arrêtée**, nouvelle démarrée, `GET /health` → **200**, service rattaché → **401** sans jeton |
| **à sec** (`--sec`) | rien n'est exécuté | toutes les commandes sont affichées préfixées `[à sec]`, **la nouvelle unité reste inactive**, aucune santé attendue (un mode d'essai qui crie au loup apprend à ignorer les cris) |
| **retour arrière** | port occupé par un autre processus → la nouvelle unité ne peut pas devenir saine | après 15 essais : `ÉCHEC` nommé, nouvelle unité **arrêtée**, **ancienne relancée**, code de sortie **1**, message « l'ancienne configuration est intacte » |

### L'ordre : l'ancienne est arrêtée AVANT — et c'est une contrainte, pas un choix

Ce document décrivait l'inverse (« l'ancienne n'est arrêtée qu'après la santé de
la nouvelle »), pour éviter toute coupure. **Cet ordre ne peut pas fonctionner**,
et c'est mesuré : le store d'identités s'ouvre en **verrou exclusif**.

```
Failed to open ObrainDB: substrate: failed to open existing store at … :
I/O error: substrate … is already open by another process (exclusive lock on
substrate.meta refused: Resource temporarily unavailable)
```

Deux passerelles ne peuvent donc **pas** coexister : démarrer la nouvelle en
laissant l'ancienne tourner échoue à l'ouverture du store d'identités, la santé
ne vient jamais, et le script déclenche un **retour arrière à chaque bascule**.
Le test `deux_passerelles_ne_partagent_pas_le_store_didentites`
(`tests/passerelle_identites.rs`) le mesure avec deux passerelles réelles ; il
tombera le jour où obrain permettra la coexistence — et l'ordre sans coupure
redeviendra alors possible, à **reposer** plutôt qu'à supposer.

La coupure est réelle mais brève : mesuré sur cette machine, avec la déclaration
de production (4 bases + le viz), la nouvelle répond sur `/health` en
**0,2 s** quand les pages de ses bases sont résidentes, **jusqu'à 3,3 s à froid** (mesuré, 4 bases + viz). `basculer.sh`
arrête donc l'ancienne, démarre la nouvelle, attend la santé, et relance
l'ancienne si elle ne vient pas.

Le script garde aussi l'unité existante de côté (`this-gateway.service.avant-bascule`) :
un retour arrière sans la copie de l'ancien fichier n'est pas un retour arrière.

### Un défaut trouvé par le test de l'artefact

L'en-tête « préparé, non déployé » était écrit **avant** le `#!/bin/sh` : le
script ne s'exécutait donc pas directement (`./basculer.sh`). Un artefact qu'on
ne peut pas lancer tel quel n'est pas déployable. Le shebang est désormais la
première ligne, et un test l'exige.

### Deux autres, trouvés le 2026-09-14 — et ce qu'ils changent aux répétitions ci-dessus

Le même test lisait le **texte** du script (shebang, mots-clés, absence de
`rm -rf`) sans jamais demander au shell s'il était **lisible**. Deux défauts
vivaient donc dans l'artefact, invisibles à la relecture :

1. **La commande d'installation n'était pas du shell valide.** `faire` ré-exécute
   son argument avec `sh -c` ; la substitution `\"$(dirname \"$0\")\"` y arrivait
   mal échappée → `sh: Syntax error: Unterminated quoted string`. La bascule
   échouait à son **premier** geste.
2. **Le binaire par défaut s'écrivait `%h/projects/…`** — un spécificateur
   **systemd**, que le **shell** ne développe pas. Le contrôle `[ -x "$BIN" ]`
   portait sur un chemin nommé `%h/…` : `basculer.sh` répondait
   `REFUS … rien n'a été touché` sur toute machine, et `prevol` annonçait
   `binaire MANQUE` là où le binaire était installé.

**Conséquence, dite franchement** : les répétitions du §9 ne peuvent pas avoir
été produites par *ce* générateur avec ses options par défaut —
`plan --config … --out DIR`, la commande documentée, écrivait un script qui
refusait de démarrer. Elles l'ont été avec un `--bin` explicite, ou par une
version antérieure du générateur. Ce qui est vérifié aujourd'hui, sur
l'artefact régénéré :

| Vérification | Résultat |
|---|---|
| `sh -n basculer.sh` | **OK** — le script est du shell valide |
| chaque commande du mode à sec relue par `sh -n -c` | **OK** (4 commandes) |
| `sh basculer.sh --sec` | **exit 0**, aucune unité touchée |
| `BIN=` du script | **absolu** (`/home/…/this-gateway.v4-j4-20260914`), plus de `%h` |
| `prevol --config deploy.yaml` sans `--bin` | **exit 0, aucun `MANQUE`** |

Dans le mode à sec, `[à sec]` annonce une **commande** qui serait exécutée (et
c'est ce que le test relit avec `sh -n -c`) ; `(à sec)` annonce une **décision**.
Un préfixe unique pour les deux rendait la sortie inanalysable — et c'est
précisément ce que la vérification automatique doit pouvoir lire.

---

## 10. Les tests qui tiennent ce contrat

```sh
cargo test --features obrain,wami,proxy --test passerelle_rattachement
```

Ils couvrent : 401 sans jeton, transmission à l'amont avec chemin dépréfixé et
requête préservée, **identité vérifiée reçue par l'amont**, **en-tête forgé
écrasé**, refus déclaré borné, service `anonymous: true` lisible en GET mais
jamais en POST, et sept déclarations invalides qui doivent refuser le démarrage.
