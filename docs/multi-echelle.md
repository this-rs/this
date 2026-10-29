# La vue multi-échelle

**Voir les sous-groupes des sous-groupes des sous-groupes** — sans charger le
graphe, et sans jamais laisser croire qu'on a tout montré.

```bash
GET /graph/hierarchy/42?depth=3&limit=50          # JSON
GET /graph/hierarchy/42?depth=2&format=outline    # texte, pour un moteur d'inférence
```

```
aletheia (2 enfant(s))
  - g1-0 (2)
    - g2-0 (2)
    - g2-1 (2)
  - g1-1 (2)
    ↳ /graph/hierarchy/17?depth=1
```

## Le principe : descendre d'un cran à la fois

Au-delà de deux ou trois niveaux, « donne-moi tout » explose et personne ne lit.
La vue répond donc : **un niveau par appel**, **combien il y a en dessous**
(`children`), et **où continuer** (`drill_down`).

| Garantie | Comment |
|---|---|
| **Bornée** | `depth` ramenée à 32 au plus, `limit` à 1 000 ; défaut 5 niveaux / 50 nœuds |
| **Sans silence** | chaque niveau publie `total` et `truncated` : on ne peut pas confondre « c'est tout » avec « on a coupé » |
| **Sans boucle** | un nœud déjà visité n'est pas redescendu ; un cycle est **signalé** (`cycles`), jamais parcouru |
| **Par adjacence** | seules les arêtes du nœud courant sont lues, filtrées par type : le coût suit ce qui est **montré**, pas la taille de la base |
| **Rien d'invisible n'est payé** | seuls les nœuds affichés sont descendus au niveau suivant |

## Ce qui compte comme « contenance »

Par défaut : `CONTAINS`, `HAS_SUBGROUP`, `PART_OF`, `SUBGROUP_OF`, `MEMBER_OF`.
Une application qui nomme autrement le dit :

```rust
use this::graph::hierarchy::{describe, HierarchySpec};

let vue = describe(&store, node, HierarchySpec {
    edge_types: vec!["REGroupe".into()],
    max_depth: 5,
    per_level_limit: 50,
});
```

Une arête d'un autre type (`CITES`, `OWNS`…) **ne crée pas** de sous-groupe :
c'est testé.

## Pour un moteur d'inférence

`format=outline` rend la même information en texte indenté, avec l'adresse à
demander à chaque branche non descendue. Le modèle explore par étapes :

1. `?depth=1&format=outline` → les enfants directs, et leur nombre d'enfants ;
2. il choisit une branche et demande `/graph/hierarchy/<enfant>?depth=2` ;
3. il s'arrête quand `children` vaut 0, ou quand `truncated` est vrai — il sait
   alors qu'il reste des nœuds, **et combien**.

La vue est **la même requête à toutes les échelles**, et elle dit toujours ce
qu'elle a coupé.
