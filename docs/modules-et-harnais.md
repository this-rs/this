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
| **Parler dans le fil** (`Intent::Send`) | ✅ (livré ce jour) | `atelier/src/ecran.rs::poster` — un message `role: "module"`, attribué, borné à 4000 caractères |
| **Rendre une bulle** (slot `message`) | ❌ | un module *écrit* dans le fil ; il ne dessine pas le corps d'une bulle |
| **Agir sur son harnais** | ❌ | 33 outils, aucun ne change les bornes, le modèle ou le consigne du harnais |
| **Widget / LockWidget / Activity / Island** | ⚠️ | surfaces déclarables, rendues par un autre processus ; l'extension iOS de l'Atelier ne lit pas encore les vues de module |

## Les intentions : sept sur seize

Une vue déclare des actions (`Intent`). L'hôte en exécute **sept** :

| Intention | État |
|---|---|
| `Create`, `Update`, `Delete`, `Link` | ✅ exécutées (`ecran.rs::executer`) |
| `Run` (lancer un programme du module) | ✅ **livrée** — `lancer_programme` est sorti de l'outil `run_program` pour être joignable depuis une vue |
| `Reload`, `Select`, `Open`, `Edit`, `Navigate`, `Dismiss` | ⚪ rendues sans effet : le rendu s'en occupe, ou l'hôte n'a pas d'état à changer |
| `Tool` (piloter l'agent) | ❌ refusée — un module peut parler *à la personne*, pas encore *à l'agent* |
| `Send` (poster dans le fil) | ✅ **livrée** — attribuée au module, bornée à 4000 caractères, refusée en le disant sans conversation ouverte |
| `Capability` (enregistrer la décision de la personne sur un accès appareil) | ✅ **livrée** — avec un garde : la décision n'est enregistrée que si une question est réellement en attente (`ask`), sinon un module s'octroierait un accès que personne n'a vu demander |
| `Invoke`, `RequestAccess` | ❌ refusées |

Une intention refusée répond « pas branché » : c'est honnête, mais ça veut dire
qu'un bouton déclaré par un module ne fait rien de plus que créer ou modifier une
ligne.

## Ce que « tout faire » demande, dans l'ordre

1. **Menus riches** — dessiner une vue `sidebar` avec le même moteur que `main`
   quand sa racine n'est pas une liste (le chemin rapide reste pour les listes).
2. ~~**`Intent::Run`**~~ — **fait** : `lancer_programme` est une fonction publique,
   appelée par l'outil `run_program` et par l'intention.
3. ~~**`Intent::Send`**~~ — **fait** : la brique `chat` d'une vue de module poste dans
   le fil de l'app, au nom du module (`atelier/src/ecran.rs`, guichet `installer_fil`).
4. **Rendre une bulle** (slot `message`) — une manière déclarative pour qu'un module
   dessine le *corps* d'une bulle, pas seulement y écrire du texte.
5. **Agir sur son harnais** — un ou deux outils explicites, bornés et consentis :
   lire sa configuration, la modifier. Aujourd'hui un module ne peut rien dire du
   harnais qui l'exécute.
6. ~~**`Intent::Capability`**~~ — **fait**, avec un garde : la décision n'est
   enregistrée que si une question est en attente. Et le contrat de vues ne
   connaissait que **cinq** capacités (caméra, micro, position, écran, mouvement) :
   `DeviceCapability` porte maintenant les vingt et une du pont, `screen` et
   `device_motion` restant explicitement sans équivalent.

Chaque point est vérifiable : la preuve est un fichier et une ligne, pas une
intention. Tant qu'un point est ❌ ou ⚠️, « un module peut tout faire » est faux, et
ce document doit le dire.
