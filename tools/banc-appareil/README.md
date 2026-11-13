# Le banc de l'appareil

Une app hôte de deux cents lignes, qui n'existe que pour une question : **sur cet
appareil-ci, quelles méthodes du pont d'appareil répondent vraiment ?**

Ni le contrat (`this-platform::device_bridge`) ni la carte du framework
(`this-agent::surface`) ne peuvent y répondre : le premier dit ce que le pont
devrait faire, la seconde ce qu'il annonce. Le troisième juge, c'est l'éditeur de
liens et le système — et il faut un appareil pour l'interroger.

## Pourquoi une app séparée

L'Atelier sait déjà faire un diagnostic, mais il l'a payé cher : 15 000 lignes,
un moteur d'embeddings, un graphe, un frontend wasm. Mesurer le pont à travers
lui, c'est mesurer les deux à la fois. Cette app ne dépend que de `tauri` et de
`tauri-plugin-this-device`, se construit en quelques minutes, s'installe et se
désinstalle sans toucher aux données de personne.

## Lancer sur un simulateur

```sh
./preparer.sh                      # une fois : gabarit Tauri + les 3 réglages Xcode manquants
cargo tauri ios build --target aarch64-sim --debug
xcrun simctl install <udid> gen/apple/build/arm64-sim/BancAppareil.app
SIMCTL_CHILD_THIS_DEVICE_BANC=1 xcrun simctl launch <udid> dev.thisrs.bancappareil
xcrun simctl get_app_container <udid> dev.thisrs.bancappareil data
# → <conteneur>/Documents/banc.json et banc.log
```

## Lancer sur un iPhone

`./preparer.sh` est **obligatoire** ici : sans lui, l'app se lance et meurt aussitôt
(« Library not loaded: @rpath/libswiftCore.dylib »). Voir l'en-tête du script.

```sh
cargo tauri ios build --target aarch64
xcrun devicectl device install app --device <udid> \
  gen/apple/build/banc-appareil_iOS.xcarchive/Products/Applications/BancAppareil.app
xcrun devicectl device process launch --device <udid> \
  -e '{"THIS_DEVICE_BANC":"1"}' dev.thisrs.bancappareil
xcrun devicectl device copy from --device <udid> \
  --domain-type appDataContainer --domain-identifier dev.thisrs.bancappareil \
  --source Documents/banc.log --destination ./banc.log
```

**L'appareil doit être déverrouillé** : `devicectl` refuse de lancer une app sur
un écran verrouillé (`FBSOpenApplicationErrorDomain error 7`). Et il faut que le
profil de signature connaisse l'appareil — le `developmentTeam` est dans
`tauri.conf.json`.

## Sur macOS

Le même binaire, empaqueté. **L'app empaquetée est obligatoire**, et c'est mesuré :

```sh
cargo tauri build --debug --bundles app
THIS_DEVICE_BANC=1 target/debug/bundle/macos/BancAppareil.app/Contents/MacOS/banc-appareil
# → ~/Documents/banc.json et ~/Documents/banc.log
```

Lancé **hors bundle** (`./target/debug/banc-appareil`), il meurt avant le premier mot :
`UNUserNotificationCenter.currentNotificationCenter()` lève une exception **Objective-C** depuis
un fil du pool tokio, et une exception ObjC n'est pas rattrapable depuis Rust —
`*** Terminating app due to uncaught exception 'NSInternalInconsistencyException',
reason: 'bundleProxyForCurrentProcess is nil'`. Le même binaire, empaqueté, rend son rapport.

Pour ne rien écrire dans le vrai `~/Documents`, lancer avec
`HOME=/tmp/banc-macos` (créer `Documents/` dans ce répertoire avant).

## Lire le rapport

