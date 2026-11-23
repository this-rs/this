# Catalogue d'outils d'auteur — le harnais construit, retrouve, versionne et renforce ses outils

Un agent qui sait écrire ses outils en écrira trop : le même, sous trois noms, avec trois
descriptions que le modèle relira à chaque tour. Ce document dit ce que `this-harness` fait
pour l'en empêcher, ce qui est **mesuré** et ce qui ne l'est **pas**.

Mesuré le 24/11/2026 sur `origin/main` = `4d75e48` + cette branche. Les fichiers :
`crates/this-harness/src/catalogue.rs` (noyau pur), `src/catalogue_toolbox.rs` (la boîte à
outils), `tests/catalogue.rs` (38 tests), `tests/catalogue_reel.rs` (moteur réel, opt-in).

## 1. Ce qu'est un outil d'auteur

Une **recette** : une suite d'au plus 8 appels à des outils que l'hôte offre **déjà**, dont les
arguments reprennent les paramètres de l'appel (`"{{plan}}"`). **Jamais du code.** Un outil
d'auteur ne peut donc rien que ses étapes ne pouvaient déjà : le plafond de capacités est
hérité, jamais élargi.

```text
tools_author create  nom=vue_du_plan  steps=[ task_list{plan:"{{plan}}"}, note_add{text:"vu {{plan}}"} ]
        └─ le modèle appelle ensuite  custom_vue_du_plan{plan:"p7"}
```

Trois noms, et seulement trois, entrent dans la liste d'outils du modèle :

| Nom | Rôle | Effet déclaré |
|---|---|---|
| `tools_find` | `search` / `show` / `history` | `Read` |
| `tools_author` | `create` / `edit` / `rollback` / `retire` | `Note` (l'agent écrit pour lui-même : jamais une preuve d'action sur le monde) |
| `custom_<nom>` | un outil d'auteur **chargé** | le plus fort de ses étapes |

**Pourquoi `_` et pas `/`.** Un moteur compatible OpenAI refuse tout nom hors de
`^[a-zA-Z0-9_-]+$` par une 400. La première version de ce chantier nommait `tools/find` ;
**32 tests passaient** (le transport factice ne valide rien) et le premier essai sur un moteur
réel a échoué (§4). Le test `chaque_nom_annonce_est_valide_pour_un_moteur_reel` garde
désormais ce que seul le moteur voyait. Le préfixe `custom_` est **réservé** : un `custom_…`
qui n'est pas un outil actif n'est jamais transmis à l'hôte.

## 2. Ne pas recréer à tout va

Le catalogue refuse, et chaque refus **dit quoi faire à la place** (un modèle relancé sans
motif répète la même action) :

| Cas | Refus | Se lève par |
|---|---|---|
| Nom déjà pris | `name_taken` → « modifie-le avec `tools_author action=edit` » | rien |
| Recette identique à celle d'un outil actif, sous un autre nom | `duplicate` | rien |
| Outil trop proche (cosinus lexical ≥ 0,6) | `similar` + les trois plus proches avec leur score | `confirm_distinct=true` |
| Catalogue plein (200) | `full` → « retire un outil inutilisé » | `retire` |

La confirmation ne lève **jamais** un nom pris ni une recette identique : seul le « proche »,
qui est un jugement, se discute.

## 3. Économie de jetons : la surface minimale, le reste à la demande

Annoncer tous les outils d'auteur à chaque tour, c'est payer leurs descriptions à chaque tour.
`specs()` ajoute donc **deux** méta-outils et, au plus, 8 outils d'auteur **chargés** : une
recherche charge les 3 meilleurs, une création charge l'outil créé, un appel le rafraîchit,
le moins récemment utilisé sort. Le harnais refuse un outil non offert à ce tour
(`session.rs`) : **chargé = appelable**, et un outil qu'on vient de chercher s'utilise au tour
**suivant**.

**Mesuré** (`le_gain_de_jetons_est_mesure_et_la_surface_reste_constante`, octets de la liste
d'outils sérialisée, pas des jetons — l'ordre de grandeur est ~4 octets par jeton, non mesuré
ici) :

| 40 outils d'auteur | octets ajoutés à la liste d'outils |
|---|---|
| tout annoncer | 16 585 |
| à la demande (rien de chargé, index compris) | 1 485 — **9 %** |

