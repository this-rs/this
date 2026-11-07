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

// ---- Beyond the scan ------------------------------------------------------------------
//
// A connection, and what it makes possible. The engine is `BluetoothCentral.swift` (CoreBluetooth
// only, type-checked on its own); everything here parses the arguments and resolves the
// invocation, exactly as the other families do.
//
// Contrat des réponses : voir `families/bluetooth.rs` (`BluetoothReply`). Aucun `reject` : une
// erreur est une réponse, avec le code du contrat.

/// Les arguments d'un appel qui ne nomme qu'un périphérique.
private struct BluetoothIdArgs: Decodable {
  let id: String
}

/// Les arguments d'un appel qui nomme un périphérique et une caractéristique.
private struct BluetoothCharArgs: Decodable {
  let id: String
  let characteristic: String
}

/// `bluetoothConnect` : le périphérique, et une borne de temps que le pont a déjà bornée.
private struct BluetoothConnectArgs: Decodable {
  let id: String
  let timeoutMs: Int

  private enum CodingKeys: String, CodingKey {
    case id
    case timeoutMs = "timeout_ms"
  }
}

/// `bluetoothWrite` : la charge utile en base64, et l'accusé demandé ou non.
private struct BluetoothWriteArgs: Decodable {
  let id: String
  let characteristic: String
  let dataBase64: String
  let withResponse: Bool

  private enum CodingKeys: String, CodingKey {
    case id, characteristic
    case dataBase64 = "data_base64"
    case withResponse = "with_response"
  }
}

private struct BtStateAnswer: Encodable {
  let kind = "state"
  let state: String
}

/// `{"kind":"device","id":..,"name":null,"state":"connected"}` — `name` est écrit même quand
/// il est absent (`null`), comme `Reply` le fait pour un périphérique sans nom.
private struct BtDeviceAnswer: Encodable {
  let kind = "device"
  let id: String
  let name: String?
  let state: String

  private enum Key: String, CodingKey {
    case kind, id, name, state
  }

  func encode(to encoder: Encoder) throws {
    var c = encoder.container(keyedBy: Key.self)
    try c.encode(kind, forKey: .kind)
    try c.encode(id, forKey: .id)
    try c.encode(name, forKey: .name)
    try c.encode(state, forKey: .state)
  }
}

private struct BtServicesAnswer: Encodable {
  let kind = "services"
  let services: [BtServiceInfo]
}

private struct BtValueAnswer: Encodable {
  let kind = "value"
  let characteristic: String
  let data_base64: String
}

private struct BtWrittenAnswer: Encodable {
  let kind = "written"
  let bytes: Int
}

private struct BtSubscribedAnswer: Encodable {
  let kind = "subscribed"
  let characteristic: String
}

private struct BtNotificationsAnswer: Encodable {
  let kind = "notifications"
  let notifications: [BtNotification]
}

extension ThisDevicePlugin {
  /// L'état de la RADIO (allumée, éteinte, refusée, absente) — distinct de la permission que
  /// rend `status`. Un téléphone dont le Bluetooth est coupé n'est pas « non autorisé ».
  @objc public func bluetoothState(_ invoke: Invoke) {
    BluetoothCentral.shared.radioState { state in
      invoke.resolve(BtStateAnswer(state: state))
    }
  }

  /// Se connecter à un périphérique qu'un scan a rendu (son `id`), avec une borne de temps.
  @objc public func bluetoothConnect(_ invoke: Invoke) throws {
    let args = try invoke.parseArgs(BluetoothConnectArgs.self)
    BluetoothCentral.shared.connect(
      id: args.id,
      timeoutSeconds: Double(args.timeoutMs) / 1000
    ) { result in
      switch result {
      case let .success(peer):
        invoke.resolve(BtDeviceAnswer(id: peer.id, name: peer.name, state: peer.state))
      case let .failure(failure):
        invoke.resolve(Reply.error(code: failure.code))
      }
    }
  }

  @objc public func bluetoothDisconnect(_ invoke: Invoke) throws {
    let args = try invoke.parseArgs(BluetoothIdArgs.self)
    BluetoothCentral.shared.disconnect(id: args.id) { result in
      switch result {
      case let .success(peer):
        invoke.resolve(BtDeviceAnswer(id: peer.id, name: peer.name, state: peer.state))
      case let .failure(failure):
        invoke.resolve(Reply.error(code: failure.code))
      }
    }
  }

  /// Les services et leurs caractéristiques. À appeler après `bluetoothConnect` : c'est ce qui
  /// remplit la table que `bluetoothRead` et `bluetoothWrite` consultent ensuite.
  @objc public func bluetoothServices(_ invoke: Invoke) throws {
    let args = try invoke.parseArgs(BluetoothIdArgs.self)
    BluetoothCentral.shared.services(id: args.id) { result in
      switch result {
      case let .success(services):
        invoke.resolve(BtServicesAnswer(services: services))
      case let .failure(failure):
        invoke.resolve(Reply.error(code: failure.code))
      }
    }
  }

  @objc public func bluetoothRead(_ invoke: Invoke) throws {
    let args = try invoke.parseArgs(BluetoothCharArgs.self)
    BluetoothCentral.shared.read(id: args.id, characteristic: args.characteristic) { result in
      switch result {
      case let .success(data):
        invoke.resolve(
          BtValueAnswer(
            characteristic: args.characteristic.uppercased(),
            data_base64: data.base64EncodedString()
          )
        )
      case let .failure(failure):
        invoke.resolve(Reply.error(code: failure.code))
      }
    }
  }

  @objc public func bluetoothWrite(_ invoke: Invoke) throws {
    let args = try invoke.parseArgs(BluetoothWriteArgs.self)
    // Le pont a déjà refusé un base64 invalide et une charge au-delà de 512 octets ; ici on
    // décode pour de bon, et un décodage impossible reste un refus, jamais un envoi vide.
    guard let data = Data(base64Encoded: args.dataBase64) else {
      invoke.resolve(Reply.error(code: "bad_base64"))
      return
    }
    BluetoothCentral.shared.write(
      id: args.id,
      characteristic: args.characteristic,
      data: data,
      withResponse: args.withResponse
    ) { result in
      switch result {
      case let .success(bytes):
        invoke.resolve(BtWrittenAnswer(bytes: bytes))
      case let .failure(failure):
        invoke.resolve(Reply.error(code: failure.code))
      }
    }
  }

  /// S'abonner aux notifications d'une caractéristique. Les valeurs sont mises en tampon par
  /// le moteur (32 au plus) et retirées par `bluetoothNotifications` : c'est un PULL assumé,
  /// pas un flux poussé — voir l'en-tête de `families/bluetooth.rs`.
  @objc public func bluetoothSubscribe(_ invoke: Invoke) throws {
    let args = try invoke.parseArgs(BluetoothCharArgs.self)
    BluetoothCentral.shared.subscribe(id: args.id, characteristic: args.characteristic) { result in
      switch result {
      case let .success(characteristic):
        invoke.resolve(BtSubscribedAnswer(characteristic: characteristic))
      case let .failure(failure):
        invoke.resolve(Reply.error(code: failure.code))
      }
    }
  }

  @objc public func bluetoothNotifications(_ invoke: Invoke) throws {
    let args = try invoke.parseArgs(BluetoothCharArgs.self)
    let values = BluetoothCentral.shared.notifications(
      id: args.id,
      characteristic: args.characteristic
    )
    invoke.resolve(BtNotificationsAnswer(notifications: values))
  }
}
