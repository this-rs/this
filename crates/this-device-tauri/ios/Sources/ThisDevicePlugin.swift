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
  /// Prises de référence sur les méthodes des familles, pour forcer l'éditeur de liens à
  /// garder leur fichier objet.
  ///
  /// Mesuré le 06/11 : `captureStatus:`, `clipboardRead:`, `faceIdStatus:` et
  /// `locationSample:` sont présents dans `libapp.a` et **absents du binaire final**. Ce sont
  /// des méthodes `@objc` sur une extension, que rien n'appelle statiquement : le dead-strip
  /// supprime l'objet entier, et le pont répond « No command … found for plugin this-device ».
  /// La caméra, le micro, le presse-papiers, Face ID et la localisation étaient donc
  /// inaccessibles alors que leur code compilait.
  ///
  /// Une référence par fichier suffit. Elle est volontairement portée par une propriété
  /// stockée, initialisée avec l'instance : un global `let` serait lui-même paresseux et
  /// pourrait être retiré.
  private let famillesLiees: [Selector]

  override init() {
    famillesLiees = [
      #selector(ThisDevicePlugin.captureStatus(_:)),
      #selector(ThisDevicePlugin.captureRequest(_:)),
      #selector(ThisDevicePlugin.liveActivityStatus(_:)),
      #selector(ThisDevicePlugin.clipboardRead(_:)),
      #selector(ThisDevicePlugin.clipboardWrite(_:)),
      #selector(ThisDevicePlugin.shareText(_:)),
      #selector(ThisDevicePlugin.faceIdStatus(_:)),
      #selector(ThisDevicePlugin.faceIdConfirm(_:)),
      #selector(ThisDevicePlugin.locationRequest(_:)),
      #selector(ThisDevicePlugin.locationSample(_:)),
      // Bluetooth au-delà du scan : le moteur est dans `BluetoothCentral.swift`, les
      // sélecteurs dans `Bluetooth.swift` (en extension). Sans cette référence, le dead-strip
      // les retire — c'est exactement le défaut mesuré ci-dessus sur quatre autres fichiers.
      #selector(ThisDevicePlugin.bluetoothConnect(_:)),
      // État système : lectures passives, sans consentement. Sans ces références, le
      // dead-strip retire le fichier `system_state.swift` entier — même défaut mesuré
      // le 06/11 sur caméra, presse-papiers, Face ID et localisation.
      #selector(ThisDevicePlugin.deviceInfo(_:)),
      #selector(ThisDevicePlugin.batteryLevel(_:)),
      #selector(ThisDevicePlugin.networkState(_:)),
      #selector(ThisDevicePlugin.locale(_:)),
      #selector(ThisDevicePlugin.screenState(_:)),
      #selector(ThisDevicePlugin.volume(_:)),
      // Flux d'état système (`streams.swift`) : même défaut de dead-strip sans référence.
      #selector(ThisDevicePlugin.streamStart(_:)),
      #selector(ThisDevicePlugin.streamStop(_:)),
    ]
    super.init()
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
