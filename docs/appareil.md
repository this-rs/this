# L'appareil — un contrat, une porte, une carte

Le pont d'appareil existe pour qu'un agent puisse **faire** quelque chose sur le
téléphone : prendre une photo, lire un contact, se connecter à un capteur
Bluetooth, écrire sur une étiquette NFC. Trois couches, et une seule règle : ce
qui est promis doit être faisable.

| Couche | Où | Rôle |
|---|---|---|
| **Le contrat** | `crates/this-platform/src/device_bridge.rs` | la liste des méthodes (`METHODS`), la capacité de chacune, les arguments par défaut et ceux qui manquent. Pur : aucun natif, aucun Tauri, compilable pour wasm. |
| **Le pont** | `crates/tauri-plugin-this-device` | l'implémentation native : Swift sur iOS et macOS, derrière une seule commande Tauri (`device_call`) et une porte de consentement (`gate`). |
| **La carte** | `crates/this-agent/src/surface.rs` | ce que l'agent **lit** (`capacites`) : quelles capacités existent, à quel étage, sur quelle plateforme. |

Une méthode qui existe dans le pont mais pas dans la carte n'existe pas pour
l'agent : il répondra « pas encore » avec aplomb. C'est mesuré, et c'est ce qui
a motivé les tests de parité (§ Vérifier).

## La convention de fil

Chaque réponse est **un objet JSON avec un `kind`**. Pas de `reject` : une
erreur est une réponse, sinon un refus ressemble à une panne.

```json
{"kind":"photo","data_base64":"…","width":1280,"height":960,"bytes":98000}
{"kind":"error","code":"not_authorized"}
```

Les codes que le pont peut rendre, et ce qu'ils veulent dire :

| Code | Sens | Ce que l'appelant en fait |
|---|---|---|
| `not_authorized` | la personne n'a pas accordé la capacité | demander via `permission_request`, puis réessayer |
| `missing_argument` | un argument requis manque | le fournir (`device_grants` liste les méthodes) |
| `not_supported` | la méthode ou la capacité n'existe pas ici | changer d'approche, pas réessayer |
| `unsupported` | le **matériel** ne sait pas faire ça | idem (simulateur sans caméra, appareil sans HealthKit) |
| `unavailable` | l'appareil ne peut pas répondre maintenant | réessayer plus tard |
| `timeout` | pas de réponse dans la borne | réessayer, ou augmenter la borne |
| `too_large` | la charge dépasse ce qu'un appel accepte | découper |
| `bad_base64` | la charge n'est pas du base64 | ré-encoder |
| `cancelled` | **la personne a annulé** | ne pas réessayer sans le lui demander |
| `nothing_recording` | rien n'était en cours | faute d'appel |
| `internal` | le natif a échoué | ce n'est pas une faute d'appel ; voir le journal |

Les octets ne traversent jamais le graphe : une photo va dans le magasin de
pièces et le modèle reçoit sa taille et son sha ; un audio ou une vidéo
deviennent un **fichier** dans le conteneur de l'app, et c'est son chemin qui
circule.

## Les vingt et une capacités

`iOS` = iPhone et iPad. Un tiret veut dire « pas annoncé sur cette plateforme » —
la carte le dit, et un test le vérifie contre `macos/Sources/dispatch.swift`.

| Capacité | Méthodes | iOS | macOS |
|---|---|---|---|
| `notifications` | `notificationsStatus` `notificationsRequest` `notificationsPost` `notificationsCancel` | oui | oui |
| | `notificationsSchedule` `notificationsPending` | oui | — |
| `camera` | `captureStatus` `captureRequest` | oui | oui |
| | `cameraCapturePhoto` `cameraRecordVideo` | oui | — |
| `microphone` | `captureStatus` `captureRequest` | oui | oui |
| | `microphoneStart` `microphoneStop` | oui | — |
| `location` | `locationStatus` `locationRequest` `locationSample` | oui | oui |
| | `locationStreamStart` `locationStreamStop` `locationRecent` | oui | — |
| `location_always` | `locationStatus` `locationRequest` | oui | — |
| `clipboard` | `clipboardRead` `clipboardWrite` (texte) | oui | oui |
| | idem, mais **lien et image** (4 Mio au plus) | oui | — |
| `share` | `shareText` | oui | oui |
| | `shareItem` (lien, image, fichier) | oui | oui |
| `live_activity` | `liveActivityStatus` `liveActivityStart` `liveActivityUpdate` `liveActivityEnd` | oui | — |
| `contacts` | `contactsStatus` `contactsRequest` `contactsList` | oui | oui |
| | `contactsGet` `contactsSearch` `contactsCreate` | oui | oui |
| `photos` | `photosStatus` `photosRequest` | oui | oui |
| | `photosList` `photosRead` | oui | oui |
| `photos_pick` | `photosPick` | oui | oui |
| `files_pick` | `filesPick` `filesRead` | oui | oui |
| | `filesWrite` | oui | oui |
| `bluetooth` | `status` `bluetoothScan` | oui | oui |
| | `bluetoothState` `bluetoothConnect` `bluetoothDisconnect` `bluetoothServices` `bluetoothRead` `bluetoothWrite` `bluetoothSubscribe` `bluetoothNotifications` | oui | — |
| `gyroscope` | `status` `gyroscopeSample` | oui | — |
| `motion_activity` | `motionActivityStatus` `motionActivityRequest` `motionStepsSample` `motionDistance` | oui | — |
| `calendar` | `calendarStatus` `calendarRequest` `calendarEvents` `calendarCreateEvent` | oui | oui |
| | `calendarCalendars` `calendarUpdateEvent` `calendarDeleteEvent` | oui | oui |
| `health` | `healthStatus` `healthRequest` `healthQuantitySample` `healthWorkouts` | oui | — |
| `face_id` | `faceIdStatus` `faceIdConfirm` | oui | oui |
| `speech` | `speechStatus` `speechRequest` `speechTranscribeFile` | oui | oui |
| `nfc` | `nfcStatus` `nfcReadOnce` `nfcWrite` | oui | — |
| `local_network` | `localNetworkStatus` `localNetworkBrowse` | oui | oui |
| | `localNetworkResolve` | oui | — |

