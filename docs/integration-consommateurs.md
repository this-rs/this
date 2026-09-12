# Intégrer la pile dans un autre programme

Ce document s'adresse à un programme **extérieur** : le Dashboard obrain, un
harnais, une application, un outil en ligne de commande. Il décrit **ce qui est
disponible**, sous quelle forme, et ce qui ne l'est pas.

Il existe trois façons d'intégrer, et elles ne dépendent pas l'une de l'autre :

| Voie | Quand | Ce que vous liez |
|---|---|---|
| **Bibliothèque** | votre programme est en Rust et parle la pile | `this-rs` avec la feature `obrain` |
| **HTTP** | votre programme est ailleurs, ou ne doit pas dépendre du framework | le relief publié (`GET /graph`) |
| **obrain seul** | vous lisez la donnée sans vouloir du framework | `obrain-engine` / `obrain-core` uniquement |

La troisième est la plus importante : **la donnée n'est pas prisonnière du
framework**. Les décisions d'autorisation sont des nœuds et des propriétés
ordinaires d'obrain, et un programme qui ne connaît que obrain peut les lire.

---

## 1. Par la bibliothèque

```toml
[dependencies]
this-rs = { version = "0.0.9", features = ["obrain"] }
```

```rust
use std::sync::Arc;
use this::storage::{GraphBackend, ObrainStore};

// Lecture seule : un tableau de bord n'a pas à prendre le bail d'écriture,
// donc il ne s'oppose pas à l'application qui écrit.
let store = ObrainStore::open_read_only("/chemin/vers/le/store")?;

// Les décisions : publiques, versionnées par `IAM_SCHEMA`.
let decision = store.iam().decision(principal, scope)?;

// La question du chemin chaud, sans reconstruire la logique :
let peut_ecrire = store.iam().allows(principal, scope, "orders:write")?;

// La façade de bascule entre les deux moteurs (`ObrainDB` ↔ `ObrainStore`) :
let nœuds = <ObrainStore as GraphBackend>::nodes_by_label(&store, "Acl");
let propriétés = <ObrainStore as GraphBackend>::node_properties(&store, nœuds[0]);

// Le bail : lisible, et **vérifiable avant d'écrire par vos propres moyens**.
let bail = store.lease_info();
store.check_write()?;   // à appeler avant toute écriture menée hors des services
```

**Points d'entrée stables** (ré-exportés, donc pas de chemins profonds à
connaître) :

| Ce que vous cherchez | Import |
|---|---|
| la décision, son stamp, ses erreurs | `this::storage::{Iam, AclStamp, IamError, RoleGrant, IAM_SCHEMA}` |
| le store et les services | `this::storage::{ObrainStore, ObrainDataService, ObrainLinkService}` |
| la façade de bascule | `this::storage::GraphBackend` |
| l'identité canonique | `this::core::identity::{IdentityRegistry, EntityKey, Qualifier}` |
| le serveur | `this::server::{ServerBuilder, RestExposure}` |
| le relief | `this::graph::{graph_routes, GraphState}` |

**Ce qui n'est pas public, volontairement** : `ObrainStore::session()` (la
session **racine**, qui ignorerait l'espace courant — passez par `GraphBackend`,
cloisonné par espace), et l'interne du moteur. Si vous avez besoin d'autre chose,
c'est un manque à signaler, pas un interdit à contourner.

---

## 2. Par HTTP — le relief

`GET /graph` publie l'état du store **et le contrat de données** :

| Route | Ce qu'elle rend |
|---|---|
| `/graph` | le relief complet : moteur, espace, procédures, identité, bail, journal, **contrat IAM**, pièges, notes |
| `/graph/identity` | le contrat d'identité ; `?counts=true` ajoute un comptage **borné** |
| `/graph/identity/{id}` | la chaîne d'identité d'une entité : motif retenu, octets canoniques, uuid |
| `/graph/spaces` | les espaces nommés du store |
| `/graph/procedures` | les procédures du registre, introspectées |

Le relief est **descriptif**, il n'affirme rien qu'il ne puisse prouver : chaque
piège est adossé à `fichier:ligne`, et chaque champ dit s'il est *introspecté* ou
*seulement déclaré*.

### Le contrat IAM, publié

`GET /graph` → `iam` :

