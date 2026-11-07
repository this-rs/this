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
| `location` | `locationRequest` `locationSample` | oui | oui |
| | `locationStreamStart` `locationStreamStop` `locationRecent` | oui | — |
| `location_always` | `locationRequest` | oui | — |
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

Soixante-seize méthodes, et **deux formes de « non »** qu'il ne faut pas
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

Le pont n'existe que pour iOS, iPadOS et une partie de macOS. Sur Android,
Windows, Linux et le Web, la carte ne nomme **aucune** capacité d'appareil — et
depuis le 08/11/2026 elle le **dit** (`device`, en `Missing`) au lieu de laisser
un vide que l'agent prendrait pour une ignorance.

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
build donné**. C'est une limite de l'hôte, pas du pont, et c'est à l'hôte de le
dire — pas à la carte de mentir dans les deux sens. L'Atelier documente son
propre cas : profil wildcard, donc santé et NFC non signés.

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
# Contrat, familles, parités (carte ↔ pont ↔ Swift) : une seconde, sans appareil
cd crates/tauri-plugin-this-device && cargo test

# Le SEUL contrôle qui compile le Swift iOS
cargo tauri ios build --target aarch64        # ~4 min, avec le vrai HOME

# Un raccourci pour les moteurs seulement (fichiers sans `import Tauri`)
xcrun --sdk iphoneos swiftc -typecheck -target arm64-apple-ios17.0 \
  ios/Sources/{BluetoothCentral,CameraCapture,AudioRecorder,Reply}.swift
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

## Limites connues

Rien de ce pont n'a été exercé sur un appareil au moment de l'écrire : les
tests couvrent le contrat, les bornes, les formes de réponse et les parités, pas
le comportement des frameworks Apple. Une connexion Bluetooth réelle, une photo
de 1280 px, un contact créé, une étiquette NFC écrite : chacun reste à mesurer
avec un téléphone devant soi.
