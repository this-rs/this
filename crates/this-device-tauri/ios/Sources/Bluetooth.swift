// One Bluetooth scan of a fixed duration (CoreBluetooth, central role).
//
// Flow: create the central manager (this is what makes iOS ask for the permission the first
// time), wait for `poweredOn`, scan for `duration_ms`, answer with the devices seen, deduplicated
// by identifier. Requires NSBluetoothAlwaysUsageDescription in Info.plist, or the system
// terminates the app on the first access.

import CoreBluetooth
import Foundation
import Tauri

final class BluetoothScan: NSObject, CBCentralManagerDelegate {
  /// How long to wait for the radio to report `poweredOn` before giving up.
  static let readyTimeoutSeconds: Double = 5

  // The scan keeps itself alive until it answers: the caller only holds the `Invoke`.
  private static var active: [ObjectIdentifier: BluetoothScan] = [:]

  private let invoke: Invoke
  private let durationSeconds: Double
  private let services: [CBUUID]?
  private var central: CBCentralManager?
  private var scanning = false
  private var finished = false
  private var order: [String] = []
  private var found: [String: BtDevice] = [:]

  private init(invoke: Invoke, durationMs: Int, services: [CBUUID]?) {
    self.invoke = invoke
    self.durationSeconds = Double(durationMs) / 1000
    self.services = services
    super.init()
  }

  /// The permission as iOS records it, read without prompting. `CBManager.authorization` is
  /// iOS 13.1 and later; on iOS 13.0 it is unknown (nil) and the manager's state decides.
  static func authorization() -> CBManagerAuthorization? {
    if #available(iOS 13.1, *) {
      return CBManager.authorization
    }
    return nil
  }

  /// Called on the main thread with arguments the Rust side has already validated.
  static func run(_ invoke: Invoke, durationMs: Int, services: [String]?) {
    // Read without prompting: a denied or restricted permission cannot be scanned anyway.
    if let auth = authorization(), auth == .denied || auth == .restricted {
      invoke.resolve(Reply.error(code: ReplyCode.notAuthorized))
      return
    }
    let uuids = services?.map { CBUUID(string: $0) }
    let scan = BluetoothScan(invoke: invoke, durationMs: durationMs, services: uuids)
    active[ObjectIdentifier(scan)] = scan
    scan.central = CBCentralManager(delegate: scan, queue: .main)
    DispatchQueue.main.asyncAfter(deadline: .now() + readyTimeoutSeconds) { [weak scan] in
      guard let scan = scan, !scan.scanning else { return }
      scan.finish(Reply.error(code: ReplyCode.timeout))
    }
  }

  func centralManagerDidUpdateState(_ central: CBCentralManager) {
    switch central.state {
    case .poweredOn:
      guard !scanning, !finished else { return }
      scanning = true
      central.scanForPeripherals(
        withServices: services,
        options: [CBCentralManagerScanOptionAllowDuplicatesKey: true])
      DispatchQueue.main.asyncAfter(deadline: .now() + durationSeconds) { [weak self] in
        self?.finish(Reply.devices(self?.snapshot() ?? []))
      }
    case .unauthorized:
      finish(Reply.error(code: ReplyCode.notAuthorized))
    case .poweredOff, .unsupported:
      finish(Reply.error(code: ReplyCode.unavailable))
    case .unknown, .resetting:
      // Transient: the manager reports the real state next. The ready timeout covers a stall.
      break
    @unknown default:
      finish(Reply.error(code: ReplyCode.unavailable))
    }
  }

  func centralManager(
    _ central: CBCentralManager,
    didDiscover peripheral: CBPeripheral,
    advertisementData: [String: Any],
    rssi RSSI: NSNumber
  ) {
    // 127 is CoreBluetooth's "no reading" marker: keep the last real value instead.
    let id = peripheral.identifier.uuidString
    let name = peripheral.name ?? (advertisementData[CBAdvertisementDataLocalNameKey] as? String)
    let rssi = RSSI.intValue == 127 ? found[id]?.rssi : RSSI.intValue
    guard let value = rssi else { return }
    if found[id] == nil { order.append(id) }
    // A name seen later is kept; an earlier one is not erased by a later advertisement without it.
    let knownName = found[id]?.name
    found[id] = BtDevice(id: id, name: name ?? knownName, rssi: value)
  }

  /// The devices seen so far, in the order they were first seen, each once.
  private func snapshot() -> [BtDevice] {
    order.compactMap { found[$0] }
  }

  private func finish(_ reply: Reply) {
    guard !finished else { return }
    finished = true
    if scanning { central?.stopScan() }
    central?.delegate = nil
    central = nil
    invoke.resolve(reply)
    BluetoothScan.active[ObjectIdentifier(self)] = nil
  }
}
