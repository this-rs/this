// The Bluetooth engine: one central manager, kept for the life of the app, and the operations
// a connection makes possible (services, read, write, subscribe).
//
// NO TAURI IMPORT, on purpose: this file is type-checked on its own with the iOS SDK
// (`xcrun --sdk iphoneos swiftc -typecheck`). The selectors that parse arguments and resolve
// the invocation live in `Bluetooth.swift`; everything that touches CoreBluetooth lives here.
//
// ## Why one shared manager
//
// The scan creates a manager, scans, and drops it. A connection cannot work that way: the
// peripheral object only stays valid while its delegate (a retained manager) is alive. So the
// engine owns `shared`, and every operation goes through it.
//
// ## Why `retrievePeripherals`
//
// CoreBluetooth never gives a MAC address. A device is named by the UUID the system assigns,
// which is what `bluetoothScan` returned. `retrievePeripherals(withIdentifiers:)` gives back
// the peripheral object for a known identifier without scanning again — and returns nothing
// for an identifier this app has never seen, which is the honest answer.
//
// ## Timeouts
//
// Every operation is bounded. A connection that never completes must answer `timeout`, not
// leave a promise unresolved: the caller is an agent turn, and a hang there is worse than a
// refusal.

import CoreBluetooth
import Foundation

/// A peripheral, as the bridge names it.
struct BtPeer: Encodable {
  let id: String
  let name: String?
  let state: String

  private enum Key: String, CodingKey {
    case id, name, state
  }

  func encode(to encoder: Encoder) throws {
    var c = encoder.container(keyedBy: Key.self)
    try c.encode(id, forKey: .id)
    // `encode` (not `encodeIfPresent`): a peripheral without a name answers `null`, and the
    // Rust side reads that as `None` rather than as a missing field.
    try c.encode(name, forKey: .name)
    try c.encode(state, forKey: .state)
  }
}

/// One characteristic of a service.
struct BtCharacteristicInfo: Encodable {
  let uuid: String
  let properties: [String]
}

/// One service, with its characteristics.
struct BtServiceInfo: Encodable {
  let uuid: String
  let characteristics: [BtCharacteristicInfo]
}

/// One value received on a subscribed characteristic.
struct BtNotification: Encodable {
  let characteristic: String
  let data_base64: String
  let ts_ms: Int64
}

/// What the engine reports: a value, or one of the contract's error codes.
enum BtFailure: Error {
  case code(String)

  var code: String {
    switch self {
    case let .code(value): return value
    }
  }
}

/// The Bluetooth central, shared by every call.
final class BluetoothCentral: NSObject {
  static let shared = BluetoothCentral()

  /// The default bound of an operation that is not a connection.
  static let defaultTimeoutSeconds: Double = 10
  /// How many values the engine keeps per subscribed characteristic.
  static let ringMax = 32

  /// `props` is the `{id}` key of the operation being waited on.
  private struct Op {
    let done: (Result<Any, BtFailure>) -> Void
    let timeoutSeconds: Double
  }

  private var manager: CBCentralManager?
  private var stateWaiters: [(CBCentralManager) -> Void] = []
  private var ops: [String: Op] = [:]
  /// One flat map, keyed `<peripheral uuid>/<characteristic uuid>`: a nested map made the
  /// lookup ambiguous, and the type-checker caught the two levels disagreeing.
  private var characteristics: [String: CBCharacteristic] = [:]
  private var rings: [String: [BtNotification]] = [:]
  private var connected: Set<String> = []

  // ---- The manager -------------------------------------------------------------------

  /// The manager, created on first use. Creating it is what makes iOS ask for the permission.
  private func central() -> CBCentralManager {
    if let manager = manager { return manager }
    let created = CBCentralManager(delegate: self, queue: .main)
    manager = created
    return created
  }

  /// Run `body` once the radio reports `poweredOn`, or answer a failure now.
  private func whenReady(
    timeoutSeconds: Double,
    done: @escaping (Result<Any, BtFailure>) -> Void,
    body: @escaping (CBCentralManager) -> Void
  ) {
    let manager = central()
    switch manager.state {
    case .poweredOn:
      body(manager)
    case .unauthorized:
      done(.failure(.code("not_authorized")))
    case .poweredOff, .unsupported:
      done(.failure(.code("unavailable")))
    case .unknown, .resetting:
      // Transient: the delegate reports the real state next. The timeout covers a stall.
      stateWaiters.append { ready in body(ready) }
      DispatchQueue.main.asyncAfter(deadline: .now() + timeoutSeconds) { [weak self] in
        _ = self
        done(.failure(.code("timeout")))
      }
    @unknown default:
      done(.failure(.code("unavailable")))
    }
  }

