# Inventaire du contexte cognitif : ce que le cœur porte, et ce qui revient à l'hôte

Issue this-rs/this#290. Ce document ne décrit pas une API : il dit, pour chaque source de
contexte (notes, décisions, skills, personas, ancres), **qui possède quoi**, avec les
plafonds et les provenances **réellement codés**. Chaque affirmation renvoie au code.

## 1. Ce que le cœur transporte

Un rappel est fait de **faits** (`Fact`, `crates/this-harness/src/recall.rs`) :

| Champ | Ce qu'il porte | Qui le remplit |
|---|---|---|
| `id` | l'identifiant demandé (`#kind:uuid`) | le cœur, depuis la citation du modèle |
| `kind` | la sorte (texte libre) | l'hôte |
| `fact` | un fait court, plafonné à `FACT_MAX` (240 caractères) | l'hôte |
| `as_of` | la date à laquelle le fait a été vrai, texte libre | l'hôte |
| `source` | qui l'affirme, plafonné à `SOURCE_MAX` (80 caractères) | l'hôte |

Le port est `Recall::recall(ids)` : l'hôte implémente la requête, le cœur ne fait que la
poser et rendre le message de rappel.

## 2. Tableau par source

| Source | Propriétaire | Source déclarée | Version | Périmètre utilisateur/projet | Budget codé | Politique de fraîcheur |
|---|---|---|---|---|---|---|
| **Notes** | l'hôte (store de graphe) | `source`, texte libre | **aucune** dans `Fact` | **aucun** dans le cœur | voir § 3 | **aucune** dans le cœur ; `as_of` est la seule date |
| **Décisions** | l'hôte | `source`, texte libre | aucune | aucun | voir § 3 | aucune ; `as_of` |
| **Skills** | l'hôte | `source`, texte libre | aucune | aucun | voir § 3 | aucune ; `as_of` |
| **Personas** | l'hôte | `source`, texte libre | aucune | aucun | voir § 3 | aucune ; `as_of` |
| **Ancres** (ids vus dans le fil) | le cœur extrait, l'hôte résout | — | — | — | `MAX_ENTITIES` = 40 entités interrogées, `MAX_LISTED` = 20 chemins/commandes | — |

`kind` est libre : **personas et skills passent par le même port que les notes**, sans
traitement propre. Le cœur ne distingue pas leur sorte.

## 3. Budgets codés (`recall.rs`)

- `MAX_ENTITIES` = 40 : nombre maximal d'entités interrogées et rendues.
- `MAX_LISTED` = 20 : nombre maximal de chemins ou de commandes rendus.
- `MAX_CHARS` = 8000 : taille maximale du message produit. Une `RESERVE_FIN` de 320
  caractères est gardée pour la fin : le message entier reste sous `MAX_CHARS`.
- `COMMAND_MAX`, `PATH_MAX`, `KIND_MAX`, `TOOL_NAME_MAX` : plafonds par champ.

Ces plafonds protègent le **rappel**. Le budget de **contexte** (fenêtre du modèle) est un
autre mécanisme (`context.rs`, `removable`, `fit`), qui ne sait pas ce qu'est un fait :
il compte des messages.

## 4. Provenance : ce que le cœur garantit

- Une citation `#kind:id` n'est **lue** que si elle a paru dans un résultat d'outil hors
  conteneur `<untrusted_data>` (`SeenLedger`, `cite.rs`). Un id seulement cité par une
  donnée non fiable n'est jamais présenté comme lu (`Provenance::Quoted`).
- Une citation de la réponse finale qui n'a pas été lue émet `HintKind::Unseen` (#347).
- Le résumé de compaction est enfermé en donnée non fiable, rôle utilisateur comme
  assistant (#353, #359), et reste enfermé après reprise (test `le_resume_reste_enferme_apres_compaction_et_reprise`).
- Un texte non fiable **reste** non fiable : il ne devient ni consigne système ni permission.

## 5. Ce que le cœur NE garantit PAS (à la charge de l'hôte)

- **Périmètre utilisateur/projet** : le cœur ne porte aucun identifiant d'utilisateur ni de
  projet. Un fait d'un autre projet ne peut être écarté que par l'hôte, avant `recall`.
- **Révocation d'accès et changement de projet** : invalider les faits concernés est un
  acte de l'hôte. Le cœur ne sait rien de l'accès d'une personne.
- **Fraîcheur** : aucune politique dans le cœur. `as_of` est une date, pas un seuil ; l'hôte
  décide ce qui est trop vieux pour être rappelé.
- **Version des sources** : un `Fact` n'a pas de révision. Une note corrigée après coup
  rend un fait différent sous le même `id` : seul l'hôte peut le dire.
- **Migration d'embeddings** : le cœur refuse le mélange d'espaces (`verifier_espace`,
  #350), mais ne réindexe rien : c'est l'hôte qui stocke les vecteurs.

## 6. Ce qui reste ouvert pour la parité PO

- Un scénario **identique** dans PO et dans un hôte minimal (même registre, même refus
  d'une citation jamais vue). Il n'est pas fait ici : il exige les deux hôtes.
- Un **budget mesuré** avec schémas d'outils et pièces jointes : les plafonds ci-dessus sont
  codés, pas mesurés sur un tour réel.
- L'inventaire des **propriétés cognitives** persistées et dérivées, séparément : le cœur ne
  persiste aucune propriété cognitive ; elles vivent dans l'hôte.
