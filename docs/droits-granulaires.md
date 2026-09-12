# Droits granulaires — ce que la connaissance **tisse**

Une permission `orders:write` dit « écrire des commandes ». Ce qu'une base de
connaissance fait vraiment, c'est **tisser** : relier des nœuds par des relations
typées, dans des espaces nommés, sur des labels précis. Donner ce droit « en
bloc » oblige à choisir entre trop et trop peu.

Une permission est donc **structurée** :

```text
famille ":" action [ ":" cible ]
```

| Permission | Ce qu'elle accorde |
|---|---|
| `graph:read` | lire le relief, la vue multi-échelle, l'identité |
| `node:read:Page` | lire les nœuds du label `Page` |
| `node:read:*` | lire les nœuds, quel que soit le label |
| `node:write:Lemma` | créer ou modifier des `Lemma` |
| `node:delete:*` | supprimer des nœuds |
| `edge:read:CITES` | lire les relations `CITES` |
| **`edge:create:CITES`** | **tisser** : créer des relations `CITES` |
| `edge:delete:*` | défaire des relations |
| `space:use:lab` | opérer dans l'espace nommé `lab` |
| `node:*:Page` | toutes les actions de la famille, sur ce label |
| `*` | tout (vocabulaire d'avant, conservé) |

## La règle de correspondance

Une permission **détenue** accorde une permission **demandée** si les familles et
les actions sont égales, **et** si la cible détenue est `*` ou égale à la cible
demandée.

* `*` détenu → tout ;
* `famille:*` → toutes les actions de cette famille ;
* **une permission sans cible ne couvre pas une demande avec cible** :
  `node:read` n'accorde **pas** `node:read:Page` (il ne dit pas quel label) ;
* **ce qui n'est pas reconnu n'accorde rien** : `node:read:` ou `Node:Read` sont
  refusés, jamais interprétés au plus large.

**Le tissage se donne séparément de la lecture** : `edge:read:CITES` permet de
lire les relations, **pas** de les créer. `edge:create:CITES` permet de tisser,
**pas** de lire les nœuds.

## Les échelles de l'interface deviennent des droits

Un Canvas décrit ce qu'il montre par une **échelle de descente** — `cosmos`,
`monde`, `groupe`, `noeud`. Le framework parle la **même langue** : plutôt que
d'inventer un second vocabulaire, il prend le sien.

| Échelle | Ce qu'elle montre | Ce que le droit permet |
|---|---|---|
| `cosmos` | l'ensemble, agrégé | voir des comptes, pas des nœuds |
| `monde` | le graphe entier | parcourir le graphe |
| `groupe` | un groupe et ses sous-groupes | descendre dans une branche |
| `noeud` | un nœud et son voisinage | lire le détail d'un nœud |

```
echelle:read:groupe        # voir JUSQU'AU groupe — pas le détail d'un nœud
```

**Un droit d'échelle est un plafond** : `echelle:read:groupe` accorde `cosmos`,
`monde` et `groupe`, mais **pas** `noeud`. On donne *jusqu'où* on peut descendre,
pas un cran isolé. Les deux orthographes `echelle` et `scale` sont acceptées
(alias anglais `monde`/`world`, `groupe`/`group`, `noeud`/`node`).

La lecture accepte l'échelle directement :

```bash
GET /graph/hierarchy/42?echelle=groupe      # l'intention
GET /graph/hierarchy/42?depth=4&limit=100   # le nombre, quand il compte plus
GET /graph/hierarchy/42?echelle=galaxie     # 400 + la liste des échelles valides
```

Une échelle inconnue est **refusée** — jamais ramenée à un défaut silencieux, ce
qui ferait croire qu'on a obéi. `Scale::descent()` est le **seul endroit** qui
traduit une intention en profondeur et en plafond.

## Ce que ça change dans les lectures

| Lecture | Sous périmètre |
|---|---|
| `GET /graph/hierarchy/{id}` | seuls les nœuds dont le label est lisible apparaissent ; une relation dont le type n'est pas lisible n'est **pas traversée** |
| `GET /graph → store` | seuls les labels lisibles et les types de relations lisibles sont publiés |
| le comptage | ce qui est masqué est **compté** (`hidden`, `hidden_labels`, `hidden_edge_types`) et **expliqué en note** |

**Le masquage est visible, jamais déduit.** Un lecteur ne peut pas deviner une
structure qu'il n'a pas le droit de voir — mais il sait qu'on lui cache quelque
chose, et combien.

Le vocabulaire est **publié** avec le contrat IAM (`GET /graph → iam.vocabulary`,
`iam.matching`) : un consommateur n'a pas à lire le code du framework pour savoir
comment écrire un droit.

## Le filtrage est appliqué, par requête

Déclarer la politique **suffit** :

```yaml
graph:
  policy: "perm:graph:read"
```

Dès qu'elle est armée, l'exposition branche un **résolveur de périmètre** : le
contexte d'authentification de la requête devient `(principal, portée)` — par le
**même mappage** que le middleware d'autorisation, jamais un second, sinon une
seule personne recevrait deux autorisations —, la décision matérialisée fournit
ses permissions, et celles-ci deviennent le périmètre de la lecture. Les deux
vont ensemble : la politique dit *qui a le droit de lire le graphe*, le périmètre
dit *ce que ce lecteur voit*.

**Le défaut est le refus.** Pas de contexte d'authentification, pas de décision
matérialisée, ou une décision illisible : le périmètre est **vide**. Un IAM qui
ne répond pas ne doit pas ressembler à un IAM qui autorise.

**Et le masquage ne fuit pas** :

* un nœud masqué par son **label** est **compté** (`hidden`) ;
* une relation dont le **type** n'est pas lisible n'est **ni parcourue ni
  comptée** — le nombre décrirait une structure hors périmètre ; le lecteur
  apprend seulement qu'il en existe ;
* le **compte d'enfants** d'un nœud passe par le périmètre : une racine n'annonce
  pas ce qu'elle cache. C'est la fuite la plus discrète — la structure sans le
  contenu — et elle est fermée par un test.

Pour remplacer le résolveur (une couture, pas une politique en dur) :
`GraphState::with_scope_resolver(...)`, ou `IamScopeResolver` avec le mappage
d'identité de l'application.
