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

## L'interface et les droits, de bout en bout

Un Canvas décrit ce qu'il montre par une **échelle** et un **sujet**. La lecture
de graphe savait demander une profondeur et un identifiant. Les deux parlent
maintenant la même langue :

```bash
GET /graph/hierarchy/self?echelle=canvas     # mon sujet, à mon échelle
GET /graph/hierarchy/self?format=outline     # lisible par un modèle
```

* **`echelle=canvas`** prend l'échelle que l'utilisateur a sous les yeux ;
* **`self`** prend le sujet qu'il regarde — résolu par son `qualified_name` dans
  l'espace servi ;
* **la décision IAM plafonne** : une interface qui demande `noeud` quand le droit
  s'arrête à `groupe` reçoit **403**, avec `finest` qui dit jusqu'où le droit va ;
* **l'état est relu à chaque requête** (pas de cache) : changer d'échelle change
  la lecture ;
* **rien n'est inventé** : pas d'état d'interface branché → `501` ; pas de sujet
  déclaré → `404` ; pas d'identification → `401` ; sujet ambigu (deux nœuds du
  même nom) → `409`. Jamais « partir de nulle part ».

Le framework **ne connaît pas** l'implémentation de l'état d'interface : il
déclare le contrat `CanvasState` (`scale_of`, `subject_of`), et l'application
l'implémente. C'est une couture : l'interface peut évoluer — faces, panneaux,
sujet enrichi — sans que la lecture de graphe change, et l'inverse est vrai
aussi. Une implémentation en mémoire (`SimpleCanvasState`) sert aux tests et à
l'exemple.

```bash
cargo run --features obrain --example canvas_inference
```

Cet exemple déroule les cinq étapes sur le routeur réel : la lecture part du
sujet, le masquage est compté, le plafond refuse, le relief suit la même
décision, et le sujet est résolu depuis l'interface — jamais depuis le chemin.

## Ce que ça coûte — mesuré, pas supposé

```bash
cargo bench --features obrain --bench scope_bench          # les chiffres
cargo bench --features obrain --bench scope_bench -- --verifier   # + les budgets
cargo test  --features obrain --test scope_budgets -- --nocapture # le garde-fou
```

| Mesure | Valeur mesurée |
|---|---|
| correspondance d'une permission — accordé | ≈ 0,03 µs |
| correspondance — **refus**, 67 permissions | ≈ 0,30 µs |
| périmètre illimité (le chemin d'avant) | ≈ 0,02 µs — **court-circuit** |
| construction d'un périmètre, 67 permissions | ≈ 7,3 µs (une fois, puis cache par époque) |
| chaîne d'une requête (décision + périmètre) | ≈ 3,9 µs |
| descente de 258 nœuds, **sans** périmètre | ≈ 0,59 ms |
| descente de 258 nœuds, **sous** périmètre | ≈ 0,85 ms (**×1,44**) |
| forme du store, sans vs sous périmètre | **×0,99** — parité |

### Ce que le banc a corrigé

| Défaut trouvé | Correction | Effet mesuré |
|---|---|---|
| `may_read_node` **allouait trois `String`** par question | comparaison de `&str`, sans allocation | refus ×1,5 · accordé **×2** · illimité ×2,5 |
| périmètre **réanalysé à chaque requête** (7,7 µs) | cache keyé par `(principal, portée, époque)` | `scope_for` **−16 %** |
| chaque nœud **lu deux fois** (visibilité puis description) | les labels lus sont réutilisés | descente filtrée **−8 %**, ratio ×1,65 → **×1,44** |

### Deux hypothèses démenties par la mesure

1. La **détection de cycle** en `Vec::contains` (O(n) par enfant) semblait le
   coupable : la remplacer par un ensemble n'a **rien** changé (573 → 575 µs). Le
   coût était dans les lectures du store.
2. Le **relief filtré** semblait deux fois plus rapide que le non filtré
   (×0,62) : c'était du bruit d'ordonnancement. La parité réelle est ×0,99.

*Les chiffres ci-dessus sont ceux du profil `release` sur cette machine ; les
ratios des tests de budget, eux, sont mesurés en debug et valent par leur
**rapport**, jamais par leur durée absolue.*