```json
{
  "schema": "this-rs/iam/2",
  "acl_index": true,
  "decisions": 1,
  "hot_path_property": "acl_perms_blob",
  "labels":   [{ "name": "Acl", "form": "nœud de décision", "meaning": "…", "written": true }],
  "properties": [{ "name": "acl_perms", "form": "json (liste de chaînes)", "meaning": "…", "written": true }],
  "edges":    [{ "name": "HAS_ROLE", "form": "Principal → Role", "meaning": "…", "written": false }],
  "trust": ["Qui peut écrire dans le store…"]
}
```

`written: true` = le framework **écrit** ce champ. `written: false` = il le
**lit** quand il est là (c'est alors à l'application de le tenir). Un
consommateur sait donc exactement à quoi il peut se fier, sans lire le code du
framework.

---

## 3. Par obrain seul — lire la donnée sans le framework

Les décisions sont des nœuds, les rôles et permissions des propriétés. Un
programme qui ne dépend que d'obrain les lit directement — c'est vérifié par un
test (`tests/consumer_contract.rs`, `an_obrain_only_reader_can_read_the_decisions`).

### Le schéma

Une **décision** est un nœud de label `Acl`, une par couple *(principal,
portée)* :

| Propriété | Forme | Sens |
|---|---|---|
| `acl_key` | texte, **indexé** | la clé du couple : `portée` + US (0x1F) + `principal` |
| `acl_scope` | texte | la portée (locataire, organisation, espace…) |
| `acl_principal` | texte | le principal |
| `acl_roles` | JSON (liste de chaînes) | les rôles effectifs, triés et dédupliqués |
| `acl_perms` | JSON (liste de chaînes) | les permissions effectives — **la forme à afficher** |
| `acl_perms_blob` | texte (permissions encadrées par US) | la forme lue à **chaque requête** par l'application des politiques |
| `acl_epoch` | entier | l'époque, strictement croissante : l'invalidation ne recule jamais |
| `acl_digest` | texte | empreinte du contenu ; vidée à l'invalidation |

Un nœud de label `Principal` porte le principal (pour la traversée et la
traçabilité) : **il ne porte pas la décision**.

Le graphe d'administration `Principal -[:HAS_ROLE]-> Role -[:GRANTS]-> Permission`
est **lu** par la traversée de référence quand il est présent ; la décision
matérialisée ne l'écrit pas — elle porte les rôles en propriété. C'est publié
ainsi (`written: false`) pour qu'un lecteur ne le cherche pas en vain.

### Lire, en obrain

```rust
// 1. Par l'index : la clé est (portée, principal) joints par un US.
let key = format!("{scope}\u{1f}{principal}");
let nodes = db.find_nodes_by_property("acl_key", &Value::from(key.as_str()));

// 2. Ou par label, pour lister ce qui existe.
let decisions = db.nodes_by_label("Acl");

// 3. Les champs : `acl_perms` est du JSON, `acl_epoch` un entier.
let perms: Vec<String> = serde_json::from_str(
    node.get_property("acl_perms").and_then(|v| v.as_str()).unwrap_or("[]")
)?;
```

**Trois pièges à connaître**, tous vérifiés par des tests :

1. **Les formes diffèrent.** `acl_epoch` est un **entier**, `acl_roles` et
   `acl_perms` sont des **listes JSON**. Un lecteur qui attend du texte partout
   lit `null` et croit la propriété absente.
2. **Ce n'est pas indexé par défaut.** Sans l'index de propriété sur `acl_key`,
   une recherche par clé devient un balayage complet (mesuré : minutes sur un
   gros graphe). Le relief publie `acl_index` pour que vous sachiez.
3. **La portée d'abord.** La clé commence par la portée : deux locataires ne
   peuvent pas se répondre. Ne comparez jamais seulement `acl_principal`.

---

## 3 bis. Quelles surfaces appliquent vos politiques

Ce n'est pas une note de bas de page : c'est la question qui décide si vos
`auth:` protègent quelque chose. La réponse est **publiée** par le relief
(`surfaces`), et vérifiée par des tests.

