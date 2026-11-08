# Catalogue des capacités d'appareil

**Cadre (décision de l'utilisateur).** Le harnais a la liberté de tout créer, sur
toutes les plateformes (iOS, Android, Windows, macOS, Linux, Web). L'app n'est
qu'un châssis : les modules doivent pouvoir faire tout ce que l'appareil permet.
Ce document est l'inventaire de ce que « tout » contient, et de ce qui le borne.

**Règle de lecture : un inventaire faux est pire qu'un inventaire incomplet.**
Toute case dont l'auteur n'est pas sûr dit `à vérifier`. Rien ici n'est mesuré sur
appareil, sauf ce que `docs/appareil.md` mesure déjà (marqué **[mesuré]**).

## Légende

- **Plateformes** : `oui` (API publique, utilisable par une app ordinaire) · `partiel` (limité par le système, le matériel ou une politique de store) · `non` (aucune API pour une app tierce) · `à vérifier`.
  *iOS* = iPhone/iPad. *Web* = navigateur / wasm, souvent Chromium seul : `partiel` quand Safari/Firefox manquent.
- **`[E]`** = exige un **entitlement Apple soumis à approbation** (formulaire Apple), une **revue de store spécifique** ou une **permission « spéciale » Android** (hors boîte de dialogue ordinaire, politique Google Play). Une simple phrase d'usage (`NS…UsageDescription`) ou une permission d'exécution standard ne compte **pas** comme `[E]`.
- **Qui l'implémente** : `slot` = déjà dans le pont d'appareil (`tauri-plugin-this-device`, voir `docs/appareil.md`) ; `plugin:X` = plugin Tauri 2 officiel (X) ; `commu:X` = plugin communautaire, **à vérifier** avant usage ; `webview` = API Web dans la webview, sans pont ; `pont natif` = à écrire (Swift / Kotlin / Rust par plateforme).
- **Nom canonique** : ce qui ira dans `this-agent/src/surface.rs`. Les 21 noms déjà existants sont en **gras** ; les autres sont des **propositions**.
- **Origine du tableau Tauri** : les plugins officiels cités sont ceux que l'auteur connaît de la doc Tauri 2 ; **ils ne sont pas dans `atelier/src-tauri/Cargo.toml`**, qui ne dépend que de `tauri` et de `tauri-plugin-this-device` (vérifié). Version et support mobile de chacun : `à vérifier` au moment de l'adopter.
- **Principe de découpe** : une capacité = un consentement. Ajouter une écriture à une capacité de lecture est un changement de consentement (`docs/appareil.md`, § La porte) : les écritures risquées sont des noms distincts.

Colonnes : `Nom | Permet | iOS | Android | Windows | macOS | Linux | Web | Permission / entitlement | Qui l'implémente`

## 1. Entrées

| Nom | Permet | iOS | Android | Win | macOS | Linux | Web | Permission / entitlement | Qui |
|---|---|---|---|---|---|---|---|---|---|
| `touch` | tactile multipoint, pression | oui | oui | oui | non | partiel | oui | aucune | `webview` (Pointer Events) |
| `keyboard` | clavier physique, raccourcis, IME | oui | oui | oui | oui | oui | oui | aucune | `webview` ; plugin:global-shortcut (raccourcis globaux, bureau) |
| `keyboard_soft` | hauteur/affichage du clavier virtuel | oui | oui | non | non | non | partiel | aucune | `webview` (VirtualKeyboard API, Chromium) ; pont natif pour l'insets |
| `mouse` | souris, molette, curseur capturé (pointer lock) | partiel | partiel | oui | oui | oui | oui | aucune | `webview` |
| `trackpad_gestures` | pincer, rotation, balayage 3 doigts | oui | non | partiel | oui | partiel | partiel | aucune | `webview` (GestureEvent Safari seul) ; pont natif |
| `stylus` | stylet : pression, inclinaison, survol, rejet de la paume | oui (Apple Pencil) | oui | oui | non | partiel | oui | aucune ; Pencil Pro (squeeze) à vérifier | `webview` (Pointer Events) ; pont natif pour squeeze/double tap |
| `gamepad` | manettes : boutons, axes, vibration | oui | oui | oui | oui | oui | oui | aucune | `webview` (Gamepad API) ; pont natif (GameController, à vérifier) |
| `gestures` | reconnaissance de gestes (appui long, glisser, 3D Touch) | oui | oui | partiel | partiel | partiel | oui | aucune | `webview` ; pont natif pour 3D Touch/haptic touch |
| `haptics` | retour haptique (impact, notification, motif) | oui | oui | non | non | non | partiel | `VIBRATE` (Android) | plugin:haptics (mobile) ; `webview` (Vibration API, Android seul) |
| `voice_input` | dictée / commande vocale | oui | oui | partiel | oui | non | partiel | `NSSpeechRecognitionUsageDescription` + micro ; Android `RECORD_AUDIO` | **`speech`** slot (iOS, macOS) ; `webview` (Web Speech, Chromium) ; pont natif ailleurs |
| `hotword` | mot de réveil en continu | non | partiel | non | non | non | non | micro en arrière-plan ; politique Android | pont natif, **à vérifier** (voir « difficile ») |
| `eye_head_tracking` | suivi du regard / de la tête | partiel (ARKit visage, accessibilité) | partiel | non | non | non | non | `NSCameraUsageDescription` ; données gaze limitées, à vérifier | pont natif |
| `external_controllers` | télécommandes, Apple Remote, boutons média, volume | partiel | partiel | partiel | oui | partiel | partiel | aucune | `webview` (MediaSession) ; pont natif |

## 2. Affichage

| Nom | Permet | iOS | Android | Win | macOS | Linux | Web | Permission / entitlement | Qui |
|---|---|---|---|---|---|---|---|---|---|
| `display_info` | densité, taille, taux de rafraîchissement, encoche/zones sûres | oui | oui | oui | oui | oui | oui | aucune | `webview` (`devicePixelRatio`, `env(safe-area-*)`) ; plugin:os partiel |
| `orientation` | lire et verrouiller l'orientation | oui | oui | non | non | non | partiel | aucune | `webview` (Screen Orientation, Chromium/Android) ; pont natif |
| `hdr_color` | HDR, gamut large, EDR | oui | oui | partiel | oui | partiel | partiel | aucune | `webview` (`color-gamut`, `dynamic-range`) ; pont natif pour EDR (à vérifier) |
| `brightness` | lire/régler la luminosité de l'écran | oui | oui | partiel | partiel | partiel | non | Android : `WRITE_SETTINGS` ([E] spéciale) pour le système, aucune pour la fenêtre | pont natif |
| `keep_awake` | empêcher la veille pendant l'usage | oui | oui | oui | oui | oui | partiel | aucune (`WAKE_LOCK` Android) | `webview` (Screen Wake Lock) ; pont natif |
| `second_display` | écran externe, AirPlay, Cast, miroir ou contenu distinct | oui | oui | oui | oui | oui | partiel | aucune ; Cast/AirPlay : réseau local | `webview` (Presentation API, Chromium) ; pont natif |
| `window_management` | position/taille de fenêtre, plein écran, always-on-top | partiel | partiel | oui | oui | oui | partiel | aucune | `tauri` (fenêtres) ; plugin:window-state |
| `screen_capture` | capturer l'écran de l'app | oui | oui | oui | oui | oui | oui | aucune pour sa propre vue | `webview` ; pont natif |
| `screen_record` | enregistrer/diffuser l'écran **du système** | partiel | oui | oui | oui | partiel | oui | iOS : ReplayKit / extension de diffusion ; Android : `MediaProjection` (consentement à chaque session) ; macOS : « Enregistrement d'écran » (TCC) | `webview` (getDisplayMedia) ; pont natif |
| `screen_secure` | interdire la capture / masquer l'app dans le sélecteur | partiel | oui | partiel | partiel | non | non | aucune (`FLAG_SECURE` Android) | pont natif |
| `dark_mode` | thème clair/sombre, contraste, transparence réduite | oui | oui | oui | oui | oui | oui | aucune | `webview` (`prefers-color-scheme`…) |
| `font_scale` | taille de texte système (Dynamic Type) | oui | oui | partiel | partiel | partiel | partiel | aucune | `webview` partiel ; pont natif |
| `foldable_posture` | écran pliable, charnière, double écran | non | oui | partiel | non | non | partiel | aucune | `webview` (Viewport Segments, expérimental, à vérifier) ; pont natif (Jetpack WindowManager) |

## 3. Média

| Nom | Permet | iOS | Android | Win | macOS | Linux | Web | Permission / entitlement | Qui |
|---|---|---|---|---|---|---|---|---|---|
| **`camera`** | photo / vidéo, avant et arrière | oui | oui | oui | partiel (statut seul, pas de capture) [mesuré] | partiel | oui | `NSCameraUsageDescription` ; `CAMERA` | slot (iOS) ; `webview` (getUserMedia) ; pont natif Android |
| `camera_multi` | plusieurs caméras à la fois, ultra grand-angle, profondeur, LiDAR | partiel | partiel | partiel | non | non | non | comme `camera` ; Android : caméras logiques concurrentes selon le fabricant | pont natif |
| `camera_controls` | zoom, mise au point, flash/torche, exposition, RAW | oui | oui | partiel | partiel | partiel | partiel | comme `camera` | pont natif ; `webview` (ImageCapture, partiel) |
| **`microphone`** | enregistrement audio | oui | oui | oui | partiel (statut seul) [mesuré] | oui | oui | `NSMicrophoneUsageDescription` ; `RECORD_AUDIO` | slot (iOS) ; `webview` |
| `audio_playback` | lecture audio, sortie, mixage | oui | oui | oui | oui | oui | oui | aucune | `webview` (Web Audio) ; pont natif pour sessions système |
| `audio_background` | audio qui continue écran éteint / app masquée | oui | oui | oui | oui | oui | partiel | iOS : `UIBackgroundModes: audio` ; Android : service au premier plan `mediaPlayback` | pont natif |
| `audio_routing` | choisir sortie/entrée (haut-parleur, casque, Bluetooth, AirPlay) | oui | oui | oui | oui | oui | partiel | aucune ; Android : `BLUETOOTH_CONNECT` | `webview` (`setSinkId`, Chromium) ; pont natif |
| `speaker_tts` | synthèse vocale | oui | oui | oui | oui | partiel | oui | aucune | `webview` (speechSynthesis) ; pont natif |
| `video_playback` | lecture vidéo, PiP, streaming adaptatif | oui | oui | oui | oui | oui | oui | aucune ; PiP : `UIBackgroundModes: audio` + capacité iOS | `webview` (`<video>`, MSE) |
| `media_codecs` | encodage/décodage matériel (H.264/HEVC/AV1/VP9) | oui | oui | oui | oui | partiel | oui | aucune | `webview` (WebCodecs, partiel) ; pont natif |
| `media_recording` | encoder en continu vers un fichier (audio/vidéo) | oui | oui | oui | oui | oui | oui | comme caméra/micro | **`microphone`** / **`camera`** slots (fichier dans le conteneur) ; `webview` (MediaRecorder) |
| `media_session` | commandes écran verrouillé, métadonnées « en lecture » | oui | oui | oui | oui | oui | oui | aucune | `webview` (Media Session) ; pont natif |
| `image_processing` | filtres, détection de visages/textes/codes, OCR | oui | oui | oui | oui | partiel | partiel | aucune | pont natif (Vision / ML Kit) ; `webview` (Shape Detection, Chromium) |
| `barcode_scan` | codes QR / codes-barres | oui | oui | non | non | non | partiel | `CAMERA` | plugin:barcode-scanner (mobile) ; `webview` (BarcodeDetector, partiel) |
| `photo_library_write` | enregistrer une image/vidéo dans la photothèque | oui | oui | non | oui | non | non | `NSPhotoLibraryAddUsageDescription` ; stockage partagé Android | pont natif |
| `drm_playback` | contenu protégé (Widevine, FairPlay) | oui | oui | oui | oui | partiel | oui | aucune ; licence côté fournisseur | `webview` (EME) |
| `streaming_cast` | envoyer le média vers un récepteur (AirPlay, Chromecast) | oui | oui | oui | oui | partiel | partiel | réseau local ; iOS 14+ : `NSLocalNetworkUsageDescription` | pont natif ; `webview` (Remote Playback) |

## 4. Radios et connectivité

| Nom | Permet | iOS | Android | Win | macOS | Linux | Web | Permission / entitlement | Qui |
|---|---|---|---|---|---|---|---|---|---|
| `wifi_state` | lire l'état et le SSID courant | partiel | partiel | oui | oui | oui | non | iOS : `com.apple.developer.networking.wifi-info` [E] + localisation ; Android : `ACCESS_FINE_LOCATION` + `ACCESS_WIFI_STATE` | pont natif |
| `wifi_scan` | lister les réseaux Wi-Fi alentour | **non** | partiel | oui | partiel | oui | non | iOS : aucune API publique ; Android : `NEARBY_WIFI_DEVICES` (13+) ou `ACCESS_FINE_LOCATION`, **limité à 4 scans / 2 min** (9+) ; macOS : CoreWLAN, localisation | pont natif |
| `wifi_connect` | rejoindre un réseau donné | partiel | partiel | oui | oui | oui | non | iOS : `NEHotspotConfiguration` [E] (entitlement Hotspot Configuration) ; Android : `WifiNetworkSuggestion`/Specifier (10+), confirmation utilisateur | pont natif |
| `wifi_direct_aware` | pair à pair Wi-Fi (Wi-Fi Direct, Wi-Fi Aware, AWDL) | non (AWDL non public) | partiel | partiel | non | partiel | non | Android : `NEARBY_WIFI_DEVICES` ; Wi-Fi Aware selon matériel | pont natif, **à vérifier** |
| `hotspot_control` | activer un point d'accès | non | partiel | partiel | partiel | partiel | non | Android : `LocalOnlyHotspot` (26+), pas le hotspot système | pont natif |
| **`bluetooth`** | scan, connexion, GATT lecture/écriture/abonnement (BLE) | oui | oui | oui | partiel (scan seul) [mesuré] | oui | partiel | `NSBluetoothAlwaysUsageDescription` ; Android 12+ `BLUETOOTH_SCAN` + `BLUETOOTH_CONNECT` | slot (iOS) ; commu:tauri-plugin-blec / btleplug, à vérifier |
| `bluetooth_classic` | Bluetooth Classique (SPP, RFCOMM, A2DP) | **non** (hors accessoires MFi [E]) | oui | oui | oui | oui | non | iOS : programme MFi (ExternalAccessory) ; Android `BLUETOOTH_CONNECT` | pont natif |
| `bluetooth_peripheral` | se comporter en périphérique BLE (annoncer, serveur GATT) | oui | oui | partiel | oui | oui | non | iOS : `NSBluetoothAlwaysUsageDescription` ; Android `BLUETOOTH_ADVERTISE` ; annonce iOS en arrière-plan bridée | pont natif |
| `bluetooth_background` | BLE écran éteint (restauration d'état, scan filtré) | oui | oui | oui | oui | oui | non | iOS : `UIBackgroundModes: bluetooth-central` ; Android : service au premier plan | pont natif |
| `bluetooth_mesh_audio` | Bluetooth LE Audio / Auracast / mesh | partiel | partiel | partiel | partiel | partiel | non | à vérifier | pont natif, **à vérifier** |
| **`nfc`** | lire / écrire des étiquettes NDEF | oui | oui | non (lecteurs USB) | non | non (lecteurs USB) | partiel (Web NFC, Chrome Android) | iOS : `NFCReaderUsageDescription` + entitlement NFC Tag Reading [E] (profil explicite) ; Android : `NFC` | slot (iOS) ; plugin:nfc (mobile) |
| `nfc_tag_raw` | ISO 7816 / FeliCa / MIFARE brut | partiel | oui | non | non | non | non | iOS : AID à déclarer dans l'Info.plist, entitlement [E] ; Android : `NFC`, `IsoDep`/`MifareClassic` (selon puce) | pont natif |
| `nfc_card_emulation` | se faire passer pour une carte / un badge (HCE) | partiel (UE/Japon, iOS 17.4+, entitlement [E] à vérifier) | oui | non | non | non | non | Android : `BIND_NFC_SERVICE` + service HCE ; iOS : entitlement HCE [E] | pont natif, **à vérifier** pour iOS |
| `uwb` | ultra-large bande : mesurer distance et direction | partiel (Nearby Interaction, U1/U2) | partiel | non | partiel | non | non | iOS : `NSNearbyInteractionUsageDescription` ; Android : `UWB_RANGING` (13+) ; **pair compatible nécessaire** | pont natif |
| **`geo`** (slot : `location`, `location_always`) | position, flux, historique ; mode « toujours » | oui | oui | oui | partiel (échantillon seul) [mesuré] | partiel | oui | iOS : `NSLocationWhenInUse…` / `…Always…` + `UIBackgroundModes: location` [mesuré] ; Android `ACCESS_FINE_LOCATION`, `ACCESS_BACKGROUND_LOCATION` (revue Play [E]) | slot (iOS) ; plugin:geolocation (mobile) ; `webview` |
| `gnss_raw` | mesures satellites brutes (constellations, C/N0) | non | oui | non | non | partiel | non | Android : `ACCESS_FINE_LOCATION` (GnssMeasurement) | pont natif |
| `geofence` | zones surveillées, entrée/sortie, même app fermée | oui | oui | non | oui | non | non | iOS : `…Always…` ; Android : `ACCESS_BACKGROUND_LOCATION` (revue Play [E]) ; limite ~20 (iOS) / 100 (Android) | pont natif |
| `beacon` | iBeacon / Eddystone, ranging de balises | oui | oui | partiel | oui | partiel | non | iOS : localisation (`CLBeaconRegion`) ; Android : `BLUETOOTH_SCAN` | pont natif |
| `cellular_info` | type de réseau (4G/5G), opérateur, force du signal | partiel | oui | partiel | non | partiel | partiel | iOS : `CTTelephonyNetworkInfo` (opérateur déprécié, valeurs vides) ; Android : `READ_PHONE_STATE`, `ACCESS_FINE_LOCATION` pour cellules | `webview` (Network Information, Chromium) ; pont natif |
| `sim_identity` | numéro de ligne, IMSI, IMEI, ICCID | **non** | partiel | partiel | non | partiel | non | Android 10+ : IMEI/ICCID réservés aux apps **privilégiées** ([E] impossible pour une app ordinaire) ; `READ_PHONE_NUMBERS` pour le numéro | pont natif |
| `ir_blaster` | émetteur infrarouge (télécommande) | non | partiel (peu d'appareils) | non | non | non | non | `TRANSMIT_IR` | pont natif |
| `local_network` (slot : **`local_network`**) | découvrir des services sur le réseau local (Bonjour/mDNS) | oui | oui | partiel | partiel (navigation seule) [mesuré] | partiel | non | iOS : `NSLocalNetworkUsageDescription` + `NSBonjourServices` ; Android : `CHANGE_WIFI_MULTICAST_STATE` | slot (iOS) ; pont natif |
| `network_sockets` | TCP/UDP bruts, serveur local, multicast | partiel | oui | oui | oui | oui | non (WebSocket/WebRTC seuls) | iOS : réseau local ; pare-feu bureau | plugin:websocket (WS seul) ; plugin:http ; pont natif pour brut |
| `webrtc_p2p` | pair à pair audio/vidéo/données | oui | oui | oui | oui | oui | oui | micro/caméra | `webview` (WebRTC) |
| `vpn_proxy` | tunnel VPN, filtre de contenu, proxy applicatif | oui | oui | oui | oui | oui | non | iOS : entitlement Network Extension [E] ; Android : `BIND_VPN_SERVICE`, consentement système | pont natif |
| `usb_host` | accéder à un périphérique USB | non (hors MFi) | oui | oui | oui | oui | partiel (WebUSB, Chromium) | Android : permission par périphérique | pont natif ; `webview` (WebUSB) |
| `serial_port` | port série / adaptateur | non | partiel (via USB) | oui | oui | oui | partiel (Web Serial, Chromium) | à vérifier | pont natif ; `webview` |
| `hid_device` | périphériques HID bruts | non | partiel | oui | oui | oui | partiel (WebHID, Chromium) | à vérifier | pont natif ; `webview` |
| `satellite_sos` | liaison satellite | non (non exposé) | non | non | non | non | non | — | **impossible** pour un module |

## 5. Capteurs

| Nom | Permet | iOS | Android | Win | macOS | Linux | Web | Permission / entitlement | Qui |
|---|---|---|---|---|---|---|---|---|---|
| `accelerometer` | accélération 3 axes | oui | oui | partiel | non | partiel | oui | iOS : `NSMotionUsageDescription` (CoreMotion) ; Android ≥ 12 : `HIGH_SAMPLING_RATE_SENSORS` au-delà de 200 Hz | `webview` (Generic Sensor / DeviceMotion, iOS : geste utilisateur) ; plugin communautaire, à vérifier |
| **`gyroscope`** | vitesse angulaire | oui | oui | partiel | non | partiel | oui | comme ci-dessus | slot (iOS) |
| `magnetometer` | champ magnétique, boussole | oui | oui | partiel | non | partiel | partiel | aucune ; boussole : localisation sur iOS | pont natif ; `webview` (Magnetometer, Chromium) |
| `device_orientation` | orientation fusionnée (attitude, rotation) | oui | oui | partiel | non | partiel | oui | aucune | `webview` (DeviceOrientation) ; pont natif |
| `barometer` | pression atmosphérique, altitude relative | oui | oui | non | non | non | non | iOS : `NSMotionUsageDescription` | pont natif |
| `proximity` | capteur de proximité (oreille) | oui | oui | non | non | non | partiel (Chromium, déprécié) | aucune | pont natif |
| `ambient_light` | luminosité ambiante | non (non public) | oui | partiel | non | partiel | partiel (Chromium, derrière drapeau) | aucune | pont natif |
| `thermometer` | température ambiante / appareil | non | partiel (rare) | non | non | non | non | aucune | pont natif, **à vérifier** |
| `thermal_state` | état thermique et pression de l'appareil | oui | oui | non | oui | non | partiel (Compute Pressure) | aucune | pont natif |
| `humidity` | humidité ambiante | non | partiel (rare) | non | non | non | non | aucune | pont natif, **à vérifier** |
| `heart_rate` | fréquence cardiaque, oxymétrie, ECG | oui (via santé / montre) | oui (Health Connect / capteur) | non | non | non | non | voir `health` | `health` ; pont natif |
| **`motion_activity`** | pas, distance, type d'activité (marche, course, véhicule) | oui | oui | non | non | non | non | iOS : `NSMotionUsageDescription` ; Android : `ACTIVITY_RECOGNITION` | slot (iOS) ; pont natif Android |
| `pedometer_raw` | podomètre matériel | oui | oui | non | non | non | non | comme `motion_activity` | **`motion_activity`** |
| `fingerprint_sensor` | capteur d'empreintes (identification seule) | non | non | non | non | non | non | — | voir `biometric` (le capteur n'est pas exposé) |
| `lidar_depth` | profondeur (LiDAR, ToF, double caméra) | partiel (Pro, iPad Pro) | partiel | non | non | non | non | caméra | pont natif (ARKit / ARCore Depth) |
| `face_tracking_depth` | maillage du visage (TrueDepth) | partiel (Face ID) | partiel | non | non | non | non | caméra ; **pas de données biométriques Face ID** | pont natif (ARKit) |
| `sensor_batch` | échantillonnage haute fréquence / en arrière-plan | partiel | partiel | non | non | non | non | Android ≥ 12 : `HIGH_SAMPLING_RATE_SENSORS`, service au premier plan | pont natif |
| `remote_sensors` | capteurs d'un **autre** appareil (montre, casque) | partiel | partiel | non | non | non | non | voir « difficile » | pont natif, **à vérifier** |

## 6. Stockage et fichiers

| Nom | Permet | iOS | Android | Win | macOS | Linux | Web | Permission / entitlement | Qui |
|---|---|---|---|---|---|---|---|---|---|
| **`files_pick`** | choisir / lire / écrire un fichier via le sélecteur | oui | oui | oui | oui | oui | partiel (File System Access, Chromium) | aucune (sélecteur = consentement) | slot (iOS, macOS) ; plugin:dialog + plugin:fs |
| `files_app_storage` | conteneur privé de l'app | oui | oui | oui | oui | oui | oui (OPFS) | aucune | plugin:fs ; `webview` |
| `files_shared_dirs` | dossiers partagés (Documents, Téléchargements, carte SD) | partiel | oui | oui | oui | oui | non | Android : `MANAGE_EXTERNAL_STORAGE` [E] (revue Play) ou SAF ; iOS : app dans « Fichiers » (`UIFileSharingEnabled`) | plugin:fs ; pont natif |
| `files_persistent_access` | rouvrir un fichier choisi plus tard (signets, URI persistants) | oui | oui | oui | oui | oui | partiel | iOS : *security-scoped bookmarks* ; Android : `takePersistableUriPermission` | pont natif |
| `files_cloud` | iCloud Drive, Google Drive (fournisseur de fichiers) | oui | oui | partiel | oui | non | non | iOS : entitlement iCloud (capacité) | pont natif |
| **`clipboard`** | presse-papiers texte, lien, image | oui | partiel (image selon version) | oui | oui (texte) [mesuré] | oui | partiel (permission) | iOS 16+ : bandeau « coller » ; lecture en arrière-plan bloquée | slot (iOS) ; plugin:clipboard-manager |
| `clipboard_watch` | observer le presse-papiers | non | non (10+, premier plan seul) | oui | partiel | oui | non | — | **impossible** sur mobile (voir « difficile ») |
| **`share`** | feuille de partage sortante (texte, lien, image, fichier) | oui | oui | oui | oui | non | partiel (Web Share, mobile) | aucune | slot ; `webview` ; commu:tauri-plugin-sharesheet, à vérifier |
| `drag_drop` | glisser-déposer entre apps | oui (iPad) | partiel | oui | oui | oui | oui | aucune | `tauri` (événements de dépôt de fichiers) ; `webview` |
| `print` | impression, export PDF | oui | oui | oui | oui | oui | oui | aucune | `webview` (`window.print`) ; pont natif |
| `document_scan` | scanner de documents avec rognage | oui (VisionKit) | oui (ML Kit Document Scanner) | non | oui (Continuity) | non | non | `NSCameraUsageDescription` ; ML Kit : Google Play Services | pont natif |
| `archive_compression` | zip/dézip, compression | oui | oui | oui | oui | oui | oui | aucune | pur Rust / wasm |
| `database_local` | SQLite / KV local | oui | oui | oui | oui | oui | oui | aucune | plugin:sql, plugin:store ; `webview` (IndexedDB) |
| `storage_quota` | espace libre, état du disque | oui | oui | oui | oui | oui | partiel (StorageManager) | aucune | pont natif ; plugin:os partiel |
| `media_library_read` | lire audio/vidéo de la bibliothèque | partiel | oui | oui | oui | oui | non | iOS : `NSAppleMusicUsageDescription` ; Android 13+ : `READ_MEDIA_*` | pont natif |
| **`photos`** | lire la photothèque entière (liste, lecture) | oui | oui | non | oui | non | non | `NSPhotoLibraryUsageDescription` ; Android 13+ `READ_MEDIA_IMAGES` ; Android 14 : accès partiel | slot (iOS, macOS) ; pont natif Android |
| **`photos_pick`** | choisir des photos sans accès global | oui | oui | oui | oui | oui | oui | aucune (PHPicker / Photo Picker) | slot ; `webview` (`<input type=file>`) |
| `trash_recycle` | corbeille, suppression récupérable | partiel | partiel | oui | oui | oui | non | à vérifier | pont natif |
| `secure_enclave_files` | fichiers chiffrés liés à l'appareil | oui | oui | oui | oui | partiel | partiel | iOS : `NSFileProtectionComplete` | pont natif |

## 7. Notifications

| Nom | Permet | iOS | Android | Win | macOS | Linux | Web | Permission / entitlement | Qui |
|---|---|---|---|---|---|---|---|---|---|
| **`notifications`** | notifications locales : poster, annuler, planifier | oui (planification : oui) | oui | oui | partiel (poster/annuler) [mesuré] | oui | partiel | iOS : demande d'autorisation ; Android 13+ : `POST_NOTIFICATIONS` | slot (iOS) ; plugin:notification |
| `push_remote` | notifications distantes (APNs / FCM / WNS) | oui | oui | oui | oui | non | partiel (Web Push) | iOS : entitlement `aps-environment` (capacité Push) ; serveur d'envoi obligatoire ; Android : `POST_NOTIFICATIONS` | pont natif + service d'envoi ; commu:tauri-plugin-push, à vérifier |
| `push_silent` | notification silencieuse qui réveille l'app | oui (bridé) | oui | partiel | oui | non | partiel | iOS : `UIBackgroundModes: remote-notification` ; **non garanti** (budget système) | pont natif |
| `notification_actions` | boutons, saisie de texte, catégories | oui | oui | oui | oui | partiel | partiel | aucune | plugin:notification (actions, à vérifier) ; pont natif |
| `notification_channels` | canaux Android (importance, son, vibration) | non | oui | non | non | non | non | aucune ; 8.0+ | plugin:notification (canaux) |
| `notification_badge` | pastille sur l'icône | oui | partiel (selon lanceur) | oui | oui | partiel | partiel (Badging API) | iOS : autorisation `badge` | pont natif ; `webview` |
| `notification_sound` | sons personnalisés, mode sonore | oui | oui | oui | oui | partiel | partiel | iOS : fichier son dans le bundle | pont natif |
| `notification_critical` | alertes critiques passant outre « Ne pas déranger » | oui [E] | partiel (`ACCESS_NOTIFICATION_POLICY`) | non | oui [E] | non | non | iOS : entitlement `com.apple.developer.usernotifications.critical-alerts` [E] (approbation Apple) | pont natif |
| `notification_timesensitive` | niveau d'interruption « urgent » | oui | oui | non | oui | non | non | iOS 15+ : entitlement Time Sensitive | pont natif |
| `notification_read` | lire les notifications **d'autres** apps | non | oui | partiel | non | partiel | non | Android : `BIND_NOTIFICATION_LISTENER_SERVICE` [E] (accès spécial) | pont natif |
| `focus_mode` | lire/refléter le mode Concentration / Ne pas déranger | partiel (statut, entitlement) | partiel | partiel | partiel | non | non | iOS : `INFocusStatusCenter`, autorisation ; Android : `ACCESS_NOTIFICATION_POLICY` [E] | pont natif, **à vérifier** |

## 8. Vie de l'application

| Nom | Permet | iOS | Android | Win | macOS | Linux | Web | Permission / entitlement | Qui |
|---|---|---|---|---|---|---|---|---|---|
| `background_tasks` | travail court/périodique hors premier plan | partiel (BGTaskScheduler, budget système) | oui (WorkManager) | oui | oui | oui | partiel (Background Sync, Chromium) | iOS : `UIBackgroundModes` ; aucun garantie d'horaire | pont natif |
| `background_long` | travail continu visible | partiel (modes dédiés : audio, location, voip…) | oui (service au premier plan) | oui | oui | oui | non | Android 14 : type de service au premier plan déclaré (`foregroundServiceType`) ; revue Play pour certains types | pont natif |
| `wake_alarm` | réveil à heure exacte | partiel (notification locale seulement) | oui | oui | oui | oui | non | Android 12+ : `SCHEDULE_EXACT_ALARM` / `USE_EXACT_ALARM` [E] (politique Play) | pont natif |
| `autostart` | lancement au démarrage / à l'ouverture de session | **non** | partiel (`RECEIVE_BOOT_COMPLETED`, restreint par fabricant) | oui | oui | oui | non | aucune | plugin:autostart (bureau) ; pont natif Android |
| `deep_links` | ouvrir l'app depuis une URL (schéma, universal/app link) | oui | oui | oui | oui | oui | oui | iOS : Associated Domains (capacité) ; Android : `assetlinks.json` | plugin:deep-link |
| `intents_outgoing` | ouvrir d'autres apps (URL, carte, appel, mail) | oui | oui | oui | oui | oui | partiel | iOS : `LSApplicationQueriesSchemes` pour tester ; Android 11+ : `<queries>` | plugin:opener |
| `intents_incoming` | recevoir des intents (ouvrir avec, partager vers) | oui | oui | oui | oui | oui | partiel (Web Share Target) | manifeste | pont natif |
| `app_shortcuts` | raccourcis d'icône, Siri/Raccourcis, App Intents | oui | oui | partiel | oui | partiel | partiel | aucune (déclaration) | pont natif |
| `siri_assistant` | exposer des actions à Siri / Google Assistant | oui | oui | non | oui | non | non | iOS : capacité Siri ; Android : App Actions | pont natif, **à vérifier** |
| `widgets` | widgets d'écran d'accueil | oui (WidgetKit) | oui (App Widgets) | partiel (Widgets Win11, à vérifier) | oui | non | non | extension / manifeste ; **code natif**, pas une webview | pont natif |
| **`live_activity`** | activité en direct, Dynamic Island | oui | non (aucun équivalent) | non | non | non | non | `NSSupportsLiveActivities` | slot (iOS) |
| `complications_tiles` | complications de montre, tuiles Wear OS | oui (watchOS) | oui (Wear OS) | non | non | non | non | app compagnon obligatoire | pont natif |
| `quick_settings_tile` | tuile des réglages rapides | non | oui (7+) | non | non | non | non | aucune | pont natif |
| `menu_bar_tray` | icône de barre de menus / zone de notification | non | non | oui | oui | oui | non | aucune | `tauri` (tray) |
| `app_lifecycle` | événements : premier plan/arrière-plan, mémoire faible, fermeture | oui | oui | oui | oui | oui | oui (Page Lifecycle) | aucune | `tauri` ; `webview` |
| `single_instance` | une seule instance, relais des arguments | non | non | oui | oui | oui | non | aucune | plugin:single-instance (bureau) |
| `updater` | mise à jour in-app | non (App Store) | partiel (Play in-app updates) | oui | oui | oui | oui (service worker) | signature de mise à jour | plugin:updater (bureau) |
| `global_shortcuts` | raccourcis clavier globaux | non | non | oui | oui | partiel (Wayland restreint) | non | macOS : accessibilité (TCC) pour certains | plugin:global-shortcut |
| `module_code_loading` | charger du code de module à chaud | **partiel** (interprété seul : JS/wasm dans la webview ; **code natif téléchargé interdit**, règle App Store 2.5.2) | oui | oui | oui | oui | oui | politique de store | `webview` / wasm (voir « difficile ») |

## 9. Système

| Nom | Permet | iOS | Android | Win | macOS | Linux | Web | Permission / entitlement | Qui |
|---|---|---|---|---|---|---|---|---|---|
| `battery` | niveau, état de charge, mode économie | oui | oui | oui | oui | oui | partiel (Chromium) | aucune | `webview` (Battery API) ; pont natif |
| `network_status` | en ligne/hors ligne, type, métrique, VPN | oui | oui | oui | oui | oui | oui | Android : `ACCESS_NETWORK_STATE` | `webview` (`navigator.onLine`) ; plugin:os partiel |
| `screen_state` | écran allumé/éteint, verrouillé/déverrouillé | partiel | oui | oui | oui | oui | partiel (Page Visibility, Idle Detection) | Idle Detection : permission | pont natif ; `webview` |
| `lock_state` | appareil verrouillé ? | partiel (protected data) | oui | oui | oui | oui | non | aucune | pont natif |
| `locale_language` | langue, région, formats, calendrier | oui | oui | oui | oui | oui | oui | aucune | `webview` ; plugin:os (locale) |
| `timezone_clock` | fuseau, horloge, changement d'heure | oui | oui | oui | oui | oui | oui | aucune | `webview` ; plugin:os |
| `device_info` | modèle, version d'OS, architecture, nom de l'appareil | oui | oui | oui | oui | oui | partiel | iOS 16+ : **nom personnalisé** nécessite l'entitlement `device-name` [E] | plugin:os |
| `device_identifier` | identifiant stable de l'appareil | partiel (`identifierForVendor`, remis à zéro à la désinstallation) | partiel (ANDROID_ID par app ; pas de numéro de série) | oui | oui | oui | non | aucune ; IDFA : `NSUserTrackingUsageDescription` (ATT) | pont natif |
| `system_settings` | lire/modifier des réglages (Wi-Fi, volume, luminosité) | **non** (ouverture de l'écran de réglages seule) | partiel | oui | oui | oui | non | Android : `WRITE_SETTINGS` [E] | pont natif |
| `settings_open` | ouvrir l'écran de réglages de l'app / du système | oui | oui | oui | oui | oui | non | aucune | plugin:opener ; pont natif |
| `volume` | lire/régler volumes (média, sonnerie) | partiel (lecture ; réglage via MPVolumeView) | oui | oui | oui | oui | non | Android : `MODIFY_AUDIO_SETTINGS` | pont natif |
| `airplane_mode` | lire / activer le mode avion | partiel (lecture indirecte) | partiel (lecture ; écriture interdite depuis 4.2) | partiel | non | partiel | non | aucune | pont natif, **à vérifier** |
| `do_not_disturb` | lire/régler « Ne pas déranger » | non | partiel [E] | partiel | non | partiel | non | Android : `ACCESS_NOTIFICATION_POLICY` [E] | pont natif |
| `power_management` | veille, extinction, redémarrage | non | non (hors admin d'appareil) | oui | oui | oui | non | privilèges d'administration | pont natif / plugin:process (quitter seulement) |
| `process_exec` | lancer des programmes, shell | **non** | partiel (son sandbox) | oui | oui | oui | non | aucune ; **sandbox** sur Mac App Store | plugin:shell (bureau) |
| `env_variables` | variables d'environnement, répertoires standards | non | non | oui | oui | oui | non | aucune | plugin:os ; `tauri` (chemins) |
| `logging` | journaux, diagnostics, rapports de plantage | oui | oui | oui | oui | oui | oui | aucune | plugin:log |
| `performance_metrics` | CPU, mémoire, fréquence d'images | partiel | partiel | oui | oui | oui | partiel | aucune | pont natif |
| `system_time_events` | réveil par minuteur, changement de fuseau | oui | oui | oui | oui | oui | oui | aucune | `tauri` |
| `app_inventory` | lister les apps installées | non (sondage par schéma seulement) | partiel | oui | oui | oui | non | Android 11+ : `QUERY_ALL_PACKAGES` [E] (revue Play) | pont natif |
| `kiosk_mode` | verrouiller sur une app (épinglage, accès guidé) | partiel (Accès guidé, gestion MDM) | oui | oui | partiel | partiel | non | MDM / propriétaire d'appareil | pont natif, **à vérifier** |

## 10. Sécurité

| Nom | Permet | iOS | Android | Win | macOS | Linux | Web | Permission / entitlement | Qui |
|---|---|---|---|---|---|---|---|---|---|
| **`face_id`** (nom canonique à élargir en `biometric`) | confirmer une action par biométrie (Face ID, Touch ID, empreinte) | oui | oui | oui (Hello) | oui | partiel (PAM, empreinte selon distribution) | partiel (WebAuthn) | iOS : `NSFaceIDUsageDescription` ; Android : `USE_BIOMETRIC` | slot (iOS, macOS) ; plugin:biometric (mobile) |
| `keychain_secrets` | stocker un secret chiffré (Keychain / Keystore / Credential Manager) | oui | oui | oui | oui | partiel (Secret Service) | non | aucune ; groupe de trousseau (capacité) pour le partage | pont natif ; plugin:stronghold (coffre propre, déprécié à terme : à vérifier) ; commu:keyring |
| `secure_enclave_keys` | clés non exportables, signature matérielle | oui | oui (StrongBox / TEE) | oui (TPM) | oui | partiel (TPM) | partiel (WebAuthn) | iOS : `kSecAttrTokenIDSecureEnclave` ; Android : `setIsStrongBoxBacked` selon matériel | pont natif |
| `crypto_primitives` | chiffrement, hachage, signature, aléatoire | oui | oui | oui | oui | oui | oui | aucune | pur Rust / wasm ; `webview` (WebCrypto) |
| `passkeys_webauthn` | clés d'accès, FIDO2 | oui | oui | oui | oui | partiel | oui | iOS : Associated Domains (`webcredentials`) ; Android : Credential Manager | `webview` ; pont natif |
| `attestation` | attestation d'intégrité de l'app/appareil (App Attest, Play Integrity) | oui | oui | partiel (TPM) | oui | non | non | iOS : capacité App Attest ; Android : Play Integrity (Google Play Services) | pont natif |
| `app_lock` | verrouiller l'app derrière une authentification | oui | oui | oui | oui | partiel | partiel | comme `biometric` | **`face_id`** + `keychain_secrets` |
| `certificate_trust` | certificats clients, épinglage, autorités | oui | oui | oui | oui | oui | non | iOS : profil de configuration | pont natif |
| `device_admin` | stratégies d'entreprise, effacement à distance | partiel (MDM) | partiel [E] | partiel | partiel | non | non | MDM / `BIND_DEVICE_ADMIN` | pont natif, **à vérifier** |
| `sandbox_info` | connaître ses propres droits (TCC, entitlements) | oui | oui | partiel | oui | partiel | partiel (Permissions API) | aucune | `webview` ; pont natif |
| `permission_state` | état et demande d'une permission donnée | oui | oui | partiel | oui | non | oui | aucune | slot par capacité (`*Status`/`*Request`) |
| `anti_screenshot_content` | masquer un contenu sensible (voir `screen_secure`) | partiel | oui | partiel | partiel | non | non | aucune | pont natif |

## 11. Identité, contacts et données personnelles

| Nom | Permet | iOS | Android | Win | macOS | Linux | Web | Permission / entitlement | Qui |
|---|---|---|---|---|---|---|---|---|---|
| **`contacts`** | lire, chercher, créer des contacts | oui | oui | partiel | oui | non | partiel (Contact Picker, Chromium Android) | `NSContactsUsageDescription` ; `READ_CONTACTS` / `WRITE_CONTACTS` ; le champ **notes** exige `com.apple.developer.contacts.notes` [E] | slot (iOS, macOS) ; pont natif Android |
| `contacts_update_delete` | modifier / supprimer un contact | oui | oui | partiel | oui | non | non | comme `contacts` ; **consentement distinct** (écriture destructrice) | pont natif (proposé, non fait) |
| **`calendar`** | événements : lire, créer, modifier, supprimer | oui | oui | partiel | oui | non | non | iOS 17+ : `NSCalendarsFullAccessUsageDescription` ; Android : `READ_CALENDAR` / `WRITE_CALENDAR` | slot (iOS, macOS) |
| `reminders` | rappels / tâches du système | oui | partiel | partiel | oui | non | non | `NSRemindersFullAccessUsageDescription` | pont natif |
| **`health`** | santé : quantités, séances (lecture) | oui | oui (Health Connect) | non | non | non | non | iOS : HealthKit [E] (entitlement + App ID explicite) [mesuré] ; Android : permissions Health Connect par type, revue Play | slot (iOS) ; pont natif Android |
| `health_write` | **écrire** dans la santé | oui | oui | non | non | non | non | comme `health` ; **volontairement non exposé** (irréversible, non attribuable) | à n'ouvrir qu'avec consentement propre |
| `health_clinical` | dossiers médicaux (FHIR) | oui [E] | non | non | non | non | non | entitlement HealthKit Clinical Records [E] (approbation Apple) | pont natif, **à vérifier** |
| `fitness_workouts` | séances en direct, parcours | oui | oui | non | non | non | non | HealthKit / Health Connect ; `UIBackgroundModes: workout-processing` | pont natif |
| `sleep_data` | sommeil | oui | oui | non | non | non | non | comme `health` | `health` |
| `location_history` | historique de position, lieux fréquents | non | partiel | non | non | non | non | pas d'API publique iOS ; Android : export Maps seul | **impossible** (voir « difficile ») |
| `usage_stats` | temps d'écran, apps utilisées | partiel (Screen Time API [E]) | oui [E] | non | non | non | non | iOS : entitlement Family Controls [E] ; Android : `PACKAGE_USAGE_STATS` (accès spécial) | pont natif |
| `user_profile` | nom/identité de l'utilisateur | partiel | partiel | oui | oui | oui | non | « Se connecter avec Apple », Google Sign-In | pont natif |
| `accounts_sso` | comptes du système, SSO | oui | oui | oui | oui | non | partiel (FedCM) | iOS : ASAuthorization ; Android : `GET_ACCOUNTS` (restreint) | pont natif |
| `tracking_id` | identifiant publicitaire | oui | oui | oui | oui | non | non | iOS : `NSUserTrackingUsageDescription` (ATT) ; Android : `AD_ID` | pont natif ; **à ne pas promettre** |
| `family_sharing` | partage familial, contrôle parental | partiel [E] | partiel | partiel | partiel | non | non | Family Controls [E] | pont natif, **à vérifier** |

## 12. Paiement et commerce

| Nom | Permet | iOS | Android | Win | macOS | Linux | Web | Permission / entitlement | Qui |
|---|---|---|---|---|---|---|---|---|---|
| `payments_wallet` | payer via Apple Pay / Google Pay | oui [E] | oui | partiel | oui [E] | non | partiel (Payment Request) | iOS : Merchant ID + certificat de traitement [E] ; Android : Google Pay API | pont natif ; `webview` (Payment Request) |
| `in_app_purchase` | achats intégrés / abonnements | oui [E] | oui [E] | oui | oui [E] | non | non | iOS : StoreKit, règles App Store (**obligatoire** pour les biens numériques) ; Android : Play Billing | pont natif ; commu, à vérifier |
| `wallet_passes` | ajouter un billet / une carte à Wallet | oui | oui | non | oui | non | non | iOS : certificat de pass ; Android : Google Wallet API | pont natif |
| `tap_to_pay` | accepter un paiement sans contact sur l'appareil | oui [E] | partiel | non | non | non | non | iOS : entitlement Tap to Pay on iPhone [E] (approbation) | pont natif, **à vérifier** |
| `secure_element_access` | puce sécurisée (carte, clé de voiture) | **non** | partiel (OMAPI, apps signées) | non | non | non | non | voir « difficile » | **impossible** |
| `digital_id` | identité numérique (permis, identité) | partiel [E] | partiel [E] | non | non | non | partiel (Digital Credentials API) | entitlement Apple ID/Identity [E], à vérifier | pont natif, **à vérifier** |
| `car_keys` | clés de voiture / maison (CarKey, Matter) | partiel [E] | partiel | non | non | non | non | entitlement [E] | pont natif, **à vérifier** |

## 13. Communication

| Nom | Permet | iOS | Android | Win | macOS | Linux | Web | Permission / entitlement | Qui |
|---|---|---|---|---|---|---|---|---|---|
| `sms_compose` | préparer un SMS (l'utilisateur envoie) | oui | oui | non | oui | non | partiel (lien `sms:`) | aucune ; **geste utilisateur obligatoire** | plugin:opener (`sms:`) ; pont natif |
| `sms_send_read` | envoyer / lire des SMS sans geste | **non** | partiel [E] | non | non | non | non | Android : `SEND_SMS` / `READ_SMS` [E] (réservé à l'app SMS par défaut, politique Play) | pont natif |
| `phone_call` | lancer un appel (composeur) | oui | oui | partiel | partiel | non | partiel (`tel:`) | Android : `CALL_PHONE` pour l'appel direct | plugin:opener (`tel:`) ; pont natif |
| `phone_state_log` | état d'appel, journal d'appels | non | partiel [E] | non | non | non | non | Android : `READ_CALL_LOG` [E] (politique Play) | pont natif |
| `voip_call` | appel VoIP à l'écran verrouillé | oui | oui | oui | oui | oui | oui | iOS : CallKit + PushKit ; Android : `ConnectionService` | pont natif ; `webview` (WebRTC) |
| `mail_compose` | préparer un e-mail | oui | oui | oui | oui | oui | partiel (`mailto:`) | aucune | plugin:opener (`mailto:`) ; pont natif |
| `mail_read_send` | lire / envoyer via le compte local | non | non | partiel | partiel | non | non | passer par le fournisseur (IMAP/OAuth) | module réseau, pas l'appareil |
| `messaging_apps` | envoyer vers WhatsApp, Telegram… | partiel | partiel | partiel | partiel | partiel | partiel | schéma d'URL / intent | `intents_outgoing` |
| `contacts_picker` | choisir un contact sans accès global | oui | oui | non | oui | non | partiel | aucune | pont natif |
| `intercom_presence` | présence, statut de disponibilité | partiel | partiel | partiel | partiel | partiel | partiel | à vérifier | pont natif, **à vérifier** |

## 14. Réalité augmentée, 3D et calcul

| Nom | Permet | iOS | Android | Win | macOS | Linux | Web | Permission / entitlement | Qui |
|---|---|---|---|---|---|---|---|---|---|
| `ar_world_tracking` | suivi du monde, plans, ancres | oui (ARKit) | oui (ARCore) | partiel | partiel | non | partiel (WebXR) | caméra ; ARCore : Google Play Services for AR | pont natif ; `webview` (WebXR, Android Chrome) |
| `ar_scene_mesh` | maillage de la scène, occlusion, profondeur | partiel (LiDAR) | partiel (ToF / Depth API) | non | non | non | non | caméra | pont natif |
| `ar_face_body_hand` | suivi du visage, du corps, des mains | oui | partiel | non | partiel | non | non | caméra | pont natif |
| `ar_geo_anchors` | ancres géospatiales | partiel (villes couvertes) | partiel (Geospatial API) | non | non | non | non | caméra + localisation ; couverture variable | pont natif, **à vérifier** |
| `xr_headset` | casques VR/AR (visionOS, Quest) | partiel (visionOS) | partiel | partiel | non | non | partiel (WebXR) | visionOS : entitlements d'accès enterprise [E] | pont natif, **à vérifier** |
| `gpu_graphics` | rendu GPU (Metal, Vulkan, DirectX, WebGPU) | oui | oui | oui | oui | oui | oui (WebGPU/WebGL) | aucune | `webview` ; `wgpu` natif |
| `gpu_compute` | calcul GPU généraliste | oui | oui | oui | oui | oui | partiel (WebGPU) | aucune | pont natif ; `webview` |
| `neural_engine` | inférence sur NPU / Neural Engine | oui (Core ML) | oui (NNAPI / LiteRT) | oui (DirectML) | oui | partiel | partiel (WebNN, expérimental) | aucune | pont natif, **à vérifier** |
| `ondevice_llm` | LLM embarqué du système (Apple Foundation Models, Gemini Nano) | partiel (Apple Intelligence, appareils compatibles) | partiel (AICore, certains appareils) | partiel | partiel | non | partiel (Prompt API, expérimental) | **à vérifier** | pont natif, **à vérifier** |
| `physical_sim_haptic` | retour haptique avancé (CoreHaptics) | oui | partiel | non | non | non | non | aucune | pont natif |
| `spatial_audio` | audio spatial, suivi de la tête | oui | oui | partiel | oui | non | partiel | `NSMotionUsageDescription` pour la tête | pont natif |

## 15. Accessibilité

| Nom | Permet | iOS | Android | Win | macOS | Linux | Web | Permission / entitlement | Qui |
|---|---|---|---|---|---|---|---|---|---|
| `screen_reader_state` | savoir si VoiceOver / TalkBack est actif | oui | oui | oui | oui | partiel | non | aucune | pont natif |
| `a11y_tree` | exposer sa propre interface aux lecteurs d'écran | oui | oui | oui | oui | oui | oui (ARIA) | aucune | `webview` (ARIA) |
| `a11y_announce` | faire annoncer un message | oui | oui | oui | oui | partiel | oui (`aria-live`) | aucune | `webview` ; pont natif |
| `reduce_motion_prefs` | réduction des animations, transparence, contraste | oui | oui | oui | oui | oui | oui | aucune | `webview` (media queries) |
| `switch_control` | contrôle par commutateur, par voix | oui | oui | oui | oui | partiel | non | aucune | support par l'interface, pas d'API |
| `a11y_service` | **observer et piloter d'autres apps** | non | oui [E] | oui | oui | oui | non | Android : `BIND_ACCESSIBILITY_SERVICE` [E] (politique Play, déclaration stricte) ; macOS : « Accessibilité » (TCC) | pont natif (voir « difficile ») |
| `captions_live` | sous-titres / transcription en direct du système | oui | oui | oui | oui | non | partiel | aucune | pont natif, **à vérifier** |
| `color_filters` | daltonisme, loupe | oui | oui | oui | oui | partiel | non | aucune | support par l'interface |

## 16. Multi-fenêtres

| Nom | Permet | iOS | Android | Win | macOS | Linux | Web | Permission / entitlement | Qui |
|---|---|---|---|---|---|---|---|---|---|
| `multi_window` | plusieurs fenêtres d'une même app | partiel (iPad : scènes) | partiel (multi-instance, 12L+) | oui | oui | oui | oui | iOS : `UIApplicationSupportsMultipleScenes` | `tauri` (fenêtres, bureau) ; pont natif mobile |
| `split_view` | écran partagé, Stage Manager, bureau libre | oui (iPad) | oui | oui | oui | oui | non | aucune | pont natif |
| `pip_window` | fenêtre flottante (PiP) | oui (vidéo/appel) | oui | partiel | oui | partiel | oui (Document PiP, Chromium) | iOS : `UIBackgroundModes: audio` + `AVPictureInPicture` | `webview` ; pont natif |
| `popup_overlay` | afficher par-dessus d'autres apps | **non** | oui [E] | oui | oui | partiel | non | Android : `SYSTEM_ALERT_WINDOW` [E] (accès spécial) | pont natif |
| `window_per_display` | une fenêtre par écran | oui | partiel | oui | oui | oui | partiel | aucune | `tauri` |
| `window_chrome` | barre de titre, transparence, forme de fenêtre | non | non | oui | oui | oui | non | aucune | `tauri` ; plugin:window-state |
| `window_state_restore` | restaurer taille/position | non | non | oui | oui | oui | non | aucune | plugin:window-state |
| `positioner` | placer près de l'icône de barre | non | non | oui | oui | oui | non | aucune | plugin:positioner |

## 17. Extensions et partage entrant

| Nom | Permet | iOS | Android | Win | macOS | Linux | Web | Permission / entitlement | Qui |
|---|---|---|---|---|---|---|---|---|---|
| `share_receive` | recevoir du contenu partagé depuis une autre app | oui (extension de partage) | oui (intent filter) | partiel | oui | partiel | partiel (Web Share Target) | iOS : cible d'extension + App Group (capacité) ; **code natif séparé** | pont natif |
| `file_open_with` | être proposé pour des types de fichiers | oui | oui | oui | oui | oui | partiel (File Handling, Chromium) | manifeste (UTI / intent filter) | pont natif ; plugin:deep-link (URL seulement) |
| `share_extension_data` | partager des données entre l'app et l'extension | oui (App Group) | oui | partiel | oui | non | non | iOS : App Group (capacité) | pont natif |
| `keyboard_extension` | clavier personnalisé | oui | oui (IME) | oui | oui | oui | non | iOS : `RequestsOpenAccess` ; données saisies sensibles | pont natif |
| `app_extensions_other` | extensions de filtre d'appel, de réseau, de fournisseur de fichiers, de saisie auto | oui [E] | partiel | non | oui | non | non | entitlements spécifiques [E] | pont natif, **à vérifier** |
| `spotlight_search_index` | indexer le contenu pour la recherche système | oui | oui (AppSearch) | oui | oui | partiel | non | aucune | pont natif |
| `autofill_provider` | fournisseur de mots de passe / saisie automatique | oui | oui | partiel | oui | non | non | iOS : extension de fournisseur de credentials ; Android : `BIND_AUTOFILL_SERVICE` | pont natif |
| `car_integration` | CarPlay / Android Auto | oui [E] | oui [E] | non | non | non | non | iOS : entitlement CarPlay [E] (catégorie approuvée) ; Android Auto : catégorie de l'app Play | pont natif |
| `watch_companion` | app de montre associée (WatchConnectivity / Wear Data Layer) | oui | oui | non | non | non | non | app compagnon, cible séparée | pont natif, **à vérifier** |
| `tv_platform` | tvOS, Android TV | partiel | partiel | non | non | non | non | cible séparée | pont natif, **à vérifier** |

## Ce qui est réellement difficile ou impossible

À lire **avant** de promettre quoi que ce soit. Trois sortes de « non » : *interdit par le système* (rien à faire), *réservé à Apple/Google après approbation* (se demande, ne se garantit pas), *vrai seulement avec un appareil en face* (à mesurer).

**A. Impossible pour une app ordinaire, aucun module n'y changera rien**

| Capacité | Pourquoi |
|---|---|
| Scan Wi-Fi sur iOS | aucune API publique ; `NEHotspotHelper` est réservé aux apps approuvées par Apple |
| Scan Wi-Fi libre sur Android | 4 scans par 2 minutes (9+), localisation exigée |
| NFC iOS hors NDEF / ISO 7816 déclaré | CoreNFC borné ; émulation de carte réservée (UE/Japon seulement, à vérifier) |
| Secure Element / Apple Pay | puce sécurisée fermée aux apps tierces |
| Capteurs d'un autre appareil | chaque appareil garde ses capteurs ; il faut une app sur l'autre appareil |
| IMEI, numéro SIM, ICCID | réservés aux apps système (Android 10+), absents sur iOS |
| SMS / appels sans geste (iOS) | ouverture du composeur seulement, l'utilisateur valide |
| Lecture du journal d'appels et des SMS (Android) | politique Play : app par défaut seulement |
| Historique de position du système | aucune API iOS ; Android : export manuel seul |
| Observer le presse-papiers (mobile) | lecture bloquée en arrière-plan, bandeau d'avertissement iOS |
| Lecture des notifications d'autres apps (iOS) | pas d'API ; Android : accès spécial à accorder à la main |
| Démarrage auto au boot (iOS) | aucun mécanisme |
| Exécuter du code natif téléchargé (iOS) | règle App Store 2.5.2 ; seul le code interprété (JS/wasm) passe |
| Exécuter un programme tiers (iOS, Android) | pas de shell ni de `fork` hors sandbox |
| Biométrie brute (gabarit Face ID/empreinte) | jamais exposée ; seulement un oui/non |
| Clés du Secure Enclave / StrongBox | non exportables par conception |
| Données de santé écrites par un modèle | non attribuables ni réversibles : choix de conception, `health_write` hors d'atteinte par défaut |
| Capteurs absents du matériel | infrarouge, thermomètre, humidité : rares ou nuls |
| Satellite, SOS, eSIM | non exposés |
| Hot-word permanent | micro en arrière-plan interdit/bridé |
| Superposition sur d'autres apps (iOS) | pas d'API |

**B. Possible mais soumis à approbation (entitlement / revue)** : HealthKit, NFC (profil explicite, pas wildcard) [mesuré], alertes critiques, CarPlay, Tap to Pay, Family Controls, Network Extension (VPN), Wi-Fi Info/Hotspot Configuration, Apple Pay (Merchant ID), contacts-notes, `MANAGE_EXTERNAL_STORAGE`, `QUERY_ALL_PACKAGES`, `SCHEDULE_EXACT_ALARM`, `ACCESS_BACKGROUND_LOCATION`, `SYSTEM_ALERT_WINDOW`, service d'accessibilité. Chacun peut être **refusé** : un module qui en dépend doit prévoir de répondre `not_supported`, pas planter.

**C. Écarts de modèle qui piègent un module** (voir `docs/appareil.md`) : identifiants Bluetooth opaques (UUID vs MAC) ; permissions **révocables** entre deux appels ; ce qui n'existe pas ne s'annonce pas (`live_activity` sur Android).

**D. Le Web / wasm ne peut pas**, par construction : Bluetooth hors Chromium, NFC hors Chrome Android, fichiers hors OPFS ailleurs, Wi-Fi, SMS, contacts hors Chromium Android, arrière-plan. C'est le sol de la plateforme, pas une lacune à combler.

**E. Les modules « natifs » sont du code natif** : widgets, extensions de partage, Live Activities, CarPlay, tuiles, services au premier plan sont des **cibles de build séparées** ; un module **ne peut pas les créer à chaud** sur iOS. Ce qu'un module peut faire à chaud, c'est de la **donnée et du programme interprété** qui pilotent un châssis déjà équipé. « Tout créer » = le châssis embarque les cibles, le module les remplit.

**F. Écart de vérité** : les `à vérifier` de ce document ne sont pas des suppositions raisonnables ; ce sont des cases où l'auteur ne peut pas affirmer. À mesurer sur appareil avant d'ouvrir le nom dans `surface.rs`.