`banc.json` porte quatre sections : `capacites` (l'état de chacune, lu sans rien
demander), `methodes` (une entrée par méthode du contrat), `systeme` (les six
lectures d'état système) et `defauts` (ce qui demande une correction du pont).

Les états d'une méthode :

| État | Sens | Défaut ? |
|---|---|---|
| `repondu` | l'appareil a répondu autre chose qu'un refus | non |
| `refuse_autorisation` | la capacité n'est pas accordée | non |
| `indisponible` | le matériel ou le système ne sait pas faire | non |
| `non_supporte` | la méthode n'existe pas ici | non |
| `refuse_argument` | le pont a refusé avant le natif, en nommant la clé | non |
| `refus` | la famille a refusé avec SON mot (`unknown_activity`, `too_large`…) | non |
| `panne` | le pont a échoué (`internal`) | **oui** |
| `delai` | le natif n'a pas répondu dans la borne | **oui** |
| `non_exerce` | le banc ne l'a pas appelée (permission, geste) | non |

`refuse_argument` n'est pas un défaut : le banc appelle chaque méthode **sans argument**,
comme un agent qui découvre l'outil ; `contactsSearch` sans `query` doit donc être refusé.
Seuls `panne` et `delai` accusent le pont — un pont sain donne `defauts =` vide.


`non_exerce` est une réponse honnête, pas un succès déguisé : le banc préfère
dire ce qu'il n'a pas mesuré plutôt que de le compter comme bon.

## Ce que le banc ne laisse jamais derrière lui

- **aucune boîte de dialogue** : avant d'appeler une lecture, il lit l'état de la
  capacité (`Request::Status`, qui ne demande rien) et ne lit la donnée que si la
  permission est déjà accordée ;
- **aucune donnée recopiée** : ni contact, ni photo, ni texte du presse-papiers.
  Le rapport ne garde que la nature, l'état, le code et la durée.

## Après la mesure

`this_agent::surface::HostLimits::from_bench(&rapport)` transforme le rapport en
limites du build : une capacité mesurée `unsupported` passe en `Missing` dans la
carte que lit l'agent, avec sa raison. Un `denied` n'est **pas** une limite — la
personne peut changer d'avis dans les Réglages.

## La passe d'écriture, opt-in

Une lecture se mesure sans rien changer. Une **écriture** crée un objet sur l'appareil, donc
elle ne part pas sans qu'on la demande :

```sh
SIMCTL_CHILD_THIS_DEVICE_BANC_ECRITURES=sures     # un fichier, une notification, une Live Activity
SIMCTL_CHILD_THIS_DEVICE_BANC_ECRITURES=visibles  # + un contact, un événement de calendrier
```

`sures` ne touche à rien de la personne : le fichier va dans le conteneur de l'app, la
notification est planifiée dans une heure **et retirée tout de suite**, la Live Activity est
terminée dans la foulée. `visibles` crée un vrai contact (« Banc Appareil ») et un vrai
événement — le contrat n'a pas de `contactsDelete`, donc **le contact reste**.

Chaque cas de `ecritures()` dit sa suite : écrire, vérifier, effacer (`notificationsPending`
puis `notificationsCancel`, `liveActivityUpdate` puis `liveActivityEnd`).

## Accorder les permissions d'un simulateur (aucune boîte de dialogue)

Sans cela, toute lecture de donnée personnelle est `non_exerce` — le banc ne pose pas la
question, c'est la règle. `simctl privacy` la donne à sa place, **après l'installation** (une
réinstallation efface les entrées TCC) :

```sh
xcrun simctl install <udid> BancAppareil.app
for s in contacts contacts-limited calendar photos photos-add microphone motion location location-always; do
  xcrun simctl privacy <udid> grant "$s" dev.thisrs.bancappareil
done
```

Mesuré : `contacts` et `calendar` passent `granted` et `contactsList`, `calendarEvents`,
`calendarCalendars` répondent ; `photos` et `location` sont restés `prompt` sur les
simulateurs iOS 18.6 et 26 — `simctl` ne les a pas accordés, et le banc le dit au lieu de
le masquer.