| Surface | Chemins | Politiques appliquées ? | Sinon, ce qui protège |
|---|---|---|---|
| **REST** | `/{plural}`, `/{plural}/{id}`, `/{plural}/{id}/{route}`, `/{plural}/{id}/{cible}/{id}`, `/links/{id}`, `/events/stream`, `/notifications/{user_id}/…`, `/device-tokens/{user_id}`, `/graph…`, `/gdpr/…`, `/auth/…`, `/health` | **oui** — pluriel + méthode + nombre de segments → opération → politique | les chemins hors routes d'entité s'abstiennent : `/health` et `/auth/*` sont publics par nature ; `/graph/*` est descriptif ; `/gdpr/*` exige un administrateur **dans le handler** ; `/notifications/*` et `/device-tokens/*` comparent l'identifiant du chemin à celui de l'appelant ; `/events/stream` est borné au locataire de l'appelant |
| **GraphQL** | `/graphql`, `/graphql/playground`, `/graphql/schema`, `/graphql/ws` | **non** | le framework **refuse de la monter** si des politiques sont armées ; l'application doit accepter explicitement (`build_router_unenforced`) |
| **gRPC** | le routeur tonic | **non** | même refus, même acceptation (`build_with_grpc_unenforced`) |
| **WebSocket** | `/ws` | **non** | même refus, même acceptation |
| **Frontend** | `/*` (repli) : assets, SPA, `/__dev/*` | **non** | c'est un **repli** : il n'est pas résolu par route, et surtout **`Frontend::attach` remplace le repli**. Appelé APRÈS vos couches, ni l'authentification ni l'autorisation ne voient le SPA ni `/__dev` (mesuré : `tests/frontend_surface.rs`). **Montez le repli avant d'appliquer les couches.** |
| **Pont de commandes** | `/__dev`, `/__dev/command*` | **non** | **fermé par défaut** : il exécute la commande de reconstruction sur demande HTTP, et l'ouvrir demande `THIS_DEV_COMMANDS=true`. À réserver au développement local. |

**Le framework ne pose aucune couche CORS** : cette politique appartient à
l'application. Ne croyez pas être protégé — ou exposé — par quelque chose que
vous n'avez pas écrit.

**Pourquoi GraphQL/gRPC/WebSocket ne peuvent pas appliquer les politiques** :
elles ne sont pas résolues par route. Le middleware REST lit le pluriel, la
méthode et le nombre de segments pour en déduire une opération ; une requête
GraphQL ne dit rien de tout cela — le contrôle devrait être fait **par
opération**, dans les résolveurs. En attendant, le choix du framework est de
**refuser** plutôt que de publier des entités sans leur politique.

Si vous montez ces surfaces avec l'acceptation explicite, sachez ce que cela
veut dire : **un client GraphQL atteint vos entités en lecture ET en mutation**
sans passer par les `auth:` déclarées. Traitez ces surfaces comme un accès
interne, ou appliquez le contrôle vous-même.

## 4. La frontière de confiance — à connaître avant de s'appuyer dessus

Publiée dans le relief (`iam.trust`), et **testée comme telle** dans
`tests/security_iam.rs` et `tests/security_durability.rs` :

* **Qui peut écrire dans le store peut écrire une décision, donc accorder des
  permissions.** La protection est la maîtrise du store **et** le bail
  d'écriture — pas une signature. Un tableau de bord doit donc ouvrir le store
  en lecture seule, ou n'être exposé qu'à des lecteurs de confiance.
* **Deux nœuds portant la même `acl_key`** rendent la décision **ambiguë : elle
  est refusée**, jamais résolue par l'ordre de l'index.
* **Lire ce graphe, c'est lire qui peut quoi.** La liste des décisions est
  sensible : ne la publiez pas plus largement que le store lui-même.
* **Le journal n'a pas de chaîne de hachage** : une entrée supprimée laisse un
  trou **visible** (`seq` manquant), jamais une preuve.
* **Le bail n'authentifie pas son jeton** : une époque abaissée dans le fichier
  est acceptée. La maîtrise du répertoire est la protection réelle.

---

## 5. Ce qui est figé, et ce qui peut changer

**Figé par des tests, donc stable pour vous** : le nom des labels et
propriétés ci-dessus, `IAM_SCHEMA` (`this-rs/iam/2`), le format de la clé
d'ACL, la forme de la trame de bail (128 octets, CRC32), les enveloppes du
relief.

**Peut évoluer sans casser le contrat** : les coûts (chacun est mesuré et
publié), les champs *ajoutés* au relief, et l'interne du chemin chaud — par
exemple `acl_perms_blob` existe pour la performance, et `acl_perms` reste la
forme à lire pour afficher.

**Politique de changement de schéma** : un changement de *forme* des décisions
incrémente `IAM_SCHEMA` **et** le publie dans le relief ; un changement de nom
casse un test du dépôt avant d'atteindre vos programmes
(`the_published_contract_matches_the_schema_constants`).
