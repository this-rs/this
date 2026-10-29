# Déployer une structure, y rattacher des projets qui existent déjà

Ce document décrit l'**hôte de déploiement** : un binaire piloté par un fichier,
qui monte les surfaces de this-rs et **rattache** des projets déjà en service —
leur base, leurs identités, leurs politiques, leur exposition — sans les
réécrire et **sans écrire de Rust** pour les rattacher.

Il complète [`adoption-projet-existant.md`](adoption-projet-existant.md) : ce
dernier décrit le parcours *dans* un projet (brancher, observer, adopter ses
identités, armer) ; celui-ci décrit la **structure** dans laquelle un ou
plusieurs projets se branchent.

---

## 1. Ce que la structure est, et ce qu'elle n'est pas

```sh
cargo build --bin this-host --features obrain
this-host --config deploy.yaml --dry-run   # le plan, sans ouvrir aucune base
this-host --config deploy.yaml             # la structure, en service
```

Trois propriétés, et elles sont volontaires :

| | |
|---|---|
| **Piloté par configuration** | une application écrit du Rust (`ServerBuilder`, `Module`, `EntityDescriptor`) ; l'hôte lit un fichier et monte ce qu'il décrit. |
| **Une base et une identité, pas du trafic** | l'hôte ne proxifie pas vers un projet existant. `config/server.yaml → proxy.caddy` **décrit** le proxy et rend un `Caddyfile` que l'exploitant applique : c'est le partage assumé du dépôt depuis l'origine. |
| **Aucune décision devinée** | ce qui n'est pas déclaré n'est pas monté ; ce qui est déclaré et illisible **arrête le démarrage**. |