La surface totale de départ (5 outils d'hôte + 2 méta-outils + index) vaut 1 868 octets avec
40 outils au catalogue et 1 882 avec 60 : elle **ne grandit pas** avec le catalogue, parce que
l'index est plafonné.

### L'index : ce que le chargement paresseux cachait

Le chargement paresseux économise la liste d'outils, mais il **cache aussi le catalogue** :
un modèle à qui rien ne dit qu'un outil existe ne le cherche pas. Mesuré sur un moteur réel
(§4), sans index la session 2 refaisait à la main ce que l'outil faisait. `tools_find` porte
donc, dans sa description, l'**index** du catalogue : les noms (jamais les descriptions) des 12
outils les plus récemment créés ou modifiés, et « … et N autres ». Trié par récence de
**version**, jamais par usage : il ne change que lorsque le catalogue change. La liste d'outils
est le préfixe du prompt, et un index qui bouge à chaque appel casserait le cache du
fournisseur (le nonce des conteneurs est, pour la même raison, tiré **une fois par boîte**).

## 4. Ce qui est mesuré sur un moteur réel — et ce qui ne l'est pas

`tests/catalogue_reel.rs`, opt-in (`#[ignore]`, `THIS_RACCORDEMENT=deepseek`, clé injectée par
le coffre, jamais lue par le test). Moteur : `deepseek-flash`. Deux sessions : un catalogue
vide, puis un **autre** harnais sur le catalogue rangé, la même demande sur un autre plan.

| Condition | Session 1 (vide) | Session 2 (catalogue relu) |
|---|---|---|
| Sans index, 1 passe | cherche → crée → utilise | **ne cherche pas**, refait à la main |
| Avec index, 3 passes sur 3 | cherche, crée, utilise (l'ordre varie d'une passe à l'autre) | **cherche → réutilise**, 0 outil créé en plus |

Ce que cela **ne prouve pas** : un seul modèle, un seul scénario, 3 passes avec l'index et 1
sans — un échantillon à confirmer, pas une statistique. Le comportement n'est donc **pas
asserté** (il est stochastique) ; seuls les invariants de **sûreté** le sont (aucun appel hors
de l'hôte, aucun doublon de recette au catalogue). Il imprime des lignes
`RACCORDEMENT catalogue …`. Aucun test ne mesure encore ce qui se passe avec un catalogue de
plusieurs dizaines d'outils et une demande ambiguë.

## 5. Versions, édition, renforcement

- **Append-only.** Éditer crée une version ; revenir en arrière crée une version qui **copie**
  l'ancienne ; rien n'est réécrit. 20 versions gardées par outil : au-delà la plus ancienne
  sort, la courante jamais, et `history` dit combien sont sorties.
- **Verrou optimiste.** `edit` et `rollback` exigent `expected_version` (celle que le modèle a
  lue) : une version périmée est refusée avec « relis-le avant de le modifier ».
- **Renforcement par l'usage, par version.** Chaque appel compte (réussi, échoué, refusé). La
  fiabilité (lissée de Laplace : un outil jamais appelé vaut 0,5) départage la recherche :
  `pertinence × (0,75 + 0,25 × fiabilité)`. Un outil qui a servi au moins 3 fois et échoue plus
  qu'il ne réussit est signalé **FRAGILE**. Une version neuve repart de zéro. Une erreur
  d'arguments du **modèle** ne compte pas contre l'outil.
- **Retrait.** Jamais une suppression : l'historique reste lisible, et un `rollback` réactive.

## 6. Sûreté — chaque garde, son test, et sa mutation

Un test qui passe du premier coup ne prouve rien : chaque garde ci-dessous a été **mutée**
(31 mutations, script de campagne : appliquer, lancer, restaurer) et au moins un test tombe
pour chacune — 31 sur 31 tuées sur le code livré. La méthode et la relecture ont trouvé trois
choses, toutes corrigées : la fiabilité n'entrait pas réellement dans l'assertion (l'ordre de
repli, la récence, donnait la même réponse) ; la règle « une étape ne vise pas le catalogue »
n'était jamais atteinte (la doublure n'offrait jamais ces noms) ; et un contrôle de `run_recipe`
était devenu du code mort depuis que le dispatch filtre les outils actifs (une mutation y
survivait : il a été supprimé, la définition résolue une fois est passée à la recette).

| Garde | Ce qu'elle empêche |
|---|---|
| Chaque étape passe par la boîte de l'hôte | Un outil d'auteur n'élargit aucun droit |
| Effet déclaré = le plus fort des étapes ; **étape non déclarée = `Write`** | Une recette qui lit **et** exécute n'est jamais prise pour une lecture |
| `grants()` : accordé **seulement si toutes** les étapes le sont | Même l'étape 1, accordée, ne part pas si l'étape 2 est refusée |
| Pas de récursion : une étape ne vise ni `tools_*` ni `custom_*` | Un outil qui s'écrit un outil |
| **Pas de `{{prev}}`** : la sortie d'une étape n'entre pas dans les arguments de la suivante | Le gate juge le **nom** d'un outil, pas ses arguments : un texte hostile rendu par un premier outil piloterait le second (confused deputy) |
| Schéma de paramètres réduit à `type` + `required` | Un `description` d'auteur entrerait dans la liste d'outils **hors** conteneur |
| Descriptions, index et résultats **enfermés** (`untrusted`) | Une description hostile ne sort pas de son conteneur |
| Échec du stockage ⇒ l'opération est **annulée** en mémoire | Un outil qui existe dans la session et pas sur le disque |
| `from_json` **refuse** un format inconnu ou un enregistrement incohérent | Appliquer de travers un état qu'on ne comprend pas |
| Annulation lue avant chaque étape | Une recette qui continue après `interrupt` |

## 7. Ce que ça ne fait pas

- **Pas atomique.** Une recette qui échoue à l'étape 3 a déjà agi aux étapes 1 et 2 ; le
  résultat le dit (« Les étapes 1 à 2 ont déjà agi »), il ne défait rien.
- **Recherche lexicale seulement.** « liste mes courses » ne retrouve pas « shopping » si aucun
  mot n'est partagé. Le port `Embed` existe dans le crate ; il n'est pas branché ici.
- **Pas de verrou entre sessions.** `expected_version` protège contre deux éditions dans la
  **même** boîte. Deux sessions qui partagent un stockage écrivent en « dernier arrivé gagne » :
  `CatalogStore::save` reçoit l'état entier. Un hôte qui veut un vrai verrou le met dans son
  stockage.
- **`save` à chaque appel.** Le compteur d'usage range l'état entier (borné : 200 outils × 20
  versions). Un hôte qui écrit cher regroupe les appels ; l'échec d'un compteur est ignoré
  (c'est un avis), celui d'une création annule.
- **Le préfixe `custom_` est réservé** : un hôte qui offre un outil de ce nom ne peut pas le
  voir composé dans une recette.
- **Hors périmètre, constaté.** Rien dans `this-harness` n'encode les noms d'outils pour le
  moteur : `web/search` (`search.rs`) et `serveur/outil` (MCP, `mcp.rs`) portent un `/`. Le refus
  de `tools/find` par un moteur réel est **mesuré** ; que `web/search` et les outils MCP soient
  refusés de la même façon est **déduit** du même motif, pas exécuté.

## 8. Brancher le catalogue (hôte)

```rust
let tools = Rc::new(CatalogToolbox::new(
    host_toolbox,                                   // la boîte de l'hôte, enveloppée
    Catalog::from_json(&saved).unwrap_or_default(), // l'état rangé (refuse un format inconnu)
    Some(store),                                    // un `CatalogStore` : `save(&str)`
).with_base_grants(grants.clone()));               // SI l'hôte a des autorisations statiques

let harness = Harness::new(brief, limits, transport, tools, sink).with_grants(grants);
```

- **Autorisations statiques.** Les noms `custom_*` naissent pendant la session : un
  `Harness::with_grants` seul les refuse (fail-closed, testé). Donner les mêmes autorisations à
  `with_base_grants` : la boîte les complète à chaque appel. Ce que l'hôte a écrit sur
  `tools_find` / `tools_author` fait foi ; sinon les activer vaut les accorder.
- **Le brief.** Ajouter `this_agent::untrusted::PREAMBLE` à la consigne (comme pour MCP : la
  constante existe, aucun code ne l'injecte). Les règles qui ont donné le comportement du §4,
  à adapter : *avant d'en créer un, cherche avec `tools_find` ; si un outil convient, appelle-le
  au lieu d'en créer un autre ; un outil est une suite d'au plus 8 appels à tes outils.*
- **Rejouer sur un moteur réel :**

```text
THIS_RACCORDEMENT=deepseek \
  orchestrator secret exec -e DEEPSEEK_API_KEY=deepseek-api-key -- \
  cargo test -p this-harness --features providers --test catalogue_reel -- \
    --ignored --test-threads=1 --nocapture
```

## 9. Rapport avec F2 (Atelier)

F2 (« usages composés », branche `atelier-f2`, non finie) déclare une chaîne d'outils **dans un
module**. Ce catalogue est la couche en dessous, côté harnais : il n'écrit pas de module et ne
touche pas à l'Atelier. F2 peut adopter `RecipeStep` et `render_args` (substitution typée, clé
retirée quand un paramètre optionnel est absent) au lieu d'une seconde implémentation ; ce
rapprochement n'est **pas fait** ici.