Soixante-dix-sept méthodes, et **deux formes de « non »** qu'il ne faut pas
confondre : une méthode absente de la plateforme (`not_supported`), et une
capacité non accordée (`not_authorized`).

## La porte

Aucune méthode n'est joignable sans passer par `gate` : la capacité est dérivée
de la méthode (`capability_of`), le module demandeur est nommé, et l'absence de
décision vaut refus. Une demande crée une entrée **en attente** que la personne
voit dans le fil ; elle répond, et la réponse est écrite dans le graphe.

La phrase qu'elle lit vient de `atelier/src/consentement.rs::verbe`. Elle dit
l'action la **plus forte** de la capacité, pas la plus douce :

> « L'agent veut lire tes contacts **et en AJOUTER un** »

**Ajouter une écriture à une capacité est un changement de consentement, pas un
ajout de fonctionnalité.** Un test (`the_verb_says_the_strongest_action_the_capability_allows`)
échoue si le verbe cesse de couvrir ce que la capacité fait. C'est la seule
erreur du système qui n'a aucun recours après coup : le graphe garde `granted`,
jamais la phrase lue.

## Ce qui n'est pas exposé, et pourquoi

- **L'écriture dans HealthKit.** `healthRequest` ne demande que la lecture.
  HealthKit ne permet pas de savoir ensuite ce que l'app a écrit : une valeur
  poussée par un modèle serait irréversible **et** non attribuable. Si le besoin
  apparaît, ce sera une capacité distincte, son propre consentement, et une
  confirmation à chaque geste.
- **La dictée continue.** `microphoneStart` → `microphoneStop` →
  `speechTranscribeFile` couvre le besoin ; un flux continu ajoute une session
  audio à tenir fermée sur tous les chemins, invisible sans appareil devant soi.
- **L'adresse d'une position.** `locationReverse` enverrait la position à un
  serveur d'Apple pour un gain cosmétique.

## Les autres systèmes

Le pont existe pour iOS, iPadOS, une partie de macOS et, depuis le 13/11/2026,
une partie de Linux. Sur Android, Windows et le Web, la carte ne nomme **aucune**
capacité d'appareil — et depuis le 08/11/2026 elle le **dit** (`device`, en
`Missing`) au lieu de laisser un vide que l'agent prendrait pour une ignorance.

### Ce que le backend Linux couvre exactement

Il parle aux services du bureau au lieu d'API système : BlueZ sur le bus système,
GeoClue2, `sysfs`/IIO pour le gyroscope, et le portail XDG pour la capture.
Mesuré le 13/11/2026, et c'est ce que le job `device_linux` compile et teste :

| Méthode | Linux | Comment |
|---|---|---|
| `captureStatus` `captureRequest` | oui | état réel, jamais un `granted` inventé — Linux n'a pas de dialogue à ouvrir depuis ce processus |
| `bluetoothState` `bluetoothScan` | oui | BlueZ (`org.bluez`, ObjectManager) |
| `bluetoothConnect` et suivantes | **non** | `BluetoothReply::Error { NotSupported }` |
| `locationRequest` `locationSample` | oui | GeoClue2 |
| `gyroscopeSample`, `status(gyroscope)` | oui | `/sys/bus/iio/devices/iio:deviceN` |
| `cameraCapturePhoto` `cameraRecordVideo` `microphoneStart` `microphoneStop` | **non** | la capture elle-même n'est pas branchée ; seul l'état l'est |
| `motionActivityStatus` `motionActivityRequest` | non | `unsupported` : pas de podomètre sur un bureau |
| tout le reste | **non** | `NotSupported` |

Deux conséquences qu'il ne faut pas confondre : une méthode non portée répond
`not_supported` (l'agent doit changer d'approche), et une capacité que le pont ne
sait pas interroger répond `unsupported`. Aucune des deux n'est un refus de la
personne.