  /// The radio's own state, in the contract's words.
  static func name(_ state: CBManagerState) -> String {
    switch state {
    case .poweredOn: return "powered_on"
    case .poweredOff: return "powered_off"
    case .unauthorized: return "unauthorized"
    case .unsupported: return "unsupported"
    case .resetting: return "resetting"
    case .unknown: return "unknown"
    @unknown default: return "unknown"
    }
  }

  /// The permission as iOS records it, read without prompting.
  static func authorization() -> CBManagerAuthorization? {
    if #available(iOS 13.1, *) {
      return CBManager.authorization
    }
    return nil
  }

  func radioState(_ done: @escaping (String) -> Void) {
    let manager = central()
    if manager.state == .unknown {
      stateWaiters.append { ready in done(Self.name(ready.state)) }
      DispatchQueue.main.asyncAfter(deadline: .now() + 5) { [weak self] in
        _ = self
        done("unknown")
      }
      return
    }
    done(Self.name(manager.state))
  }

  // ---- Operations --------------------------------------------------------------------

  private func key(_ op: String, _ peer: String, _ characteristic: String? = nil) -> String {
    if let characteristic = characteristic {
      return "\(op):\(peer):\(characteristic)"
    }
    return "\(op):\(peer)"
  }

  /// Register an operation and arm its timeout.
  private func begin(
    _ name: String,
    timeoutSeconds: Double = BluetoothCentral.defaultTimeoutSeconds,
    done: @escaping (Result<Any, BtFailure>) -> Void
  ) {
    ops[name] = Op(done: done, timeoutSeconds: timeoutSeconds)
    DispatchQueue.main.asyncAfter(deadline: .now() + timeoutSeconds) { [weak self] in
      self?.finish(name, .failure(.code("timeout")))
    }
  }

  /// Answer an operation exactly once, and only if it is still waiting.
  private func finish(_ name: String, _ result: Result<Any, BtFailure>) {
    guard let op = ops.removeValue(forKey: name) else { return }
    op.done(result)
  }

  /// Fail every operation, with the code matching the reason.
  private func failAll(_ code: String) {
    let waiting = ops
    ops.removeAll()
    for (_, op) in waiting {
      op.done(.failure(.code(code)))
    }
  }

  private func peripheral(id: String, _ manager: CBCentralManager) -> CBPeripheral? {
    guard let uuid = UUID(uuidString: id) else { return nil }
    return manager.retrievePeripherals(withIdentifiers: [uuid]).first
  }

  // ---- The operations ------------------------------------------------------------------

  func connect(
    id: String,
    timeoutSeconds: Double,
    done: @escaping (Result<BtPeer, BtFailure>) -> Void
  ) {
    let id = id.uppercased()
    whenReady(timeoutSeconds: timeoutSeconds, done: { result in
      done(result.map { $0 as! BtPeer })
    }) { [weak self] manager in
      guard let self = self else { return }
      guard let peer = self.peripheral(id: id, manager) else {
        // Not scanned on this phone, or forgotten by the system: the honest answer is that
        // this app cannot address it, not that the radio is unavailable.
        done(.failure(.code("not_supported")))
        return
      }
      if peer.state == .connected {
        self.connected.insert(id)
        done(.success(BtPeer(id: id, name: peer.name, state: "connected")))
        return
      }
      let name = self.key("connect", id)
      self.begin(name, timeoutSeconds: timeoutSeconds) { result in
        done(result.map { $0 as! BtPeer })
      }
      peer.delegate = self
      manager.connect(peer, options: nil)
    }
  }

  func disconnect(id: String, done: @escaping (Result<BtPeer, BtFailure>) -> Void) {
    let id = id.uppercased()
    whenReady(timeoutSeconds: Self.defaultTimeoutSeconds, done: { result in
      done(result.map { $0 as! BtPeer })
    }) { [weak self] manager in
      guard let self = self else { return }
      guard let peer = self.peripheral(id: id, manager) else {
        done(.failure(.code("not_supported")))
        return
      }
      let name = self.key("disconnect", id)
      self.begin(name) { result in
        done(result.map { $0 as! BtPeer })
      }
      if peer.state == .disconnected {
        self.connected.remove(id)
        self.finish(name, .success(BtPeer(id: id, name: peer.name, state: "disconnected")))
        return
      }
      manager.cancelPeripheralConnection(peer)
    }
  }

  func services(id: String, done: @escaping (Result<[BtServiceInfo], BtFailure>) -> Void) {
    let id = id.uppercased()
    guard let peer = readyPeer(id: id, done: { result in
      done(result.map { $0 as! [BtServiceInfo] })
    }) else { return }
    guard connected.contains(id) else {
      done(.failure(.code("unavailable")))
      return
    }
    let name = key("services", id)
    begin(name) { result in
      done(result.map { $0 as! [BtServiceInfo] })
    }
    peer.delegate = self
    peer.discoverServices(nil)
  }

  func read(
    id: String,
    characteristic: String,
    done: @escaping (Result<Data, BtFailure>) -> Void
  ) {
    let id = id.uppercased()
    let characteristic = characteristic.uppercased()
    guard let found = withCharacteristic(id: id, characteristic: characteristic, done: { result in
      done(result.map { $0 as! Data })
    })
    else { return }
    let (peer, target) = found
    let name = key("read", id, characteristic)
    begin(name) { result in
      done(result.map { $0 as! Data })
    }
    peer.readValue(for: target)
  }

  func write(
    id: String,
    characteristic: String,
    data: Data,
    withResponse: Bool,
    done: @escaping (Result<Int, BtFailure>) -> Void
  ) {
    let id = id.uppercased()
    let characteristic = characteristic.uppercased()
    guard let found = withCharacteristic(id: id, characteristic: characteristic, done: { result in
      done(result.map { $0 as! Int })
    })
    else { return }
    let (peer, target) = found
    let type: CBCharacteristicWriteType = withResponse ? .withResponse : .withoutResponse
    if !withResponse {
      // There is no callback for a write without response: the queue accepting it is the
      // whole confirmation, and pretending otherwise would answer a value nobody sent.
      peer.writeValue(data, for: target, type: type)
      done(.success(data.count))
      return
    }
    let name = key("write", id, characteristic)
    begin(name) { result in
      done(result.map { $0 as! Int })
    }
    peer.writeValue(data, for: target, type: type)
  }

  func subscribe(
    id: String,
    characteristic: String,
    done: @escaping (Result<String, BtFailure>) -> Void
  ) {
    let id = id.uppercased()
    let characteristic = characteristic.uppercased()
    guard let found = withCharacteristic(id: id, characteristic: characteristic, done: { result in
      done(result.map { $0 as! String })
    })
    else { return }
    let (peer, target) = found
    let name = key("subscribe", id, characteristic)
    begin(name) { result in
      done(result.map { $0 as! String })
    }
    if target.properties.contains(.notify) || target.properties.contains(.indicate) {
      peer.setNotifyValue(true, for: target)
    } else {
      finish(name, .failure(.code("not_supported")))
    }
  }

  /// Drain the buffer of a subscribed characteristic. Empty means "nothing arrived since the
  /// last call", which is an answer, not a failure.
  func notifications(id: String, characteristic: String) -> [BtNotification] {
    let name = key("notify", id.uppercased(), characteristic.uppercased())
    let values = rings[name] ?? []
    rings[name] = []
    return values
  }

  // ---- Helpers -------------------------------------------------------------------------

  private func readyPeer(
    id: String,
    done: @escaping (Result<Any, BtFailure>) -> Void
  ) -> CBPeripheral? {
    var peer: CBPeripheral?
    whenReady(timeoutSeconds: Self.defaultTimeoutSeconds, done: done) { [weak self] manager in
      peer = self?.peripheral(id: id, manager)
    }
    return peer
  }

  private func withCharacteristic(
    id: String,
    characteristic: String,
    done: @escaping (Result<Any, BtFailure>) -> Void
  ) -> (CBPeripheral, CBCharacteristic)? {
    guard let peer = readyPeer(id: id, done: done) else { return nil }
    guard let found = characteristics["\(id)/\(characteristic)"] else {
      // The services of this peripheral have not been read yet, or this characteristic does
      // not exist: `not_supported` says which of the two the caller can fix.
      done(.failure(.code("not_supported")))
      return nil
    }
    return (peer, found)
  }

  private static func properties(_ characteristic: CBCharacteristic) -> [String] {
    var names: [String] = []
    let props = characteristic.properties
    if props.contains(.read) { names.append("read") }
    if props.contains(.write) { names.append("write") }
    if props.contains(.writeWithoutResponse) { names.append("write_without_response") }
    if props.contains(.notify) { names.append("notify") }
    if props.contains(.indicate) { names.append("indicate") }
    return names
  }
}