L'hôte assume une chose que la bibliothèque refuse : **ouvrir un store depuis un
chemin de configuration**. Le framework ne le fait jamais (« ouvrir une base est
une décision d'application ») ; l'hôte **est** cette application, il prend la
décision — et il la publie dans son plan, avec ses conséquences.

---

## 2. Le fichier de déploiement

```yaml
bind: 127.0.0.1:8080        # défaut : 127.0.0.1:8080 — jamais 0.0.0.0 par défaut

attachments:
  - name: facturation                 # [a-zA-Z0-9_-], unique
    mount: /attached/facturation      # défaut : /attached/<name>
    store:
      kind: obrain                    # obrain | memory
      path: /srv/facturation/app.obrain
      read_only: true                 # défaut : true
    exposure: /srv/facturation/config/server.yaml   # CORS, domaines, Caddy, DNS
    policies: /srv/facturation/config/links.yaml    # liens + politiques `auth:`
    auth: /srv/facturation/config/auth.yaml         # WAMI (feature `wami`)
    authorization:
      mode: enforce                   # enforce (défaut) | report_only | off
    identity:
      principal: "account:{user_id}"
      scope: "org:{tenant_id}"
```

### Les trois régimes d'autorisation

| `mode` | Ce qui se passe | Quand |
|---|---|---|
| `enforce` *(défaut)* | un refus arrête la requête (403) | politiques déjà vérifiées |
| `report_only` | les refus sont calculés, journalisés et publiés (`x-this-rs-authz`), **rien n'est bloqué** | adoption d'un projet qui tourne |
| `off` | les politiques sont **décoratives** | service réellement public |

### Les placeholders d'identité

`identity` traduit l'identité de this-rs vers **les noms du projet**. Les
placeholders connus sont exactement ceux que porte `AuthContext` :

`{user_id}`, `{tenant_id}`, `{role}`, `{service_name}`, `{admin_id}`,
`{resource_id}`, `{resource_type}`.

Tout autre nom **refuse le chargement** (une faute de frappe ne doit pas donner
un mappage silencieusement vide). Et une valeur **absente** pour le contexte
courant (un `{user_id}` sur un appel de service) ne fait pas deviner : le
mappage ne rend rien, la politique devient non tranchable — donc **refusée**.

> Sans `identity`, le modèle natif de this-rs s'applique
> (`User`/`Owner` → `(user_id, tenant_id)`, `Service` → `(service_name,
> tenant_id)`, `Admin` → `(admin_id, "global")`), et le plan le dit.

### Ce qui est refusé au chargement

Une configuration ne casse pas à la première requête :

* `bind` qui n'est pas une adresse `IP:port` ;
* aucun attachement déclaré ;
* nom vide, hors `[a-zA-Z0-9_-]`, ou en double ;
* `mount` sans `/` initial, finissant par `/`, réservé (`/`, `/health`, `/healthz`) ou
  déjà pris par un autre attachement ;
* `store.path` vide, ou **deux attachements sur la même base** (le second
  échouerait de toute façon sur le bail d'écriture — autant le dire avant) ;
* fichier `exposure` / `policies` / `auth` déclaré et **absent** ;
* `auth:` déclaré sans la feature `wami` ;
* placeholder d'identité inconnu ;
* clé inconnue **n'importe où** (`deny_unknown_fields`) : `authorisation:` au
  lieu de `authorization:` ne passe pas.

---

## 3. Le plan : ce qui est rattaché, et ce qui ne l'est pas

`GET /` publie le plan de la structure en service. `--dry-run` publie **le même
document** sans ouvrir aucune base (`inspected: false`).

```sh
this-host --config deploy.yaml --dry-run --json | jq
curl -s localhost:8080/ | jq '.attachments[].mount, .not_attached'
```

```json
{
  "structure": "this-rs host",
  "version": "0.0.9",
  "bind": "127.0.0.1:8080",
  "inspected": true,
  "attachments": [
    {
      "name": "facturation",
      "mount": "/attached/facturation",
      "store": { "kind": "obrain", "path": "/srv/facturation/app.obrain", "read_only": true },
      "authorization": { "mode": "enforce", "policies": "/srv/facturation/config/links.yaml" },
      "identity": { "kind": "mapped", "principal": "account:{user_id}", "scope": "org:{tenant_id}" },
      "surfaces": ["/attached/facturation/health", "/attached/facturation/graph", "…"],
      "exposure": { "cors_enabled": true, "cors_origins": 1, "domains_guarded": true, "hosts_allowed": 2, "…": "…" },
      "warnings": ["base ouverte en lecture seule : aucune décision IAM ne peut y être matérialisée"]
    }
  ],
  "not_attached": [
    "routes CRUD d'entités : elles exigent un descripteur Rust (`EntityDescriptor`, donc un service de données) — l'hôte n'en invente pas depuis la configuration",
    "reverse proxy vers un projet existant : l'hôte ne transporte pas de trafic, il rattache une base et une identité"
  ],
  "notes": ["le plan ne contient aucun secret : ni politique, ni clé, ni jeton"]
}
```

Deux règles tenues par des tests :

* **aucun secret** : le plan publie le *chemin* d'un fichier déclaré, jamais son
  contenu (`the_plan_publishes_paths_never_the_contents_of_declared_files`) ;
* **`--dry-run` ne touche à rien** : il n'ouvre ni ne crée la base
  (`dry_run_publishes_the_plan_without_opening_any_base`).

---

## 4. Ce que chaque projet rattaché obtient

Monté sous son `mount`, et rien d'autre — l'hôte **n'invente aucune route** :

| Chemin | Ce que c'est |
|---|---|
| `{mount}/health`, `{mount}/healthz` | la sonde de vie du projet rattaché |
| `{mount}/graph` | le **relief** : ce que le moteur sait faire, et la forme de la base |
| `{mount}/graph/spaces` | l'espace actif et les espaces connus |
| `{mount}/graph/identity`, `{mount}/graph/identity/{id}` | le contrat d'identité canonique, et l'identité d'une entité |
| `{mount}/graph/hierarchy/{id}` | la vue **multi-échelle**, un niveau borné à la fois |
| `{mount}/graph/procedures` | la surface d'appel du moteur |

S'y ajoute l'**exposition** déclarée dans le `server.yaml` du projet : CORS,
domaines servis (tout autre `Host` est refusé en 421), locataire désigné par un
domaine, délégation `Caddyfile`, zone PowerDNS. Ses politiques `auth:` sont
lues **telles quelles** dans son `links.yaml`.

### Lire le relief d'une base rattachée : le couplage à connaître

Depuis que **l'abstention d'autorisation est un refus**, armer l'autorisation
ferme les lectures de graphe tant que le projet n'a pas déclaré de
`graph.policy` — et un 403 sur `/graph` ne s'explique pas tout seul. Le plan
l'annonce, et le comportement est verrouillé par un test :

| `mode` | `/graph*` | Quand |
|---|---|---|
| `enforce` **sans** `graph.policy` | **403** — la route s'abstient, et l'abstention est un refus | jamais : c'est un état à corriger |
| `enforce` **avec** `graph.policy` | jugé, puis **filtré** par le périmètre IAM du demandeur | la base doit porter les décisions matérialisées |
| `report_only` | 200 — les refus sont observés et publiés, rien n'est bloqué | adoption d'une base en service |
| `off` | 200 — relief descriptif et **public** | publier une base |

Et le couplage qui surprend : une base ouverte en **lecture seule** ne peut pas
recevoir de décisions IAM matérialisées (une politique `perm:` est tranchée par
une décision **dans cette base**). `enforce` + `graph.policy` + `read_only: true`
n'est donc pas satisfiable par cet hôte seul : c'est l'écrivain de la base qui
doit avoir matérialisé les décisions — ou l'opérateur publie en `off`. Le plan
publie cet avertissement (`aucune décision IAM ne peut y être matérialisée`).

### Ce que la structure ne rattache pas (et qu'elle publie)

1. **Les routes CRUD d'entités.** Elles viennent d'un `EntityDescriptor`, donc
   d'un service de données — écrit en Rust. Un projet qui veut ses entités
   servies par l'hôte garde son `Module` et appelle la bibliothèque
   (`ServerBuilder`), ou fournit le descripteur. La configuration **ne l'invente
   pas**.
2. **Le transport.** L'hôte ne proxifie pas : mettez-le derrière votre proxy
   (Caddy), qui route vers lui **et** vers le projet existant.
3. **Les décisions IAM quand la base est en lecture seule.** Une base ouverte en
   `read_only: true` ne peut pas recevoir de décision matérialisée. L'hôte le
   publie dans les `warnings` de l'attachement — et c'est le mode à préférer
   pour *observer* une base en service.

---

## 5. Parcours de rattachement

```sh
# 1. Déclarer, et relire le plan SANS toucher aux bases
cat > deploy.yaml <<'YAML'
bind: 127.0.0.1:8080
attachments:
  - name: facturation
    store:
      kind: obrain
      path: /srv/facturation/app.obrain
      read_only: true
    exposure: /srv/facturation/config/server.yaml
YAML
this-host --config deploy.yaml --dry-run

# 2. Mettre en service en LECTURE SEULE, et observer
this-host --config deploy.yaml
curl -s localhost:8080/attached/facturation/graph | jq '.engine'
curl -s 'localhost:8080/attached/facturation/graph/identity?counts=true' | jq

# 3. Ajouter les politiques du projet, en OBSERVATION
#    (authorization.mode: report_only) : rien n'est bloqué, tout est publié
curl -s -D - localhost:8080/attached/facturation/graph | grep x-this-rs-authz

# 4. Armer (mode: enforce), puis — si le projet doit écrire ses décisions IAM —
#    passer read_only: false, en sachant que le bail d'écriture est alors pris.
```

---

## 6. Vérifier une structure

```sh
cargo test --test deploy_host                       # sans obrain : le memory store
cargo test --features obrain --test deploy_host     # avec une vraie base rattachée
cargo test --lib deploy::                           # la validation, et le plan
```

Les tests couvrent : le plan publié, les sondes de vie, l'absence de route
inventée hors d'un `mount`, l'exposition d'un projet rattaché **effective et
publiée en faits**, `--dry-run` qui n'ouvre rien, et — avec `obrain` — une base
écrite par un autre outil, rattachée en lecture seule, dont le relief répond et
dont le **compteur de nœuds ne bouge pas**.