**La carte nomme Linux là où il porte quelque chose, et seulement là.** Depuis le
13/11/2026 : `device.bluetooth` (scan, pas connexion), `device.camera` et
`device.microphone` (état et demande, pas capture), `device.gyroscope`, et une
entrée à part `device.location.fix` (la position, sans l'état ni le flux). Ces
quatre entrées portent la plateforme `Linux` en plus des autres, et la cinquième
n'existe que pour elle.

Ce que le modèle ne sait toujours pas dire : une même entrée ne peut pas promettre
`bluetoothConnect` sur iOS et le taire sur Linux. D'où les entrées d'aspect
(`device.bluetooth.connect`, `device.location.fix`) plutôt qu'un ajout de
plateforme à une entrée trop large — c'est le découpage que la carte pratique
depuis le début pour macOS, appliqué à Linux.

Deux contrôles tiennent cette déclaration, dans `tests/linux_surface.rs` (ils
tournent dans le job `device_linux`) :

1. `linux::SUPPORTED` est confrontée au **dispatch lui-même** : pour chaque
   méthode du contrat, `call` doit refuser exactement celles qui n'y sont pas ;
2. une entrée de la carte qui nomme `Linux` ne peut citer que des méthodes de
   `SUPPORTED` — et `status` que pour les quatre capacités dont le backend lit
   l'état (`linux::STATUS_CAPABILITIES`).

Écrite à la main, `SUPPORTED` pourrait mentir ; ces deux règles le lui interdisent,
et la seconde dit aussi au framework ce qu'il a le droit d'annoncer.

Ce tableau n'est pas une feuille de route : c'est ce qu'un pont devrait faire,
système par système, pour que les mêmes méthodes veuillent dire la même chose.
Les écarts de **modèle** comptent plus que les écarts d'API.

| Capacité | iOS (fait) | Android | Windows / Linux / Web |
|---|---|---|---|
| identifiant d'un appareil Bluetooth | UUID opaque, stable par app | **adresse MAC** | MAC aussi (BlueZ) / opaque (Web Bluetooth) |
| permission Bluetooth | demandée une fois, par le système | `BLUETOOTH_SCAN` + `BLUETOOTH_CONNECT` (API 31+), **révocables** ; drapeau `neverForLocation` | pas de modèle unifié (BlueZ D-Bus) / geste utilisateur (Web) |
| caméra, micro | `NSCameraUsageDescription` + permission | `CAMERA`, `RECORD_AUDIO` à l'exécution | manifeste / `getUserMedia` |
| position | par capacité, `always` séparé | `ACCESS_FINE_LOCATION`, + `ACCESS_BACKGROUND_LOCATION` avec **service au premier plan visible** | géoloc système / `navigator.geolocation` |
| contacts | lire tout, créer | `READ_CONTACTS` / `WRITE_CONTACTS` (`ContactsContract`) | People API limitée / rien |
| calendrier | EventKit | `READ_CALENDAR` / `WRITE_CALENDAR` (`CalendarContract`) | rien de standard / rien |
| santé | HealthKit, lecture seule ici | **Health Connect**, permissions par type, lecture et écriture séparées, app tierce | rien |
| notifications | poster / retirer / planifier | `POST_NOTIFICATIONS` (API 33+), planification par `AlarmManager`/`WorkManager` — **pas d'équivalent direct** du déclencheur de temps | Windows App SDK / D-Bus / Notification API |
| activité en direct | Live Activity, Dynamic Island | **aucun équivalent** : au plus une notification de service | aucun / aucune |
| Face ID | LocalAuthentication | `BiometricPrompt` (classe 3) | Windows Hello / PAM / WebAuthn |
| parole | reconnaissance sur l'appareil | `SpeechRecognizer` — souvent **sur réseau**, pas de transcription de fichier | rien / Web Speech |
| NFC | CoreNFC, lecture et écriture | `NfcAdapter`, foreground dispatch, écriture NDEF | rien / Web NFC (Chrome) |
| réseau local | Bonjour | `NsdManager` (+ `CHANGE_WIFI_MULTICAST_STATE` pour le multicast) | mDNS selon l'OS / pas d'API |
| presse-papiers | texte, lien, image | texte et URI partout ; image selon la version | texte / API Clipboard (permission) |

Trois conséquences pour le contrat, déjà visibles dans le code :

1. **L'identifiant doit rester opaque à l'appelant.** `bluetoothConnect { id }`
   prend ce qu'un scan a rendu ; sur iOS c'est un UUID, sur Android une adresse
   MAC. Un module qui analyserait l'identifiant casserait en changeant de
   système — c'est pourquoi rien dans le pont ne le laisse deviner.
2. **Une permission peut être révoquée entre deux appels.** Le pont revérifie à
   chaque appel et répond `not_authorized` ; il ne met jamais la décision en
   cache. Sur Android, iOS et le Web, « accordé » n'est pas un état acquis.
3. **Ce qui n'a pas d'équivalent ne s'annonce pas.** `liveActivityStart` sur
   Android n'a pas de sens ; la bonne réponse est de ne pas l'annoncer dans la
   carte, pas de rendre `internal` à l'appel.

## Le setup de l'app hôte

Une méthode peut exister dans le pont et **échouer quand même** sur l'appareil,
parce que le *build* de l'app ne lui donne pas ce qu'iOS exige. Trois choses,
mesurées sur l'Atelier le 08/11/2026.

**1. Les phrases d'usage vivent dans la source du projet, pas dans le plist.**
`xcodegen` **génère** `Info.plist` à partir de `project.yml`. Quatorze phrases
avaient été écrites à la main dans le plist : un `xcodegen generate` les efface
d'un coup, et une app qui demande la caméra sans sa phrase est **tuée** par iOS.
Elles doivent donc être dans `project.yml` (`info.properties`), et le contrôle se
fait sur **l'archive**, pas sur la source :

```sh
plutil -p gen/apple/build/*.xcarchive/Products/Applications/*.app/Info.plist \
  | grep -c UsageDescription      # 14 attendues
```

**2. `location_always` demande `UIBackgroundModes: [location]`.** Sans cette clé,
iOS suspend les mises à jour dès que l'app passe en arrière-plan : la capacité
est annoncée, elle ne marche pas, et rien ne le dit.

**3. HealthKit et NFC demandent un *entitlement*, et un entitlement ne vit pas
sur un profil wildcard.** Mesuré en les ajoutant : le build échoue à la
signature — « Provisioning profile « iOS Team Provisioning Profile: * » doesn't
include the HealthKit capability ». Avec un profil d'équipe générique,
`device.health` et `device.nfc` répondront `unavailable` quel que soit le code.
Pour les activer : un App ID explicite dans le portail Apple avec **HealthKit**
et **NFC Tag Reading** cochés, puis les clés
`com.apple.developer.healthkit` et
`com.apple.developer.nfc.readersession.formats: [NDEF, TAG]` dans les
entitlements.

Un fichier d'entitlements **vide** est lui-même un piège : Xcode refuse un
fichier dont le mtime bouge pendant la compilation (« was modified during the
build »). Sans entitlement à déclarer, ne pas en déclarer du tout.

### Ce que ça implique pour la carte

Une capacité peut être `Live` dans la carte du framework et **impossible dans un
build donné**. C'est une limite de l'hôte, pas du pont, et le framework ne peut
pas la deviner : il ne connaît ni le profil de signature, ni le bundle, ni le nom
de l'app.

L'hôte le déclare donc, et la carte en tient compte :

```rust
use this_agent::surface::{HostLimits, Platform, surface};

let limites = HostLimits::new()
    .unavailable("device.health", "profil de signature sans l'entitlement HealthKit")
    .unavailable("device.nfc", "profil de signature sans l'entitlement NFC Tag Reading");

let cartepour_l_agent = surface().with_limits(&limites).describe_for(Platform::Ios, 12_000);
```

`with_limits` rend une **copie** : les entrées déclarées passent en `Missing` et
leur titre porte la raison. L'agent lit alors

```text
· device.health — indisponible sur ce build : profil de signature sans l'entitlement HealthKit [pas encore]
```

ce qui lui dit qu'insister ne servira à rien — un `not_authorized` ne le dit pas.
Un identifiant inconnu est ignoré, et la carte d'origine n'est pas modifiée.

`HostLimits` ne connaît aucun hôte. L'Atelier n'a qu'une fonction à vider le jour
où son App ID portera HealthKit et NFC : `appareil::limites_de_ce_build()`.
Changer le nom de l'app, ou son bundle, ne demande rien au framework.

### Et pour une coquille qui n'est pas celle-là

`this-agent` décrit ce que le **harnais** fournit (le tour d'outils : modules,
graphe, mémoire, rangées, modèle). Ce qu'une coquille fournit **en plus** — sa
fenêtre, son bac, ses notifications — ne regarde qu'elle :

```rust
let carte = surface().with_host_entries(hote_surface);
```

Une entrée de l'hôte portant un identifiant déjà connu **remplace** celle du
framework : l'hôte sait mieux que le framework ce que sa coquille expose. Les
emplacements se fusionnent de la même façon.

C'est la règle générale : **le framework décrit ce qu'il fournit, l'hôte déclare
ce qu'il ajoute et ce qu'il ne peut pas.** Rien de l'un ne doit être écrit en dur
chez l'autre.

## Ajouter une méthode

Quatre endroits, dans cet ordre — et les tests de parité vous disent lequel vous
avez oublié :

1. `device_bridge.rs` : `METHODS`, le bras de `capability_of`, les défauts de
   `complete_args`, les arguments requis de `missing_args`.
2. `src/families/<famille>.rs` : les bornes et les formes de réponse, avec leurs
   tests. C'est de la logique pure : elle se vérifie sans téléphone.
3. `ios/Sources/` : le **moteur** (sans `import Tauri`) et le **sélecteur**
   `@objc` (avec `import Tauri`). Un sélecteur ajouté dans une extension doit
   être référencé dans `famillesLiees` de `ThisDevicePlugin.swift`, sinon le
   dead-strip l'emporte.
4. `surface.rs` : l'entrée `device.<capacité>` ou `device.<capacité>.<aspect>`
   qui la nomme, avec ses plateformes réelles.

## Vérifier

```sh
# Contrat, familles, parités (carte ↔ pont ↔ Swift ↔ arguments) : une seconde, sans appareil.
# `--features banc` ajoute les tests du banc. Ces deux commandes tournent en CI, sur le job
# macOS : le crate est hors workspace, donc `cargo test --workspace` ne le voyait pas.
cd crates/this-device-tauri && cargo test --features banc

# Le SEUL contrôle qui compile le Swift iOS (les sélecteurs `@objc` importent Tauri)
cargo tauri ios build --target aarch64        # ~4 min, avec le vrai HOME

# Un raccourci pour les moteurs seulement (fichiers sans `import Tauri`), celui de la CI
xcrun --sdk iphoneos swiftc -typecheck -target arm64-apple-ios17.0 \
  ios/Sources/{BluetoothCentral,CameraCapture,AudioRecorder,Reply}.swift

# Mesurer un appareil : voir `tools/banc-appareil` et sa section plus bas
```

Trois pièges mesurés, qui font croire que tout va bien :

- `cargo build --target aarch64-apple-ios` **ne compile pas** le Swift iOS (un
  `touch` sur un `.swift` rend `Finished in 2.40s`). Seul un build d'app le fait,
  et la CI ne le fait pas : d'où le job `swift_ios`, qui ne vérifie que les
  moteurs et **le dit** quand le runner n'a pas de SDK.
- Le crate du pont est **hors workspace** (Tauri tire GTK sous Linux) : les
  `OUTILS` du workspace ne le voient pas.
- `git ls-files '*.rs' | xargs rustfmt --edition 2024 --check` est le contrôle
  de formatage de la CI, et il porte sur **tous les fichiers suivis** — pas
  seulement les membres du workspace.

## Quatre défauts mesurés sur un appareil, et les contrôles qui les tiennent fermés

Le 13/11/2026, deux pannes vivaient dans `main` alors que **tous les tests
passaient** et que `device_selftest` rendait un tableau vert des vingt et une
capacités. Les deux méritent d'être racontées, parce qu'aucune des deux ne se
voit depuis le code.

### 1. Un `#selector` ne garde pas une méthode : il produit une chaîne

`ThisDevicePlugin.famillesLiees` listait des `#selector(ThisDevicePlugin.captureStatus(_:))`
« pour forcer l'éditeur de liens à garder le fichier objet ». **Ça ne marche
pas.** `#selector` compile en un nom de sélecteur — une chaîne — pas en une
référence au symbole de la méthode. Le dead-strip retire donc l'objet entier, et
le pont répond :

```text
No command captureStatus found for plugin this-device
```

Mesuré sur un iPhone 16 Pro, avec le banc (§ Le banc) : **dix-huit méthodes** de
familles — caméra, micro, contacts, photothèque, fichiers, calendrier, santé,
Face ID, parole, NFC, réseau local, activité motrice, notifications, Live
Activity, presse-papiers, localisation, état système, flux — étaient présentes
dans `libapp.a`, leurs sélecteurs étaient bien dans le binaire **comme chaînes**,
et elles répondaient toutes « No command ». L'Atelier, lui, fonctionnait : son
`project.yml` passe `-ObjC`, qui garde tout. C'est-à-dire que le pont dépendait
d'un drapeau que l'hôte devait deviner.

**Le remède est local** : chaque fichier de famille porte une
`final class Famille<Nom>: NSObject {}` vide, et `ThisDevicePlugin.famillesAncrees`
référence ces classes. Référencer une **classe** garde l'objet qui la définit,
donc toutes ses méthodes. La liste est lue par `listCommands` — un vrai usage,
donc l'optimiseur ne peut pas la retirer — et trois règles la vérifient dans
`tests/swift_parity.rs` :

1. chaque fichier qui déclare un `@objc public func` porte son ancre ;
2. chaque ancre est listée dans `famillesAncrees` ;
3. la liste ne nomme aucune ancre qui n'existe pas.

Après le remède, sur le même appareil : **zéro** « No command ».

### 2. Un argument requis par le décodeur natif, absent du pont

`faceIdConfirm` répondait, sur l'iPhone :

```text
DecodingError.keyNotFound: Key 'reason' not found in keyed decoding container
```

Le décodeur Swift est strict — c'est correct — et le pont traduit cet échec en
`internal`, un code qui accuse le pont alors qu'un argument manquait. L'appelant,
lui, n'avait aucun moyen de deviner `reason`.

Le défaut n'était pas seul : `tests/args_parity.rs` en a trouvé **vingt** de la
même famille (`notificationsPost` sans `id` ni `title`, `calendarCreateEvent`
sans `start_ms`/`end_ms`, `filesWrite` sans `data_base64`, `speechTranscribeFile`
sans `locale`…). Le test lit les sources Swift réelles, extrait de chaque
`Decodable` les clés **non optionnelles sans valeur par défaut**, et exige pour
chacune :

- soit `complete_args` la remplit d'un défaut honnête ;
- soit `missing_args` la **nomme**.

`missing_args` rend maintenant **tous** les manquants, pas le premier : découvrir
la signature de `bluetoothWrite` une clé à la fois coûtait trois allers-retours.
Et la réponse du pont les porte (`{"code":"missing_argument","missing":[…]}`
au lieu du seul code), parce que le journal n'est pas ce que l'appelant lit.

Ce qui n'a **pas** de défaut honnête reste nommé, jamais inventé : un `reason` de
Face ID s'affiche à la personne ; un identifiant de notification ne se devine pas
puisque la réponse ne le rend pas.

### 3. La borne basse était fausse : iOS 14 annoncé, iOS 15 requis

Avec Xcode 26.2, les objets du crate Rust sont compilés pour iOS 15.0 ; un hôte
qui annonce 14.0 se lie en 14.0 et le lieur le dit :

```text
object file … was built for newer 'iOS-simulator' version (15.0) than being linked (14.0)
```

`ios/Package.swift` déclare donc `.iOS(.v15)`. C'est la borne réelle du crate.

### 4. Deux capacités que l'appareil AVAIT se lisaient absentes

Mesuré sur un iPhone 16 Pro Max, banc à l'appui : `capacite face_id = unsupported` et
`capacite location = unsupported`, sur un téléphone qui a les deux.

**Face ID.** `faceIdStatus` répondait `{"kind":"face_id_biometry","biometry":…}`. Or le pont ne
lit qu'une clé pour un état : `state` (`perform`, branche `Request::Status`). Un natif qui répond
une autre forme n'est pas une erreur — c'est un `unsupported` silencieux. La méthode rend
maintenant `state` **et** `biometry`, et `tests/swift_parity.rs` exige, pour chaque méthode que
`status_method` nomme, qu'elle écrive `state` (ou construise une réponse typée dont le champ
s'appelle `state`, ou délègue à un lecteur qui rend un `Reply`).

**Localisation.** `status_method` rendait `None` pour `location` et `location_always` : le pont
répondait donc `unsupported`, alors qu'iOS donne `authorizationStatus` sans rien demander. La
méthode `locationStatus` manquait — elle existe maintenant, sur iOS **et** macOS, et
`capability_of` lit sa capacité dans ses arguments comme `captureStatus`.

Ces deux-là comptent double, parce que `HostLimits::from_bench` transforme un `unsupported`
mesuré en « indisponible sur ce build » : ce qui se lit absent **sort de la carte de l'agent**.
D'où la règle jumelle côté banc — une capacité sans permission système (sélecteur, presse-papiers,
partage) n'est pas `unsupported` mais `sans_etat`, et `from_bench` l'ignore.

