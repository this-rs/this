// Le vocabulaire d'état du contrat, une seule fois, pour iOS ET macOS.
//
// Ce fichier est compilé dans les DEUX arbres, et c'est ce qui en fait du commun :
//   • côté iOS il appartient au paquet SPM (`ios/Package.swift` a `path: "Sources"`, qui inclut
//     les sous-répertoires) ;
//   • côté macOS `build.rs` compile `macos/Sources/` depuis une liste explicite — il ajoute
//     `ios/Sources/Commun/` à cette liste.
//
// Il ne doit donc dépendre NI de `ThisDevicePlugin` NI d'un framework d'une seule plateforme :
// c'est un vocabulaire, rien de plus. Le test `swift_parity::l_etat_swift_dit_les_memes_mots_que_le_contrat`
// le tient contre `PermissionState`.

import Foundation

/// Les cinq mots que `this_platform::device_bridge::PermissionState` met sur le fil.
///
/// Écrits à la main **195 fois** avant ce fichier (102 côté iOS, 93 côté macOS) : c'est le seul
/// endroit du pont où une faute de frappe change le sens d'une réponse sans que rien ne le
/// signale — et c'est la classe exacte du défaut du 13/11/2026, où `faceIdStatus` ne rendait pas
/// `state` du tout et où le pont lisait `unsupported` sur un appareil qui a un Face ID.
///
/// Ici, une faute devient une erreur de compilation.
enum Etat: String, CaseIterable {
  case granted
  case prompt
  case denied
  case limited
  case unsupported

  /// Le nom de fil — ce que le pont lit.
  var fil: String { rawValue }
}
