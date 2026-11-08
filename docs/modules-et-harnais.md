# Ce qu'un module peut étendre — et ce qu'il ne peut pas encore

Un module déclaratif ne devrait pas être un invité : il devrait pouvoir **tout**
faire dans l'hôte qui l'installe — écrire un outil, dessiner une page, occuper un
menu, poser un composant, agir sur le harnais qui l'exécute. Ce document dit, pour
chaque point d'extension, ce qui marche **aujourd'hui** et ce qui manque, avec le
fichier qui le prouve.

Mesuré le 08/11/2026.

| Point d'extension | État | Preuve |
|---|---|---|
| **Pages** (surface `main`) | ✅ | `atelier/src/ecran.rs` — `Portfolio::at(Surface::Main)` |
| **Vues** (`header`, `panel`) | ✅ (livré ce jour) | `ecran.rs` : même moteur de briques, seule la place change |
| **Menus** (surface `sidebar`) | ⚠️ **liste seulement** | `atelier/src/lib.rs::vues_laterales` saute toute vue dont la racine n'est pas `Leaf{List}` |
| **Composants** (17 briques, 6 conteneurs) | ✅ | `this-ui/src/views/render.rs` rend les 17 |
| **Outils** (les programmes d'un module deviennent des outils nommés) | ✅ | `atelier/src/harness.rs::outils_des_modules` |
| **Représentations dans le chat** | ❌ | la brique `Chat` dessine un fil *dans une vue* ; rien n'injecte dans le fil de l'app |
| **Agir sur son harnais** | ❌ | 33 outils, aucun ne change les bornes, le modèle ou le consigne du harnais |
| **Widget / LockWidget / Activity / Island** | ⚠️ | surfaces déclarables, rendues par un autre processus ; l'extension iOS de l'Atelier ne lit pas encore les vues de module |

## Les intentions : quatre sur quinze

Une vue déclare des actions (`Intent`). L'hôte en exécute **quatre** :

| Intention | État |
|---|---|
| `Create`, `Update`, `Delete`, `Link` | ✅ exécutées (`ecran.rs::executer`) |
| `Run` (lancer un programme du module) | ❌ **refusée** — alors que le programme est déjà joignable par l'outil `run_program` |
| `Send` (poster dans le fil) | ❌ refusée |
| `Tool` (piloter l'agent) | ❌ refusée |
| `Capability` (demander une autorisation) | ❌ refusée — la porte existe pourtant (`gate`) |
| `Invoke`, `RequestAccess`, `Reload`, `Select`, `Open`, `Edit`, `Navigate` | ❌ refusées |

Une intention refusée répond « pas branché » : c'est honnête, mais ça veut dire
qu'un bouton déclaré par un module ne fait rien de plus que créer ou modifier une
ligne.

## Ce que « tout faire » demande, dans l'ordre

1. **Menus riches** — dessiner une vue `sidebar` avec le même moteur que `main`
   quand sa racine n'est pas une liste (le chemin rapide reste pour les listes).
2. **`Intent::Run`** — un bouton qui lance un programme du module. La mécanique
   existe (`run_program`) ; il faut l'extraire de `run_tool` pour l'appeler depuis
   `ecran::executer`.
3. **`Intent::Send`** — poster dans le fil de l'app depuis une vue de module.
4. **Représentations dans le chat** — une manière déclarative pour qu'un module
   ajoute une bulle ou un composant à une conversation (par exemple une brique
   `Result` rendue dans le fil), pas seulement dans sa page.
5. **Agir sur son harnais** — un ou deux outils explicites, bornés et consentis :
   lire sa configuration, la modifier. Aujourd'hui un module ne peut rien dire du
   harnais qui l'exécute.
6. **`Intent::Capability`** — une vue de module qui demande un accès appareil : la
   porte (`this_platform::device_bridge::gate`) et la carte des capacités existent,
   l'intention reste à brancher.

Chaque point est vérifiable : la preuve est un fichier et une ligne, pas une
intention. Tant qu'un point est ❌ ou ⚠️, « un module peut tout faire » est faux, et
ce document doit le dire.