## Ce que « commun » voudrait dire entre iOS et macOS — et ce que la mesure en dit

Les deux implémentations natives se ressemblent : dix fichiers portent le même nom des deux
côtés. Mesuré le 13/11/2026 (Jaccard sur les lignes non vides, hors commentaires) :

| Fichier | iOS | macOS | lignes communes | Jaccard |
|---|---|---|---|---|
| `location.swift` | 225 | 187 | 105 | 34 % |
| `contacts.swift` | 140 | 108 | 57 | 30 % |
| `calendar.swift` | 159 | 143 | 61 | 25 % |
| `speech.swift` | 91 | 66 | 29 | 23 % |
| `bluetooth.swift` | 168 | 370 | 72 | 15 % |
| `photos.swift` | 220 | 208 | 54 | 14 % |
| `camera_mic.swift` | 151 | 292 | 56 | 14 % |
| `notifications.swift` | 90 | 46 | 16 | 13 % |
| `files.swift` | 157 | 106 | 28 | 12 % |
| `local_network.swift` | 217 | 160 | 36 | 11 % |

**Mais aucune déclaration n'est identique octet pour octet** entre les deux arbres : le
recouvrement est *sémantique*, avec des divergences réelles (champs présents d'un côté
seulement, découpage différent des mêmes types). Ce n'est donc pas une extraction mécanique —
c'est une réconciliation, déclaration par déclaration, et chaque divergence doit être tranchée
en connaissance de cause.