extension BluetoothCentral: CBCentralManagerDelegate {
  func centralManagerDidUpdateState(_ central: CBCentralManager) {
    let waiters = stateWaiters
    stateWaiters.removeAll()
    switch central.state {
    case .poweredOn:
      for waiter in waiters { waiter(central) }
    case .unauthorized:
      failAll("not_authorized")
    case .poweredOff, .unsupported:
      failAll("unavailable")
    case .unknown, .resetting:
      // Transient: keep the waiters, the timeout bounds them.
      stateWaiters = waiters
    @unknown default:
      failAll("unavailable")
    }
  }

  func centralManager(
    _ central: CBCentralManager,
    didConnect peripheral: CBPeripheral
  ) {
    let id = peripheral.identifier.uuidString
    connected.insert(id)
    peripheral.delegate = self
    finish(key("connect", id), .success(BtPeer(id: id, name: peripheral.name, state: "connected")))
  }

  func centralManager(
    _ central: CBCentralManager,
    didFailToConnect peripheral: CBPeripheral,
    error: Error?
  ) {
    let id = peripheral.identifier.uuidString
    finish(key("connect", id), .failure(.code("unavailable")))
  }

  func centralManager(
    _ central: CBCentralManager,
    didDisconnectPeripheral peripheral: CBPeripheral,
    error: Error?
  ) {
    let id = peripheral.identifier.uuidString
    connected.remove(id)
    // The characteristics are gone with the connection: keeping them would let a read target
    // an object CoreBluetooth has already invalidated.
    for key in characteristics.keys where key.hasPrefix("\(id)/") {
      characteristics.removeValue(forKey: key)
    }
    finish(key("disconnect", id), .success(BtPeer(id: id, name: peripheral.name, state: "disconnected")))
  }
}

