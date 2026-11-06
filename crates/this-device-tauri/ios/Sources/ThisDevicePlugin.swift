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