Ce qui **est** identique et vérifiable : le vocabulaire d'état du contrat —
`"granted"`, `"prompt"`, `"denied"`, `"limited"`, `"unsupported"`. Il est écrit à la main
**102 fois côté iOS et 93 fois côté macOS**. C'est le seul endroit où une faute de frappe change
le sens d'une réponse, et c'est exactement la classe du défaut Face ID (un `state` absent, lu
`unsupported` par le pont). Un type unique, `enum Etat: String`, le supprimerait.

### Les trois formes possibles, et ce qu'elles coûtent

| Forme | Ce que c'est | Le prix |
|---|---|---|
| **Paquet SPM `Famille`** | un paquet local dont dépendent `ios/Package.swift` et macOS | l'`xcodegen` de Tauri doit résoudre une dépendance de chemin ; à vérifier sur une app réelle |
| **Paquet unique à trois cibles** | `Common`, `Ios`, `Macos` dans un seul manifeste | change `ios_path` dans `build.rs` et la façon dont Tauri fabrique le projet Xcode |
| **Fichier unique dans `ios/Sources/`** | `build.rs` ajoute `ios/Sources/Commun/*.swift` à sa liste `swiftc` | macOS lit un fichier rangé côté iOS — laid, mais **aucun** changement d'empaquetage |