extension BluetoothCentral: CBPeripheralDelegate {
  func peripheral(_ peripheral: CBPeripheral, didDiscoverServices error: Error?) {
    let id = peripheral.identifier.uuidString
    if error != nil {
      finish(key("services", id), .failure(.code("internal")))
      return
    }
    let services = peripheral.services ?? []
    if services.isEmpty {
      finish(key("services", id), .success([BtServiceInfo]()))
      return
    }
    for service in services {
      peripheral.discoverCharacteristics(nil, for: service)
    }
  }

  func peripheral(
    _ peripheral: CBPeripheral,
    didDiscoverCharacteristicsFor service: CBService,
    error: Error?
  ) {
    let id = peripheral.identifier.uuidString
    if error != nil {
      finish(key("services", id), .failure(.code("internal")))
      return
    }
    // Answer once, when the last service has been walked.
    let total = peripheral.services?.count ?? 0
    let done = (peripheral.services ?? []).filter { $0.characteristics != nil }.count
    if done < total { return }
    var answer: [BtServiceInfo] = []
    for service in peripheral.services ?? [] {
      var infos: [BtCharacteristicInfo] = []
      for characteristic in service.characteristics ?? [] {
        let uuid = characteristic.uuid.uuidString
        characteristics["\(id)/\(uuid)"] = characteristic
        infos.append(BtCharacteristicInfo(uuid: uuid, properties: Self.properties(characteristic)))
      }
      answer.append(BtServiceInfo(uuid: service.uuid.uuidString, characteristics: infos))
    }
    finish(key("services", id), .success(answer))
  }

  func peripheral(
    _ peripheral: CBPeripheral,
    didUpdateValueFor characteristic: CBCharacteristic,
    error: Error?
  ) {
    let id = peripheral.identifier.uuidString
    let uuid = characteristic.uuid.uuidString
    let data = characteristic.value ?? Data()
    if error != nil {
      finish(key("read", id, uuid), .failure(.code("internal")))
      return
    }
    finish(key("read", id, uuid), .success(data))
    // A value also arrives when nobody is reading: that is a notification.
    let name = key("notify", id, uuid)
    var ring = rings[name] ?? []
    ring.append(
      BtNotification(
        characteristic: uuid,
        data_base64: data.base64EncodedString(),
        ts_ms: Int64(Date().timeIntervalSince1970 * 1000)
      )
    )
    if ring.count > Self.ringMax { ring.removeFirst(ring.count - Self.ringMax) }
    rings[name] = ring
  }

  func peripheral(
    _ peripheral: CBPeripheral,
    didWriteValueFor characteristic: CBCharacteristic,
    error: Error?
  ) {
    let id = peripheral.identifier.uuidString
    let uuid = characteristic.uuid.uuidString
    if error != nil {
      finish(key("write", id, uuid), .failure(.code("internal")))
      return
    }
    finish(key("write", id, uuid), .success(characteristic.value?.count ?? 0))
  }

  func peripheral(
    _ peripheral: CBPeripheral,
    didUpdateNotificationStateFor characteristic: CBCharacteristic,
    error: Error?
  ) {
    let id = peripheral.identifier.uuidString
    let uuid = characteristic.uuid.uuidString
    if error != nil {
      finish(key("subscribe", id, uuid), .failure(.code("not_supported")))
      return
    }
    finish(key("subscribe", id, uuid), .success(uuid))
  }
}
