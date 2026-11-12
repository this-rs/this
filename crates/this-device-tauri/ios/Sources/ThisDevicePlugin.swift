// Entry point of the iOS side. The Rust plugin calls these methods by name (`run_mobile_plugin`).
// Every method answers with a `Reply` (see Reply.swift): errors are answers too, so Rust
// never has to translate a native rejection.

import CoreBluetooth
import CoreMotion
import Foundation
import Tauri

struct StatusArgs: Decodable {
  let capability: String
}

struct BluetoothScanArgs: Decodable {
  let durationMs: Int
  let services: [String]?

  private enum CodingKeys: String, CodingKey {
    case durationMs = "duration_ms"
    case services
  }
}

class ThisDevicePlugin: Plugin {
  /// Les classes qui gardent vivantes les méthodes des familles.
  ///
  /// **Un `#selector(...)` ne fait pas ce travail.** Il produit une CHAÎNE, pas une référence au
  /// symbole de la méthode : le dead-strip retire donc l'objet entier, et le pont répond
  /// « No command … found for plugin this-device ». C'était le mécanisme précédent
  /// (`famillesLiees`), et il ne gardait rien — mesuré le 13/11/2026 sur un iPhone 16 Pro : les
  /// dix-huit fichiers de famille étaient présents dans `libapp.a`, leurs sélecteurs étaient bien
  /// dans le binaire (comme chaînes), et dix-huit méthodes répondaient « No command ». La caméra,
  /// les contacts, la photothèque, le calendrier, HealthKit, Face ID, la parole, le NFC, le
  /// réseau local, l'activité motrice, les notifications, la Live Activity, le presse-papiers, la
  /// localisation et les six lectures d'état système étaient **injoignables**.
  ///
  /// Ce qui marche : référencer une CLASSE de chaque fichier. Le lien garde l'objet qui la
  /// définit, donc toutes les méthodes qu'il contient. Chaque fichier de famille porte donc une
  /// `Famille<Nom>` vide, et cette liste est la seule raison de sa présence.
  ///
  /// La liste est lue par `listCommands` — un vrai usage, donc l'optimiseur ne peut pas la
  /// retirer — et `tests/swift_parity.rs` vérifie qu'aucun fichier de famille n'en manque.
  static let famillesAncrees: [AnyObject.Type] = [
    FamilleBluetooth.self,
    FamilleCalendar.self,
    FamilleCameraMic.self,
    FamilleClipboardShare.self,
    FamilleContacts.self,
    FamilleFaceId.self,
    FamilleFiles.self,
    FamilleHealth.self,
    FamilleLiveActivity.self,
    FamilleLocalNetwork.self,
    FamilleLocation.self,
    FamilleMotionActivity.self,
    FamilleNfc.self,
    FamilleNotifications.self,
    FamillePhotos.self,
    FamilleSpeech.self,
    FamilleStreams.self,
    FamilleSystemState.self,
  ]

  override init() {
    super.init()
    // Force la lecture de la liste au moment où l'instance est construite : une constante
    // statique peut rester paresseuse, et une liste jamais lue ne garde rien vivant.
    _ = ThisDevicePlugin.famillesAncrees.count
  }

  /// Les familles réellement liées dans ce binaire. Diagnostic, pas un appel de famille : il
  /// passe autour du contrat exprès, parce que le contrat est ce dont on doute.
  ///
  /// Sans cette méthode, `this_device_tauri::commands()` échouait toujours : il appelait
  /// `listCommands`, qui n'existait pas côté natif.
  @objc public func listCommands(_ invoke: Invoke) {
    invoke.resolve([
      "kind": "commands",
      "plugin": "this-device",
      "familles": ThisDevicePlugin.famillesAncrees.map { NSStringFromClass($0) },
    ] as JsonObject)
  }

  /// The permission state of a capability, read without prompting the person.
  @objc public func status(_ invoke: Invoke) throws {
    let args = try invoke.parseArgs(StatusArgs.self)
    switch args.capability {
    case "gyroscope":
      // No permission exists for raw gyroscope data: the state is whether the hardware is there.
      let available = CMMotionManager().isGyroAvailable
      invoke.resolve(Reply.status(state: available ? "granted" : "unsupported"))
    case "bluetooth":
      invoke.resolve(Reply.status(state: BluetoothStatus.name(BluetoothScan.authorization())))
    default:
      // Not a bridged capability: the bridge cannot tell, so it says so.
      invoke.resolve(Reply.status(state: "unsupported"))
    }
  }

  @objc public func gyroscopeSample(_ invoke: Invoke) {
    GyroscopeSession.sample(invoke)
  }

  @objc public func bluetoothScan(_ invoke: Invoke) throws {
    let args = try invoke.parseArgs(BluetoothScanArgs.self)
    BluetoothScan.run(invoke, durationMs: args.durationMs, services: args.services)
  }
}

/// The permission state of Bluetooth, in the contract's words (`PermissionState`).
enum BluetoothStatus {
  static func name(_ authorization: CBManagerAuthorization?) -> String {
    switch authorization {
    case .allowedAlways?: return "granted"
    case .denied?, .restricted?: return "denied"
    // Not asked yet, or unknown before iOS 13.1: the system asks the first time it is used.
    case .notDetermined?, nil: return "prompt"
    @unknown default: return "prompt"
    }
  }
}

@_cdecl("init_plugin_this_device")
public func initPlugin() -> Plugin {
  return ThisDevicePlugin()
}