La troisième est la moins risquée et la plus rapide : `build.rs` compile déjà macOS à partir
d'une liste de fichiers explicite, donc y ajouter un répertoire ne touche ni SPM ni Xcode. C'est
par là qu'il faut commencer, ne serait-ce que pour le vocabulaire d'état, avant d'envisager de
déplacer des familles entières.

## Le banc : mesurer l'appareil, pas le promettre

`this-device-tauri` porte une feature `banc` (`src/banc.rs`) et
`tools/banc-appareil` une app hôte de deux cents lignes qui n'existe que pour
elle. Ensemble ils répondent à la seule question que ni le contrat ni la carte ne
traitent : **sur cet appareil-ci, qu'est-ce qui répond vraiment ?**

```sh
cd tools/banc-appareil
cargo tauri ios build --target aarch64-sim --debug
xcrun simctl install <udid> gen/apple/build/arm64-sim/BancAppareil.app
SIMCTL_CHILD_THIS_DEVICE_BANC=1 xcrun simctl launch <udid> dev.thisrs.bancappareil
# le rapport : <conteneur>/Documents/banc.json (+ banc.log, une ligne par méthode)
```

Sur un iPhone physique, `devicectl` fait la même chose :

```sh
cargo tauri ios build --target aarch64 --debug
xcrun devicectl device install app --device <udid> <archive>/BancAppareil.app
xcrun devicectl device process launch --device <udid> \
  -e '{"THIS_DEVICE_BANC":"1"}' dev.thisrs.bancappareil
```

