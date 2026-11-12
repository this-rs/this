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
cargo tauri ios init --ci          # une fois : écrit gen/apple
cargo tauri ios build --target aarch64-sim --debug
xcrun simctl install <udid> gen/apple/build/arm64-sim/BancAppareil.app
SIMCTL_CHILD_THIS_DEVICE_BANC=1 xcrun simctl launch <udid> dev.thisrs.bancappareil
xcrun simctl get_app_container <udid> dev.thisrs.bancappareil data
# → <conteneur>/Documents/banc.json et banc.log
```

## Lancer sur un iPhone

```sh
cargo tauri ios build --target aarch64 --debug
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
| `refuse_argument` | le pont a refusé avant le natif, un argument manque | **oui** |
| `panne` | le pont a échoué (`internal`) | **oui** |
| `delai` | le natif n'a pas répondu dans la borne | **oui** |
| `non_exerce` | le banc ne l'a pas appelée (permission, geste, écriture) | non |

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