### Ce que le banc exerce, et ce qu'il refuse d'exercer

Chaque méthode du contrat figure au plan, rangée par `Nature` :

| Nature | Exemple | Le banc l'appelle ? |
|---|---|---|
| `lecture` | `contactsStatus`, `photosList`, `bluetoothState` | oui, si la permission est **déjà** accordée |
| `permission` | `captureRequest`, `locationRequest` | non : une question se pose par un geste |
| `interactif` | `photosPick`, `nfcReadOnce`, `faceIdConfirm` | non |
| `ecriture` | `contactsCreate`, `notificationsPost` | non |

Deux règles rendent la mesure sûre sur l'appareil de quelqu'un :

1. **Aucune boîte de dialogue.** Avant d'appeler une lecture, le banc lit l'état
   de la capacité (`Request::Status`, qui ne demande rien) et ne lit la donnée
   que si la réponse est déjà là. Sans cela, `photosList` sur une app neuve
   ouvrirait la question de la photothèque — un diagnostic qui se transforme en
   formulaire.
2. **Rien de ce que l'appareil rend n'est recopié.** Ni contact, ni photo, ni
   texte du presse-papiers : le rapport ne garde que la nature, l'état, le code
   et la durée.

### Ce qu'il a donné, le 13/11/2026

Trois simulateurs (iPhone 16 Pro / iOS 18.6, iPhone 17 Pro / iOS 26.0, iPhone 17
/ iOS 26.2), build `--debug` :

- **zéro** `panne` après le remède des ancres (contre 15 sur le même build
  avant) ;
- un seul défaut restant, et c'est une réponse honnête : `filesRead` sans
  `bookmark` → `missing_argument` ;
- `captureStatus`, `nfcStatus`, `motionActivityStatus` et `gyroscopeSample`
  répondent `unsupported` : le simulateur n'a ni caméra, ni NFC, ni podomètre ;
- les lectures de données personnelles sont `non_exerce` tant que la permission
  est `prompt` — sur un appareil réel, c'est la personne qui la donne.

Et sur l'**iPhone 16 Pro Max** (iOS 26, build `release`, appareil déverrouillé) :

```text
defauts = filesRead
panne   = 0
face_id = granted        location = prompt      location_always = prompt
bluetooth = prompt       nfc = unsupported      live_activity = granted
clipboard · share · photos_pick · files_pick = sans_etat
```

`nfc = unsupported` est honnête : ce build-là n'a pas l'entitlement NFC (profil
générique). `sans_etat` est la marque des capacités qui n'ont pas de permission
système — un sélecteur, le presse-papiers, la feuille de partage.

Le même banc sur un iPhone physique demande l'appareil **déverrouillé** :
`devicectl` refuse de lancer une app sur un écran verrouillé
(`FBSOpenApplicationErrorDomain error 7`).

### Les écritures, exercées aussi

Une lecture se mesure sans rien changer ; une écriture, non. Le banc les exerce donc sur
demande (`THIS_DEVICE_BANC_ECRITURES`), avec un nettoyage là où le contrat en offre un. Mesuré
sur un simulateur iPhone 16 Pro / iOS 18.6, permissions accordées par `simctl privacy` :

```text
defauts =
ecriture filesWrite              [sures]    = repondu
ecriture notificationsSchedule   [sures]    = not_authorized   (permission système non accordée)
ecriture notificationsPending    [sures]    = repondu
ecriture notificationsCancel     [sures]    = repondu
ecriture liveActivityStart       [sures]    = unavailable      (le simulateur n'en a pas)
ecriture contactsCreate          [visibles] = repondu
ecriture contactsSearch          [visibles] = repondu          (le contact est bien là)
ecriture calendarCreateEvent     [visibles] = repondu
```

Et les lectures que la permission débloquait : `contactsList` (12 ms), `calendarEvents`
(17 ms), `calendarCalendars` (4 ms) — trois chemins qui n'avaient jamais été exercés.

### Ce que cette passe a corrigé dans le banc lui-même

`liveActivityUpdate` sans titre répondait `titre_absent` **après un aller-retour**, alors que
`LiveActivityArgs` déclare tous ses champs facultatifs : le contrat ne réclamait rien, le natif
exigeait tout. `missing_args` nomme maintenant ce que le natif exige —
`liveActivityStart` → `titre`, `liveActivityUpdate` → `id` + `titre`, `liveActivityEnd` → `id`.

Et le banc se trompait sur ce qu'est un défaut. Il comptait `refuse_argument` comme une panne
du pont : `filesRead` (qui ne peut pas se lire sans le `bookmark` d'un sélecteur) faisait donc
afficher « 1 défaut » sur un pont sain. Deux corrections :

- `refuse_argument` est le pont qui fait son travail — il a refusé **avant** le natif, en
  nommant la clé. Ce n'est pas un défaut ;
- un code de famille (`unknown_activity`, `too_large`…) est une **réponse**, pas une panne :
  il a son propre état, `refus`. Seuls `internal` et `timeout` accusent le pont.

Après quoi `defauts` est vide sur un pont sain, et la colonne veut de nouveau dire quelque
chose.

### Le banc sur macOS — mesuré pour la première fois le 13/11/2026

L'app du banc se construit aussi pour le bureau (`cargo tauri build --debug --bundles app`),
et le même `THIS_DEVICE_BANC=1` produit le rapport. Sur le Mac de développement :

```text
defauts =
notifications = prompt      camera = prompt        microphone = prompt
location = prompt           contacts = prompt      photos = prompt
calendar = prompt           speech = prompt        bluetooth = prompt
local_network = granted     clipboard/share/photos_pick/files_pick = sans_etat
live_activity · health · nfc · gyroscope · motion_activity · face_id = unsupported
localNetworkBrowse = repondu (5 351 ms)   filesRead = refus (invalid_bookmark)
```

Les cinq `unsupported` sont honnêtes : un Mac n'a ni Live Activity, ni HealthKit, ni NFC, ni
podomètre, et n'a pas de Face ID (Touch ID n'est pas une biométrie « appareil » au sens du
contrat — `faceIdStatus` répond quand même, par le chemin Touch ID).

**Le piège, et il tue le processus** : lancé **hors bundle** (le binaire nu dans
`target/debug/`), le banc meurt sur

```text
*** Terminating app due to uncaught exception 'NSInternalInconsistencyException',
    reason: 'bundleProxyForCurrentProcess is nil'
```

`ThisDeviceMacOS.notificationsStatus` appelle `UNUserNotificationCenter.currentNotificationCenter()`
depuis un fil du pool tokio ; sans bundle, UserNotifications lève une exception Objective-C, et
**une exception ObjC n'est pas rattrapable depuis Rust** — `catch_unwind` ne voit rien, le
processus est terminé. Mesuré : le même binaire, empaqueté en `.app`, rend son rapport
normalement. Deux conséquences : pour mesurer macOS il faut une app empaquetée, et un hôte qui
appellerait ce chemin depuis un processus non empaqueté mourrait sans trace Rust.

### Et ce que le banc a coûté à l'hôte pour y arriver

Une app qui ajoute le pont doit régler deux choses que le gabarit Tauri n'écrit
pas, et que rien ne signale avant le premier lancement sur un iPhone réel :

```yaml
ALWAYS_EMBED_SWIFT_STANDARD_LIBRARIES: true   # sinon : Library not loaded: libswiftCore.dylib
IPHONEOS_DEPLOYMENT_TARGET: "15.0"            # sinon : objets Rust en 15.0 liés en 14.0
```

Sans la première, l'app s'installe, se lance et meurt avant le premier écran
(`dyld: Library not loaded: @rpath/libswiftCore.dylib`), en debug **comme** en
release. L'Atelier porte la ligne depuis le 08/11 ; le banc l'a retrouvée par un
plantage. `tools/banc-appareil/preparer.sh` les écrit, et `cargo tauri ios build`
ne les applique qu'après un `xcodegen generate` — le build ne régénère pas le
projet depuis `project.yml`.

## Limites connues

Ce qui reste à mesurer avec un téléphone devant soi, après le banc :

- **les écritures** : un contact créé, un événement de calendrier, une
  notification planifiée, un fichier écrit. Le banc les déclare `non_exerce` par
  construction ;
- **les méthodes interactives** : `photosPick`, `filesPick`, `shareText`,
  `nfcReadOnce`/`nfcWrite`, `faceIdConfirm`, la capture photo et vidéo, la
  session micro. Elles demandent une main ;
- **Bluetooth au-delà du scan** : `bluetoothConnect` et ses suivantes veulent un
  périphérique réel en face ;
- **HealthKit et NFC** : le profil de signature de l'hôte porte ou ne porte pas
  l'entitlement ; sans lui, `unavailable` quel que soit le code (voir plus haut).

Le reste — les états de permission, les six lectures d'état système, les flux
d'orientation et de batterie, le scan Bluetooth, le presse-papiers en lecture et
`locationRecent` — est exercé par le banc, et le rapport est le seul juge.
